//! Keyword topic classifier (no model): a note only leaves `inbox/` when
//! the words make the topic clear.

use crate::readers::SourceKind;

/// Topic → telling words. Topics beyond the seed list (`git`, `shell`,
/// `networking`) are only used with `allow_create_topics`.
const TABLE: &[(&str, &[&str])] = &[
    (
        "rust",
        &[
            "rust", "cargo", "rustc", "crate", "crates", "trait", "borrow", "rustup", "clippy",
            "impl", "vec", "struct", "enum",
        ],
    ),
    (
        "python",
        &[
            "python",
            "pip",
            "pypi",
            "venv",
            "virtualenv",
            "pytest",
            "def",
            "import",
            "asyncio",
            "pydoc",
        ],
    ),
    (
        "linux-garuda",
        &[
            "pacman",
            "garuda",
            "arch",
            "systemd",
            "systemctl",
            "journalctl",
            "kernel",
            "aur",
            "paru",
            "yay",
            "grub",
            "mkinitcpio",
            "btrfs",
            "snapper",
        ],
    ),
    (
        "git",
        &[
            "git", "commit", "branch", "rebase", "merge", "checkout", "stash",
        ],
    ),
    (
        "shell",
        &[
            "bash", "zsh", "fish", "shell", "alias", "pipe", "stdin", "stdout", "redirect",
        ],
    ),
    (
        "networking",
        &[
            "tcp", "udp", "dns", "ip", "socket", "port", "ssh", "curl", "http", "route",
        ],
    ),
    (
        "coding",
        &[
            "compile", "compiler", "debug", "function", "variable", "loop", "api", "library",
        ],
    ),
    (
        "tools",
        &["command", "option", "flag", "utility", "usage", "synopsis"],
    ),
];

/// The topic for `text` when the classifier is sure (≥2 hits and strictly
/// ahead of the runner-up), with a hint from the source kind.
pub fn classify(text: &str, kind: SourceKind) -> Option<&'static str> {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric() && c != '-')
        .filter(|w| !w.is_empty())
        .collect();
    let mut scores: Vec<(&'static str, usize)> = TABLE
        .iter()
        .map(|(t, keys)| (*t, words.iter().filter(|w| keys.contains(w)).count()))
        .collect();
    let boost = match kind {
        SourceKind::RustDoc => Some("rust"),
        SourceKind::PythonDoc => Some("python"),
        _ => None,
    };
    if let Some(b) = boost {
        if let Some(s) = scores.iter_mut().find(|(t, _)| *t == b) {
            s.1 += 2;
        }
    }
    scores.sort_by(|a, b| b.1.cmp(&a.1));
    let (best, n) = scores[0];
    let second = scores.get(1).map(|s| s.1).unwrap_or(0);
    (n >= 2 && n > second).then_some(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sure_or_nothing() {
        assert_eq!(
            classify("pacman -Syu with systemd on Garuda", SourceKind::Man),
            Some("linux-garuda")
        );
        assert_eq!(
            classify("Vec is a growable array", SourceKind::RustDoc),
            Some("rust")
        );
        assert_eq!(classify("the weather is nice", SourceKind::Doc), None);
        assert_eq!(
            classify("git and cargo", SourceKind::Doc),
            None,
            "a tie is not sure"
        );
    }
}
