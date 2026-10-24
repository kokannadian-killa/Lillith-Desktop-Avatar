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
    match h.parse::<std::net::Ipv4Addr>() {
        Ok(ip) => {
            let o = ip.octets();
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_multicast()
                || ip.is_documentation()
                || (o[0] == 100 && (64..128).contains(&o[1]))
                || o[0] == 0
        }
        // A dotted all-numeric host that isn't a valid IPv4 (e.g. `127.1`,
        // `2130706433`) is refused too: resolvers read those as IPs.
        Err(_) => h.split('.').all(|l| l.bytes().all(|b| b.is_ascii_digit())),
    }
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
