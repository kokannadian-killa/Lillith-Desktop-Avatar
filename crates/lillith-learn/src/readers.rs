//! Installed-docs readers (§1, P1): man and info pages, `/usr/share/doc`,
//! and the local Rust / Python html docs. Parsed in-process (gzip via
//! flate2, a basic roff strip, html tags stripped); never runs `man`,
//! `info` or `pydoc`, never touches the network. Every file goes through
//! [`Roots::admit`] (canonical, inside a root, not denied, ≤ the size cap).

use crate::paths::{drop_secret_lines, is_binary, Refusal, Roots};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Kind of a doc source (the `sources.type` / note `source.type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKind {
    Man,
    Info,
    Doc,
    RustDoc,
    PythonDoc,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Man => "man",
            Self::Info => "info",
            Self::Doc => "doc",
            Self::RustDoc => "rust-doc",
            Self::PythonDoc => "python-doc",
        }
    }

    /// Prior usefulness of the kind for the picker.
    pub fn weight(self) -> f64 {
        match self {
            Self::Man => 1.0,
            Self::Info => 0.7,
            Self::Doc => 0.6,
            Self::RustDoc => 0.8,
            Self::PythonDoc => 0.6,
        }
    }
}

/// One readable file.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub kind: SourceKind,
    pub path: PathBuf,
    /// Stable reference (`man:ls(1)`, `info:coreutils`, `doc:<path>`).
    pub reference: String,
    /// The topic word (`ls`, `coreutils`, the package folder).
    pub name: String,
    pub size: u64,
}

/// Where each kind is looked for.
#[derive(Debug, Clone, Default)]
pub struct DocRoots {
    pub man: Vec<PathBuf>,
    pub info: Vec<PathBuf>,
    pub doc: Vec<PathBuf>,
    pub rust_doc: Vec<PathBuf>,
    pub python_doc: Vec<PathBuf>,
}

/// Files of one kind kept at most (keeps memory small on big systems).
const MAX_PER_KIND: usize = 6000;
/// Folder depth walked under a root.
const MAX_DEPTH: usize = 6;

impl DocRoots {
    /// The system locations (those that exist are used).
    pub fn system() -> Self {
        let mut rust_doc = Vec::new();
        if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
            let tc = PathBuf::from(home).join(".rustup/toolchains");
            if let Ok(rd) = std::fs::read_dir(&tc) {
                for e in rd.flatten() {
                    rust_doc.push(e.path().join("share/doc/rust/html/std"));
                }
            }
        }
        rust_doc.push(PathBuf::from("/usr/share/doc/rust/html/std"));
        Self {
            man: vec![PathBuf::from("/usr/share/man")],
            info: vec![PathBuf::from("/usr/share/info")],
            doc: vec![PathBuf::from("/usr/share/doc")],
            rust_doc,
            python_doc: vec![
                PathBuf::from("/usr/share/doc/python/html"),
                PathBuf::from("/usr/share/doc/python3/html"),
            ],
        }
    }

    /// Every root, for [`Roots`].
    pub fn all(&self) -> Vec<PathBuf> {
        [
            &self.man,
            &self.info,
            &self.doc,
            &self.rust_doc,
            &self.python_doc,
        ]
        .iter()
        .flat_map(|v| v.iter().cloned())
        .collect()
    }

    /// The confinement for these roots.
    pub fn confinement(&self) -> Roots {
        Roots::new(self.all())
    }

    /// Every candidate of the enabled `sources` (`man`, `info`, `doc`,
    /// `rust-doc`, `python-doc`), at most `max_bytes` each.
    pub fn discover(&self, sources: &[String], max_bytes: u64) -> Vec<Candidate> {
        let on = |k: SourceKind| sources.iter().any(|s| s.trim() == k.as_str());
        let mut out = Vec::new();
        if on(SourceKind::Man) {
            for r in &self.man {
                for sect in ["1", "5", "8"] {
                    let dir = r.join(format!("man{sect}"));
                    for (p, size) in walk(&dir, 1, MAX_PER_KIND) {
                        let file = file_name(&p);
                        let Some(stem) = file.strip_suffix(".gz") else {
                            continue;
                        };
                        // `ls.1` / `systemd.service.5`.
                        let name = stem.rsplit_once('.').map(|(n, _)| n).unwrap_or(stem);
                        if size <= max_bytes && !name.is_empty() {
                            out.push(Candidate {
                                kind: SourceKind::Man,
                                reference: format!("man:{name}({sect})"),
                                name: name.to_string(),
                                path: p,
                                size,
                            });
                        }
                    }
                }
            }
        }
        if on(SourceKind::Info) {
            for r in &self.info {
                for (p, size) in walk(r, 1, MAX_PER_KIND) {
                    let file = file_name(&p);
                    let Some(name) = file.strip_suffix(".info.gz") else {
                        continue;
                    };
                    if size <= max_bytes {
                        out.push(Candidate {
                            kind: SourceKind::Info,
                            reference: format!("info:{name}"),
                            name: name.to_string(),
                            path: p,
                            size,
                        });
                    }
                }
            }
        }
        let skip: Vec<PathBuf> = self
            .rust_doc
            .iter()
            .chain(&self.python_doc)
            .filter_map(|p| std::fs::canonicalize(p).ok())
            .collect();
        for (kind, roots) in [
            (SourceKind::Doc, &self.doc),
            (SourceKind::RustDoc, &self.rust_doc),
            (SourceKind::PythonDoc, &self.python_doc),
        ] {
            if !on(kind) {
                continue;
            }
            for r in roots {
                let Ok(rc) = std::fs::canonicalize(r) else {
                    continue;
                };
                for (p, size) in walk(&rc, MAX_DEPTH, MAX_PER_KIND) {
                    if size > max_bytes || !doc_file(&p, kind) {
                        continue;
                    }
                    if kind == SourceKind::Doc && skip.iter().any(|s| p.starts_with(s)) {
                        continue;
                    }
                    let rel = p.strip_prefix(&rc).unwrap_or(&p);
                    let name = match kind {
                        SourceKind::Doc => rel
                            .components()
                            .next()
                            .map(|c| c.as_os_str().to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        _ => file_name(&p)
                            .trim_end_matches(".html")
                            .rsplit('.')
                            .next()
                            .unwrap_or("")
                            .to_string(),
                    };
                    out.push(Candidate {
                        kind,
                        reference: format!("{}:{}", kind.as_str(), p.display()),
                        name,
                        path: p,
                        size,
                    });
                }
            }
        }
        out
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn doc_file(p: &Path, kind: SourceKind) -> bool {
    let f = file_name(p).to_lowercase();
    match kind {
        SourceKind::Doc => {
            f.ends_with(".md")
                || f.ends_with(".txt")
                || f.ends_with(".html")
                || f.starts_with("readme")
        }
        _ => f.ends_with(".html") && !f.starts_with("index") && !f.starts_with("all"),
    }
}

/// Regular files (and file symlinks; [`Roots::admit`] checks those) under
/// `dir`, at most `depth` levels and `cap` files. Folder symlinks are not
/// followed.
fn walk(dir: &Path, depth: usize, cap: usize) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 1usize)];
    while let Some((d, lvl)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if crate::paths::denied(&p) {
                continue;
            }
            if ft.is_dir() {
                if lvl < depth {
                    stack.push((p, lvl + 1));
                }
            } else if ft.is_file() || ft.is_symlink() {
                let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(u64::MAX);
                out.push((p, size));
                if out.len() >= cap {
                    return out;
                }
            }
        }
    }
    out
}

/// Why a file gave no text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    Refused(Refusal),
    Binary,
    Io(String),
}

/// Decompressed text never grows past this (a gzip bomb stops here).
const MAX_TEXT_BYTES: u64 = 1024 * 1024;

/// The plain text of `c` (decompressed, roff/html stripped, secret-looking
/// lines dropped), read through `roots`.
pub fn read_text(c: &Candidate, roots: &Roots, max_bytes: u64) -> Result<String, ReadError> {
    let path = roots
        .admit(&c.path, max_bytes)
        .map_err(ReadError::Refused)?;
    let raw = std::fs::read(&path).map_err(|e| ReadError::Io(e.to_string()))?;
    let bytes = if file_name(&path).ends_with(".gz") {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(&raw[..])
            .take(MAX_TEXT_BYTES)
            .read_to_end(&mut out)
            .map_err(|e| ReadError::Io(e.to_string()))?;
        out
    } else {
        raw
    };
    if is_binary(&bytes) {
        return Err(ReadError::Binary);
    }
    let text = String::from_utf8_lossy(&bytes);
    let plain = match c.kind {
        SourceKind::Man => strip_roff(&text),
        SourceKind::Info => strip_info(&text),
        _ if file_name(&path).to_lowercase().ends_with(".html") => strip_html(&text),
        _ => text.into_owned(),
    };
    Ok(tidy(&drop_secret_lines(&plain)))
}

/// Collapses runs of blank lines and trailing spaces.
fn tidy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank = 0;
    for l in s.lines() {
        let l = l.trim_end();
        if l.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(l);
        out.push('\n');
    }
    out.trim().to_string()
}

/// Chunk `idx` of `text` (≈`size` bytes, cut at line ends). `None` past
/// the end.
pub fn chunk(text: &str, idx: usize, size: usize) -> Option<String> {
    chunks(text, size).into_iter().nth(idx)
}

/// How many chunks `text` has.
pub fn chunk_count(text: &str, size: usize) -> usize {
    chunks(text, size).len()
}

fn chunks(text: &str, size: usize) -> Vec<String> {
    let size = size.max(64);
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in text.lines() {
        let mut line = line;
        // A very long line is cut on char boundaries.
        while line.len() > size {
            let mut cut = size;
            while !line.is_char_boundary(cut) {
                cut -= 1;
            }
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            out.push(line[..cut].to_string());
            line = &line[cut..];
        }
        if cur.len() + line.len() + 1 > size && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.push_str(line);
        cur.push('\n');
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// A basic roff strip: macros dropped (their text kept for headings and
/// font macros), escapes resolved or removed.
pub fn strip_roff(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let l = line.trim_end();
        if l.starts_with(".\\\"") || l.starts_with("'\\\"") || l.starts_with("\\\"") {
            continue;
        }
        if let Some(rest) = l.strip_prefix('.').or_else(|| l.strip_prefix('\'')) {
            let (mac, args) = rest
                .trim_start()
                .split_once(char::is_whitespace)
                .unwrap_or((rest.trim(), ""));
            let args = args.trim().replace('"', "");
            match mac {
                "SH" | "SS" => {
                    out.push('\n');
                    out.push_str(&unescape_roff(&args).to_uppercase());
                    out.push('\n');
                }
                "B" | "I" | "BR" | "IR" | "RB" | "RI" | "BI" | "IB" | "SM" | "SB" | "IP" | "TP"
                | "Nm" | "Nd" | "Fl" | "Ar" | "Op" | "Xr" => {
                    let t = unescape_roff(&args);
                    if !t.is_empty() {
                        out.push_str(&t);
                        out.push('\n');
                    }
                }
                "PP" | "LP" | "P" | "sp" | "br" | "Pp" => out.push('\n'),
                _ => {}
            }
            continue;
        }
        out.push_str(&unescape_roff(l));
        out.push('\n');
    }
    out
}

fn unescape_roff(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('f') => {
                // \fB, \fI, \fR, \fP, \f(XX, \f[...]
                match it.next() {
                    Some('(') => {
                        it.next();
                        it.next();
                    }
                    Some('[') => {
                        for c in it.by_ref() {
                            if c == ']' {
                                break;
                            }
                        }
                    }
                    _ => {}
                }
            }
            Some('(') => {
                let a = it.next().unwrap_or(' ');
                let b = it.next().unwrap_or(' ');
                out.push_str(match (a, b) {
                    ('e', 'm') | ('e', 'n') => "-",
                    ('a', 'q') => "'",
                    ('l', 'q') | ('r', 'q') | ('d', 'q') => "\"",
                    ('b', 'u') => "*",
                    ('t', 'i') => "~",
                    ('h', 'a') => "^",
                    _ => "",
                });
            }
            Some('*') => {
                // \*x, \*(xx, \*[...]
                match it.next() {
                    Some('(') => {
                        it.next();
                        it.next();
                    }
                    Some('[') => {
                        for c in it.by_ref() {
                            if c == ']' {
                                break;
                            }
                        }
                    }
                    _ => {}
                }
            }
            Some('s') => {
                if matches!(it.peek(), Some('+') | Some('-')) {
                    it.next();
                }
                while matches!(it.peek(), Some(c) if c.is_ascii_digit()) {
                    it.next();
                }
            }
            Some('-') => out.push('-'),
            Some('e') | Some('\\') => out.push('\\'),
            Some('&') | Some('c') | Some('%') | Some('|') | Some('^') => {}
            Some(' ') | Some('~') => out.push(' '),
            Some('"') => break,
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

/// Info files: drop the node separators and control bytes.
fn strip_info(src: &str) -> String {
    src.lines()
        .filter(|l| !l.starts_with('\u{1f}') && !l.starts_with("File: "))
        .map(|l| l.replace(['\u{7f}', '\u{8}'], ""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Html → text: `<script>`/`<style>` dropped, tags removed (block tags
/// become line breaks), the common entities decoded.
pub fn strip_html(src: &str) -> String {
    let mut out = String::with_capacity(src.len() / 2);
    let lower = src.to_ascii_lowercase();
    let mut i = 0;
    let b = src.as_bytes();
    while i < b.len() {
        if b[i] == b'<' {
            let rest = &lower[i..];
            if let Some(skip) = ["script", "style", "nav", "head"].iter().find(|t| {
                rest.strip_prefix('<')
                    .and_then(|r| r.strip_prefix(*t))
                    .is_some_and(|r| r.starts_with(['>', ' ', '\t', '\n', '/']))
            }) {
                let close = format!("</{skip}>");
                i = match rest.find(&close) {
                    Some(e) => i + e + close.len(),
                    None => b.len(),
                };
                continue;
            }
            let end = rest.find('>').map(|e| i + e + 1).unwrap_or(b.len());
            let name: String = rest[1..]
                .trim_start_matches('/')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            let name = name.as_str();
            if matches!(
                name,
                "p" | "br"
                    | "div"
                    | "li"
                    | "tr"
                    | "pre"
                    | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "section"
                    | "dt"
                    | "dd"
                    | "table"
                    | "ul"
                    | "ol"
            ) {
                out.push('\n');
            }
            i = end;
            continue;
        }
        let next = lower[i..].find('<').map(|e| i + e).unwrap_or(b.len());
        out.push_str(&decode_entities(&src[i..next]));
        i = next;
    }
    out
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    pub(crate) fn gz(text: &str) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(text.as_bytes()).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn roff_strip() {
        let t = strip_roff(
            ".\\\" comment\n.TH LS 1\n.SH NAME\nls \\- list directory contents\n.SH OPTIONS\n.TP\n.B \\-a\ndo not ignore entries starting with \\fB.\\fR\n",
        );
        assert!(t.contains("NAME"));
        assert!(t.contains("ls - list directory contents"));
        assert!(t.contains("-a"));
        assert!(t.contains("starting with ."));
        assert!(!t.contains("comment") && !t.contains("\\f"));
    }

    #[test]
    fn html_strip() {
        let t = strip_html(
            "<html><head><title>x</title></head><body><script>evil()</script><h1>Vec</h1><p>A &lt;growable&gt; array &amp; more</p></body></html>",
        );
        assert!(t.contains("Vec"));
        assert!(t.contains("A <growable> array & more"));
        assert!(!t.contains("evil") && !t.contains("<h1>") && !t.contains("title"));
    }

    #[test]
    fn chunks_are_bounded_and_cover_text() {
        let text = (0..200)
            .map(|i| format!("line {i} with some words"))
            .collect::<Vec<_>>()
            .join("\n");
        let n = chunk_count(&text, 2048);
        assert!(n >= 2);
        for i in 0..n {
            assert!(chunk(&text, i, 2048).unwrap().len() <= 2048);
        }
        assert!(chunk(&text, n, 2048).is_none());
        let long = "x".repeat(5000);
        assert_eq!(chunk_count(&long, 2048), 3);
    }

    #[test]
    fn discover_and_read_man_info_doc() {
        let d = std::env::temp_dir().join(format!("lillith-readers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("man/man1")).unwrap();
        std::fs::create_dir_all(d.join("man/man3")).unwrap();
        std::fs::create_dir_all(d.join("info")).unwrap();
        std::fs::create_dir_all(d.join("doc/pacman")).unwrap();
        std::fs::write(
            d.join("man/man1/ls.1.gz"),
            gz(".SH NAME\nls \\- list directory contents\n"),
        )
        .unwrap();
        std::fs::write(d.join("man/man3/printf.3.gz"), gz("libc")).unwrap();
        std::fs::write(
            d.join("info/sed.info.gz"),
            gz("\u{1f}\nFile: sed.info\nsed is a stream editor\n"),
        )
        .unwrap();
        std::fs::write(
            d.join("doc/pacman/README.md"),
            "pacman -Syu updates\nsecret = Zx9Qw8Er7Ty6Ui5Op4As3Df2Gh1Jk0LmNb\n",
        )
        .unwrap();
        std::fs::write(d.join("doc/pacman/blob.bin"), b"\x00\x01").unwrap();
        std::fs::write(d.join("doc/pacman/big.txt"), vec![b'a'; 4096]).unwrap();
        std::fs::write(d.join("doc/pacman/elf.txt"), b"\x7fELF\x00\x00").unwrap();
        let roots = DocRoots {
            man: vec![d.join("man")],
            info: vec![d.join("info")],
            doc: vec![d.join("doc")],
            ..DocRoots::default()
        };
        let src: Vec<String> = ["man", "info", "doc"].map(String::from).to_vec();
        let found = roots.discover(&src, 1024);
        let refs: Vec<&str> = found.iter().map(|c| c.reference.as_str()).collect();
        assert!(refs.contains(&"man:ls(1)"), "{refs:?}");
        assert!(refs.contains(&"info:sed"), "{refs:?}");
        assert!(!refs.iter().any(|r| r.contains("printf")), "only man1/5/8");
        assert!(!refs
            .iter()
            .any(|r| r.contains("blob.bin") || r.contains("big.txt")));
        let conf = roots.confinement();
        let get = |r: &str| found.iter().find(|c| c.reference == r).unwrap();
        let ls = read_text(get("man:ls(1)"), &conf, 1024).unwrap();
        assert!(ls.contains("ls - list directory contents"));
        let sed = read_text(get("info:sed"), &conf, 1024).unwrap();
        assert_eq!(sed, "sed is a stream editor");
        let readme = found
            .iter()
            .find(|c| c.name == "pacman" && c.reference.ends_with("README.md"))
            .unwrap();
        let t = read_text(readme, &conf, 1024).unwrap();
        assert_eq!(t, "pacman -Syu updates", "secret line dropped");
        let elf = found
            .iter()
            .find(|c| c.reference.ends_with("elf.txt"))
            .unwrap();
        assert_eq!(read_text(elf, &conf, 1024), Err(ReadError::Binary));
        // Turned-off kinds are not listed.
        assert!(roots
            .discover(&["doc".into()], 1024)
            .iter()
            .all(|c| c.kind == SourceKind::Doc));
        let _ = std::fs::remove_dir_all(d);
    }
}
