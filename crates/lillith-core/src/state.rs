//! The interaction & behaviour state machine (PLAN.md §3.1, milestone M3).
//!
//! [`Brain`] owns everything above the animator: the locomotion phase
//! (idle / walking / dragged / asleep), the [`MoveMode`] (Wander or Stay),
//! sleep/wake timers, moods, the mischief meter, pointer gestures (with the
//! secret sequence), README-SP keys, the 16 s pose cycle and the event →
//! reaction rules (priority + cooldown + line + SFX + mood). It is pure: the
//! overlay feeds it [`Input`]s, [`Event`]s and `tick(dt, busy)`, and applies
//! the [`Action`]s it returns. Time is the sum of the `dt`s it was given, so
//! unit tests drive it with a fake clock.
//!
//! What happens, in short:
//!
//! | Input | Reaction (clip via `rules.toml`) |
//! |---|---|
//! | Left click | `knife_tap`; a second left click within `left_repeat_s` waves instead (then alternates). While mischief is high it is always the knife, in Mischievous mood |
//! | Double click | `input.click.double` (hacking), priority 90 so it cuts the first click's knife |
//! | Right click | `input.click.right` (cleaning) + [`Action::ContextMenu`] (pose-menu stub). Never quits |
//! | Middle click | `input.click.middle` (help) |
//! | Left → Double → Right ≤ 3 s | `input.secret` (tail_wag, "You found me out~", Happy) instead of the right click |
//! | Hover enter / sustained | `input.hover` / `input.hover_long` (cooldowns from rules) |
//! | Drag | base `drag_held` while held, back to idle on drop |
//! | Keys | README-SP `[keys]` table; `G` toggles Wander/Stay, `Q`/`Esc` quits, `Y`/`Enter` asks for chat |
//!
//! Inactivity (no input on her, no `user.activity` event) makes her drowsy
//! after `sleepy_after_s` (`idle.long`, mood Sleepy) and puts her to sleep
//! after `asleep_after_s` (base clip `sleep`). Any input on her, hovering
//! her (proximity) or `user.returned` wakes her with the `wake` clip. While
//! asleep she sleeps through events below `event_wake_priority`.

use crate::bag::ShuffleBag;
use crate::config::MoveMode;
use crate::event::Event;
use crate::interaction::{Button, Gesture, GestureConfig, Gestures};
use crate::mood::{Mischief, MischiefConfig, Mood, MoodState};
use crate::rng::Rng;
use crate::rules::{KeyBinding, RuleSet};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Behaviour tunables (`[behavior]` in config.toml). Defaults follow
/// README-SP / rules.toml thresholds (checked by a test).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BehaviorConfig {
    pub gestures: GestureConfig,
    pub mischief: MischiefConfig,
    /// Mood strength e-folding time (s).
    pub mood_tau_s: f32,
    /// No interaction for this long → drowsy (`idle.long`).
    pub sleepy_after_s: f32,
    /// No interaction for this long → asleep (`idle.asleep`).
    pub asleep_after_s: f32,
    /// Automatic pose cycle while idle (s). Calm default 45 s (README-SP's
    /// `cycle_interval` was 16 s).
    pub pose_cycle_s: f32,
    /// A left click this soon after the knife waves instead.
    pub left_repeat_s: f32,
    /// Pause between wander walks, `[min, max]` s.
    pub wander_every_s: (f32, f32),
    /// Glide speed for walks whose clip has no `root_motion`. Walk clips
    /// with root motion set their own pace (see `walk_root_motion`).
    pub wander_speed_px_s: f32,
    /// Move the window by the walk clip's measured root motion (the overlay
    /// feeds [`Brain::root_motion`]) so the planted foot stays put on the
    /// desktop, instead of gliding at `wander_speed_px_s`.
    pub walk_root_motion: bool,
    /// Shortest walk worth taking (px).
    pub wander_min_step_px: f32,
    /// Chance (0..=1) that she strikes an idle pose shortly after a wander
    /// walk ends (`wander_pose_delay_s` later) instead of waiting for the
    /// next pose cycle: more variety while she roams.
    pub wander_pose_chance: f32,
    /// Delay (s) from arriving to that pose.
    pub wander_pose_delay_s: f32,
    /// Events at or above this priority wake her up.
    pub event_wake_priority: u8,
    /// Idle life: breathing, blinks, idle transitions/gestures, idle-take
    /// switching (`[behavior.idle]`).
    pub idle: crate::idle::IdleConfig,
    /// Window-bottom margin (px, like `[overlay] margin_bottom`) of the
    /// floor she falls to when the KWin bridge has not reported the usable
    /// area (no bridge, or before its first snapshot).
    pub floor_offset_px: i32,
    /// Falling after a drop and perching on windows (`[behavior.gravity]`).
    pub gravity: GravityConfig,
    /// "Come closer" tease every few minutes of idle (`[behavior.tease]`).
    pub tease: crate::tease::TeaseConfig,
}

/// `[behavior.gravity]`: after a drag-release she falls (holding the
/// `drag_held` pose), bounces softly and lands on the floor or, with the
/// KWin bridge, on the top edge of the window under her.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GravityConfig {
    /// Gravity (px/s², +down). `0` turns falling off: she stays where dropped.
    pub g_px_s2: f32,
    /// Terminal fall speed (px/s).
    pub max_fall_speed: f32,
    /// Landing bounce: fraction of the impact speed she rebounds with
    /// (`0` = no bounce, clamped to 0..0.9). Bounces slower than ~60 px/s
    /// settle instead.
    pub bounce: f32,
    /// Land on window tops (needs the `kwin/lillith-bridge` script); off =
    /// always the floor.
    pub perch_enabled: bool,
    /// Pause between sits while perched, `[min, max]` s.
    pub sit_every_s: (f32, f32),
    /// How long a sit on a window edge lasts, `[min, max]` s.
    pub sit_for_s: (f32, f32),
}

impl Default for GravityConfig {
    fn default() -> Self {
        Self {
            g_px_s2: 2600.0,
            max_fall_speed: 2200.0,
            bounce: 0.22,
            perch_enabled: true,
            sit_every_s: (8.0, 20.0),
            sit_for_s: (20.0, 45.0),
        }
    }
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            gestures: GestureConfig::default(),
            mischief: MischiefConfig::default(),
            mood_tau_s: 45.0,
            sleepy_after_s: 300.0,
            asleep_after_s: 900.0,
            pose_cycle_s: 45.0,
            left_repeat_s: 4.0,
            wander_every_s: (20.0, 45.0),
            wander_speed_px_s: 45.0,
            walk_root_motion: true,
            wander_min_step_px: 120.0,
            wander_pose_chance: 0.35,
            wander_pose_delay_s: 1.5,
            event_wake_priority: 80,
            idle: crate::idle::IdleConfig::default(),
            floor_offset_px: 48,
            gravity: GravityConfig::default(),
            tease: crate::tease::TeaseConfig::default(),
        }
    }
}

/// What the overlay tells the brain.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// A click on her (press without drag; right/middle on press).
    Click(Button),
    /// The pointer entered (`true`) or left her silhouette.
    Hover(bool),
    /// A left press turned into a drag.
    DragStart,
    /// She was dropped at this left margin / x (px).
    DragEnd { left: i32 },
    /// A key while she has keyboard focus.
    Key(Key),
}

/// Keys the overlay forwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A printable key (case-insensitive).
    Char(char),
    Escape,
    Enter,
}

/// What the brain wants the overlay to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Play `clip` once as a reaction at `priority`.
    React {
        clip: String,
        priority: u8,
    },
    /// Switch the looping base clip (`sleep`, `drag_held`, …).
    SetBase {
        clip: String,
    },
    /// Back to the idle base (ends a walk, sleep or drag pose).
    Stand,
    /// Start the walk cycle facing right (`true`) or left.
    Walk {
        right: bool,
    },
    /// Move horizontally to this left margin / x (px); bottom unchanged.
    MoveTo {
        left: i32,
    },
    /// Speech bubble line (bubbles render in a later milestone; logged now).
    Say(String),
    /// README_SOUNDS cue (never emitted while muted).
    Sfx(String),
    MoodChanged(Mood),
    /// Right click: open the pose/context menu (stub until the M6 menu).
    ContextMenu,
    /// `Y` / `Enter`: open the chat panel (M7).
    OpenChat,
    Muted(bool),
    ModeChanged(MoveMode),
    Quit,
}

/// Locomotion phase.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    Idle,
    Walking { target: f32, right: bool },
    Dragged,
    Asleep,
}

/// Idle variants for the pose cycle, per mood. Clips missing from the
/// loaded set are skipped, so older sets keep their original cycle.
///
/// The jiggle/flirty batch (2026-10) adds the ten `idle_flirt_*` mannerisms
/// (interleaved into Playful, a few in Sleepy/Alert) and the `allure` poses
/// (Happy: sweet ones; Mischievous: teasing ones). All are tasteful,
/// non-explicit poses. `strut_left` / `strut_right` are locomotion-like and
/// stay out of the cycle.
fn cycle_variants(mood: Mood) -> &'static [&'static str] {
    match mood {
        Mood::Playful => &[
            "hip_sway",
            "idle_flirt_hip_lean",
            "tail_wag",
            "idle_flirt_pigtail_twirl",
            "lean_forward",
            "idle_flirt_hands_behind_sway",
            "shoulder_roll",
            "idle_flirt_glove_adjust",
            "nod",
            "idle_flirt_tail_curl",
            "wave",
            "idle_flirt_foot_tap",
            "idle_flirt_shy_giggle",
            "idle_flirt_look_away",
            "idle_flirt_lip_bite",
            "idle_flirt_stretch",
        ],
        Mood::Happy => &[
            "hip_sway",
            "heart_hands",
            "tail_wag",
            "blow_kiss",
            "lean_forward",
            "peace_sign_pose",
            "shoulder_roll",
            "twirl",
            "nod",
            "bounce_giggle",
            "wave",
            "victory_jump",
            "sway_dance",
            "hair_twirl",
            "tiptoe_bounce",
            "hands_behind_head_pose",
            "idle_flirt_shy_giggle",
        ],
        Mood::Mischievous => &[
            "tail_wag",
            "hip_pop_wink",
            "knife_tap",
            "finger_wag_tease",
            "hip_sway",
            "cheeky_tongue_wink",
            "dance",
            "tail_tease",
            "lean_tease",
            "over_shoulder_look",
            "shoulder_shimmy",
            "hair_flip",
        ],
        Mood::Alert => &[
            "lean_forward",
            "think",
            "shoulder_roll",
            "idle_flirt_look_away",
        ],
        Mood::Sleepy => &[
            "shoulder_roll",
            "idle_flirt_stretch",
            "lean_forward",
            "stretch_arch",
            "hip_sway",
        ],
    }
}

/// Playful interruptions when she has been ignored.
/// The dominant / seduce clips are used once they're in the manifest.
const PRANKS: &[&str] = &[
    "knife_tap",
    "dance",
    "tail_wag",
    "point",
    "dom_beckon_finger",
    "dom_arms_crossed_demand",
    "seduce_tap_glass",
    "kneel_pout_beg",
    "dom_throne_lean",
    "dom_chin_tilt_summon",
];
/// The pool for an event, with a `'static` key for the no-repeat memory.
fn banter_key(key: &str) -> Option<(&'static str, &'static [&'static str])> {
    use crate::banter as b;
    const KEYS: &[&str] = &[
        "input.click.left",
        "input.click.double",
        "input.drag",
        b::DROP,
        "input.hover_long",
        "user.returned",
        b::WAKE,
        "idle.long",
        "idle.asleep",
        "focus.long_work",
        "window.opened.terminal",
        b::PRANK,
        b::CHAT_OPEN,
    ];
    let k = KEYS.iter().copied().find(|k| *k == key)?;
    Some((k, b::pool(k)?))
}

/// Clips that are bases/helpers, never a random reaction. Baked-jiggle
/// `<clip>_jiggle` variants are drawn by the animator in place of `<clip>`
/// (config `baked_jiggle`), never picked by name.
fn is_base_clip(c: &str) -> bool {
    matches!(c, "idle" | "sleep" | "drag_held" | "blink_wink")
        || c.ends_with("_jiggle")
        || c.starts_with("walk")
        || c.starts_with("run")
        || c.starts_with("blend_")
}

/// Priorities above the rule defaults.
pub const PRIO_DOUBLE: u8 = 90;
pub const PRIO_WAKE: u8 = 95;
pub const PRIO_KEY: u8 = 80;
pub const PRIO_PRANK: u8 = 20;

/// The behaviour state machine. See the module docs.
#[derive(Debug, Clone)]
pub struct Brain {
    cfg: BehaviorConfig,
    rules: RuleSet,
    clips: BTreeSet<String>,
    rng: Rng,
    t: f32,
    phase: Phase,
    mode: MoveMode,
    drowsy: bool,
    last_activity: f32,
    gestures: Gestures,
    mood: MoodState,
    mischief: Mischief,
    /// Rule event → earliest time it may fire again.
    cooldowns: BTreeMap<String, f32>,
    last_knife: Option<f32>,
    next_cycle: f32,
    /// Weighted no-repeat bag for the pose cycle, and the mood it was
    /// built for (rebuilt when the mood changes).
    cycle_bag: ShuffleBag,
    cycle_mood: Option<Mood>,
    next_wander: f32,
    x: f32,
    area: Option<(f32, f32)>,
    muted: bool,
    /// The overlay drives walks by root motion right now.
    root_motion: bool,
    /// Last clip picked from each `[rule_clip_pools]` entry (no repeats).
    last_pool_pick: BTreeMap<String, String>,
    /// Canned banter lines ([`crate::banter`]); its own RNG so lines
    /// never shift the behaviour randomness.
    banter: crate::banter::Picker,
    /// When she last greeted Ko (`user.returned`).
    last_greet: Option<f32>,
    out: Vec<Action>,
}

impl Brain {
    /// `clips` = clip names in the loaded manifest (unknown clips are never
    /// requested).
    pub fn new(
        cfg: BehaviorConfig,
        mode: MoveMode,
        rules: RuleSet,
        clips: impl IntoIterator<Item = String>,
        seed: u64,
    ) -> Self {
        let mut rng = Rng::new(seed);
        let next_wander = rng.range(cfg.wander_every_s.0, cfg.wander_every_s.1);
        Self {
            gestures: Gestures::new(cfg.gestures),
            mood: MoodState::new(cfg.mood_tau_s),
            mischief: Mischief::new(cfg.mischief),
            next_cycle: cfg.pose_cycle_s,
            cfg,
            rules,
            clips: clips.into_iter().collect(),
            rng,
            t: 0.0,
            phase: Phase::Idle,
            mode,
            drowsy: false,
            last_activity: 0.0,
            cooldowns: BTreeMap::new(),
            last_knife: None,
            cycle_bag: ShuffleBag::default(),
            cycle_mood: None,
            next_wander,
            x: 0.0,
            area: None,
            muted: false,
            root_motion: false,
            last_pool_pick: BTreeMap::new(),
            banter: crate::banter::Picker::new(seed ^ 0xB4_7E5D_EA10),
            last_greet: None,
            out: Vec::new(),
        }
    }

    // ── Read-outs ────────────────────────────────────────────────────────

    /// Seconds of fake clock so far.
    pub fn time(&self) -> f32 {
        self.t
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn mode(&self) -> MoveMode {
        self.mode
    }
    pub fn mood(&self) -> Mood {
        self.mood.mood()
    }
    pub fn mood_state(&self) -> &MoodState {
        &self.mood
    }
    pub fn mischief(&self) -> &Mischief {
        &self.mischief
    }
    pub fn drowsy(&self) -> bool {
        self.drowsy
    }
    pub fn asleep(&self) -> bool {
        self.phase == Phase::Asleep
    }
    pub fn muted(&self) -> bool {
        self.muted
    }
    pub fn hovering(&self) -> bool {
        self.gestures.hovering()
    }
    /// Where she is (left margin / x, px) as far as the brain knows.
    pub fn x(&self) -> f32 {
        self.x
    }
    pub fn config(&self) -> &BehaviorConfig {
        &self.cfg
    }
    /// Wander walking speed (screen px/s), e.g. matched to the walk clip's
    /// foot-locked `move_speed_px_s` so her boots don't slide.
    pub fn set_walk_speed(&mut self, px_s: f32) {
        if px_s.is_finite() && px_s > 0.0 {
            self.cfg.wander_speed_px_s = px_s;
        }
    }

    // ── Setup from the overlay ───────────────────────────────────────────

    /// Range of x positions she may wander over (left margin, px).
    pub fn set_area(&mut self, min: f32, max: f32) {
        self.area = Some((min.min(max), max.max(min)));
    }

    /// Current x (after a drop or at start-up).
    pub fn set_position(&mut self, x: f32) {
        self.x = x;
    }

    /// She lost her footing (her window moved/closed under her): like a
    /// drag start without the poke reaction: walks stop, the drag pose
    /// (`input.drag`) plays while she falls. [`Input::DragEnd`] lands her.
    pub fn begin_fall(&mut self) -> Vec<Action> {
        self.phase = Phase::Dragged;
        if let Some(clip) = self.resolve("input.drag") {
            self.out.push(Action::SetBase { clip });
        }
        self.take()
    }

    /// The overlay reports whether the drawn walk has root motion. While on
    /// (and `walk_root_motion` is set) walks advance only through
    /// [`Brain::root_motion`], not at `wander_speed_px_s`.
    pub fn set_root_motion(&mut self, on: bool) {
        self.root_motion = on && self.cfg.walk_root_motion;
    }

    /// Ground the walk animation covered this tick (screen px, signed,
    /// +x = right). While walking she advances by it (never past the
    /// target, which ends the walk); right after a stop it carries the last
    /// steps of the walk-out, clamped to the wander area.
    pub fn root_motion(&mut self, dx: f32) -> Vec<Action> {
        if dx == 0.0 || !self.root_motion {
            return self.take();
        }
        match self.phase {
            Phase::Walking { target, .. } => {
                let d = target - self.x;
                if dx.signum() != d.signum() || dx.abs() >= d.abs() {
                    self.x = target;
                    self.phase = Phase::Idle;
                    self.out.push(Action::MoveTo {
                        left: self.x.round() as i32,
                    });
                    self.out.push(Action::Stand);
                    self.schedule_wander();
                    self.arrived();
                } else {
                    self.x += dx;
                    self.out.push(Action::MoveTo {
                        left: self.x.round() as i32,
                    });
                }
            }
            Phase::Idle => {
                let mut x = self.x + dx;
                if let Some((lo, hi)) = self.area {
                    x = x.clamp(lo, hi);
                }
                if x != self.x {
                    self.x = x;
                    self.out.push(Action::MoveTo {
                        left: self.x.round() as i32,
                    });
                }
            }
            _ => {}
        }
        self.take()
    }

    pub fn set_mode(&mut self, mode: MoveMode) -> Vec<Action> {
        self.apply_mode(mode);
        self.take()
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    /// Feels `mood` at full strength (right-click menu, debug). It fades
    /// back to the baseline like any other feeling (`mood_tau_s`). Returns
    /// [`Action::MoodChanged`] if the mood changed.
    pub fn set_mood(&mut self, mood: Mood) -> Vec<Action> {
        self.feel(mood, 1.0);
        self.refresh_baseline();
        self.take()
    }

    // ── Inputs ───────────────────────────────────────────────────────────

    /// Handles one overlay input; returns the actions to apply.
    pub fn input(&mut self, input: Input) -> Vec<Action> {
        match input {
            Input::Click(b) => self.on_click(b),
            Input::Hover(inside) => {
                if let Some(g) = self.gestures.hover(inside, self.t) {
                    self.on_hover(g);
                }
            }
            Input::DragStart => {
                self.activity(1.0);
                self.phase = Phase::Dragged;
                let clip = self.resolve("input.drag").unwrap_or_default();
                if !clip.is_empty() {
                    self.out.push(Action::SetBase { clip });
                }
                self.rule_extras("input.drag");
            }
            Input::DragEnd { left } => {
                self.x = left as f32;
                if self.phase == Phase::Dragged {
                    self.phase = Phase::Idle;
                    self.out.push(Action::Stand);
                    self.say_banter_with_accent(crate::banter::DROP, PRIO_PRANK);
                }
                self.last_activity = self.t;
                self.schedule_wander();
            }
            Input::Key(k) => self.on_key(k),
        }
        self.refresh_baseline();
        self.take()
    }

    /// Handles a bus event (sensor or `lillith event …` injector).
    pub fn event(&mut self, ev: &Event) -> Vec<Action> {
        match ev.name.as_str() {
            "idle.long" => self.get_drowsy(),
            "idle.asleep" => self.fall_asleep(),
            "user.returned" => {
                // Waking up says its own line; otherwise greet Ko.
                if !self.activity(0.0) {
                    self.greet();
                }
            }
            "user.activity" => {
                self.activity(0.0);
            }
            name => {
                let prio = self.rules.find(name).map(|r| r.priority);
                if let Some(p) = prio {
                    if self.asleep() {
                        if p >= self.cfg.event_wake_priority {
                            self.activity(0.0);
                        } else {
                            return self.take();
                        }
                    }
                    self.fire_rule(name, Some(ev), false);
                }
            }
        }
        self.refresh_baseline();
        self.take()
    }

    /// Advances the fake clock by `dt`. `busy` = the animator is playing a
    /// reaction (pose cycle, wander and pranks wait for it).
    pub fn tick(&mut self, dt: f32, busy: bool) -> Vec<Action> {
        let dt = dt.max(0.0);
        self.t += dt;
        let t = self.t;
        let hovering = self.gestures.hovering();

        // Mischief and mood.
        let ignored = self.phase == Phase::Idle && !self.drowsy && !hovering;
        if self.mischief.tick(dt, ignored) && !busy {
            self.prank();
        }
        if self.mood.tick(dt) {
            self.out.push(Action::MoodChanged(self.mood.mood()));
        }

        // Sustained hover.
        if let Some(g) = self.gestures.tick(t) {
            self.on_hover(g);
        }

        // Sleep timers.
        let idle_for = t - self.last_activity;
        if !matches!(self.phase, Phase::Asleep | Phase::Dragged) && !hovering {
            if idle_for >= self.cfg.asleep_after_s {
                self.fall_asleep();
            } else if !self.drowsy && idle_for >= self.cfg.sleepy_after_s {
                self.get_drowsy();
            }
        }

        // Walking.
        // (With root motion on, the overlay advances her via `root_motion`.)
        if let (Phase::Walking { target, .. }, false) = (self.phase, self.root_motion) {
            let step = self.cfg.wander_speed_px_s * dt;
            let d = target - self.x;
            if d.abs() <= step {
                self.x = target;
                self.phase = Phase::Idle;
                self.out.push(Action::MoveTo {
                    left: self.x.round() as i32,
                });
                self.out.push(Action::Stand);
                self.schedule_wander();
                self.arrived();
            } else {
                self.x += step * d.signum();
                self.out.push(Action::MoveTo {
                    left: self.x.round() as i32,
                });
            }
        }

        // Wander: start a walk.
        if self.mode == MoveMode::Wander
            && self.phase == Phase::Idle
            && !busy
            && !self.drowsy
            && !hovering
            && t >= self.next_wander
        {
            self.start_wander();
        }

        // Pose cycle.
        if self.phase == Phase::Idle && !busy && !hovering && t >= self.next_cycle {
            self.cycle_pose();
        }

        self.refresh_baseline();
        self.take()
    }

    // ── Internals ────────────────────────────────────────────────────────

    fn take(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.out)
    }

    fn has(&self, clip: &str) -> bool {
        self.clips.contains(clip)
    }

    /// Clip for a rule event: `[rule_clips]` first, else the rule's pose if
    /// a clip by that name exists.
    pub fn resolve(&self, event: &str) -> Option<String> {
        if let Some(c) = self.rules.clip_for(event) {
            if self.has(c) {
                return Some(c.to_string());
            }
        }
        let pose = &self.rules.find(event)?.pose;
        self.has(pose).then(|| pose.clone())
    }

    /// A random loaded clip from the event's `[rule_clip_pools]` entry,
    /// never the one picked last time for that event.
    fn resolve_pool(&mut self, event: &str) -> Option<String> {
        let pool: Vec<String> = self
            .rules
            .clip_pool(event)
            .iter()
            .filter(|c| self.has(c))
            .cloned()
            .collect();
        let last = self
            .last_pool_pick
            .get(event)
            .and_then(|l| pool.iter().position(|c| c == l));
        let i = crate::bag::pick_no_repeat(&mut self.rng, pool.len(), last)?;
        let clip = pool[i].clone();
        self.last_pool_pick.insert(event.to_string(), clip.clone());
        Some(clip)
    }

    /// Clip for a key-map pose: the clip itself, else the clip of the first
    /// rule using that pose, else `event_<pose>`.
    pub fn resolve_pose(&self, pose: &str) -> Option<String> {
        if self.has(pose) {
            return Some(pose.to_string());
        }
        let via_rule = self
            .rules
            .rules
            .iter()
            .filter(|r| r.enabled && r.pose == pose)
            .find_map(|r| self.resolve(&r.event));
        via_rule.or_else(|| {
            let e = format!("event_{pose}");
            self.has(&e).then_some(e)
        })
    }

    fn sfx(&mut self, cue: &str) {
        if !self.muted && !cue.is_empty() {
            self.out.push(Action::Sfx(cue.to_string()));
        }
    }

    fn feel(&mut self, mood: Mood, strength: f32) {
        if self.mood.feel(mood, strength) {
            self.out.push(Action::MoodChanged(mood));
        }
    }

    fn baseline(&self) -> Mood {
        if self.asleep() || self.drowsy {
            Mood::Sleepy
        } else if self.mischief.is_high() {
            Mood::Mischievous
        } else {
            Mood::Playful
        }
    }

    fn refresh_baseline(&mut self) {
        let b = self.baseline();
        if self.mood.baseline() != b && self.mood.set_baseline(b) {
            self.out.push(Action::MoodChanged(b));
        }
    }

    /// Line, SFX and mood of a rule (no clip).
    fn rule_extras(&mut self, event: &str) {
        let Some(rule) = self.rules.find(event).cloned() else {
            return;
        };
        let line = rule
            .line_for(&Event::new(event))
            .filter(|l| !l.trim().is_empty())
            .or_else(|| self.banter_line(event));
        if let Some(line) = line {
            self.out.push(Action::Say(line));
        }
        if let Some(s) = &rule.sfx {
            self.sfx(s);
        }
        if let Some(m) = rule.mood {
            self.feel(m, 0.7);
        }
    }

    /// Fires a rule: clip at its priority (or `priority` override), line,
    /// SFX, mood. Respects the rule's cooldown unless `force`. Returns
    /// `true` if it fired.
    fn fire_rule_at(
        &mut self,
        event: &str,
        ev: Option<&Event>,
        force: bool,
        priority: Option<u8>,
    ) -> bool {
        let Some(rule) = self.rules.find(event).cloned() else {
            return false;
        };
        if !force {
            if let Some(&until) = self.cooldowns.get(event) {
                if self.t < until {
                    return false;
                }
            }
        }
        if rule.cooldown_s > 0.0 {
            self.cooldowns
                .insert(event.to_string(), self.t + rule.cooldown_s);
        }
        if let Some(clip) = self.resolve_pool(event).or_else(|| self.resolve(event)) {
            self.out.push(Action::React {
                clip,
                priority: priority.unwrap_or(rule.priority),
            });
        }
        let line = match ev {
            Some(e) => rule.line_for(e),
            None => rule.line_for(&Event::new(event)),
        };
        let line = line
            .filter(|l| !l.trim().is_empty())
            .or_else(|| self.banter_line(event));
        if let Some(line) = line {
            self.out.push(Action::Say(line));
        }
        if let Some(s) = &rule.sfx {
            self.sfx(s);
        }
        if let Some(m) = rule.mood {
            self.feel(m, 0.8);
        }
        true
    }

    fn fire_rule(&mut self, event: &str, ev: Option<&Event>, force: bool) -> bool {
        self.fire_rule_at(event, ev, force, None)
    }

    /// A canned line for `key` from [`crate::banter::pool`], if it has one.
    /// A rule's own `line` in rules.toml still wins over the pool.
    fn banter_line(&mut self, key: &str) -> Option<String> {
        let (key, pool) = banter_key(key)?;
        self.banter.pick(key, pool)
    }

    fn say_banter(&mut self, key: &str) {
        if let Some(line) = self.banter_line(key) {
            self.out.push(Action::Say(line));
        }
    }

    /// Says a line for `key` and, if one of its optional accent clips
    /// ([`crate::banter::accent_clips`]) is in the manifest, plays it.
    /// Missing clips are skipped: the line alone is fine.
    fn say_banter_with_accent(&mut self, key: &str, priority: u8) {
        let have: Vec<&str> = crate::banter::accent_clips(key)
            .iter()
            .copied()
            .filter(|c| self.has(c))
            .collect();
        if !have.is_empty() && self.phase == Phase::Idle {
            let i = self.banter.index(have.len());
            self.out.push(Action::React {
                clip: have[i].to_string(),
                priority,
            });
        }
        self.say_banter(key);
    }

    /// Ko came back: a greeting, at most every `GREET_GAP_S`.
    fn greet(&mut self) {
        let gap = crate::banter::GREET_GAP_S;
        if self.last_greet.is_some_and(|t0| self.t - t0 < gap) {
            return;
        }
        self.last_greet = Some(self.t);
        self.say_banter_with_accent("user.returned", PRIO_PRANK);
    }

    /// Someone paid attention. `poke` > 0 adds mischief energy. Wakes her
    /// (returns `true` if she was asleep), clears drowsiness, stops a walk,
    /// and pushes the next pose cycle back.
    fn activity(&mut self, poke: f32) -> bool {
        self.last_activity = self.t;
        if poke > 0.0 {
            self.mischief.poke(poke);
        } else {
            self.mischief.attend();
        }
        self.next_cycle = self.t + self.cfg.pose_cycle_s;
        self.drowsy = false;
        if let Phase::Walking { .. } = self.phase {
            self.phase = Phase::Idle;
            self.out.push(Action::Stand);
            self.schedule_wander();
        }
        if self.asleep() {
            self.wake();
            return true;
        }
        false
    }

    fn wake(&mut self) {
        self.phase = Phase::Idle;
        self.drowsy = false;
        self.out.push(Action::Stand);
        let clip = self.resolve("user.returned");
        if let Some(clip) = clip {
            self.out.push(Action::React {
                clip,
                priority: PRIO_WAKE,
            });
        }
        if let Some(s) = self.rules.find("user.returned").and_then(|r| r.sfx.clone()) {
            self.sfx(&s);
        }
        self.say_banter(crate::banter::WAKE);
        self.mood.set_baseline(Mood::Playful);
        self.feel(Mood::Happy, 0.9);
        self.schedule_wander();
    }

    fn get_drowsy(&mut self) {
        if self.drowsy || self.asleep() {
            return;
        }
        self.drowsy = true;
        if let Phase::Walking { .. } = self.phase {
            self.phase = Phase::Idle;
            self.out.push(Action::Stand);
        }
        self.fire_rule("idle.long", None, true);
        self.mood.set_baseline(Mood::Sleepy);
        self.feel(Mood::Sleepy, 0.8);
    }

    fn fall_asleep(&mut self) {
        if self.asleep() {
            return;
        }
        self.phase = Phase::Asleep;
        self.drowsy = true;
        // Stay asleep until something wakes her.
        self.last_activity = self.t - self.cfg.asleep_after_s;
        if let Some(clip) = self.resolve("idle.asleep") {
            self.out.push(Action::SetBase { clip });
        }
        if let Some(s) = self.rules.find("idle.asleep").and_then(|r| r.sfx.clone()) {
            self.sfx(&s);
        }
        self.say_banter("idle.asleep");
        self.mood.set_baseline(Mood::Sleepy);
        self.feel(Mood::Sleepy, 1.0);
    }

    fn on_click(&mut self, b: Button) {
        let g = self.gestures.click(b, self.t);
        if self.activity(1.0) {
            return; // waking up is the reaction
        }
        if self.phase == Phase::Dragged {
            return;
        }
        match g {
            Gesture::Left => self.left_click(),
            Gesture::Double => {
                self.fire_rule_at("input.click.double", None, false, Some(PRIO_DOUBLE));
            }
            Gesture::Right => {
                self.fire_rule("input.click.right", None, false);
                self.out.push(Action::ContextMenu);
            }
            Gesture::Middle => {
                self.fire_rule("input.click.middle", None, false);
            }
            Gesture::Secret => {
                if self.fire_rule("input.secret", None, false) {
                    self.feel(Mood::Happy, 1.0);
                } else {
                    // On cooldown: behave like a plain right click.
                    self.fire_rule("input.click.right", None, false);
                    self.out.push(Action::ContextMenu);
                }
            }
            _ => {}
        }
    }

    fn left_click(&mut self) {
        let high = self.mischief.is_high();
        let recent = self
            .last_knife
            .is_some_and(|t0| self.t - t0 < self.cfg.left_repeat_s);
        if high || !recent || !self.has("wave") {
            self.fire_rule("input.click.left", None, false);
            self.last_knife = Some(self.t);
            if high {
                self.feel(Mood::Mischievous, 0.9);
            }
        } else {
            let prio = self
                .rules
                .find("input.click.left")
                .map(|r| r.priority)
                .unwrap_or(85);
            self.out.push(Action::React {
                clip: "wave".into(),
                priority: prio,
            });
            self.sfx("click");
            self.feel(Mood::Playful, 0.6);
            self.last_knife = None;
        }
    }

    fn on_hover(&mut self, g: Gesture) {
        match g {
            Gesture::HoverEnter => {
                // Proximity wakes her: "one of the most alive moments".
                if self.activity(0.0) {
                    return;
                }
                if self.phase == Phase::Idle {
                    self.fire_rule("input.hover", None, false);
                }
            }
            Gesture::HoverLong => {
                self.last_activity = self.t;
                if self.phase == Phase::Idle {
                    self.fire_rule("input.hover_long", None, false);
                }
            }
            _ => {}
        }
    }

    fn on_key(&mut self, k: Key) {
        let c = match k {
            Key::Escape => {
                self.out.push(Action::Quit);
                return;
            }
            Key::Enter => 'Y',
            Key::Char(c) => c.to_ascii_uppercase(),
        };
        match c {
            'Q' => {
                self.out.push(Action::Quit);
                return;
            }
            'Y' => {
                self.activity(0.0);
                self.out.push(Action::OpenChat);
                return;
            }
            'G' => {
                self.activity(0.0);
                let m = if self.mode == MoveMode::Wander {
                    MoveMode::Stay
                } else {
                    MoveMode::Wander
                };
                self.apply_mode(m);
                return;
            }
            _ => {}
        }
        let binding = self.rules.keys.get(&c.to_string()).cloned();
        match binding {
            Some(KeyBinding::Action { action }) => match action.as_str() {
                "sleep" => self.fall_asleep(),
                "toggle_mute" => {
                    self.activity(0.0);
                    self.muted = !self.muted;
                    self.out.push(Action::Muted(self.muted));
                }
                "cycle_pose" => {
                    self.activity(0.0);
                    self.cycle_pose();
                }
                "random_reaction" => {
                    self.activity(0.0);
                    let pool: Vec<&String> =
                        self.clips.iter().filter(|c| !is_base_clip(c)).collect();
                    if !pool.is_empty() {
                        let clip = pool[self.rng.index(pool.len())].clone();
                        self.out.push(Action::React {
                            clip,
                            priority: PRIO_KEY,
                        });
                        self.sfx("idle");
                    }
                }
                other => tracing::debug!("key {c}: action {other} not implemented"),
            },
            Some(KeyBinding::Pose { pose }) => {
                self.activity(0.0);
                match self.resolve_pose(&pose) {
                    Some(clip) => self.out.push(Action::React {
                        clip,
                        priority: PRIO_KEY,
                    }),
                    None => tracing::debug!("key {c}: no clip for pose {pose}"),
                }
            }
            None => {}
        }
    }

    fn apply_mode(&mut self, mode: MoveMode) {
        if mode == self.mode {
            return;
        }
        self.mode = mode;
        if mode != MoveMode::Wander {
            if let Phase::Walking { .. } = self.phase {
                self.phase = Phase::Idle;
                self.out.push(Action::Stand);
            }
        } else {
            self.schedule_wander();
        }
        self.out.push(Action::ModeChanged(mode));
    }

    fn schedule_wander(&mut self) {
        self.next_wander = self.t
            + self
                .rng
                .range(self.cfg.wander_every_s.0, self.cfg.wander_every_s.1);
    }

    fn start_wander(&mut self) {
        let Some((lo, hi)) = self.area else {
            self.schedule_wander();
            return;
        };
        let min_step = self.cfg.wander_min_step_px;
        if hi - lo < min_step {
            self.schedule_wander();
            return;
        }
        // A few tries for a target far enough away.
        for _ in 0..8 {
            let target = self.rng.range(lo, hi).round();
            if (target - self.x).abs() >= min_step {
                let right = target > self.x;
                self.phase = Phase::Walking { target, right };
                self.out.push(Action::Walk { right });
                return;
            }
        }
        self.schedule_wander();
    }

    /// A wander walk just ended: now and then she strikes a pose soon
    /// after arriving (`wander_pose_chance`).
    fn arrived(&mut self) {
        let chance = self.cfg.wander_pose_chance;
        if chance.is_finite() && chance > 0.0 && self.rng.next_f32() < chance {
            let at = self.t + self.cfg.wander_pose_delay_s.max(0.0);
            self.next_cycle = self.next_cycle.min(at);
        }
    }

    /// Rebuilds the pose-cycle bag when the mood changed: the mood's
    /// variants that exist, flirty ones weighted by `[behavior.idle]
    /// flirt_weight`, never repeating the last pick across the rebuild.
    fn sync_cycle_bag(&mut self) {
        let mood = self.mood.mood();
        if self.cycle_mood == Some(mood) {
            return;
        }
        let fw = self.cfg.idle.flirt_weight;
        let entries: Vec<(String, u32)> = cycle_variants(mood)
            .iter()
            .filter(|c| self.clips.contains(**c))
            .map(|c| (c.to_string(), crate::idle::flirt_weight_of(c, fw)))
            .collect();
        let last = self.cycle_bag.last().map(str::to_string);
        self.cycle_bag = ShuffleBag::new(entries);
        if let Some(l) = last {
            self.cycle_bag.set_last(&l);
        }
        self.cycle_mood = Some(mood);
    }

    fn cycle_pose(&mut self) {
        self.next_cycle = self.t + self.cfg.pose_cycle_s;
        self.sync_cycle_bag();
        let Some(clip) = self.cycle_bag.next(&mut self.rng).map(str::to_string) else {
            return;
        };
        let paired = crate::banter::clip_pool(&clip);
        self.out.push(Action::React { clip, priority: 0 });
        self.sfx("soft");
        if let Some(pool) = paired {
            if self.banter.chance(crate::banter::CLIP_LINE_CHANCE) {
                if let Some(line) = self.banter.pick("clip", pool) {
                    self.out.push(Action::Say(line));
                }
            }
        }
    }

    fn prank(&mut self) {
        let pool: Vec<&str> = PRANKS.iter().copied().filter(|c| self.has(c)).collect();
        if pool.is_empty() || self.phase != Phase::Idle {
            return;
        }
        let clip = pool[self.rng.index(pool.len())].to_string();
        self.out.push(Action::React {
            clip,
            priority: PRIO_PRANK,
        });
        self.say_banter(crate::banter::PRANK);
        self.feel(Mood::Mischievous, 0.8);
        self.next_cycle = self.t + self.cfg.pose_cycle_s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIPS: &[&str] = &[
        "idle",
        "walk_right",
        "walk_left",
        "sleep",
        "wake",
        "wave",
        "knife_tap",
        "drag_held",
        "sit_swing",
        "tail_wag",
        "hip_sway",
        "lean_forward",
        "shoulder_roll",
        "nod",
        "think",
        "dance",
        "event_help",
        "event_input_click_double",
        "event_input_click_right",
        "event_input_hover_long",
        "event_wifi",
        "event_cpu_busy",
        "event_battery_low",
    ];

    fn cfg() -> BehaviorConfig {
        BehaviorConfig {
            sleepy_after_s: 30.0,
            asleep_after_s: 60.0,
            ..BehaviorConfig::default()
        }
    }

    fn brain_with(cfg: BehaviorConfig, mode: MoveMode) -> Brain {
        Brain::new(
            cfg,
            mode,
            RuleSet::bundled(),
            CLIPS.iter().map(|s| s.to_string()),
            7,
        )
    }

    fn brain() -> Brain {
        brain_with(cfg(), MoveMode::Stay)
    }

    /// Runs `secs` of fake clock at 10 Hz, collecting actions.
    fn run(b: &mut Brain, secs: f32, busy: bool) -> Vec<Action> {
        let mut out = Vec::new();
        for _ in 0..(secs * 10.0).round() as usize {
            out.extend(b.tick(0.1, busy));
        }
        out
    }

    fn reacts(a: &[Action]) -> Vec<&str> {
        a.iter()
            .filter_map(|a| match a {
                Action::React { clip, .. } => Some(clip.as_str()),
                _ => None,
            })
            .collect()
    }

    fn click(b: &mut Brain, btn: Button) -> Vec<Action> {
        b.input(Input::Click(btn))
    }

    fn says(a: &[Action]) -> Vec<&str> {
        a.iter()
            .filter_map(|a| match a {
                Action::Say(l) => Some(l.as_str()),
                _ => None,
            })
            .collect()
    }

    fn in_pool(line: &str, key: &str) -> bool {
        crate::banter::pool(key).unwrap().contains(&line)
    }

    #[test]
    fn banter_for_pokes_drags_drops_and_greetings() {
        let mut b = brain();
        let a = click(&mut b, Button::Left);
        let s = says(&a);
        assert_eq!(s.len(), 1, "{a:?}");
        assert!(in_pool(s[0], "input.click.left"), "{s:?}");
        b.tick(5.0, false);
        let a = b.input(Input::DragStart);
        assert!(in_pool(says(&a)[0], "input.drag"));
        let a = b.input(Input::DragEnd { left: 10 });
        let s = says(&a);
        assert!(in_pool(s[0], crate::banter::DROP), "{s:?}");
        // The dominant accent clips aren't in this manifest: no clip, line only.
        assert!(reacts(&a).is_empty(), "{a:?}");
        // Greeting on user.returned (awake), at most once a minute.
        b.tick(5.0, false);
        let a = b.event(&Event::new("user.returned"));
        assert!(in_pool(says(&a)[0], "user.returned"), "{a:?}");
        b.tick(5.0, false);
        let a = b.event(&Event::new("user.returned"));
        assert!(says(&a).is_empty(), "greeting gap");
    }

    #[test]
    fn banter_accent_clip_only_when_present() {
        let mut clips: Vec<String> = CLIPS.iter().map(|s| s.to_string()).collect();
        clips.push("dom_arms_crossed_demand".into());
        let mut b = Brain::new(cfg(), MoveMode::Stay, RuleSet::bundled(), clips, 7);
        b.input(Input::DragStart);
        let a = b.input(Input::DragEnd { left: 10 });
        assert_eq!(reacts(&a), ["dom_arms_crossed_demand"]);
        assert!(in_pool(says(&a)[0], crate::banter::DROP));
    }

    #[test]
    fn banter_wake_and_goodnight() {
        let mut b = brain();
        let a = b.event(&Event::new("idle.asleep"));
        assert!(b.asleep());
        assert!(in_pool(says(&a)[0], "idle.asleep"), "{a:?}");
        let a = b.event(&Event::new("user.returned"));
        let s = says(&a);
        assert_eq!(s.len(), 1, "wake line, no extra greeting: {s:?}");
        assert!(in_pool(s[0], crate::banter::WAKE), "{s:?}");
    }

    #[test]
    fn defaults_match_rules_thresholds() {
        let r = RuleSet::bundled();
        let d = BehaviorConfig::default();
        assert_eq!(r.threshold("idle_sleepy_s"), Some(d.sleepy_after_s as f64));
        assert_eq!(r.threshold("idle_asleep_s"), Some(d.asleep_after_s as f64));
        assert_eq!(
            r.threshold("secret_sequence_window_s"),
            Some(d.gestures.secret_window_s as f64)
        );
        assert_eq!(crate::config::Config::default().pose_cycle_s, None);
        // Defaults: long pose cycle, calm but regular wandering (Ko: she
        // walks left and right on her own between idle poses).
        assert_eq!(d.pose_cycle_s, 45.0);
        assert_eq!(d.wander_every_s, (20.0, 45.0));
        assert_eq!(d.wander_speed_px_s, 45.0);
        assert_eq!(crate::config::Config::default().mode, MoveMode::Wander);
    }

    #[test]
    fn left_click_knife_then_wave_then_knife() {
        let mut b = brain();
        let a = click(&mut b, Button::Left);
        assert_eq!(reacts(&a), ["knife_tap"]);
        assert!(a.contains(&Action::Sfx("click".into())));
        run(&mut b, 1.0, false);
        assert_eq!(reacts(&click(&mut b, Button::Left)), ["wave"]);
        run(&mut b, 1.0, false);
        assert_eq!(reacts(&click(&mut b, Button::Left)), ["knife_tap"]);
        // After left_repeat_s the knife comes back on its own.
        run(&mut b, 5.0, true);
        assert_eq!(reacts(&click(&mut b, Button::Left)), ["knife_tap"]);
    }

    #[test]
    fn double_click_cuts_in_and_right_click_never_quits() {
        let mut b = brain();
        click(&mut b, Button::Left);
        b.tick(0.2, true);
        let a = click(&mut b, Button::Left);
        assert_eq!(
            a.iter().find(|a| matches!(a, Action::React { .. })),
            Some(&Action::React {
                clip: "event_input_click_double".into(),
                priority: PRIO_DOUBLE
            })
        );
        run(&mut b, 5.0, true);
        let a = click(&mut b, Button::Right);
        assert_eq!(reacts(&a), ["event_input_click_right"]);
        assert!(a.contains(&Action::ContextMenu));
        assert!(!a.contains(&Action::Quit));
        let a = click(&mut b, Button::Middle);
        assert_eq!(reacts(&a), ["event_help"]);
    }

    #[test]
    fn secret_sequence_fires_once_then_cools_down() {
        let mut b = brain();
        let seq = |b: &mut Brain| {
            click(b, Button::Left);
            b.tick(1.0, true);
            click(b, Button::Left);
            b.tick(0.2, true);
            click(b, Button::Left);
            b.tick(0.5, true);
            click(b, Button::Right)
        };
        let a = seq(&mut b);
        assert_eq!(reacts(&a), ["tail_wag"]);
        assert!(a.contains(&Action::Say("You found me out~".into())));
        assert_eq!(b.mood(), Mood::Happy);
        assert!(!a.contains(&Action::ContextMenu));
        // Within the 30 s cooldown it is just a right click.
        b.tick(2.0, true);
        let a = seq(&mut b);
        assert_eq!(reacts(&a), ["event_input_click_right"]);
    }

    #[test]
    fn mischief_unlocks_knife_and_mischievous_mood_then_decays() {
        let mut b = brain();
        for _ in 0..5 {
            click(&mut b, Button::Middle);
            b.tick(1.0, true);
        }
        assert!(b.mischief().is_high());
        assert_eq!(b.mood(), Mood::Mischievous);
        // High mischief: repeated left clicks keep stabbing.
        assert_eq!(reacts(&click(&mut b, Button::Left)), ["knife_tap"]);
        b.tick(1.0, true);
        assert_eq!(reacts(&click(&mut b, Button::Left)), ["knife_tap"]);
        // A couple of quiet minutes: back to normal.
        run(&mut b, 25.0, true);
        assert!(!b.mischief().is_high());
        run(&mut b, 3.0, true);
        assert_eq!(b.mood(), Mood::Playful, "fades back to baseline");
    }

    #[test]
    fn ignored_she_pranks() {
        let mut b = brain_with(
            BehaviorConfig {
                sleepy_after_s: 1e6,
                asleep_after_s: 2e6,
                pose_cycle_s: 1e6,
                ..BehaviorConfig::default()
            },
            MoveMode::Stay,
        );
        let a = run(&mut b, 151.0, false);
        let r = reacts(&a);
        assert_eq!(r.len(), 1, "{r:?}");
        assert!(PRANKS.contains(&r[0]));
        assert!(a.iter().any(|a| matches!(a, Action::Say(_))));
        assert_eq!(b.mood(), Mood::Mischievous);
    }

    #[test]
    fn sleepy_then_asleep_then_wake_on_hover() {
        let mut b = brain();
        let a = run(&mut b, 29.0, true);
        assert!(!b.drowsy() && reacts(&a).is_empty());
        let a = run(&mut b, 2.0, true);
        assert!(b.drowsy());
        assert_eq!(b.mood(), Mood::Sleepy);
        assert_eq!(reacts(&a), ["sit_swing"], "idle.long");
        let a = run(&mut b, 30.0, true);
        assert!(b.asleep());
        assert!(a.contains(&Action::SetBase {
            clip: "sleep".into()
        }));
        // Asleep: no pose cycle, sleeps through low-priority events.
        assert!(reacts(&run(&mut b, 100.0, false)).is_empty());
        assert!(b.asleep());
        assert!(b.event(&Event::new("wifi.changed")).is_empty());
        // Proximity wakes her.
        let a = b.input(Input::Hover(true));
        assert!(!b.asleep());
        assert_eq!(a[0], Action::Stand);
        assert!(a.contains(&Action::React {
            clip: "wake".into(),
            priority: PRIO_WAKE
        }));
        assert_eq!(b.mood(), Mood::Happy);
        // While hovered she does not fall asleep again.
        run(&mut b, 200.0, false);
        assert!(!b.asleep());
    }

    #[test]
    fn click_wakes_without_click_reaction_and_events_wake() {
        let mut b = brain();
        b.event(&Event::new("idle.asleep"));
        assert!(b.asleep());
        let a = click(&mut b, Button::Left);
        assert_eq!(reacts(&a), ["wake"]);
        assert!(!b.asleep());
        // Key S forces sleep; a high-priority event wakes her.
        b.input(Input::Key(Key::Char('s')));
        assert!(b.asleep());
        let a = b.event(&Event::new("battery.low"));
        assert!(!b.asleep());
        assert_eq!(reacts(&a), ["wake", "event_battery_low"]);
        assert_eq!(b.mood(), Mood::Alert);
        // user.returned also wakes.
        b.event(&Event::new("idle.asleep"));
        let a = b.event(&Event::new("user.returned"));
        assert_eq!(reacts(&a), ["wake"]);
    }

    #[test]
    fn hover_enter_and_long_with_cooldowns() {
        let mut b = brain();
        let a = b.input(Input::Hover(true));
        assert_eq!(reacts(&a), ["sit_swing"]);
        let a = run(&mut b, 1.5, true);
        assert_eq!(reacts(&a), ["event_input_hover_long"]);
        b.input(Input::Hover(false));
        // Re-enter within the 3 s hover cooldown: nothing.
        assert!(reacts(&b.input(Input::Hover(true))).is_empty());
        let a = run(&mut b, 2.0, true);
        assert!(reacts(&a).is_empty(), "hover_long on 10 s cooldown");
        b.input(Input::Hover(false));
        run(&mut b, 2.0, true);
        assert_eq!(reacts(&b.input(Input::Hover(true))), ["sit_swing"]);
    }

    #[test]
    fn hover_and_click_pick_flirty_pool_clips_without_repeats() {
        let mut clips: Vec<String> = CLIPS.iter().map(|s| s.to_string()).collect();
        for c in [
            "hip_pop_wink",
            "blow_kiss",
            "lean_tease",
            "cheeky_tongue_wink",
        ] {
            clips.push(c.to_string());
        }
        let mut b = Brain::new(
            BehaviorConfig {
                sleepy_after_s: 1e6,
                asleep_after_s: 2e6,
                ..BehaviorConfig::default()
            },
            MoveMode::Stay,
            RuleSet::bundled(),
            clips,
            11,
        );
        let mut seen = Vec::new();
        for _ in 0..12 {
            let a = b.input(Input::Hover(true));
            let r: Vec<String> = reacts(&a).into_iter().map(String::from).collect();
            assert_eq!(r.len(), 1, "{r:?}");
            assert!(["hip_pop_wink", "blow_kiss", "lean_tease"].contains(&&*r[0]));
            seen.push(r[0].clone());
            b.input(Input::Hover(false));
            run(&mut b, 3.5, false);
        }
        for w in seen.windows(2) {
            assert_ne!(w[0], w[1], "{seen:?}");
        }
        assert!(seen.iter().collect::<std::collections::BTreeSet<_>>().len() >= 2);
    }

    #[test]
    fn drag_sets_pose_and_drop_stands() {
        let mut b = brain();
        let a = b.input(Input::DragStart);
        assert_eq!(b.phase(), Phase::Dragged);
        assert!(a.contains(&Action::SetBase {
            clip: "drag_held".into()
        }));
        // Clicks during a drag (other buttons) do nothing visible.
        assert!(reacts(&click(&mut b, Button::Middle)).is_empty());
        let a = b.input(Input::DragEnd { left: 333 });
        assert!(a.contains(&Action::Stand), "{a:?}");
        // Losing her footing: drag pose, no walking, until she lands.
        let a = b.begin_fall();
        assert_eq!(
            a,
            [Action::SetBase {
                clip: "drag_held".into()
            }]
        );
        assert_eq!(b.phase(), Phase::Dragged);
        let a = b.input(Input::DragEnd { left: 333 });
        assert_eq!(b.phase(), Phase::Idle);
        assert!(a.contains(&Action::Stand));
        assert_eq!(b.x(), 333.0);
    }

    #[test]
    fn flirty_batch_clips_are_reachable_from_the_pose_cycle() {
        let moods = [
            Mood::Playful,
            Mood::Happy,
            Mood::Mischievous,
            Mood::Alert,
            Mood::Sleepy,
        ];
        let all: BTreeSet<&str> = moods
            .iter()
            .flat_map(|&m| cycle_variants(m).iter().copied())
            .collect();
        let mannerisms = [
            "hip_lean",
            "pigtail_twirl",
            "hands_behind_sway",
            "stretch",
            "glove_adjust",
            "tail_curl",
            "lip_bite",
            "shy_giggle",
            "foot_tap",
            "look_away",
        ];
        for m in mannerisms {
            let name = format!("idle_flirt_{m}");
            assert!(
                cycle_variants(Mood::Playful).contains(&name.as_str()),
                "{name}"
            );
        }
        for pose in [
            "hip_pop_wink",
            "shoulder_shimmy",
            "hair_flip",
            "blow_kiss",
            "tiptoe_bounce",
            "lean_tease",
            "twirl",
            "stretch_arch",
            "peace_sign_pose",
            "heart_hands",
            "tail_tease",
            "over_shoulder_look",
            "hands_behind_head_pose",
            "sway_dance",
            "finger_wag_tease",
            "bounce_giggle",
            "hair_twirl",
            "cheeky_tongue_wink",
            "victory_jump",
        ] {
            assert!(all.contains(pose), "{pose}");
        }
        assert!(all.iter().all(|c| !c.ends_with("_jiggle")));
        assert!(is_base_clip("knife_tap_jiggle") && !is_base_clip("idle_flirt_lip_bite"));
    }

    #[test]
    fn pose_cycle_every_16s_when_idle() {
        let mut b = brain_with(
            BehaviorConfig {
                pose_cycle_s: 16.0,
                sleepy_after_s: 1e6,
                asleep_after_s: 2e6,
                mischief: MischiefConfig {
                    boredom_s: 0.0,
                    ..MischiefConfig::default()
                },
                ..BehaviorConfig::default()
            },
            MoveMode::Stay,
        );
        let mut times = Vec::new();
        let mut clips = Vec::new();
        for _ in 0..650 {
            for a in b.tick(0.1, false) {
                if let Action::React { clip, priority: 0 } = a {
                    times.push(b.time());
                    clips.push(clip);
                }
            }
        }
        assert_eq!(times.len(), 4, "{times:?}");
        assert!((times[0] - 16.0).abs() < 0.15);
        for w in times.windows(2) {
            assert!((w[1] - w[0] - 16.0).abs() < 0.15);
        }
        // Weighted shuffle bag over the mood's variants: no immediate
        // repeats, only clips of the current mood that exist.
        for w in clips.windows(2) {
            assert_ne!(w[0], w[1], "{clips:?}");
        }
        assert!(
            clips
                .iter()
                .all(|c| cycle_variants(b.mood()).contains(&c.as_str()) && b.has(c)),
            "{clips:?}"
        );
        // Busy holds it back; an interaction pushes it back.
        assert!(reacts(&run(&mut b, 20.0, true)).is_empty());
        click(&mut b, Button::Middle);
        assert!(run(&mut b, 15.0, false)
            .iter()
            .all(|a| !matches!(a, Action::React { priority: 0, .. })));
    }

    #[test]
    fn wander_walks_between_positions_and_stay_stops() {
        let mut b = brain_with(
            BehaviorConfig {
                sleepy_after_s: 1e6,
                asleep_after_s: 2e6,
                pose_cycle_s: 1e6,
                wander_every_s: (20.0, 45.0),
                wander_speed_px_s: 60.0,
                mischief: MischiefConfig {
                    boredom_s: 0.0,
                    ..MischiefConfig::default()
                },
                ..BehaviorConfig::default()
            },
            MoveMode::Wander,
        );
        b.set_area(0.0, 1500.0);
        b.set_position(700.0);
        // Nothing before the first pause (≥ 20 s).
        assert!(run(&mut b, 19.9, false).is_empty());
        let a = run(&mut b, 25.2, false);
        let walk = a.iter().position(|a| matches!(a, Action::Walk { .. }));
        let walk = walk.expect("started a walk");
        let Action::Walk { right } = a[walk] else {
            unreachable!()
        };
        let Phase::Walking { target, .. } = b.phase() else {
            panic!("walking: {:?}", b.phase());
        };
        assert_eq!(right, target > 700.0);
        assert!((target - 700.0).abs() >= 120.0);
        // Moves 6 px per 0.1 s tick towards the target, then stands.
        let secs = (target - b.x()).abs() / 60.0 + 0.3;
        let a = run(&mut b, secs, false);
        let moves: Vec<i32> = a
            .iter()
            .filter_map(|a| match a {
                Action::MoveTo { left } => Some(*left),
                _ => None,
            })
            .collect();
        assert!(moves.windows(2).all(|w| (w[1] - w[0]).abs() <= 6));
        assert_eq!(*moves.last().unwrap(), target as i32);
        assert!(a.contains(&Action::Stand));
        assert_eq!(b.phase(), Phase::Idle);
        // Next walk, then G switches to Stay mid-walk.
        let a = run(&mut b, 46.0, false);
        assert!(a.iter().any(|a| matches!(a, Action::Walk { .. })));
        let a = b.input(Input::Key(Key::Char('g')));
        assert!(a.contains(&Action::ModeChanged(MoveMode::Stay)));
        assert_eq!(b.phase(), Phase::Idle);
        assert!(run(&mut b, 120.0, false)
            .iter()
            .all(|a| !matches!(a, Action::Walk { .. } | Action::MoveTo { .. })));
        let a = b.set_mode(MoveMode::Wander);
        assert_eq!(a, [Action::ModeChanged(MoveMode::Wander)]);
    }

    fn calm_cfg() -> BehaviorConfig {
        BehaviorConfig {
            sleepy_after_s: 1e6,
            asleep_after_s: 2e6,
            mischief: MischiefConfig {
                boredom_s: 0.0,
                ..MischiefConfig::default()
            },
            ..BehaviorConfig::default()
        }
    }

    /// Pose-cycle picks over `secs` with every Playful variant available.
    fn cycle_picks(flirt_weight: u32, secs: f32) -> Vec<String> {
        let mut cfg = calm_cfg();
        cfg.pose_cycle_s = 1.0;
        cfg.idle.flirt_weight = flirt_weight;
        let clips = CLIPS
            .iter()
            .chain(cycle_variants(Mood::Playful))
            .map(|s| s.to_string());
        let mut b = Brain::new(cfg, MoveMode::Stay, RuleSet::bundled(), clips, 11);
        let picks: Vec<String> = run(&mut b, secs, false)
            .into_iter()
            .filter_map(|a| match a {
                Action::React { clip, priority: 0 } => Some(clip),
                _ => None,
            })
            .collect();
        assert_eq!(b.mood(), Mood::Playful);
        picks
    }

    #[test]
    fn pose_cycle_is_weighted_flirty_and_never_repeats() {
        let flirty_share = |picks: &[String]| {
            picks.iter().filter(|c| crate::idle::is_flirty(c)).count() as f32 / picks.len() as f32
        };
        let flat = cycle_picks(1, 2000.0);
        let default = cycle_picks(crate::idle::DEFAULT_FLIRT_WEIGHT, 2000.0);
        let high = cycle_picks(8, 2000.0);
        for picks in [&flat, &default, &high] {
            assert!(picks.len() > 1500, "{}", picks.len());
            for w in picks.windows(2) {
                assert_ne!(w[0], w[1], "immediate repeat");
            }
        }
        // Every Playful variant still comes up (variety), flirty ones more.
        for v in cycle_variants(Mood::Playful) {
            assert!(default.iter().any(|c| c == v), "{v} never picked");
        }
        let (f, d, h) = (
            flirty_share(&flat),
            flirty_share(&default),
            flirty_share(&high),
        );
        assert!(f < d && d < h, "{f} {d} {h}");
        // Playful: 11 flirty of 16 → 11·3 / (11·3 + 5) ≈ 87 % at weight 3.
        assert!(d > 0.8, "{d}");
    }

    /// First pose-cycle react within `secs` of a wander walk ending.
    fn pose_after_arrival(chance: f32, secs: f32) -> Option<f32> {
        let mut cfg = calm_cfg();
        cfg.pose_cycle_s = 1e6;
        cfg.wander_every_s = (5.0, 5.0);
        cfg.wander_speed_px_s = 600.0;
        cfg.wander_pose_chance = chance;
        cfg.wander_pose_delay_s = 1.5;
        let mut b = brain_with(cfg, MoveMode::Wander);
        b.set_area(0.0, 1500.0);
        b.set_position(700.0);
        // Run until the first walk ends.
        let mut walked = false;
        for _ in 0..200 {
            let a = b.tick(0.1, false);
            walked |= a.iter().any(|a| matches!(a, Action::Walk { .. }));
            if walked && a.contains(&Action::Stand) {
                let t0 = b.time();
                for _ in 0..(secs * 10.0) as usize {
                    let a = b.tick(0.1, false);
                    if a.iter()
                        .any(|a| matches!(a, Action::React { priority: 0, .. }))
                    {
                        return Some(b.time() - t0);
                    }
                }
                return None;
            }
        }
        panic!("no walk ended");
    }

    #[test]
    fn wander_arrival_can_strike_a_pose() {
        let dt = pose_after_arrival(1.0, 5.0).expect("posed after arriving");
        assert!((1.4..=1.7).contains(&dt), "{dt}");
        assert_eq!(pose_after_arrival(0.0, 5.0), None);
        assert_eq!(BehaviorConfig::default().wander_pose_chance, 0.35);
    }

    #[test]
    fn tease_config_lives_under_behavior() {
        let c: BehaviorConfig =
            toml::from_str("[tease]\nenabled = false\npeak_scale = 1.3\n").unwrap();
        assert!(!c.tease.enabled);
        assert_eq!(c.tease.peak_scale, 1.3);
        assert_eq!(c.tease.beat_clips[0], "tease_closeup");
    }

    #[test]
    fn keys_map_poses_and_actions() {
        let mut b = brain();
        assert_eq!(
            reacts(&b.input(Input::Key(Key::Char('t')))),
            ["tail_wag"],
            "T dual_tail -> input.secret clip"
        );
        assert_eq!(
            reacts(&b.input(Input::Key(Key::Char('K')))),
            ["event_input_click_double"],
            "K hacking"
        );
        assert_eq!(reacts(&b.input(Input::Key(Key::Char('w')))), ["event_wifi"]);
        assert_eq!(reacts(&b.input(Input::Key(Key::Char('h')))), ["event_help"]);
        let a = b.input(Input::Key(Key::Char('m')));
        assert_eq!(a, [Action::Muted(true)]);
        assert!(!click(&mut b, Button::Left)
            .iter()
            .any(|a| matches!(a, Action::Sfx(_))));
        assert_eq!(reacts(&b.input(Input::Key(Key::Char('c')))).len(), 1);
        let r = b.input(Input::Key(Key::Char('r')));
        assert!(reacts(&r).iter().all(|c| !is_base_clip(c)) && reacts(&r).len() == 1);
        assert!(b.input(Input::Key(Key::Escape)).contains(&Action::Quit));
        assert!(b.input(Input::Key(Key::Char('q'))).contains(&Action::Quit));
        assert!(b.input(Input::Key(Key::Enter)).contains(&Action::OpenChat));
    }

    #[test]
    fn events_use_rule_priority_mood_and_cooldown() {
        let mut b = brain_with(
            BehaviorConfig {
                sleepy_after_s: 1e6,
                asleep_after_s: 2e6,
                ..BehaviorConfig::default()
            },
            MoveMode::Stay,
        );
        let a = b.event(&Event::new("cpu.high"));
        let rule = RuleSet::bundled().find("cpu.high").cloned().unwrap();
        assert_eq!(
            a.iter().find(|a| matches!(a, Action::React { .. })),
            Some(&Action::React {
                clip: "event_cpu_busy".into(),
                priority: rule.priority
            })
        );
        assert_eq!(b.mood(), Mood::Alert);
        if rule.cooldown_s > 0.0 {
            assert!(reacts(&b.event(&Event::new("cpu.high"))).is_empty());
            run(&mut b, rule.cooldown_s + 0.2, true);
            assert_eq!(
                reacts(&b.event(&Event::new("cpu.high"))),
                ["event_cpu_busy"]
            );
        }
        // Unknown events do nothing.
        assert!(b.event(&Event::new("no.such.event")).is_empty());
    }
}
