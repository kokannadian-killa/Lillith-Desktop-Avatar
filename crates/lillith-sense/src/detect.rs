//! Pure, clock-free edge detectors the monitor is built from. Time is
//! passed in as seconds (`now`) so tests drive them with a fake clock.

use std::collections::HashMap;

/// Rising/falling edge of a level detector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Rise,
    Fall,
}

/// Two-threshold detector with sustain times:
/// goes **high** once the value has stayed `≥ high` for `rise_s`, and only
/// re-arms (goes **low**) after it has stayed `< low` for `fall_s`. Values
/// between `low` and `high` hold the current state, so a reading hovering
/// around the threshold cannot chatter.
#[derive(Debug, Clone, PartialEq)]
pub struct Hysteresis {
    pub high: f32,
    pub low: f32,
    pub rise_s: f64,
    pub fall_s: f64,
    is_high: bool,
    since: Option<f64>,
}

impl Hysteresis {
    pub fn new(high: f32, low: f32, rise_s: f32, fall_s: f32) -> Self {
        Self {
            high,
            low: low.min(high),
            rise_s: rise_s.max(0.0) as f64,
            fall_s: fall_s.max(0.0) as f64,
            is_high: false,
            since: None,
        }
    }

    pub fn is_high(&self) -> bool {
        self.is_high
    }

    /// Feeds a sample taken at `now` (s).
    pub fn update(&mut self, now: f64, value: f32) -> Option<Edge> {
        let toward = if self.is_high {
            value < self.low
        } else {
            value >= self.high
        };
        if !toward {
            self.since = None;
            return None;
        }
        let since = *self.since.get_or_insert(now);
        let need = if self.is_high {
            self.fall_s
        } else {
            self.rise_s
        };
        if now - since + 1e-9 >= need {
            self.is_high = !self.is_high;
            self.since = None;
            Some(if self.is_high { Edge::Rise } else { Edge::Fall })
        } else {
            None
        }
    }
}

/// Debounced boolean: the first sample sets the baseline silently; a new
/// value is committed (and returned) only after it held for `hold_s`.
#[derive(Debug, Clone, PartialEq)]
pub struct Debounce {
    pub hold_s: f64,
    value: Option<bool>,
    pending: Option<(bool, f64)>,
}

impl Debounce {
    pub fn new(hold_s: f32) -> Self {
        Self {
            hold_s: hold_s.max(0.0) as f64,
            value: None,
            pending: None,
        }
    }

    pub fn value(&self) -> Option<bool> {
        self.value
    }

    /// Returns `Some(new)` when a change has been committed.
    pub fn update(&mut self, now: f64, v: bool) -> Option<bool> {
        let Some(cur) = self.value else {
            self.value = Some(v);
            return None;
        };
        if v == cur {
            self.pending = None;
            return None;
        }
        let since = match self.pending {
            Some((p, t)) if p == v => t,
            _ => {
                self.pending = Some((v, now));
                now
            }
        };
        if now - since + 1e-9 >= self.hold_s {
            self.value = Some(v);
            self.pending = None;
            Some(v)
        } else {
            None
        }
    }
}

/// Per-event-name minimum spacing.
#[derive(Debug, Clone, Default)]
pub struct Cooldowns {
    last: HashMap<String, f64>,
}

impl Cooldowns {
    /// `true` (and records `now`) if `name` last passed ≥ `cooldown_s` ago.
    pub fn ready(&mut self, name: &str, now: f64, cooldown_s: f64) -> bool {
        match self.last.get(name) {
            Some(&t) if now - t < cooldown_s => false,
            _ => {
                self.last.insert(name.to_string(), now);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hysteresis_sustain_and_band() {
        // cpu.high defaults: 72 % for 10 s, re-arm below 62 % for 10 s, 2 s polls.
        let mut h = Hysteresis::new(72.0, 62.0, 10.0, 10.0);
        let mut t = 0.0;
        let mut feed = |v: f32| {
            let e = h.update(t, v);
            t += 2.0;
            e
        };
        for _ in 0..5 {
            assert_eq!(feed(90.0), None); // 0..8 s above
        }
        assert_eq!(feed(90.0), Some(Edge::Rise)); // 10 s sustained
        assert_eq!(feed(95.0), None, "no re-fire while high");
        assert_eq!(feed(70.0), None, "inside the band holds high");
        for _ in 0..5 {
            assert_eq!(feed(40.0), None);
        }
        assert_eq!(feed(40.0), Some(Edge::Fall));
        // A short spike does not fire.
        assert_eq!(feed(99.0), None);
        assert_eq!(feed(50.0), None);
        assert_eq!(feed(99.0), None, "sustain restarted");
        assert!(!h.is_high());
    }

    #[test]
    fn hysteresis_zero_sustain_fires_immediately() {
        let mut h = Hysteresis::new(10.0, 5.0, 0.0, 0.0);
        assert_eq!(h.update(0.0, 11.0), Some(Edge::Rise));
        assert_eq!(h.update(1.0, 7.0), None);
        assert_eq!(h.update(2.0, 4.0), Some(Edge::Fall));
    }

    #[test]
    fn debounce_baseline_and_hold() {
        let mut d = Debounce::new(4.0);
        assert_eq!(d.update(0.0, true), None, "baseline is silent");
        assert_eq!(d.update(2.0, false), None);
        assert_eq!(d.update(4.0, true), None, "flap cancels");
        assert_eq!(d.update(6.0, false), None);
        assert_eq!(d.update(8.0, false), None);
        assert_eq!(d.update(10.0, false), Some(false));
        assert_eq!(d.value(), Some(false));
        assert_eq!(d.update(12.0, false), None);
    }

    #[test]
    fn cooldowns() {
        let mut c = Cooldowns::default();
        assert!(c.ready("a", 0.0, 10.0));
        assert!(!c.ready("a", 5.0, 10.0));
        assert!(c.ready("b", 5.0, 10.0));
        assert!(c.ready("a", 10.0, 10.0));
    }
}
