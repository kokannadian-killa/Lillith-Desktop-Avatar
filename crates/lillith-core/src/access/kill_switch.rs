//! The safeword kill switch. When Ko says the safeword ("red", or
//! "OOC: stop"), [`stop_all`] bumps a process-wide epoch. Every command
//! runner started before that moment sees the change and kills its child
//! process tree, and the registered hooks run. The Windows admin broker
//! registers one that tells the elevated helper to exit.
//!
//! Nothing here approves anything: it can only stop work.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

static EPOCH: AtomicU64 = AtomicU64::new(0);
static HOOKS: Mutex<Vec<fn()>> = Mutex::new(Vec::new());

/// Tests that call [`stop_all`] take the write side; every test command run
/// takes the read side, so a test safeword never kills another test's child.
#[cfg(test)]
pub(crate) static TEST_GATE: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// The current stop epoch. A runner remembers it at start and stops its
/// child once [`stopped_since`] says it moved.
pub fn epoch() -> u64 {
    EPOCH.load(Ordering::SeqCst)
}

/// Whether [`stop_all`] ran since `start` (an [`epoch`] value).
pub fn stopped_since(start: u64) -> bool {
    epoch() != start
}

/// Registers a hook [`stop_all`] runs (once per hook fn; duplicates ignored).
pub fn on_stop(hook: fn()) {
    if let Ok(mut h) = HOOKS.lock() {
        if !h.iter().any(|x| std::ptr::fn_addr_eq(*x, hook)) {
            h.push(hook);
        }
    }
}

/// Kills every running command child (they poll the epoch) and runs the
/// stop hooks (the admin helper exits). Returns the new epoch.
pub fn stop_all() -> u64 {
    let e = EPOCH.fetch_add(1, Ordering::SeqCst) + 1;
    let hooks: Vec<fn()> = HOOKS.lock().map(|h| h.clone()).unwrap_or_default();
    for h in hooks {
        h();
    }
    tracing::warn!("safeword: stopped every running command (epoch {e})");
    e
}

/// Whether `text` is the safeword: the whole message is "red" (any case,
/// trailing punctuation ignored) or it starts with "OOC: stop" / "OOC stop".
/// "I read a red book" is not.
pub fn is_safeword(text: &str) -> bool {
    let t = text
        .trim()
        .trim_matches(|c: char| !c.is_alphanumeric() && c != ':')
        .to_lowercase();
    if t == "red" {
        return true;
    }
    let squashed: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    squashed.starts_with("ooc:stop") || squashed.starts_with("oocstop")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safeword_matches_only_the_word() {
        for s in [
            "red",
            "RED",
            " Red! ",
            "red.",
            "OOC: stop",
            "ooc stop please",
            "(OOC: STOP)",
        ] {
            assert!(is_safeword(s), "{s}");
        }
        for s in [
            "",
            "redo",
            "I read a red book",
            "stop",
            "ooc: go on",
            "reddit",
        ] {
            assert!(!is_safeword(s), "{s}");
        }
    }

    #[test]
    fn stop_all_moves_the_epoch() {
        let _g = TEST_GATE.write().unwrap_or_else(|e| e.into_inner());
        let e = epoch();
        assert!(!stopped_since(epoch()));
        stop_all();
        assert!(stopped_since(e));
    }
}
