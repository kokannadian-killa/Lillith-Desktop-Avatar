//! The network side of a trip, behind [`Fetcher`] so the engine is tested
//! with no socket at all.
//!
//! [`TorFetcher`] is the only real transport: one HTTP/1.1 GET written by
//! hand over `std::net::TcpStream` to the **local Tor SOCKS5 proxy**
//! (`[learn] tor_socks`, loopback only), with the host name sent to Tor
//! (`socks5h`: no local DNS lookup ever happens) and the trip id as the
//! SOCKS user name and password, so Tor (`IsolateSOCKSAuth`, on by default)
//! gives every trip its own circuit. One request per connection
//! (`Connection: close`); nothing persists between requests: no cookies,
//! cache, `Referer`, auth, `ETag` / `If-Modified-Since`, no JavaScript, no
//! TLS session resumption. Headers are a fixed, generic Tor-Browser-like
//! set pinned below.
//!
//! `https://` runs rustls (ring provider, TLS 1.2 + 1.3) over that same
//! SOCKS stream, SNI = the target host name, verified against the Mozilla
//! roots compiled in from `webpki-roots` (nothing is downloaded at run
//! time, no OpenSSL, no system store). Plain `http://` is refused by the
//! engine unless `[learn] allow_http = true` (an exit node can read and
//! change plaintext).
//!
//! Every socket read and write waits at most [`POLL`] and then checks the
//! trip's `stop()` (safeword, End early, the trip clock), so a trip stops
//! within about a second even in the middle of a slow body.

use super::url::Url;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Tor Browser 14 (ESR 128) on Windows: the most common Tor Browser
/// fingerprint. Pinned; changed only by a normal code change.
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; rv:128.0) Gecko/20100101 Firefox/128.0";
/// Tor Browser's `Accept` for a top-level page.
pub const ACCEPT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
/// Generic `en-US` only (§12: no locale beyond en-US).
pub const ACCEPT_LANGUAGE: &str = "en-US,en;q=0.5";
/// Uncompressed bodies only, so the byte cap counts what is stored. (Tor
/// Browser sends `gzip, deflate, br, zstd`; this one header differs.)
pub const ACCEPT_ENCODING: &str = "identity";

/// Largest response head read.
pub const MAX_HEAD_BYTES: usize = 32 * 1024;
/// A request with no progress for this long fails.
pub const IO_TIMEOUT: Duration = Duration::from_secs(20);
/// Longest single socket wait before `stop()` is checked again.
pub const POLL: Duration = Duration::from_millis(500);

/// One GET's answer (redirects are NOT followed: the engine checks each hop
/// against the approved list before it is requested).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedPage {
    /// The URL that was requested.
    pub url: Url,
    pub status: u16,
    /// The raw `Location` header of a 3xx.
    pub location: Option<String>,
    /// The `Content-Type` header (empty when missing).
    pub content_type: String,
    /// The body, at most the requested cap (empty for a 3xx).
    pub body: Vec<u8>,
    /// More body was there than the cap allowed (not read).
    pub truncated: bool,
}

/// Why a GET failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The Tor SOCKS port is not answering (or not loopback).
    TorUnavailable(String),
    /// The TLS handshake or certificate check failed.
    Tls(String),
    /// `stop()` turned true (safeword, End early, the trip clock).
    Stopped,
    /// The SOCKS handshake failed.
    Socks(String),
    Io(String),
    /// A malformed HTTP answer.
    Http(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::TorUnavailable(w) => write!(f, "Tor is not available: {w}"),
            FetchError::Tls(w) => write!(f, "TLS: {w}"),
            FetchError::Stopped => f.write_str("stopped"),
            FetchError::Socks(w) => write!(f, "Tor SOCKS: {w}"),
            FetchError::Io(w) => write!(f, "network: {w}"),
            FetchError::Http(w) => write!(f, "bad HTTP answer: {w}"),
        }
    }
}

impl From<std::io::Error> for FetchError {
    fn from(e: std::io::Error) -> Self {
        if e.get_ref().is_some_and(|i| i.is::<StopMark>()) {
            return FetchError::Stopped;
        }
        if let Some(t) = e.get_ref().and_then(|i| i.downcast_ref::<rustls::Error>()) {
            return FetchError::Tls(t.to_string());
        }
        FetchError::Io(e.to_string())
    }
}

/// The io error [`Stoppable`] returns once `stop()` is true.
#[derive(Debug)]
struct StopMark;

impl std::fmt::Display for StopMark {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("stopped")
    }
}

impl std::error::Error for StopMark {}

fn stopped() -> std::io::Error {
    std::io::Error::other(StopMark)
}

/// A stream whose reads and writes give up when `stop()` turns true. The
/// inner stream must time out after at most [`POLL`] (a `TcpStream` with
/// `set_read_timeout(Some(POLL))`); each timeout re-checks `stop()`, and
/// `idle` without any progress fails the call.
pub struct Stoppable<'a, S> {
    inner: S,
    stop: &'a dyn Fn() -> bool,
    idle: Duration,
}

impl<'a, S> Stoppable<'a, S> {
    pub fn new(inner: S, stop: &'a dyn Fn() -> bool, idle: Duration) -> Self {
        Self { inner, stop, idle }
    }

    pub fn get_ref(&self) -> &S {
        &self.inner
    }

    fn retry<T>(&mut self, mut f: impl FnMut(&mut S) -> std::io::Result<T>) -> std::io::Result<T> {
        let start = Instant::now();
        loop {
            if (self.stop)() {
                return Err(stopped());
            }
            match f(&mut self.inner) {
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    if start.elapsed() >= self.idle {
                        return Err(std::io::Error::new(
                            ErrorKind::TimedOut,
                            format!("no answer for {:?}", self.idle),
                        ));
                    }
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                r => return r,
            }
        }
    }
}

impl<S: Read> Read for Stoppable<'_, S> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.retry(|s| s.read(buf))
    }
}

impl<S: Write> Write for Stoppable<'_, S> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.retry(|s| s.write(buf))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.retry(|s| s.flush())
    }
}

/// The transport of a trip.
pub trait Fetcher {
    /// `Ok` when trips may run at all (Tor answering).
    fn available(&self) -> Result<(), String>;
    /// One read-only GET of `url` on `trip_id`'s circuit, reading at most
    /// `max_bytes` of body. Gives up with [`FetchError::Stopped`] within
    /// about a second of `stop()` turning true.
    fn get(
        &self,
        url: &Url,
        trip_id: &str,
        max_bytes: u64,
        stop: &dyn Fn() -> bool,
    ) -> Result<FetchedPage, FetchError>;
}

/// The Tor SOCKS address, refused unless it is a loopback `ip:port`
/// (traffic must never go to a proxy on another machine).
pub fn socks_addr(tor_socks: &str) -> Result<SocketAddr, String> {
    let a: SocketAddr = tor_socks
        .trim()
        .parse()
        .map_err(|_| format!("tor_socks {tor_socks:?} is not an ip:port"))?;
    if !a.ip().is_loopback() {
        return Err(format!(
            "tor_socks {a} is not a loopback address; only a local Tor is used"
        ));
    }
    Ok(a)
}

/// The exact request bytes of a GET of `url` (pinned headers, nothing
/// per-user, no cookies / Referer / auth / revalidation).
pub fn request_bytes(url: &Url) -> Vec<u8> {
    format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {USER_AGENT}\r\nAccept: {ACCEPT}\r\n\
         Accept-Language: {ACCEPT_LANGUAGE}\r\nAccept-Encoding: {ACCEPT_ENCODING}\r\n\
         Connection: close\r\n\r\n",
        url.path_and_query(),
        url.host_header()
    )
    .into_bytes()
}

/// SOCKS5 handshake on `s`: user/password auth only (`user` = `pass` =
/// the trip id, so Tor isolates the circuit), then CONNECT by host name
/// (ATYP 3: Tor resolves it, never the local resolver).
pub fn socks5_connect<S: Read + Write>(
    s: &mut S,
    host: &str,
    port: u16,
    trip_id: &str,
) -> Result<(), FetchError> {
    let user = trip_id.as_bytes();
    if user.is_empty() || user.len() > 255 || host.is_empty() || host.len() > 255 {
        return Err(FetchError::Socks("bad trip id or host length".into()));
    }
    s.write_all(&[0x05, 0x01, 0x02])?;
    let mut m = [0u8; 2];
    s.read_exact(&mut m)?;
    if m[0] != 0x05 {
        return Err(FetchError::Socks("not a SOCKS5 proxy".into()));
    }
    if m[1] != 0x02 {
        return Err(FetchError::Socks(
            "the proxy refused per-trip credentials (no circuit isolation)".into(),
        ));
    }
    let mut auth = vec![0x01, user.len() as u8];
    auth.extend_from_slice(user);
    auth.push(user.len() as u8);
    auth.extend_from_slice(user);
    s.write_all(&auth)?;
    s.read_exact(&mut m)?;
    if m[1] != 0x00 {
        return Err(FetchError::Socks("credentials refused".into()));
    }
    let mut req = vec![0x05, 0x01, 0x00, 0x03, host.len() as u8];
    req.extend_from_slice(host.as_bytes());
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req)?;
    let mut head = [0u8; 4];
    s.read_exact(&mut head)?;
    if head[0] != 0x05 {
        return Err(FetchError::Socks("bad reply".into()));
    }
    if head[1] != 0x00 {
        let why = match head[1] {
            0x01 => "general failure",
            0x02 => "not allowed",
            0x03 => "network unreachable",
            0x04 => "host unreachable",
            0x05 => "connection refused",
            0x06 => "TTL expired",
            0x07 => "command not supported",
            0x08 => "address type not supported",
            _ => "unknown error",
        };
        return Err(FetchError::Socks(why.into()));
    }
    let skip = match head[3] {
        0x01 => 4,
        0x04 => 16,
        0x03 => {
            let mut l = [0u8; 1];
            s.read_exact(&mut l)?;
            usize::from(l[0])
        }
        _ => return Err(FetchError::Socks("bad bound address".into())),
    };
    let mut rest = vec![0u8; skip + 2];
    s.read_exact(&mut rest)?;
    Ok(())
}

fn read_line<R: BufRead>(r: &mut R, budget: &mut usize) -> Result<String, FetchError> {
    let mut buf = Vec::new();
    let n = r
        .by_ref()
        .take((*budget as u64) + 1)
        .read_until(b'\n', &mut buf)?;
    if n > *budget {
        return Err(FetchError::Http("response head too large".into()));
    }
    *budget -= n;
    if n == 0 {
        return Err(FetchError::Http("connection closed early".into()));
    }
    Ok(String::from_utf8_lossy(&buf)
        .trim_end_matches(['\r', '\n'])
        .to_string())
}

/// Reads up to `cap` more body bytes into `body`; `true` when there was more.
fn read_capped<R: Read>(
    r: &mut R,
    body: &mut Vec<u8>,
    n: Option<u64>,
    cap: u64,
) -> std::io::Result<bool> {
    let room = cap.saturating_sub(body.len() as u64);
    let want = n.map_or(room + 1, |n| n.min(room + 1));
    let before = body.len();
    match r.by_ref().take(want).read_to_end(body) {
        // A TLS peer that closes without close_notify: what arrived counts
        // (the length checks below still apply).
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => {}
        r => {
            r?;
        }
    }
    let got = (body.len() - before) as u64;
    if got > room {
        body.truncate(before + room as usize);
        return Ok(true);
    }
    Ok(n.is_some_and(|n| n > room))
}

/// Parses one HTTP/1.x response from `r`, reading at most `max_bytes` of
/// body (chunked or `Content-Length` or to EOF). A 3xx body is not read.
pub fn read_response<R: Read>(r: R, url: &Url, max_bytes: u64) -> Result<FetchedPage, FetchError> {
    let mut r = BufReader::new(r);
    let mut budget = MAX_HEAD_BYTES;
    let status_line = read_line(&mut r, &mut budget)?;
    let mut parts = status_line.split_whitespace();
    let ver = parts.next().unwrap_or("");
    if !ver.starts_with("HTTP/1.") {
        return Err(FetchError::Http(format!("status line {status_line:?}")));
    }
    let status: u16 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .filter(|s| (100..600).contains(s))
        .ok_or_else(|| FetchError::Http("bad status".into()))?;
    let (mut ct, mut location, mut len, mut chunked) = (String::new(), None, None, false);
    loop {
        let l = read_line(&mut r, &mut budget)?;
        if l.is_empty() {
            break;
        }
        let Some((k, v)) = l.split_once(':') else {
            continue;
        };
        let v = v.trim();
        match k.trim().to_ascii_lowercase().as_str() {
            "content-type" => ct = v.to_string(),
            "location" => location = Some(v.to_string()),
            "content-length" => len = v.parse::<u64>().ok(),
            "transfer-encoding" => chunked = v.to_ascii_lowercase().contains("chunked"),
            // Set-Cookie, ETag, … are read and dropped: nothing persists.
            _ => {}
        }
    }
    let mut body = Vec::new();
    let mut truncated = false;
    if !(300..400).contains(&status) {
        if chunked {
            loop {
                let mut b = 1024;
                let size_line = read_line(&mut r, &mut b)?;
                let hex = size_line.split(';').next().unwrap_or("").trim();
                let size = u64::from_str_radix(hex, 16)
                    .map_err(|_| FetchError::Http("bad chunk size".into()))?;
                if size == 0 {
                    break;
                }
                if read_capped(&mut r, &mut body, Some(size), max_bytes)? {
                    truncated = true;
                    break;
                }
                let mut b = 8;
                let _ = read_line(&mut r, &mut b)?;
            }
        } else {
            truncated = read_capped(&mut r, &mut body, len, max_bytes)?;
        }
    }
    Ok(FetchedPage {
        url: url.clone(),
        status,
        location,
        content_type: ct,
        body,
        truncated,
    })
}

/// The Mozilla roots compiled into the binary (`webpki-roots`).
pub fn bundled_roots() -> rustls::RootCertStore {
    rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned())
}

/// The client config of one trip: ring, safe protocol versions, the given
/// roots, no client auth, no session resumption (nothing links two
/// requests), no ALPN (HTTP/1.1 only).
pub fn tls_config(
    roots: Arc<rustls::RootCertStore>,
) -> Result<Arc<rustls::ClientConfig>, FetchError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut c = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| FetchError::Tls(e.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    c.resumption = rustls::client::Resumption::disabled();
    c.enable_sni = true;
    Ok(Arc::new(c))
}

/// Writes the GET to `s` and reads the answer.
fn exchange<S: Read + Write>(
    s: &mut S,
    url: &Url,
    max_bytes: u64,
) -> Result<FetchedPage, FetchError> {
    s.write_all(&request_bytes(url))?;
    s.flush()?;
    read_response(s, url, max_bytes)
}

/// The real transport: Tor SOCKS5h on loopback, HTTP/1.1 GET, https via
/// rustls over the SOCKS stream.
#[derive(Debug, Clone)]
pub struct TorFetcher {
    socks: String,
    timeout: Duration,
    roots: Arc<rustls::RootCertStore>,
}

impl TorFetcher {
    pub fn new(tor_socks: &str) -> Self {
        Self {
            socks: tor_socks.to_string(),
            timeout: IO_TIMEOUT,
            roots: Arc::new(bundled_roots()),
        }
    }

    /// `[learn] tor_socks`.
    pub fn from_config(cfg: &lillith_core::config::LearnConfig) -> Self {
        Self::new(&cfg.tor_socks)
    }

    /// Trust only `roots` instead of the bundled Mozilla set (tests, pinning).
    pub fn with_roots(mut self, roots: rustls::RootCertStore) -> Self {
        self.roots = Arc::new(roots);
        self
    }

    /// No progress for this long fails a request (default [`IO_TIMEOUT`]).
    pub fn with_timeout(mut self, d: Duration) -> Self {
        self.timeout = d;
        self
    }

    fn get_inner(
        &self,
        url: &Url,
        trip_id: &str,
        max_bytes: u64,
        stop: &dyn Fn() -> bool,
    ) -> Result<FetchedPage, FetchError> {
        let a = socks_addr(&self.socks).map_err(FetchError::TorUnavailable)?;
        if stop() {
            return Err(FetchError::Stopped);
        }
        let tcp = TcpStream::connect_timeout(&a, self.timeout.min(Duration::from_secs(5)))
            .map_err(|e| FetchError::TorUnavailable(e.to_string()))?;
        tcp.set_read_timeout(Some(POLL))?;
        tcp.set_write_timeout(Some(POLL))?;
        tcp.set_nodelay(true)?;
        let mut s = Stoppable::new(tcp, stop, self.timeout);
        socks5_connect(&mut s, url.host(), url.port(), trip_id)?;
        if url.is_https() {
            let name = rustls::pki_types::ServerName::try_from(url.host().to_string())
                .map_err(|e| FetchError::Tls(format!("server name: {e}")))?;
            let conn = rustls::ClientConnection::new(tls_config(self.roots.clone())?, name)
                .map_err(|e| FetchError::Tls(e.to_string()))?;
            let mut tls = rustls::StreamOwned::new(conn, s);
            let page = exchange(&mut tls, url, max_bytes);
            let _ = tls.sock.get_ref().shutdown(std::net::Shutdown::Both);
            page
        } else {
            let page = exchange(&mut s, url, max_bytes);
            let _ = s.get_ref().shutdown(std::net::Shutdown::Both);
            page
        }
    }
}

impl Fetcher for TorFetcher {
    /// Tor must already be running: a TCP connect to the loopback SOCKS
    /// port (closed straight away). Tor is never installed or started here.
    fn available(&self) -> Result<(), String> {
        let a = socks_addr(&self.socks)?;
        TcpStream::connect_timeout(&a, Duration::from_secs(2))
            .map(drop)
            .map_err(|e| format!("nothing answers on the Tor SOCKS port {a} ({e})"))
    }

    fn get(
        &self,
        url: &Url,
        trip_id: &str,
        max_bytes: u64,
        stop: &dyn Fn() -> bool,
    ) -> Result<FetchedPage, FetchError> {
        let r = self.get_inner(url, trip_id, max_bytes, stop);
        // Whatever the inner layers made of it: a stop is a stop.
        if r.is_err() && stop() {
            return Err(FetchError::Stopped);
        }
        r
    }
}
