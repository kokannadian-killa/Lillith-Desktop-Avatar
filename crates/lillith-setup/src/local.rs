//! The local-only guarantee.
//!
//! Lillith's LLM must stay on the machine: this crate never talks to
//! anything but loopback. Every URL that reaches the network layer
//! ([`crate::http`]) first goes through [`LocalUrl::parse`]. Config values
//! (`server_url`, `ollama_host`) are validated with the same function when
//! read and when written, so a remote host is refused with a clear error
//! instead of being silently used, and they are *stored* in the canonical
//! IP-literal form ([`LocalUrl::base`]) so any other component that reads
//! the config never has to resolve a name either.
//!
//! ## Accepted forms (deliberately narrow)
//!
//! `[http://]HOST[:PORT][/path]` where
//!
//! * the scheme, if present, is `http` (any case). `https`, `ws`, `ftp`, ...
//!   are refused: the server is on this computer, TLS buys nothing and an
//!   `https` URL would hint at a remote service.
//! * `HOST` is exactly one of
//!   * `localhost` (ASCII, any case) — mapped straight to `127.0.0.1`, never
//!     looked up, so a tampered `/etc/hosts` or resolver can't redirect it.
//!     `localhost.` (trailing dot), `*.localhost`, `localhost.evil.com` are
//!     refused (they are names a resolver would have to answer).
//!   * a strict dotted-quad IPv4 address in `127.0.0.0/8`
//!     (`127.0.0.1`, `127.1.2.3`). The legacy `inet_aton` shorthands that some
//!     parsers accept — `127.1`, `0x7f.0.0.1`, `0177.0.0.1`, `2130706433`,
//!     leading zeros — are refused, because different libraries disagree on
//!     what they mean.
//!   * `[::1]` (brackets required). Bare `::1`, IPv4-mapped / compatible IPv6
//!     (`[::ffff:127.0.0.1]`, `[::127.0.0.1]`), NAT64 and zone ids
//!     (`[::1%lo]`) are refused: they are ambiguous across stacks
//!     (`IPV6_V6ONLY`, OS differences) and nobody needs them.
//! * `PORT`, if the `:` is present, is 1–5 ASCII digits in `1..=65535`
//!   (`:`, `:0`, `:+80`, `:99999` are refused). No `:` means `default_port`.
//! * no user info (`@` anywhere is refused), no query or fragment (`?`, `#`),
//!   no backslashes, whitespace, control characters or non-ASCII anywhere
//!   (so there is nothing for an IDNA / WHATWG / lax parser to interpret
//!   differently). Leading / trailing whitespace is trimmed.
//! * `0.0.0.0`, private LAN ranges, public addresses and every other name
//!   are refused — no DNS lookup is ever made to "check" a name.

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
         use 127.0.0.1, [::1] or localhost so your chats never leave the machine"
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
    /// Parses and validates `url` (rules in the [module docs](self)). A
    /// missing scheme means `http://` (Ollama's `OLLAMA_HOST` style, e.g.
    /// `127.0.0.1:11434`); a missing port means `default_port`.
    pub fn parse(url: &str, default_port: u16) -> Result<Self, LocalUrlError> {
        let raw = url.trim();
        let malformed = || LocalUrlError::Malformed { url: raw.into() };
        if raw.is_empty() {
            return Err(LocalUrlError::Empty(url.into()));
        }
        if raw.chars().any(|c| !c.is_ascii() || c.is_ascii_control() || c.is_ascii_whitespace() || c == '\\') {
            return Err(malformed());
        }
        if raw.contains('@') {
            return Err(LocalUrlError::Userinfo { url: raw.into() });
        }
        if raw.contains(['?', '#']) {
            return Err(malformed());
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
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        if authority.is_empty() {
            return Err(malformed());
        }
        // Split host / port, handling [v6]:port.
        let (host, port_s, bracketed) = if let Some(after) = authority.strip_prefix('[') {
            let end = after.find(']').ok_or_else(malformed)?;
            let host = &after[..end];
            let port = match &after[end + 1..] {
                "" => None,
                t => Some(t.strip_prefix(':').ok_or_else(malformed)?),
            };
            (host, port, true)
        } else if authority.contains(']') {
            return Err(malformed());
        } else if authority.matches(':').count() > 1 {
            // Bare IPv6 without brackets: refused (ambiguous with a port).
            return Err(LocalUrlError::NotLoopback { url: raw.into(), host: authority.into() });
        } else {
            match authority.split_once(':') {
                Some((h, p)) => (h, Some(p), false),
                None => (authority, None, false),
            }
        };
        let port = match port_s {
            None => default_port,
            Some(p) => parse_port(p).ok_or(LocalUrlError::Port { url: raw.into(), port: p.into() })?,
        };
        let not_loopback = || LocalUrlError::NotLoopback { url: raw.into(), host: host.into() };
        let ip = if bracketed { loopback_v6(host) } else { loopback_ip(host) }.ok_or_else(not_loopback)?;
        if !bracketed && ip.is_ipv6() {
            return Err(not_loopback());
        }
        Ok(Self {
            ip,
            port,
            host: if bracketed { format!("[{host}]") } else { host.to_string() },
            base_path: path.trim_end_matches('/').to_string(),
        })
    }

    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.ip, self.port)
    }

    /// Canonical `http://IP:port[/base]` form, with the IP literal (never a
    /// name), e.g. `http://127.0.0.1:8080` or `http://[::1]:11434/v1`. This
    /// is what gets stored in the config, so no reader has to resolve it.
    pub fn base(&self) -> String {
        format!("http://{}{}", self.socket_addr(), self.base_path)
    }

    /// `IP:port` (`[::1]:11434` for IPv6), e.g. for `OLLAMA_HOST`.
    pub fn host_port(&self) -> String {
        self.socket_addr().to_string()
    }
}

impl fmt::Display for LocalUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.base())
    }
}

/// `1..=65535`, 1–5 ASCII digits only (no sign, no spaces).
fn parse_port(p: &str) -> Option<u16> {
    if p.is_empty() || p.len() > 5 || !p.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    p.parse::<u16>().ok().filter(|p| *p != 0)
}

/// `::1` only (the text between `[` and `]`).
fn loopback_v6(h: &str) -> Option<IpAddr> {
    match h.parse::<Ipv6Addr>() {
        Ok(v6) if v6 == Ipv6Addr::LOCALHOST => Some(IpAddr::V6(v6)),
        _ => None,
    }
}

/// The loopback IP for an unbracketed `host`, or `None` if it is not one of
/// the accepted forms: literal `localhost` (any ASCII case) or a strict
/// dotted-quad in `127.0.0.0/8`. A bracketed `[::1]` is also accepted here
/// for convenience. Names are never resolved, so every other name is refused.
pub fn loopback_ip(host: &str) -> Option<IpAddr> {
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        return loopback_v6(inner);
    }
    if host.eq_ignore_ascii_case("localhost") {
        return Some(IpAddr::V4(Ipv4Addr::LOCALHOST));
    }
    // std's parser is strict: exactly four decimal octets, no leading zeros,
    // no hex/octal/shorthand forms.
    match host.parse::<Ipv4Addr>() {
        Ok(v4) if v4.is_loopback() => Some(IpAddr::V4(v4)),
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
        for (u, port, base) in [
            ("http://127.0.0.1:8080", 8080, "http://127.0.0.1:8080"),
            ("http://localhost:11434", 11434, "http://127.0.0.1:11434"),
            ("127.0.0.1:11434", 11434, "http://127.0.0.1:11434"),
            ("localhost", 11434, "http://127.0.0.1:11434"),
            ("LOCALHOST", 11434, "http://127.0.0.1:11434"),
            ("LocalHost:9", 9, "http://127.0.0.1:9"),
            ("http://[::1]:8080/", 8080, "http://[::1]:8080"),
            ("[::1]", 11434, "http://[::1]:11434"),
            ("[0:0:0:0:0:0:0:1]:1", 1, "http://[::1]:1"),
            ("http://127.1.2.3:9000/v1", 9000, "http://127.1.2.3:9000/v1"),
            ("http://127.255.255.254:65535", 65535, "http://127.255.255.254:65535"),
            ("HTTP://LOCALHOST:8080", 8080, "http://127.0.0.1:8080"),
            ("  http://127.0.0.1:8080/  ", 8080, "http://127.0.0.1:8080"),
            ("\thttp://localhost\n", 11434, "http://127.0.0.1:11434"),
            ("http://127.0.0.1:08080", 8080, "http://127.0.0.1:8080"),
            ("http://127.0.0.1:8080/v1/%7Elily", 8080, "http://127.0.0.1:8080/v1/%7Elily"),
        ] {
            let l = LocalUrl::parse(u, 11434).unwrap_or_else(|e| panic!("{u:?}: {e}"));
            assert_eq!(l.port, port, "{u:?}");
            assert!(l.ip.is_loopback(), "{u:?}");
            assert_eq!(l.base(), base, "{u:?}");
            // The canonical form re-parses to the same address.
            let again = LocalUrl::parse(&l.base(), 1).unwrap();
            assert_eq!((again.ip, again.port, again.base()), (l.ip, l.port, l.base()), "{u:?}");
        }
        assert_eq!(LocalUrl::parse("http://localhost:8080/v1/", 1).unwrap().base_path, "/v1");
        assert_eq!(LocalUrl::parse("[::1]:11434", 1).unwrap().host_port(), "[::1]:11434");
        assert_eq!(LocalUrl::parse("localhost", 11434).unwrap().host_port(), "127.0.0.1:11434");
    }

    fn not_loopback(u: &str) {
        match LocalUrl::parse(u, 8080) {
            Err(LocalUrlError::NotLoopback { .. }) => {}
            other => panic!("{u:?}: expected NotLoopback, got {other:?}"),
        }
    }

    fn refused(u: &str) {
        let r = LocalUrl::parse(u, 8080);
        assert!(r.is_err(), "{u:?} must be refused, got {r:?}");
    }

    #[test]
    fn rejects_remote_hosts() {
        for u in [
            "http://192.168.1.20:11434",
            "192.168.0.1",
            "http://10.0.0.5:8080",
            "http://172.16.0.1",
            "http://169.254.169.254",
            "http://0.0.0.0:8080",
            "0.0.0.0",
            "http://example.com:8080",
            "api.openai.com",
            "http://8.8.8.8",
            "http://[2001:db8::1]:8080",
            "http://[::]:8080",
            "http://my-desktop.lan:11434",
            "http://localhost.evil.com:8080",
            "http://127.0.0.1.nip.io:8080",
            "http://127.0.0.1.:8080",
            "http://localhost.:8080",
            "localhost.",
            "http://foo.localhost:8080",
            "http://local%68ost:8080",
            "http://%31%32%37.0.0.1:8080",
            "http://127%2e0%2e0%2e1:8080",
            "localhostx",
        ] {
            not_loopback(u);
        }
    }

    #[test]
    fn rejects_ambiguous_numeric_ipv4_forms() {
        // inet_aton shorthands: some resolvers map these to 127.0.0.1, others
        // to something else or nothing. Refused deliberately.
        for u in [
            "http://127.1:8080",
            "http://127.0.1:8080",
            "http://0x7f.0.0.1:8080",
            "http://0x7f000001:8080",
            "http://0177.0.0.1:8080",
            "http://127.000.000.001:8080",
            "http://127.0.0.01:8080",
            "http://2130706433:8080",
            "2130706433",
            "http://017700000001",
            "http://127.0.0.1.1",
            "http://127.0.0.256",
        ] {
            not_loopback(u);
        }
    }

    #[test]
    fn rejects_mapped_compat_zoned_and_bare_ipv6() {
        for u in [
            "http://[::ffff:127.0.0.1]:8080",
            "http://[::ffff:7f00:1]:8080",
            "http://[::127.0.0.1]:8080",
            "http://[64:ff9b::7f00:1]:8080",
            "http://[::1%25lo]:8080",
            "http://[::1%lo]:8080",
            "http://[fe80::1%25eth0]:8080",
            "::1",
            "http://::1",
            "::1:8080",
            "::ffff:127.0.0.1",
            "http://[127.0.0.1]:8080",
            "http://[localhost]:8080",
        ] {
            not_loopback(u);
        }
        for u in ["http://[::1", "http://[::1]x", "http://::1]", "http://[::1]]:80"] {
            refused(u);
        }
    }

    #[test]
    fn rejects_tls_other_schemes_userinfo_and_junk() {
        for (u, scheme) in [
            ("https://127.0.0.1:8080", "https"),
            ("HTTPS://localhost", "HTTPS"),
            ("ftp://localhost", "ftp"),
            ("ws://127.0.0.1:8080", "ws"),
            ("file://localhost/etc/passwd", "file"),
            ("gopher://127.0.0.1", "gopher"),
            ("://127.0.0.1", ""),
        ] {
            match LocalUrl::parse(u, 1) {
                Err(LocalUrlError::Scheme { scheme: s, .. }) => assert_eq!(s, scheme, "{u}"),
                other => panic!("{u}: expected Scheme, got {other:?}"),
            }
        }
        for u in [
            "http://evil.com@127.0.0.1:8080",
            "http://127.0.0.1@evil.com",
            "http://user:pw@localhost:8080",
            "user@host",
            "evil.com#@127.0.0.1",
            "http://127.0.0.1:8080/@evil.com",
        ] {
            assert!(matches!(LocalUrl::parse(u, 1), Err(LocalUrlError::Userinfo { .. })), "{u}");
        }
        assert!(matches!(LocalUrl::parse("", 1), Err(LocalUrlError::Empty(_))));
        assert!(matches!(LocalUrl::parse("   \t ", 1), Err(LocalUrlError::Empty(_))));
        for u in [
            "http://127.0.0.1:99999",
            "http://127.0.0.1:65536",
            "http://127.0.0.1:0",
            "http://127.0.0.1:",
            "http://127.0.0.1:+80",
            "http://127.0.0.1:-1",
            "http://127.0.0.1:8o80",
            "http://127.0.0.1:000080",
            "http://[::1]:",
            "localhost:abc",
        ] {
            assert!(matches!(LocalUrl::parse(u, 1), Err(LocalUrlError::Port { .. })), "{u}");
        }
        for u in [
            "http://",
            "http:///v1",
            "//evil.com",
            "/v1",
            "http://127.0.0.1:8080/x y",
            "http://127.0.0.1 :8080",
            "http://127.0.0.1:8080/\r\nHost: evil",
            "http://127.0.0.1\\.evil.com",
            "http://evil.com\\@127.0.0.1",
            "http://127.0.0.1:8080?x=1",
            "http://127.0.0.1#frag",
            "evil.com#127.0.0.1",
            "http://ｌｏｃａｌｈｏｓｔ:8080",
            "http://127。0。0。1:8080",
            "http://127.0.0.1\u{0}:8080",
        ] {
            assert!(matches!(LocalUrl::parse(u, 1), Err(LocalUrlError::Malformed { .. })), "{u:?}: {:?}", LocalUrl::parse(u, 1));
        }
        let msg = LocalUrl::parse("http://example.com", 1).unwrap_err().to_string();
        assert!(msg.contains("local-only"), "{msg}");
    }

    #[test]
    fn loopback_ip_helper() {
        assert_eq!(loopback_ip("localhost"), Some(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert_eq!(loopback_ip("127.9.9.9"), Some(IpAddr::V4(Ipv4Addr::new(127, 9, 9, 9))));
        assert_eq!(loopback_ip("[::1]"), Some(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        for h in ["::1", "localhost.", "a.localhost", "[::ffff:127.0.0.1]", "127.1", "0.0.0.0", " localhost", ""] {
            assert_eq!(loopback_ip(h), None, "{h:?}");
        }
    }

    #[test]
    fn ensure_local_matches_parse() {
        assert!(ensure_local("127.0.0.1:1", 2).is_ok());
        assert!(ensure_local("10.1.1.1", 2).is_err());
    }
}
