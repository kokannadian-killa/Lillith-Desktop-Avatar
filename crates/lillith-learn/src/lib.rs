//! Lillith's self-learning, milestone P1 (docs/SELF_LEARNING.md).
//!
//! While the desktop is idle she reads one small chunk of an installed doc
//! (man / info pages, `/usr/share/doc`, the local Rust and Python html
//! docs), asks the local model for at most three short facts, and keeps
//! them as private markdown notes under `knowledge/<topic>/`, indexed in
//! the memory graph. Everything is local: the idle loop never fetches from
//! the network, installs anything or runs a program; the only connection
//! is the loopback `llama-server`, and only through the shared
//! [`LlmGate`] that chat always wins.
//!
//! - [`scheduler`]: Off → Waiting → Running → Paused, duty cycle and
//!   budgets; the safeword drops a step but never rests her (pure, clock injected).
//! - [`paths`] + [`readers`]: confined, denylisted, in-process doc readers.
//! - [`store`]: the note files (0700/0600, atomic, single writer).
//! - [`extract`], [`classify`], [`dedupe`]: one chunk → notes.
//! - [`learner`]: one step (pick, read, summarize, dedupe, write, link).
//! - [`runner`]: the background thread the overlay starts.
//! - [`online`]: user-gated, Tor-only online research trips (P5, off by
//!   default; only ever started by Ko, never by the idle loop).
//! - [`workers`]: the helper-worker supervisor (§12, P7 first slice; off
//!   with online research, never started by the idle loop).

pub mod audit;
pub mod classify;
pub mod dedupe;
pub mod extract;
pub mod hygiene;
pub mod learner;
pub mod online;
pub mod paths;
pub mod readers;
pub mod runner;
pub mod scheduler;
pub mod sha256;
pub mod store;
pub mod summarize;
pub mod workers;

pub use learner::{Learner, StepReport};
pub use lillith_core::config::LearnConfig;
pub use lillith_core::llm_gate::{GateGuard, LlmGate};
pub use runner::{LearnHandle, Sensors};
pub use scheduler::{Decision, Limits, Scheduler, State, WaitReason};
pub use store::{Note, NoteStore, WebSource};
pub use summarize::{FakeSummarizer, LlmSummarizer, SumError, Summarizer, Summary};

/// Learner requests never ask for more than this (§1).
pub const MAX_TOKENS: u32 = 256;
/// Learner sampling temperature.
pub const TEMPERATURE: f32 = 0.2;
/// Whole-request timeout of a learner request.
pub const REQUEST_TIMEOUT_S: u64 = 60;
/// Bytes of a doc read per step.
pub const CHUNK_BYTES: usize = 2048;
/// Largest note file.
pub const MAX_NOTE_BYTES: usize = 8 * 1024;

/// FNV-1a 64 of `bytes` as `fnv1a:<hex>` (source / content hashes).
pub fn fnv1a(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("fnv1a:{h:016x}")
}

/// `true` when both memory and learning are on: the only case anything of
/// the learner runs, opens the DB or touches the knowledge folder.
pub fn should_run(cfg: &lillith_core::config::Config) -> bool {
    cfg.learn.enabled && cfg.memory.enabled
}

/// Seconds since the Unix epoch.
pub fn now_s() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_is_stable() {
        assert_eq!(fnv1a(b""), "fnv1a:cbf29ce484222325");
        assert_ne!(fnv1a(b"a"), fnv1a(b"b"));
    }

    #[test]
    fn learning_needs_memory() {
        let mut c = lillith_core::config::Config::default();
        assert!(c.learn.enabled, "[learn] enabled defaults to true");
        assert!(should_run(&c));
        c.memory.enabled = false;
        assert!(!should_run(&c));
        c.memory.enabled = true;
        c.learn.enabled = false;
        assert!(!should_run(&c));
    }
}
