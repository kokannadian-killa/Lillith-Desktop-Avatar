//! A tiny blocking HTTP/1.0 GET client for loopback probes only.
//!
//! It takes a [`LocalUrl`] (already proven loopback) and connects to its IP
//! directly, so there is no DNS and no way to reach another host. No TLS, no
//! redirects, no proxies, short timeouts, capped response size.

use crate::local::{LocalUrl, LocalUrlError};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Largest body read (Ollama's /api/tags with many models is a few KB).
const MAX_BODY: u64 = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error(transparent)]
    NotLocal(#[from] LocalUrlError),
    #[error("connect {0}: {1}")]
    Connect(String, std::io::Error),
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("bad HTTP response: {0}")]
    Bad(String),
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

/// Largest request body [`post_json`] will send.
pub const MAX_REQUEST_BODY: usize = 256 * 1024;

/// `GET {base}{path}` on loopback with `timeout` for connect and each read.
pub fn get(base: &LocalUrl, path: &str, timeout: Duration) -> Result<Response, HttpError> {
    request(base, "GET", path, None, timeout, timeout)
}

/// `POST {base}{path}` with a JSON body, on loopback only. `connect_timeout`
/// bounds the connect/write; `read_timeout` bounds each read (a CPU model can
/// take a while before the first byte). The body is capped at
/// [`MAX_REQUEST_BODY`], the response at 4 MB.
pub fn post_json(
    base: &LocalUrl,
    path: &str,
    body: &str,
    connect_timeout: Duration,
    read_timeout: Duration,
) -> Result<Response, HttpError> {
    if body.len() > MAX_REQUEST_BODY {
        return Err(HttpError::Bad(format!(
            "request body {} bytes > {MAX_REQUEST_BODY}",
            body.len()
        )));
    }
    request(
        base,
        "POST",
        path,
        Some(body),
        connect_timeout,
        read_timeout,
    )
}

fn request(
    base: &LocalUrl,
    method: &str,
    path: &str,
    body: Option<&str>,
    connect_timeout: Duration,
    read_timeout: Duration,
) -> Result<Response, HttpError> {
    let addr = base.socket_addr();
    let mut s = TcpStream::connect_timeout(&addr, connect_timeout)
        .map_err(|e| HttpError::Connect(addr.to_string(), e))?;
    s.set_read_timeout(Some(read_timeout))?;
    s.set_write_timeout(Some(connect_timeout))?;
    let full_path = format!("{}{}", base.base_path, path);
    let mut req = format!(
        "{method} {full_path} HTTP/1.0\r\nHost: {}:{}\r\nUser-Agent: lillith-setup\r\nAccept: application/json\r\nConnection: close\r\n",
        base.host, base.port
    );
    if let Some(b) = body {
        req.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            b.len()
        ));
    }
    req.push_str("\r\n");
    if let Some(b) = body {
        req.push_str(b);
    }
    s.write_all(req.as_bytes())?;
    let mut raw = Vec::new();
    s.take(MAX_BODY).read_to_end(&mut raw)?;
    parse_response(&raw)
}

fn parse_response(raw: &[u8]) -> Result<Response, HttpError> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| HttpError::Bad("no header terminator".into()))?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let mut lines = head.lines();
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| HttpError::Bad(format!("status line `{status_line}`")))?;
    let chunked = lines.any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("transfer-encoding:") && l.contains("chunked")
    });
    let body = &raw[split + 4..];
    let body = if chunked {
        dechunk(body)
    } else {
        body.to_vec()
    };
    Ok(Response {
        status,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

fn dechunk(mut b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let Some(nl) = b.windows(2).position(|w| w == b"\r\n") else {
            break;
        };
        let size_s = String::from_utf8_lossy(&b[..nl]);
        let size_s = size_s.split(';').next().unwrap_or("").trim();
        let Ok(size) = usize::from_str_radix(size_s, 16) else {
            break;
        };
        b = &b[nl + 2..];
        if size == 0 || b.len() < size {
            out.extend_from_slice(&b[..size.min(b.len())]);
            break;
        }
        out.extend_from_slice(&b[..size]);
        b = b.get(size + 2..).unwrap_or(&[]);
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::net::TcpListener;

    /// Serves `responses` (path -> (status, body)) on a random loopback port.
    pub fn fake_server(responses: Vec<(&'static str, u16, &'static str)>) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in l.incoming() {
                let Ok(mut s) = stream else { continue };
                let req = read_request(&mut s);
                let method = req.split_whitespace().next().unwrap_or("GET").to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                let keyed = format!("{method} {path}");
                let (status, body) = responses
                    .iter()
                    .find(|(p, _, _)| *p == keyed)
                    .or_else(|| responses.iter().find(|(p, _, _)| *p == path))
                    .map(|(_, s, b)| (*s, *b))
                    .unwrap_or((404, "{\"error\":\"not found\"}"));
                let _ = write!(
                    s,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        port
    }

    /// Reads one request (headers + Content-Length body).
    fn read_request(s: &mut TcpStream) -> String {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = s.read(&mut chunk).unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(h) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..h]).to_ascii_lowercase();
                let len = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if buf.len() >= h + 4 + len {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    #[test]
    fn post_json_against_fake_server() {
        let port = fake_server(vec![
            ("POST /echo", 200, "{\"ok\":true}"),
            ("GET /echo", 405, "{}"),
        ]);
        let u = LocalUrl::parse(&format!("127.0.0.1:{port}"), 1).unwrap();
        let t = Duration::from_secs(2);
        let r = post_json(&u, "/echo", "{\"a\":1}", t, t).unwrap();
        assert_eq!((r.status, r.body.as_str()), (200, "{\"ok\":true}"));
        assert_eq!(get(&u, "/echo", t).unwrap().status, 405);
        let big = "x".repeat(MAX_REQUEST_BODY + 1);
        assert!(matches!(
            post_json(&u, "/echo", &big, t, t),
            Err(HttpError::Bad(_))
        ));
    }

    #[test]
    fn get_against_fake_server() {
        let port = fake_server(vec![("/health", 200, "{\"status\":\"ok\"}")]);
        let u = LocalUrl::parse(&format!("http://127.0.0.1:{port}"), 1).unwrap();
        let r = get(&u, "/health", Duration::from_secs(2)).unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body, "{\"status\":\"ok\"}");
        assert_eq!(
            get(&u, "/nope", Duration::from_secs(2)).unwrap().status,
            404
        );
    }

    #[test]
    fn chunked_bodies() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n";
        assert_eq!(parse_response(raw).unwrap().body, "Wikipedia");
    }

    #[test]
    fn closed_port_is_an_error_not_a_panic() {
        // Bind then drop to get a (very likely) closed port.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let u = LocalUrl::parse(&format!("127.0.0.1:{port}"), 1).unwrap();
        assert!(get(&u, "/", Duration::from_millis(300)).is_err());
    }
}
