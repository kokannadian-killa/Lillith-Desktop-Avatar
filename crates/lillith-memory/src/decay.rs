//! Strength decay and reinforcement math.
//!
//! A memory stores its strength *as of* `last_seen`; the effective strength
//! at `now` is `s * 0.5^((now - last_seen) / half_life)`. Nothing is ever
//! decayed twice: reinforcing first takes the effective value, then bumps it
//! and moves `last_seen` to `now`.

/// Strength after `age_s` seconds with the given half-life (seconds).
/// A non-positive half-life disables decay. Negative ages count as 0.
pub fn decayed(strength: f64, age_s: f64, half_life_s: f64) -> f64 {
    if half_life_s <= 0.0 || !half_life_s.is_finite() {
        return strength;
    }
    strength * 0.5f64.powf(age_s.max(0.0) / half_life_s)
}

/// Recency in 0..=1 (1 = just now, 0.5 after one half-life).
pub fn recency(age_s: f64, half_life_s: f64) -> f64 {
    decayed(1.0, age_s, half_life_s)
}

/// Reinforces `s` toward 1 by `rate` (0..=1) of the remaining gap.
pub fn reinforce(s: f64, rate: f64) -> f64 {
    let s = s.clamp(0.0, 1.0);
    (s + rate.clamp(0.0, 1.0) * (1.0 - s)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: f64 = 86_400.0;

    #[test]
    fn half_life_halves() {
        let hl = 14.0 * DAY;
        assert!((decayed(1.0, hl, hl) - 0.5).abs() < 1e-12);
        assert!((decayed(0.8, 2.0 * hl, hl) - 0.2).abs() < 1e-12);
        assert!((decayed(0.8, 0.0, hl) - 0.8).abs() < 1e-12);
        assert!(
            (decayed(0.8, -5.0, hl) - 0.8).abs() < 1e-12,
            "future = no decay"
        );
        assert_eq!(decayed(0.7, 1e9, 0.0), 0.7, "half-life 0 disables decay");
    }

    #[test]
    fn decay_composes() {
        // Decaying in two steps equals one step of the summed age.
        let hl = 3.0 * DAY;
        let two = decayed(decayed(0.9, DAY, hl), 2.0 * DAY, hl);
        assert!((two - decayed(0.9, 3.0 * DAY, hl)).abs() < 1e-12);
    }

    #[test]
    fn reinforce_moves_toward_one() {
        assert!((reinforce(0.5, 0.5) - 0.75).abs() < 1e-12);
        assert_eq!(reinforce(1.0, 0.3), 1.0);
        assert_eq!(reinforce(0.4, 0.0), 0.4);
        assert!(reinforce(0.99, 1.0) <= 1.0);
        let mut s = 0.1;
        for _ in 0..50 {
            let n = reinforce(s, 0.2);
            assert!(n > s && n <= 1.0);
            s = n;
        }
    }

    #[test]
    fn recency_bounds() {
        assert_eq!(recency(0.0, DAY), 1.0);
        assert!(recency(10.0 * DAY, DAY) < 0.001);
    }
}
