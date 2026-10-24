//! A small, strict URL type for online trips (§5): `http` / `https` only,
//! no user info, a plain DNS host (no loopback / LAN names or private IP
//! literals, §12), and tracking parameters stripped. No new dependency:
//! the gate builds `--locked` and offline.

use std::fmt;

/// Why a URL was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlError {
    Empty,
    /// Not `http://` / `https://`.
    Scheme,
    /// `user:pass@host` (would carry an identifier).
    UserInfo,
    /// Spaces or control characters.
    Whitespace,
    /// Empty or malformed host (or an IPv6 literal, not supported).
    Host,
    Port,
    /// Loopback, LAN, link-local or `.local` / `.lan` names (§12: a trip
    /// never reaches Ko's own network).
    Local,
}

impl fmt::Display for UrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            UrlError::Empty => "empty address",
            UrlError::Scheme => "only http:// and https:// addresses",
            UrlError::UserInfo => "addresses with a user name or password are refused",
            UrlError::Whitespace => "spaces or control characters in the address",
            UrlError::Host => "bad host name",
            UrlError::Port => "bad port",
            UrlError::Local => "loopback / local-network addresses are refused",
        })
    }
}

/// Query keys that only track (`utm_*` too).
const TRACKING: &[&str] = &[
    "fbclid",
    "gclid",
    "gclsrc",
    "dclid",
    "gbraid",
    "wbraid",
    "msclkid",
    "yclid",
    "igshid",
    "mc_cid",
    "mc_eid",
    "_hsenc",
    "_hsmi",
    "mkt_tok",
    "oly_anon_id",
    "oly_enc_id",
    "vero_id",
    "vero_conv",
    "s_cid",
    "twclid",
    "ttclid",
    "li_fat_id",
    "ref_src",
    "ref_url",
    "spm",
    "_ga",
    "_gl",
    "trk",
    "trkcampaign",
    "si",
];

/// `true` for a tracking query key.
pub fn is_tracking_key(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    k.starts_with("utm_") || TRACKING.contains(&k.as_str())
}

/// A parsed `http(s)` URL.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Url {
    https: bool,
    host: String,
    port: u16,
    /// Path plus `?query` (never a `#fragment`), always starting with `/`.
    path: String,
}

fn local_host(h: &str) -> bool {
    if h == "localhost" || h == "localhost.localdomain" {
        return true;
    }
    if [
        ".localhost",
        ".local",
        ".lan",
        ".home",
        ".internal",
        ".intranet",
        ".corp",
        ".home.arpa",
        ".localdomain",
    ]
    .iter()
    .any(|s| h.ends_with(s))
    {
        return true;
    }
    match ipv4_any(h) {
        // Not an IP literal: an ordinary host name.
        None => false,
        // Numeric-looking but not a valid IPv4 (`1.2.3.4.5`, `0x1g`,
        // `99999999999`): resolvers disagree on those, so refuse.
        Some(Err(())) => true,
        // Refused when local / reserved, and in any non-dotted-decimal
        // spelling (`0x7f000001`, `0177.0.0.1`, `2130706433`, `127.1`):
        // only the canonical form of a public address is let through.
        Some(Ok(ip)) => reserved_v4(ip) || h != ip.to_string(),
    }
}

/// Loopback, private, link-local, CGNAT, documentation, benchmarking,
/// IETF-reserved, class E, broadcast, multicast or "this network".
fn reserved_v4(ip: std::net::Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || o[0] == 0
        || (o[0] == 100 && (64..128).contains(&o[1])) // 100.64.0.0/10
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0.0/24
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19)) // 198.18.0.0/15
        || o[0] >= 240 // 240.0.0.0/4 (and broadcast)
}

/// One IPv4 label in any spelling resolvers accept: decimal, `0x` hex or
/// leading-zero octal. `None` if it isn't numeric at all; `Some(None)` if it
/// is numeric-looking but malformed or too big.
fn ipv4_label(l: &str) -> Option<Option<u64>> {
    let (digits, radix) = if let Some(x) = l.strip_prefix("0x").or_else(|| l.strip_prefix("0X")) {
        (x, 16)
    } else if l.len() > 1 && l.starts_with('0') {
        (&l[1..], 8)
    } else {
        (l, 10)
    };
    if l.is_empty()
        || !l
            .bytes()
            .all(|b| b.is_ascii_hexdigit() || b == b'x' || b == b'X')
    {
        return None;
    }
    if radix != 16 && !l.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if digits.is_empty() {
        return Some(Some(0));
    }
    Some(
        u64::from_str_radix(digits, radix)
            .ok()
            .filter(|&v| v <= u64::from(u32::MAX)),
    )
}

/// `h` read as an IPv4 literal the way resolvers (inet_aton, WHATWG) do:
/// 1–4 dot-separated labels, each decimal / hex / octal, the last filling
/// the remaining bytes. `None` when `h` is a host name (its last label is
/// not numeric).
fn ipv4_any(h: &str) -> Option<Result<std::net::Ipv4Addr, ()>> {
    let labels: Vec<&str> = h.split('.').collect();
    let last = *labels.last()?;
    ipv4_label(last)?;
    if labels.len() > 4 {
        return Some(Err(()));
    }
    let mut vals = Vec::with_capacity(labels.len());
    for l in &labels {
        match ipv4_label(l) {
            Some(Some(v)) => vals.push(v),
            _ => return Some(Err(())),
        }
    }
    let (head, tail) = vals.split_at(vals.len() - 1);
    if head.iter().any(|&v| v > 255) || tail[0] >= 1u64 << (8 * (5 - vals.len())) {
        return Some(Err(()));
    }
    let mut n = tail[0];
    for (i, &v) in head.iter().enumerate() {
        n |= v << (8 * (3 - i));
    }
    Some(Ok(std::net::Ipv4Addr::from(n as u32)))
}

impl Url {
    /// Parses `s` (fragment dropped; host lowercased; default ports).
    pub fn parse(s: &str) -> Result<Url, UrlError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(UrlError::Empty);
        }
        if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(UrlError::Whitespace);
        }
        let (scheme, rest) = s.split_once("://").ok_or(UrlError::Scheme)?;
        let https = match scheme.to_ascii_lowercase().as_str() {
            "https" => true,
            "http" => false,
            _ => return Err(UrlError::Scheme),
        };
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (auth, tail) = rest.split_at(end);
        if auth.contains('@') {
            return Err(UrlError::UserInfo);
        }
        if auth.starts_with('[') {
            return Err(UrlError::Host);
        }
        let (host, port) = match auth.rsplit_once(':') {
            Some((h, p)) => {
                let p: u16 = p.parse().map_err(|_| UrlError::Port)?;
                if p == 0 {
                    return Err(UrlError::Port);
                }
                (h, p)
            }
            None => (auth, if https { 443 } else { 80 }),
        };
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if host.is_empty()
            || host.len() > 253
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
            || host
                .split('.')
                .any(|l| l.is_empty() || l.len() > 63 || l.starts_with('-') || l.ends_with('-'))
        {
            return Err(UrlError::Host);
        }
        if local_host(&host) {
            return Err(UrlError::Local);
        }
        let tail = tail.split('#').next().unwrap_or("");
        let path = if tail.starts_with('/') {
            tail.to_string()
        } else {
            format!("/{tail}")
        };
        Ok(Url {
            https,
            host,
            port,
            path,
        })
    }

    /// An approved-list entry: a full URL, or a bare domain (`example.org`
    /// means `https://example.org/`).
    pub fn parse_entry(s: &str) -> Result<Url, UrlError> {
        let t = s.trim();
        if !t.contains("://") && !t.contains('/') && !t.is_empty() {
            return Url::parse(&format!("https://{t}/"));
        }
        Url::parse(t)
    }

    pub fn is_https(&self) -> bool {
        self.https
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Path and query (`/a/b?x=1`).
    pub fn path_and_query(&self) -> &str {
        &self.path
    }

    fn default_port(&self) -> bool {
        self.port == if self.https { 443 } else { 80 }
    }

    /// The `Host` header value.
    pub fn host_header(&self) -> String {
        if self.default_port() {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// The same URL without tracking query parameters.
    pub fn strip_tracking(&self) -> Url {
        let Some((p, q)) = self.path.split_once('?') else {
            return self.clone();
        };
        let kept: Vec<&str> = q
            .split('&')
            .filter(|kv| !kv.is_empty())
            .filter(|kv| !is_tracking_key(kv.split('=').next().unwrap_or("")))
            .collect();
        let path = if kept.is_empty() {
            p.to_string()
        } else {
            format!("{p}?{}", kept.join("&"))
        };
        Url {
            path,
            ..self.clone()
        }
    }

    /// Resolves a redirect `Location` against this URL.
    pub fn join(&self, location: &str) -> Result<Url, UrlError> {
        let l = location.trim();
        if l.is_empty() {
            return Err(UrlError::Empty);
        }
        if l.contains("://") {
            return Url::parse(l);
        }
        let scheme = if self.https { "https" } else { "http" };
        if let Some(rest) = l.strip_prefix("//") {
            return Url::parse(&format!("{scheme}://{rest}"));
        }
        let origin = format!("{scheme}://{}", self.host_header());
        if l.starts_with('/') {
            return Url::parse(&format!("{origin}{l}"));
        }
        let base = self.path.split('?').next().unwrap_or("/");
        let dir = &base[..base.rfind('/').map(|i| i + 1).unwrap_or(1)];
        Url::parse(&format!("{origin}{dir}{l}"))
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let scheme = if self.https { "https" } else { "http" };
        write!(f, "{scheme}://{}{}", self.host_header(), self.path)
    }
}

/// `%XX` decoded (lossy), `+` as space: what a server would read, for the
/// identifier check.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                        continue;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_display() {
        let u = Url::parse("HTTPS://Docs.Rust-Lang.org/std/index.html?x=1#frag").unwrap();
        assert_eq!(
            u.to_string(),
            "https://docs.rust-lang.org/std/index.html?x=1"
        );
        assert_eq!(u.port(), 443);
        assert_eq!(
            Url::parse("http://example.org:8080").unwrap().to_string(),
            "http://example.org:8080/"
        );
        assert_eq!(Url::parse("ftp://x.org/"), Err(UrlError::Scheme));
        assert_eq!(Url::parse("http://ko@x.org/"), Err(UrlError::UserInfo));
        assert_eq!(Url::parse("http://x.org/a b"), Err(UrlError::Whitespace));
        for local in [
            "http://localhost/",
            "http://127.0.0.1:8080/",
            "http://192.168.1.10/",
            "http://10.0.0.1/",
            "http://169.254.1.1/",
            "http://printer.local/",
            "http://nas.lan/",
            "http://127.1/",
            "http://2130706433/",
        ] {
            assert_eq!(Url::parse(local), Err(UrlError::Local), "{local}");
        }
        // N3: every spelling of a local / reserved IPv4, plus 198.18/15
        // and 240/4; public addresses only in canonical dotted form.
        for local in [
            "http://0x7f000001/",
            "http://0X7F000001/",
            "http://0x7f.1/",
            "http://0177.0.0.1/",
            "http://0177.1/",
            "http://017700000001/",
            "http://3232235777/", // 192.168.1.1
            "http://0xc0a80101/", // 192.168.1.1
            "http://012.0.0.1/",  // 10.0.0.1
            "http://198.18.0.1/",
            "http://198.19.255.254/",
            "http://240.0.0.1/",
            "http://255.255.255.254/",
            "http://0xf0000001/", // 240.0.0.1
            "http://192.0.0.8/",
            "http://224.0.0.1/",
            "http://0x08080808/", // 8.8.8.8, but not canonical
            "http://134744072/",  // 8.8.8.8 as an integer
            "http://1.2.3.4.5/",
            "http://1.2.3.256/",
            "http://0x100000000/",
            "http://08.1.1.1/",
        ] {
            assert_eq!(Url::parse(local), Err(UrlError::Local), "{local}");
        }
        for public in [
            "https://8.8.8.8/",
            "https://198.20.0.1/",
            "https://239.255.255.255.example/",
            "https://example.cafe/",
            "https://0x.example.org/",
            "https://1e100.net/",
        ] {
            assert!(Url::parse(public).is_ok(), "{public}");
        }
        assert_eq!(
            Url::parse_entry("wiki.archlinux.org").unwrap().to_string(),
            "https://wiki.archlinux.org/"
        );
    }

    #[test]
    fn tracking_and_join() {
        let u = Url::parse("https://a.org/p?utm_source=x&id=3&fbclid=y&gclid=z&UTM_Medium=m")
            .unwrap()
            .strip_tracking();
        assert_eq!(u.to_string(), "https://a.org/p?id=3");
        let u = Url::parse("https://a.org/p?utm_source=x")
            .unwrap()
            .strip_tracking();
        assert_eq!(u.to_string(), "https://a.org/p");
        let b = Url::parse("https://a.org/dir/page?q=1").unwrap();
        assert_eq!(
            b.join("other").unwrap().to_string(),
            "https://a.org/dir/other"
        );
        assert_eq!(b.join("/root").unwrap().to_string(), "https://a.org/root");
        assert_eq!(b.join("//b.org/x").unwrap().host(), "b.org");
        assert_eq!(b.join("http://c.org/").unwrap().host(), "c.org");
        assert_eq!(b.join("http://127.0.0.1/"), Err(UrlError::Local));
    }

    #[test]
    fn decode() {
        assert_eq!(percent_decode("a%20b+c%2Fd%zz%"), "a b c/d%zz%");
    }
}
