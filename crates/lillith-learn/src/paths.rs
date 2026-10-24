//! Where the learner may read (§1, §8): every path is canonicalized and
//! must stay inside one of the configured roots (a symlink that escapes is
//! refused), and a hard denylist (keys, keyrings, browser profiles,
//! Lillith's own config, anything named like a password/secret/token) is
//! refused even inside a root. Secret-looking lines are dropped from what
//! was read.

use std::path::{Path, PathBuf};

/// Why a path was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Does not exist / unreadable.
    Missing,
    /// Resolves outside every root (e.g. through a symlink).
    Escapes,
    /// On the hard denylist.
    Denied,
    /// Not a regular file.
    NotFile,
    /// Bigger than `max_file_bytes`.
    TooBig,
}

/// Folder names never read (any depth).
const DENY_DIRS: &[&str] = &[
    ".ssh",
    ".gnupg",
    "keyrings",
    ".password-store",
    ".mozilla",
    ".librewolf",
    ".thunderbird",
    "google-chrome",
    "chromium",
    "bravesoftware",
    "vivaldi",
    "opera",
    "microsoft-edge",
];

/// File extensions never read.
const DENY_EXT: &[&str] = &["key", "pem", "kdbx", "p12", "pfx", "gpg", "asc", "keystore"];

/// Words that make a file or folder name off limits.
const DENY_WORDS: &[&str] = &["password", "passwd", "secret", "token", "credential"];

/// `true` when `p` is on the hard denylist.
pub fn denied(p: &Path) -> bool {
    let s = p.to_string_lossy().to_lowercase().replace('\\', "/");
    // Lillith's own config (persona, model files, the config itself).
    if s.contains("/.config/lillith") || s.contains("/appdata/roaming/lillith") {
        return true;
    }
    let parts: Vec<&str> = s.split('/').filter(|c| !c.is_empty()).collect();
    for c in &parts {
        if DENY_DIRS.contains(c) || DENY_WORDS.iter().any(|w| c.contains(w)) {
            return true;
        }
    }
    let Some(name) = parts.last() else {
        return false;
    };
    if *name == ".env" || name.starts_with(".env.") || name.ends_with(".env") {
        return true;
    }
    match name.rsplit_once('.') {
        Some((_, ext)) => DENY_EXT.contains(&ext),
        None => false,
    }
}

/// The canonical read roots.
#[derive(Debug, Clone, Default)]
pub struct Roots {
    roots: Vec<PathBuf>,
}

impl Roots {
    /// Roots that exist, canonicalized (missing ones are skipped).
    pub fn new<I: IntoIterator<Item = PathBuf>>(paths: I) -> Self {
        let mut roots: Vec<PathBuf> = paths
            .into_iter()
            .filter_map(|p| std::fs::canonicalize(p).ok())
            .filter(|p| !denied(p))
            .collect();
        roots.sort();
        roots.dedup();
        Self { roots }
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// The canonical path of `p` when it may be read: inside a root, not
    /// denied, a regular file of at most `max_bytes`.
    pub fn admit(&self, p: &Path, max_bytes: u64) -> Result<PathBuf, Refusal> {
        if denied(p) {
            return Err(Refusal::Denied);
        }
        let c = std::fs::canonicalize(p).map_err(|_| Refusal::Missing)?;
        if !self.roots.iter().any(|r| c.starts_with(r)) {
            return Err(Refusal::Escapes);
        }
        if denied(&c) {
            return Err(Refusal::Denied);
        }
        let md = std::fs::metadata(&c).map_err(|_| Refusal::Missing)?;
        if !md.is_file() {
            return Err(Refusal::NotFile);
        }
        if md.len() > max_bytes {
            return Err(Refusal::TooBig);
        }
        Ok(c)
    }
}

/// `true` for bytes that look binary (a NUL, or many control bytes).
pub fn is_binary(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(8192)];
    if head.contains(&0) {
        return true;
    }
    let ctrl = head
        .iter()
        .filter(|b| **b < 0x20 && !matches!(**b, b'\n' | b'\r' | b'\t' | 0x0c | 0x1b | 0x08))
        .count();
    ctrl * 10 > head.len().max(1)
}

fn entropy(s: &str) -> f64 {
    let mut counts = [0u32; 256];
    for b in s.bytes() {
        counts[b as usize] += 1;
    }
    let n = s.len() as f64;
    counts
        .iter()
        .filter(|c| **c > 0)
        .map(|c| {
            let p = f64::from(*c) / n;
            -p * p.log2()
        })
        .sum()
}

/// `true` for a line that carries something secret-looking: a private-key
/// armor line, or a token of ≥32 key-ish characters with high entropy.
pub fn looks_secret(line: &str) -> bool {
    let l = line.trim();
    if l.contains("-----BEGIN") && l.contains("PRIVATE KEY") {
        return true;
    }
    l.split(|c: char| c.is_whitespace() || matches!(c, '=' | ':' | '"' | '\'' | ',' | ';'))
        .filter(|t| t.len() >= 32)
        .any(|t| {
            let keyish = t
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-'));
            let slashes = t.matches('/').count();
            let classes = [
                t.chars().any(|c| c.is_ascii_uppercase()),
                t.chars().any(|c| c.is_ascii_lowercase()),
                t.chars().any(|c| c.is_ascii_digit()),
            ]
            .iter()
            .filter(|b| **b)
            .count();
            keyish && slashes <= 2 && classes >= 2 && entropy(t) >= 3.5
        })
}

/// `text` without its secret-looking lines.
pub fn drop_secret_lines(text: &str) -> String {
    text.lines()
        .filter(|l| !looks_secret(l))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lillith-paths-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn denylist() {
        for p in [
            "/home/ko/.ssh/id_ed25519",
            "/home/ko/.gnupg/pubring.kbx",
            "/home/ko/.config/lillith/config.toml",
            "/home/ko/.mozilla/firefox/x.default/logins.json",
            "/home/ko/.config/google-chrome/Default/Cookies",
            "/home/ko/.local/share/keyrings/login.keyring",
            "/srv/app/.env",
            "/srv/app/server.pem",
            "/srv/app/tls.key",
            "/home/ko/vault.kdbx",
            "/usr/share/doc/foo/password-notes.txt",
            "/usr/share/doc/foo/my_secret.md",
            "/usr/share/doc/api-token/README",
        ] {
            assert!(denied(Path::new(p)), "{p}");
        }
        for p in [
            "/usr/share/man/man1/ls.1.gz",
            "/usr/share/doc/pacman/README.md",
            "/usr/share/info/coreutils.info.gz",
        ] {
            assert!(!denied(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn confinement_and_symlink_escape() {
        let d = tmp("conf");
        let root = d.join("root");
        let outside = d.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(root.join("ok.txt"), "hello").unwrap();
        std::fs::write(outside.join("private.txt"), "nope").unwrap();
        std::fs::write(root.join("big.txt"), vec![b'a'; 300]).unwrap();
        let roots = Roots::new([root.clone(), d.join("missing")]);
        assert_eq!(roots.roots().len(), 1);
        assert!(roots.admit(&root.join("ok.txt"), 256).is_ok());
        assert_eq!(
            roots.admit(&root.join("big.txt"), 256),
            Err(Refusal::TooBig)
        );
        assert_eq!(
            roots.admit(&outside.join("private.txt"), 256),
            Err(Refusal::Escapes)
        );
        assert_eq!(
            roots.admit(&root.join("../outside/private.txt"), 256),
            Err(Refusal::Escapes)
        );
        assert_eq!(roots.admit(&root, 256), Err(Refusal::NotFile));
        assert_eq!(roots.admit(&root.join("gone"), 256), Err(Refusal::Missing));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.join("private.txt"), root.join("link.txt")).unwrap();
            std::os::unix::fs::symlink(root.join("ok.txt"), root.join("inner.txt")).unwrap();
            assert_eq!(
                roots.admit(&root.join("link.txt"), 256),
                Err(Refusal::Escapes),
                "a symlink out of the root is refused"
            );
            assert!(roots.admit(&root.join("inner.txt"), 256).is_ok());
        }
        std::fs::write(root.join("server.pem"), "x").unwrap();
        assert_eq!(
            roots.admit(&root.join("server.pem"), 256),
            Err(Refusal::Denied)
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn secret_lines_are_dropped() {
        assert!(looks_secret(
            "api_key = \"sk-AbCdEf0123456789GhIjKlMnOpQrStUv\""
        ));
        assert!(looks_secret("-----BEGIN OPENSSH PRIVATE KEY-----"));
        assert!(looks_secret("ghp_9fK2LmQ8xR4tY7uI1oP3aS5dF6gH0jKzZ"));
        assert!(!looks_secret("ls - list directory contents"));
        assert!(!looks_secret(
            "see /usr/share/doc/packages/coreutils/README"
        ));
        assert!(!looks_secret("supercalifragilisticexpialidociousness"));
        let t = drop_secret_lines("keep me\ntoken: Zx9Qw8Er7Ty6Ui5Op4As3Df2Gh1Jk0LmNb\nand me");
        assert_eq!(t, "keep me\nand me");
    }

    #[test]
    fn binary_detection() {
        assert!(is_binary(b"\x7fELF\x00\x01"));
        assert!(!is_binary(b"plain text\nwith lines\t"));
    }
}
