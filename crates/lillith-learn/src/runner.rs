//! The background learner thread the overlay starts next to the memory
//! thread (its own SQLite connection). Started only when
//! `[learn] enabled` **and** `[memory] enabled`; otherwise nothing is
//! opened or created.
//!
//! The thread lowers its own priority ([`crate::hygiene`]), feeds the
//! [`Scheduler`] from the desktop events it is shown ([`LearnHandle::observe`])
//! plus load / battery readings, and runs at most one step at a time. The
//! safeword (`kill_switch::hush_epoch()` moving, or a `chat.safeword` event)
//! drops a running step at once (nothing kept); there is no rest after it.

use crate::learner::{Learner, Outcome};
use crate::scheduler::{Decision, Inputs, Limits, Scheduler, State};
use crate::summarize::{LlmSummarizer, SumError, Summarizer};
use lillith_core::access::kill_switch;
use lillith_core::config::Config;
use lillith_core::Event;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The overlay's safeword event.
pub const SAFEWORD_EVENT: &str = "chat.safeword";
/// How often the scheduler is ticked.
const TICK: Duration = Duration::from_secs(2);
/// Rest after the model server wasn't there.
const UNAVAILABLE_BACKOFF_S: f64 = 600.0;

/// Desktop readings the scheduler needs besides activity.
pub trait Sensors: Send {
    fn load1(&self) -> Option<f32>;
    fn on_battery(&self) -> bool;
    /// Stub in P1 (presence already counts idle inhibitors, e.g. fullscreen
    /// video, as activity).
    fn fullscreen(&self) -> bool {
        false
    }
}

/// `/proc/loadavg` and `/sys/class/power_supply` (Linux); nothing elsewhere.
#[derive(Debug, Default, Clone, Copy)]
pub struct SysSensors;

impl Sensors for SysSensors {
    fn load1(&self) -> Option<f32> {
        std::fs::read_to_string("/proc/loadavg")
            .ok()?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    }

    fn on_battery(&self) -> bool {
        let Ok(rd) = std::fs::read_dir("/sys/class/power_supply") else {
            return false;
        };
        let read = |p: std::path::PathBuf| {
            std::fs::read_to_string(p)
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        };
        let (mut ac_online, mut discharging) = (false, false);
        for e in rd.flatten() {
            let d = e.path();
            match read(d.join("type")).as_str() {
                "Mains" | "USB" => ac_online |= read(d.join("online")) == "1",
                "Battery" => discharging |= read(d.join("status")) == "Discharging",
                _ => {}
            }
        }
        discharging && !ac_online
    }
}

/// `true` for events that mean Ko is at the desktop.
pub fn is_activity(ev: &Event) -> bool {
    ev.name.starts_with("user.") || ev.name.starts_with("chat.") || ev.name.starts_with("pointer.")
}

#[derive(Debug)]
struct Shared {
    start: Instant,
    last_activity_ms: AtomicU64,
    safeword: AtomicBool,
    quit: AtomicBool,
    state: AtomicU8,
}

impl Shared {
    fn now_s(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn last_activity_s(&self) -> f64 {
        self.last_activity_ms.load(Ordering::SeqCst) as f64 / 1000.0
    }
}

fn state_code(s: State) -> u8 {
    match s {
        State::Off => 0,
        State::Waiting => 1,
        State::Running => 2,
        State::Paused => 3,
    }
}

fn state_of(c: u8) -> State {
    match c {
        1 => State::Waiting,
        2 => State::Running,
        3 => State::Paused,
        _ => State::Off,
    }
}

/// The running learner. Dropping it stops the thread (a running step is
/// dropped within the poll interval).
#[derive(Debug)]
pub struct LearnHandle {
    shared: Arc<Shared>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl LearnHandle {
    /// Starts the learner for `cfg` when [`crate::should_run`] and
    /// `llm_allowed` (not `--no-llm`). `None`: nothing started, nothing
    /// opened or created.
    pub fn spawn(cfg: &Config, llm_allowed: bool) -> Option<Self> {
        if !crate::should_run(cfg) || !llm_allowed {
            return None;
        }
        let gate = lillith_core::llm_gate::LlmGate::shared()?;
        let chat = cfg.chat.clone();
        Self::spawn_with(
            cfg,
            Box::new(SysSensors),
            Box::new(move || {
                LlmSummarizer::new(&chat, gate)
                    .map(|s| Box::new(s) as Box<dyn Summarizer>)
                    .map_err(|e| format!("{e:?}"))
            }),
        )
    }

    /// [`spawn`](Self::spawn) with injected sensors and summarizer (tests).
    #[allow(clippy::type_complexity)]
    pub fn spawn_with(
        cfg: &Config,
        sensors: Box<dyn Sensors>,
        make_summarizer: Box<dyn FnOnce() -> Result<Box<dyn Summarizer>, String> + Send>,
    ) -> Option<Self> {
        if !crate::should_run(cfg) {
            return None;
        }
        let shared = Arc::new(Shared {
            start: Instant::now(),
            last_activity_ms: AtomicU64::new(0),
            safeword: AtomicBool::new(false),
            quit: AtomicBool::new(false),
            state: AtomicU8::new(state_code(State::Waiting)),
        });
        let cfg = cfg.clone();
        let sh = shared.clone();
        let join = std::thread::Builder::new()
            .name("lillith-learn".into())
            .spawn(move || run(cfg, sh, sensors, make_summarizer))
            .map_err(|e| tracing::warn!("learn: thread: {e}"))
            .ok()?;
        Some(Self {
            shared,
            join: Some(join),
        })
    }

    /// Offers a desktop / chat event (never blocks).
    pub fn observe(&self, ev: &Event) {
        if ev.name == SAFEWORD_EVENT {
            self.shared.safeword.store(true, Ordering::SeqCst);
        }
        if is_activity(ev) {
            let ms = (self.shared.now_s() * 1000.0) as u64;
            self.shared.last_activity_ms.store(ms, Ordering::SeqCst);
        }
    }

    pub fn state(&self) -> State {
        state_of(self.shared.state.load(Ordering::SeqCst))
    }

    /// Stops the thread and waits for it.
    pub fn shutdown(&mut self) {
        self.shared.quit.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for LearnHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run(
    cfg: Config,
    sh: Arc<Shared>,
    sensors: Box<dyn Sensors>,
    make_summarizer: Box<dyn FnOnce() -> Result<Box<dyn Summarizer>, String> + Send>,
) {
    let how = crate::hygiene::lower_current_thread();
    let brain = match lillith_memory::Brain::open(&cfg.memory) {
        Ok(b) if b.is_enabled() => b,
        Ok(_) => return,
        Err(e) => {
            tracing::warn!("learn: memory db: {e}; learning off");
            return;
        }
    };
    let mut sum = match make_summarizer() {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("learn: model: {e}; learning off");
            return;
        }
    };
    let mut learner = Learner::new(&cfg, brain);
    run_loop(&cfg, &sh, sensors.as_ref(), sum.as_mut(), &mut learner, how);
    sh.state.store(state_code(State::Off), Ordering::SeqCst);
}

fn run_loop(
    cfg: &Config,
    sh: &Shared,
    sensors: &dyn Sensors,
    sum: &mut dyn Summarizer,
    learner: &mut Learner,
    how: &str,
) {
    let mut sched = Scheduler::new(Limits::from(&cfg.learn));
    tracing::info!(priority = how, "learn: idle learner ready");
    while !sh.quit.load(Ordering::SeqCst) {
        let now = sh.now_s();
        let inputs = Inputs {
            now,
            idle_s: now - sh.last_activity_s(),
            load1: sensors.load1(),
            on_battery: sensors.on_battery(),
            fullscreen: sensors.fullscreen(),
            kill_epoch: kill_switch::hush_epoch(),
            safeword: sh.safeword.swap(false, Ordering::SeqCst),
        };
        if sched.tick(&inputs) == Decision::Run {
            sched.begin(now);
            sh.state.store(state_code(State::Running), Ordering::SeqCst);
            let epoch0 = inputs.kill_epoch;
            let act0 = sh.last_activity_ms.load(Ordering::SeqCst);
            let pause_on_activity = cfg.learn.pause_on_activity;
            let safeword =
                || sh.safeword.load(Ordering::SeqCst) || kill_switch::hushed_since(epoch0);
            let stop = || {
                sh.quit.load(Ordering::SeqCst)
                    || safeword()
                    || (pause_on_activity && sh.last_activity_ms.load(Ordering::SeqCst) != act0)
            };
            let rep = learner.step(sum, &stop);
            let end = sh.now_s();
            if safeword() {
                // Dropped: nothing of the step is kept. No rest period after
                // the safeword; she waits for the next idle window as usual.
                sh.safeword.store(false, Ordering::SeqCst);
                sched.abort(end, true);
                tracing::info!("learn: safeword, step dropped");
            } else {
                match &rep.outcome {
                    Outcome::Discarded(
                        SumError::Stopped | SumError::Preempted | SumError::Busy,
                    ) => sched.abort(end, false),
                    Outcome::Discarded(SumError::Unavailable(_)) => {
                        sched.finish(end, rep.tokens);
                        sched.backoff(end, UNAVAILABLE_BACKOFF_S);
                    }
                    Outcome::NothingToRead => {
                        sched.finish(end, 0);
                        sched.backoff(end, 3600.0);
                    }
                    _ => sched.finish(end, rep.tokens),
                }
                tracing::debug!(
                    source = rep.source.as_deref().unwrap_or("-"),
                    outcome = ?rep.outcome,
                    notes = rep.written.len(),
                    "learn: step"
                );
            }
        }
        sh.state.store(state_code(sched.state()), Ordering::SeqCst);
        // Sleep in short slices so quit is seen quickly.
        let until = Instant::now() + TICK;
        while Instant::now() < until && !sh.quit.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_and_safeword_events() {
        assert!(is_activity(&Event::new("user.activity")));
        assert!(is_activity(&Event::new("user.returned")));
        assert!(is_activity(&Event::new("chat.open")));
        assert!(!is_activity(&Event::new("app.focus")));
    }

    #[test]
    fn memory_off_starts_nothing_and_creates_nothing() {
        let d = std::env::temp_dir().join(format!("lillith-runner-off-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let mut cfg = Config::default();
        cfg.memory.enabled = false;
        cfg.memory.db_path = d.join("memory.db").display().to_string();
        cfg.mind.knowledge_dir = d.join("knowledge").display().to_string();
        assert!(cfg.learn.enabled, "[learn] stays on by default");
        assert!(LearnHandle::spawn(&cfg, true).is_none());
        assert!(LearnHandle::spawn_with(
            &cfg,
            Box::new(SysSensors),
            Box::new(|| Err("never called".into()))
        )
        .is_none());
        assert!(!d.exists(), "no db, no knowledge folder, nothing");
        // Learning off with memory on: same.
        cfg.memory.enabled = true;
        cfg.learn.enabled = false;
        assert!(LearnHandle::spawn(&cfg, true).is_none());
        assert!(!d.exists());
    }
}
