//! Moods, mischief meter and relationship warmth (PLAN.md §3.1, layer 4).
//!
//! Everything advances on `tick(dt)` (fake clock in tests):
//!
//! - [`MoodState`]: the current [`Mood`] with a strength that decays
//!   exponentially; once it fades below [`MoodState::FADE_BELOW`] she drifts
//!   back to her *baseline* mood (Playful normally, Sleepy while drowsy or
//!   asleep, Mischievous while the mischief meter is high).
//! - [`Mischief`]: two parts. `level` is teasing energy: every poke (click,
//!   drag) adds to it and it decays over time; at [`MischiefConfig::high`]
//!   mischievous reactions unlock. `boredom` grows while she is ignored
//!   (README-SP: "grows when ignored, causes playful interruptions") and
//!   fires an interruption when it reaches 1.0; any interaction resets it.

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

/// Mood with a decaying strength.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoodState {
    mood: Mood,
    strength: f32,
    baseline: Mood,
    /// Strength e-folding time (s).
    pub tau_s: f32,
}

impl Default for MoodState {
    fn default() -> Self {
        Self::new(45.0)
    }
}

impl MoodState {
    /// Below this strength a non-baseline mood gives way to the baseline.
    pub const FADE_BELOW: f32 = 0.25;

    pub fn new(tau_s: f32) -> Self {
        Self {
            mood: Mood::Playful,
            strength: 0.5,
            baseline: Mood::Playful,
            tau_s: tau_s.max(0.1),
        }
    }

    pub fn mood(&self) -> Mood {
        self.mood
    }

    pub fn strength(&self) -> f32 {
        self.strength
    }

    pub fn baseline(&self) -> Mood {
        self.baseline
    }

    /// Feels `mood` at `strength` (0..=1). Returns `true` if the mood changed.
    /// The same mood only ever gets stronger.
    pub fn feel(&mut self, mood: Mood, strength: f32) -> bool {
        let strength = strength.clamp(0.0, 1.0);
        if mood == self.mood {
            self.strength = self.strength.max(strength);
            return false;
        }
        self.mood = mood;
        self.strength = strength;
        true
    }

    /// Sets the mood she falls back to. If she is in the old baseline (no
    /// active feeling) or the current mood is weak she switches right away. Returns `true` if the mood changed.
    pub fn set_baseline(&mut self, mood: Mood) -> bool {
        let was_baseline = self.mood == self.baseline;
        self.baseline = mood;
        if self.mood != mood && (was_baseline || self.strength < Self::FADE_BELOW * 2.0) {
            self.mood = mood;
            self.strength = 0.5;
            return true;
        }
        false
    }

    /// Decays the strength; returns `true` if she fell back to the baseline.
    pub fn tick(&mut self, dt: f32) -> bool {
        self.strength *= (-dt.max(0.0) / self.tau_s).exp();
        if self.mood != self.baseline && self.strength < Self::FADE_BELOW {
            self.mood = self.baseline;
            self.strength = 0.5;
            return true;
        }
        if self.mood == self.baseline {
            // The baseline never fades out completely.
            self.strength = self.strength.max(0.5);
        }
        false
    }
}

/// Mischief tunables.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MischiefConfig {
    /// Level added per poke (click/drag).
    pub poke: f32,
    /// Level e-folding time (s).
    pub decay_tau_s: f32,
    /// Level at which mischievous reactions unlock.
    pub high: f32,
    /// Seconds of being ignored until a playful interruption.
    pub boredom_s: f32,
}

impl Default for MischiefConfig {
    fn default() -> Self {
        Self {
            poke: 0.15,
            decay_tau_s: 60.0,
            high: 0.7,
            boredom_s: 150.0,
        }
    }
}

/// The mischief meter.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Mischief {
    pub cfg: MischiefConfig,
    /// Teasing energy 0..=1 (pokes add, time decays).
    pub level: f32,
    /// 0..=1 towards a playful interruption (grows while ignored).
    pub boredom: f32,
}

impl Mischief {
    pub fn new(cfg: MischiefConfig) -> Self {
        Self {
            cfg,
            level: 0.0,
            boredom: 0.0,
        }
    }

    /// An interaction: adds `poke`-sized energy (scaled by `amount`) and
    /// resets boredom.
    pub fn poke(&mut self, amount: f32) {
        self.level = (self.level + self.cfg.poke * amount).min(1.0);
        self.boredom = 0.0;
    }

    /// Attention without teasing (hover, key press): resets boredom only.
    pub fn attend(&mut self) {
        self.boredom = 0.0;
    }

    /// `true` while mischievous reactions are unlocked.
    pub fn is_high(&self) -> bool {
        self.level >= self.cfg.high
    }

    /// Advances by `dt`. `ignored` = she is awake and nobody is interacting
    /// (boredom only grows then). Returns `true` (and resets boredom) when a
    /// playful interruption is due.
    pub fn tick(&mut self, dt: f32, ignored: bool) -> bool {
        let dt = dt.max(0.0);
        self.level *= (-dt / self.cfg.decay_tau_s.max(0.1)).exp();
        if ignored && self.cfg.boredom_s > 0.0 {
            self.boredom += dt / self.cfg.boredom_s;
            if self.boredom >= 1.0 {
                self.boredom = 0.0;
                // The interruption itself is a bit of mischief.
                self.level = (self.level + self.cfg.poke).min(1.0);
                return true;
            }
        }
        false
    }

    /// Clears both parts.
    pub fn reset(&mut self) {
        self.level = 0.0;
        self.boredom = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_mood(m: &mut MoodState, secs: f32) -> bool {
        let mut changed = false;
        for _ in 0..(secs * 10.0) as usize {
            changed |= m.tick(0.1);
        }
        changed
    }

    #[test]
    fn mood_fades_back_to_baseline() {
        let mut m = MoodState::new(30.0);
        assert_eq!(m.mood(), Mood::Playful);
        assert!(m.feel(Mood::Alert, 1.0));
        // ln(1/0.25) * 30 s ≈ 41.6 s until it fades.
        assert!(!run_mood(&mut m, 40.0));
        assert_eq!(m.mood(), Mood::Alert);
        assert!(run_mood(&mut m, 3.0));
        assert_eq!(m.mood(), Mood::Playful);
        // The baseline itself never fades.
        assert!(!run_mood(&mut m, 600.0));
        assert_eq!(m.mood(), Mood::Playful);
    }

    #[test]
    fn baseline_switch_and_same_mood_strengthens() {
        let mut m = MoodState::new(30.0);
        assert!(m.set_baseline(Mood::Sleepy), "weak mood switches now");
        assert_eq!(m.mood(), Mood::Sleepy);
        assert!(m.feel(Mood::Happy, 0.9));
        assert!(!m.set_baseline(Mood::Sleepy), "strong mood is kept");
        assert!(!m.feel(Mood::Happy, 0.3));
        assert!((m.strength() - 0.9).abs() < 1e-6);
        run_mood(&mut m, 60.0);
        assert_eq!(m.mood(), Mood::Sleepy);
    }

    #[test]
    fn mischief_rises_on_pokes_and_decays() {
        let mut m = Mischief::default();
        for _ in 0..5 {
            m.poke(1.0);
        }
        assert!(m.is_high(), "5 pokes = 0.75");
        // e^(-dt/60): below 0.7 after ~4.1 s, ~0.28 after a minute.
        for _ in 0..50 {
            m.tick(0.1, false);
        }
        assert!(!m.is_high());
        for _ in 0..550 {
            m.tick(0.1, false);
        }
        assert!(
            (m.level - 0.75 * (-1.0f32).exp()).abs() < 0.01,
            "{}",
            m.level
        );
    }

    #[test]
    fn boredom_fires_only_when_ignored() {
        let mut m = Mischief::default();
        // Not ignored: never fires.
        assert!(!(0..3000).any(|_| m.tick(0.1, false)));
        // Ignored: fires once after boredom_s.
        let fired: Vec<usize> = (0..3100).filter(|_| m.tick(0.1, true)).collect();
        assert_eq!(fired.len(), 2, "150 s each over 310 s");
        assert!((1495..=1505).contains(&fired[0]), "{fired:?}");
        // Attention resets the countdown.
        for _ in 0..1000 {
            m.tick(0.1, true);
        }
        m.attend();
        assert_eq!(m.boredom, 0.0);
    }
}
