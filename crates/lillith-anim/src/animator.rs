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
use lillith_core::spring::{Channel, Physics, SpringMask, Transform};
use std::collections::{BTreeMap, VecDeque};

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
    /// Switch the engine's chest, pigtail and hip (`root_x`) springs fully
    /// off while a clip tagged [`BAKED_MOTION_TAG`] is drawn, exactly as if
    /// the clip listed them in `disable_springs` (same [`SpringMask`] path:
    /// parked at rest, kicks dropped). The v2 video clips bake that jiggle
    /// into the pixels, so engine springs on top would double it. Breathing
    /// scale is still damped by `baked_motion_gain`; drag sway (`angle`) and
    /// footfall bob (`root_y`) stay.
    pub baked_motion_springs_off: bool,
    /// Global clip playback speed: frame timing (fps / `frame_durations_ms`)
    /// of every clip is stretched by `1 / playback_speed` (0.7 = 30%
    /// slower). Physics, breathing and idle timers keep real time.
    pub playback_speed: f32,
    /// Lip-flap pace while she talks (1.0 = original fast pace; lower is
    /// slower). See [`crate::talk::LipFlap::with_speed`].
    pub talk_speed: f32,
    /// Draw a clip's baked-jiggle sibling (`jiggle_variant`) in its place
    /// whenever one exists. The sibling's `disable_springs` then switches the
    /// matching engine springs off while it is drawn (no double bounce).
    pub prefer_jiggle_variants: bool,
    /// Play the manifest's blend helpers (`blend_idle_to_walk_*`,
    /// `blend_walk_*_to_idle`) when a walk starts from idle or stops,
    /// instead of cutting straight between the front-facing idle and the
    /// side-on walk. The stop waits (at most one cycle) for the walk frame
    /// the blend was cut from, so the legs don't jump.
    pub walk_blends: bool,
    /// Clips always drawn engine-clean even when
    /// [`AnimatorConfig::prefer_jiggle_variants`] is on (e.g. the talk
    /// clips when `[chat] talk_jiggle = false`).
    pub no_jiggle_clips: Vec<String>,
    /// Smooth mode: cross-fade into the next frame over the last
    /// `crossfade` part of each frame (0 = hard frame steps).
    pub smooth_frames: bool,
    /// Fraction (0..1) of each frame spent cross-fading into the next.
    pub crossfade: f32,
}

/// Clip tag: the frames already contain bouncy secondary motion.
pub const BAKED_MOTION_TAG: &str = "baked_motion";
/// Clip tag prefix: `variant_of:<clip>` marks an alternative take of `<clip>`.
/// With [`IdleConfig::variant_play_once`] (default) any tagged base (idle,
/// walk_right/left, sit_swing) loops its clean clip for
/// [`IdleConfig::variant_hold_s`], then at a loop end plays one random
/// variant once and returns to the base. Without it (legacy), only the idle
/// base cycles: one variant loops until the hold is over, then a new one is
/// picked at a loop end.
pub const VARIANT_OF_TAG: &str = "variant_of:";

/// Sultry walk takes (name prefixes + `_left` / `_right`). When a walk base
/// has any of these among its takes, only these are drawn as walk variants
/// (the skip and other playful takes are left out); `strut_<dir>` joins
/// `walk_<dir>`'s takes even without a `variant_of` tag.
pub const SULTRY_WALK_TAKES: &[&str] = &["walk_saunter_", "walk_sassy_", "strut_"];

/// Adds `strut_<dir>` to `walk_<dir>` and narrows walk takes to the sultry
/// ones when any exist (see [`SULTRY_WALK_TAKES`]).
pub fn prefer_sultry_walks(manifest: &Manifest, map: &mut BTreeMap<String, Vec<String>>) {
    for dir in ["left", "right"] {
        let base = format!("walk_{dir}");
        if !manifest.clips.contains_key(&base) {
            continue;
        }
        let strut = format!("strut_{dir}");
        let v = map.entry(base.clone()).or_default();
        if manifest.clips.contains_key(&strut) && !v.contains(&strut) {
            v.push(strut);
        }
        let sultry: Vec<String> = v
            .iter()
            .filter(|n| SULTRY_WALK_TAKES.iter().any(|p| n.starts_with(p)))
            .cloned()
            .collect();
        if !sultry.is_empty() {
            *v = sultry;
        }
        v.sort();
        v.dedup();
        if v.is_empty() {
            map.remove(&base);
        }
    }
}

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
            baked_motion_springs_off: true,
            playback_speed: 1.0,
            talk_speed: crate::talk::DEFAULT_TALK_SPEED,
            prefer_jiggle_variants: false,
            walk_blends: true,
            no_jiggle_clips: Vec::new(),
            smooth_frames: false,
            crossfade: 0.5,
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
    /// Clip drawn for it (its baked-jiggle sibling, or `clip`), fixed at
    /// start: the player is timed for this clip's own frames.
    drawn: String,
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
    /// Mouth patch to paste over the face (true lip-sync while talking).
    pub mouth: Option<MouthDraw<'a>>,
    /// Smooth mode: the next frame of the same clip and its cross-fade
    /// weight (0..1) toward the end of the current frame.
    pub blend: Option<(usize, f32)>,
}

impl FrameRef<'_> {
    /// Frame of the clean clip this frame shows (differs from `frame` on
    /// baked-jiggle clips with settle sub-frames, see
    /// [`Clip::clean_frame_index`]).
    pub fn clean_frame(&self) -> usize {
        self.clip.clean_frame(self.frame)
    }
}

/// A mouth patch placed on the current frame, in cell coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MouthDraw<'a> {
    pub patch: &'a crate::atlas::MouthPatch,
    pub shape: crate::talk::Mouth,
    /// Where the patch centre lands (cell px).
    pub point: (f32, f32),
    /// Rotation about `point` (degrees, + = clockwise).
    pub angle_deg: f32,
}

/// Mouth patch for `shape` on `frame` of `clip` (patches/points looked up
/// in `clip`, then in `fallback`, e.g. the clean clip of a jiggle sibling).
pub fn mouth_draw<'a>(
    clip: &'a Clip,
    fallback: Option<&'a Clip>,
    frame: usize,
    shape: crate::talk::Mouth,
) -> Option<MouthDraw<'a>> {
    let key = shape.name();
    let patch = clip
        .mouth_patches
        .get(key)
        .or_else(|| fallback.and_then(|f| f.mouth_patches.get(key)))?;
    let point = clip
        .frame_points
        .get("mouth_point")
        .and_then(|v| v.get(frame))
        .or_else(|| clip.points.get("mouth_point"))
        .map(|&(x, y)| (x as f32, y as f32))?;
    let angle_deg = clip.mouth_angle_deg.get(frame).copied().unwrap_or(0.0);
    Some(MouthDraw {
        patch,
        shape,
        point,
        angle_deg,
    })
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
    /// `variant_of:<base>` tags: base clip -> its variant clips (sorted).
    variants: BTreeMap<String, Vec<String>>,
    /// Per base with variants: the take drawn now and its age.
    takes: BTreeMap<String, VariantTake>,
    /// xorshift state for the variant pick (seeded from `cfg.seed`).
    variant_rng: u64,
    /// Springs switched off for the clip drawn now (its `disable_springs`).
    spring_mask: SpringMask,
    /// Root motion (cell px, +x = right on screen) the drawn locomotion
    /// frames advanced since the last [`Animator::take_root_motion`].
    root_dx: f32,
    /// Footfalls fired so far (alternates the pigtail sway).
    foot: u32,
    /// A walk→idle blend waiting for its matching walk frame.
    stopping: Option<String>,
    /// Seconds spent waiting in `stopping`.
    stop_wait_s: f32,
    /// After an idle→walk blend: the walk clip and the frame to resume on.
    entry: Option<(String, usize)>,
    /// Externally chosen base frame (lip-flap on `talk_loop`); cleared
    /// whenever the base clip changes.
    pinned_base_frame: Option<usize>,
    /// Mouth shape to paste on the base clip (true lip-sync); cleared
    /// whenever the base clip changes.
    mouth: Option<crate::talk::Mouth>,
}

/// Which take of a base with variants is drawn, and for how long.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct VariantTake {
    /// Index into the base's variants; `None` = the clean base clip.
    pick: Option<usize>,
    /// Last variant played (play-once mode avoids repeating it).
    last: Option<usize>,
    /// Seconds on the current take (play-once: since the last variant).
    age: f32,
}

/// `playback_speed` sanitised to 0.1..=4 (non-finite = 1).
fn clamp_speed(s: f32) -> f32 {
    if s.is_finite() {
        s.clamp(0.1, 4.0)
    } else {
        1.0
    }
}

/// Base clip -> its variant clips (tag `variant_of:<base>`, sorted by
/// name), for every base that exists in `manifest`. A clip is never its
/// own variant.
pub fn variant_map(manifest: &Manifest) -> BTreeMap<String, Vec<String>> {
    let mut m: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, clip) in &manifest.clips {
        for t in &clip.tags {
            if let Some(base) = t.strip_prefix(VARIANT_OF_TAG) {
                let base = base.trim();
                if base != name && manifest.clips.contains_key(base) {
                    m.entry(base.to_string()).or_default().push(name.clone());
                }
            }
        }
    }
    for v in m.values_mut() {
        v.sort();
        v.dedup();
    }
    m
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
        let mut variants = variant_map(&manifest);
        prefer_sultry_walks(&manifest, &mut variants);
        // Legacy mode starts on the first idle variant; play-once on idle.
        let first = match variants.get(&idle_base) {
            Some(v) if !cfg.idle.variant_play_once => &v[0],
            _ => &idle_base,
        };
        let base_player = ClipPlayer::for_clip(&manifest.clips[first]);
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
        let mut a = Self {
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
            variants,
            takes: BTreeMap::new(),
            variant_rng: cfg.seed | 1,
            pinned_base_frame: None,
            mouth: None,
            spring_mask: SpringMask::NONE,
            root_dx: 0.0,
            foot: 0,
            stopping: None,
            stop_wait_s: 0.0,
            entry: None,
            manifest,
            cfg,
        };
        // The starting clip's spring mask (e.g. a baked_motion idle).
        a.sync_spring_mask();
        Ok(a)
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

    /// `true`: variants play once and hand back to their base
    /// ([`IdleConfig::variant_play_once`]).
    fn play_once(&self) -> bool {
        self.cfg.idle.variant_play_once
    }

    /// Variant clips that may be drawn for base `name` (sorted): its
    /// `variant_of:<name>` clips; legacy mode only cycles the idle base.
    /// A mirrored walk (`walk_right` drawn flipped for a left walk, see
    /// [`Animator::locomotion_clip`]) uses the drawn clip's variants,
    /// flipped the same way.
    pub fn variants(&self, name: &str) -> &[String] {
        if !self.play_once() && name != self.idle_base {
            return &[];
        }
        self.variants.get(name).map_or(&[], Vec::as_slice)
    }

    /// The take state of base `name` (a fresh one if never drawn).
    fn take(&self, name: &str) -> VariantTake {
        self.takes.get(name).copied().unwrap_or(VariantTake {
            // Legacy mode always draws a variant, starting with the first.
            pick: (!self.play_once()).then_some(0),
            ..VariantTake::default()
        })
    }

    /// The clip actually drawn for base `name`: its current variant take
    /// when one is on, else `name` itself.
    fn drawn_base<'a>(&'a self, name: &'a str) -> &'a str {
        let v = self.variants(name);
        match self.take(name).pick {
            Some(i) if i < v.len() => &v[i],
            _ => name,
        }
    }

    /// Variant currently drawn for the base clip, if any (e.g. `idle_yawn_stretch`).
    pub fn base_variant(&self) -> Option<&str> {
        let d = self.drawn_base(&self.base);
        (d != self.base).then_some(d)
    }

    /// Idle variant currently drawn, if the idle base shows one.
    pub fn idle_variant(&self) -> Option<&str> {
        let d = self.drawn_base(&self.idle_base);
        (d != self.idle_base).then_some(d)
    }

    fn next_rng(&mut self) -> u64 {
        let mut x = self.variant_rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.variant_rng = x;
        x
    }

    /// Picks the next variant of the current base at random (never the
    /// same one twice in a row when there are two or more) and restarts the
    /// base player on it: once (play-once mode) or looping (legacy).
    fn next_variant(&mut self) {
        let n = self.variants(&self.base).len();
        let mut t = self.take(&self.base);
        let pick = if self.play_once() {
            if n == 0 {
                return;
            }
            match t.last {
                Some(l) if n >= 2 => (l + 1 + (self.next_rng() % (n as u64 - 1)) as usize) % n,
                _ => (self.next_rng() % n as u64) as usize,
            }
        } else {
            if n < 2 {
                return;
            }
            let cur = t.pick.unwrap_or(0);
            (cur + 1 + (self.next_rng() % (n as u64 - 1)) as usize) % n
        };
        t.pick = Some(pick);
        t.last = Some(pick);
        t.age = 0.0;
        self.takes.insert(self.base.clone(), t);
        self.base_player = self.base_player_for_drawn();
    }

    /// Play-once mode: a variant pass ended, back to the clean base.
    fn end_variant(&mut self) {
        let mut t = self.take(&self.base);
        t.pick = None;
        t.age = 0.0;
        self.takes.insert(self.base.clone(), t);
        self.base_player = self.base_player_for_drawn();
    }

    /// `true` once the current take of the base has been on long enough
    /// to switch.
    fn variant_due(&self) -> bool {
        let hold = if self.base.starts_with("walk_") {
            self.cfg.idle.walk_variant_hold_s
        } else {
            self.cfg.idle.variant_hold_s
        };
        self.take(&self.base).age >= hold
    }

    /// A fresh base player timed for the clip actually drawn for the base
    /// (variant / baked-jiggle sibling: their frame counts and durations
    /// can differ from the clean clip's). A play-once variant gets a
    /// `Once` player.
    fn base_player_for_drawn(&self) -> ClipPlayer {
        let name = self.drawn(self.drawn_base(&self.base));
        let clip = &self.manifest.clips[name];
        if self.play_once() && self.base_variant().is_some() {
            once_player(clip)
        } else {
            ClipPlayer::for_clip(clip)
        }
    }

    /// Cross-fade target for a player in smooth mode.
    fn blend_for(&self, p: &ClipPlayer, frames: usize) -> Option<(usize, f32)> {
        if !self.cfg.smooth_frames || frames < 2 {
            return None;
        }
        let cf = self.cfg.crossfade.clamp(0.0, 1.0);
        if cf <= 0.0 {
            return None;
        }
        let (f, next, frac) = p.phase();
        let next = next.filter(|&n| n < frames && n != f)?;
        let x = ((frac - (1.0 - cf)) / cf).clamp(0.0, 1.0);
        let w = x * x * (3.0 - 2.0 * x);
        (w > 0.0).then_some((next, w))
    }

    /// Smooth mode on/off and its cross-fade fraction (live, Settings).
    pub fn set_smooth_frames(&mut self, on: bool, crossfade: f32) {
        self.cfg.smooth_frames = on;
        self.cfg.crossfade = crossfade.clamp(0.0, 1.0);
    }

    /// Clip playback speed (live, Settings).
    pub fn set_playback_speed(&mut self, speed: f32) {
        self.cfg.playback_speed = speed;
    }

    /// Effective clip playback speed (sanitised).
    pub fn playback_speed(&self) -> f32 {
        clamp_speed(self.cfg.playback_speed)
    }

    /// Spring tuning + gains (live, Settings): applied to the driver's
    /// config (mood multipliers stay on top) or the plain springs.
    pub fn set_springs(&mut self, s: &lillith_core::config::Springs) {
        self.cfg.springs = s.clone();
        match &mut self.driver {
            Some(d) => d.set_user_springs(s),
            None => {
                let p = &mut self.physics;
                p.set_params(Channel::Chest, s.chest);
                p.set_params(Channel::Pigtail, s.pig);
                p.set_params(Channel::RootX, s.root);
                p.set_params(Channel::RootY, s.root);
                p.set_params(Channel::Angle, s.angle);
                p.set_params(Channel::Scale, s.scale);
                p.set_gains(s.jiggle_gain, s.chest_bounce_gain, s.pigtail_sway_gain);
            }
        }
    }

    /// Clip drawn in place of `name` (its baked-jiggle sibling when
    /// [`AnimatorConfig::prefer_jiggle_variants`] is on, else `name`).
    pub fn drawn_clip<'a>(&'a self, name: &'a str) -> &'a str {
        if self.manifest.clips.contains_key(name) {
            self.drawn(name)
        } else {
            name
        }
    }

    /// Engine spring channels switched off while `clip` is drawn: its
    /// `disable_springs`, plus chest / pigtails / hip for [`BAKED_MOTION_TAG`]
    /// clips when [`AnimatorConfig::baked_motion_springs_off`] is on.
    pub fn clip_mask(&self, clip: &Clip) -> SpringMask {
        let mut m = clip.spring_mask();
        if self.cfg.baked_motion_springs_off && clip.has_tag(BAKED_MOTION_TAG) {
            for c in [Channel::Chest, Channel::Pigtail, Channel::RootX] {
                m.insert(c);
            }
        }
        m
    }

    /// Spring transform for a frame of `clip`: secondary channels (chest,
    /// pigtail, breathing scale) are scaled by `baked_motion_gain` when the
    /// clip is tagged [`BAKED_MOTION_TAG`].
    fn transform_for(&self, clip: &Clip) -> Transform {
        let mut t = self.physics().transform();
        // Baked-jiggle clips: their disabled springs read as rest, even
        // before the next tick parks them (see `sync_spring_mask`).
        let m = self.clip_mask(clip);
        let rest = Transform::IDENTITY;
        if m.contains(Channel::RootX) {
            t.x = rest.x;
        }
        if m.contains(Channel::RootY) {
            t.y = rest.y;
        }
        if m.contains(Channel::Angle) {
            t.angle = rest.angle;
        }
        if m.contains(Channel::Scale) {
            t.scale = rest.scale;
        }
        if m.contains(Channel::Chest) {
            t.chest_offset = rest.chest_offset;
        }
        if m.contains(Channel::Pigtail) {
            t.pigtail_offset = rest.pigtail_offset;
        }
        if clip.tags.iter().any(|x| x == BAKED_MOTION_TAG) {
            let g = self.cfg.baked_motion_gain;
            t.chest_offset *= g;
            t.pigtail_offset *= g;
            t.scale = 1.0 + (t.scale - 1.0) * g;
        }
        if m.contains(Channel::Chest) {
            // chest bounce > 100 %: a light engine layer over the baked art
            t.chest_offset = self.physics().layered_chest_offset();
        }
        t
    }

    /// The clip actually drawn for `name`: its baked-jiggle sibling when
    /// [`AnimatorConfig::prefer_jiggle_variants`] is on and one exists.
    fn drawn<'a>(&'a self, name: &'a str) -> &'a str {
        if self.cfg.prefer_jiggle_variants && !self.cfg.no_jiggle_clips.iter().any(|c| c == name) {
            if let Some(v) = self.manifest.clips[name]
                .jiggle_variant
                .as_deref()
                .filter(|v| self.manifest.clips.contains_key(*v))
            {
                return v;
            }
        }
        name
    }

    /// Draw baked-jiggle siblings in place of clean clips (see
    /// [`AnimatorConfig::prefer_jiggle_variants`]).
    pub fn set_prefer_jiggle_variants(&mut self, on: bool) {
        if self.cfg.prefer_jiggle_variants != on {
            self.cfg.prefer_jiggle_variants = on;
            self.base_player = self.base_player_for_drawn();
            self.pinned_base_frame = None;
        }
        self.sync_spring_mask();
    }

    /// Engine springs switched off right now (the drawn clip's
    /// `disable_springs`; empty for engine-clean clips).
    pub fn spring_mask(&self) -> SpringMask {
        self.spring_mask
    }

    /// Applies the drawn clip's `disable_springs` to the physics: the named
    /// channels are parked at rest and ignore kicks while it is drawn, and
    /// come back (from rest) as soon as a clip without them is drawn. A
    /// `gait` entry ([`Clip::bakes_gait`]) suppresses the driver's engine
    /// gait the same way.
    fn sync_spring_mask(&mut self) {
        let clip = self.current().clip;
        let (mask, gait) = (self.clip_mask(clip), clip.bakes_gait());
        self.apply_clip_mask(mask, gait);
    }

    fn apply_clip_mask(&mut self, mask: SpringMask, bakes_gait: bool) {
        if mask != self.spring_mask {
            self.spring_mask = mask;
            self.physics_mut().set_suppressed(mask);
        }
        if let Some(d) = &mut self.driver {
            d.set_gait_suppressed(bakes_gait);
        }
    }

    fn clip(&self, name: &str) -> Result<&Clip, AnimError> {
        self.manifest
            .clips
            .get(name)
            .ok_or_else(|| AnimError::UnknownClip(name.to_string()))
    }

    /// Switches the base clip (restarts it), e.g. `idle`, `sleep`, `sit_swing`.
    pub fn set_base(&mut self, name: &str, flip: bool) -> Result<(), AnimError> {
        self.clip(name)?;
        if self.base != name || self.base_flip != flip {
            if self.stopping.take().is_some() {
                // A pending walk stop overtaken by another base (e.g. a
                // drag): the locomotion still has to end.
                if let Some(d) = &mut self.driver {
                    d.set_motion(Motion::Idle);
                }
            }
            self.entry = None;
            self.base = name.to_string();
            if self.play_once() && self.takes.contains_key(name) {
                // Play-once: (re)entering a base always starts on the clean
                // clip (blends and walk entry frames are cut from it); the
                // hold keeps counting from where it was.
                let mut t = self.take(name);
                t.pick = None;
                self.takes.insert(name.to_string(), t);
            }
            self.base_player = self.base_player_for_drawn();
            // Legacy, back to idle: a new take once the current one was
            // held long enough, else resume the same take
            // (`base_player_for_drawn` already picked it).
            if !self.play_once() && !self.variants(name).is_empty() && self.variant_due() {
                self.next_variant();
            }
            self.base_flip = flip;
            self.pinned_base_frame = None;
            self.mouth = None;
        }
        self.sync_spring_mask();
        Ok(())
    }

    /// Draws frame `frame` of the base clip instead of the player's frame
    /// (`None` = play normally). Used for token-driven lip-flap on the
    /// `talk_loop` base; reactions still draw over it. Changing the base
    /// clip clears the pin.
    pub fn pin_base_frame(&mut self, frame: Option<usize>) {
        self.pinned_base_frame = frame;
    }

    /// Pastes the `shape` mouth patch on the base clip's frames (`None` =
    /// the frames' own mouths). Only clips with `mouth_patches` and a
    /// `mouth_point` show it.
    pub fn set_mouth(&mut self, shape: Option<crate::talk::Mouth>) {
        self.mouth = shape;
    }

    pub fn mouth(&self) -> Option<crate::talk::Mouth> {
        self.mouth
    }

    /// Frame the base player is on (ignores the pin).
    pub fn base_frame(&self) -> usize {
        self.base_player.frame()
    }

    /// The pinned base frame, if any.
    pub fn pinned_base_frame(&self) -> Option<usize> {
        self.pinned_base_frame
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
        let starting = self.base != name || self.base_flip != flip;
        let from_idle = self.base == self.idle_base;
        // A new walk order cancels a pending stop.
        self.stopping = None;
        self.set_base(&name, flip)?;
        if starting {
            let s = if dir == Dir::Right { 1.0 } else { -1.0 };
            let frame_footfalls = self.manifest.clips[&name].contact_frames.is_some();
            if let Some(d) = &mut self.driver {
                d.set_frame_footfalls(frame_footfalls);
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
            self.foot = 0;
            if from_idle && self.cfg.walk_blends && self.overlay.is_none() {
                let idle = self.idle_base.clone();
                if let Some(b) = self.blend_between(&idle, &name) {
                    let to = self.manifest.clips[&b]
                        .blend
                        .as_ref()
                        .map_or(0, |bl| bl.to_frame as usize);
                    let dur = ClipPlayer::for_clip(&self.manifest.clips[&b]).duration_s();
                    self.start_overlay(b, flip, OverlayKind::Transition { remaining_s: dur });
                    self.entry = Some((name.clone(), to));
                }
            }
        }
        Ok(())
    }

    /// The blend helper clip from `from` to `to`, if the manifest has one.
    fn blend_between(&self, from: &str, to: &str) -> Option<String> {
        self.manifest
            .clips
            .iter()
            .find(|(_, c)| {
                c.blend
                    .as_ref()
                    .is_some_and(|b| b.from == from && b.to == to)
            })
            .map(|(n, _)| n.clone())
    }

    /// `root_motion` of the drawn clip `name` (or of the clean clip a
    /// baked-jiggle sibling was made from).
    fn root_motion_of(&self, name: &str) -> Option<&[f32]> {
        let c = self.manifest.clips.get(name)?;
        c.root_motion.as_deref().or_else(|| {
            c.jiggle_of
                .as_deref()
                .and_then(|o| self.manifest.clips.get(o))
                .and_then(|o| o.root_motion.as_deref())
        })
    }

    /// `contact_frames` of the drawn clip `name` (see [`Self::root_motion_of`]).
    fn contact_frames_of(&self, name: &str) -> Option<&[usize]> {
        let c = self.manifest.clips.get(name)?;
        c.contact_frames.as_deref().or_else(|| {
            c.jiggle_of
                .as_deref()
                .and_then(|o| self.manifest.clips.get(o))
                .and_then(|o| o.contact_frames.as_deref())
        })
    }

    /// `true` if the base clip is a locomotion cycle with `root_motion`: the
    /// window should then move by [`Animator::take_root_motion`] instead of
    /// at a fixed speed, so the planted foot stays put on the desktop.
    pub fn base_has_root_motion(&self) -> bool {
        self.root_motion_of(self.drawn_base(&self.base)).is_some()
    }

    /// Ground advance (cell px, +x = right on screen) of the locomotion
    /// frames drawn since the last call; resets it. Multiply by the overlay
    /// scale for screen px.
    pub fn take_root_motion(&mut self) -> f32 {
        std::mem::take(&mut self.root_dx)
    }

    /// `true` while a stop is waiting for the walk→idle blend's frame.
    pub fn stopping(&self) -> bool {
        self.stopping.is_some()
    }

    /// Back to the idle base; a walk→stop overshoots the other way. With
    /// [`AnimatorConfig::walk_blends`] and a `blend_<walk>_to_idle` clip, the
    /// walk carries on until it reaches the frame the blend starts from
    /// (or the matching frame of the other step), then the blend plays.
    pub fn stand(&mut self) {
        if self.stopping.is_some() {
            return;
        }
        if self.base != self.idle_base
            && self.cfg.walk_blends
            && self.overlay.is_none()
            && footfall_for(&self.base).is_some()
        {
            let (base, idle) = (self.base.clone(), self.idle_base.clone());
            if let Some(b) = self.blend_between(&base, &idle) {
                self.stopping = Some(b);
                self.stop_wait_s = 0.0;
                return;
            }
        }
        self.finish_stand(None);
    }

    /// Switches the base to the seated clip `name` and plays the manifest's
    /// `<base> -> name` blend over it when there is one (and nothing else is
    /// playing). Returns the blend's length in seconds.
    pub fn sit_down(&mut self, name: &str) -> Result<Option<f32>, AnimError> {
        let from = self.base.clone();
        self.set_base(name, false)?;
        if self.overlay.is_some() {
            return Ok(None);
        }
        let Some(b) = self.blend_between(&from, name) else {
            return Ok(None);
        };
        let dur = ClipPlayer::for_clip(&self.manifest.clips[&b]).duration_s();
        self.start_overlay(b, false, OverlayKind::Transition { remaining_s: dur });
        Ok(Some(dur))
    }

    /// [`Self::stand`] from a seated (non-walk) base through its
    /// `<base> -> idle` blend when the manifest has one. Returns the blend's
    /// length in seconds (`None`: a plain [`Self::stand`]).
    pub fn stand_blended(&mut self) -> Option<f32> {
        if self.stopping.is_none()
            && self.base != self.idle_base
            && self.overlay.is_none()
            && footfall_for(&self.base).is_none()
        {
            let (base, idle) = (self.base.clone(), self.idle_base.clone());
            if let Some(b) = self.blend_between(&base, &idle) {
                let dur = ClipPlayer::for_clip(&self.manifest.clips[&b]).duration_s();
                self.finish_stand(Some(b));
                return Some(dur);
            }
        }
        self.stand();
        None
    }

    /// The actual switch to idle, optionally through `blend`.
    fn finish_stand(&mut self, blend: Option<String>) {
        self.stopping = None;
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
            let flip = self.base_flip;
            let idle = self.idle_base.clone();
            let _ = self.set_base(&idle, false);
            if let Some(b) = blend {
                let dur = ClipPlayer::for_clip(&self.manifest.clips[&b]).duration_s();
                self.start_overlay(b, flip, OverlayKind::Transition { remaining_s: dur });
            }
        }
    }

    fn start_overlay(&mut self, clip: String, flip: bool, kind: OverlayKind) {
        let drawn = self.drawn(&clip).to_string();
        let player = once_player(&self.manifest.clips[&drawn]);
        let sign = if flip { -1.0 } else { 1.0 };
        // Mask first, so a baked-jiggle clip's own start kick is dropped on
        // the channels it disables.
        let d = &self.manifest.clips[&drawn];
        let (mask, gait) = (self.clip_mask(d), d.bakes_gait());
        self.apply_clip_mask(mask, gait);
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
            drawn,
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

        // Clip frame timing runs at `playback_speed`; everything else
        // (physics, breath, idle timers, transition lengths) in real time.
        let clip_dt = dt * clamp_speed(self.cfg.playback_speed);
        let before = self.base_player.frame();
        self.base_player.advance(clip_dt);
        // Variants: at a loop end of the base once the hold is over, a new
        // random take; play-once mode returns to the base after one pass.
        if !self.variants(&self.base).is_empty() {
            let mut t = self.take(&self.base);
            t.age += dt;
            self.takes.insert(self.base.clone(), t);
            let wrapped = self.base_player.frame() < before;
            if self.play_once() {
                if t.pick.is_some() {
                    if self.base_player.finished() {
                        self.end_variant();
                    }
                } else if wrapped && self.variant_due() {
                    self.next_variant();
                }
            } else if wrapped && self.variant_due() {
                self.next_variant();
            }
        }
        if let Some(bl) = &mut self.blink {
            bl.elapsed += dt;
            if bl.elapsed >= *bl.ends.last().unwrap_or(&0.0) {
                self.blink = None;
            }
        }
        let mut done = false;
        if let Some(o) = &mut self.overlay {
            o.player.advance(clip_dt);
            done = match &mut o.kind {
                OverlayKind::Reaction { .. } => o.player.finished(),
                OverlayKind::Transition { remaining_s } => {
                    *remaining_s -= dt;
                    *remaining_s <= 0.0
                }
            };
        }
        if done {
            let ended = self.overlay.take().map(|o| o.clip);
            // An idle→walk blend hands over on the walk frame it leads into.
            if let (Some(c), Some((walk, f))) = (ended, self.entry.clone()) {
                let leads_in = self.manifest.clips[&c]
                    .blend
                    .as_ref()
                    .is_some_and(|b| b.to == walk);
                if leads_in && self.base == walk {
                    self.entry = None;
                    self.base_player = self.base_player_for_drawn();
                    self.base_player.seek(f);
                }
            }
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
            let sign = if flip { -1.0 } else { 1.0 };
            // Root motion: the frames stepped through on the same clip
            // (normally one; more after a long tick).
            if let (Some((prev, pf)), Some(rm)) = (&self.last_key, self.root_motion_of(&name)) {
                if *prev == name && !rm.is_empty() {
                    let n = rm.len();
                    let mut k = *pf % n;
                    let mut dx = 0.0;
                    for _ in 0..n {
                        if k == frame {
                            break;
                        }
                        dx += rm[k];
                        k = (k + 1) % n;
                    }
                    self.root_dx += sign * dx;
                }
            }
            // Footfalls: on the art's contact frames when the clip lists
            // them, else twice per cycle (legacy) / cycle-derived (driver).
            if let Some(kick) = footfall_for(&name) {
                if let Some(cf) = self.contact_frames_of(&name) {
                    if cf.contains(&frame) {
                        let foot = self.foot;
                        self.foot += 1;
                        match &mut self.driver {
                            Some(d) => d.on_footfall(foot),
                            None => kick.apply(&mut self.physics, sign),
                        }
                    }
                } else if self.driver.is_none() {
                    let n = self.manifest.clips[&name].frames.len();
                    let half = (n / 2).max(1);
                    if frame % half == 0 {
                        kick.apply(&mut self.physics, sign);
                    }
                }
            }
            // A pending stop: hand over to the walk→idle blend on the frame
            // it was cut from (or the same pose on the other foot).
            if let Some(b) = self.stopping.clone() {
                let from = self.manifest.clips[&b]
                    .blend
                    .as_ref()
                    .map_or(0, |bl| bl.from_frame as usize);
                let walking = self.drawn(&self.base) == name;
                let n = self.manifest.clips[&name].frames.len().max(1);
                if walking && (frame == from % n || frame == (from + n / 2) % n) {
                    self.finish_stand(Some(b));
                }
            }
            self.last_key = Some(key);
        }
        if self.stopping.is_some() {
            // Never wait more than one cycle (or behind a reaction).
            self.stop_wait_s += dt;
            let cycle = self.manifest.clips[&self.base].duration_s();
            if self.overlay.is_some() || self.stop_wait_s > cycle + 0.05 {
                self.finish_stand(None);
            }
        }

        self.sync_spring_mask();
        match &mut self.driver {
            Some(d) => d.tick(dt),
            None => self.physics.step(dt),
        }
    }

    /// The frame to draw now, with the spring transform.
    pub fn current(&self) -> FrameRef<'_> {
        if let Some(o) = &self.overlay {
            let name = o.drawn.as_str();
            let clip = &self.manifest.clips[name];
            let transform = self.transform_for(clip);
            let frame = o.player.frame().min(clip.frames.len() - 1);
            return FrameRef {
                clip_name: name,
                clip,
                frame,
                rect: clip.frames[frame],
                flip: o.flip,
                layer: match o.kind {
                    OverlayKind::Reaction { .. } => Layer::Reaction,
                    OverlayKind::Transition { .. } => Layer::Transition,
                },
                transform,
                mouth: None,
                blend: self.blend_for(&o.player, clip.frames.len()),
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
                mouth: None,
                blend: None,
            };
        }
        let name = self.drawn(self.drawn_base(&self.base));
        let clip = &self.manifest.clips[name];
        let frame = self
            .pinned_base_frame
            .unwrap_or_else(|| self.base_player.frame())
            .min(clip.frames.len() - 1);
        let transform = self.transform_for(clip);
        let mouth = self.mouth.and_then(|m| {
            let clean = clip
                .jiggle_of
                .as_deref()
                .and_then(|c| self.manifest.clips.get(c))
                .or_else(|| self.manifest.clips.get(&self.base));
            mouth_draw(clip, clean, frame, m)
        });
        let blend = if self.pinned_base_frame.is_some() {
            None
        } else {
            self.blend_for(&self.base_player, clip.frames.len())
        };
        // A mouth patch follows the cross-fade (point and tilt in between).
        let mouth = match (mouth, blend) {
            (Some(mut m), Some((next, w))) if w > 0.0 => {
                if let Some(n) = self.mouth.and_then(|s| {
                    let clean = clip
                        .jiggle_of
                        .as_deref()
                        .and_then(|c| self.manifest.clips.get(c));
                    mouth_draw(clip, clean, next, s)
                }) {
                    m.point.0 += (n.point.0 - m.point.0) * w;
                    m.point.1 += (n.point.1 - m.point.1) * w;
                    m.angle_deg += (n.angle_deg - m.angle_deg) * w;
                }
                Some(m)
            }
            (m, _) => m,
        };
        FrameRef {
            clip_name: name,
            clip,
            frame,
            rect: clip.frames[frame],
            flip: self.base_flip,
            layer: Layer::Base,
            transform,
            mouth,
            blend,
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

    #[test]
    fn playback_speed_stretches_clip_timing() {
        // idle: 4 frames at 10 fps.
        let mut normal = Animator::new(manifest(), quiet()).unwrap();
        let mut slow = Animator::new(
            manifest(),
            AnimatorConfig {
                playback_speed: 0.5,
                ..quiet()
            },
        )
        .unwrap();
        for _ in 0..3 {
            normal.tick(0.05);
            slow.tick(0.05);
        }
        assert_eq!(normal.base_frame(), 1);
        assert_eq!(
            slow.base_frame(),
            0,
            "half speed: still on frame 0 after 0.15 s"
        );
        for _ in 0..2 {
            slow.tick(0.05);
        }
        assert_eq!(slow.base_frame(), 1);
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
        // physics.toml walk_footfall.root_y = 0 (physics-lab retune): the
        // baked walk frames / engine [gait] hip bob own the vertical step,
        // a root_y kick on top would double it. Runs still kick root_y.
        assert!(max_y < 0.05, "walk root y {max_y}");
        a.walk(Dir::Right, true).unwrap();
        let mut max_y = 0f32;
        for _ in 0..120 {
            a.tick(1.0 / 60.0);
            max_y = max_y.max(a.current().transform.y);
        }
        assert!(max_y > 0.1, "run root y {max_y}");
    }

    #[test]
    fn idle_life_drives_blink_and_transitions() {
        // Scheduler mechanics with the old busy timings (40 s window).
        let cfg = AnimatorConfig {
            idle: lillith_core::idle::IdleConfig::lively(),
            ..AnimatorConfig::default()
        };
        let mut a = Animator::new(manifest(), cfg).unwrap();
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
        // physics.toml breathing.scale_amplitude 0.003 (physics-lab retune).
        assert!(hi > 1.0015 && lo < 0.9985, "{lo}..{hi}");
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
        // Legacy mode (variant_play_once = false): one variant loops.
        let mut cfg = quiet();
        cfg.idle.variant_hold_s = 0.0;
        cfg.idle.variant_play_once = false;
        let mut a = Animator::new(variant_manifest(), cfg).unwrap();
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
    fn legacy_idle_variants_are_held_for_the_hold() {
        let mut cfg = quiet();
        cfg.idle.variant_play_once = false;
        cfg.idle.variant_hold_s = 30.0;
        let mut a = Animator::new(variant_manifest(), cfg).unwrap();
        assert_eq!(a.cfg.idle.variant_hold_s, 30.0);
        let mut last = a.current().clip_name.to_string();
        let mut switches = Vec::new();
        for i in 0..(70 * 60) {
            a.tick(1.0 / 60.0);
            let c = a.current().clip_name.to_string();
            if c != last {
                switches.push(i as f32 / 60.0);
                last = c;
            }
        }
        // 0.4 s loops, 30 s hold: two switches in 70 s, not ~175.
        assert_eq!(switches.len(), 2, "{switches:?}");
        assert!(switches[0] >= 29.9 && switches[0] <= 30.5, "{switches:?}");
    }

    /// Clips `(name, frames, tags)`, all looping at 10 fps.
    fn tagged_manifest(clips: &[(&str, usize, &str)]) -> Manifest {
        let mut s = String::new();
        for (name, n, tags) in clips {
            s += &format!("[clips.{name}]\npage = \"{name}.png\"\nfps = 10.0\nmode = \"loop\"\ntags = {tags}\nframes = [");
            for i in 0..*n {
                s += &format!(
                    "{{ x = {}, y = 0, w = 400, h = 400, anchor = [200, 392] }},",
                    i * 400
                );
            }
            s += "]\n";
        }
        Manifest::from_toml_str(&s).unwrap()
    }

    const BAKED: &str = "[\"baked_motion\"]";

    fn variant_set() -> Manifest {
        tagged_manifest(&[
            ("idle", 4, BAKED),
            ("idle_yawn", 6, "[\"baked_motion\", \"variant_of:idle\"]"),
            ("idle_nails", 6, "[\"baked_motion\", \"variant_of:idle\"]"),
            ("walk_right", 8, "[]"),
            (
                "walk_sassy_right",
                7,
                "[\"baked_motion\", \"variant_of:walk_right\"]",
            ),
            (
                "walk_skip_right",
                5,
                "[\"baked_motion\", \"variant_of:walk_right\"]",
            ),
            ("walk_left", 8, "[]"),
            (
                "walk_skip_left",
                5,
                "[\"baked_motion\", \"variant_of:walk_left\"]",
            ),
            ("sit_swing", 4, "[]"),
            (
                "sit_heel_kick",
                6,
                "[\"baked_motion\", \"variant_of:sit_swing\"]",
            ),
            // Base missing from the set: ignored.
            ("orphan", 3, "[\"variant_of:nope\"]"),
        ])
    }

    #[test]
    fn variant_map_resolves_variant_of_tags() {
        let m = variant_set();
        let map = variant_map(&m);
        let keys: Vec<&str> = map.keys().map(String::as_str).collect();
        assert_eq!(keys, ["idle", "sit_swing", "walk_left", "walk_right"]);
        assert_eq!(map["idle"], ["idle_nails", "idle_yawn"]);
        assert_eq!(map["walk_right"], ["walk_sassy_right", "walk_skip_right"]);
        assert_eq!(map["walk_left"], ["walk_skip_left"]);
        assert_eq!(map["sit_swing"], ["sit_heel_kick"]);
        let a = Animator::new(m, quiet()).unwrap();
        assert_eq!(a.variants("sit_swing"), ["sit_heel_kick"]);
        assert!(a.variants("orphan").is_empty() && a.variants("nope").is_empty());
        // Legacy mode cycles the idle base only.
        let mut cfg = quiet();
        cfg.idle.variant_play_once = false;
        let a = Animator::new(variant_set(), cfg).unwrap();
        assert_eq!(a.variants("idle").len(), 2);
        assert!(a.variants("walk_right").is_empty() && a.variants("sit_swing").is_empty());
    }

    /// Ticks until a variant of the base is drawn (max `secs`); returns the
    /// drawn clip and its flip.
    fn until_variant(a: &mut Animator, secs: f32) -> Option<(String, bool)> {
        for _ in 0..(secs * 60.0) as usize {
            a.tick(1.0 / 60.0);
            if a.base_variant().is_some() {
                let f = a.current();
                return Some((f.clip_name.to_string(), f.flip));
            }
        }
        None
    }

    #[test]
    fn mirrored_walk_draws_the_drawn_clips_variants_flipped() {
        let mut cfg = quiet();
        cfg.idle.variant_hold_s = 0.0;
        // Only walk_right (+ variants): a left walk is walk_right mirrored,
        // and so are its variants.
        let m = tagged_manifest(&[
            ("idle", 4, "[]"),
            ("walk_right", 8, "[]"),
            ("walk_sassy_right", 7, "[\"variant_of:walk_right\"]"),
        ]);
        let mut a = Animator::new(m, cfg.clone()).unwrap();
        assert_eq!(
            a.locomotion_clip(Dir::Left, false),
            Some(("walk_right".into(), true))
        );
        a.walk(Dir::Left, false).unwrap();
        // Entering the walk starts on the clean clip.
        assert_eq!(
            (a.current().clip_name, a.current().flip),
            ("walk_right", true)
        );
        assert_eq!(
            until_variant(&mut a, 2.0),
            Some(("walk_sassy_right".into(), true))
        );
        // Dedicated walk_left: its own variants, unflipped.
        let mut a = Animator::new(variant_set(), cfg).unwrap();
        a.walk(Dir::Left, false).unwrap();
        assert_eq!(
            until_variant(&mut a, 2.0),
            Some(("walk_skip_left".into(), false))
        );
        a.walk(Dir::Right, false).unwrap();
        assert_eq!(a.current().clip_name, "walk_right", "new base: clean clip");
        let (c, flip) = until_variant(&mut a, 2.0).unwrap();
        assert!(c == "walk_sassy_right" || c == "walk_skip_right", "{c}");
        assert!(!flip);
        a.stand();
        assert_eq!(a.current().clip_name, "idle");
    }

    #[test]
    fn play_once_short_variant_returns_to_base() {
        let mut cfg = quiet();
        cfg.idle.variant_hold_s = 2.0;
        assert!(cfg.idle.variant_play_once, "default");
        for base in ["idle", "sit_swing"] {
            let mut a = Animator::new(variant_set(), cfg.clone()).unwrap();
            a.set_base(base, false).unwrap();
            // (clip, first tick, last tick) runs of what the base draws.
            let mut runs: Vec<(String, usize, usize)> = Vec::new();
            for i in 0..(12 * 60) {
                a.tick(1.0 / 60.0);
                let f = a.current();
                assert_eq!(f.layer, Layer::Base);
                match runs.last_mut() {
                    Some(r) if r.0 == f.clip_name => r.2 = i,
                    _ => runs.push((f.clip_name.to_string(), i, i)),
                }
            }
            let variants = a.variants(base).to_vec();
            assert!(runs.len() >= 5, "{base}: {runs:?}");
            for (k, (clip, start, end)) in runs.iter().enumerate() {
                let secs = (end - start + 1) as f32 / 60.0;
                if variants.contains(clip) {
                    // A 6-frame (0.6 s) variant, far shorter than the 2 s
                    // hold, plays exactly once and hands back to the base.
                    assert!((secs - 0.6).abs() < 0.05, "{base}: {clip} {secs}");
                    assert_eq!(runs.get(k + 1).map(|r| r.0.as_str()).unwrap_or(base), base);
                } else {
                    assert_eq!(clip, base);
                    if k + 1 < runs.len() {
                        // The base loops for the hold before the next one.
                        assert!(secs >= 2.0 - 0.02, "{base}: base held {secs}");
                    }
                }
            }
        }
    }

    #[test]
    fn play_once_two_variants_never_repeat_back_to_back() {
        let mut cfg = quiet();
        cfg.idle.variant_hold_s = 0.0;
        let mut a = Animator::new(variant_set(), cfg).unwrap();
        let mut played = Vec::new();
        let mut last = None;
        for _ in 0..(20 * 60) {
            a.tick(1.0 / 60.0);
            let v = a.idle_variant().map(str::to_string);
            if v != last {
                if let Some(v) = &v {
                    played.push(v.clone());
                }
                last = v;
            }
        }
        assert!(played.len() > 10, "{played:?}");
        assert!(played.windows(2).all(|w| w[0] != w[1]), "{played:?}");
    }

    #[test]
    fn walks_prefer_sultry_takes_and_add_strut() {
        let m = variant_set();
        let mut map = variant_map(&m);
        prefer_sultry_walks(&m, &mut map);
        // variant_set(): walk_right has sassy + skip; skip is dropped.
        assert_eq!(map["walk_right"], ["walk_sassy_right"]);
        // walk_left only had skip (no sultry take): left as is.
        assert_eq!(map["walk_left"], ["walk_skip_left"]);
    }

    #[test]
    fn play_once_default_draws_clean_idle_until_the_hold() {
        let mut cfg = quiet();
        cfg.idle.variant_hold_s = 30.0;
        let mut a = Animator::new(variant_manifest(), cfg).unwrap();
        assert!(a.cfg.idle.variant_play_once);
        assert_eq!(a.cfg.idle.variant_hold_s, 30.0);
        assert_eq!(a.current().clip_name, "idle");
        let mut first = None;
        for i in 0..(35 * 60) {
            a.tick(1.0 / 60.0);
            if first.is_none() && a.idle_variant().is_some() {
                first = Some(i as f32 / 60.0);
            }
        }
        let t = first.expect("a variant after the hold");
        assert!((29.9..=30.5).contains(&t), "{t}");
        // Baked-motion masking is untouched while variants play.
        assert!(a.spring_mask().contains(Channel::Chest));
    }

    #[test]
    fn baked_motion_clips_switch_off_engine_jiggle() {
        // variant_manifest(): idle (+ variants) tagged baked_motion, wave not.
        let mut a = Animator::new(variant_manifest(), quiet()).unwrap();
        let m = a.spring_mask();
        assert!(m.contains(Channel::Chest) && m.contains(Channel::Pigtail));
        assert!(m.contains(Channel::RootX));
        a.physics_mut().impulse(Channel::Chest, 80.0);
        a.physics_mut().impulse(Channel::Pigtail, 80.0);
        a.physics_mut().impulse(Channel::RootX, 80.0);
        for _ in 0..30 {
            a.tick(1.0 / 60.0);
            let t = a.current().transform;
            assert_eq!((t.chest_offset, t.pigtail_offset, t.x), (0.0, 0.0, 0.0));
        }
        // An engine-clean clip gets its springs back.
        a.set_base("wave", false).unwrap();
        assert!(a.spring_mask().is_empty());
        // Opt out: only the gain applies (scaffold's damped behaviour).
        let cfg = AnimatorConfig {
            baked_motion_springs_off: false,
            ..quiet()
        };
        let a = Animator::new(variant_manifest(), cfg).unwrap();
        assert!(a.spring_mask().is_empty());
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

    /// idle + knife_tap with baked-jiggle siblings (6 frames @ 10 fps).
    fn jiggle_manifest() -> Manifest {
        let mut s = String::new();
        for (name, extra) in [
            ("idle", "mode = \"loop\"\njiggle_variant = \"idle_jiggle\"\n"),
            (
                "idle_jiggle",
                "mode = \"loop\"\njiggle_baked = true\njiggle_of = \"idle\"\ndisable_springs = [\"chest\", \"pigtails\", \"hip\"]\n",
            ),
            ("knife_tap", "mode = \"once\"\njiggle_variant = \"knife_tap_jiggle\"\n"),
            (
                "knife_tap_jiggle",
                "mode = \"once\"\njiggle_baked = true\njiggle_of = \"knife_tap\"\ndisable_springs = [\"chest\", \"pigtails\", \"hip\"]\n",
            ),
            ("wave", "mode = \"once\"\n"),
        ] {
            s += &format!("[clips.{name}]\npage = \"{name}.png\"\nfps = 10.0\n{extra}frames = [");
            for i in 0..6 {
                s += &format!(
                    "{{ x = {}, y = 0, w = 400, h = 400, anchor = [200, 392] }},",
                    i * 400
                );
            }
            s += "]\n";
        }
        Manifest::from_toml_str(&s).unwrap()
    }

    /// Largest |chest|, |pigtail|, |root x| and |root y| offsets over `secs`.
    fn peaks(a: &mut Animator, secs: f32) -> (f32, f32, f32, f32) {
        let mut m = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for _ in 0..(secs * 60.0).round() as usize {
            a.tick(1.0 / 60.0);
            let t = a.current().transform;
            m = (
                m.0.max(t.chest_offset.abs()),
                m.1.max(t.pigtail_offset.abs()),
                m.2.max(t.x.abs()),
                m.3.max(t.y.abs()),
            );
        }
        m
    }

    fn kick_all(a: &mut Animator) {
        for c in [
            Channel::Chest,
            Channel::Pigtail,
            Channel::RootX,
            Channel::RootY,
        ] {
            a.physics_mut().impulse(c, 80.0);
        }
    }

    #[test]
    fn jiggle_clip_disables_chest_pigtail_hip_springs_and_normal_clip_does_not() {
        for driver in [false, true] {
            let cfg = AnimatorConfig {
                physics_driver: driver,
                ..quiet()
            };
            let mut a = Animator::new(jiggle_manifest(), cfg).unwrap();
            // Normal clip: full springs.
            a.react("wave", 1).unwrap();
            assert!(a.spring_mask().is_empty());
            kick_all(&mut a);
            let (c, p, x, _) = peaks(&mut a, 0.3);
            assert!(
                c > 0.3 && p > 0.3 && x > 0.3,
                "normal clip {driver}: {c} {p} {x}"
            );
            run(&mut a, 3.0);

            // Baked jiggle clip: chest/pigtails/hip (root x) parked at rest,
            // kicks dropped; root y (footfalls/hops) still live.
            a.react("knife_tap_jiggle", 2).unwrap();
            assert_eq!(a.current().clip_name, "knife_tap_jiggle");
            let m = a.spring_mask();
            for ch in [Channel::Chest, Channel::Pigtail, Channel::RootX] {
                assert!(m.contains(ch), "{ch:?}");
            }
            assert!(!m.contains(Channel::RootY) && !m.contains(Channel::Angle));
            kick_all(&mut a);
            let (c, p, x, y) = peaks(&mut a, 0.5);
            assert_eq!((c, p, x), (0.0, 0.0, 0.0), "double bounce {driver}");
            assert!(y > 0.1, "root_y stays live {driver}: {y}");

            // Restored once a clean clip is drawn again (the base).
            run(&mut a, 1.0);
            assert_eq!(a.current().clip_name, "idle");
            assert!(a.spring_mask().is_empty());
            kick_all(&mut a);
            let (c, p, x, _) = peaks(&mut a, 0.3);
            assert!(
                c > 0.3 && p > 0.3 && x > 0.3,
                "restored {driver}: {c} {p} {x}"
            );
        }
    }

    /// idle + a baked walk_right (`disable_springs = ["gait"]`) + an
    /// engine-clean walk_left (6 frames @ 10 fps).
    fn gait_manifest() -> Manifest {
        let mut s = String::new();
        for (name, extra) in [
            ("idle", ""),
            (
                "walk_right",
                "tags = [\"baked_motion\"]\ndisable_springs = [\"gait\"]\n",
            ),
            ("walk_left", ""),
        ] {
            s += &format!("[clips.{name}]\npage = \"{name}.png\"\nfps = 10.0\n{extra}frames = [");
            for i in 0..6 {
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
    fn engine_gait_is_off_by_default_and_skips_baked_walk_clips() {
        let mut a = Animator::new(gait_manifest(), driven()).unwrap();
        assert!(a.manifest().clips["walk_right"].bakes_gait());
        assert!(!a.manifest().clips["walk_left"].bakes_gait());
        assert!(a.spring_mask().is_empty(), "gait is not a spring channel");
        // Bundled physics.toml: [gait] enabled = false.
        assert!(!a.driver().unwrap().gait_enabled());
        a.walk(Dir::Left, false).unwrap();
        run(&mut a, 2.0);
        assert_eq!(a.physics_overlay().unwrap().gait.gait_amp, 0.0);
        a.stand();
        run(&mut a, 1.0);

        // Ko flips it on.
        a.driver_mut().unwrap().set_gait_enabled(true);
        // Baked walk: suppressed, nothing procedural on top of the video.
        a.walk(Dir::Right, false).unwrap();
        assert_eq!(a.current().clip_name, "walk_right");
        assert!(a.driver().unwrap().gait_suppressed());
        // baked_motion: the engine jiggle is off too (no double jiggle);
        // drag sway / footfall bob channels stay.
        let m = a.spring_mask();
        assert!(m.contains(Channel::Chest) && m.contains(Channel::Pigtail));
        assert!(m.contains(Channel::RootX));
        assert!(!m.contains(Channel::Angle) && !m.contains(Channel::RootY));
        run(&mut a, 2.0);
        let g = a.physics_overlay().unwrap().gait;
        assert_eq!((g.gait_amp, g.hip_bob_px, g.left_arm_deg), (0.0, 0.0, 0.0));
        // Engine-clean walk: the gait drives it.
        a.walk(Dir::Left, false).unwrap();
        assert_eq!(a.current().clip_name, "walk_left");
        let d = a.driver().unwrap();
        assert!(!d.gait_suppressed() && d.gait_active());
        run(&mut a, 2.0);
        let g = a.physics_overlay().unwrap().gait;
        assert!(g.gait_amp > 0.99, "stride {}", g.gait_amp);
        assert!(g.left_arm_deg.abs() + g.right_arm_deg.abs() > 1.0);
        // Back to idle: not a baked clip, gait ramps out on its own.
        a.stand();
        assert!(!a.driver().unwrap().gait_suppressed());
    }

    #[test]
    fn prefer_jiggle_variants_draws_baked_sibling_with_springs_off() {
        let mut a = Animator::new(jiggle_manifest(), quiet()).unwrap();
        assert_eq!(a.current().clip_name, "idle");
        a.set_prefer_jiggle_variants(true);
        assert_eq!(a.current().clip_name, "idle_jiggle");
        assert!(a.spring_mask().contains(Channel::Chest));
        a.react("knife_tap", 1).unwrap();
        assert_eq!(a.current().clip_name, "knife_tap_jiggle");
        // No sibling: drawn as is, springs on.
        run(&mut a, 1.0);
        a.react("wave", 1).unwrap();
        assert_eq!(a.current().clip_name, "wave");
        a.tick(1.0 / 60.0);
        assert!(a.spring_mask().is_empty());
        a.set_prefer_jiggle_variants(false);
        run(&mut a, 1.0);
        assert_eq!(a.current().clip_name, "idle");
        assert!(a.spring_mask().is_empty());
    }

    #[test]
    fn no_jiggle_clips_stay_clean_and_jiggle_base_plays_its_own_frames() {
        let mut s = String::new();
        for (name, n, extra) in [
            ("idle", 2, ""),
            ("talk_loop", 4, "jiggle_variant = \"talk_loop_jiggle\"\n"),
            (
                "talk_loop_jiggle",
                6,
                "jiggle_baked = true\njiggle_of = \"talk_loop\"\ndisable_springs = [\"chest\", \"pigtails\", \"hip\"]\n",
            ),
        ] {
            s += &format!("[clips.{name}]\npage = \"{name}.png\"\nfps = 10.0\nmode = \"loop\"\n{extra}frames = [");
            for i in 0..n {
                s += &format!("{{ x = {}, y = 0, w = 40, h = 40, anchor = [20, 38] }},", i * 40);
            }
            s += "]\n";
        }
        let m = Manifest::from_toml_str(&s).unwrap();
        let cfg = AnimatorConfig {
            prefer_jiggle_variants: true,
            ..quiet()
        };
        let mut a = Animator::new(m.clone(), cfg.clone()).unwrap();
        a.set_base("talk_loop", false).unwrap();
        assert_eq!(a.current().clip_name, "talk_loop_jiggle");
        assert!(a.spring_mask().contains(Channel::Chest));
        // The jiggle sibling has 6 frames: all of them play.
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..120 {
            a.tick(1.0 / 60.0);
            seen.insert(a.current().frame);
        }
        assert_eq!(seen.len(), 6, "{seen:?}");
        // `[chat] talk_jiggle = false`: talk clips stay engine-clean.
        let mut b = Animator::new(
            m,
            AnimatorConfig {
                no_jiggle_clips: vec!["talk_loop".into()],
                ..cfg
            },
        )
        .unwrap();
        b.set_base("talk_loop", false).unwrap();
        assert_eq!(b.current().clip_name, "talk_loop");
        assert!(b.spring_mask().is_empty());
        assert_eq!(b.drawn_clip("talk_loop"), "talk_loop");
    }

    #[test]
    fn mouth_patch_follows_frame_point_and_tilt() {
        let toml = r#"
            [clips.idle]
            page = "idle.png"
            fps = 10.0
            frames = [ { x = 0, y = 0, w = 40, h = 40, anchor = [20, 38] } ]
            [clips.talk_loop]
            page = "t.png"
            fps = 10.0
            mouth_angle_deg = [0.0, 4.5]
            frames = [ { x = 0, y = 0, w = 40, h = 40, anchor = [20, 38] }, { x = 40, y = 0, w = 40, h = 40, anchor = [20, 38] } ]
            [clips.talk_loop.points]
            mouth_point = [20, 10]
            [clips.talk_loop.frame_points]
            mouth_point = [[20, 10], [22, 11]]
            [clips.talk_loop.mouth_patches.open]
            page = "m/open.png"
            centre = [3.0, 2.0]
        "#;
        let mut a = Animator::new(Manifest::from_toml_str(toml).unwrap(), quiet()).unwrap();
        a.set_base("talk_loop", false).unwrap();
        a.set_mouth(Some(crate::talk::Mouth::Open));
        a.pin_base_frame(Some(1));
        let m = a.current().mouth.expect("patch drawn");
        assert_eq!(m.patch.page, "m/open.png");
        assert_eq!((m.point, m.angle_deg), ((22.0, 11.0), 4.5));
        // No patch for this shape: nothing pasted.
        a.set_mouth(Some(crate::talk::Mouth::Smile));
        assert!(a.current().mouth.is_none());
        // Changing the base clears the mouth.
        a.set_mouth(Some(crate::talk::Mouth::Open));
        a.set_base("idle", false).unwrap();
        assert_eq!(a.mouth(), None);
        assert!(a.current().mouth.is_none());
    }

    #[test]
    fn once_jiggle_with_settle_subframes_plays_its_own_frames_and_timing() {
        // knife_tap: 3 frames x 200 ms; its jiggle sibling splits the last
        // hold into 3 sub-frames (5 frames, same 600 ms in total).
        let toml = r#"
            [clips.idle]
            page = "i.png"
            fps = 10.0
            frames = [ { x = 0, y = 0, w = 40, h = 40, anchor = [20, 38] } ]
            [clips.knife_tap]
            page = "k.png"
            mode = "once"
            fps = 5.0
            jiggle_variant = "knife_tap_jiggle"
            frame_durations_ms = [200, 200, 200]
            frames = [ { x = 0, y = 0, w = 40, h = 40, anchor = [20, 38] }, { x = 40, y = 0, w = 40, h = 40, anchor = [20, 38] }, { x = 80, y = 0, w = 40, h = 40, anchor = [20, 38] } ]
            [clips.knife_tap_jiggle]
            page = "kj.png"
            mode = "once"
            fps = 5.0
            jiggle_baked = true
            jiggle_of = "knife_tap"
            disable_springs = ["chest"]
            clean_frame_index = [0, 1, 2, 2, 2]
            frame_durations_ms = [200, 200, 66, 67, 67]
            frames = [ { x = 0, y = 0, w = 40, h = 40, anchor = [20, 38] }, { x = 40, y = 0, w = 40, h = 40, anchor = [20, 38] }, { x = 80, y = 0, w = 40, h = 40, anchor = [20, 38] }, { x = 120, y = 0, w = 40, h = 40, anchor = [20, 38] }, { x = 160, y = 0, w = 40, h = 40, anchor = [20, 38] } ]
        "#;
        let m = Manifest::from_toml_str(toml).unwrap();
        let mut a = Animator::new(
            m,
            AnimatorConfig {
                prefer_jiggle_variants: true,
                playback_speed: 1.0,
                ..quiet()
            },
        )
        .unwrap();
        a.react("knife_tap", 1).unwrap();
        let mut seen = Vec::new();
        let mut t = 0.0f32;
        while a.overlay_clip().is_some() && t < 2.0 {
            let f = a.current();
            assert_eq!(f.clip_name, "knife_tap_jiggle");
            if seen.last().map(|s: &(usize, usize)| s.0) != Some(f.frame) {
                seen.push((f.frame, f.clean_frame()));
            }
            a.tick(1.0 / 120.0);
            t += 1.0 / 120.0;
        }
        // Every jiggle frame shown, each mapped to its clean frame.
        assert_eq!(seen, vec![(0, 0), (1, 1), (2, 2), (3, 2), (4, 2)]);
        // Same length as the clean clip (600 ms), not cut at 3 frames.
        assert!((t - 0.6).abs() < 0.03, "reaction lasted {t}s");
        assert_eq!(a.current().clip_name, "idle");
    }
}
