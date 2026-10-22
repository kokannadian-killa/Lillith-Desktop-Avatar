//! Spring-damper channels (PLAN.md §3.1, layer 5).
//!
//! One spring per channel (root x/y, angle, scale, chest, pigtails),
//! integrated with semi-implicit Euler in fixed 120 Hz sub-steps with
//! velocity clamping. Reactions apply **impulses** (velocity kicks); there is
//! no random per-frame jitter like the v01 Python prototype.
//!
//! Units: root x/y in pixels (screen space, +y down), angle in radians,
//! scale as a multiplier (rest 1.0), chest and pigtail offsets in pixels.
//! Without layered art the renderer applies chest/pigtail as a subtle
//! whole-sprite squash and skew (PLAN.md §3.1 design note).

use crate::config::Springs;
use serde::{Deserialize, Serialize};

/// Stiffness/damping pair, as in README-SP `[springs]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpringParams {
    pub stiffness: f32,
    pub damping: f32,
}

/// A single 1-D spring-damper.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    pub params: SpringParams,
    pub position: f32,
    pub velocity: f32,
    pub target: f32,
    /// Velocity is clamped to `±max_velocity` after every kick and step.
    pub max_velocity: f32,
    /// Optional hard limit on `|position - target|`.
    pub max_offset: Option<f32>,
}

impl Spring {
    /// New spring at rest on `target`.
    pub fn new(params: SpringParams, target: f32) -> Self {
        Self {
            params,
            position: target,
            velocity: 0.0,
            target,
            max_velocity: 50.0,
            max_offset: None,
        }
    }

    /// Builder: velocity clamp.
    pub fn with_max_velocity(mut self, v: f32) -> Self {
        self.max_velocity = v.abs();
        self
    }

    /// Builder: displacement clamp around the target.
    pub fn with_max_offset(mut self, d: f32) -> Self {
        self.max_offset = Some(d.abs());
        self
    }

    /// Adds an instantaneous velocity kick (reactions use impulses, not jitter).
    pub fn impulse(&mut self, dv: f32) {
        self.velocity = (self.velocity + dv).clamp(-self.max_velocity, self.max_velocity);
    }

    /// Semi-implicit Euler step.
    pub fn step(&mut self, dt: f32) {
        let accel = -self.params.stiffness * (self.position - self.target)
            - self.params.damping * self.velocity;
        self.velocity = (self.velocity + accel * dt).clamp(-self.max_velocity, self.max_velocity);
        self.position += self.velocity * dt;
        if let Some(m) = self.max_offset {
            let off = self.position - self.target;
            if off.abs() > m {
                self.position = self.target + off.signum() * m;
                // Kill the velocity component pushing further out.
                if self.velocity.signum() == off.signum() {
                    self.velocity = 0.0;
                }
            }
        }
    }

    /// Steps in fixed `1/hz` sub-steps covering `dt`.
    pub fn advance(&mut self, dt: f32, hz: f32) {
        let h = 1.0 / hz;
        let mut remaining = dt;
        while remaining > 1e-7 {
            let s = remaining.min(h);
            self.step(s);
            remaining -= s;
        }
    }

    /// Displacement from the target.
    pub fn offset(&self) -> f32 {
        self.position - self.target
    }

    /// `true` when (nearly) at rest on the target.
    pub fn at_rest(&self, eps: f32) -> bool {
        self.offset().abs() < eps && self.velocity.abs() < eps
    }
}

/// The physics channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    RootX,
    RootY,
    Angle,
    Scale,
    Chest,
    /// Serialized as `"pigtail"`; `"pigtails"` (physics.toml naming) and
    /// `"pig"` (README-SP `[springs]` key) are accepted too.
    #[serde(alias = "pigtails", alias = "pig")]
    Pigtail,
}

impl Channel {
    pub const ALL: [Channel; 6] = [
        Channel::RootX,
        Channel::RootY,
        Channel::Angle,
        Channel::Scale,
        Channel::Chest,
        Channel::Pigtail,
    ];

    /// The physics.toml channel name (`"pigtails"` for [`Channel::Pigtail`]).
    pub fn name(self) -> &'static str {
        match self {
            Channel::RootX => "root_x",
            Channel::RootY => "root_y",
            Channel::Angle => "angle",
            Channel::Scale => "scale",
            Channel::Chest => "chest",
            Channel::Pigtail => "pigtails",
        }
    }

    /// Parses physics.toml / `Command::Impulse` channel names
    /// (`"pigtails"`, `"pigtail"` and `"pig"` all mean [`Channel::Pigtail`]).
    pub fn from_name(s: &str) -> Option<Channel> {
        Some(match s {
            "root_x" => Channel::RootX,
            "root_y" => Channel::RootY,
            "angle" => Channel::Angle,
            "scale" => Channel::Scale,
            "chest" => Channel::Chest,
            "pigtails" | "pigtail" | "pig" => Channel::Pigtail,
            _ => return None,
        })
    }
}

impl Channel {
    fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

/// A set of spring channels the engine must leave alone, e.g. while a clip
/// whose frames already contain baked jiggle is drawn (manifest
/// `engine_springs: "disable chest/pigtails/hip for this clip"`).
///
/// Art-side group names map onto channels: `chest` → [`Channel::Chest`],
/// `pigtails`/`pigtail`/`pig` → [`Channel::Pigtail`], `hip` → [`Channel::RootX`]
/// (the lateral hip/root sway; physics.toml plants root motion at the hip).
/// Every [`Channel::from_name`] name is accepted as well.
///
/// One more group, [`SpringMask::GAIT_GROUP`] (`gait`), names no spring
/// channel: it marks a clip whose frames bake the walk itself (legs, arms,
/// hip bob), so the engine gait (`physics.toml [gait]`) must not drive it.
/// It is accepted everywhere a group name is, but never sets a mask bit;
/// test it with [`SpringMask::names_gait`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SpringMask(u8);

impl SpringMask {
    pub const NONE: Self = Self(0);

    /// Non-channel group: "this clip bakes its own gait" (see the type docs).
    pub const GAIT_GROUP: &'static str = "gait";

    /// `true` if `name` is the [`Self::GAIT_GROUP`] group.
    pub fn is_gait_group(name: &str) -> bool {
        name.trim().eq_ignore_ascii_case(Self::GAIT_GROUP)
    }

    /// `true` if any of `names` is the [`Self::GAIT_GROUP`] group.
    pub fn names_gait<S: AsRef<str>>(names: &[S]) -> bool {
        names.iter().any(|n| Self::is_gait_group(n.as_ref()))
    }

    /// Mask with exactly `channels`.
    pub fn of(channels: &[Channel]) -> Self {
        Self(channels.iter().fold(0, |m, c| m | c.bit()))
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn contains(self, c: Channel) -> bool {
        self.0 & c.bit() != 0
    }

    pub fn insert(&mut self, c: Channel) {
        self.0 |= c.bit();
    }

    /// Channels in the mask, in [`Channel::ALL`] order.
    pub fn channels(self) -> impl Iterator<Item = Channel> {
        Channel::ALL.into_iter().filter(move |&c| self.contains(c))
    }

    /// Channel for one art-side group name (`chest`, `pigtails`, `hip`, or
    /// any [`Channel::from_name`] name). `None` for unknown names.
    pub fn group(name: &str) -> Option<Channel> {
        match name.trim().to_ascii_lowercase().as_str() {
            "hip" | "hips" => Some(Channel::RootX),
            other => Channel::from_name(other),
        }
    }

    /// Mask from group names; unknown names are returned as the error.
    /// [`Self::GAIT_GROUP`] is accepted and sets no channel.
    pub fn from_groups<S: AsRef<str>>(names: &[S]) -> Result<Self, String> {
        let mut m = Self::NONE;
        for n in names {
            if Self::is_gait_group(n.as_ref()) {
                continue;
            }
            m.insert(Self::group(n.as_ref()).ok_or_else(|| n.as_ref().to_string())?);
        }
        Ok(m)
    }

    /// Group names named in a free-text `engine_springs` directive such as
    /// `"disable chest/pigtails/hip for this clip"`: the words between
    /// `disable` and the first non-channel word (separators `/`, `,`, `+`,
    /// `and`, whitespace). Empty when the text has no `disable`.
    pub fn parse_directive(text: &str) -> Vec<String> {
        let lower = text.to_ascii_lowercase();
        let Some(i) = lower.find("disable") else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for w in lower[i + "disable".len()..]
            .split(|c: char| c == '/' || c == ',' || c == '+' || c.is_whitespace())
            .filter(|w| !w.is_empty() && *w != "and" && *w != "the")
        {
            if Self::group(w).is_some() || Self::is_gait_group(w) {
                out.push(w.to_string());
            } else if !out.is_empty() {
                break;
            }
        }
        out
    }
}

/// The spring-driven transform applied on top of the current sprite frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    /// Root offset in px (+y down).
    pub x: f32,
    pub y: f32,
    /// Rotation about the feet anchor, radians.
    pub angle: f32,
    /// Uniform scale multiplier (rest 1.0).
    pub scale: f32,
    /// Chest bounce offset in px (renderer: vertical squash/stretch).
    pub chest_offset: f32,
    /// Pigtail sway offset in px (renderer: slight skew of the upper body).
    pub pigtail_offset: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    pub const IDENTITY: Self = Self {
        x: 0.0,
        y: 0.0,
        angle: 0.0,
        scale: 1.0,
        chest_offset: 0.0,
        pigtail_offset: 0.0,
    };
}

/// All six channels, tuned from [`Springs`] (README-SP defaults).
#[derive(Debug, Clone, PartialEq)]
pub struct Physics {
    pub root_x: Spring,
    pub root_y: Spring,
    pub angle: Spring,
    pub scale: Spring,
    pub chest: Spring,
    pub pigtail: Spring,
    pub jiggle_gain: f32,
    pub chest_bounce_gain: f32,
    pub pigtail_sway_gain: f32,
    /// Sub-step rate (README-SP: 120 Hz).
    pub hz: f32,
    /// Channels currently switched off (see [`Physics::set_suppressed`]).
    pub suppressed: SpringMask,
}

impl Default for Physics {
    fn default() -> Self {
        Self::new(&Springs::default(), 120.0)
    }
}

impl Physics {
    /// Builds the channels from config. Clamps keep a bad config or a pile of
    /// impulses from flinging her off screen.
    pub fn new(cfg: &Springs, hz: f32) -> Self {
        Self {
            root_x: Spring::new(cfg.root, 0.0)
                .with_max_velocity(400.0)
                .with_max_offset(40.0),
            root_y: Spring::new(cfg.root, 0.0)
                .with_max_velocity(400.0)
                .with_max_offset(40.0),
            angle: Spring::new(cfg.angle, 0.0)
                .with_max_velocity(4.0)
                .with_max_offset(0.35),
            scale: Spring::new(cfg.scale, 1.0)
                .with_max_velocity(2.0)
                .with_max_offset(0.2),
            chest: Spring::new(cfg.chest, 0.0)
                .with_max_velocity(120.0)
                .with_max_offset(12.0),
            pigtail: Spring::new(cfg.pig, 0.0)
                .with_max_velocity(160.0)
                .with_max_offset(20.0),
            jiggle_gain: cfg.jiggle_gain,
            chest_bounce_gain: cfg.chest_bounce_gain,
            pigtail_sway_gain: cfg.pigtail_sway_gain,
            hz: if hz > 0.0 { hz } else { 120.0 },
            suppressed: SpringMask::NONE,
        }
    }

    pub fn spring(&self, c: Channel) -> &Spring {
        match c {
            Channel::RootX => &self.root_x,
            Channel::RootY => &self.root_y,
            Channel::Angle => &self.angle,
            Channel::Scale => &self.scale,
            Channel::Chest => &self.chest,
            Channel::Pigtail => &self.pigtail,
        }
    }

    pub fn spring_mut(&mut self, c: Channel) -> &mut Spring {
        match c {
            Channel::RootX => &mut self.root_x,
            Channel::RootY => &mut self.root_y,
            Channel::Angle => &mut self.angle,
            Channel::Scale => &mut self.scale,
            Channel::Chest => &mut self.chest,
            Channel::Pigtail => &mut self.pigtail,
        }
    }

    /// Gain applied to impulses on channel `c`: `chest_bounce` for the chest,
    /// `pigtail_sway` for pigtails, `jiggle` for root/angle/scale.
    pub fn gain(&self, c: Channel) -> f32 {
        match c {
            Channel::Chest => self.chest_bounce_gain,
            Channel::Pigtail => self.pigtail_sway_gain,
            _ => self.jiggle_gain,
        }
    }

    /// Velocity kick of `strength` (channel units per second), scaled by the
    /// channel's gain.
    pub fn impulse(&mut self, c: Channel, strength: f32) {
        if self.suppressed.contains(c) {
            return;
        }
        let g = self.gain(c);
        self.spring_mut(c).impulse(strength * g);
    }

    /// Moves the rest target (e.g. breathing drives the scale/chest targets).
    pub fn set_target(&mut self, c: Channel, target: f32) {
        self.spring_mut(c).target = target;
    }

    /// Retunes one channel (e.g. mood multipliers) without touching its
    /// position, velocity or target.
    pub fn set_params(&mut self, c: Channel, p: SpringParams) {
        self.spring_mut(c).params = p;
    }

    /// Sets the three README-SP gains at once.
    pub fn set_gains(&mut self, jiggle: f32, chest_bounce: f32, pigtail_sway: f32) {
        self.jiggle_gain = jiggle;
        self.chest_bounce_gain = chest_bounce;
        self.pigtail_sway_gain = pigtail_sway;
    }

    /// Advances every channel by `dt` seconds in fixed `1/hz` sub-steps.
    pub fn step(&mut self, dt: f32) {
        let hz = self.hz;
        for c in Channel::ALL {
            if self.suppressed.contains(c) {
                Self::hold(self.spring_mut(c));
            } else {
                self.spring_mut(c).advance(dt, hz);
            }
        }
    }

    /// Parks a suppressed spring on its target with no velocity.
    fn hold(s: &mut Spring) {
        s.position = s.target;
        s.velocity = 0.0;
    }

    /// Switches channels off (e.g. a baked-jiggle clip is drawn) or back on.
    /// Suppressed channels are parked on their target with zero velocity,
    /// ignore impulses and read as rest in [`Physics::transform`], so the
    /// engine adds no motion on top of the baked frames. Channels leaving
    /// the mask resume from that parked state (no pop, no stored energy).
    pub fn set_suppressed(&mut self, mask: SpringMask) {
        self.suppressed = mask;
        for c in mask.channels() {
            Self::hold(self.spring_mut(c));
        }
    }

    /// Current transform. Root x/y, angle, chest and pigtail are reported as
    /// offsets from rest; scale is absolute.
    pub fn transform(&self) -> Transform {
        let rest = Transform::IDENTITY;
        let m = self.suppressed;
        let pick = |c: Channel, live: f32, rest: f32| if m.contains(c) { rest } else { live };
        Transform {
            x: pick(Channel::RootX, self.root_x.position, rest.x),
            y: pick(Channel::RootY, self.root_y.position, rest.y),
            angle: pick(Channel::Angle, self.angle.position, rest.angle),
            scale: pick(Channel::Scale, self.scale.position, rest.scale),
            chest_offset: pick(Channel::Chest, self.chest.position, rest.chest_offset),
            pigtail_offset: pick(Channel::Pigtail, self.pigtail.position, rest.pigtail_offset),
        }
    }

    /// `true` when every channel is (nearly) at rest on its target.
    pub fn at_rest(&self, eps: f32) -> bool {
        Channel::ALL.iter().all(|&c| self.spring(c).at_rest(eps))
    }
}

/// Breathing: a slow sine whose period is re-picked every cycle from
/// `[min_s, max_s]` (README-SP: 3.5–4.5 s) so it never looks mechanical.
#[derive(Debug, Clone, PartialEq)]
pub struct Breath {
    pub period_s: f32,
    phase: f32,
}

impl Breath {
    pub fn new(period_s: f32) -> Self {
        Self {
            period_s: period_s.max(0.1),
            phase: 0.0,
        }
    }

    /// Advances; returns `true` when a new cycle starts (so the caller can
    /// pick a fresh period).
    pub fn advance(&mut self, dt: f32) -> bool {
        self.phase += dt / self.period_s;
        if self.phase >= 1.0 {
            self.phase = self.phase.fract();
            true
        } else {
            false
        }
    }

    /// Phase in `[0, 1)`.
    pub fn phase(&self) -> f32 {
        self.phase
    }

    /// Breath value in `[-1, 1]` (0 at the start of each cycle, inhaling).
    pub fn value(&self) -> f32 {
        (self.phase * std::f32::consts::TAU).sin()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settles_on_target_after_impulse() {
        let mut s = Spring::new(
            SpringParams {
                stiffness: 65.0,
                damping: 9.5,
            },
            0.0,
        );
        s.impulse(5.0);
        s.advance(5.0, 120.0);
        assert!(s.position.abs() < 1e-3, "position {}", s.position);
    }

    #[test]
    fn physics_at_rest_stays_at_rest() {
        let mut p = Physics::default();
        for _ in 0..600 {
            p.step(1.0 / 30.0);
        }
        assert!(p.at_rest(1e-6));
        assert_eq!(p.transform(), Transform::IDENTITY);
    }

    #[test]
    fn impulse_moves_then_decays() {
        let mut p = Physics::default();
        p.impulse(Channel::Chest, 60.0);
        p.step(0.05);
        let peak = p.transform().chest_offset;
        assert!(peak > 0.5, "chest should move, got {peak}");
        p.step(4.0);
        assert!(p.chest.at_rest(1e-2), "chest {:?}", p.chest);
        assert!(p.root_x.at_rest(1e-9), "other channels untouched");
    }

    #[test]
    fn gains_scale_impulses() {
        let mut p = Physics::default();
        p.impulse(Channel::Chest, 10.0);
        p.impulse(Channel::Pigtail, 10.0);
        p.impulse(Channel::Angle, 1.0);
        assert!((p.chest.velocity - 10.0).abs() < 1e-6); // chest_bounce 1.0
        assert!((p.pigtail.velocity - 9.0).abs() < 1e-5); // pigtail_sway 0.9
        assert!((p.angle.velocity - 0.6).abs() < 1e-6); // jiggle 0.6
    }

    #[test]
    fn velocity_and_offset_are_clamped() {
        let mut p = Physics::default();
        p.impulse(Channel::RootY, 1e6);
        assert_eq!(p.root_y.velocity, 400.0);
        for _ in 0..240 {
            p.step(1.0 / 120.0);
            assert!(p.root_y.offset().abs() <= 40.0 + 1e-4);
            assert!(p.root_y.velocity.abs() <= 400.0);
        }
        p.impulse(Channel::Angle, -1e6);
        p.step(0.5);
        assert!(p.angle.offset().abs() <= 0.35 + 1e-6);
    }

    #[test]
    fn substeps_match_fine_stepping() {
        let mut a = Physics::default();
        let mut b = Physics::default();
        a.impulse(Channel::Pigtail, 30.0);
        b.impulse(Channel::Pigtail, 30.0);
        a.step(0.5); // one call, sub-stepped internally
        for _ in 0..60 {
            b.step(1.0 / 120.0);
        }
        assert!((a.pigtail.position - b.pigtail.position).abs() < 1e-3);
    }

    #[test]
    fn spring_follows_moving_target() {
        let mut p = Physics::default();
        p.set_target(Channel::Scale, 1.02);
        p.step(3.0);
        assert!((p.transform().scale - 1.02).abs() < 1e-3);
    }

    #[test]
    fn breath_oscillates_with_period() {
        let mut b = Breath::new(4.0);
        let mut samples = Vec::new();
        let mut wrapped = 0;
        for _ in 0..(8 * 30 + 3) {
            if b.advance(1.0 / 30.0) {
                wrapped += 1;
            }
            samples.push(b.value());
        }
        assert_eq!(wrapped, 2, "two 4 s cycles in 8 s");
        let max = samples.iter().cloned().fold(f32::MIN, f32::max);
        let min = samples.iter().cloned().fold(f32::MAX, f32::min);
        assert!(max > 0.99 && min < -0.99, "{min}..{max}");
        // Quarter period in: near the peak.
        let mut q = Breath::new(4.0);
        q.advance(1.0);
        assert!((q.value() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn set_params_keeps_state_and_names_parse() {
        let mut p = Physics::default();
        p.impulse(Channel::Chest, 30.0);
        p.step(0.05);
        let (x, v) = (p.chest.position, p.chest.velocity);
        p.set_params(
            Channel::Chest,
            SpringParams {
                stiffness: 84.5,
                damping: 10.45,
            },
        );
        assert_eq!((p.chest.position, p.chest.velocity), (x, v));
        assert_eq!(p.chest.params.stiffness, 84.5);
        p.set_gains(0.5, 1.1, 0.8);
        assert_eq!(p.gain(Channel::Pigtail), 0.8);
        for c in Channel::ALL {
            assert_eq!(Channel::from_name(c.name()), Some(c));
        }
        assert_eq!(Channel::from_name("pigtail"), Some(Channel::Pigtail));
        assert_eq!(Channel::from_name("pig"), Some(Channel::Pigtail));
        assert_eq!(Channel::from_name("tail_tip_x"), None);
        // serde: canonical "pigtail", aliases accepted.
        #[derive(serde::Deserialize)]
        struct W {
            c: Channel,
        }
        for n in ["pigtail", "pigtails", "pig"] {
            let w: W = toml::from_str(&format!("c = \"{n}\"")).unwrap();
            assert_eq!(w.c, Channel::Pigtail);
        }
    }

    #[test]
    fn engine_springs_directive_parses_to_chest_pigtail_hip() {
        let groups = SpringMask::parse_directive("disable chest/pigtails/hip for this clip");
        assert_eq!(groups, ["chest", "pigtails", "hip"]);
        let m = SpringMask::from_groups(&groups).unwrap();
        assert_eq!(
            m.channels().collect::<Vec<_>>(),
            [Channel::RootX, Channel::Chest, Channel::Pigtail]
        );
        assert!(SpringMask::parse_directive("keep everything").is_empty());
        assert_eq!(
            SpringMask::parse_directive("Disable chest, pigtails and hip"),
            ["chest", "pigtails", "hip"]
        );
        assert_eq!(SpringMask::from_groups(&["elbow"]), Err("elbow".into()));
    }

    #[test]
    fn gait_group_is_accepted_but_sets_no_channel() {
        let g = SpringMask::parse_directive("disable gait for this clip (baked walk)");
        assert_eq!(g, ["gait"]);
        assert_eq!(SpringMask::from_groups(&g), Ok(SpringMask::NONE));
        assert!(SpringMask::names_gait(&g));
        let g = SpringMask::parse_directive("disable chest/gait");
        let m = SpringMask::from_groups(&g).unwrap();
        assert_eq!(m.channels().collect::<Vec<_>>(), [Channel::Chest]);
        assert!(SpringMask::names_gait(&["Gait"]));
        assert!(!SpringMask::names_gait(&["chest", "hip"]));
        assert_eq!(SpringMask::group("gait"), None, "not a spring channel");
    }

    #[test]
    fn suppressed_channels_hold_ignore_kicks_and_resume() {
        let mut p = Physics::default();
        p.impulse(Channel::Chest, 80.0);
        p.impulse(Channel::RootY, 80.0);
        p.step(0.05);
        assert!(p.chest.position.abs() > 0.1);
        p.set_suppressed(SpringMask::of(&[Channel::Chest, Channel::Pigtail]));
        // Parked at once, kicks dropped, transform reads rest.
        assert_eq!((p.chest.position, p.chest.velocity), (0.0, 0.0));
        p.impulse(Channel::Chest, 80.0);
        p.impulse(Channel::Pigtail, 80.0);
        p.step(0.1);
        let t = p.transform();
        assert_eq!((t.chest_offset, t.pigtail_offset), (0.0, 0.0));
        assert!(t.y.abs() > 0.01, "unmasked root_y keeps moving");
        // Restored: kicks work again, starting from rest.
        p.set_suppressed(SpringMask::NONE);
        p.impulse(Channel::Chest, 80.0);
        p.step(0.05);
        assert!(p.transform().chest_offset.abs() > 0.1);
    }
}
