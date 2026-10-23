//! Chest bounce: the Settings ▸ Motion "Chest bounce" knobs
//! (`config.toml [springs.bounce]`) and how they map onto the physics.toml
//! chest spring.
//!
//! Two user numbers drive everything:
//!
//! * **intensity** (0 … 3, shown as 0 … 300 %): multiplies the chest impulse
//!   gain. 0 % switches the engine chest bounce off entirely; 100 % is the
//!   tuned look; above 100 % also layers a light engine bounce over
//!   *baked* chest art (see [`baked_layer`]).
//! * **softness** (0 … 1, shown as "Firm … Soft" 0 … 100 %): picks the
//!   spring's character. It slides along a curve of (frequency, damping
//!   ratio, kick boost, body-coupling) anchors:
//!
//!   | softness | freq    | damping ratio ζ | feel                               |
//!   |----------|---------|-----------------|------------------------------------|
//!   | 0.0      | 1.80 Hz | 0.90            | firm, one small settle             |
//!   | 0.3      | 1.51 Hz | 0.764           | = physics.toml 90 / 14.5 (Natural) |
//!   | 0.85     | 1.19 Hz | 0.29            | Extra (default): lag, overshoot, 2–3 wobbles |
//!   | 1.0      | 1.10 Hz | 0.16            | softest, 4–5 slow wobbles           |
//!
//!   `stiffness = ω²`, `damping = 2ζω` with `ω = 2π·freq`. Every point is
//!   unconditionally stable for the semi-implicit Euler sub-step (ω·h ≤ 0.4
//!   even at 30 Hz physics), never under-damped below ζ = 0.15 (no endless
//!   buzz) and slow (≤ 1.8 Hz, no jitter).
//!
//! Softer settings also add *body coupling* (`extra` > 0 above Natural):
//! the chest reacts to the body's vertical acceleration so it lags behind
//! hops / landings, and walk starts and turns give it a small kick.
//! Displayed offsets go through a smooth soft-knee ([`soft_limit`]) so even
//! 300 % never flat-tops against a hard clamp.

use serde::{Deserialize, Serialize};

/// `config.toml [springs.bounce]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChestBounce {
    /// Impulse gain multiplier, 0 … [`MAX_INTENSITY`] (1 = 100 %).
    pub intensity: f32,
    /// 0 = firm … 1 = softest / most wobbly.
    pub softness: f32,
}

/// Slider top: 300 %.
pub const MAX_INTENSITY: f32 = 3.0;

impl Default for ChestBounce {
    /// [`BouncePreset::Extra`].
    fn default() -> Self {
        BouncePreset::Extra.values()
    }
}

impl ChestBounce {
    /// Range-clamped copy (NaN reads as the default).
    pub fn sanitized(self) -> Self {
        let d = Self::default();
        let fix = |v: f32, dv: f32, hi: f32| if v.is_finite() { v.clamp(0.0, hi) } else { dv };
        Self {
            intensity: fix(self.intensity, d.intensity, MAX_INTENSITY),
            softness: fix(self.softness, d.softness, 1.0),
        }
    }

    /// Spring character for this softness.
    pub fn feel(self) -> BounceFeel {
        BounceFeel::at(self.sanitized().softness)
    }

    /// Multiplier on physics.toml's `[gains] chest_bounce`.
    pub fn gain_scale(self) -> f32 {
        let s = self.sanitized();
        s.intensity * s.feel().kick_boost
    }

    /// The preset these values match, if any (else the UI shows "Custom").
    pub fn preset(self) -> Option<BouncePreset> {
        BouncePreset::ALL.into_iter().find(|p| {
            let v = p.values();
            (v.intensity - self.intensity).abs() < 0.005
                && (v.softness - self.softness).abs() < 0.005
        })
    }
}

/// Settings presets (they only set the two sliders).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BouncePreset {
    Subtle,
    Natural,
    Bouncy,
    Extra,
}

impl BouncePreset {
    pub const ALL: [BouncePreset; 4] = [Self::Subtle, Self::Natural, Self::Bouncy, Self::Extra];

    pub fn label(self) -> &'static str {
        match self {
            Self::Subtle => "Subtle",
            Self::Natural => "Natural",
            Self::Bouncy => "Bouncy",
            Self::Extra => "Extra (sexy)",
        }
    }

    pub fn values(self) -> ChestBounce {
        let (intensity, softness) = match self {
            Self::Subtle => (0.6, 0.0),
            Self::Natural => (1.0, NATURAL_SOFTNESS),
            Self::Bouncy => (1.0, 0.6),
            Self::Extra => (1.0, 0.85),
        };
        ChestBounce {
            intensity,
            softness,
        }
    }
}

/// Softness at which the spring is exactly physics.toml's authored chest
/// (90 / 14.5, kick boost 1, no extra coupling).
pub const NATURAL_SOFTNESS: f32 = 0.3;

/// What a softness value does to the chest spring.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BounceFeel {
    pub stiffness: f32,
    pub damping: f32,
    /// Natural frequency, Hz.
    pub freq_hz: f32,
    /// Damping ratio ζ.
    pub zeta: f32,
    /// Extra impulse gain for the softer, slower spring (same kick = bigger,
    /// lazier swing).
    pub kick_boost: f32,
    /// 0 at/below Natural … 1 at the softest: scales body coupling and the
    /// walk-start / turn kicks.
    pub extra: f32,
}

/// Lowest damping ratio any softness maps to.
pub const MIN_ZETA: f32 = 0.15;

impl BounceFeel {
    pub fn at(softness: f32) -> Self {
        let s = if softness.is_finite() {
            softness.clamp(0.0, 1.0)
        } else {
            0.85
        } as f64;
        let tau = std::f64::consts::TAU;
        // (omega, zeta, boost, extra) anchors
        let a0 = (tau * 1.8, 0.9, 0.85, 0.0);
        let w1 = 90f64.sqrt();
        let a1 = (w1, 14.5 / (2.0 * w1), 1.0, 0.0);
        let a2 = (tau * 1.1, 0.16, 1.6, 1.0);
        let n = NATURAL_SOFTNESS as f64;
        let (p, q, t) = if s <= n {
            (a0, a1, s / n)
        } else {
            (a1, a2, (s - n) / (1.0 - n))
        };
        let l = |x: f64, y: f64| x + (y - x) * t;
        let w = l(p.0, q.0);
        let zeta = l(p.1, q.1).max(MIN_ZETA as f64);
        let r2 = |v: f64| ((v * 100.0).round() / 100.0) as f32;
        Self {
            stiffness: r2(w * w),
            damping: r2(2.0 * zeta * w),
            freq_hz: (w / tau) as f32,
            zeta: zeta as f32,
            kick_boost: l(p.2, q.2) as f32,
            extra: l(p.3, q.3) as f32,
        }
    }

    /// How many visible wobbles (peaks ≥ 10 % of the first) follow a kick.
    pub fn wobbles(&self) -> u32 {
        let z = self.zeta.clamp(0.0, 0.999);
        // ratio between successive half-cycle peaks
        let r = (-std::f32::consts::PI * z / (1.0 - z * z).sqrt()).exp();
        if r <= 0.0 {
            return 0;
        }
        ((0.1f32).ln() / r.ln()).floor().max(0.0) as u32
    }
}

/// Body-coupling at `extra` = 1: chest velocity −= this × the body's
/// vertical velocity change (it lags hops and landings).
pub const BODY_COUPLING: f32 = 0.45;
/// Chest kick (px/s before gains) when she starts walking, at `extra` = 1.
pub const WALK_START_KICK: f32 = 12.0;
/// Chest kick (px/s before gains) when she turns around, at `extra` = 1.
pub const TURN_KICK: f32 = 14.0;
/// Hard clamps while bounce tuning is applied (the soft knee does the real
/// limiting; these only guard against a pile-up of kicks).
pub const HARD_MAX_VELOCITY: f32 = 400.0;
pub const HARD_MAX_OFFSET: f32 = 60.0;
/// Soft knee: linear up to `KNEE_PX`, then eases towards `CAP_PX`.
pub const KNEE_PX: f32 = 8.0;
pub const CAP_PX: f32 = 24.0;
/// Chest kicks are delivered over this time constant (s) instead of in one
/// sub-step, so a hit never leaves a corner in the curve.
pub const KICK_TAU_S: f32 = 0.03;

/// Smooth (C1) saturation of a displayed offset: identity up to `knee`,
/// then `knee + r·u/(1+u)` with `r = cap-knee`, `u = (|x|-knee)/r`. The
/// slow rational approach keeps big swings rounded (no flat top).
pub fn soft_limit(x: f32, knee: f32, cap: f32) -> f32 {
    let a = x.abs();
    if !x.is_finite() {
        return 0.0;
    }
    if a <= knee || cap <= knee {
        return x.clamp(-cap.max(knee), cap.max(knee));
    }
    let r = cap - knee;
    let u = (a - knee) / r;
    x.signum() * (knee + r * u / (1.0 + u))
}

/// Share of the engine chest bounce layered over clips whose chest motion
/// is baked into the art (`baked_motion` / `jiggle_exaggerated`, where the
/// engine chest spring is otherwise switched off). 0 up to 100 % (the art
/// already jiggles: no double jiggle), easing up to [`BAKED_LAYER_MAX`] at
/// 300 %.
pub fn baked_layer(intensity: f32) -> f32 {
    let i = ChestBounce {
        intensity,
        softness: 0.5,
    }
    .sanitized()
    .intensity;
    let t = ((i - 1.0) / (MAX_INTENSITY - 1.0)).clamp(0.0, 1.0);
    BAKED_LAYER_MAX * t * t * (3.0 - 2.0 * t)
}

/// Layer at 300 %.
pub const BAKED_LAYER_MAX: f32 = 0.45;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_is_the_authored_spring() {
        let f = BounceFeel::at(NATURAL_SOFTNESS);
        assert_eq!((f.stiffness, f.damping), (90.0, 14.5));
        assert_eq!((f.kick_boost, f.extra), (1.0, 0.0));
    }

    #[test]
    fn default_is_extra_and_wobbly_but_damped() {
        let b = ChestBounce::default();
        assert_eq!(b.preset(), Some(BouncePreset::Extra));
        let f = b.feel();
        assert!((1.1..1.3).contains(&f.freq_hz), "{f:?}");
        assert!((0.25..0.33).contains(&f.zeta), "{f:?}");
        assert!((2..=3).contains(&f.wobbles()), "{f:?} {}", f.wobbles());
        assert!(BounceFeel::at(0.0).wobbles() == 0);
    }

    #[test]
    fn feel_is_monotonic_and_stable_everywhere() {
        let mut prev = BounceFeel::at(0.0);
        for i in 1..=100 {
            let f = BounceFeel::at(i as f32 / 100.0);
            assert!(f.freq_hz <= prev.freq_hz + 1e-6 && f.zeta <= prev.zeta + 1e-6);
            assert!(f.zeta >= MIN_ZETA && f.freq_hz <= 1.81);
            // semi-implicit Euler stable at 30 Hz with margin
            assert!(f.stiffness.sqrt() / 30.0 < 0.5);
            prev = f;
        }
    }

    #[test]
    fn sanitize_and_old_values() {
        let b = ChestBounce {
            intensity: 9.0,
            softness: f32::NAN,
        }
        .sanitized();
        assert_eq!(b.intensity, 3.0);
        assert_eq!(b.softness, ChestBounce::default().softness);
        assert_eq!(
            ChestBounce {
                intensity: 0.0,
                softness: 0.5
            }
            .gain_scale(),
            0.0
        );
    }

    #[test]
    fn baked_layer_is_zero_until_100() {
        assert_eq!(baked_layer(0.0), 0.0);
        assert_eq!(baked_layer(1.0), 0.0);
        assert!(baked_layer(2.0) > 0.0 && baked_layer(2.0) < BAKED_LAYER_MAX);
        assert!((baked_layer(3.0) - BAKED_LAYER_MAX).abs() < 1e-6);
    }

    #[test]
    fn soft_limit_is_smooth_and_bounded() {
        assert_eq!(soft_limit(5.0, KNEE_PX, CAP_PX), 5.0);
        assert_eq!(soft_limit(-5.0, KNEE_PX, CAP_PX), -5.0);
        let mut prev = 0.0;
        for i in 0..400 {
            let x = i as f32 * 0.25;
            let y = soft_limit(x, KNEE_PX, CAP_PX);
            assert!(y >= prev && y < CAP_PX);
            // slope ≤ 1 (C1 at the knee: no kink)
            assert!(y - prev <= 0.25 + 1e-5);
            prev = y;
        }
        assert_eq!(soft_limit(f32::NAN, KNEE_PX, CAP_PX), 0.0);
    }

    #[test]
    fn old_configs_load_and_new_ones_round_trip() {
        use crate::config::Config;
        let old = "[springs]\nchest_bounce_gain = 1.2\njiggle_gain = 0.5\n";
        let c = Config::from_toml_str(old).unwrap();
        assert_eq!(c.springs.bounce, ChestBounce::default());
        assert_eq!(c.springs.chest_bounce_gain, 1.2);
        let new = "[springs.bounce]\nintensity = 2.5\n";
        let c = Config::from_toml_str(new).unwrap();
        assert_eq!(c.springs.bounce.intensity, 2.5);
        assert_eq!(c.springs.bounce.softness, ChestBounce::default().softness);
        let text = toml::to_string(&c).unwrap();
        assert_eq!(
            Config::from_toml_str(&text).unwrap().springs.bounce,
            c.springs.bounce
        );
    }
}
