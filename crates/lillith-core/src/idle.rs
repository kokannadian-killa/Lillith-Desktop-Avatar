//! Idle-life scheduler (PLAN.md §3.1, layer 3).
//!
//! Pure timers driven by `tick(dt)`, so tests can use a fake clock:
//!
//! - **breathing**: a [`Breath`] wave, period re-picked each cycle in 3.5–4.5 s;
//! - **blink** every 4–6 s;
//! - **idle transition** every 8–12 s, lasting 1.2–2 s, picked from the
//!   transition clips that actually exist (hip_sway, lean_forward,
//!   shoulder_roll, …);
//! - **random idle reaction** every 12–26 s.
//!
//! While the avatar is busy (a reaction is playing, she is walking or being
//! dragged) transitions and idle reactions are held back; they fire as soon
//! as she is idle again. Blinks keep their rhythm but are only emitted when
//! idle.

use crate::rng::Rng;
use crate::spring::Breath;
use serde::{Deserialize, Serialize};

/// Inclusive `[min, max]` seconds.
pub type Range = (f32, f32);

/// Idle timing (README-SP defaults).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IdleConfig {
    pub breath_period_s: Range,
    pub blink_every_s: Range,
    pub transition_every_s: Range,
    pub transition_len_s: Range,
    pub reaction_every_s: Range,
}

impl Default for IdleConfig {
    fn default() -> Self {
        Self {
            breath_period_s: (3.5, 4.5),
            blink_every_s: (4.0, 6.0),
            transition_every_s: (8.0, 12.0),
            transition_len_s: (1.2, 2.0),
            reaction_every_s: (12.0, 26.0),
        }
    }
}

/// Default idle-transition clips, in preference order (physics refs A–D).
pub const DEFAULT_TRANSITIONS: &[&str] = &["hip_sway", "lean_forward", "shoulder_roll"];

/// Default random idle reactions (gestures that read well unprompted).
pub const DEFAULT_IDLE_REACTIONS: &[&str] =
    &["tail_wag", "think", "nod", "wave", "shrug", "knife_tap"];

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
    next_blink: f32,
    next_transition: f32,
    next_reaction: f32,
    last_transition: Option<usize>,
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
        Self {
            cfg,
            rng,
            breath,
            transitions,
            reactions,
            next_blink,
            next_transition,
            next_reaction,
            last_transition: None,
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

        if self.next_transition <= 0.0 && !self.transitions.is_empty() {
            let mut i = self.rng.index(self.transitions.len());
            // Don't repeat the same transition twice in a row.
            if self.transitions.len() > 1 && Some(i) == self.last_transition {
                i = (i + 1) % self.transitions.len();
            }
            self.last_transition = Some(i);
            let duration_s = self
                .rng
                .range(self.cfg.transition_len_s.0, self.cfg.transition_len_s.1);
            cues.push(IdleCue::Transition {
                clip: self.transitions[i].clone(),
                duration_s,
            });
            self.next_transition = duration_s
                + self
                    .rng
                    .range(self.cfg.transition_every_s.0, self.cfg.transition_every_s.1);
            // Keep idle reactions from landing on top of a transition.
            self.next_reaction = self.next_reaction.max(duration_s + 0.5);
        } else if self.next_reaction <= 0.0 && !self.reactions.is_empty() {
            let i = self.rng.index(self.reactions.len());
            cues.push(IdleCue::Reaction {
                clip: self.reactions[i].clone(),
            });
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
        IdleLife::with_available(IdleConfig::default(), seed, |_| true)
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
            IdleConfig::default(),
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
    fn deterministic_for_a_seed() {
        let a = run(&mut life(42), 60.0);
        let b = run(&mut life(42), 60.0);
        assert_eq!(a, b);
    }
}
