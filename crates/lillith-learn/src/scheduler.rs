//! When a step may run (§1): a pure state machine with an injected clock.
//!
//! ```text
//! Off ── enabled ──▶ Waiting ── idle + budgets + duty ok ──▶ Running
//!  ▲                   ▲  ▲                                     │
//!  └── disabled        │  └──────────── step done ──────────────┤
//!                      │                                        │ activity
//!                Paused ◀── load / battery / fullscreen ────────┘
//!                         (a safeword / stop drops the step → Waiting)
//! ```
//!
//! - idle: no activity for `idle_after_s` (when `pause_on_activity`);
//! - load: 1-min load average ≤ `max_load`; battery / fullscreen pauses;
//! - duty: after a step of length t she rests ≥ t·(1/max_duty − 1)
//!   (9·t at 10 %), and busy time in the last 10 minutes stays ≤ max_duty;
//! - budgets: `max_steps_per_hour`, `max_tokens_per_day`;
//! - safeword (`kill_switch::hush_epoch()` changed or a `chat.safeword` event):
//!   the running step is dropped and nothing of it is kept. There is no
//!   rest period after it any more (`learn.safeword_cooldown_min` is not
//!   read); only the normal duty rest for the dropped step applies.

use lillith_core::config::LearnConfig;
use std::collections::VecDeque;

/// Rolling duty window.
pub const DUTY_WINDOW_S: f64 = 600.0;

/// The configured limits.
#[derive(Debug, Clone, PartialEq)]
pub struct Limits {
    pub enabled: bool,
    pub idle_after_s: f64,
    pub max_duty: f64,
    pub max_load: f32,
    pub pause_on_activity: bool,
    pub pause_on_battery: bool,
    pub pause_on_fullscreen: bool,
    pub max_tokens_per_day: u32,
    pub max_steps_per_hour: u32,
}

impl From<&LearnConfig> for Limits {
    fn from(c: &LearnConfig) -> Self {
        Self {
            enabled: c.enabled,
            idle_after_s: f64::from(c.idle_after_s.max(0.0)),
            max_duty: f64::from(c.max_duty).clamp(0.01, 1.0),
            max_load: c.max_load,
            pause_on_activity: c.pause_on_activity,
            pause_on_battery: c.pause_on_battery,
            pause_on_fullscreen: c.pause_on_fullscreen,
            max_tokens_per_day: c.max_tokens_per_day,
            max_steps_per_hour: c.max_steps_per_hour,
        }
    }
}

/// Learner state (shown in logs; the panel comes in P5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Off,
    Waiting,
    Running,
    Paused,
}

/// Why no step may start now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitReason {
    NotIdle,
    Load,
    Battery,
    Fullscreen,
    Duty,
    StepBudget,
    TokenBudget,
}

/// What [`Scheduler::tick`] decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Off,
    Wait(WaitReason),
    Run,
}

/// What the sensors say right now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Inputs {
    /// Seconds (any monotonic origin).
    pub now: f64,
    /// Seconds since the last activity.
    pub idle_s: f64,
    pub load1: Option<f32>,
    pub on_battery: bool,
    pub fullscreen: bool,
    /// `kill_switch::hush_epoch()` (the safeword or a stop).
    pub kill_epoch: u64,
    /// A `chat.safeword` event arrived since the last tick.
    pub safeword: bool,
}

impl Inputs {
    /// Idle, mains, no load, no safeword (tests start from this).
    pub fn calm(now: f64) -> Self {
        Self {
            now,
            idle_s: 1e9,
            load1: Some(0.1),
            on_battery: false,
            fullscreen: false,
            kill_epoch: 0,
            safeword: false,
        }
    }
}

/// The scheduler.
#[derive(Debug, Clone)]
pub struct Scheduler {
    limits: Limits,
    state: State,
    epoch: Option<u64>,
    next_allowed: f64,
    run_started: Option<f64>,
    /// (start, end) of recent steps.
    busy: VecDeque<(f64, f64)>,
    /// Start times of steps in the last hour.
    steps: VecDeque<f64>,
    day: i64,
    tokens_today: u32,
}

impl Scheduler {
    pub fn new(limits: Limits) -> Self {
        let state = if limits.enabled {
            State::Waiting
        } else {
            State::Off
        };
        Self {
            limits,
            state,
            epoch: None,
            next_allowed: f64::NEG_INFINITY,
            run_started: None,
            busy: VecDeque::new(),
            steps: VecDeque::new(),
            day: i64::MIN,
            tokens_today: 0,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Tokens spent today (UTC day of `now`).
    pub fn tokens_today(&self) -> u32 {
        self.tokens_today
    }

    /// `true` when the safeword fired (epoch changed or the event) — also
    /// used while a step runs, to drop it.
    pub fn safeword_fired(&self, i: &Inputs) -> bool {
        i.safeword || self.epoch.is_some_and(|e| e != i.kill_epoch)
    }

    fn roll(&mut self, now: f64) {
        while self.busy.front().is_some_and(|b| b.1 < now - DUTY_WINDOW_S) {
            self.busy.pop_front();
        }
        while self.steps.front().is_some_and(|s| *s < now - 3600.0) {
            self.steps.pop_front();
        }
        let day = (now / 86_400.0).floor() as i64;
        if day != self.day {
            self.day = day;
            self.tokens_today = 0;
        }
    }

    /// Busy seconds inside `[now − window, now]`.
    pub fn busy_in_window(&self, now: f64) -> f64 {
        let from = now - DUTY_WINDOW_S;
        self.busy
            .iter()
            .map(|(s, e)| (e.min(now) - s.max(from)).max(0.0))
            .sum()
    }

    /// Decides whether a step may start now (does not start it).
    pub fn tick(&mut self, i: &Inputs) -> Decision {
        if !self.limits.enabled {
            self.state = State::Off;
            return Decision::Off;
        }
        self.epoch = Some(i.kill_epoch);
        self.roll(i.now);
        if self.state == State::Running {
            // Only `begin`/`finish`/`abort` move out of Running.
            return Decision::Wait(WaitReason::Duty);
        }
        let pause = if self.limits.pause_on_battery && i.on_battery {
            Some(WaitReason::Battery)
        } else if self.limits.pause_on_fullscreen && i.fullscreen {
            Some(WaitReason::Fullscreen)
        } else if i.load1.is_some_and(|l| l > self.limits.max_load) {
            Some(WaitReason::Load)
        } else {
            None
        };
        if let Some(r) = pause {
            self.state = State::Paused;
            return Decision::Wait(r);
        }
        if self.limits.pause_on_activity && i.idle_s < self.limits.idle_after_s {
            if self.state != State::Paused {
                self.state = State::Waiting;
            }
            return Decision::Wait(WaitReason::NotIdle);
        }
        self.state = State::Waiting;
        if self.steps.len() as u32 >= self.limits.max_steps_per_hour {
            return Decision::Wait(WaitReason::StepBudget);
        }
        if self.tokens_today >= self.limits.max_tokens_per_day {
            return Decision::Wait(WaitReason::TokenBudget);
        }
        if i.now < self.next_allowed
            || self.busy_in_window(i.now) >= self.limits.max_duty * DUTY_WINDOW_S
        {
            return Decision::Wait(WaitReason::Duty);
        }
        Decision::Run
    }

    /// A step starts.
    pub fn begin(&mut self, now: f64) {
        self.state = State::Running;
        self.run_started = Some(now);
        self.steps.push_back(now);
    }

    fn record(&mut self, now: f64) {
        let start = self.run_started.take().unwrap_or(now);
        let t = (now - start).max(0.0);
        self.busy.push_back((start, now));
        // Rest ≥ t·(1/d − 1): 9·t at 10 %.
        self.next_allowed = now + t * (1.0 / self.limits.max_duty - 1.0);
    }

    /// The step finished (or failed) after spending `tokens`.
    pub fn finish(&mut self, now: f64, tokens: u32) {
        self.record(now);
        self.roll(now);
        self.tokens_today = self.tokens_today.saturating_add(tokens);
        self.state = State::Waiting;
    }

    /// The step was dropped: by the safeword / a stop (→ Waiting, with only
    /// the normal duty rest; no safeword rest period) or by activity / chat
    /// (→ Paused until idle again).
    pub fn abort(&mut self, now: f64, safeword: bool) {
        self.record(now);
        self.state = if safeword {
            State::Waiting
        } else {
            State::Paused
        };
    }

    /// No step before `now + secs` (the model server isn't there).
    pub fn backoff(&mut self, now: f64, secs: f64) {
        self.next_allowed = self.next_allowed.max(now + secs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sched() -> Scheduler {
        Scheduler::new(Limits::from(&LearnConfig::default()))
    }

    #[test]
    fn transitions() {
        let mut off = Scheduler::new(Limits::from(&LearnConfig {
            enabled: false,
            ..LearnConfig::default()
        }));
        assert_eq!(off.state(), State::Off);
        assert_eq!(off.tick(&Inputs::calm(0.0)), Decision::Off);

        let mut s = sched();
        assert_eq!(s.state(), State::Waiting);
        // Not idle long enough.
        let mut i = Inputs::calm(1000.0);
        i.idle_s = 10.0;
        assert_eq!(s.tick(&i), Decision::Wait(WaitReason::NotIdle));
        assert_eq!(s.state(), State::Waiting);
        // Idle → Run → Running → finish → Waiting.
        i.idle_s = 301.0;
        assert_eq!(s.tick(&i), Decision::Run);
        s.begin(1000.0);
        assert_eq!(s.state(), State::Running);
        assert_ne!(
            s.tick(&Inputs::calm(1001.0)),
            Decision::Run,
            "one at a time"
        );
        s.finish(1010.0, 100);
        assert_eq!(s.state(), State::Waiting);
        // Duty: 10 s step → 90 s rest.
        assert_eq!(
            s.tick(&Inputs::calm(1050.0)),
            Decision::Wait(WaitReason::Duty)
        );
        assert_eq!(s.tick(&Inputs::calm(1101.0)), Decision::Run);
        // Battery / fullscreen / load → Paused.
        let mut b = Inputs::calm(1200.0);
        b.on_battery = true;
        assert_eq!(s.tick(&b), Decision::Wait(WaitReason::Battery));
        assert_eq!(s.state(), State::Paused);
        b.on_battery = false;
        b.fullscreen = true;
        assert_eq!(s.tick(&b), Decision::Wait(WaitReason::Fullscreen));
        b.fullscreen = false;
        b.load1 = Some(2.0);
        assert_eq!(s.tick(&b), Decision::Wait(WaitReason::Load));
        assert_eq!(s.state(), State::Paused);
        b.load1 = Some(0.5);
        assert_eq!(s.tick(&b), Decision::Run);
        assert_eq!(s.state(), State::Waiting);
        // Activity mid-step → Paused, stays Paused until idle again.
        s.begin(1200.0);
        s.abort(1201.0, false);
        assert_eq!(s.state(), State::Paused);
        let mut busy = Inputs::calm(1300.0);
        busy.idle_s = 5.0;
        assert_eq!(s.tick(&busy), Decision::Wait(WaitReason::NotIdle));
        assert_eq!(s.state(), State::Paused);
        assert_eq!(s.tick(&Inputs::calm(1400.0)), Decision::Run);
    }

    #[test]
    fn safeword_drops_the_step_but_never_rests_her() {
        let mut s = sched();
        assert_eq!(s.tick(&Inputs::calm(0.0)), Decision::Run);
        s.begin(0.0);
        // kill_switch epoch changes while running → the step is dropped.
        let mut i = Inputs::calm(1.0);
        i.kill_epoch = 1;
        assert!(s.safeword_fired(&i));
        s.abort(1.0, true);
        assert_eq!(s.state(), State::Waiting, "no safeword cooldown");
        // Only the duty rest for the 1 s step (9 s at 10 %), not an hour.
        let mut later = Inputs::calm(11.0);
        later.kill_epoch = 1;
        assert_eq!(s.tick(&later), Decision::Run);
        // The chat.safeword event alone does not hold her back either,
        // even with safeword_cooldown_min set (that key is no longer read).
        let mut s = Scheduler::new(Limits::from(&LearnConfig {
            safeword_cooldown_min: 60.0,
            ..LearnConfig::default()
        }));
        let mut ev = Inputs::calm(100.0);
        ev.safeword = true;
        assert!(s.safeword_fired(&ev));
        assert_eq!(s.tick(&ev), Decision::Run);
        assert_eq!(s.state(), State::Waiting);
    }

    #[test]
    fn budgets() {
        let mut s = sched();
        let mut t = 0.0;
        for _ in 0..12 {
            assert_eq!(s.tick(&Inputs::calm(t)), Decision::Run);
            s.begin(t);
            s.finish(t + 1.0, 10);
            t += 11.0;
        }
        assert_eq!(
            s.tick(&Inputs::calm(t)),
            Decision::Wait(WaitReason::StepBudget)
        );
        assert_eq!(s.tick(&Inputs::calm(3601.0)), Decision::Run);
        let mut s = sched();
        s.begin(0.0);
        s.finish(1.0, 40_000);
        assert_eq!(
            s.tick(&Inputs::calm(100.0)),
            Decision::Wait(WaitReason::TokenBudget)
        );
        // A new UTC day resets the token budget.
        assert_eq!(s.tick(&Inputs::calm(86_400.0)), Decision::Run);
    }

    #[test]
    fn duty_stays_at_or_below_ten_percent() {
        let mut s = Scheduler::new(Limits {
            max_steps_per_hour: 10_000,
            ..Limits::from(&LearnConfig::default())
        });
        // Steps of varying length (≤ 60 s, the request timeout), ticks every
        // second for 6 simulated hours.
        let lens = [3.0, 17.0, 45.0, 8.0, 60.0, 1.0, 29.0];
        let (mut busy_total, mut k) = (0.0, 0usize);
        let mut t = 0.0;
        let end = 6.0 * 3600.0;
        let mut worst_window: f64 = 0.0;
        let mut last = 0.0;
        while t < end {
            if s.tick(&Inputs::calm(t)) == Decision::Run {
                let len = lens[k % lens.len()];
                k += 1;
                s.begin(t);
                t += len;
                busy_total += len;
                last = len;
                s.finish(t, 0);
                worst_window = worst_window.max(s.busy_in_window(t));
            } else {
                t += 1.0;
            }
        }
        assert!(k > 20, "it did run ({k} steps)");
        // Count the rest the last step is owed (9·t) as part of the span.
        let duty = busy_total / (t + 9.0 * last);
        assert!(duty <= 0.10 + 1e-9, "duty {duty}");
        assert!(
            worst_window <= 0.10 * DUTY_WINDOW_S + 60.0 + 1e-9,
            "{worst_window}"
        );
        // And every rest was at least 9× the step before it.
        let mut s = sched();
        s.begin(0.0);
        s.finish(20.0, 0);
        assert_eq!(
            s.tick(&Inputs::calm(199.0)),
            Decision::Wait(WaitReason::Duty)
        );
        assert_eq!(s.tick(&Inputs::calm(200.0)), Decision::Run);
    }
}
