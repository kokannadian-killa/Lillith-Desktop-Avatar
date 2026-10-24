//! [`Voice`]: the handle the app owns (feature `voice`). Lines pass a
//! [`Gate`] (cooldown + same-line debounce) on the caller's thread, then go
//! into a bounded queue drained by ONE speech worker thread that runs the
//! [`Engine`]. Nothing here blocks the caller (never the render thread).
//!
//! Policy:
//! - Queue holds at most `[voice] queue` lines; when full the OLDEST waiting
//!   line is dropped (newest news wins).
//! - [`Urgency::Interrupt`] clears the queue and cuts the current line.
//! - [`Voice::set_muted`] cuts the current line, clears the queue, and drops
//!   everything until unmuted (runtime toggle; the menu's "Mute voice").
//! - With no engine ([`NullEngine`]) no thread is spawned; every line is
//!   dropped as [`Dropped::NoEngine`] (logged once at start-up).
//! - [`Voice::set_enabled`] is the live "Speak out loud (speakers)" switch
//!   (`[voice] enabled`): off cuts the current line and drops every line as
//!   [`Dropped::Disabled`]; on speaks again without a restart (the worker
//!   runs whenever a real engine exists). Text on screen is not affected.
//! - [`Voice::say_reply`] speaks a whole chat reply (cuts banter, skips the
//!   cooldown, longer limit than banter lines).
//! - Daemon re-probe: when `[voice] engine` is `auto` / `daemon` but the
//!   voice daemon did not answer at start-up, a small prober thread pings
//!   it again on every say (at most once per [`REPROBE_MIN_GAP`]) and every
//!   [`REPROBE_PERIOD`]. Once it answers, the worker switches to the cloned
//!   voice ([`crate::engine::DaemonEngine`]) before its next line (never
//!   mid-line), the switch is logged once and the fallback warning clears.
//!   The ping never runs on the caller's thread.

use crate::config::{EngineChoice, VoiceConfig};
use crate::daemon::{DaemonClient, PingInfo};
use crate::engine::{Engine, Playback, SpeakOpts};
use crate::lines::{DefaultLines, LineSource};
use lillith_core::state::Action;
use lillith_core::{Event, Mood};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How a line enters the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Urgency {
    /// Wait in line (drop-oldest when full); respects the cooldown.
    #[default]
    Normal,
    /// Cut whatever is playing, clear the queue, speak now; skips the
    /// global cooldown (not the same-line repeat guard).
    Interrupt,
}

/// Why a line was not queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dropped {
    Disabled,
    Muted,
    NoEngine,
    Empty,
    /// Another line was spoken less than `cooldown_s` ago.
    Cooldown,
    /// The same line was spoken less than `repeat_cooldown_s` ago.
    Repeat,
}

/// Anti-chatter gate (pure; time in seconds is passed in).
#[derive(Debug, Clone)]
pub struct Gate {
    pub cooldown_s: f64,
    pub repeat_cooldown_s: f64,
    last_any: Option<f64>,
    last_line: HashMap<String, f64>,
}

impl Gate {
    pub fn new(cooldown_s: f32, repeat_cooldown_s: f32) -> Self {
        Self {
            cooldown_s: cooldown_s as f64,
            repeat_cooldown_s: repeat_cooldown_s as f64,
            last_any: None,
            last_line: HashMap::new(),
        }
    }

    /// Lets `line` through at `now` (and records it) or says why not.
    pub fn admit(&mut self, line: &str, now: f64, urgency: Urgency) -> Result<(), Dropped> {
        let key = line.trim().to_lowercase();
        if let Some(&t) = self.last_line.get(&key) {
            if now - t < self.repeat_cooldown_s {
                return Err(Dropped::Repeat);
            }
        }
        if urgency == Urgency::Normal {
            if let Some(t) = self.last_any {
                if now - t < self.cooldown_s {
                    return Err(Dropped::Cooldown);
                }
            }
        }
        self.last_any = Some(now);
        // Forget lines older than the repeat window (bounded map).
        let rc = self.repeat_cooldown_s;
        self.last_line.retain(|_, t| now - *t < rc);
        self.last_line.insert(key, now);
        Ok(())
    }
}

/// Collapses whitespace, drops control chars, cuts at a word boundary.
pub fn clean_line(text: &str, max_chars: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut out = String::new();
    for w in words {
        let w: String = w.chars().filter(|c| !c.is_control()).collect();
        if w.is_empty() {
            continue;
        }
        let extra = usize::from(!out.is_empty()) + w.chars().count();
        if out.chars().count() + extra > max_chars {
            if out.is_empty() {
                out = w.chars().take(max_chars).collect();
            }
            out.push('…');
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&w);
    }
    out
}

/// Counters for the status page / tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub queued: u64,
    pub spoken: u64,
    /// Waiting lines dropped because the queue was full.
    pub dropped_full: u64,
    /// Lines cut while playing (interrupt / mute / shutdown).
    pub cut: u64,
    pub engine_errors: u64,
}

struct State {
    queue: VecDeque<String>,
    cap: usize,
    interrupt: bool,
    shutdown: bool,
    stats: Stats,
    busy: bool,
}

struct Shared {
    state: Mutex<State>,
    cv: Condvar,
    muted: AtomicBool,
    /// `[voice] enabled`, switchable live ([`Voice::set_enabled`]).
    enabled: AtomicBool,
    /// An engine the worker takes over before its next line (re-probe).
    pending: Mutex<Option<Box<dyn Engine>>>,
    /// What [`Voice::engine_name`] / [`Voice::voice_warning`] report.
    info: Mutex<EngineInfo>,
}

#[derive(Debug, Clone)]
struct EngineInfo {
    name: &'static str,
    warning: Option<String>,
}

/// A daemon probe the prober thread can own ([`crate::engine::ping_daemon`],
/// or a fake in tests).
pub type SharedProbe = Arc<dyn Fn(&VoiceConfig) -> Option<(DaemonClient, PingInfo)> + Send + Sync>;

/// A say re-checks the daemon at most this often while on the fallback.
pub const REPROBE_MIN_GAP: Duration = Duration::from_secs(10);
/// Background re-check period while on the fallback.
pub const REPROBE_PERIOD: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy)]
struct ReprobeTiming {
    min_gap: Duration,
    period: Duration,
}

impl Default for ReprobeTiming {
    fn default() -> Self {
        Self {
            min_gap: REPROBE_MIN_GAP,
            period: REPROBE_PERIOD,
        }
    }
}

#[derive(Default)]
struct Reprobe {
    state: Mutex<ReprobeState>,
    cv: Condvar,
}

#[derive(Default)]
struct ReprobeState {
    kicked: bool,
    stop: bool,
}

impl Shared {
    /// `true` while lines must not be heard (muted or speakers off).
    fn silenced(&self) -> bool {
        self.muted.load(Ordering::SeqCst) || !self.enabled.load(Ordering::SeqCst)
    }
}

/// The voice (see the module docs). Dropping it stops the worker.
pub struct Voice {
    cfg: VoiceConfig,
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    /// The daemon prober while on the fallback (`None`: not re-probing).
    reprobe: Option<Arc<Reprobe>>,
    gate: Gate,
    lines: Box<dyn LineSource>,
    t0: Instant,
}

impl std::fmt::Debug for Voice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Voice")
            .field("engine", &self.engine_name())
            .field("muted", &self.muted())
            .finish()
    }
}

/// Poll interval of the worker while a line plays.
const POLL: Duration = Duration::from_millis(20);

/// Longest chat reply read aloud in one go (characters; banter lines use
/// `[voice] max_chars`).
pub const REPLY_MAX_CHARS: usize = 1200;

impl Voice {
    /// Detects the engine for `cfg` (logs the choice once) with the
    /// [`DefaultLines`] table.
    pub fn from_config(cfg: VoiceConfig) -> Self {
        Self::from_config_with(
            cfg,
            &crate::engine::which,
            Arc::new(crate::engine::ping_daemon),
            ReprobeTiming::default(),
        )
    }

    fn from_config_with(
        cfg: VoiceConfig,
        lookup: &dyn Fn(&str) -> Option<std::path::PathBuf>,
        probe: SharedProbe,
        timing: ReprobeTiming,
    ) -> Self {
        // Probe even when the speakers are off so the live toggle can switch
        // them on later without a restart; `enabled` stays as configured.
        let probe_cfg = VoiceConfig {
            enabled: true,
            ..cfg.clone()
        };
        let d = crate::engine::detect_full(&probe_cfg, lookup, &*probe);
        if d.engine.is_real() && !cfg.enabled {
            tracing::info!("voice: {} (speakers off until toggled on)", d.reason);
        } else if d.engine.is_real() {
            tracing::info!("voice: {}", d.reason);
        } else {
            tracing::info!("voice off: {}", d.reason);
        }
        let on_fallback = d.engine.name() != "voiced";
        let mut v = Self::with_engine(cfg, d.engine, Box::new(DefaultLines::default()));
        if on_fallback && matches!(probe_cfg.engine, EngineChoice::Auto | EngineChoice::Daemon) {
            v.start_reprobe(probe_cfg, probe, timing);
        }
        v
    }

    /// Starts the daemon prober (see the module docs). The current engine
    /// is the fallback; its warning says so until the daemon answers.
    fn start_reprobe(&mut self, probe_cfg: VoiceConfig, probe: SharedProbe, timing: ReprobeTiming) {
        if let Ok(mut i) = self.shared.info.lock() {
            if i.warning.is_none() {
                i.warning = Some(if i.name == "none" {
                    "voice daemon not running; she stays silent until it starts".into()
                } else {
                    format!(
                        "voice daemon not running; using the {} voice until it starts",
                        i.name
                    )
                });
            }
        }
        let rp = Arc::new(Reprobe::default());
        let (sh, r) = (Arc::clone(&self.shared), Arc::clone(&rp));
        match std::thread::Builder::new()
            .name("lillith-voice-reprobe".into())
            .spawn(move || prober(&sh, &r, &probe_cfg, &*probe, timing))
        {
            Ok(_) => self.reprobe = Some(rp),
            Err(e) => tracing::warn!("voice daemon re-probe thread: {e}"),
        }
    }

    /// Asks the prober for a re-check (rate-limited there; never blocks).
    fn kick_reprobe(&self) {
        if let Some(rp) = &self.reprobe {
            if let Ok(mut st) = rp.state.lock() {
                st.kicked = true;
            }
            rp.cv.notify_all();
        }
    }

    /// With no worker yet (the fallback was silent), a daemon engine found
    /// by the prober gets one now.
    fn adopt_pending(&mut self) {
        if self.worker.is_some() {
            return;
        }
        let Some(engine) = self.shared.pending.lock().ok().and_then(|mut p| p.take()) else {
            return;
        };
        self.worker = Some(spawn_worker(&self.shared, engine, &self.cfg));
    }

    /// Explicit engine + line source (tests, custom engines).
    pub fn with_engine(
        cfg: VoiceConfig,
        engine: Box<dyn Engine>,
        lines: Box<dyn LineSource>,
    ) -> Self {
        let mut cfg = cfg;
        cfg.sanitize();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                queue: VecDeque::new(),
                cap: cfg.queue,
                interrupt: false,
                shutdown: false,
                stats: Stats::default(),
                busy: false,
            }),
            cv: Condvar::new(),
            muted: AtomicBool::new(cfg.mute),
            enabled: AtomicBool::new(cfg.enabled),
            pending: Mutex::new(None),
            info: Mutex::new(EngineInfo {
                name: engine.name(),
                warning: engine.voice_warning(),
            }),
        });
        // Spawned even while disabled so the speakers switch works live.
        let worker = engine
            .is_real()
            .then(|| spawn_worker(&shared, engine, &cfg));
        Self {
            gate: Gate::new(cfg.cooldown_s, cfg.repeat_cooldown_s),
            cfg,
            shared,
            worker,
            reprobe: None,
            lines,
            t0: Instant::now(),
        }
    }

    /// The engine in use (`"voiced"` once a late daemon was picked up).
    pub fn engine_name(&self) -> &'static str {
        self.shared.info.lock().map(|i| i.name).unwrap_or("none")
    }
    /// Degraded-voice notice for the UI, e.g. "using fallback voice p1
    /// (cloned 'lillith' voice unavailable)" or "voice daemon not running;
    /// using the espeak-ng voice until it starts". `None` when all is well.
    pub fn voice_warning(&self) -> Option<String> {
        self.shared.info.lock().ok().and_then(|i| i.warning.clone())
    }
    /// `true` if lines can actually be heard (enabled + a real engine).
    pub fn is_active(&self) -> bool {
        self.has_engine() && self.enabled()
    }
    /// `true` if a real TTS engine was found (the speakers switch can work).
    pub fn has_engine(&self) -> bool {
        self.worker.is_some()
            || self
                .shared
                .pending
                .lock()
                .map(|p| p.is_some())
                .unwrap_or(false)
    }
    /// "Speak out loud (speakers)": `[voice] enabled`, live.
    pub fn enabled(&self) -> bool {
        self.shared.enabled.load(Ordering::SeqCst)
    }

    /// Live speakers switch. Off cuts the current line and clears the
    /// queue; text elsewhere (speech bubble, chat) is unaffected.
    pub fn set_enabled(&mut self, on: bool) {
        self.adopt_pending();
        self.cfg.enabled = on;
        self.shared.enabled.store(on, Ordering::SeqCst);
        if !on {
            if let Ok(mut s) = self.shared.state.lock() {
                s.queue.clear();
                s.interrupt = true;
            }
        }
        self.shared.cv.notify_all();
    }
    pub fn config(&self) -> &VoiceConfig {
        &self.cfg
    }
    pub fn muted(&self) -> bool {
        self.shared.muted.load(Ordering::SeqCst)
    }
    pub fn stats(&self) -> Stats {
        self.shared
            .state
            .lock()
            .map(|s| s.stats)
            .unwrap_or_default()
    }
    /// `true` while a line is playing or waiting.
    pub fn speaking(&self) -> bool {
        self.shared
            .state
            .lock()
            .map(|s| s.busy || !s.queue.is_empty())
            .unwrap_or(false)
    }

    /// Runtime mute: cuts the current line and clears the queue.
    pub fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::SeqCst);
        if muted {
            if let Ok(mut s) = self.shared.state.lock() {
                s.queue.clear();
            }
        }
        self.shared.cv.notify_all();
    }

    /// Stops the current line and clears the queue (stays unmuted).
    pub fn stop(&self) {
        if let Ok(mut s) = self.shared.state.lock() {
            s.queue.clear();
            s.interrupt = true;
        }
        self.shared.cv.notify_all();
    }

    /// Speaks `text` (e.g. an `Action::Say` line).
    pub fn say(&mut self, text: &str, urgency: Urgency) -> Result<(), Dropped> {
        let now = self.t0.elapsed().as_secs_f64();
        self.say_at(text, urgency, now)
    }

    /// [`Voice::say`] at an explicit clock value (seconds; tests).
    pub fn say_at(&mut self, text: &str, urgency: Urgency, now: f64) -> Result<(), Dropped> {
        let max = self.cfg.max_chars;
        self.enqueue(text, urgency, now, max)
    }

    /// Reads a finished chat reply aloud (`chat.end`): cuts whatever plays,
    /// skips the cooldown, and allows up to [`REPLY_MAX_CHARS`]. Nothing
    /// happens while the speakers are off or muted.
    pub fn say_reply(&mut self, text: &str) -> Result<(), Dropped> {
        let now = self.t0.elapsed().as_secs_f64();
        self.say_reply_at(text, now)
    }

    /// [`Voice::say_reply`] at an explicit clock value (tests).
    pub fn say_reply_at(&mut self, text: &str, now: f64) -> Result<(), Dropped> {
        let max = self.cfg.max_chars.max(REPLY_MAX_CHARS);
        self.enqueue(text, Urgency::Interrupt, now, max)
    }

    fn enqueue(
        &mut self,
        text: &str,
        urgency: Urgency,
        now: f64,
        max_chars: usize,
    ) -> Result<(), Dropped> {
        // On the fallback: the prober re-checks the daemon in the
        // background; a switch applies to later lines.
        self.kick_reprobe();
        self.adopt_pending();
        if !self.enabled() {
            return Err(Dropped::Disabled);
        }
        if self.muted() {
            return Err(Dropped::Muted);
        }
        if self.worker.is_none() {
            return Err(Dropped::NoEngine);
        }
        // The bubble keeps her emoji / emoticons; the voice must not read
        // them out ("smiling face with horns", "colon parenthesis"). Applies
        // to banter lines and chat replies alike.
        let line = clean_line(&crate::speech_text::speech_text(text), max_chars);
        if line.is_empty() {
            return Err(Dropped::Empty);
        }
        self.gate.admit(&line, now, urgency)?;
        let Ok(mut s) = self.shared.state.lock() else {
            return Err(Dropped::NoEngine);
        };
        if urgency == Urgency::Interrupt {
            s.queue.clear();
            s.interrupt = true;
        }
        while s.queue.len() >= s.cap {
            s.queue.pop_front();
            s.stats.dropped_full += 1;
        }
        s.queue.push_back(line);
        s.stats.queued += 1;
        drop(s);
        self.shared.cv.notify_all();
        Ok(())
    }

    /// Speaks for a bus event. A rule line in `actions` (`Action::Say`, from
    /// `Brain::event`) wins; otherwise the [`LineSource`] is asked. Returns
    /// the line that was queued, if any.
    pub fn on_event(&mut self, event: &Event, mood: Mood, actions: &[Action]) -> Option<String> {
        let now = self.t0.elapsed().as_secs_f64();
        self.on_event_at(event, mood, actions, now)
    }

    /// [`Voice::on_event`] at an explicit clock value (tests).
    pub fn on_event_at(
        &mut self,
        event: &Event,
        mood: Mood,
        actions: &[Action],
        now: f64,
    ) -> Option<String> {
        let rule_line = actions.iter().find_map(|a| match a {
            Action::Say(l) => Some(l.clone()),
            _ => None,
        });
        let line = rule_line.or_else(|| self.lines.line_for(event, mood))?;
        self.say_at(&line, Urgency::Normal, now).ok().map(|_| line)
    }

    /// Applies the voice-relevant brain actions: `Say` → [`Voice::say`],
    /// `Muted(m)` → [`Voice::set_muted`]. Call it with every action batch
    /// the overlay applies (input, tick and event batches alike).
    pub fn on_actions(&mut self, actions: &[Action]) {
        for a in actions {
            match a {
                Action::Say(l) => {
                    let _ = self.say(l, Urgency::Normal);
                }
                Action::Muted(m) => self.set_muted(*m),
                _ => {}
            }
        }
    }
}

impl Drop for Voice {
    fn drop(&mut self) {
        if let Some(rp) = &self.reprobe {
            if let Ok(mut st) = rp.state.lock() {
                st.stop = true;
            }
            rp.cv.notify_all();
        }
        if let Ok(mut s) = self.shared.state.lock() {
            s.shutdown = true;
            s.queue.clear();
        }
        self.shared.cv.notify_all();
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

fn spawn_worker(
    shared: &Arc<Shared>,
    engine: Box<dyn Engine>,
    cfg: &VoiceConfig,
) -> JoinHandle<()> {
    let sh = Arc::clone(shared);
    let opts = SpeakOpts::from_config(cfg);
    std::thread::Builder::new()
        .name("lillith-voice".into())
        .spawn(move || worker(sh, engine, opts))
        .expect("spawn voice worker")
}

/// The prober thread: waits for a kick (a say; ignored within
/// `min_gap` of the last probe) or the `period` timer, pings, and on the
/// first answer stages a [`crate::engine::DaemonEngine`] and exits. The
/// start-up probe counts as the first one.
fn prober(
    sh: &Shared,
    rp: &Reprobe,
    cfg: &VoiceConfig,
    probe: &(dyn Fn(&VoiceConfig) -> Option<(DaemonClient, PingInfo)> + Send + Sync),
    t: ReprobeTiming,
) {
    let mut last = Instant::now();
    loop {
        {
            let Ok(mut st) = rp.state.lock() else { return };
            loop {
                if st.stop {
                    return;
                }
                let now = Instant::now();
                let kicked = std::mem::take(&mut st.kicked);
                if kicked && now >= last + t.min_gap {
                    break;
                }
                let Some(wait) = (last + t.period).checked_duration_since(now) else {
                    break;
                };
                if wait.is_zero() {
                    break;
                }
                st = match rp.cv.wait_timeout(st, wait) {
                    Ok((g, _)) => g,
                    Err(_) => return,
                };
            }
        }
        last = Instant::now();
        let Some((client, info)) = probe(cfg) else {
            tracing::debug!("voice daemon still not answering; staying on the fallback");
            continue;
        };
        if rp.state.lock().map(|s| s.stop).unwrap_or(true) {
            return;
        }
        let engine = crate::engine::DaemonEngine::new(client, &info, cfg.preset());
        tracing::info!(
            endpoint = %engine.client.endpoint().display(),
            preset = engine.preset.as_deref().unwrap_or("daemon default"),
            "voice: the voice daemon is up now; switching to the cloned voice from the next line"
        );
        if let Ok(mut i) = sh.info.lock() {
            i.name = engine.name();
            i.warning = engine.voice_warning();
        }
        if let Ok(mut p) = sh.pending.lock() {
            *p = Some(Box::new(engine));
        }
        sh.cv.notify_all();
        return;
    }
}

fn worker(sh: Arc<Shared>, mut engine: Box<dyn Engine>, opts: SpeakOpts) {
    let mut warned = false;
    loop {
        let line = {
            let Ok(mut s) = sh.state.lock() else { return };
            loop {
                if s.shutdown {
                    return;
                }
                if let Some(l) = s.queue.pop_front() {
                    s.interrupt = false;
                    s.busy = true;
                    break l;
                }
                s.interrupt = false;
                s = match sh.cv.wait(s) {
                    Ok(g) => g,
                    Err(_) => return,
                };
            }
        };
        // Between lines only: a late daemon takes over here.
        if let Some(e) = sh.pending.lock().ok().and_then(|mut p| p.take()) {
            engine = e;
            warned = false;
        }
        if !sh.silenced() {
            match engine.start(&line, &opts) {
                Ok(p) => play(&sh, p),
                Err(e) => {
                    if !warned {
                        tracing::warn!(engine = engine.name(), "voice engine failed: {e}");
                        warned = true;
                    }
                    if let Ok(mut s) = sh.state.lock() {
                        s.stats.engine_errors += 1;
                    }
                }
            }
        }
        if let Ok(mut s) = sh.state.lock() {
            s.busy = false;
        }
    }
}

fn play(sh: &Shared, mut p: Box<dyn Playback>) {
    loop {
        if p.poll() {
            if let Ok(mut s) = sh.state.lock() {
                s.stats.spoken += 1;
            }
            return;
        }
        let Ok(s) = sh.state.lock() else {
            p.stop();
            return;
        };
        if s.shutdown || s.interrupt || sh.silenced() {
            drop(s);
            p.stop();
            if let Ok(mut s) = sh.state.lock() {
                s.stats.cut += 1;
            }
            return;
        }
        let _ = sh.cv.wait_timeout(s, POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::NullEngine;
    use crate::lines::NoLines;
    use std::sync::atomic::AtomicUsize;

    /// Records lines; each playback lasts until `release` is set (or forever
    /// when `hold`), and counts stops.
    #[derive(Clone, Default)]
    struct Mock {
        started: Arc<Mutex<Vec<String>>>,
        stops: Arc<AtomicUsize>,
        hold: Arc<AtomicBool>,
    }
    struct MockPlay {
        hold: Arc<AtomicBool>,
        stops: Arc<AtomicUsize>,
    }
    impl Playback for MockPlay {
        fn poll(&mut self) -> bool {
            !self.hold.load(Ordering::SeqCst)
        }
        fn stop(&mut self) {
            self.stops.fetch_add(1, Ordering::SeqCst);
        }
    }
    impl Engine for Mock {
        fn name(&self) -> &'static str {
            "mock"
        }
        fn start(&mut self, text: &str, _: &SpeakOpts) -> std::io::Result<Box<dyn Playback>> {
            self.started.lock().unwrap().push(text.to_string());
            Ok(Box::new(MockPlay {
                hold: Arc::clone(&self.hold),
                stops: Arc::clone(&self.stops),
            }))
        }
    }

    fn cfg() -> VoiceConfig {
        VoiceConfig {
            cooldown_s: 0.0,
            repeat_cooldown_s: 0.0,
            queue: 2,
            ..VoiceConfig::default()
        }
    }

    fn wait_until(f: impl Fn() -> bool) {
        let t = Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(5), "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn gate_cooldown_and_repeat() {
        let mut g = Gate::new(8.0, 90.0);
        assert_eq!(g.admit("Hi", 0.0, Urgency::Normal), Ok(()));
        assert_eq!(
            g.admit("Other", 3.0, Urgency::Normal),
            Err(Dropped::Cooldown)
        );
        assert_eq!(g.admit("Other", 8.5, Urgency::Normal), Ok(()));
        assert_eq!(
            g.admit("hi ", 20.0, Urgency::Normal),
            Err(Dropped::Repeat),
            "case/space-insensitive"
        );
        assert_eq!(g.admit("Hi", 100.0, Urgency::Normal), Ok(()));
        // Interrupt skips the global cooldown but not the repeat guard.
        assert_eq!(g.admit("Alarm", 101.0, Urgency::Interrupt), Ok(()));
        assert_eq!(
            g.admit("Alarm", 102.0, Urgency::Interrupt),
            Err(Dropped::Repeat)
        );
    }

    #[test]
    fn speakers_switch_is_live() {
        let m = Mock::default();
        m.hold.store(true, Ordering::SeqCst);
        let mut v = Voice::with_engine(cfg(), Box::new(m.clone()), Box::new(NoLines));
        v.say("playing", Urgency::Normal).unwrap();
        wait_until(|| m.started.lock().unwrap().len() == 1);
        // Off: the playing line is cut and new lines are dropped.
        v.set_enabled(false);
        wait_until(|| m.stops.load(Ordering::SeqCst) == 1);
        assert!(!v.enabled() && !v.is_active() && v.has_engine());
        assert_eq!(v.say("quiet", Urgency::Normal), Err(Dropped::Disabled));
        assert_eq!(v.say_reply("quiet reply"), Err(Dropped::Disabled));
        // On again: speaks without a new Voice (no restart).
        m.hold.store(false, Ordering::SeqCst);
        v.set_enabled(true);
        assert!(v.is_active());
        v.say("back", Urgency::Normal).unwrap();
        wait_until(|| v.stats().spoken == 1);
        assert_eq!(*m.started.lock().unwrap(), ["playing", "back"]);
    }

    #[test]
    fn disabled_at_start_can_be_switched_on() {
        let m = Mock::default();
        let c = VoiceConfig {
            enabled: false,
            ..cfg()
        };
        let mut v = Voice::with_engine(c, Box::new(m.clone()), Box::new(NoLines));
        assert_eq!(v.say("no", Urgency::Normal), Err(Dropped::Disabled));
        v.set_enabled(true);
        v.say("yes", Urgency::Normal).unwrap();
        wait_until(|| v.stats().spoken == 1);
        assert_eq!(*m.started.lock().unwrap(), ["yes"]);
    }

    #[test]
    fn speakers_off_in_config_still_finds_the_engine_for_the_live_toggle() {
        let c = VoiceConfig {
            enabled: false,
            engine: crate::config::EngineChoice::EspeakNg,
            ..cfg()
        };
        let lookup = |b: &str| (b == "espeak-ng").then(|| std::path::PathBuf::from("/x/espeak-ng"));
        let mut v = Voice::from_config_with(
            c,
            &lookup,
            Arc::new(|_: &VoiceConfig| None),
            ReprobeTiming::default(),
        );
        assert!(v.has_engine() && !v.enabled() && !v.is_active());
        v.set_enabled(true);
        assert!(v.is_active());
    }

    #[test]
    fn reply_cuts_banter_skips_cooldown_and_keeps_long_text() {
        let m = Mock::default();
        m.hold.store(true, Ordering::SeqCst);
        let c = VoiceConfig {
            cooldown_s: 60.0,
            max_chars: 20,
            ..cfg()
        };
        let mut v = Voice::with_engine(c, Box::new(m.clone()), Box::new(NoLines));
        v.say_at("banter line", Urgency::Normal, 0.0).unwrap();
        wait_until(|| m.started.lock().unwrap().len() == 1);
        let reply = "This chat reply is a good deal longer than twenty characters.";
        v.say_reply_at(reply, 1.0).unwrap();
        wait_until(|| m.stops.load(Ordering::SeqCst) == 1);
        m.hold.store(false, Ordering::SeqCst);
        wait_until(|| m.started.lock().unwrap().len() == 2);
        assert_eq!(m.started.lock().unwrap()[1], reply, "not cut at max_chars");
        // Muted: replies stay silent too.
        v.set_muted(true);
        assert_eq!(v.say_reply_at("later", 99.0), Err(Dropped::Muted));
        assert_eq!(v.say_reply("   "), Err(Dropped::Muted));
        v.set_muted(false);
        assert_eq!(v.say_reply_at("  ", 99.0), Err(Dropped::Empty));
        // Chat replies are emoji-filtered too (speech_text); emoji-only is Empty.
        assert_eq!(v.say_reply_at("😈💜 <3", 200.0), Err(Dropped::Empty));
        v.say_reply_at("*purrs* Good boy 😈", 300.0).unwrap();
        wait_until(|| m.started.lock().unwrap().len() == 3);
        assert_eq!(m.started.lock().unwrap()[2], "Good boy");
    }

    #[test]
    fn clean_line_cuts_at_words() {
        assert_eq!(clean_line("  hello \n  world\t", 100), "hello world");
        assert_eq!(clean_line("one two three four", 9), "one two…");
        assert_eq!(clean_line("", 10), "");
    }

    #[test]
    fn speaks_in_order_on_the_worker() {
        let m = Mock::default();
        let mut v = Voice::with_engine(cfg(), Box::new(m.clone()), Box::new(NoLines));
        assert!(v.is_active());
        v.say("one", Urgency::Normal).unwrap();
        wait_until(|| v.stats().spoken == 1);
        v.say("two", Urgency::Normal).unwrap();
        wait_until(|| v.stats().spoken == 2);
        assert_eq!(*m.started.lock().unwrap(), ["one", "two"]);
    }

    #[test]
    fn engine_never_hears_emoji_or_emoticons() {
        let m = Mock::default();
        let mut v = Voice::with_engine(cfg(), Box::new(m.clone()), Box::new(NoLines));
        v.say("*winks* Gotcha! 🗡️ :)", Urgency::Normal).unwrap();
        wait_until(|| v.stats().spoken == 1);
        assert_eq!(*m.started.lock().unwrap(), ["Gotcha!"]);
        // Nothing speakable left: skipped, the engine is not started.
        assert_eq!(v.say("😈💜 <3", Urgency::Normal), Err(Dropped::Empty));
        assert_eq!(v.stats().queued, 1);
    }

    #[test]
    fn bounded_queue_drops_oldest_and_interrupt_cuts() {
        let m = Mock::default();
        m.hold.store(true, Ordering::SeqCst);
        let mut v = Voice::with_engine(cfg(), Box::new(m.clone()), Box::new(NoLines));
        v.say("playing", Urgency::Normal).unwrap();
        wait_until(|| m.started.lock().unwrap().len() == 1);
        for l in ["a", "b", "c", "d"] {
            v.say(l, Urgency::Normal).unwrap();
        }
        assert_eq!(v.stats().dropped_full, 2, "cap 2: a and b dropped");
        // Interrupt: cut "playing", clear c/d, speak "now".
        v.say("now", Urgency::Interrupt).unwrap();
        wait_until(|| m.started.lock().unwrap().len() == 2);
        assert_eq!(m.started.lock().unwrap()[1], "now");
        assert_eq!(m.stops.load(Ordering::SeqCst), 1);
        m.hold.store(false, Ordering::SeqCst);
        wait_until(|| !v.speaking());
        assert_eq!(*m.started.lock().unwrap(), ["playing", "now"]);
        assert_eq!(v.stats().cut, 1);
    }

    #[test]
    fn mute_cuts_and_blocks_until_unmuted() {
        let m = Mock::default();
        m.hold.store(true, Ordering::SeqCst);
        let mut v = Voice::with_engine(cfg(), Box::new(m.clone()), Box::new(NoLines));
        v.say("long line", Urgency::Normal).unwrap();
        v.say("queued", Urgency::Normal).unwrap();
        wait_until(|| m.started.lock().unwrap().len() == 1);
        v.set_muted(true);
        wait_until(|| m.stops.load(Ordering::SeqCst) == 1);
        assert_eq!(v.say("hello", Urgency::Interrupt), Err(Dropped::Muted));
        wait_until(|| !v.speaking());
        assert_eq!(
            m.started.lock().unwrap().len(),
            1,
            "queued line was cleared"
        );
        m.hold.store(false, Ordering::SeqCst);
        v.set_muted(false);
        v.say("back", Urgency::Normal).unwrap();
        wait_until(|| v.stats().spoken == 1);
        assert_eq!(m.started.lock().unwrap().last().unwrap(), "back");
        // Muted from config and via the brain's Action::Muted.
        let v2 = Voice::with_engine(
            VoiceConfig {
                mute: true,
                ..cfg()
            },
            Box::new(Mock::default()),
            Box::new(NoLines),
        );
        assert!(v2.muted());
        v.on_actions(&[Action::Muted(true)]);
        assert!(v.muted());
    }

    #[test]
    fn no_engine_is_a_clean_noop() {
        let mut v = Voice::with_engine(
            cfg(),
            Box::new(NullEngine),
            Box::new(DefaultLines::default()),
        );
        assert!(!v.is_active());
        assert_eq!(v.engine_name(), "none");
        assert_eq!(v.say("hi", Urgency::Normal), Err(Dropped::NoEngine));
        assert_eq!(
            v.on_event(&Event::new("cpu.high"), Mood::Playful, &[]),
            None
        );
        assert!(!v.speaking());
        let mut off = Voice::with_engine(
            VoiceConfig {
                enabled: false,
                ..cfg()
            },
            Box::new(Mock::default()),
            Box::new(NoLines),
        );
        assert!(!off.is_active(), "disabled: silent even with a real engine");
        assert!(off.has_engine(), "worker kept for the live speakers switch");
        assert_eq!(off.say("hi", Urgency::Normal), Err(Dropped::Disabled));
        drop(v); // no thread to join, must not hang
    }

    #[test]
    fn rule_line_beats_line_source_and_cooldown_applies() {
        let m = Mock::default();
        let c = VoiceConfig {
            cooldown_s: 8.0,
            ..cfg()
        };
        let mut v = Voice::with_engine(c, Box::new(m.clone()), Box::new(DefaultLines::new(1)));
        let ev = Event::new("cpu.high");
        let said = v.on_event_at(&ev, Mood::Playful, &[Action::Say("Rule line".into())], 0.0);
        assert_eq!(said.as_deref(), Some("Rule line"));
        // Within the cooldown nothing is queued.
        assert_eq!(v.on_event_at(&ev, Mood::Playful, &[], 2.0), None);
        // After it, the default table speaks.
        let l = v
            .on_event_at(&ev, Mood::Playful, &[], 9.0)
            .expect("table line");
        assert!(crate::lines::DEFAULT_TABLE
            .iter()
            .any(|(k, _, ls)| *k == "cpu.high" && ls.contains(&l.as_str())));
        // Quiet events never speak.
        assert_eq!(
            v.on_event_at(&Event::new("input.hover"), Mood::Playful, &[], 30.0),
            None
        );
        wait_until(|| v.stats().spoken == 2);
    }

    fn fast(min_gap_ms: u64, period_ms: u64) -> ReprobeTiming {
        ReprobeTiming {
            min_gap: Duration::from_millis(min_gap_ms),
            period: Duration::from_millis(period_ms),
        }
    }

    /// A Mock fallback plus a prober using `probe`.
    fn reprobing(m: &Mock, probe: SharedProbe, t: ReprobeTiming) -> Voice {
        let mut v = Voice::with_engine(cfg(), Box::new(m.clone()), Box::new(NoLines));
        v.start_reprobe(cfg(), probe, t);
        v
    }

    #[test]
    fn reprobe_on_say_is_rate_limited_and_never_blocks_the_caller() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&calls);
        let probe: SharedProbe = Arc::new(move |_: &VoiceConfig| {
            c.fetch_add(1, Ordering::SeqCst);
            // A slow (hung) daemon: the say must not wait for this.
            std::thread::sleep(Duration::from_millis(300));
            None
        });
        let m = Mock::default();
        let mut v = reprobing(&m, probe, fast(150, 3_600_000));
        assert_eq!(v.engine_name(), "mock");
        assert!(v.voice_warning().unwrap().contains("daemon not running"));
        // Within the gap of the start-up probe: no re-check.
        v.say_at("a", Urgency::Normal, 1.0).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        std::thread::sleep(Duration::from_millis(200));
        let t = Instant::now();
        for i in 0..5 {
            v.say_at(&format!("line {i}"), Urgency::Normal, 2.0 + f64::from(i))
                .unwrap();
        }
        assert!(t.elapsed() < Duration::from_millis(100), "say blocked");
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one probe for five says");
        // Still the fallback, still warned.
        assert_eq!(v.engine_name(), "mock");
        assert!(v.voice_warning().is_some());
    }

    #[test]
    fn reprobe_timer_rechecks_without_any_say() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&calls);
        let probe: SharedProbe = Arc::new(move |_: &VoiceConfig| {
            c.fetch_add(1, Ordering::SeqCst);
            None
        });
        let m = Mock::default();
        let v = reprobing(&m, probe, fast(3_600_000, 40));
        wait_until(|| calls.load(Ordering::SeqCst) >= 3);
        drop(v);
        // Dropping the voice stops the prober.
        std::thread::sleep(Duration::from_millis(100));
        let n = calls.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(calls.load(Ordering::SeqCst), n);
    }

    #[test]
    fn no_reprobe_when_the_daemon_answered_or_engine_is_fixed() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&calls);
        let probe: SharedProbe = Arc::new(move |_: &VoiceConfig| {
            c.fetch_add(1, Ordering::SeqCst);
            None
        });
        let lookup = |b: &str| (b == "espeak-ng").then(|| std::path::PathBuf::from("/x/espeak-ng"));
        let fixed = VoiceConfig {
            engine: crate::config::EngineChoice::EspeakNg,
            ..cfg()
        };
        let mut v = Voice::from_config_with(fixed, &lookup, probe, fast(0, 10));
        assert!(v.reprobe.is_none());
        assert_eq!(v.voice_warning(), None);
        let _ = v.say_at("x", Urgency::Normal, 1.0);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    /// The daemon at a FakeDaemon tag's (deterministic) socket path.
    #[cfg(unix)]
    fn daemon_cfg(tag: &str) -> VoiceConfig {
        let sock = std::env::temp_dir()
            .join(format!("lillith-fake-voiced-{tag}-{}", std::process::id()))
            .join("voiced.sock");
        VoiceConfig {
            daemon_socket: Some(sock),
            ..cfg()
        }
    }

    #[cfg(unix)]
    fn say_ok_daemon(tag: &str) -> crate::daemon::fake::FakeDaemon {
        use crate::daemon::{fake::FakeDaemon, request_command};
        FakeDaemon::start(
            tag,
            Box::new(|v: &serde_json::Value| match request_command(v) {
                "ping" => vec![r#"{"ok":true,"protocol":2,"presets":["lillith","p1"]}"#.into()],
                "say" => vec![r#"{"ok":true,"utt":1}"#.into()],
                _ => vec![r#"{"ok":true}"#.into()],
            }),
        )
    }

    #[cfg(unix)]
    #[test]
    fn late_daemon_is_picked_up_between_lines_never_mid_line() {
        let c = daemon_cfg("late-start");
        let m = Mock::default();
        m.hold.store(true, Ordering::SeqCst);
        let mut v = Voice::with_engine(c.clone(), Box::new(m.clone()), Box::new(NoLines));
        v.start_reprobe(
            c.clone(),
            Arc::new(crate::engine::ping_daemon),
            fast(50, 3_600_000),
        );
        // No daemon yet: the fallback speaks (and holds the line).
        v.say_at("one", Urgency::Normal, 1.0).unwrap();
        wait_until(|| m.started.lock().unwrap().len() == 1);
        // Now the daemon starts; the next say kicks a re-check.
        let d = say_ok_daemon("late-start");
        std::thread::sleep(Duration::from_millis(80));
        v.say_at("two", Urgency::Normal, 2.0).unwrap();
        wait_until(|| v.engine_name() == "voiced");
        assert_eq!(
            v.voice_warning(),
            None,
            "cloned voice present: warning cleared"
        );
        // "one" is still playing on the fallback: not cut, not switched.
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(m.stops.load(Ordering::SeqCst), 0, "never switched mid-line");
        assert_eq!(*m.started.lock().unwrap(), ["one"]);
        // When it ends, "two" (and later lines) go to the daemon.
        m.hold.store(false, Ordering::SeqCst);
        wait_until(|| d.commands().iter().filter(|c| *c == "say").count() == 1);
        v.say_at("three", Urgency::Normal, 3.0).unwrap();
        wait_until(|| d.commands().iter().filter(|c| *c == "say").count() == 2);
        let seen = d.seen.lock().unwrap().clone();
        let says: Vec<_> = seen
            .iter()
            .filter(|r| crate::daemon::request_command(r) == "say")
            .map(|r| r["text"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(says, ["two", "three"]);
        assert_eq!(
            *m.started.lock().unwrap(),
            ["one"],
            "fallback no longer used"
        );
    }

    #[cfg(unix)]
    #[test]
    fn late_daemon_gives_a_silent_voice_a_worker_on_the_timer() {
        let c = daemon_cfg("late-timer");
        let lookup = |_: &str| None;
        let mut v = Voice::from_config_with(
            c,
            &lookup,
            Arc::new(crate::engine::ping_daemon),
            fast(3_600_000, 40),
        );
        assert_eq!(v.engine_name(), "none");
        assert!(!v.has_engine());
        assert_eq!(v.say_at("x", Urgency::Normal, 1.0), Err(Dropped::NoEngine));
        assert!(v.voice_warning().unwrap().contains("silent"));
        let d = say_ok_daemon("late-timer");
        wait_until(|| v.engine_name() == "voiced");
        assert!(v.has_engine());
        v.say_at("hello", Urgency::Normal, 2.0).unwrap();
        wait_until(|| d.commands().contains(&"say".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn hung_daemon_times_out_and_she_falls_back_without_hanging() {
        use crate::daemon::fake::FakeDaemon;
        // Accepts every connection, never answers.
        let d = FakeDaemon::start("hung-voice", Box::new(|_: &serde_json::Value| Vec::new()));
        let c = VoiceConfig {
            daemon_socket: Some(d.sock.clone()),
            ..cfg()
        };
        let lookup = |b: &str| (b == "espeak-ng").then(|| std::path::PathBuf::from("/x/espeak-ng"));
        let t = Instant::now();
        let mut v = Voice::from_config_with(
            c,
            &lookup,
            Arc::new(crate::engine::ping_daemon),
            fast(20, 3_600_000),
        );
        assert!(
            t.elapsed() < Duration::from_secs(2),
            "start-up probe bounded: {:?}",
            t.elapsed()
        );
        assert_eq!(v.engine_name(), "espeak-ng");
        assert!(v.voice_warning().unwrap().contains("espeak-ng"));
        v.set_muted(true); // don't run the (fake) espeak-ng binary
        std::thread::sleep(Duration::from_millis(30));
        let t = Instant::now();
        let _ = v.say_at("x", Urgency::Normal, 1.0);
        assert!(
            t.elapsed() < Duration::from_millis(50),
            "say never waits on the ping"
        );
        // The re-check pings, times out, and she stays on the fallback.
        wait_until(|| d.commands().iter().filter(|c| *c == "ping").count() >= 2);
        std::thread::sleep(crate::daemon::PING_TIMEOUT + Duration::from_millis(200));
        assert_eq!(v.engine_name(), "espeak-ng");
        assert!(
            d.commands().iter().all(|c| c == "ping"),
            "{:?}",
            d.commands()
        );
        let t = Instant::now();
        drop(v);
        assert!(t.elapsed() < Duration::from_secs(1), "drop does not hang");
    }
}
