//! Moods, mischief meter and relationship warmth (PLAN.md §3.1, layer 4).

use serde::{Deserialize, Serialize};

/// Lillith's current mood (README-SP).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Mood {
    #[default]
    Playful,
    Happy,
    Mischievous,
    Alert,
    Sleepy,
}

/// Mischief rises while she is ignored and triggers a playful interruption
/// at a threshold. TODO(M3): tune rates against README-SP.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mischief {
    pub level: f32,
    pub rate_per_s: f32,
    pub threshold: f32,
}

impl Default for Mischief {
    fn default() -> Self {
        Self {
            level: 0.0,
            rate_per_s: 0.01,
            threshold: 1.0,
        }
    }
}

impl Mischief {
    /// Advances the meter by `dt` seconds of being ignored. Returns `true`
    /// (and resets) when the threshold is crossed.
    pub fn tick(&mut self, dt: f32) -> bool {
        self.level += self.rate_per_s * dt;
        if self.level >= self.threshold {
            self.level = 0.0;
            true
        } else {
            false
        }
    }

    /// Any interaction calms her down.
    pub fn reset(&mut self) {
        self.level = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mischief_fires_after_threshold() {
        let mut m = Mischief::default();
        let fired = (0..200).any(|_| m.tick(1.0));
        assert!(fired);
        assert!(m.level < m.threshold);
    }
}
