//! Error signatures: the same failure with a different PID, path, address
//! or line number hashes to the same signature, so a known fix is found.

/// Normalises error text: lowercase, numbers / hex / paths / quoted strings
/// squashed, whitespace collapsed, capped at 400 chars.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(400));
    for tok in text.split_whitespace() {
        let t = tok.to_lowercase();
        let t = if t.contains('/') || t.contains('\\') {
            "<path>".to_string()
        } else if t.starts_with("0x") {
            "#".to_string()
        } else if (t.starts_with('"') && t.ends_with('"') && t.len() > 1)
            || (t.starts_with('\'') && t.ends_with('\'') && t.len() > 1)
        {
            "<str>".to_string()
        } else {
            let mut s = String::new();
            let mut in_digits = false;
            for c in t.chars() {
                if c.is_ascii_digit() {
                    if !in_digits {
                        s.push('#');
                    }
                    in_digits = true;
                } else {
                    in_digits = false;
                    s.push(c);
                }
            }
            s
        };
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&t);
        if out.len() >= 400 {
            let mut cut = 400;
            while !out.is_char_boundary(cut) {
                cut -= 1;
            }
            out.truncate(cut);
            break;
        }
    }
    out
}

/// FNV-1a 64-bit (stable across builds and platforms, unlike `DefaultHasher`).
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Stable signature (16 hex chars) of an error message.
pub fn signature(text: &str) -> String {
    format!("{:016x}", fnv1a(normalize(text).as_bytes()))
}

/// Whether a fix worked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixOutcome {
    Worked,
    Failed,
}

impl FixOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            FixOutcome::Worked => "worked",
            FixOutcome::Failed => "failed",
        }
    }
}

/// Result of recording one failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorRecord {
    pub signature: String,
    /// How many times this signature has been seen (including this one).
    pub count: u32,
    /// A fix that worked before, if any.
    pub known_fix: Option<String>,
}

/// What is remembered about one error signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownFix {
    pub signature: String,
    /// The fix that worked (more successes than failures), if any.
    pub fix: Option<String>,
    pub successes: u32,
    pub failures: u32,
    /// Times this error has been seen.
    pub recurrences: u32,
    /// Fixes already tried that did not work (don't repeat them).
    pub failed_fixes: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_error_different_details_same_signature() {
        let a =
            "llama-server crashed (pid 4242) at 0x7ffde12 reading /home/ko/models/a.gguf line 17";
        let b = "llama-server crashed (pid 99) at 0xdeadbeef reading /tmp/b.gguf line 3";
        assert_eq!(signature(a), signature(b));
        assert_ne!(signature(a), signature("voice daemon: piper not found"));
        assert_eq!(signature(a).len(), 16);
        assert_eq!(
            normalize("Error  E0425   at 12:7"),
            "error e#   at #:#".replace("   ", " ")
        );
    }

    #[test]
    fn fnv_is_stable() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn normalize_is_bounded() {
        let long = "x".repeat(5000);
        assert!(normalize(&long).len() <= 400);
        let multi = "é".repeat(500);
        assert!(normalize(&multi).len() <= 400);
    }
}
