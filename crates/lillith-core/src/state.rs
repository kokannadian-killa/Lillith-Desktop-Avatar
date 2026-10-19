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
    /// Automatic pose cycle (README-SP `cycle_interval`, 16 s).
    pub pose_cycle_s: f32,
    /// A left click this soon after the knife waves instead.
    pub left_repeat_s: f32,
    /// Pause between wander walks, `[min, max]` s.
    pub wander_every_s: (f32, f32),
    pub wander_speed_px_s: f32,
    /// Shortest walk worth taking (px).
    pub wander_min_step_px: f32,
    /// Events at or above this priority wake her up.
    pub event_wake_priority: u8,
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            gestures: GestureConfig::default(),
            mischief: MischiefConfig::default(),
            mood_tau_s: 45.0,
            sleepy_after_s: 300.0,
            asleep_after_s: 900.0,
            pose_cycle_s: 16.0,
            left_repeat_s: 4.0,
            wander_every_s: (20.0, 45.0),
            wander_speed_px_s: 60.0,
            wander_min_step_px: 120.0,
            event_wake_priority: 80,
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

/// Idle variants for the pose cycle, per mood.
fn cycle_variants(mood: Mood) -> &'static [&'static str] {
    match mood {
        Mood::Playful | Mood::Happy => &[
            "hip_sway",
            "tail_wag",
            "lean_forward",
            "shoulder_roll",
            "nod",
            "wave",
        ],
        Mood::Mischievous => &["tail_wag", "knife_tap", "hip_sway", "dance"],
        Mood::Alert => &["lean_forward", "think", "shoulder_roll"],
        Mood::Sleepy => &["shoulder_roll", "lean_forward", "hip_sway"],
    }
}

/// Playful interruptions when she has been ignored.
const PRANKS: &[&str] = &["knife_tap", "dance", "tail_wag", "point"];
const PRANK_LINES: &[&str] = &[
    "Psst… pay attention to me~",
    "Bored. Entertain me.",
    "I could poke something, you know.",
];
/// Clips that are bases/helpers, never a random reaction.
fn is_base_clip(c: &str) -> bool {
    matches!(c, "idle" | "sleep" | "drag_held" | "blink_wink")
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
    cycle_idx: usize,
    next_wander: f32,
    x: f32,
    area: Option<(f32, f32)>,
    muted: bool,
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
            cycle_idx: 0,
            next_wander,
            x: 0.0,
            area: None,
            muted: false,
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

    // ── Setup from the overlay ───────────────────────────────────────────

    /// Range of x positions she may wander over (left margin, px).
    pub fn set_area(&mut self, min: f32, max: f32) {
        self.area = Some((min.min(max), max.max(min)));
    }

    /// Current x (after a drop or at start-up).
    pub fn set_position(&mut self, x: f32) {
        self.x = x;
    }

    pub fn set_mode(&mut self, mode: MoveMode) -> Vec<Action> {
        self.apply_mode(mode);
        self.take()
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
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
            "user.returned" | "user.activity" => {
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
        if let Phase::Walking { target, .. } = self.phase {
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
        if let Some(line) = rule.line_for(&Event::new(event)) {
            if !line.is_empty() {
                self.out.push(Action::Say(line));
            }
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
        if let Some(clip) = self.resolve(event) {
            self.out.push(Action::React {
                clip,
                priority: priority.unwrap_or(rule.priority),
            });
        }
        let line = match ev {
            Some(e) => rule.line_for(e),
            None => rule.line_for(&Event::new(event)),
        };
        if let Some(line) = line.filter(|l| !l.trim().is_empty()) {
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

    fn cycle_pose(&mut self) {
        self.next_cycle = self.t + self.cfg.pose_cycle_s;
        let variants: Vec<&str> = cycle_variants(self.mood.mood())
            .iter()
            .copied()
            .filter(|c| self.has(c))
            .collect();
        if variants.is_empty() {
            return;
        }
        let clip = variants[self.cycle_idx % variants.len()].to_string();
        self.cycle_idx += 1;
        self.out.push(Action::React { clip, priority: 0 });
        self.sfx("soft");
    }

    fn prank(&mut self) {
        let pool: Vec<&str> = PRANKS.iter().copied().filter(|c| self.has(c)).collect();
        if pool.is_empty() || self.phase != Phase::Idle {
            return;
        }
        let clip = pool[self.rng.index(pool.len())].to_string();
        let line = PRANK_LINES[self.rng.index(PRANK_LINES.len())].to_string();
        self.out.push(Action::React {
            clip,
            priority: PRIO_PRANK,
        });
        self.out.push(Action::Say(line));
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
        assert_eq!(
            d.pose_cycle_s,
            crate::config::Config::default().pose_cycle_s
        );
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
        assert_eq!(b.phase(), Phase::Idle);
        assert!(a.contains(&Action::Stand));
        assert_eq!(b.x(), 333.0);
    }

    #[test]
    fn pose_cycle_every_16s_when_idle() {
        let mut b = brain_with(
            BehaviorConfig {
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
        assert_eq!(
            clips,
            ["hip_sway", "tail_wag", "lean_forward", "shoulder_roll"]
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
