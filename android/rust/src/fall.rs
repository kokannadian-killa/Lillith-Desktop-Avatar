//! Drop → fall → soft bounce → land, in screen px (+y = down). Mirrors the
//! desktop `[behavior.gravity]` numbers ([`GravityConfig`]) but always lands
//! on the bottom of the screen (no window perching on Android).

use lillith_core::state::GravityConfig;

/// Bounces slower than this settle instead (px/s), as on desktop.
pub const MIN_BOUNCE_SPEED: f32 = 60.0;
/// At most this many rebounds.
pub const MAX_BOUNCES: u8 = 2;

/// One fall in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fall {
    /// Window top (px).
    pub top: f32,
    /// Vertical speed (px/s, + = down).
    pub vy: f32,
    bounces: u8,
}

/// Outcome of a [`Fall::step`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    Falling,
    /// Resting on the floor.
    Landed,
}

impl Fall {
    pub fn new(top: f32) -> Self {
        Self {
            top,
            vy: 0.0,
            bounces: 0,
        }
    }

    /// Advances by `dt` s toward `floor_top` (the window top that puts her
    /// feet on the bottom of the screen).
    pub fn step(&mut self, cfg: &GravityConfig, floor_top: f32, dt: f32) -> Step {
        if cfg.g_px_s2 <= 0.0 {
            return Step::Landed;
        }
        let dt = dt.clamp(0.0, 0.1);
        self.vy = (self.vy + cfg.g_px_s2 * dt).min(cfg.max_fall_speed.max(1.0));
        self.top += self.vy * dt;
        if self.top < floor_top {
            return Step::Falling;
        }
        self.top = floor_top;
        let rebound = self.vy * cfg.bounce.clamp(0.0, 0.9);
        if rebound >= MIN_BOUNCE_SPEED && self.bounces < MAX_BOUNCES {
            self.bounces += 1;
            self.vy = -rebound;
            Step::Falling
        } else {
            self.vy = 0.0;
            Step::Landed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(cfg: &GravityConfig, from: f32, floor: f32) -> (Fall, usize, f32) {
        let mut f = Fall::new(from);
        let mut lowest = from;
        for i in 0..2000 {
            let s = f.step(cfg, floor, 1.0 / 60.0);
            lowest = lowest.max(f.top);
            if s == Step::Landed {
                return (f, i, lowest);
            }
        }
        panic!("never landed");
    }

    #[test]
    fn falls_to_the_bottom_and_never_through_it() {
        let cfg = GravityConfig::default();
        let (f, steps, lowest) = run(&cfg, 0.0, 1800.0);
        assert_eq!(f.top, 1800.0);
        assert!(lowest <= 1800.0);
        assert!(steps > 10, "fell too fast: {steps} steps");
    }

    #[test]
    fn bounces_softly_then_settles() {
        let cfg = GravityConfig::default();
        let mut f = Fall::new(0.0);
        let mut ups = 0;
        let mut was_down = true;
        for _ in 0..2000 {
            if f.step(&cfg, 1500.0, 1.0 / 60.0) == Step::Landed {
                break;
            }
            let down = f.vy >= 0.0;
            if was_down && !down {
                ups += 1;
            }
            was_down = down;
        }
        assert!((1..=MAX_BOUNCES as usize).contains(&ups), "{ups} bounces");
    }

    #[test]
    fn zero_gravity_lands_where_dropped() {
        let cfg = GravityConfig {
            g_px_s2: 0.0,
            ..GravityConfig::default()
        };
        let mut f = Fall::new(300.0);
        assert_eq!(f.step(&cfg, 1500.0, 0.016), Step::Landed);
        assert_eq!(f.top, 300.0);
    }

    #[test]
    fn already_on_the_floor_lands_at_once() {
        let cfg = GravityConfig::default();
        let mut f = Fall::new(1500.0);
        assert_eq!(f.step(&cfg, 1500.0, 0.016), Step::Landed);
    }
}
