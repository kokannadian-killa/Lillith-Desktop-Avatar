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

use crate::config::VoiceConfig;
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
    engine_name: &'static str,
    voice_warning: Option<String>,
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    gate: Gate,
    lines: Box<dyn LineSource>,
    t0: Instant,
}

impl std::fmt::Debug for Voice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Voice")
            .field("engine", &self.engine_name)
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
        Self::from_config_with(cfg, &crate::engine::which, &crate::engine::ping_daemon)
    }

    fn from_config_with(
        cfg: VoiceConfig,
        lookup: &dyn Fn(&str) -> Option<std::path::PathBuf>,
        probe: &crate::engine::DaemonProbe,
    ) -> Self {
        // Probe even when the speakers are off so the live toggle can switch
        // them on later without a restart; `enabled` stays as configured.
        let probe_cfg = VoiceConfig {
            enabled: true,
            ..cfg.clone()
        };
        let d = crate::engine::detect_full(&probe_cfg, lookup, probe);
        if d.engine.is_real() && !cfg.enabled {
            tracing::info!("voice: {} (speakers off until toggled on)", d.reason);
        } else if d.engine.is_real() {
            tracing::info!("voice: {}", d.reason);
        } else {
            tracing::info!("voice off: {}", d.reason);
        }
        Self::with_engine(cfg, d.engine, Box::new(DefaultLines::default()))
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
        });
        let engine_name = engine.name();
        let voice_warning = engine.voice_warning();
        // Spawned even while disabled so the speakers switch works live.
        let worker = engine.is_real().then(|| {
            let sh = Arc::clone(&shared);
            let opts = SpeakOpts::from_config(&cfg);
            std::thread::Builder::new()
                .name("lillith-voice".into())
                .spawn(move || worker(sh, engine, opts))
                .expect("spawn voice worker")
        });
        Self {
            gate: Gate::new(cfg.cooldown_s, cfg.repeat_cooldown_s),
            cfg,
            engine_name,
            voice_warning,
            shared,
            worker,
            lines,
            t0: Instant::now(),
        }
    }

    pub fn engine_name(&self) -> &'static str {
        self.engine_name
    }
    /// Degraded-voice notice for the UI, e.g. "using fallback voice p1
    /// (cloned 'lillith' voice unavailable)". `None` when all is well.
    pub fn voice_warning(&self) -> Option<&str> {
        self.voice_warning.as_deref()
    }
    /// `true` if lines can actually be heard (enabled + a real engine).
    pub fn is_active(&self) -> bool {
        self.worker.is_some() && self.enabled()
    }
    /// `true` if a real TTS engine was found (the speakers switch can work).
    pub fn has_engine(&self) -> bool {
        self.worker.is_some()
    }
    /// "Speak out loud (speakers)": `[voice] enabled`, live.
    pub fn enabled(&self) -> bool {
        self.shared.enabled.load(Ordering::SeqCst)
    }

    /// Live speakers switch. Off cuts the current line and clears the
    /// queue; text elsewhere (speech bubble, chat) is unaffected.
    pub fn set_enabled(&mut self, on: bool) {
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
        let mut v = Voice::from_config_with(c, &lookup, &|_| None);
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
}
