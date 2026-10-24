//! What a fetched body may be and the text kept from it (§5.2, §12: only
//! extracted text is stored; raw HTML is discarded).

/// Content types a trip accepts (`text/*` too).
pub const ALLOWED_TYPES: &[&str] = &["application/xhtml+xml", "application/json"];

/// Leading bytes of executables, archives and other binaries.
const MAGIC: &[&[u8]] = &[
    b"MZ",
    b"\x7fELF",
    b"PK\x03\x04",
    b"PK\x05\x06",
    b"\x1f\x8b",
    b"BZh",
    b"\xfd7zXZ",
    b"7z\xbc\xaf\x27\x1c",
    b"Rar!",
    b"%PDF",
    b"\xca\xfe\xba\xbe",
    b"\xcf\xfa\xed\xfe",
    b"\xce\xfa\xed\xfe",
    b"\xfe\xed\xfa",
    b"\x00asm",
    b"#!",
    b"\x89PNG",
    b"\xff\xd8\xff",
    b"GIF8",
    b"OggS",
    b"ustar",
];

/// `Ok` when a body of `content_type` may be kept: an allowed type AND
/// bytes that don't sniff as binary (a server can lie about the type).
pub fn content_allowed(content_type: &str, body: &[u8]) -> Result<(), String> {
    let main = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if main.is_empty() {
        return Err("no content type".into());
    }
    if !(main.starts_with("text/") || ALLOWED_TYPES.contains(&main.as_str())) {
        return Err(format!("content type {main} is not allowed"));
    }
    let head = body.strip_prefix(b"\xef\xbb\xbf").unwrap_or(body);
    if MAGIC.iter().any(|m| head.starts_with(m))
        || head.get(257..262) == Some(b"ustar")
        || crate::paths::is_binary(head)
    {
        return Err(format!("the bytes are not text (sent as {main})"));
    }
    Ok(())
}

/// `needle` in `lower` (an ASCII-lowercased copy of the text, same byte
/// offsets) at or after `from`.
fn find_from(lower: &str, needle: &str, from: usize) -> Option<usize> {
    lower.get(from..)?.find(needle).map(|i| i + from)
}

/// Removes every `<tag …>…</tag>` block (script, style, …).
fn drop_blocks(s: &str, tag: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let (open, close) = (format!("<{tag}"), format!("</{tag}"));
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while let Some(a) = find_from(&lower, &open, i) {
        out.push_str(&s[i..a]);
        match find_from(&lower, &close, a) {
            Some(b) => {
                i = s[b..].find('>').map(|e| b + e + 1).unwrap_or(s.len());
            }
            None => {
                i = s.len();
            }
        }
    }
    if i < s.len() {
        out.push_str(&s[i..]);
    }
    out
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(a) = rest.find('&') {
        out.push_str(&rest[..a]);
        let tail = &rest[a..];
        let end = tail.find(';').filter(|e| *e <= 10);
        let decoded = end.and_then(|e| {
            let name = &tail[1..e];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some(' '),
                _ => name
                    .strip_prefix("#x")
                    .or_else(|| name.strip_prefix("#X"))
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            };
            c.map(|c| (c, e + 1))
        });
        match decoded {
            Some((c, n)) => {
                out.push(c);
                rest = &tail[n..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The page title and its visible text (HTML stripped; JSON / plain text
/// kept as is), one paragraph per line, blank lines dropped.
pub fn extract(content_type: &str, body: &[u8]) -> (Option<String>, String) {
    let raw = String::from_utf8_lossy(body);
    let ct = content_type.to_ascii_lowercase();
    let html = ct.contains("html") || ct.contains("xml");
    if !html {
        return (None, tidy(&raw));
    }
    // Comments first, then non-visible blocks.
    let mut s = String::with_capacity(raw.len());
    let mut rest: &str = &raw;
    while let Some(a) = rest.find("<!--") {
        s.push_str(&rest[..a]);
        s.push(' ');
        rest = match rest[a..].find("-->") {
            Some(e) => &rest[a + e + 3..],
            None => "",
        };
    }
    s.push_str(rest);
    let lower = s.to_ascii_lowercase();
    let title = find_from(&lower, "<title", 0).and_then(|a| {
        let start = s[a..].find('>')? + a + 1;
        let end = find_from(&lower, "</title", start)?;
        Some(tidy(&decode_entities(&s[start..end])).replace('\n', " "))
    });
    for t in [
        "head", "script", "style", "noscript", "template", "svg", "iframe", "object", "nav", "form",
    ] {
        s = drop_blocks(&s, t);
    }
    let mut out = String::with_capacity(s.len() / 2);
    let mut i = 0;
    while let Some(a) = s[i..].find('<').map(|x| x + i) {
        out.push_str(&s[i..a]);
        let Some(e) = s[a..].find('>').map(|x| a + x) else {
            i = s.len();
            break;
        };
        let tag = s[a + 1..e]
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(
            tag.as_str(),
            "p" | "br"
                | "div"
                | "li"
                | "tr"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "pre"
                | "section"
                | "article"
                | "dd"
                | "dt"
                | "blockquote"
                | "table"
                | "ul"
                | "ol"
        ) {
            out.push('\n');
        } else {
            out.push(' ');
        }
        i = e + 1;
    }
    if i < s.len() {
        out.push_str(&s[i..]);
    }
    (
        title.filter(|t| !t.is_empty()),
        tidy(&decode_entities(&out)),
    )
}

fn tidy(s: &str) -> String {
    s.lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// At most one line of at most `max` chars: the first sentence.
pub fn one_line_summary(text: &str, max: usize) -> String {
    let first = text.lines().next().unwrap_or("").trim();
    let sentence = match first.find(". ") {
        Some(i) => &first[..=i],
        None => first,
    };
    if sentence.chars().count() <= max {
        sentence.to_string()
    } else {
        let mut t: String = sentence.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_and_sniff() {
        assert!(content_allowed("text/html; charset=utf-8", b"<p>hi</p>").is_ok());
        assert!(content_allowed("application/json", b"{\"a\":1}").is_ok());
        assert!(content_allowed("application/xhtml+xml", b"<html/>").is_ok());
        assert!(content_allowed("application/octet-stream", b"hello").is_err());
        assert!(content_allowed("application/zip", b"hello").is_err());
        assert!(content_allowed("", b"hello").is_err());
        assert!(content_allowed("text/plain", b"MZ\x90\x00").is_err());
        assert!(content_allowed("text/plain", b"\x7fELF\x02\x01").is_err());
        assert!(content_allowed("text/html", b"PK\x03\x04zip").is_err());
        assert!(content_allowed("text/plain", b"#!/bin/sh\nrm -rf ~").is_err());
    }

    #[test]
    fn html_text() {
        let (t, x) = extract(
            "text/html",
            b"<html><head><title>Rust &amp; Cargo</title><script>evil()</script></head>\
              <body><!-- c --><nav>menu</nav><h1>Cargo</h1><p>Cargo is Rust&#39;s <b>build</b> tool.</p>\
              <style>.x{}</style><p>Second&nbsp;para.</p></body></html>",
        );
        assert_eq!(t.as_deref(), Some("Rust & Cargo"));
        assert_eq!(x, "Cargo\nCargo is Rust's build tool.\nSecond para.");
        assert_eq!(one_line_summary("A b. C d.\nnext", 80), "A b.");
        assert_eq!(one_line_summary("abcdef", 4), "abc…");
    }
}
