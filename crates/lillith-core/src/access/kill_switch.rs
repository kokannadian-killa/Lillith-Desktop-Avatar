//! The stop switches. Two process-wide epochs:
//!
//! - [`hush`] (the safeword: "red" or "OOC: stop"). She stops talking, her
//!   queued lines are dropped, and anything still waiting for Ko is
//!   cancelled (Soul Contract cards, the model-server wait, an
//!   online-research card). It shuts **nothing** down: running commands,
//!   the Windows admin helper, helper workers, sandbox builds and idle
//!   learning are left alone (Ko, 2026-10-24: "Only remove the shutdown
//!   part; red still stops her talking", then "Just the voice").
//! - [`stop_all`] (Ctrl-C / SIGTERM / app shutdown). Every command runner
//!   started before that moment sees [`epoch`] move and kills its child
//!   process tree, and the registered hooks run. The Windows admin broker
//!   registers one that force-kills the elevated helper out-of-band (Job /
//!   process handle — never waits on the in-flight pipe lock). A stop also
//!   counts as a hush, so everything a safeword cancels is cancelled too.
//!
//! Nothing here approves anything: it can only stop work.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use super::winjob::{stop_chat_line, HelperKill};

static EPOCH: AtomicU64 = AtomicU64::new(0);
static HUSH: AtomicU64 = AtomicU64::new(0);
static HOOKS: Mutex<Vec<fn() -> HelperKill>> = Mutex::new(Vec::new());

/// Tests that call [`stop_all`] take the write side; every test command run
/// takes the read side, so a test safeword never kills another test's child.
#[cfg(test)]
pub(crate) static TEST_GATE: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// What [`stop_all`] did, for a truthful chat reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopReport {
    pub epoch: u64,
    pub helper: HelperKill,
}

impl StopReport {
    /// The line Lillith says in chat. See [`HelperKill::chat_line`].
    pub fn chat_line(&self) -> String {
        stop_chat_line(self.helper)
    }
}

/// The current stop epoch. A runner remembers it at start and stops its
/// child once [`stopped_since`] says it moved.
pub fn epoch() -> u64 {
    EPOCH.load(Ordering::SeqCst)
}

/// Whether [`stop_all`] ran since `start` (an [`epoch`] value).
pub fn stopped_since(start: u64) -> bool {
    epoch() != start
}

/// The safeword: bumps the hush epoch only. Her talking, queued lines and
/// anything waiting for Ko (cards, waits) stop; running commands, the admin
/// helper, helper workers and learning are NOT stopped ([`epoch`] doesn't
/// move).
pub fn hush() -> u64 {
    let e = HUSH.fetch_add(1, Ordering::SeqCst) + 1;
    tracing::warn!("safeword: she stops talking (hush epoch {e}); nothing is killed");
    e
}

/// The current hush epoch (moved by [`hush`] and by [`stop_all`]).
pub fn hush_epoch() -> u64 {
    HUSH.load(Ordering::SeqCst)
}

/// Whether the safeword ([`hush`]) or a [`stop_all`] happened since `start`
/// (a [`hush_epoch`] value). What cards and waits that "red" cancels check.
pub fn hushed_since(start: u64) -> bool {
    hush_epoch() != start
}

/// Registers a hook [`stop_all`] runs (once per hook fn; duplicates ignored).
/// The hook's [`HelperKill`] feeds the chat line.
pub fn on_stop(hook: fn() -> HelperKill) {
    if let Ok(mut h) = HOOKS.lock() {
        if !h.iter().any(|x| std::ptr::fn_addr_eq(*x, hook)) {
            h.push(hook);
        }
    }
}

/// Test-only: drop every stop hook so unit tests do not leak into each other.
#[cfg(test)]
pub(crate) fn clear_hooks_for_test() {
    if let Ok(mut h) = HOOKS.lock() {
        h.clear();
    }
}

/// Kills every running command child (they poll the epoch) and runs the
/// stop hooks (the admin helper is force-killed out-of-band). Returns a
/// report the chat layer uses for a truthful reply. Ctrl-C / SIGTERM / app
/// shutdown only — the safeword calls [`hush`] instead. Also hushes.
pub fn stop_all() -> StopReport {
    HUSH.fetch_add(1, Ordering::SeqCst);
    let e = EPOCH.fetch_add(1, Ordering::SeqCst) + 1;
    let hooks: Vec<fn() -> HelperKill> = HOOKS.lock().map(|h| h.clone()).unwrap_or_default();
    let mut helper = HelperKill::Idle;
    for h in hooks {
        let r = h();
        // Prefer the strongest confirmed outcome across hooks.
        helper = merge_helper(helper, r);
    }
    tracing::warn!("stop: stopped every running command (epoch {e}, helper={helper:?})");
    StopReport { epoch: e, helper }
}

fn merge_helper(a: HelperKill, b: HelperKill) -> HelperKill {
    use HelperKill::*;
    match (a, b) {
        (Gone, _) | (_, Gone) => Gone,
        (Failed, _) | (_, Failed) => Failed,
        (Unconfirmed, _) | (_, Unconfirmed) => Unconfirmed,
        (Idle, Idle) => Idle,
    }
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AO};
    use std::sync::{Arc, Barrier, Mutex as StdMutex};
    use std::time::{Duration, Instant};

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
        let h = hush_epoch();
        assert!(!stopped_since(epoch()));
        stop_all();
        assert!(stopped_since(e));
        assert!(hushed_since(h), "a stop cancels what the safeword cancels");
    }

    #[test]
    fn the_safeword_hush_never_moves_the_kill_epoch_or_runs_hooks() {
        let _g = TEST_GATE.write().unwrap_or_else(|e| e.into_inner());
        clear_hooks_for_test();
        static RAN: AtomicUsize = AtomicUsize::new(0);
        fn hook() -> HelperKill {
            RAN.fetch_add(1, AO::SeqCst);
            HelperKill::Gone
        }
        on_stop(hook);
        let e = epoch();
        let h = hush_epoch();
        hush();
        assert!(hushed_since(h));
        assert!(!stopped_since(e), "no command is killed");
        assert_eq!(RAN.load(AO::SeqCst), 0, "the admin helper is not killed");
        clear_hooks_for_test();
    }

    #[test]
    fn stop_report_chat_line_follows_helper_state() {
        assert!(StopReport {
            epoch: 1,
            helper: HelperKill::Gone,
        }
        .chat_line()
        .contains("helper is gone"));
        assert!(!StopReport {
            epoch: 1,
            helper: HelperKill::Unconfirmed,
        }
        .chat_line()
        .contains("helper is gone"));
    }

    /// A stop hook that must not wait on a lock held by an "in-flight"
    /// command. Mirrors the broker: force-kill uses a separate path.
    #[test]
    fn stop_hook_does_not_block_on_inflight_session_lock() {
        let _g = TEST_GATE.write().unwrap_or_else(|e| e.into_inner());
        clear_hooks_for_test();

        // Simulate the old bug: session lock held during a long read.
        static SESSION: StdMutex<()> = StdMutex::new(());
        static KILL_RAN: AtomicBool = AtomicBool::new(false);

        fn hook() -> HelperKill {
            // Out-of-band: never touch SESSION.
            KILL_RAN.store(true, AO::SeqCst);
            HelperKill::Gone
        }

        on_stop(hook);

        let barrier = Arc::new(Barrier::new(2));
        let b2 = barrier.clone();
        let holder = std::thread::spawn(move || {
            let _guard = SESSION.lock().unwrap();
            b2.wait(); // signal: lock held
            std::thread::sleep(Duration::from_millis(400));
            // lock released on drop
        });
        barrier.wait(); // lock is held

        let start = Instant::now();
        let report = stop_all();
        let elapsed = start.elapsed();

        assert!(KILL_RAN.load(AO::SeqCst), "kill hook must have run");
        assert_eq!(report.helper, HelperKill::Gone);
        // Must finish while the session lock is still held (~400ms).
        assert!(
            elapsed < Duration::from_millis(300),
            "stop blocked on session lock: {elapsed:?}"
        );
        holder.join().unwrap();
        clear_hooks_for_test();
    }

    #[test]
    fn merge_prefers_gone_over_idle() {
        assert_eq!(
            merge_helper(HelperKill::Idle, HelperKill::Gone),
            HelperKill::Gone
        );
        assert_eq!(
            merge_helper(HelperKill::Unconfirmed, HelperKill::Gone),
            HelperKill::Gone
        );
    }

    // Keep AtomicUsize referenced in case future tests need call counts.
    #[allow(dead_code)]
    static _CALLS: AtomicUsize = AtomicUsize::new(0);
}
