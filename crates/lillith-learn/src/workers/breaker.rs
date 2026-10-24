//! Circuit breakers and the error memory (§12 "Advanced error handling").
//!
//! Keys are `domain:<host>` or `source:<name>`. `breaker_failures` failures
//! in a row open a breaker for [`super::BREAKER_OPEN`] (30 min); after that
//! it is half-open and lets exactly one probe through: success closes it,
//! failure opens it again. Time is injected (`Instant`) so tests need no
//! sleeping.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// One breaker's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    Closed,
    Open,
    /// The open time is over; one probe may go.
    HalfOpen,
}

#[derive(Debug, Clone, Default)]
struct Entry {
    fails: u32,
    opened: Option<Instant>,
    probing: bool,
}

/// Per-domain / per-source circuit breakers.
#[derive(Debug, Clone)]
pub struct Breakers {
    failures: u32,
    open_for: Duration,
    map: HashMap<String, Entry>,
}

/// `domain:<host>` (lowercase).
pub fn domain_key(host: &str) -> String {
    format!("domain:{}", host.to_ascii_lowercase())
}

/// `source:<name>`.
pub fn source_key(source: &str) -> String {
    format!("source:{source}")
}

impl Breakers {
    pub fn new(failures: u32, open_for: Duration) -> Self {
        Self {
            failures: failures.max(1),
            open_for,
            map: HashMap::new(),
        }
    }

    pub fn state(&self, key: &str, now: Instant) -> BreakerState {
        match self.map.get(key).and_then(|e| e.opened) {
            None => BreakerState::Closed,
            Some(t) if now.saturating_duration_since(t) < self.open_for => BreakerState::Open,
            Some(_) => BreakerState::HalfOpen,
        }
    }

    /// Whether a request may go now. A half-open breaker lets one probe go
    /// and refuses the rest until that probe reports.
    pub fn allow(&mut self, key: &str, now: Instant) -> bool {
        match self.state(key, now) {
            BreakerState::Closed => true,
            BreakerState::Open => false,
            BreakerState::HalfOpen => {
                let e = self.map.entry(key.to_string()).or_default();
                if e.probing {
                    false
                } else {
                    e.probing = true;
                    true
                }
            }
        }
    }

    /// Whether a request could go (no probe is taken).
    pub fn would_allow(&self, key: &str, now: Instant) -> bool {
        match self.state(key, now) {
            BreakerState::Closed => true,
            BreakerState::Open => false,
            BreakerState::HalfOpen => !self.map.get(key).is_some_and(|e| e.probing),
        }
    }

    pub fn success(&mut self, key: &str) {
        self.map.remove(key);
    }

    /// Records a failure; returns `true` when this opened the breaker.
    pub fn failure(&mut self, key: &str, now: Instant) -> bool {
        let n = self.failures;
        let e = self.map.entry(key.to_string()).or_default();
        if e.probing {
            // The half-open probe failed: open again for the full time.
            e.probing = false;
            e.opened = Some(now);
            e.fails = n;
            return true;
        }
        e.fails += 1;
        if e.fails >= n && e.opened.is_none() {
            e.opened = Some(now);
            return true;
        }
        false
    }
}

/// One remembered failure kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorStat {
    pub key: String,
    pub kind: String,
    pub count: u32,
    pub sample: String,
}

/// Where the supervisor's failures also go (the parent wires this to the
/// memory's `record_error`; workers never see it).
pub trait ErrorSink {
    fn record_error(&self, text: &str, context: &str);
}

/// Per-domain / per-source error record. Every failure is counted by
/// `(key, kind)` and feeds the matching breaker, so repeats are avoided.
#[derive(Debug, Clone, Default)]
pub struct ErrorMemory {
    stats: Vec<ErrorStat>,
}

impl ErrorMemory {
    pub fn record(&mut self, key: &str, kind: &str, sample: &str) -> u32 {
        let sample: String = sample.chars().take(200).collect();
        if let Some(s) = self
            .stats
            .iter_mut()
            .find(|s| s.key == key && s.kind == kind)
        {
            s.count += 1;
            s.sample = sample;
            return s.count;
        }
        self.stats.push(ErrorStat {
            key: key.to_string(),
            kind: kind.to_string(),
            count: 1,
            sample,
        });
        1
    }

    pub fn count(&self, key: &str, kind: &str) -> u32 {
        self.stats
            .iter()
            .find(|s| s.key == key && s.kind == kind)
            .map_or(0, |s| s.count)
    }

    pub fn stats(&self) -> &[ErrorStat] {
        &self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaker_opens_after_breaker_failures_for_30_min_then_half_open_probe() {
        let t0 = Instant::now();
        let mut b = Breakers::new(3, super::super::BREAKER_OPEN);
        let k = domain_key("Docs.Example.org");
        assert_eq!(k, "domain:docs.example.org");
        assert!(!b.failure(&k, t0));
        assert!(!b.failure(&k, t0));
        assert!(b.allow(&k, t0), "still closed after 2 of 3");
        assert!(b.failure(&k, t0), "third failure in a row opens it");
        assert_eq!(b.state(&k, t0), BreakerState::Open);
        assert!(!b.allow(&k, t0 + Duration::from_secs(29 * 60 + 59)));
        let t1 = t0 + Duration::from_secs(30 * 60);
        assert_eq!(b.state(&k, t1), BreakerState::HalfOpen);
        assert!(b.allow(&k, t1), "one probe");
        assert!(!b.allow(&k, t1), "only one probe");
        // Probe fails: open for another 30 min.
        assert!(b.failure(&k, t1));
        assert!(!b.allow(&k, t1 + Duration::from_secs(60)));
        let t2 = t1 + Duration::from_secs(30 * 60);
        assert!(b.allow(&k, t2));
        b.success(&k);
        assert_eq!(b.state(&k, t2), BreakerState::Closed);
        assert!(b.allow(&k, t2));
    }

    #[test]
    fn success_resets_the_run_of_failures() {
        let t0 = Instant::now();
        let mut b = Breakers::new(3, super::super::BREAKER_OPEN);
        let k = source_key("local");
        b.failure(&k, t0);
        b.failure(&k, t0);
        b.success(&k);
        b.failure(&k, t0);
        b.failure(&k, t0);
        assert_eq!(b.state(&k, t0), BreakerState::Closed, "not 3 in a row");
    }

    #[test]
    fn error_memory_counts_per_key_and_kind() {
        let mut m = ErrorMemory::default();
        assert_eq!(m.record("domain:a.org", "io", "reset"), 1);
        assert_eq!(m.record("domain:a.org", "io", "reset again"), 2);
        assert_eq!(m.record("domain:a.org", "tls", "bad cert"), 1);
        assert_eq!(m.record("domain:b.org", "io", "x"), 1);
        assert_eq!(m.count("domain:a.org", "io"), 2);
        assert_eq!(m.stats().len(), 3);
    }
}
