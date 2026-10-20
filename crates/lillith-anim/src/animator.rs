//! Animator: composes the base (locomotion) clip, idle life, queued
//! reactions and spring physics into "what to draw this frame"
//! (PLAN.md §3.1 layers 1, 2, 3 and 5).
//!
//! It is window-free: [`Animator::tick`] advances everything on a fake clock
//! and [`Animator::current`] reports the clip, frame index, flip and spring
//! [`Transform`]. [`crate::render`] turns that into pixels.
//!
//! Layers, top wins:
//! 1. **Reaction** (priority queue; higher priority interrupts, equal or lower
//!    waits). Loop-mode clips used as reactions play one pass.
//! 2. **Idle transition** (hip_sway, lean_forward, … for 1.2–2 s).
//! 3. **Blink** (short segment of `blink_wink`, only over the idle base).
//! 4. **Base** (idle / walk_* / run_* / sleep / sit_swing …).
//!
//! Physics: breathing drives the scale/chest spring targets; reactions apply
//! impulses on start ([`kick_for`]); walk/run/dance apply footfall impulses
//! twice per cycle; starting/stopping a walk tips the angle spring. There is
//! no random per-frame jitter. The tier values, footfalls and breathing
//! amplitudes come from the bundled `assets/physics.toml`
//! (`[impulse_recipes]` high/medium/low, `[walk]`, `[breathing]`).
//!
//! With [`AnimatorConfig::physics_driver`] on, a
//! [`lillith_core::physics_driver::PhysicsDriver`] runs the springs instead:
//! clip starts go through `on_clip_start` (per-clip recipes, impact-frame
//! delays, double bounces, baked idle clips get kicks only), walking through
//! `set_motion` (cycle-derived footfalls, pigtail trail), breathing through
//! its shaped breath curve, and [`Animator::on_event`] / [`Animator::set_mood`]
//! feed rule events and moods.

use crate::atlas::{Clip, FrameRect, LoopMode, Manifest};
use crate::player::ClipPlayer;
use lillith_core::config::Springs;
use lillith_core::idle::{IdleConfig, IdleCue, IdleLife};
use lillith_core::mood::Mood;
use lillith_core::physics_config::{PhysicsConfig, Recipe};
use lillith_core::physics_driver::{Motion, Overlay as PhysicsOverlay, PhysicsDriver};
use lillith_core::spring::{Channel, Physics, Transform};
use std::collections::VecDeque;

/// Errors from the animator.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum AnimError {
    #[error("unknown clip `{0}`")]
    UnknownClip(String),
    #[error("manifest has no clips")]
    Empty,
}

/// Horizontal facing for locomotion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
}

/// Which layer produced the current frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Base,
    Blink,
    Transition,
    Reaction,
}

/// Animator tunables.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimatorConfig {
    pub springs: Springs,
    pub physics_hz: f32,
    pub idle: IdleConfig,
    /// Seed for the idle scheduler (fixed in tests, time-based in the app).
    pub seed: u64,
    /// Breathing amplitude on the scale target (fraction, e.g. 0.006 = 0.6 %).
    pub breath_scale: f32,
    /// Breathing amplitude on the chest target (px).
    pub breath_chest: f32,
    /// Clip used for blinks and the frame range to show (`None` = auto: the
    /// first run of frames held ≤ 100 ms).
    pub blink_clip: String,
    /// Run idle life (blinks/transitions/idle reactions). Off for previews.
    pub idle_enabled: bool,
    /// Drive the springs with the physics.toml [`PhysicsDriver`] instead of
    /// the built-in kick tables (off by default; the app turns it on).
    pub physics_driver: bool,
    /// Gain on the engine's secondary motion (chest/pigtail springs and the
    /// breathing scale) while a clip tagged [`BAKED_MOTION_TAG`] is drawn.
    /// Video-based (generated_v2) clips bake bounce and hair/tail follow-through
    /// into the pixels; full engine springs on top would double the motion.
    /// 1.0 = no change, 0.0 = off. Root offset/angle (drag, footfalls) stay.
    pub baked_motion_gain: f32,
}

/// Clip tag: the frames already contain bouncy secondary motion.
pub const BAKED_MOTION_TAG: &str = "baked_motion";
/// Clip tag prefix: `variant_of:<clip>` marks an alternative take of `<clip>`.
/// While `<clip>` is the idle base, the animator draws one of its variants
/// and picks a new one at random at every loop end.
pub const VARIANT_OF_TAG: &str = "variant_of:";

impl Default for AnimatorConfig {
    fn default() -> Self {
        Self {
            springs: Springs::default(),
            physics_hz: 120.0,
            idle: IdleConfig::default(),
            seed: 0x1111_7777,
            breath_scale: physics_toml().breathing.scale_amplitude,
            // Legacy path keeps M2's +1.2 px chest breath; physics.toml uses
            // -0.6 px (inhale lifts, preview pass 2), applied in driver mode.
            breath_chest: 1.2,
            blink_clip: "blink_wink".into(),
            idle_enabled: true,
            physics_driver: false,
            baked_motion_gain: 0.2,
        }
    }
}

/// An impulse profile, in channel units per second (before gains).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Kick {
    pub root_y: f32,
    pub angle: f32,
    pub scale: f32,
    pub chest: f32,
    pub pigtail: f32,
}

impl Kick {
    pub fn is_zero(&self) -> bool {
        *self == Kick::default()
    }

    fn apply(&self, p: &mut Physics, sign: f32) {
        p.impulse(Channel::RootY, self.root_y);
        p.impulse(Channel::Angle, self.angle * sign);
        p.impulse(Channel::Scale, self.scale);
        p.impulse(Channel::Chest, self.chest);
        p.impulse(Channel::Pigtail, self.pigtail * sign);
    }
}

/// Clips that start with a big burst of energy.
pub const HIGH_ENERGY: &[&str] = &[
    "knife_tap",
    "dance",
    "tada_celebrate",
    "spell_orb_cast",
    "beam_cast",
    "wake",
    "event_cpu_busy",
    "event_error",
    "event_net_lost",
    "event_screenshot",
    "event_firewall_block",
    "event_input_click_double",
    "event_window_close",
    "event_battery_low",
    "event_ram_high",
];

/// Clips with a light start (idle transitions, thinking, small gestures).
pub const LOW_ENERGY: &[&str] = &[
    "hip_sway",
    "lean_forward",
    "shoulder_roll",
    "think",
    "nod",
    "blink_wink",
    "idle",
    "sleep",
    "sit_swing",
    "event_loading",
    "event_incognito",
];

fn physics_toml() -> &'static PhysicsConfig {
    PhysicsConfig::bundled_shared()
}

/// A physics.toml recipe flattened to one simultaneous kick (delays and
/// double bounces are the driver's job).
fn recipe_kick(name: &str) -> Kick {
    let mut k = Kick::default();
    let Some(Recipe { impulses, .. }) = physics_toml().impulse_recipes.get(name) else {
        return k;
    };
    for i in impulses {
        match Channel::from_name(&i.channel) {
            Some(Channel::RootY) => k.root_y += i.strength,
            Some(Channel::Angle) => k.angle += i.strength,
            Some(Channel::Scale) => k.scale += i.strength,
            Some(Channel::Chest) => k.chest += i.strength,
            Some(Channel::Pigtail) => k.pigtail += i.strength,
            _ => {}
        }
    }
    k
}

/// Impulse to apply when `name` starts as a reaction/transition.
/// High-energy clips get a real bounce; other gestures and every remaining
/// `event_*` clip a medium one; idle-ish clips a gentle sway. Values are
/// physics.toml `[impulse_recipes]` `high` / `medium` / `low`.
pub fn kick_for(name: &str) -> Kick {
    if HIGH_ENERGY.contains(&name) {
        recipe_kick("high")
    } else if LOW_ENERGY.contains(&name) {
        recipe_kick("low")
    } else {
        recipe_kick("medium")
    }
}

/// Footfall impulse for locomotion clips (`None` for non-locomotion).
/// Values are physics.toml `[walk]` `walk_footfall` / `run_footfall`.
pub fn footfall_for(name: &str) -> Option<Kick> {
    let w = &physics_toml().walk;
    let f = if name.starts_with("run") {
        w.run_footfall
    } else if name.starts_with("walk") || name == "dance" {
        w.walk_footfall
    } else {
        return None;
    };
    Some(Kick {
        root_y: f.root_y,
        chest: f.chest,
        pigtail: f.pigtails,
        ..Kick::default()
    })
}

#[derive(Debug, Clone)]
enum OverlayKind {
    Reaction { priority: u8 },
    Transition { remaining_s: f32 },
}

#[derive(Debug, Clone)]
struct Overlay {
    clip: String,
    player: ClipPlayer,
    flip: bool,
    kind: OverlayKind,
}

#[derive(Debug, Clone)]
struct Blink {
    frames: Vec<usize>,
    ends: Vec<f32>,
    elapsed: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Queued {
    clip: String,
    priority: u8,
    flip: bool,
}

/// What to draw now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameRef<'a> {
    pub clip_name: &'a str,
    pub clip: &'a Clip,
    pub frame: usize,
    pub rect: FrameRect,
    /// Mirror horizontally around the feet anchor.
    pub flip: bool,
    pub layer: Layer,
    pub transform: Transform,
}

/// The composed animator.
#[derive(Debug, Clone)]
pub struct Animator {
    manifest: Manifest,
    cfg: AnimatorConfig,
    base: String,
    base_flip: bool,
    base_player: ClipPlayer,
    overlay: Option<Overlay>,
    queue: VecDeque<Queued>,
    blink: Option<Blink>,
    blink_frames: Vec<usize>,
    idle: IdleLife,
    physics: Physics,
    /// Present when [`AnimatorConfig::physics_driver`] is on; owns the springs.
    driver: Option<PhysicsDriver>,
    last_key: Option<(String, usize)>,
    idle_base: String,
    /// Clips tagged `variant_of:<idle_base>` (sorted); empty = no cycling.
    idle_variants: Vec<String>,
    /// Index into `idle_variants` currently drawn for the idle base.
    idle_pick: usize,
    /// xorshift state for the variant pick (seeded from `cfg.seed`).
    variant_rng: u64,
}

/// Variant clips of `base` (tag `variant_of:<base>`), sorted by name.
fn variants_of(manifest: &Manifest, base: &str) -> Vec<String> {
    let tag = format!("{VARIANT_OF_TAG}{base}");
    let mut v: Vec<String> = manifest
        .clips
        .iter()
        .filter(|(_, c)| c.tags.iter().any(|t| *t == tag))
        .map(|(n, _)| n.clone())
        .collect();
    v.sort();
    v
}

fn once_player(clip: &Clip) -> ClipPlayer {
    let mut p = ClipPlayer::for_clip(clip);
    p.mode = LoopMode::Once;
    p
}

/// First contiguous run of frames held ≤ 100 ms (the eyes-closed part of a
/// blink clip). Falls back to the middle frame.
fn auto_blink_frames(clip: &Clip) -> Vec<usize> {
    let durs = clip.frame_durations_s();
    let mut run = Vec::new();
    for (i, d) in durs.iter().enumerate() {
        if *d <= 0.1 + 1e-6 {
            run.push(i);
        } else if !run.is_empty() {
            break;
        }
    }
    if run.is_empty() {
        run.push(clip.frames.len() / 2);
    }
    run
}

impl Animator {
    /// Builds an animator over `manifest`. The base clip is `idle` when
    /// present, else the first clip.
    pub fn new(manifest: Manifest, cfg: AnimatorConfig) -> Result<Self, AnimError> {
        let idle_base = if manifest.clips.contains_key("idle") {
            "idle".to_string()
        } else {
            manifest
                .clips
                .keys()
                .next()
                .ok_or(AnimError::Empty)?
                .clone()
        };
        let idle_variants = variants_of(&manifest, &idle_base);
        let base_player =
            ClipPlayer::for_clip(&manifest.clips[idle_variants.first().unwrap_or(&idle_base)]);
        let idle = IdleLife::with_available(cfg.idle.clone(), cfg.seed, |n| {
            manifest.clips.contains_key(n)
        });
        let blink_frames = manifest
            .clips
            .get(&cfg.blink_clip)
            .map(auto_blink_frames)
            .unwrap_or_default();
        let physics = Physics::new(&cfg.springs, cfg.physics_hz);
        let driver = cfg
            .physics_driver
            .then(|| PhysicsDriver::bundled(&cfg.springs, cfg.physics_hz, cfg.seed));
        Ok(Self {
            base: idle_base.clone(),
            idle_base,
            base_flip: false,
            base_player,
            overlay: None,
            queue: VecDeque::new(),
            blink: None,
            blink_frames,
            idle,
            physics,
            driver,
            last_key: None,
            idle_variants,
            idle_pick: 0,
            variant_rng: cfg.seed | 1,
            manifest,
            cfg,
        })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Clip names, sorted.
    pub fn clip_names(&self) -> impl Iterator<Item = &str> {
        self.manifest.clips.keys().map(String::as_str)
    }

    pub fn has_clip(&self, name: &str) -> bool {
        self.manifest.clips.contains_key(name)
    }

    pub fn physics(&self) -> &Physics {
        match &self.driver {
            Some(d) => d.physics(),
            None => &self.physics,
        }
    }

    pub fn physics_mut(&mut self) -> &mut Physics {
        match &mut self.driver {
            Some(d) => d.physics_mut(),
            None => &mut self.physics,
        }
    }

    /// The physics.toml driver, when [`AnimatorConfig::physics_driver`] is on.
    pub fn driver(&self) -> Option<&PhysicsDriver> {
        self.driver.as_ref()
    }

    pub fn driver_mut(&mut self) -> Option<&mut PhysicsDriver> {
        self.driver.as_mut()
    }

    /// Procedural tail/horn/glow overlay (driver mode only).
    pub fn physics_overlay(&self) -> Option<PhysicsOverlay> {
        self.driver.as_ref().map(PhysicsDriver::overlay)
    }

    /// Feeds a fired rule event (dotted name) to the driver: its mood and
    /// impulse recipe. Call it next to `react(clip)`; the clip start is then
    /// not kicked twice. Returns `false` without a driver or for unknown events.
    pub fn on_event(&mut self, event: &str) -> bool {
        self.driver.as_mut().is_some_and(|d| d.on_event(event))
    }

    /// Mood change for the springs (driver mode; no-op otherwise).
    pub fn set_mood(&mut self, mood: Mood) {
        if let Some(d) = &mut self.driver {
            d.set_mood(mood);
        }
    }

    /// Held by the cursor (`Some(cursor vx px/s)`) or released (`None`):
    /// dangling sway and a soft drop (driver mode; no-op otherwise).
    pub fn set_dragged(&mut self, vx: Option<f32>) {
        if let Some(d) = &mut self.driver {
            d.set_motion(match vx {
                Some(vx) => Motion::Dragged { vx },
                None => Motion::Idle,
            });
        }
    }

    pub fn idle_life(&self) -> &IdleLife {
        &self.idle
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// Frames used for a blink (empty if there is no blink clip).
    pub fn blink_frames(&self) -> &[usize] {
        &self.blink_frames
    }

    pub fn set_idle_enabled(&mut self, on: bool) {
        self.cfg.idle_enabled = on;
    }

    /// The clip actually drawn for base `name`: a variant for the idle base
    /// when variants exist, else `name` itself.
    fn drawn_base<'a>(&'a self, name: &'a str) -> &'a str {
        if name == self.idle_base && !self.idle_variants.is_empty() {
            &self.idle_variants[self.idle_pick]
        } else {
            name
        }
    }

    /// Idle variant currently drawn, if the idle base has variants.
    pub fn idle_variant(&self) -> Option<&str> {
        self.idle_variants.get(self.idle_pick).map(String::as_str)
    }

    /// Picks the next idle variant at random (never the same one twice in a
    /// row when there are two or more) and restarts the base player on it.
    fn next_idle_variant(&mut self) {
        let n = self.idle_variants.len();
        if n < 2 {
            return;
        }
        let mut x = self.variant_rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.variant_rng = x;
        let step = 1 + (x % (n as u64 - 1)) as usize;
        self.idle_pick = (self.idle_pick + step) % n;
        let clip = &self.manifest.clips[&self.idle_variants[self.idle_pick]];
        self.base_player = ClipPlayer::for_clip(clip);
    }

    /// Spring transform for a frame of `clip`: secondary channels (chest,
    /// pigtail, breathing scale) are scaled by `baked_motion_gain` when the
    /// clip is tagged [`BAKED_MOTION_TAG`].
    fn transform_for(&self, clip: &Clip) -> Transform {
        let mut t = self.physics().transform();
        if clip.tags.iter().any(|x| x == BAKED_MOTION_TAG) {
            let g = self.cfg.baked_motion_gain;
            t.chest_offset *= g;
            t.pigtail_offset *= g;
            t.scale = 1.0 + (t.scale - 1.0) * g;
        }
        t
    }

    fn clip(&self, name: &str) -> Result<&Clip, AnimError> {
        self.manifest
            .clips
            .get(name)
            .ok_or_else(|| AnimError::UnknownClip(name.to_string()))
    }

    /// Switches the base clip (restarts it), e.g. `idle`, `sleep`, `sit_swing`.
    pub fn set_base(&mut self, name: &str, flip: bool) -> Result<(), AnimError> {
        let clip = self.clip(name)?;
        if self.base != name || self.base_flip != flip {
            self.base_player = ClipPlayer::for_clip(clip);
            if name == self.idle_base && !self.idle_variants.is_empty() {
                self.next_idle_variant();
            }
            self.base = name.to_string();
            self.base_flip = flip;
        }
        Ok(())
    }

    /// Picks the walk (or run) clip for `dir`: the dedicated `<kind>_left` /
    /// `<kind>_right` clip if it exists, else the other side mirrored.
    pub fn locomotion_clip(&self, dir: Dir, running: bool) -> Option<(String, bool)> {
        let kind = if running { "run" } else { "walk" };
        let (want, other) = match dir {
            Dir::Right => (format!("{kind}_right"), format!("{kind}_left")),
            Dir::Left => (format!("{kind}_left"), format!("{kind}_right")),
        };
        if self.has_clip(&want) {
            Some((want, false))
        } else if self.has_clip(&other) {
            Some((other, true))
        } else if self.has_clip(kind) {
            // A single unsuffixed clip is assumed to face right.
            Some((kind.to_string(), dir == Dir::Left))
        } else {
            None
        }
    }

    /// Starts walking/running in `dir`, tipping the angle spring into the
    /// direction of travel.
    pub fn walk(&mut self, dir: Dir, running: bool) -> Result<(), AnimError> {
        let (name, flip) = self.locomotion_clip(dir, running).ok_or_else(|| {
            AnimError::UnknownClip(if running { "run_*" } else { "walk_*" }.into())
        })?;
        let starting = self.base != name;
        self.set_base(&name, flip)?;
        if starting {
            let s = if dir == Dir::Right { 1.0 } else { -1.0 };
            if let Some(d) = &mut self.driver {
                let vx = s * d.config().walk.demo_speed_px_s * if running { 2.0 } else { 1.0 };
                let cycle_s = self.manifest.clips[&name].duration_s();
                // Leave the current locomotion first so a direction change
                // re-tips the body.
                d.set_motion(Motion::Idle);
                d.set_motion(if running {
                    Motion::Running { vx, cycle_s }
                } else {
                    Motion::Walking { vx, cycle_s }
                });
            } else {
                let tip = physics_toml().walk.start_tip_angle;
                self.physics
                    .impulse(Channel::Angle, s * if running { 2.0 * tip } else { tip });
                self.physics.impulse(Channel::Pigtail, -s * 40.0);
            }
        }
        Ok(())
    }

    /// Back to the idle base; a walk→stop overshoots the other way.
    pub fn stand(&mut self) {
        if self.base != self.idle_base {
            let s = if self.base_flip || self.base.ends_with("_left") {
                -1.0
            } else {
                1.0
            };
            if let Some(d) = &mut self.driver {
                d.set_motion(Motion::Idle);
            } else if footfall_for(&self.base).is_some() {
                self.physics.impulse(Channel::Angle, -s * 0.3);
                self.physics.impulse(Channel::Pigtail, s * 50.0);
                self.physics.impulse(Channel::Chest, 25.0);
            }
            let idle = self.idle_base.clone();
            let _ = self.set_base(&idle, false);
        }
    }

    fn start_overlay(&mut self, clip: String, flip: bool, kind: OverlayKind) {
        let player = once_player(&self.manifest.clips[&clip]);
        let sign = if flip { -1.0 } else { 1.0 };
        match &mut self.driver {
            Some(d) => {
                d.set_facing(!flip);
                d.on_clip_start(&clip);
            }
            None => kick_for(&clip).apply(&mut self.physics, sign),
        }
        self.blink = None;
        self.overlay = Some(Overlay {
            clip,
            player,
            flip,
            kind,
        });
    }

    /// Plays `name` once as a reaction over the base. A higher `priority`
    /// interrupts the current reaction; otherwise it is queued (FIFO within
    /// a priority). Idle transitions are always interrupted.
    pub fn react(&mut self, name: &str, priority: u8) -> Result<(), AnimError> {
        self.react_flipped(name, priority, false)
    }

    /// [`Animator::react`] with an explicit mirror (e.g. point-based event
    /// clips whose icon is on screen-left, when she faces the other way).
    pub fn react_flipped(&mut self, name: &str, priority: u8, flip: bool) -> Result<(), AnimError> {
        self.clip(name)?;
        let interrupt = match &self.overlay {
            None => true,
            Some(Overlay {
                kind: OverlayKind::Transition { .. },
                ..
            }) => true,
            Some(Overlay {
                kind: OverlayKind::Reaction { priority: p },
                ..
            }) => priority > *p,
        };
        if interrupt {
            self.start_overlay(name.to_string(), flip, OverlayKind::Reaction { priority });
        } else {
            let pos = self
                .queue
                .iter()
                .position(|q| q.priority < priority)
                .unwrap_or(self.queue.len());
            self.queue.insert(
                pos,
                Queued {
                    clip: name.to_string(),
                    priority,
                    flip,
                },
            );
        }
        Ok(())
    }

    /// Plays `name` as an idle transition for `duration_s` (only if nothing
    /// else is overlaid).
    pub fn transition(&mut self, name: &str, duration_s: f32) -> Result<(), AnimError> {
        self.clip(name)?;
        if self.overlay.is_none() {
            self.start_overlay(
                name.to_string(),
                false,
                OverlayKind::Transition {
                    remaining_s: duration_s,
                },
            );
            // Transitions loop for their duration rather than one pass.
            if let Some(o) = &mut self.overlay {
                o.player.mode = LoopMode::Loop;
            }
        }
        Ok(())
    }

    /// Starts a blink if the idle base is showing and a blink clip exists.
    pub fn blink(&mut self) {
        if self.overlay.is_some() || self.base != self.idle_base || self.blink_frames.is_empty() {
            return;
        }
        let clip = &self.manifest.clips[&self.cfg.blink_clip];
        let durs = clip.frame_durations_s();
        let mut t = 0.0;
        let ends = self
            .blink_frames
            .iter()
            .map(|&i| {
                t += durs.get(i).copied().unwrap_or(0.06);
                t
            })
            .collect();
        self.blink = Some(Blink {
            frames: self.blink_frames.clone(),
            ends,
            elapsed: 0.0,
        });
    }

    /// Drops all reactions/transitions/blinks.
    pub fn clear_overlays(&mut self) {
        self.overlay = None;
        self.queue.clear();
        self.blink = None;
    }

    /// Number of queued (not yet playing) reactions.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Name of the playing reaction/transition, if any.
    pub fn overlay_clip(&self) -> Option<&str> {
        self.overlay.as_ref().map(|o| o.clip.as_str())
    }

    /// Advances clips, idle life and physics by `dt` seconds.
    pub fn tick(&mut self, dt: f32) {
        // Idle life: busy while a reaction plays or she is not on the idle base.
        let busy = matches!(
            self.overlay,
            Some(Overlay {
                kind: OverlayKind::Reaction { .. },
                ..
            })
        ) || self.base != self.idle_base;
        self.idle.set_busy(busy);
        let cues = self.idle.tick(dt);
        if self.cfg.idle_enabled {
            for cue in cues {
                match cue {
                    IdleCue::Blink => self.blink(),
                    IdleCue::Transition { clip, duration_s } => {
                        let _ = self.transition(&clip, duration_s);
                    }
                    IdleCue::Reaction { clip } => {
                        let _ = self.react(&clip, 0);
                    }
                }
            }
        }

        // Breathing drives spring targets (the springs smooth it). The
        // driver has its own shaped breath curve.
        if self.driver.is_none() {
            let b = self.idle.breath();
            self.physics
                .set_target(Channel::Scale, 1.0 + self.cfg.breath_scale * b);
            self.physics
                .set_target(Channel::Chest, self.cfg.breath_chest * b);
        }

        let before = self.base_player.frame();
        self.base_player.advance(dt);
        // Idle variants: a new random take at every loop end of the idle base.
        if self.base == self.idle_base && self.base_player.frame() < before {
            self.next_idle_variant();
        }
        if let Some(bl) = &mut self.blink {
            bl.elapsed += dt;
            if bl.elapsed >= *bl.ends.last().unwrap_or(&0.0) {
                self.blink = None;
            }
        }
        let mut done = false;
        if let Some(o) = &mut self.overlay {
            o.player.advance(dt);
            done = match &mut o.kind {
                OverlayKind::Reaction { .. } => o.player.finished(),
                OverlayKind::Transition { remaining_s } => {
                    *remaining_s -= dt;
                    *remaining_s <= 0.0
                }
            };
        }
        if done {
            self.overlay = None;
            if let Some(q) = self.queue.pop_front() {
                self.start_overlay(
                    q.clip,
                    q.flip,
                    OverlayKind::Reaction {
                        priority: q.priority,
                    },
                );
            }
        }

        // Footfalls: two impulses per locomotion cycle, on frame change.
        let (name, frame, flip) = {
            let f = self.current();
            (f.clip_name.to_string(), f.frame, f.flip)
        };
        let key = (name.clone(), frame);
        if self.last_key.as_ref() != Some(&key) {
            // (The driver derives footfalls from the locomotion cycle.)
            if let Some(kick) = footfall_for(&name).filter(|_| self.driver.is_none()) {
                let n = self.manifest.clips[&name].frames.len();
                let half = (n / 2).max(1);
                if frame % half == 0 {
                    kick.apply(&mut self.physics, if flip { -1.0 } else { 1.0 });
                }
            }
            self.last_key = Some(key);
        }

        match &mut self.driver {
            Some(d) => d.tick(dt),
            None => self.physics.step(dt),
        }
    }

    /// The frame to draw now, with the spring transform.
    pub fn current(&self) -> FrameRef<'_> {
        if let Some(o) = &self.overlay {
            let clip = &self.manifest.clips[&o.clip];
            let transform = self.transform_for(clip);
            let frame = o.player.frame().min(clip.frames.len() - 1);
            return FrameRef {
                clip_name: &o.clip,
                clip,
                frame,
                rect: clip.frames[frame],
                flip: o.flip,
                layer: match o.kind {
                    OverlayKind::Reaction { .. } => Layer::Reaction,
                    OverlayKind::Transition { .. } => Layer::Transition,
                },
                transform,
            };
        }
        if let Some(bl) = &self.blink {
            let idx = bl
                .ends
                .iter()
                .position(|&e| bl.elapsed < e)
                .unwrap_or(bl.frames.len() - 1);
            let (name, clip) = self
                .manifest
                .clips
                .get_key_value(&self.cfg.blink_clip)
                .expect("blink clip checked on start");
            let frame = bl.frames[idx].min(clip.frames.len() - 1);
            let transform = self.transform_for(clip);
            return FrameRef {
                clip_name: name,
                clip,
                frame,
                rect: clip.frames[frame],
                flip: false,
                layer: Layer::Blink,
                transform,
            };
        }
        let name = self.drawn_base(&self.base);
        let clip = &self.manifest.clips[name];
        let frame = self.base_player.frame().min(clip.frames.len() - 1);
        let transform = self.transform_for(clip);
        FrameRef {
            clip_name: name,
            clip,
            frame,
            rect: clip.frames[frame],
            flip: self.base_flip,
            layer: Layer::Base,
            transform,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic manifest: every clip is `n` frames at 10 fps.
    fn manifest() -> Manifest {
        let mut s = String::new();
        let clips: &[(&str, usize, &str)] = &[
            ("idle", 4, "loop"),
            ("walk_right", 8, "loop"),
            ("run_left", 6, "loop"),
            ("hip_sway", 4, "loop"),
            ("lean_forward", 4, "loop"),
            ("knife_tap", 5, "loop"),
            ("wave", 3, "loop"),
            ("think", 3, "loop"),
            ("event_cpu_busy", 4, "once"),
        ];
        for (name, n, mode) in clips {
            s += &format!(
                "[clips.{name}]\npage = \"{name}.png\"\nfps = 10.0\nmode = \"{mode}\"\nframes = ["
            );
            for i in 0..*n {
                s += &format!(
                    "{{ x = {}, y = 0, w = 400, h = 400, anchor = [200, 392] }},",
                    i * 400
                );
            }
            s += "]\n";
        }
        s += "[clips.blink_wink]\npage = \"b.png\"\nframe_durations_ms = [1400, 50, 70, 50, 900]\nframes = [";
        for i in 0..5 {
            s += &format!("{{ x = {}, y = 0, w = 400, h = 400 }},", i * 400);
        }
        s += "]\n";
        Manifest::from_toml_str(&s).unwrap()
    }

    fn quiet() -> AnimatorConfig {
        AnimatorConfig {
            idle_enabled: false,
            ..AnimatorConfig::default()
        }
    }

    fn run(a: &mut Animator, secs: f32) {
        for _ in 0..(secs * 60.0).round() as usize {
            a.tick(1.0 / 60.0);
        }
    }

    #[test]
    fn plays_idle_by_default() {
        let mut a = Animator::new(manifest(), quiet()).unwrap();
        assert_eq!(a.current().clip_name, "idle");
        assert_eq!(a.current().layer, Layer::Base);
        run(&mut a, 0.25);
        assert_eq!(a.current().frame, 2);
    }

    #[test]
    fn walk_uses_dedicated_or_mirrored_clip() {
        let mut a = Animator::new(manifest(), quiet()).unwrap();
        assert_eq!(
            a.locomotion_clip(Dir::Right, false),
            Some(("walk_right".into(), false))
        );
        // No walk_left: mirror walk_right.
        assert_eq!(
            a.locomotion_clip(Dir::Left, false),
            Some(("walk_right".into(), true))
        );
        assert_eq!(
            a.locomotion_clip(Dir::Left, true),
            Some(("run_left".into(), false))
        );
        assert_eq!(
            a.locomotion_clip(Dir::Right, true),
            Some(("run_left".into(), true))
        );
        a.walk(Dir::Left, false).unwrap();
        let f = a.current();
        assert_eq!((f.clip_name, f.flip), ("walk_right", true));
        a.stand();
        assert_eq!(a.current().clip_name, "idle");
        assert!(!a.current().flip);
    }

    #[test]
    fn reaction_plays_once_then_returns_to_base() {
        let mut a = Animator::new(manifest(), quiet()).unwrap();
        a.react("wave", 1).unwrap(); // loop clip, plays one pass (0.3 s)
        assert_eq!(a.current().layer, Layer::Reaction);
        run(&mut a, 0.15);
        assert_eq!((a.current().clip_name, a.current().frame), ("wave", 1));
        run(&mut a, 0.2);
        assert_eq!(a.current().clip_name, "idle");
        assert!(a.overlay_clip().is_none());
    }

    #[test]
    fn priority_interrupts_and_queue_orders() {
        let mut a = Animator::new(manifest(), quiet()).unwrap();
        a.react("think", 1).unwrap();
        a.react("wave", 1).unwrap(); // same priority: queued
        a.react("knife_tap", 0).unwrap(); // lower: queued after wave
        assert_eq!(a.overlay_clip(), Some("think"));
        assert_eq!(a.queued(), 2);
        a.react("event_cpu_busy", 5).unwrap(); // higher: interrupts
        assert_eq!(a.overlay_clip(), Some("event_cpu_busy"));
        run(&mut a, 0.42);
        assert_eq!(a.overlay_clip(), Some("wave"));
        run(&mut a, 0.31);
        assert_eq!(a.overlay_clip(), Some("knife_tap"));
        run(&mut a, 0.6);
        assert_eq!(a.overlay_clip(), None);
        assert_eq!(a.current().clip_name, "idle");
        assert!(a.react("nope", 1).is_err());
    }

    #[test]
    fn high_energy_reaction_kicks_springs() {
        let mut a = Animator::new(manifest(), quiet()).unwrap();
        run(&mut a, 2.0);
        let calm = a.physics().chest.velocity.abs();
        a.react("knife_tap", 1).unwrap();
        assert!(a.physics().chest.velocity > calm + 50.0);
        assert!(a.physics().root_y.velocity < -40.0, "hop up");
        run(&mut a, 0.08);
        let t = a.current().transform;
        assert!(t.y < -1.0 && t.chest_offset.abs() > 1.0, "{t:?}");
        // ... and settles back to breathing, not random jitter.
        a.clear_overlays();
        run(&mut a, 5.0);
        let t = a.current().transform;
        assert!(t.y.abs() < 0.05 && t.angle.abs() < 1e-3, "{t:?}");
    }

    #[test]
    fn energy_tiers() {
        assert!(kick_for("knife_tap").chest > kick_for("wave").chest);
        assert!(kick_for("wave").chest > kick_for("hip_sway").chest);
        assert_eq!(kick_for("event_usb_added"), kick_for("wave"));
        assert!(
            footfall_for("run_left").unwrap().chest > footfall_for("walk_right").unwrap().chest
        );
        assert!(footfall_for("idle").is_none());
    }

    #[test]
    fn walking_has_footfall_bounce() {
        let mut a = Animator::new(manifest(), quiet()).unwrap();
        a.walk(Dir::Right, false).unwrap();
        let mut max_chest = 0f32;
        let mut max_y = 0f32;
        for _ in 0..120 {
            a.tick(1.0 / 60.0);
            max_chest = max_chest.max(a.current().transform.chest_offset.abs());
            max_y = max_y.max(a.current().transform.y);
        }
        assert!(max_chest > 1.5, "chest {max_chest}");
        assert!(max_y > 0.1, "root y {max_y}");
    }

    #[test]
    fn idle_life_drives_blink_and_transitions() {
        let mut a = Animator::new(manifest(), AnimatorConfig::default()).unwrap();
        assert_eq!(a.blink_frames(), [1, 2, 3]);
        let mut saw = (false, false, false);
        for _ in 0..(40 * 60) {
            a.tick(1.0 / 60.0);
            match a.current().layer {
                Layer::Blink => saw.0 = true,
                Layer::Transition => {
                    saw.1 = true;
                    assert!(["hip_sway", "lean_forward"].contains(&a.current().clip_name));
                }
                Layer::Reaction => saw.2 = true,
                Layer::Base => {}
            }
        }
        assert!(
            saw.0 && saw.1 && saw.2,
            "blink/transition/reaction: {saw:?}"
        );
    }

    #[test]
    fn breathing_moves_scale() {
        let mut a = Animator::new(manifest(), quiet()).unwrap();
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for _ in 0..(10 * 60) {
            a.tick(1.0 / 60.0);
            let s = a.current().transform.scale;
            lo = lo.min(s);
            hi = hi.max(s);
        }
        assert!(hi > 1.003 && lo < 0.997, "{lo}..{hi}");
        assert!(hi < 1.02 && lo > 0.98);
    }

    #[test]
    fn reaction_cancels_transition() {
        let mut a = Animator::new(manifest(), quiet()).unwrap();
        a.transition("hip_sway", 1.5).unwrap();
        run(&mut a, 0.5);
        assert_eq!(a.current().layer, Layer::Transition);
        a.react("wave", 0).unwrap();
        assert_eq!(a.overlay_clip(), Some("wave"));
        // A transition plays for its duration, looping.
        a.clear_overlays();
        a.transition("hip_sway", 1.0).unwrap();
        run(&mut a, 0.9);
        assert_eq!(a.overlay_clip(), Some("hip_sway"));
        run(&mut a, 0.2);
        assert_eq!(a.overlay_clip(), None);
    }

    fn driven() -> AnimatorConfig {
        AnimatorConfig {
            physics_driver: true,
            ..quiet()
        }
    }

    #[test]
    fn kick_tables_come_from_physics_toml() {
        let cfg = PhysicsConfig::bundled_shared();
        let high = &cfg.impulse_recipes["high"].impulses;
        let k = kick_for("knife_tap");
        assert_eq!(
            k.chest,
            high.iter().find(|i| i.channel == "chest").unwrap().strength
        );
        assert_eq!(k.root_y, -90.0);
        assert_eq!(
            footfall_for("walk_right").unwrap().chest,
            cfg.walk.walk_footfall.chest
        );
        assert_eq!(
            footfall_for("run_left").unwrap().root_y,
            cfg.walk.run_footfall.root_y
        );
        let d = AnimatorConfig::default();
        assert_eq!(d.breath_scale, cfg.breathing.scale_amplitude);
        assert_eq!(d.breath_chest, 1.2, "legacy M2 chest breath");
        assert!(!d.physics_driver);
    }

    #[test]
    fn driver_mode_knife_tap_lands_on_impact_frame() {
        let mut a = Animator::new(manifest(), driven()).unwrap();
        assert!(a.driver().is_some());
        run(&mut a, 2.0);
        let calm = a.physics().chest.velocity.abs();
        a.react("knife_tap", 1).unwrap();
        run(&mut a, 0.3); // impact frame is 0.4 s in
        assert!(a.physics().chest.velocity.abs() < calm + 5.0, "too early");
        let mut peak = 0f32;
        for _ in 0..60 {
            a.tick(1.0 / 60.0);
            peak = peak.max(a.current().transform.chest_offset.abs());
        }
        assert!(peak > 1.0, "knife_tap chest jiggle {peak}");
        assert!(a.physics_overlay().is_some());
    }

    #[test]
    fn driver_mode_cpu_high_event_and_clip() {
        let mut a = Animator::new(manifest(), driven()).unwrap();
        run(&mut a, 1.0);
        assert!(a.on_event("cpu.high"));
        assert_eq!(a.driver().unwrap().mood(), Mood::Alert);
        a.react("event_cpu_busy", 5).unwrap(); // deduped against the event
        let mut hop = 0f32;
        for _ in 0..60 {
            a.tick(1.0 / 60.0);
            hop = hop.min(a.current().transform.y);
        }
        assert!(hop < -2.0, "hop {hop}");
        assert!(!a.on_event("no.such.event"));
        // Without a driver, events are ignored.
        let mut plain = Animator::new(manifest(), quiet()).unwrap();
        assert!(!plain.on_event("cpu.high"));
    }

    #[test]
    fn driver_mode_walk_bounces_and_settles() {
        let mut a = Animator::new(manifest(), driven()).unwrap();
        a.walk(Dir::Right, false).unwrap();
        let mut max_chest = 0f32;
        for _ in 0..180 {
            a.tick(1.0 / 60.0);
            max_chest = max_chest.max(a.current().transform.chest_offset.abs());
        }
        assert!(max_chest > 1.0, "chest {max_chest}");
        a.stand();
        run(&mut a, 5.0);
        let t = a.current().transform;
        assert!(t.y.abs() < 0.1 && t.angle.abs() < 0.01, "{t:?}");
        a.set_dragged(Some(300.0));
        run(&mut a, 1.0);
        assert!(a.current().transform.angle.abs() > 0.05, "dangling sway");
        a.set_dragged(None);
    }

    /// idle (alias) + two tagged variants with baked motion, 4 frames @ 10 fps.
    fn variant_manifest() -> Manifest {
        let mut s = String::new();
        for (name, tags) in [
            ("idle", "[\"baked_motion\"]"),
            ("idle_a", "[\"baked_motion\", \"variant_of:idle\"]"),
            ("idle_b", "[\"baked_motion\", \"variant_of:idle\"]"),
            ("wave", "[]"),
        ] {
            s += &format!("[clips.{name}]\npage = \"{name}.png\"\nfps = 10.0\nmode = \"loop\"\ntags = {tags}\nframes = [");
            for i in 0..4 {
                s += &format!(
                    "{{ x = {}, y = 0, w = 400, h = 400, anchor = [200, 392] }},",
                    i * 400
                );
            }
            s += "]\n";
        }
        Manifest::from_toml_str(&s).unwrap()
    }

    #[test]
    fn idle_variants_cycle_at_loop_end() {
        let mut a = Animator::new(variant_manifest(), quiet()).unwrap();
        assert_eq!(a.base(), "idle");
        let mut seen = std::collections::BTreeSet::new();
        let mut last = a.current().clip_name.to_string();
        let mut switches = 0;
        for _ in 0..(20 * 60) {
            a.tick(1.0 / 60.0);
            let c = a.current().clip_name.to_string();
            assert!(c == "idle_a" || c == "idle_b", "drew {c}");
            if c != last {
                switches += 1;
                last = c.clone();
            }
            seen.insert(c);
        }
        assert_eq!(seen.len(), 2, "both variants drawn");
        // 0.4 s loops for 20 s: ~50 loop ends, two variants -> always alternate.
        assert!(switches > 30, "{switches} switches");
        // Leaving and re-entering idle keeps drawing a variant.
        a.set_base("wave", false).unwrap();
        a.tick(0.05);
        assert_eq!(a.current().clip_name, "wave");
        a.stand();
        assert!(a.current().clip_name.starts_with("idle_"));
    }

    #[test]
    fn baked_motion_clips_damp_secondary_springs() {
        let cfg = AnimatorConfig {
            baked_motion_gain: 0.0,
            ..quiet()
        };
        let mut a = Animator::new(variant_manifest(), cfg).unwrap();
        a.physics_mut().impulse(Channel::Chest, 80.0);
        a.physics_mut().impulse(Channel::Pigtail, 80.0);
        let mut max_baked: f32 = 0.0;
        for _ in 0..60 {
            a.tick(1.0 / 60.0);
            let t = a.current().transform;
            max_baked = max_baked
                .max(t.chest_offset.abs())
                .max(t.pigtail_offset.abs())
                .max((t.scale - 1.0).abs());
        }
        assert!(max_baked < 1e-6, "baked clip still moved {max_baked}");
        // Untagged clip: full springs.
        a.set_base("wave", false).unwrap();
        a.physics_mut().impulse(Channel::Chest, 80.0);
        let mut max_plain: f32 = 0.0;
        for _ in 0..60 {
            a.tick(1.0 / 60.0);
            max_plain = max_plain.max(a.current().transform.chest_offset.abs());
        }
        assert!(
            max_plain > 0.5,
            "untagged clip lost its springs {max_plain}"
        );
    }
}
