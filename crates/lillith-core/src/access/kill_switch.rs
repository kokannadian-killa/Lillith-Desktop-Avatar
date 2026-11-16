//! The stop switches. Two process-wide epochs:
//!
//! - [`hush`] (the safeword: "red" or "OOC: stop"). She stops talking, her
//!   queued lines are dropped, and anything still waiting for Ko is
//!   cancelled (Soul Contract cards, the model-server wait, an
//!   online-research card). It shuts **nothing** down: running commands,
//!   the Windows admin helper, helper workers, sandbox builds and idle
//!   learning are left alone (Ko, 2026-10-24: "Only remove the shutdown
//!   part; red still stops her talking", then "Just the voice").
//! - [`stop_all`] (Ctrl-C / SIGTERM / app shutdown / Settings "Stop all").
//!   Every command runner started before that moment sees [`epoch`] move and
//!   kills its child process tree, and the registered hooks run. The Windows
//!   admin broker registers one that force-kills the elevated helper
//!   out-of-band (Job / process handle — never waits on the in-flight pipe
//!   lock). A stop also counts as a hush, so everything a safeword cancels is
//!   cancelled too.
//!
//! **Across processes.** The epochs themselves are process-wide statics, so
//! on their own a stop in the detached `lillith-settings` window could not
//! reach the overlay (and the other way round). A process that called
//! [`join_session`] (the apps do at start-up) also folds the per-user stop
//! barrier ([`super::stop_session`]) into [`epoch`] and [`hush_epoch`]:
//! [`stop_all`] appends to it and fsyncs **before** it returns, and every
//! other joined process's runners see the moved epoch and kill their trees;
//! their watcher runs their stop hooks and acknowledges. [`stop_all_confirmed`]
//! waits for those acks and only reports a confirmed stop when every live
//! process said its trees are gone. [`stop_process`] is the old process-local
//! stop (a single bot's Stop). The safeword stays local and never touches the
//! barrier.
//!
//! Nothing here approves anything: it can only stop work.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use super::stop_session::{self, StopOutcome};
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
    /// Whether the cross-process barrier was saved.
    pub barrier: StopBarrier,
}

/// What happened to the cross-process stop barrier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopBarrier {
    /// This process never joined a stop session (process-local stop only,
    /// as before; e.g. library users and unit tests).
    NotJoined,
    /// Appended and fsynced; the new barrier generation.
    Saved(u64),
    /// Joined, but the write failed: other processes were NOT told.
    Failed(String),
    /// A process-local stop on purpose ([`stop_process`]).
    Local,
}

impl StopReport {
    /// The line Lillith says in chat. See [`HelperKill::chat_line`].
    pub fn chat_line(&self) -> String {
        let line = stop_chat_line(self.helper);
        match &self.barrier {
            StopBarrier::Failed(e) => {
                format!("{line} Other Lillith windows were NOT told (stop barrier: {e}).")
            }
            _ => line,
        }
    }
}

/// Joins the per-user cross-process stop session (see the module docs and
/// [`super::stop_session`]). Call once at app start-up.
pub fn join_session() -> std::io::Result<std::path::PathBuf> {
    stop_session::join()
}

/// [`join_session`] at an explicit directory (tests).
pub fn join_session_at(dir: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    stop_session::join_at(dir)
}

/// The current stop epoch. A runner remembers it at start and stops its
/// child once [`stopped_since`] says it moved.
pub fn epoch() -> u64 {
    // The barrier generation only grows (it is a file length), so the sum
    // moves whenever either part moves.
    EPOCH
        .load(Ordering::SeqCst)
        .wrapping_add(stop_session::generation())
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
    // A stop from another process also cancels what the safeword cancels.
    HUSH.load(Ordering::SeqCst)
        .wrapping_add(stop_session::generation())
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
/// stop hooks (the admin helper is force-killed out-of-band). In a joined
/// process the stop barrier is appended and fsynced before this returns, so
/// every other joined Lillith process stops its commands too. Returns a
/// report the chat layer uses for a truthful reply. Ctrl-C / SIGTERM / app
/// shutdown / "Stop all" — the safeword calls [`hush`] instead. Also hushes.
pub fn stop_all() -> StopReport {
    stop_inner(true)
}

/// The process-local stop: like [`stop_all`] but the barrier is not touched,
/// so other Lillith processes keep running (one bot's Stop).
pub fn stop_process() -> StopReport {
    stop_inner(false)
}

fn stop_inner(global: bool) -> StopReport {
    HUSH.fetch_add(1, Ordering::SeqCst);
    EPOCH.fetch_add(1, Ordering::SeqCst);
    let barrier = if !global {
        StopBarrier::Local
    } else if !stop_session::joined() {
        StopBarrier::NotJoined
    } else {
        match stop_session::persist("stop_all") {
            Ok(g) => StopBarrier::Saved(g),
            Err(e) => {
                tracing::error!(
                    "stop barrier not saved: other Lillith processes were not told: {e}"
                );
                StopBarrier::Failed(e.to_string())
            }
        }
    };
    let helper = run_hooks();
    let e = epoch();
    tracing::warn!(
        "stop: stopped every running command (epoch {e}, helper={helper:?}, barrier={barrier:?})"
    );
    StopReport {
        epoch: e,
        helper,
        barrier,
    }
}

/// [`stop_all`], then waits up to `deadline` until every joined Lillith
/// process confirmed its task trees are gone. Blocking: call it off the UI
/// thread. Only [`StopOutcome::Quiescent`] is a confirmed stop.
pub fn stop_all_confirmed(deadline: std::time::Duration) -> (StopReport, StopOutcome) {
    let r = stop_all();
    let out = match &r.barrier {
        StopBarrier::Saved(g) => stop_session::confirm(*g, deadline),
        StopBarrier::Failed(e) => StopOutcome::LocalOnly {
            reason: e.clone(),
            local: stop_session::local_quiescence(deadline),
        },
        StopBarrier::NotJoined | StopBarrier::Local => StopOutcome::LocalOnly {
            reason: "no stop session joined".into(),
            local: stop_session::local_quiescence(deadline),
        },
    };
    (r, out)
}

/// Another process moved the barrier: run this process's stop hooks (the
/// epochs already moved through the barrier generation).
pub(crate) fn run_foreign_stop_hooks() -> HelperKill {
    run_hooks()
}

fn run_hooks() -> HelperKill {
    let hooks: Vec<fn() -> HelperKill> = HOOKS.lock().map(|h| h.clone()).unwrap_or_default();
    let mut helper = HelperKill::Idle;
    for h in hooks {
        let r = h();
        // Prefer the strongest confirmed outcome across hooks.
        helper = merge_helper(helper, r);
    }
    helper
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
            barrier: StopBarrier::NotJoined,
        }
        .chat_line()
        .contains("helper is gone"));
        assert!(!StopReport {
            epoch: 1,
            helper: HelperKill::Unconfirmed,
            barrier: StopBarrier::NotJoined,
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
    fn an_unjoined_process_stays_process_local() {
        let _g = TEST_GATE.write().unwrap_or_else(|e| e.into_inner());
        let e = epoch();
        let r = stop_all();
        assert_eq!(r.barrier, StopBarrier::NotJoined);
        assert!(stopped_since(e));
        let e = epoch();
        assert_eq!(stop_process().barrier, StopBarrier::Local);
        assert!(stopped_since(e));
    }

    #[test]
    fn a_failed_barrier_says_so_in_chat() {
        let r = StopReport {
            epoch: 1,
            helper: HelperKill::Idle,
            barrier: StopBarrier::Failed("disk full".into()),
        };
        assert!(r.chat_line().contains("NOT told"));
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
