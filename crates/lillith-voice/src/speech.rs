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
//! - Daemon failover (`[voice] engine` = `auto` / `daemon`): the worker
//!   holds the cloned voice ([`crate::engine::DaemonEngine`]) when the
//!   daemon answers AND a local fallback voice (piper / espeak-ng / spd-say
//!   / SAPI), always. While the daemon is down every line is spoken by the
//!   fallback (with a WARN at start-up and on every daemon → fallback
//!   transition, and [`Voice::voice_warning`] set); a line whose daemon
//!   `say` fails is spoken again by the fallback, never dropped.
//! - While on the fallback the daemon is re-checked before a line (at most
//!   once per [`REPROBE_MIN_GAP`], on the worker thread) and by a prober
//!   thread every [`REPROBE_PERIOD`]. Once it answers, the switch back is
//!   logged once (INFO) and applies from the next line (never mid-line).
//!   No ping ever runs on the caller's (UI) thread.

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
    /// WARNs that a fallback voice is in use (start-up + each daemon →
    /// fallback transition).
    pub fallback_warnings: u64,
    /// Switches fallback → cloned voice (one INFO each).
    pub daemon_switches: u64,
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
    /// The cloned voice is in use (no re-probing needed).
    daemon_in_use: AtomicBool,
    /// When the daemon was last pinged (start-up probe included).
    last_probe: Mutex<Instant>,
    /// How to re-probe (`None`: fixed engine, no failover).
    probe: Mutex<Option<Arc<ProbeCtx>>>,
    /// Wakes the prober thread.
    reprobe: Reprobe,
}

struct ProbeCtx {
    cfg: VoiceConfig,
    probe: SharedProbe,
    timing: ReprobeTiming,
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
    /// A prober thread runs (daemon failover is on).
    reprobing: bool,
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
        let failover = matches!(probe_cfg.engine, EngineChoice::Auto | EngineChoice::Daemon);
        let (fallback, daemon) = if d.engine.name() == "voiced" {
            // Kept ready for when the daemon goes away mid-session.
            let f = crate::engine::detect_full(&probe_cfg, lookup, &|_| None);
            tracing::debug!("voice: fallback if the daemon goes away: {}", f.reason);
            (f.engine, Some(d.engine))
        } else {
            (d.engine, None)
        };
        let mut v = Self::with_engines(cfg, fallback, daemon, Box::new(DefaultLines::default()));
        if failover {
            v.start_reprobe(probe_cfg, probe, timing);
        }
        v
    }

    /// Turns on daemon failover (see the module docs) and starts the prober
    /// thread. On the fallback now: WARN once and set the warning.
    fn start_reprobe(&mut self, probe_cfg: VoiceConfig, probe: SharedProbe, timing: ReprobeTiming) {
        if let Ok(mut l) = self.shared.last_probe.lock() {
            *l = Instant::now();
        }
        if !self.shared.daemon_in_use.load(Ordering::SeqCst) {
            let at = crate::daemon::DaemonClient::from_config(&probe_cfg)
                .map(|c| c.endpoint().display().to_string())
                .unwrap_or_else(|| "no endpoint".into());
            let name = self.engine_name();
            let w = fallback_warning(name, &format!("voice daemon not running ({at})"), "starts");
            self.shared.warn_fallback(name, w);
        }
        if let Ok(mut c) = self.shared.probe.lock() {
            *c = Some(Arc::new(ProbeCtx {
                cfg: probe_cfg,
                probe,
                timing,
            }));
        }
        let sh = Arc::clone(&self.shared);
        match std::thread::Builder::new()
            .name("lillith-voice-reprobe".into())
            .spawn(move || prober(&sh))
        {
            Ok(_) => self.reprobing = true,
            Err(e) => tracing::warn!("voice daemon re-probe thread: {e}"),
        }
    }

    /// No worker (silent fallback): asks the prober for a re-check
    /// (rate-limited there; never blocks). With a worker, the worker
    /// re-checks before its next line itself.
    fn kick_reprobe(&self) {
        if !self.reprobing || self.worker.is_some() {
            return;
        }
        if let Ok(mut st) = self.shared.reprobe.state.lock() {
            st.kicked = true;
        }
        self.shared.reprobe.cv.notify_all();
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
        let engines = Engines {
            fallback: Box::new(crate::engine::NullEngine),
            daemon: Some(engine),
        };
        self.worker = Some(spawn_worker(&self.shared, engines, &self.cfg));
    }

    /// Explicit engine + line source (tests, custom engines).
    pub fn with_engine(
        cfg: VoiceConfig,
        engine: Box<dyn Engine>,
        lines: Box<dyn LineSource>,
    ) -> Self {
        Self::with_engines(cfg, engine, None, lines)
    }

    /// `fallback` speaks whenever `daemon` is absent or fails.
    fn with_engines(
        cfg: VoiceConfig,
        fallback: Box<dyn Engine>,
        daemon: Option<Box<dyn Engine>>,
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
            info: Mutex::new(match &daemon {
                Some(d) => EngineInfo {
                    name: d.name(),
                    warning: d.voice_warning(),
                },
                None => EngineInfo {
                    name: fallback.name(),
                    warning: fallback.voice_warning(),
                },
            }),
            daemon_in_use: AtomicBool::new(daemon.is_some()),
            last_probe: Mutex::new(Instant::now()),
            probe: Mutex::new(None),
            reprobe: Reprobe::default(),
        });
        // Spawned even while disabled so the speakers switch works live.
        let worker = (fallback.is_real() || daemon.is_some())
            .then(|| spawn_worker(&shared, Engines { fallback, daemon }, &cfg));
        Self {
            gate: Gate::new(cfg.cooldown_s, cfg.repeat_cooldown_s),
            cfg,
            shared,
            worker,
            reprobing: false,
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
        // On the fallback: the daemon is re-checked off this thread; a
        // switch applies to later lines.
        self.adopt_pending();
        self.kick_reprobe();
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
        if let Ok(mut st) = self.shared.reprobe.state.lock() {
            st.stop = true;
        }
        self.shared.reprobe.cv.notify_all();
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

/// The worker's engines: the cloned voice while the daemon is up, and the
/// local fallback (possibly [`crate::engine::NullEngine`]).
struct Engines {
    fallback: Box<dyn Engine>,
    daemon: Option<Box<dyn Engine>>,
}

fn spawn_worker(shared: &Arc<Shared>, engines: Engines, cfg: &VoiceConfig) -> JoinHandle<()> {
    let sh = Arc::clone(shared);
    let opts = SpeakOpts::from_config(cfg);
    std::thread::Builder::new()
        .name("lillith-voice".into())
        .spawn(move || worker(sh, engines, opts))
        .expect("spawn voice worker")
}

/// The UI notice while the fallback speaks.
fn fallback_warning(fallback: &str, why: &str, until: &str) -> String {
    if fallback == "none" {
        format!("{why}; no fallback voice installed, she stays silent until it {until}")
    } else {
        format!("{why}; using the fallback {fallback} voice until it {until}")
    }
}

impl Shared {
    fn probe_ctx(&self) -> Option<Arc<ProbeCtx>> {
        self.probe.lock().ok().and_then(|c| c.clone())
    }

    fn since_last_probe(&self) -> Duration {
        self.last_probe
            .lock()
            .map(|l| l.elapsed())
            .unwrap_or(Duration::MAX)
    }

    /// Pings the daemon now (never on the caller's thread); on an answer
    /// stages its engine for the worker's next line, logs the switch once
    /// (INFO) and clears the fallback warning.
    fn probe_and_stage(&self, ctx: &ProbeCtx) {
        if let Ok(mut l) = self.last_probe.lock() {
            *l = Instant::now();
        }
        let Some((client, info)) = (ctx.probe)(&ctx.cfg) else {
            tracing::debug!("voice daemon still not answering; staying on the fallback");
            return;
        };
        let engine = crate::engine::DaemonEngine::new(client, &info, ctx.cfg.preset());
        let Ok(mut pending) = self.pending.lock() else {
            return;
        };
        if self.daemon_in_use.swap(true, Ordering::SeqCst) {
            return; // someone else switched already
        }
        if let Ok(mut s) = self.state.lock() {
            s.stats.daemon_switches += 1;
        }
        tracing::info!(
            endpoint = %engine.client.endpoint().display(),
            preset = engine.preset.as_deref().unwrap_or("daemon default"),
            "voice: the voice daemon is up now; switching to the cloned voice from the next line"
        );
        if let Ok(mut i) = self.info.lock() {
            i.name = engine.name();
            i.warning = engine.voice_warning();
        }
        *pending = Some(Box::new(engine));
        drop(pending);
        self.cv.notify_all();
    }

    /// The one WARN (stderr log, never stdout) for a fallback voice now in
    /// use; it is also the UI's [`Voice::voice_warning`].
    fn warn_fallback(&self, fallback: &'static str, w: String) {
        tracing::warn!("voice: {w}");
        if let Ok(mut s) = self.state.lock() {
            s.stats.fallback_warnings += 1;
        }
        if let Ok(mut i) = self.info.lock() {
            i.name = fallback;
            i.warning = Some(w);
        }
    }

    /// Daemon → fallback (once per transition): WARN and set the warning.
    fn fall_back(&self, fallback: &'static str, why: &str) {
        if !self.daemon_in_use.swap(false, Ordering::SeqCst) {
            return;
        }
        let w = fallback_warning(
            fallback,
            &format!("voice daemon unreachable (voice engine failed: {why})"),
            "is back",
        );
        self.warn_fallback(fallback, w);
        // The prober's timer resumes (it idles while the daemon is in use).
        self.reprobe.cv.notify_all();
    }
}

/// The prober thread: while on the fallback, pings the daemon when kicked
/// (a say with no worker; ignored within `min_gap` of the last probe) or
/// every `period`; idles while the cloned voice is in use. Exits when the
/// voice is dropped.
fn prober(sh: &Shared) {
    let Some(ctx) = sh.probe_ctx() else { return };
    let t = ctx.timing;
    loop {
        {
            let Ok(mut st) = sh.reprobe.state.lock() else {
                return;
            };
            loop {
                if st.stop {
                    return;
                }
                let kicked = std::mem::take(&mut st.kicked);
                let wait = if sh.daemon_in_use.load(Ordering::SeqCst) {
                    t.period
                } else {
                    let since = sh.since_last_probe();
                    if (kicked && since >= t.min_gap) || since >= t.period {
                        break;
                    }
                    t.period - since
                };
                st = match sh.reprobe.cv.wait_timeout(st, wait) {
                    Ok((g, _)) => g,
                    Err(_) => return,
                };
            }
        }
        sh.probe_and_stage(&ctx);
    }
}

fn worker(sh: Arc<Shared>, mut eng: Engines, opts: SpeakOpts) {
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
        // Between lines only: on the fallback, a say re-checks the daemon
        // (rate-limited), and a staged daemon engine takes over here.
        if eng.daemon.is_none() && !sh.silenced() {
            if let Some(ctx) = sh.probe_ctx() {
                if sh.since_last_probe() >= ctx.timing.min_gap {
                    sh.probe_and_stage(&ctx);
                }
            }
        }
        if let Some(e) = sh.pending.lock().ok().and_then(|mut p| p.take()) {
            eng.daemon = Some(e);
        }
        // A stop / the safeword during that check: this line is gone.
        let cut = sh
            .state
            .lock()
            .map(|s| s.interrupt || s.shutdown)
            .unwrap_or(true);
        if !cut && !sh.silenced() {
            let mut spoken = false;
            if let Some(d) = eng.daemon.as_mut() {
                let failed = match d.start(&line, &opts) {
                    Ok(p) => match play(&sh, p) {
                        Outcome::Failed(why) => Some(why),
                        Outcome::Done | Outcome::Cut => None,
                    },
                    Err(e) => Some(e.to_string()),
                };
                match failed {
                    None => spoken = true,
                    Some(why) => {
                        // The line is spoken by the fallback instead.
                        if let Ok(mut s) = sh.state.lock() {
                            s.stats.engine_errors += 1;
                        }
                        eng.daemon = None;
                        sh.fall_back(eng.fallback.name(), &why);
                    }
                }
            }
            let cut = sh
                .state
                .lock()
                .map(|s| s.interrupt || s.shutdown)
                .unwrap_or(true);
            if !spoken && !cut && !sh.silenced() {
                match eng.fallback.start(&line, &opts) {
                    Ok(p) => {
                        if let Outcome::Failed(why) = play(&sh, p) {
                            tracing::debug!(
                                engine = eng.fallback.name(),
                                "voice line failed: {why}"
                            );
                        }
                    }
                    Err(e) => {
                        if !warned {
                            tracing::warn!(
                                engine = eng.fallback.name(),
                                "voice engine failed: {e}"
                            );
                            warned = true;
                        }
                        if let Ok(mut s) = sh.state.lock() {
                            s.stats.engine_errors += 1;
                        }
                    }
                }
            }
        }
        if let Ok(mut s) = sh.state.lock() {
            s.busy = false;
        }
    }
}

/// How one playback ended.
enum Outcome {
    Done,
    /// Interrupt / mute / speakers off / shutdown.
    Cut,
    /// The engine failed mid-line (e.g. the daemon died).
    Failed(String),
}

fn play(sh: &Shared, mut p: Box<dyn Playback>) -> Outcome {
    loop {
        if p.poll() {
            if let Some(why) = p.failure() {
                return Outcome::Failed(why);
            }
            if let Ok(mut s) = sh.state.lock() {
                s.stats.spoken += 1;
            }
            return Outcome::Done;
        }
        let Ok(s) = sh.state.lock() else {
            p.stop();
            return Outcome::Cut;
        };
        if s.shutdown || s.interrupt || sh.silenced() {
            drop(s);
            p.stop();
            if let Ok(mut s) = sh.state.lock() {
                s.stats.cut += 1;
            }
            return Outcome::Cut;
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

    const HOUR: u64 = 3_600_000;

    /// A Mock fallback plus failover using `probe`.
    fn reprobing(m: &Mock, probe: SharedProbe, t: ReprobeTiming) -> Voice {
        let mut v = Voice::with_engine(cfg(), Box::new(m.clone()), Box::new(NoLines));
        v.start_reprobe(cfg(), probe, t);
        v
    }

    #[test]
    fn startup_fallback_warning_is_emitted_once() {
        let m = Mock::default();
        let v = reprobing(&m, Arc::new(|_: &VoiceConfig| None), fast(HOUR, HOUR));
        assert_eq!(v.engine_name(), "mock");
        let w = v.voice_warning().unwrap();
        assert!(w.contains("voice daemon not running"), "{w}");
        assert!(w.contains("fallback mock voice"), "{w}");
        assert_eq!(v.stats().fallback_warnings, 1);
        assert_eq!(v.stats().daemon_switches, 0);
    }

    #[test]
    fn startup_fallback_warning_from_config() {
        let lookup = |b: &str| (b == "espeak-ng").then(|| std::path::PathBuf::from("/x/espeak-ng"));
        let v = Voice::from_config_with(
            cfg(),
            &lookup,
            Arc::new(|_: &VoiceConfig| None),
            fast(HOUR, HOUR),
        );
        assert_eq!(v.engine_name(), "espeak-ng");
        assert!(v
            .voice_warning()
            .unwrap()
            .contains("fallback espeak-ng voice"));
        assert_eq!(v.stats().fallback_warnings, 1);
        // A fixed engine is no fallback: no warning.
        let fixed = VoiceConfig {
            engine: crate::config::EngineChoice::EspeakNg,
            ..cfg()
        };
        let v = Voice::from_config_with(
            fixed,
            &lookup,
            Arc::new(|_: &VoiceConfig| None),
            fast(HOUR, HOUR),
        );
        assert_eq!(v.voice_warning(), None);
        assert_eq!(v.stats().fallback_warnings, 0);
    }

    #[test]
    fn reprobe_before_a_line_is_rate_limited_and_never_blocks_the_caller() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&calls);
        let probe: SharedProbe = Arc::new(move |_: &VoiceConfig| {
            c.fetch_add(1, Ordering::SeqCst);
            // A slow (hung) daemon: the say must not wait for this.
            std::thread::sleep(Duration::from_millis(300));
            None
        });
        let m = Mock::default();
        let mut v = reprobing(&m, probe, fast(700, HOUR));
        // Within the gap of the start-up probe: no re-check.
        v.say_at("a", Urgency::Normal, 1.0).unwrap();
        wait_until(|| v.stats().spoken == 1);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        std::thread::sleep(Duration::from_millis(750));
        let t = Instant::now();
        for i in 0..5 {
            v.say_at(&format!("line {i}"), Urgency::Normal, 2.0 + f64::from(i))
                .unwrap();
        }
        assert!(t.elapsed() < Duration::from_millis(100), "say blocked");
        wait_until(|| !v.speaking());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "one probe for a burst of says"
        );
        assert_eq!(v.engine_name(), "mock");
        assert_eq!(v.stats().fallback_warnings, 1);
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
        let v = reprobing(&m, probe, fast(HOUR, 40));
        wait_until(|| calls.load(Ordering::SeqCst) >= 3);
        drop(v);
        // Dropping the voice stops the prober.
        std::thread::sleep(Duration::from_millis(100));
        let n = calls.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(calls.load(Ordering::SeqCst), n);
    }

    #[test]
    fn no_reprobe_when_the_engine_is_fixed() {
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
        assert!(!v.reprobing);
        v.set_muted(true);
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
    fn daemon_says(d: &crate::daemon::fake::FakeDaemon) -> Vec<String> {
        d.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|r| crate::daemon::request_command(r) == "say")
            .map(|r| r["text"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn late_daemon_is_picked_up_between_lines_never_mid_line() {
        let c = daemon_cfg("late-start");
        let m = Mock::default();
        m.hold.store(true, Ordering::SeqCst);
        let mut v = Voice::with_engine(c.clone(), Box::new(m.clone()), Box::new(NoLines));
        v.start_reprobe(c, Arc::new(crate::engine::ping_daemon), fast(HOUR, 60));
        // No daemon yet: the fallback speaks (and holds the line).
        v.say_at("one", Urgency::Normal, 1.0).unwrap();
        wait_until(|| m.started.lock().unwrap().len() == 1);
        v.say_at("two", Urgency::Normal, 2.0).unwrap();
        // Now the daemon starts; the timer picks it up mid-line.
        let d = say_ok_daemon("late-start");
        wait_until(|| v.engine_name() == "voiced");
        assert_eq!(
            v.voice_warning(),
            None,
            "cloned voice present: warning cleared"
        );
        // "one" is still playing on the fallback: not cut, not switched.
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(m.stops.load(Ordering::SeqCst), 0, "never switched mid-line");
        assert!(daemon_says(&d).is_empty());
        // When it ends, "two" (and later lines) go to the daemon.
        m.hold.store(false, Ordering::SeqCst);
        wait_until(|| daemon_says(&d).len() == 1);
        v.say_at("three", Urgency::Normal, 3.0).unwrap();
        wait_until(|| daemon_says(&d).len() == 2);
        assert_eq!(daemon_says(&d), ["two", "three"]);
        assert_eq!(
            *m.started.lock().unwrap(),
            ["one"],
            "fallback no longer used"
        );
        let st = v.stats();
        assert_eq!((st.fallback_warnings, st.daemon_switches), (1, 1));
    }

    #[cfg(unix)]
    #[test]
    fn a_say_rechecks_the_daemon_and_speaks_that_line_on_it() {
        let c = daemon_cfg("say-kick");
        let m = Mock::default();
        let mut v = Voice::with_engine(c.clone(), Box::new(m.clone()), Box::new(NoLines));
        v.start_reprobe(c, Arc::new(crate::engine::ping_daemon), fast(50, HOUR));
        let d = say_ok_daemon("say-kick");
        std::thread::sleep(Duration::from_millis(80));
        v.say_at("hi", Urgency::Normal, 1.0).unwrap();
        wait_until(|| daemon_says(&d) == ["hi"]);
        assert!(m.started.lock().unwrap().is_empty());
        assert_eq!(v.stats().daemon_switches, 1);
    }

    #[cfg(unix)]
    #[test]
    fn daemon_dies_then_returns_fallback_speaks_and_switch_back_between_lines() {
        let c = daemon_cfg("die-return");
        let m = Mock::default();
        let d = say_ok_daemon("die-return");
        let (client, info) = crate::engine::ping_daemon(&c).expect("up");
        let daemon = crate::engine::DaemonEngine::new(client, &info, c.preset());
        let mut v = Voice::with_engines(
            c.clone(),
            Box::new(m.clone()),
            Some(Box::new(daemon)),
            Box::new(NoLines),
        );
        v.start_reprobe(c, Arc::new(crate::engine::ping_daemon), fast(150, HOUR));
        assert_eq!(v.engine_name(), "voiced");
        assert_eq!(v.voice_warning(), None);
        v.say_at("one", Urgency::Normal, 1.0).unwrap();
        wait_until(|| daemon_says(&d) == ["one"]);
        // The daemon dies (crash: no socket owner).
        drop(d);
        v.say_at("two", Urgency::Normal, 2.0).unwrap();
        wait_until(|| *m.started.lock().unwrap() == ["two"]);
        assert_eq!(v.engine_name(), "mock");
        assert!(v.voice_warning().unwrap().contains("unreachable"));
        v.say_at("three", Urgency::Normal, 3.0).unwrap();
        wait_until(|| m.started.lock().unwrap().len() == 2);
        assert_eq!(
            *m.started.lock().unwrap(),
            ["two", "three"],
            "no line dropped"
        );
        let st = v.stats();
        // start-up was on the daemon: the only WARN is this transition.
        assert_eq!((st.fallback_warnings, st.daemon_switches), (1, 0));
        // It comes back: the next line (past the gap) re-checks and goes to it.
        let d = say_ok_daemon("die-return");
        std::thread::sleep(Duration::from_millis(200));
        m.hold.store(true, Ordering::SeqCst);
        v.say_at("four", Urgency::Normal, 4.0).unwrap();
        wait_until(|| daemon_says(&d) == ["four"]);
        assert_eq!(v.engine_name(), "voiced");
        assert_eq!(v.voice_warning(), None);
        v.say_at("five", Urgency::Normal, 5.0).unwrap();
        wait_until(|| daemon_says(&d) == ["four", "five"]);
        assert_eq!(m.started.lock().unwrap().len(), 2, "fallback idle again");
        let st = v.stats();
        assert_eq!(
            (st.fallback_warnings, st.daemon_switches),
            (1, 1),
            "one per transition"
        );
        // The safeword / stop still cuts whatever speaks.
        v.stop();
    }

    #[cfg(unix)]
    #[test]
    fn stop_cuts_the_fallback_after_a_fallover() {
        let c = daemon_cfg("die-stop");
        let m = Mock::default();
        let d = say_ok_daemon("die-stop");
        let (client, info) = crate::engine::ping_daemon(&c).expect("up");
        let daemon = crate::engine::DaemonEngine::new(client, &info, c.preset());
        let mut v = Voice::with_engines(
            c.clone(),
            Box::new(m.clone()),
            Some(Box::new(daemon)),
            Box::new(NoLines),
        );
        v.start_reprobe(c, Arc::new(crate::engine::ping_daemon), fast(HOUR, HOUR));
        drop(d);
        m.hold.store(true, Ordering::SeqCst);
        v.say_at("long one", Urgency::Normal, 1.0).unwrap();
        wait_until(|| m.started.lock().unwrap().len() == 1);
        v.stop();
        wait_until(|| m.stops.load(Ordering::SeqCst) == 1);
        wait_until(|| !v.speaking());
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
            fast(HOUR, 40),
        );
        assert_eq!(v.engine_name(), "none");
        assert!(!v.has_engine());
        assert_eq!(v.say_at("x", Urgency::Normal, 1.0), Err(Dropped::NoEngine));
        assert!(v.voice_warning().unwrap().contains("silent"));
        let d = say_ok_daemon("late-timer");
        wait_until(|| v.engine_name() == "voiced");
        assert!(v.has_engine());
        v.say_at("hello", Urgency::Normal, 2.0).unwrap();
        wait_until(|| daemon_says(&d) == ["hello"]);
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
            fast(HOUR, 20),
        );
        assert!(
            t.elapsed() < Duration::from_secs(2),
            "start-up probe bounded: {:?}",
            t.elapsed()
        );
        assert_eq!(v.engine_name(), "espeak-ng");
        assert!(v.voice_warning().unwrap().contains("espeak-ng"));
        v.set_muted(true); // don't run the (fake) espeak-ng binary
        let t = Instant::now();
        let _ = v.say_at("x", Urgency::Normal, 1.0);
        assert!(
            t.elapsed() < Duration::from_millis(50),
            "say never waits on the ping"
        );
        // The timer re-pings, times out, and she stays on the fallback.
        wait_until(|| d.commands().iter().filter(|c| *c == "ping").count() >= 2);
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
