//! Idle-life scheduler (PLAN.md §3.1, layer 3).
//!
//! Pure timers driven by `tick(dt)`, so tests can use a fake clock:
//!
//! - **breathing**: a [`Breath`] wave, period re-picked each cycle in 3.5–4.5 s;
//! - **blink** every 4–6 s;
//! - **idle transition** every 8–15 s, lasting 1.2–2 s, picked from the
//!   transition clips that actually exist (hip_sway, idle_flirt_hip_lean,
//!   shoulder_shimmy, …);
//! - **random idle reaction** every 15–30 s (idle_flirt_*, hip_pop_wink,
//!   lean_tease, blow_kiss, hair_flip, …).
//!
//! Both pools are weighted [`ShuffleBag`]s: the seductive clips come up
//! about three times as often as the neutral ones, and the same clip never
//! plays twice in a row.
//!
//! All of it is configurable in `[behavior.idle]` of config.toml.
//!
//! While the avatar is busy (a reaction is playing, she is walking or being
//! dragged) transitions and idle reactions are held back; they fire as soon
//! as she is idle again. Blinks keep their rhythm but are only emitted when
//! idle.

use crate::bag::ShuffleBag;
use crate::rng::Rng;
use crate::spring::Breath;
use serde::{Deserialize, Serialize};

/// Inclusive `[min, max]` seconds.
pub type Range = (f32, f32);

/// Idle timing (`[behavior.idle]`). Defaults are calm: she mostly stands
/// and breathes, with an occasional sway or gesture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IdleConfig {
    pub breath_period_s: Range,
    pub blink_every_s: Range,
    /// Pause between idle transitions (hip sway, lean, shoulder roll).
    pub transition_every_s: Range,
    pub transition_len_s: Range,
    /// Pause between unprompted idle gestures (tail wag, think, nod, …).
    pub reaction_every_s: Range,
    /// Minimum time on one idle take (`variant_of:idle` clips) before the
    /// animator may switch to another at a loop end. 0 = every loop.
    /// With [`Self::variant_play_once`] it is the time spent on the clean
    /// base (idle, walk_*, sit_swing) between two variant passes.
    pub variant_hold_s: f32,
    /// `true` (default): a `variant_of:<base>` clip plays once and hands
    /// back to its base clip, which then loops for `variant_hold_s` before
    /// the next variant; covers every tagged base (idle, walk_right/left,
    /// sit_swing). `false`: the old behaviour, idle only, where one idle
    /// variant loops for the whole hold and the clean idle is never drawn.
    pub variant_play_once: bool,
    /// Like `variant_hold_s` but for the walk bases (walk_left/right):
    /// how long she walks the plain gait before a sultry take (saunter,
    /// sassy, strut) plays. Short, so walks look sultry almost at once.
    pub walk_variant_hold_s: f32,
    /// Shuffle-bag weight of the flirty clips (idle transitions, idle
    /// gestures and the brain's pose cycle); neutral clips weigh 1. 3 (the
    /// default) = a flirty clip comes up about three times as often as a
    /// neutral one; 1 = all equal. Semi-flirty clips get about half.
    pub flirt_weight: u32,
}

impl Default for IdleConfig {
    fn default() -> Self {
        Self {
            breath_period_s: (3.5, 4.5),
            blink_every_s: (4.0, 6.0),
            transition_every_s: (8.0, 15.0),
            transition_len_s: (1.2, 2.0),
            reaction_every_s: (15.0, 30.0),
            variant_hold_s: 12.0,
            variant_play_once: true,
            walk_variant_hold_s: 1.0,
            flirt_weight: DEFAULT_FLIRT_WEIGHT,
        }
    }
}

impl IdleConfig {
    /// The pre-0.x busy timings (transition every 8–12 s, gesture every
    /// 12–26 s, new idle take every loop). Used by tests of the scheduler
    /// mechanics; set them in `[behavior.idle]` to get the old feel back.
    pub fn lively() -> Self {
        Self {
            transition_every_s: (8.0, 12.0),
            reaction_every_s: (12.0, 26.0),
            variant_hold_s: 0.0,
            ..Self::default()
        }
    }
}

/// Default idle-transition clips (short loops played for 1.2–2 s) with
/// their shuffle-bag weight: sultry sways 3, neutral ones 1.
pub const WEIGHTED_TRANSITIONS: &[(&str, u32)] = &[
    ("hip_sway", 3),
    ("idle_flirt_hip_lean", 3),
    ("idle_flirt_hands_behind_sway", 3),
    ("shoulder_shimmy", 3),
    ("sway_dance", 2),
    ("idle_flirt_tail_curl", 3),
    ("lean_forward", 1),
    ("shoulder_roll", 1),
];

/// Default random idle reactions (played once) with weights: the
/// seductive takes 3, the playful neutral ones 1.
pub const WEIGHTED_IDLE_REACTIONS: &[(&str, u32)] = &[
    ("idle_flirt_lip_bite", 3),
    ("idle_flirt_glove_adjust", 3),
    ("idle_flirt_stretch", 3),
    ("idle_flirt_pigtail_twirl", 3),
    ("idle_flirt_look_away", 3),
    ("idle_flirt_foot_tap", 3),
    ("hip_pop_wink", 3),
    ("lean_tease", 3),
    ("tail_tease", 3),
    ("blow_kiss", 3),
    ("stretch_arch", 3),
    ("hair_flip", 3),
    ("hair_twirl", 3),
    ("over_shoulder_look", 3),
    ("blow_kiss_hip_pop", 3),
    ("bend_forward_wink", 2),
    ("smug_smirk", 2),
    ("dom_beckon_finger", 3),
    ("kiss_then_come_here", 3),
    ("tail_wrap_pull", 2),
    ("hug_screen_possessive", 2),
    ("hip_sway_slow", 3),
    ("kneel_sit_back_beckon", 3),
    ("dom_throne_lean", 3),
    ("come_hither_crawl", 2),
    ("taskbar_lounge_lie", 2),
    ("body_roll_slow", 2),
    ("cheeky_tongue_wink", 2),
    ("idle_flirt_shy_giggle", 2),
    ("idle_tail_fidget", 1),
    ("idle_nail_check", 1),
    ("tail_wag", 1),
    ("nod", 1),
    ("wave", 1),
];

/// Names of [`WEIGHTED_TRANSITIONS`].
pub const DEFAULT_TRANSITIONS: &[&str] = &[
    "hip_sway",
    "idle_flirt_hip_lean",
    "idle_flirt_hands_behind_sway",
    "shoulder_shimmy",
    "sway_dance",
    "idle_flirt_tail_curl",
    "lean_forward",
    "shoulder_roll",
];

/// Names of [`WEIGHTED_IDLE_REACTIONS`].
pub const DEFAULT_IDLE_REACTIONS: &[&str] = &[
    "idle_flirt_lip_bite",
    "idle_flirt_glove_adjust",
    "idle_flirt_stretch",
    "idle_flirt_pigtail_twirl",
    "idle_flirt_look_away",
    "idle_flirt_foot_tap",
    "hip_pop_wink",
    "lean_tease",
    "tail_tease",
    "blow_kiss",
    "stretch_arch",
    "hair_flip",
    "hair_twirl",
    "over_shoulder_look",
    "blow_kiss_hip_pop",
    "bend_forward_wink",
    "smug_smirk",
    "dom_beckon_finger",
    "kiss_then_come_here",
    "tail_wrap_pull",
    "hug_screen_possessive",
    "hip_sway_slow",
    "kneel_sit_back_beckon",
    "dom_throne_lean",
    "come_hither_crawl",
    "taskbar_lounge_lie",
    "body_roll_slow",
    "cheeky_tongue_wink",
    "idle_flirt_shy_giggle",
    "idle_tail_fidget",
    "idle_nail_check",
    "tail_wag",
    "nod",
    "wave",
];

/// Default [`IdleConfig::flirt_weight`].
pub const DEFAULT_FLIRT_WEIGHT: u32 = 3;

/// Flirty / seductive clips (tasteful, outfit intact): weighted by
/// [`IdleConfig::flirt_weight`] wherever idle variety is picked.
pub const FLIRTY_CLIPS: &[&str] = &[
    "hip_sway",
    "shoulder_shimmy",
    "sway_dance",
    "hip_pop_wink",
    "lean_tease",
    "tail_tease",
    "blow_kiss",
    "blow_kiss_hip_pop",
    "stretch_arch",
    "hair_flip",
    "hair_twirl",
    "over_shoulder_look",
    "bend_forward_wink",
    "smug_smirk",
    "dom_beckon_finger",
    "kiss_then_come_here",
    "tail_wrap_pull",
    "hug_screen_possessive",
    "hungry_lip_lick",
    "predator_lean_in",
    "seduce_blow_kiss_screen",
    "hip_sway_slow",
    "kneel_sit_back_beckon",
    "dom_throne_lean",
    "come_hither_crawl",
    "taskbar_lounge_lie",
    "body_roll_slow",
    "cheeky_tongue_wink",
    "heart_hands",
    "tease_closeup",
];

/// `true` for the flirty clips (every `idle_flirt_*` mannerism and
/// [`FLIRTY_CLIPS`]).
pub fn is_flirty(name: &str) -> bool {
    name.starts_with("idle_flirt_") || FLIRTY_CLIPS.contains(&name)
}

/// Scales a table weight (3 = flirty, 2 = semi-flirty, 1 = neutral) by
/// `flirt_weight` (relative to the default 3). Never below 1.
pub fn scaled_weight(table_weight: u32, flirt_weight: u32) -> u32 {
    if table_weight <= 1 {
        return 1;
    }
    let w = (table_weight * flirt_weight + DEFAULT_FLIRT_WEIGHT / 2) / DEFAULT_FLIRT_WEIGHT;
    w.max(1)
}

/// Shuffle-bag weight of `name` for a free-form pool (e.g. the pose
/// cycle): `flirt_weight` for flirty clips, 1 otherwise.
pub fn flirt_weight_of(name: &str, flirt_weight: u32) -> u32 {
    if is_flirty(name) {
        flirt_weight.max(1)
    } else {
        1
    }
}

fn weight_of(table: &[(&str, u32)], name: &str, flirt_weight: u32) -> u32 {
    let w = table.iter().find(|(n, _)| *n == name).map_or(1, |e| e.1);
    scaled_weight(w, flirt_weight)
}

/// Something the idle scheduler wants the animator to do now.
#[derive(Debug, Clone, PartialEq)]
pub enum IdleCue {
    Blink,
    /// Play this clip for `duration_s`, then return to the base clip.
    Transition {
        clip: String,
        duration_s: f32,
    },
    /// Play this clip once as a light reaction.
    Reaction {
        clip: String,
    },
}

/// Idle-life timers.
#[derive(Debug, Clone)]
pub struct IdleLife {
    cfg: IdleConfig,
    rng: Rng,
    breath: Breath,
    transitions: Vec<String>,
    reactions: Vec<String>,
    transition_bag: ShuffleBag,
    reaction_bag: ShuffleBag,
    next_blink: f32,
    next_transition: f32,
    next_reaction: f32,
    busy: bool,
    time: f32,
}

impl IdleLife {
    /// `transitions` / `reactions` are the clips available in the loaded
    /// manifest; empty lists simply disable those cues.
    pub fn new(
        cfg: IdleConfig,
        seed: u64,
        transitions: Vec<String>,
        reactions: Vec<String>,
    ) -> Self {
        let mut rng = Rng::new(seed);
        let breath = Breath::new(rng.range(cfg.breath_period_s.0, cfg.breath_period_s.1));
        let next_blink = rng.range(cfg.blink_every_s.0, cfg.blink_every_s.1);
        let next_transition = rng.range(cfg.transition_every_s.0, cfg.transition_every_s.1);
        let next_reaction = rng.range(cfg.reaction_every_s.0, cfg.reaction_every_s.1);
        let transition_bag = ShuffleBag::new(transitions.iter().map(|n| {
            (
                n.clone(),
                weight_of(WEIGHTED_TRANSITIONS, n, cfg.flirt_weight),
            )
        }));
        let reaction_bag = ShuffleBag::new(reactions.iter().map(|n| {
            (
                n.clone(),
                weight_of(WEIGHTED_IDLE_REACTIONS, n, cfg.flirt_weight),
            )
        }));
        Self {
            cfg,
            rng,
            breath,
            transitions,
            reactions,
            transition_bag,
            reaction_bag,
            next_blink,
            next_transition,
            next_reaction,
            busy: false,
            time: 0.0,
        }
    }

    /// Filters the default transition/reaction names down to what `has_clip`
    /// says exists.
    pub fn with_available(cfg: IdleConfig, seed: u64, has_clip: impl Fn(&str) -> bool) -> Self {
        let pick = |names: &[&str]| -> Vec<String> {
            names
                .iter()
                .filter(|n| has_clip(n))
                .map(|n| n.to_string())
                .collect()
        };
        Self::new(
            cfg,
            seed,
            pick(DEFAULT_TRANSITIONS),
            pick(DEFAULT_IDLE_REACTIONS),
        )
    }

    pub fn transitions(&self) -> &[String] {
        &self.transitions
    }

    pub fn reactions(&self) -> &[String] {
        &self.reactions
    }

    /// Seconds since creation (fake clock).
    pub fn time(&self) -> f32 {
        self.time
    }

    /// Breath value in `[-1, 1]`.
    pub fn breath(&self) -> f32 {
        self.breath.value()
    }

    pub fn breath_period(&self) -> f32 {
        self.breath.period_s
    }

    /// Marks the avatar busy/idle (busy holds back transitions and reactions).
    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }

    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// Advances the clock and returns the cues due now (at most one of each).
    pub fn tick(&mut self, dt: f32) -> Vec<IdleCue> {
        self.time += dt;
        if self.breath.advance(dt) {
            self.breath.period_s = self
                .rng
                .range(self.cfg.breath_period_s.0, self.cfg.breath_period_s.1);
        }
        let mut cues = Vec::new();

        self.next_blink -= dt;
        if self.next_blink <= 0.0 {
            self.next_blink += self
                .rng
                .range(self.cfg.blink_every_s.0, self.cfg.blink_every_s.1);
            self.next_blink = self.next_blink.max(0.05);
            if !self.busy {
                cues.push(IdleCue::Blink);
            }
        }

        // Held timers stay at <= 0 until she is idle again.
        self.next_transition -= dt;
        self.next_reaction -= dt;
        if self.busy {
            return cues;
        }

        if self.next_transition <= 0.0 && !self.transition_bag.is_empty() {
            // Weighted shuffle bag: never the same transition twice in a row.
            let clip = self
                .transition_bag
                .next(&mut self.rng)
                .unwrap_or_default()
                .to_string();
            let duration_s = self
                .rng
                .range(self.cfg.transition_len_s.0, self.cfg.transition_len_s.1);
            cues.push(IdleCue::Transition { clip, duration_s });
            self.next_transition = duration_s
                + self
                    .rng
                    .range(self.cfg.transition_every_s.0, self.cfg.transition_every_s.1);
            // Keep idle reactions from landing on top of a transition.
            self.next_reaction = self.next_reaction.max(duration_s + 0.5);
        } else if self.next_reaction <= 0.0 && !self.reaction_bag.is_empty() {
            let clip = self
                .reaction_bag
                .next(&mut self.rng)
                .unwrap_or_default()
                .to_string();
            cues.push(IdleCue::Reaction { clip });
            self.next_reaction = self
                .rng
                .range(self.cfg.reaction_every_s.0, self.cfg.reaction_every_s.1);
            self.next_transition = self.next_transition.max(2.0);
        }
        cues
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn life(seed: u64) -> IdleLife {
        IdleLife::with_available(IdleConfig::lively(), seed, |_| true)
    }

    /// Runs `secs` at 30 Hz, returning (time, cue) pairs.
    fn run(l: &mut IdleLife, secs: f32) -> Vec<(f32, IdleCue)> {
        let dt = 1.0 / 30.0;
        let mut out = Vec::new();
        for _ in 0..(secs / dt) as usize {
            for c in l.tick(dt) {
                out.push((l.time(), c));
            }
        }
        out
    }

    #[test]
    fn blink_intervals_in_range() {
        for seed in 1..20 {
            let mut l = IdleLife::new(IdleConfig::default(), seed, vec![], vec![]);
            let blinks: Vec<f32> = run(&mut l, 120.0)
                .into_iter()
                .filter(|(_, c)| *c == IdleCue::Blink)
                .map(|(t, _)| t)
                .collect();
            assert!(blinks.len() >= 19, "seed {seed}: {} blinks", blinks.len());
            assert!(blinks[0] <= 6.05);
            for w in blinks.windows(2) {
                let d = w[1] - w[0];
                assert!((3.95..=6.05).contains(&d), "seed {seed}: gap {d}");
            }
        }
    }

    #[test]
    fn transitions_use_available_clips_and_timing() {
        let mut l = IdleLife::new(
            IdleConfig::lively(),
            3,
            vec!["hip_sway".into(), "lean_forward".into()],
            vec![],
        );
        let ts: Vec<(f32, String, f32)> = run(&mut l, 120.0)
            .into_iter()
            .filter_map(|(t, c)| match c {
                IdleCue::Transition { clip, duration_s } => Some((t, clip, duration_s)),
                _ => None,
            })
            .collect();
        assert!(ts.len() >= 8, "{} transitions", ts.len());
        assert!(ts[0].0 <= 12.05);
        for w in ts.windows(2) {
            let gap = w[1].0 - w[0].0;
            let len = w[0].2;
            assert!((1.2..=2.0).contains(&len));
            assert!(
                gap >= 8.0 + len - 0.05 && gap <= 12.0 + len + 0.05,
                "gap {gap}"
            );
            assert_ne!(w[0].1, w[1].1, "no immediate repeats");
            assert!(w[1].1 == "hip_sway" || w[1].1 == "lean_forward");
        }
    }

    #[test]
    fn missing_clips_are_skipped() {
        let l = IdleLife::with_available(IdleConfig::default(), 1, |n| n == "shoulder_roll");
        assert_eq!(l.transitions(), ["shoulder_roll".to_string()]);
        assert!(l.reactions().is_empty());
    }

    #[test]
    fn busy_holds_back_then_fires() {
        let mut l = life(5);
        l.set_busy(true);
        let cues = run(&mut l, 40.0);
        assert!(cues.is_empty(), "nothing while busy: {cues:?}");
        l.set_busy(false);
        let cues = l.tick(1.0 / 30.0);
        assert!(
            cues.iter()
                .any(|c| matches!(c, IdleCue::Transition { .. } | IdleCue::Reaction { .. })),
            "overdue cue fires once idle: {cues:?}"
        );
    }

    #[test]
    fn idle_reactions_happen() {
        let mut l = life(11);
        let n = run(&mut l, 180.0)
            .into_iter()
            .filter(|(_, c)| matches!(c, IdleCue::Reaction { .. }))
            .count();
        assert!(n >= 3, "{n} idle reactions in 3 min");
    }

    #[test]
    fn breathing_period_stays_in_range() {
        let mut l = life(9);
        let dt = 1.0 / 30.0;
        let mut prev = l.breath();
        let mut crossings = 0;
        for _ in 0..(60.0 / dt) as usize {
            l.tick(dt);
            assert!((3.5..=4.5).contains(&l.breath_period()));
            let b = l.breath();
            assert!((-1.0..=1.0).contains(&b));
            if prev < 0.0 && b >= 0.0 {
                crossings += 1;
            }
            prev = b;
        }
        // 60 s / 3.5–4.5 s per cycle.
        assert!((13..=18).contains(&crossings), "{crossings} cycles");
    }

    #[test]
    fn variant_play_once_defaults_true_and_parses() {
        assert!(IdleConfig::default().variant_play_once);
        assert!(IdleConfig::lively().variant_play_once);
        // Missing key in [behavior.idle] keeps the default.
        let c: IdleConfig = toml::from_str("variant_hold_s = 12.0").unwrap();
        assert!(c.variant_play_once);
        assert_eq!(c.variant_hold_s, 12.0);
        let c: IdleConfig = toml::from_str("variant_play_once = false").unwrap();
        assert!(!c.variant_play_once);
        assert_eq!(c.variant_hold_s, 12.0);
    }

    #[test]
    fn lively_flirty_defaults_rotate_without_repeats() {
        let d = IdleConfig::default();
        assert_eq!(d.transition_every_s, (8.0, 15.0));
        assert_eq!(d.reaction_every_s, (15.0, 30.0));
        assert_eq!(d.variant_hold_s, 12.0);
        assert!(d.variant_play_once);
        // Breathing and blinks keep their subtle rhythm.
        assert_eq!(d.breath_period_s, (3.5, 4.5));
        assert_eq!(d.blink_every_s, (4.0, 6.0));
        for seed in 1..10 {
            let mut l = IdleLife::with_available(d.clone(), seed, |_| true);
            let cues = run(&mut l, 180.0);
            let moves = cues
                .iter()
                .filter(|(_, c)| !matches!(c, IdleCue::Blink))
                .count();
            let blinks = cues.len() - moves;
            assert!(blinks >= 29, "seed {seed}: {blinks} blinks");
            // 3 min: transitions every 8–15 s (+len) and gestures every
            // 15–30 s, interleaved: plenty of variety, never frantic.
            assert!((10..=32).contains(&moves), "seed {seed}: {moves} moves");
            let first = cues
                .iter()
                .find(|(_, c)| !matches!(c, IdleCue::Blink))
                .map(|(t, _)| *t)
                .unwrap();
            assert!(first >= 7.9, "seed {seed}: first move at {first}");
            // No clip twice in a row, per kind and overall.
            let clips: Vec<&str> = cues
                .iter()
                .filter_map(|(_, c)| match c {
                    IdleCue::Transition { clip, .. } | IdleCue::Reaction { clip } => {
                        Some(clip.as_str())
                    }
                    IdleCue::Blink => None,
                })
                .collect();
            for w in clips.windows(2) {
                assert_ne!(w[0], w[1], "seed {seed}: {clips:?}");
            }
        }
    }

    #[test]
    fn reactions_never_repeat_immediately_and_favour_flirty_clips() {
        let mut l = IdleLife::with_available(IdleConfig::lively(), 77, |_| true);
        let rs: Vec<String> = run(&mut l, 3600.0)
            .into_iter()
            .filter_map(|(_, c)| match c {
                IdleCue::Reaction { clip } => Some(clip),
                _ => None,
            })
            .collect();
        assert!(rs.len() > 60, "{} reactions", rs.len());
        for w in rs.windows(2) {
            assert_ne!(w[0], w[1], "immediate repeat");
        }
        let flirty = rs.iter().filter(|c| is_flirty(c)).count();
        assert!(flirty * 10 >= rs.len() * 7, "{flirty}/{} flirty", rs.len());
        // The foot tap is in the pool.
        assert!(rs.iter().any(|c| c == "idle_flirt_foot_tap"));
    }

    #[test]
    fn every_weighted_flirty_clip_is_marked_flirty() {
        for (n, w) in WEIGHTED_TRANSITIONS.iter().chain(WEIGHTED_IDLE_REACTIONS) {
            assert_eq!(*w >= 2, is_flirty(n), "{n} (weight {w})");
        }
        assert!(is_flirty("idle_flirt_anything") && !is_flirty("nod"));
    }

    #[test]
    fn flirt_weight_scales_the_flirty_clips_only() {
        assert_eq!(IdleConfig::default().flirt_weight, 3);
        // Default 3: table weights as written.
        assert_eq!((scaled_weight(3, 3), scaled_weight(2, 3)), (3, 2));
        assert_eq!(scaled_weight(1, 3), 1);
        // 6: twice as flirty; 1: all equal; 0 never drops a clip.
        assert_eq!((scaled_weight(3, 6), scaled_weight(2, 6)), (6, 4));
        assert_eq!((scaled_weight(3, 1), scaled_weight(2, 1)), (1, 1));
        assert_eq!((scaled_weight(3, 0), scaled_weight(1, 0)), (1, 1));
        assert_eq!(flirt_weight_of("lean_tease", 5), 5);
        assert_eq!(flirt_weight_of("nod", 5), 1);
        assert_eq!(flirt_weight_of("lean_tease", 0), 1);
        let c: IdleConfig = toml::from_str("flirt_weight = 5\n").unwrap();
        assert_eq!(c.flirt_weight, 5);
    }

    fn flirty_share(flirt_weight: u32) -> f32 {
        let cfg = IdleConfig {
            flirt_weight,
            ..IdleConfig::lively()
        };
        let mut l = IdleLife::with_available(cfg, 5, |_| true);
        let rs: Vec<String> = run(&mut l, 7200.0)
            .into_iter()
            .filter_map(|(_, c)| match c {
                IdleCue::Reaction { clip } => Some(clip),
                _ => None,
            })
            .collect();
        for w in rs.windows(2) {
            assert_ne!(w[0], w[1], "immediate repeat at weight {flirt_weight}");
        }
        rs.iter().filter(|c| is_flirty(c)).count() as f32 / rs.len() as f32
    }

    #[test]
    fn higher_flirt_weight_means_more_flirty_reactions() {
        let (flat, default, high) = (flirty_share(1), flirty_share(3), flirty_share(8));
        assert!(flat < default && default < high, "{flat} {default} {high}");
        assert!(high > 0.85, "{high}");
    }

    #[test]
    fn deterministic_for_a_seed() {
        let a = run(&mut life(42), 60.0);
        let b = run(&mut life(42), 60.0);
        assert_eq!(a, b);
    }
}
