//! serde model of `assets/physics.toml` (schema_version 1): spring tuning,
//! impulse recipes, breathing, footfalls, moods and the procedural tail/horn
//! overlay for [`crate::physics_driver::PhysicsDriver`].
//!
//! The file is a copy of the physics-lab's authoritative
//! `/workspace/lillith/physics-lab/physics.toml`; every section is documented
//! in the TOML itself and in `assets/processed/physics/SPEC.md`. It is bundled
//! like `rules.toml` ([`PhysicsConfig::bundled`]); [`PhysicsConfig::load`]
//! reads an override from disk. Channel names are the physics.toml ones
//! (`pigtails`, see [`crate::spring::Channel::from_name`]); `tail_tip_x`,
//! `tail_tip_y` and `horn_tilt` are overlay outputs, not spring channels.

use crate::config::Springs as UserSprings;
use crate::mood::Mood;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// The bundled `assets/physics.toml` text.
pub const BUNDLED_TOML: &str = include_str!("../../../assets/physics.toml");

pub type Range = [f32; 2];

/// The whole physics.toml.
#[derive(Debug, Clone, Deserialize)]
pub struct PhysicsConfig {
    pub schema_version: u32,
    pub physics_hz: f32,
    pub units: Units,
    pub springs: Springs,
    pub gains: Gains,
    pub breathing: Breathing,
    pub walk: Walk,
    pub pigtails: Pigtails,
    pub chest: Chest,
    pub tail: Tail,
    pub horn: Horn,
    pub tattoo: Tattoo,
    pub face: Face,
    pub heartbeat: Heartbeat,
    pub idle: Idle,
    pub idle_transitions: BTreeMap<String, Vec<TransitionStep>>,
    pub moods: BTreeMap<String, MoodMul>,
    pub mood_blend: MoodBlend,
    pub drag: Drag,
    pub mischief: Mischief,
    pub impulse_recipes: BTreeMap<String, Recipe>,
    pub clip_recipes: ClipRecipes,
    pub events: BTreeMap<String, EventCfg>,
    #[serde(default)]
    pub attachments: toml::Table,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Units {
    pub px_per_cm_sprite: f32,
    pub px_per_cm_motion: f32,
    pub deg_scale_motion: f32,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct SpringCfg {
    pub stiffness: f32,
    pub damping: f32,
    pub max_velocity: f32,
    #[serde(default)]
    pub max_offset: Option<f32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Springs {
    pub root_x: SpringCfg,
    pub root_y: SpringCfg,
    pub angle: SpringCfg,
    pub scale: SpringCfg,
    pub chest: SpringCfg,
    pub pigtails: SpringCfg,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Gains {
    pub jiggle: f32,
    pub chest_bounce: f32,
    pub pigtail_sway: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Breathing {
    pub period_s: Range,
    pub inhale: f32,
    pub hold: f32,
    pub exhale: f32,
    pub settle: f32,
    pub shape: String,
    pub chest_amplitude_px: f32,
    pub scale_amplitude: f32,
    pub exhale_bounce_chest: f32,
    pub inhale_pigtails: f32,
    pub exhale_pigtails: f32,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
pub struct Footfall {
    #[serde(default)]
    pub root_y: f32,
    #[serde(default)]
    pub chest: f32,
    #[serde(default)]
    pub pigtails: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Walk {
    pub footfalls_per_cycle: u32,
    pub walk_footfall: Footfall,
    pub run_footfall: Footfall,
    pub alternate_pigtails: bool,
    pub start_tip_angle: f32,
    pub stop_chest: f32,
    pub demo_speed_px_s: f32,
    pub demo_cycle_s: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Pigtails {
    pub micro_sway_px: f32,
    pub micro_sway_period_s: f32,
    pub velocity_coupling: f32,
    pub velocity_clamp_px: f32,
    pub lag_s: f32,
    pub double_bounce_delay_s: f32,
    pub double_bounce_ratio: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Chest {
    pub secondary_delay_s: f32,
    pub double_jiggle_delay_s: f32,
    pub double_jiggle_ratio: f32,
    pub light_tap_peak_px: f32,
    pub strong_tap_peak_px: f32,
    pub sympathetic_from_pigtails: f32,
    /// Renderer hint only (spring_proposal TASTEFUL_CHANGE): visible chest cap in px.
    pub render_visual_cap_px: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tail {
    pub mode: String,
    pub figure8_period_s: Range,
    pub figure8_period_default_s: f32,
    pub figure8_amp_x_px: f32,
    pub figure8_amp_y_px: f32,
    pub inhale_lift_px: f32,
    pub tip_spring: SpringCfg,
    pub flick_light: f32,
    pub flick_strong: f32,
    pub flick_x_ratio: f32,
    pub heart_scale_pulse: f32,
    pub heart_glow_base: f32,
    pub heart_glow_breath: f32,
    pub heart_glow_flare: f32,
    pub heart_glow_tau_s: f32,
    pub trail_s: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Horn {
    pub mode: String,
    pub tilt_follow: f32,
    pub tilt_spring: SpringCfg,
    pub nebula_period_s: Range,
    pub nebula_period_default_s: f32,
    pub nebula_left_dir: f32,
    pub nebula_right_dir: f32,
    pub nebula_inhale_mult: f32,
    pub nebula_exhale_mult: f32,
    pub burst_mult: f32,
    pub burst_s: f32,
    pub burst_sparks: [u32; 2],
    pub star_twinkle_s: Range,
    pub rim_glow_base: f32,
    pub rim_glow_flare: f32,
    pub rim_glow_tau_s: f32,
    pub spike_lift_px: f32,
    pub spike_s: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tattoo {
    pub glow_base: f32,
    pub glow_breath: f32,
    pub wave_zones: Vec<String>,
    pub wave_zone_delay_s: f32,
    pub flare_light: f32,
    pub flare_strong: f32,
    pub flare_tau_light_s: f32,
    pub flare_tau_strong_s: f32,
    pub particles_per_tap: [u32; 2],
    pub particle_travel_px: Range,
    pub particle_trail_s: f32,
    pub particle_life_s: Range,
    pub idle_drift_per_breath: [u32; 2],
    pub palette: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Face {
    pub blink_every_s: Range,
    pub blink_close_s: f32,
    pub lazy_blink_close_s: f32,
    pub tongue_peek_s: f32,
    pub tongue_peek_long_s: f32,
    pub tongue_every_n_taps: [u32; 2],
    pub tongue_on_exhale_chance: f32,
    pub wink_on_strong_tap_chance: f32,
    pub oh_widen_s: f32,
    pub brow_raise_px: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Heartbeat {
    pub base_opacity: f32,
    pub pulse_amplitude: f32,
    pub bpm: f32,
    pub dub_delay_s: f32,
    pub dub_ratio: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Idle {
    pub transition_every_s: Range,
    pub transition_len_s: Range,
    pub reaction_every_s: Range,
    pub auto_gesture_cooldown_s: f32,
    pub family_order: Vec<String>,
    /// Clips with baked body motion: on_clip_start applies kicks only.
    #[serde(default)]
    pub baked_transition_clips: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImpulseCfg {
    pub channel: String,
    pub strength: f32,
    #[serde(default)]
    pub delay_s: f32,
    /// Multiply by the facing/direction sign (toward the window = +).
    #[serde(default)]
    pub signed: bool,
    #[serde(default)]
    pub double_ratio: f32,
    #[serde(default)]
    pub double_delay_s: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TransitionStep {
    pub ramp_s: f32,
    pub hold_s: f32,
    #[serde(default)]
    pub targets: BTreeMap<String, f32>,
    #[serde(default)]
    pub impulses: Vec<ImpulseCfg>,
    #[serde(default)]
    pub fx: Vec<String>,
}

/// Per-mood multipliers (1.0 = neutral).
#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
pub struct MoodMul {
    pub stiffness: f32,
    pub damping: f32,
    pub gain: f32,
    pub idle_amplitude: f32,
    pub breath_period: f32,
    pub breath_amplitude: f32,
    pub tail_amplitude: f32,
    pub tail_speed: f32,
    pub nebula_speed: f32,
    pub glow: f32,
    pub blink_interval: f32,
    pub idle_interval: f32,
}

impl MoodMul {
    pub const NEUTRAL: MoodMul = MoodMul {
        stiffness: 1.0,
        damping: 1.0,
        gain: 1.0,
        idle_amplitude: 1.0,
        breath_period: 1.0,
        breath_amplitude: 1.0,
        tail_amplitude: 1.0,
        tail_speed: 1.0,
        nebula_speed: 1.0,
        glow: 1.0,
        blink_interval: 1.0,
        idle_interval: 1.0,
    };
    pub fn lerp(a: &MoodMul, b: &MoodMul, t: f32) -> MoodMul {
        let l = |x: f32, y: f32| x + (y - x) * t;
        MoodMul {
            stiffness: l(a.stiffness, b.stiffness),
            damping: l(a.damping, b.damping),
            gain: l(a.gain, b.gain),
            idle_amplitude: l(a.idle_amplitude, b.idle_amplitude),
            breath_period: l(a.breath_period, b.breath_period),
            breath_amplitude: l(a.breath_amplitude, b.breath_amplitude),
            tail_amplitude: l(a.tail_amplitude, b.tail_amplitude),
            tail_speed: l(a.tail_speed, b.tail_speed),
            nebula_speed: l(a.nebula_speed, b.nebula_speed),
            glow: l(a.glow, b.glow),
            blink_interval: l(a.blink_interval, b.blink_interval),
            idle_interval: l(a.idle_interval, b.idle_interval),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MoodBlend {
    pub blend_s: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Drag {
    pub hold_angle_per_px_s: f32,
    pub hold_angle_clamp: f32,
    pub pickup_recipe: String,
    pub pickup_scale: f32,
    pub drop_recipe: String,
    pub drop_scale: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Mischief {
    pub rate_per_s: f32,
    pub threshold: f32,
    pub tail_amplitude_boost: f32,
    pub pigtail_gain_boost: f32,
    pub tongue_chance_boost: f32,
    pub spontaneous_flick_above: f32,
    pub spontaneous_flick_every_s: Range,
    pub on_threshold_event: String,
    pub on_threshold_mood: Mood,
    pub on_threshold_mood_s: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Recipe {
    pub impulses: Vec<ImpulseCfg>,
    #[serde(default)]
    pub fx: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClipRecipes {
    pub default: String,
    /// Extra delay so the kick lands on the clip's impact frame.
    #[serde(default)]
    pub impact_delay_s: BTreeMap<String, f32>,
    pub exact: BTreeMap<String, String>,
    pub locomotion_prefixes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventCfg {
    pub recipe: String,
    #[serde(default = "one")]
    pub scale: f32,
    /// Mood the rule switches to (applied before the kick).
    #[serde(default)]
    pub mood: Option<Mood>,
}

fn one() -> f32 {
    1.0
}

/// Moods physics.toml must define (`[moods.<Name>]`, `Mood`'s serde names).
pub const MOODS: [&str; 5] = ["Playful", "Happy", "Mischievous", "Alert", "Sleepy"];

/// `[moods]` key of a mood (its serde / `Debug` name, e.g. `"Playful"`).
pub fn mood_key(m: Mood) -> String {
    format!("{m:?}")
}

impl PhysicsConfig {
    pub fn from_toml_str(s: &str) -> Result<Self, String> {
        let c: PhysicsConfig = toml::from_str(s).map_err(|e| e.to_string())?;
        c.validate()?;
        Ok(c)
    }

    /// Reads an override file (the app's only I/O path into this module).
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let s = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_toml_str(&s)
    }

    /// The bundled `assets/physics.toml` (validated by tests).
    pub fn bundled() -> Self {
        Self::bundled_shared().clone()
    }

    /// Parsed once and shared (e.g. for the animator's default tables).
    pub fn bundled_shared() -> &'static Self {
        static CFG: OnceLock<PhysicsConfig> = OnceLock::new();
        CFG.get_or_init(|| {
            Self::from_toml_str(BUNDLED_TOML)
                .unwrap_or_else(|e| panic!("bundled assets/physics.toml is invalid:\n{e}"))
        })
    }

    /// Multipliers for `mood` (neutral if the file lacks it; `validate`
    /// requires all five).
    pub fn mood(&self, mood: Mood) -> MoodMul {
        self.moods
            .get(&mood_key(mood))
            .copied()
            .unwrap_or(MoodMul::NEUTRAL)
    }

    /// Applies the user's `config.toml [springs]` (README-SP keys) over
    /// `[springs]` stiffness/damping and `[gains]`. The defaults are identical,
    /// so this is a no-op unless the user tuned something. Clamps stay.
    pub fn apply_user_springs(&mut self, u: &UserSprings) {
        let set = |c: &mut SpringCfg, p: crate::spring::SpringParams| {
            c.stiffness = p.stiffness;
            c.damping = p.damping;
        };
        set(&mut self.springs.root_x, u.root);
        set(&mut self.springs.root_y, u.root);
        set(&mut self.springs.angle, u.angle);
        set(&mut self.springs.scale, u.scale);
        set(&mut self.springs.chest, u.chest);
        set(&mut self.springs.pigtails, u.pig);
        self.gains = Gains {
            jiggle: u.jiggle_gain,
            chest_bounce: u.chest_bounce_gain,
            pigtail_sway: u.pigtail_sway_gain,
        };
    }

    /// Cross-reference checks: channel names, recipe names, moods.
    pub fn validate(&self) -> Result<(), String> {
        let extra = ["tail_tip_x", "tail_tip_y", "horn_tilt"];
        // Exact physics.toml names only (no singular "pigtail" / "pig").
        let ch_ok = |n: &str| {
            crate::spring::Channel::from_name(n).is_some() && n != "pigtail" && n != "pig"
        };
        let mut errs = Vec::new();
        for (rn, r) in &self.impulse_recipes {
            for i in &r.impulses {
                if !ch_ok(&i.channel) {
                    errs.push(format!("recipe {rn}: bad channel {}", i.channel));
                }
            }
        }
        for (tn, steps) in &self.idle_transitions {
            for s in steps {
                for k in s.targets.keys() {
                    if !ch_ok(k) {
                        errs.push(format!("transition {tn}: bad target channel {k}"));
                    }
                }
                for i in &s.impulses {
                    if !ch_ok(&i.channel) {
                        errs.push(format!("transition {tn}: bad channel {}", i.channel));
                    }
                }
            }
        }
        for (en, e) in &self.events {
            if !self.impulse_recipes.contains_key(&e.recipe) {
                errs.push(format!("event {en}: unknown recipe {}", e.recipe));
            }
            if let Some(m) = e.mood {
                if !self.moods.contains_key(&mood_key(m)) {
                    errs.push(format!("event {en}: no [moods.{m:?}]"));
                }
            }
        }
        for (cn, r) in &self.clip_recipes.exact {
            if r != "none" && !self.impulse_recipes.contains_key(r) {
                errs.push(format!("clip {cn}: unknown recipe {r}"));
            }
        }
        for m in MOODS {
            if !self.moods.contains_key(m) {
                errs.push(format!("missing [moods.{m}]"));
            }
        }
        for n in &self.idle.family_order {
            if !self.idle_transitions.contains_key(n) {
                errs.push(format!("family_order: unknown transition {n}"));
            }
        }
        // Attachment names from assets/generated/spring_proposal.json.
        for a in [
            "chest_point",
            "pigtail_left_point",
            "pigtail_right_point",
            "hip_point",
            "tail_tip_point",
        ] {
            if let Some(toml::Value::Table(t)) = self.attachments.get(a) {
                if let Some(toml::Value::String(ch)) = t.get("channel") {
                    if !ch_ok(ch) && !extra.contains(&ch.as_str()) {
                        errs.push(format!("attachment {a}: bad channel {ch}"));
                    }
                }
            } else {
                errs.push(format!("missing [attachments.{a}]"));
            }
        }
        for r in [&self.drag.pickup_recipe, &self.drag.drop_recipe] {
            if !self.impulse_recipes.contains_key(r) {
                errs.push(format!("[drag]: unknown recipe {r}"));
            }
        }
        let r = &self.breathing;
        if ((r.inhale + r.hold + r.exhale + r.settle) - 1.0).abs() > 1e-3 {
            errs.push("breathing ratios must sum to 1".into());
        }
        if !(self.physics_hz.is_finite() && self.physics_hz > 0.0) {
            errs.push(format!("physics_hz must be > 0 (got {})", self.physics_hz));
        }
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs.join("\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_physics_toml_parses_and_cross_references() {
        let cfg = PhysicsConfig::bundled(); // panics with every validation error
        assert_eq!(cfg.schema_version, 1);
        assert_eq!(cfg.physics_hz, 120.0);
        // README-SP defaults == config.rs Springs::default().
        let d = UserSprings::default();
        assert_eq!(cfg.springs.chest.stiffness, d.chest.stiffness);
        assert_eq!(cfg.springs.chest.damping, d.chest.damping);
        assert_eq!(cfg.springs.pigtails.stiffness, d.pig.stiffness);
        assert_eq!(cfg.springs.root_x.damping, d.root.damping);
        assert_eq!(cfg.springs.angle.stiffness, d.angle.stiffness);
        assert_eq!(cfg.springs.scale.damping, d.scale.damping);
        assert_eq!(
            (
                cfg.gains.jiggle,
                cfg.gains.chest_bounce,
                cfg.gains.pigtail_sway
            ),
            (d.jiggle_gain, d.chest_bounce_gain, d.pigtail_sway_gain)
        );
        assert!(!BUNDLED_TOML.contains("channel = \"pigtail\""));
        for e in [
            "knife_tap",
            "knife_tap.strong",
            "cpu.high",
            "input.click.left",
        ] {
            assert!(cfg.events.contains_key(e), "{e}");
        }
        assert_eq!(cfg.events["cpu.high"].mood, Some(Mood::Alert));
        assert_eq!(cfg.mischief.on_threshold_mood, Mood::Mischievous);
        assert_eq!(cfg.mood(Mood::Sleepy).gain, 0.6);
        for a in [
            "chest_point",
            "pigtail_left_point",
            "pigtail_right_point",
            "hip_point",
            "tail_tip_point",
        ] {
            assert!(cfg.attachments.contains_key(a), "attachment {a}");
        }
        assert_eq!(cfg.tail.mode, "overlay");
        assert_eq!(cfg.horn.mode, "overlay");
    }

    #[test]
    fn every_rule_event_has_a_physics_recipe() {
        let rules = crate::rules::RuleSet::bundled();
        let cfg = PhysicsConfig::bundled_shared();
        let missing: Vec<&str> = rules
            .rules
            .iter()
            .map(|r| r.event.as_str())
            .filter(|e| !cfg.events.contains_key(*e))
            .collect();
        assert!(missing.is_empty(), "no [events] entry for {missing:?}");
    }

    #[test]
    fn user_springs_override_and_validation_errors() {
        let mut cfg = PhysicsConfig::bundled();
        let mut u = UserSprings::default();
        u.chest.stiffness = 80.0;
        u.pigtail_sway_gain = 1.2;
        cfg.apply_user_springs(&u);
        assert_eq!(cfg.springs.chest.stiffness, 80.0);
        assert_eq!(cfg.springs.chest.max_offset, Some(12.0), "clamps kept");
        assert_eq!(cfg.gains.pigtail_sway, 1.2);

        let bad = BUNDLED_TOML.replacen(
            "{ channel = \"angle\", strength = 0.6, signed = true },",
            "{ channel = \"elbow\", strength = 0.6, signed = true },",
            1,
        );
        assert_ne!(bad, BUNDLED_TOML);
        let err = PhysicsConfig::from_toml_str(&bad).unwrap_err();
        assert!(err.contains("bad channel elbow"), "{err}");
    }
}
