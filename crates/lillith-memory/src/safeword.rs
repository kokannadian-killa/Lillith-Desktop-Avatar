//! Safeword handling for the memory loop.
//!
//! The canonical check lives in `lillith_core::access::kill_switch`
//! (the whole message is "red", or it starts with "OOC: stop"). Memory is
//! stricter about what it *stores*: a message that contains the safeword
//! anywhere ("ok... red", "(ooc: stop) sorry") is never written either.

/// Whether `text` is (or carries) the out-of-character safeword: the
/// canonical kill-switch check, or "OOC: stop" / "OOC stop" anywhere in it.
/// When this is true the learning loop stops immediately and stores nothing.
pub fn is_safeword(text: &str) -> bool {
    if lillith_core::access::kill_switch::is_safeword(text) {
        return true;
    }
    let squashed: String = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    squashed.contains("oocstop")
}

/// Whether `text` contains the safeword "red" as a standalone word anywhere
/// ("wait, red", "RED!!"). Such messages are never stored, even if they are
/// not a full stop for the chat ("I read a red book" is not stored either:
/// we err on the side of forgetting).
pub fn mentions_safeword(text: &str) -> bool {
    is_safeword(text)
        || text
            .split(|c: char| !c.is_alphanumeric())
            .any(|w| w.eq_ignore_ascii_case("red"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_and_embedded_safewords() {
        for s in ["red", "RED", " Red! ", "OOC: stop", "(ooc stop)"] {
            assert!(is_safeword(s), "{s}");
        }
        for s in ["ok... ooc: stop", "sorry OOC:STOP please", "hey (OOC stop)"] {
            assert!(is_safeword(s), "embedded {s}");
        }
        for s in [
            "",
            "redo",
            "reddit",
            "stop",
            "ooc: go on",
            "I read a red book",
        ] {
            assert!(!is_safeword(s), "{s}");
        }
    }

    #[test]
    fn mentions_red_as_a_word() {
        assert!(mentions_safeword("wait, red"));
        assert!(mentions_safeword("I read a red book"));
        assert!(!mentions_safeword("I shredded it on reddit"));
        assert!(!mentions_safeword("hello"));
    }
}
