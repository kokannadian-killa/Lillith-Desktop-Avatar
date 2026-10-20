//! The local-only guarantee.
//!
//! Lillith's LLM must stay on the machine: this crate never talks to
//! anything but loopback. Every URL that reaches the network layer
//! ([`crate::http`]) first goes through [`LocalUrl::parse`], which accepts
//! only `http://` URLs whose host is `127.0.0.0/8`, `::1` or `localhost`.
//! `localhost` is mapped straight to `127.0.0.1` (no DNS lookup, so a
//! tampered `/etc/hosts` or resolver can't redirect it). Config values
//! (`server_url`, `ollama_host`) are validated with the same function when
//! read and when written, so a remote host is refused with a clear error
//! instead of being silently used.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// Why a URL was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LocalUrlError {
    #[error("`{0}` is empty")]
    Empty(String),
    #[error("`{url}`: only plain http:// to this computer is allowed (got scheme `{scheme}`)")]
    Scheme { url: String, scheme: String },
    #[error(
        "`{url}`: host `{host}` is not on this computer. Lillith is local-only: \
         use 127.0.0.1, ::1 or localhost so your chats never leave the machine"
    )]
    NotLoopback { url: String, host: String },
    #[error("`{url}`: user names / passwords in the URL are not allowed")]
    Userinfo { url: String },
    #[error("`{url}`: bad port `{port}`")]
    Port { url: String, port: String },
    #[error("`{url}`: malformed URL")]
    Malformed { url: String },
}

/// A validated loopback HTTP URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalUrl {
    /// The loopback IP to connect to (never resolved through DNS).
    pub ip: IpAddr,
    pub port: u16,
    /// Host as written (`localhost`, `127.0.0.1`, `[::1]`), for the Host header.
    pub host: String,
    /// Path prefix without a trailing `/` (usually empty).
    pub base_path: String,
}

impl LocalUrl {
    /// Parses and validates `url`. A missing scheme means `http://`
    /// (Ollama's `OLLAMA_HOST` style, e.g. `127.0.0.1:11434`); a missing
    /// port means `default_port`.
    pub fn parse(url: &str, default_port: u16) -> Result<Self, LocalUrlError> {
        let raw = url.trim();
        if raw.is_empty() {
            return Err(LocalUrlError::Empty(url.into()));
        }
        let rest = match raw.split_once("://") {
            Some((scheme, rest)) => {
                if !scheme.eq_ignore_ascii_case("http") {
                    return Err(LocalUrlError::Scheme { url: raw.into(), scheme: scheme.into() });
                }
                rest
            }
            None => raw,
        };
        let (authority, path) = match rest.find(['/', '?', '#']) {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        if authority.contains('@') {
            return Err(LocalUrlError::Userinfo { url: raw.into() });
        }
        if authority.is_empty() {
            return Err(LocalUrlError::Malformed { url: raw.into() });
        }
        // Split host / port, handling [v6]:port.
        let (host, port_s) = if let Some(after) = authority.strip_prefix('[') {
            let end = after.find(']').ok_or(LocalUrlError::Malformed { url: raw.into() })?;
            let host = &after[..end];
            let tail = &after[end + 1..];
            let port = match tail {
                "" => None,
                t => Some(t.strip_prefix(':').ok_or(LocalUrlError::Malformed { url: raw.into() })?),
            };
            (host, port)
        } else if authority.matches(':').count() > 1 {
            // Bare IPv6 without brackets (e.g. "::1") — no port possible.
            (authority, None)
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) => (h, Some(p)),
                None => (authority, None),
            }
        };
        let port = match port_s {
            None | Some("") => default_port,
            Some(p) => p
                .parse::<u16>()
                .ok()
                .filter(|p| *p != 0)
                .ok_or(LocalUrlError::Port { url: raw.into(), port: p.into() })?,
        };
        let ip = loopback_ip(host).ok_or(LocalUrlError::NotLoopback {
            url: raw.into(),
            host: host.into(),
        })?;
        let path = path.split(['?', '#']).next().unwrap_or("");
        Ok(Self {
            ip,
            port,
            host: if host.contains(':') { format!("[{host}]") } else { host.to_string() },
            base_path: path.trim_end_matches('/').to_string(),
        })
    }

    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.ip, self.port)
    }

    /// Canonical `http://host:port[/base]` form.
    pub fn base(&self) -> String {
        format!("http://{}:{}{}", self.host, self.port, self.base_path)
    }
}

impl fmt::Display for LocalUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.base())
    }
}

/// The loopback IP for `host`, or `None` if it is not loopback.
/// Names other than `localhost` / `localhost.` / `*.localhost` are never
/// resolved, so they are always refused.
pub fn loopback_ip(host: &str) -> Option<IpAddr> {
    let h = host.trim().trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    if h == "localhost" || h == "localhost." || h.ends_with(".localhost") {
        return Some(IpAddr::V4(Ipv4Addr::LOCALHOST));
    }
    match h.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) if v4.is_loopback() => Some(IpAddr::V4(v4)),
        Ok(IpAddr::V6(v6)) if v6.is_loopback() => Some(IpAddr::V6(Ipv6Addr::LOCALHOST)),
        Ok(IpAddr::V6(v6)) => match v6.to_ipv4_mapped() {
            Some(v4) if v4.is_loopback() => Some(IpAddr::V4(v4)),
            _ => None,
        },
        _ => None,
    }
}

/// Validates without keeping the parsed value.
pub fn ensure_local(url: &str, default_port: u16) -> Result<(), LocalUrlError> {
    LocalUrl::parse(url, default_port).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_loopback_forms() {
        for (u, port) in [
            ("http://127.0.0.1:8080", 8080),
            ("http://localhost:11434", 11434),
            ("127.0.0.1:11434", 11434),
            ("localhost", 11434),
            ("http://[::1]:8080/", 8080),
            ("::1", 11434),
            ("http://127.1.2.3:9000/v1", 9000),
            ("HTTP://LOCALHOST:8080", 8080),
            ("http://[::ffff:127.0.0.1]:8080", 8080),
        ] {
            let l = LocalUrl::parse(u, 11434).unwrap_or_else(|e| panic!("{u}: {e}"));
            assert_eq!(l.port, port, "{u}");
            assert!(l.ip.is_loopback(), "{u}");
        }
        assert_eq!(LocalUrl::parse("http://localhost:8080/v1/", 1).unwrap().base_path, "/v1");
    }

    #[test]
    fn rejects_remote_hosts() {
        for u in [
            "http://192.168.1.20:11434",
            "http://10.0.0.5:8080",
            "http://0.0.0.0:8080",
            "http://example.com:8080",
            "api.openai.com",
            "http://8.8.8.8",
            "http://[2001:db8::1]:8080",
            "http://my-desktop.lan:11434",
            "http://localhost.evil.com:8080",
            "http://127.0.0.1.nip.io:8080",
        ] {
            match LocalUrl::parse(u, 8080) {
                Err(LocalUrlError::NotLoopback { .. }) => {}
                other => panic!("{u}: expected NotLoopback, got {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_tls_userinfo_and_junk() {
        assert!(matches!(
            LocalUrl::parse("https://127.0.0.1:8080", 1),
            Err(LocalUrlError::Scheme { .. })
        ));
        assert!(matches!(
            LocalUrl::parse("ftp://localhost", 1),
            Err(LocalUrlError::Scheme { .. })
        ));
        assert!(matches!(
            LocalUrl::parse("http://evil.com@127.0.0.1:8080", 1),
            Err(LocalUrlError::Userinfo { .. })
        ));
        assert!(matches!(LocalUrl::parse("", 1), Err(LocalUrlError::Empty(_))));
        assert!(matches!(
            LocalUrl::parse("http://127.0.0.1:99999", 1),
            Err(LocalUrlError::Port { .. })
        ));
        let msg = LocalUrl::parse("http://example.com", 1).unwrap_err().to_string();
        assert!(msg.contains("local-only"), "{msg}");
    }
}
