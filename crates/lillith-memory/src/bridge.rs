//! The desktop → memory bridge, and proactive suggestions.
//!
//! [`MemoryBridge`] is what the app holds: [`MemoryBridge::observe`] is
//! called from sensor / UI threads for every event and never blocks (a
//! pure [`EventFilter`] decides in the caller, then a `try_send` on a
//! bounded channel). One dedicated thread owns its own SQLite connection,
//! learns from the admitted events, and every `proactive_interval_min`
//! asks [`Brain::predict`]; a [`Proactive`] gate decides whether a short
//! in-character line goes out through the `say` callback (the app turns it
//! into a `memory.suggest` event for the normal speech bubble).
//!
//! Privacy: only an app name (`firefox`) and the event kind are ever
//! stored; window titles, notification text and log lines are dropped.
//! With `[memory] enabled = false` there is no thread, no file, nothing.

use crate::recovery::SafeFix;
use crate::{mentions_safeword, Brain, Ctx, FixOutcome, Prediction};
use lillith_core::config::MemoryConfig;
use lillith_core::Event;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The same app focused again within this many seconds is not a new use.
pub const FOCUS_REPEAT_S: f64 = 600.0;
/// The same crash / error (kind + app) within this many seconds is one.
pub const ERROR_REPEAT_S: f64 = 60.0;
/// No suggestions this long after the safeword.
pub const SAFEWORD_QUIET_S: f64 = 1800.0;
/// No suggestions this long after any chat activity.
pub const CHAT_QUIET_S: f64 = 300.0;
/// An app used this recently is not suggested.
pub const RECENT_USE_S: f64 = 1800.0;
/// Bounded queue between the callers and the memory thread.
pub const QUEUE: usize = 64;
/// The event name the app shows suggestions with.
pub const SUGGEST_EVENT: &str = "memory.suggest";

/// `org.kde.konsole` → `konsole`, `Firefox` → `firefox`; anything that is
/// not a plain app id (spaces, empty) gives `None`.
pub fn app_name(class: &str) -> Option<String> {
    let c = class.trim();
    if c.is_empty() || c.chars().any(char::is_whitespace) {
        return None;
    }
    let c = c
        .strip_suffix(".exe")
        .or(c.strip_suffix(".EXE"))
        .unwrap_or(c);
    let last = c.rsplit(['.', '/', '\\']).find(|s| !s.is_empty())?;
    let name: String = last
        .to_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
        .take(32)
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Pure gate in front of the memory thread: sanitizes, debounces and caps.
#[derive(Debug, Clone)]
pub struct EventFilter {
    per_minute: u32,
    seen: HashMap<String, f64>,
    window: VecDeque<f64>,
}

impl EventFilter {
    pub fn new(per_minute: u32) -> Self {
        Self {
            per_minute,
            seen: HashMap::new(),
            window: VecDeque::new(),
        }
    }

    /// The sanitized event to learn from, or `None` (dropped).
    ///
    /// - `window.focused` (detail = window class) → `window.opened.app` with
    ///   only the app name; the same app again within [`FOCUS_REPEAT_S`]
    ///   (alt-tabbing back and forth) is dropped.
    /// - `app.crash`, `*.crash`, `*.error` → the same name with detail
    ///   `"<kind>: <app>"` (first word of the detail as an app id, else just
    ///   the kind); repeats within [`ERROR_REPEAT_S`] dropped. `chat.*`
    ///   errors are recorded by the chat itself, not here.
    /// - Everything else (window titles, notifications, `window.opened.*`,
    ///   which carries the caption) is dropped.
    /// - Anything carrying the safeword, and anything beyond
    ///   `per_minute` in the last 60 s, is dropped.
    pub fn admit(&mut self, ev: &Event, now: f64) -> Option<Event> {
        let detail = ev.detail.as_deref().unwrap_or("");
        if mentions_safeword(detail) || mentions_safeword(&ev.name) {
            return None;
        }
        let name = ev.name.as_str();
        if name.starts_with("window.private") {
            return None;
        }
        let (key, out) = if name == "window.focused" {
            let app = app_name(detail)?;
            let key = format!("focus:{app}");
            if self.recent(&key, now, FOCUS_REPEAT_S) {
                return None;
            }
            (key, Event::with_detail("window.opened.app", app))
        } else if !name.starts_with("chat.")
            && (name == "app.crash" || name.ends_with(".crash") || name.ends_with(".error"))
        {
            let kind: String = name
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_')
                .take(48)
                .collect();
            let app = detail.split_whitespace().next().and_then(app_name);
            let text = match &app {
                Some(a) => format!("{kind}: {a}"),
                None => kind.clone(),
            };
            let key = format!("err:{text}");
            if self.recent(&key, now, ERROR_REPEAT_S) {
                return None;
            }
            (key, Event::with_detail(kind, text))
        } else if name.starts_with("quantumsec.") {
            // Phase 2: shield / teams / deploy (detail truncated; no secrets).
            let key = format!("qsec:{name}");
            if self.recent(&key, now, 30.0) {
                return None;
            }
            let detail = detail.chars().take(160).collect::<String>();
            (
                key,
                if detail.is_empty() {
                    Event::new(name)
                } else {
                    Event::with_detail(name, detail)
                },
            )
        } else {
            return None;
        };
        while self
            .window
            .front()
            .is_some_and(|t| now - t >= 60.0 || *t > now)
        {
            self.window.pop_front();
        }
        if self.window.len() >= self.per_minute as usize {
            return None;
        }
        self.window.push_back(now);
        self.seen.insert(key, now);
        if self.seen.len() > 512 {
            self.seen
                .retain(|_, t| now - *t < FOCUS_REPEAT_S.max(ERROR_REPEAT_S));
        }
        Some(out)
    }

    fn recent(&self, key: &str, now: f64, within: f64) -> bool {
        self.seen
            .get(key)
            .is_some_and(|t| now >= *t && now - t < within)
    }
}

/// Why a check stayed quiet (tests and debug logs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quiet {
    Off,
    Safeword,
    Busy,
    HourlyCap,
    NothingConfident,
}

/// What the gate knows about "now".
#[derive(Debug, Clone, Copy, Default)]
pub struct Moment {
    pub now: f64,
    /// The safeword stop is still in effect.
    pub safeword: bool,
    /// Ko is chatting / busy (or fullscreen, when a sensor reports it).
    pub busy: bool,
}

/// Decides which prediction (if any) becomes a proactive line.
#[derive(Debug, Clone)]
pub struct Proactive {
    pub enabled: bool,
    pub min_confidence: f64,
    pub cooldown_s: f64,
    pub max_per_hour: u32,
    shown: VecDeque<f64>,
    last_by_subject: HashMap<String, f64>,
}

impl Proactive {
    pub fn from_config(cfg: &MemoryConfig) -> Self {
        Self {
            enabled: cfg.enabled && cfg.proactive,
            min_confidence: f64::from(cfg.proactive_min_confidence).clamp(0.0, 1.0),
            cooldown_s: f64::from(cfg.proactive_cooldown_min.max(0.0)) * 60.0,
            max_per_hour: cfg.proactive_max_per_hour,
            shown: VecDeque::new(),
            last_by_subject: HashMap::new(),
        }
    }

    /// The prediction to say now, or why not. `recently_used(kind, subject)`
    /// filters out what Ko is already doing. Records the pick.
    pub fn choose(
        &mut self,
        preds: &[Prediction],
        m: Moment,
        recently_used: impl Fn(&str, &str) -> bool,
    ) -> Result<Prediction, Quiet> {
        if !self.enabled {
            return Err(Quiet::Off);
        }
        if m.safeword {
            return Err(Quiet::Safeword);
        }
        if m.busy {
            return Err(Quiet::Busy);
        }
        while self
            .shown
            .front()
            .is_some_and(|t| m.now - t >= 3600.0 || *t > m.now)
        {
            self.shown.pop_front();
        }
        if self.shown.len() >= self.max_per_hour as usize {
            return Err(Quiet::HourlyCap);
        }
        let pick = preds.iter().find(|p| {
            let key = format!("{}:{}", p.kind, p.subject);
            p.confidence >= self.min_confidence
                && !recently_used(&p.kind, &p.subject)
                && !self
                    .last_by_subject
                    .get(&key)
                    .is_some_and(|t| m.now >= *t && m.now - t < self.cooldown_s)
        });
        let Some(p) = pick else {
            return Err(Quiet::NothingConfident);
        };
        self.shown.push_back(m.now);
        self.last_by_subject
            .insert(format!("{}:{}", p.kind, p.subject), m.now);
        Ok(p.clone())
    }
}

/// A short in-character line for a prediction (`salt` rotates the wording).
pub fn suggestion_line(p: &Prediction, salt: usize) -> String {
    let s: String = p
        .subject
        .chars()
        .filter(|c| c.is_alphanumeric() || " -_".contains(*c))
        .take(32)
        .collect();
    let lines: [String; 3] = match p.kind.as_str() {
        "app" => [
            format!("Mm, about now you usually open {s}. Want it, darling?"),
            format!("Isn't this your {s} hour? I keep track of you~"),
            format!("You tend to reach for {s} around this time. Just saying."),
        ],
        "request" => [
            format!("You usually ask me about {s} around now. I'm listening~"),
            format!("Time for your {s} question? I'm ready, darling."),
            format!("Mm, {s} o'clock, isn't it? Ask away."),
        ],
        _ => [
            format!("Thinking of {s}? You usually are, about now."),
            format!("{s}, right on schedule? I noticed~"),
            format!("I have a feeling it's {s} time."),
        ],
    };
    lines[salt % lines.len()].clone()
}

/// A report for an error recovery: what to try, sent back to the caller.
pub type PlanReply = Receiver<Vec<SafeFix>>;

enum Msg {
    Event(Event),
    Error {
        text: String,
        context: String,
        candidates: Vec<SafeFix>,
        reply: Option<SyncSender<Vec<SafeFix>>>,
    },
    Fix {
        text: String,
        fix: SafeFix,
        worked: bool,
    },
    CheckNow,
    Shutdown,
}

/// The app's clock (unix seconds); swappable for tests.
pub type Clock = Arc<dyn Fn() -> f64 + Send + Sync>;
/// Where proactive lines go.
pub type Say = Box<dyn Fn(String) + Send>;

/// Receives Ko's presence profile ([`Brain::presence_profile`]) when the
/// thread starts and every [`PRESENCE_EVERY_S`] after.
pub type PresenceSink = Box<dyn Fn([f32; 8]) + Send>;

/// The overlay event carrying the presence profile (detail:
/// [`presence_detail`]); the overlay's loneliness reads it.
pub const PRESENCE_EVENT: &str = "lonely.presence";

/// How often the presence profile is re-sent (s).
pub const PRESENCE_EVERY_S: f64 = 1800.0;

/// `[f32; 8]` as the [`PRESENCE_EVENT`] detail: 8 comma-separated counts.
pub fn presence_detail(p: &[f32; 8]) -> String {
    p.iter()
        .map(|v| {
            if v.is_finite() && *v >= 0.0 {
                format!("{v}")
            } else {
                "0".into()
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}
/// Opens the thread's own brain (on the thread).
pub type BrainFactory = Box<dyn FnOnce() -> Option<Brain> + Send>;

/// Knobs for [`MemoryBridge::spawn_with`] (tests swap clock / brain / stop epoch).
pub struct BridgeOptions {
    pub cfg: MemoryConfig,
    pub clock: Clock,
    pub open: BrainFactory,
    /// The process-wide safeword epoch ([`lillith_core::access::kill_switch::hush_epoch`]:
    /// the safeword or a stop).
    pub stop_epoch: Arc<dyn Fn() -> u64 + Send + Sync>,
    pub say: Say,
    /// Where the presence profile goes (`None`: not computed).
    pub presence: Option<PresenceSink>,
}

impl BridgeOptions {
    /// Real clock, the configured DB file, the real kill switch.
    pub fn new(cfg: &MemoryConfig, say: Say) -> Self {
        let c = cfg.clone();
        Self {
            cfg: cfg.clone(),
            clock: Arc::new(unix_now),
            open: Box::new(move || match Brain::open(&c) {
                Ok(b) => Some(b),
                Err(e) => {
                    tracing::warn!("memory bridge: could not open the memory db: {e}");
                    None
                }
            }),
            stop_epoch: Arc::new(lillith_core::access::kill_switch::hush_epoch),
            say,
            presence: None,
        }
    }
}

fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or_default()
}

/// Shared between the callers and the thread (lock-free timestamps).
#[derive(Default)]
struct Signals {
    /// f64 bits of the last chat activity (0 = never).
    last_chat: AtomicU64,
    /// f64 bits of the last safeword seen on the feed.
    last_safeword: AtomicU64,
    /// f64 bits of "busy until" (fullscreen etc.).
    busy_until: AtomicU64,
}

fn load(a: &AtomicU64) -> f64 {
    f64::from_bits(a.load(Ordering::Relaxed))
}

fn store(a: &AtomicU64, v: f64) {
    a.store(v.to_bits(), Ordering::Relaxed);
}

/// See the module docs. Cheap to call from any thread.
pub struct MemoryBridge {
    tx: Option<SyncSender<Msg>>,
    filter: Mutex<EventFilter>,
    signals: Arc<Signals>,
    clock: Clock,
    dropped: AtomicU64,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl MemoryBridge {
    /// Starts the bridge for `cfg` (real clock and DB). Disabled memory →
    /// no thread and no file.
    pub fn spawn(cfg: &MemoryConfig, say: Say) -> Self {
        Self::spawn_with(BridgeOptions::new(cfg, say))
    }

    /// A bridge that does nothing (memory off).
    pub fn disabled() -> Self {
        Self {
            tx: None,
            filter: Mutex::new(EventFilter::new(0)),
            signals: Arc::default(),
            clock: Arc::new(unix_now),
            dropped: AtomicU64::new(0),
            thread: None,
        }
    }

    pub fn spawn_with(o: BridgeOptions) -> Self {
        if !o.cfg.enabled {
            return Self::disabled();
        }
        let (tx, rx) = sync_channel(QUEUE);
        let signals: Arc<Signals> = Arc::default();
        let worker = Worker {
            proactive: Proactive::from_config(&o.cfg),
            // Read here, on the caller, so a stop right after spawn counts.
            epoch: (o.stop_epoch)(),
            interval_s: f64::from(o.cfg.proactive_interval_min.max(1.0)) * 60.0,
            clock: o.clock.clone(),
            stop_epoch: o.stop_epoch,
            say: o.say,
            presence: o.presence,
            signals: signals.clone(),
            used: HashMap::new(),
            salt: 0,
        };
        let open = o.open;
        let thread = std::thread::Builder::new()
            .name("lillith-memory".into())
            .spawn(move || {
                let Some(brain) = open().filter(Brain::is_enabled) else {
                    // Drain so callers never block (they only try_send anyway).
                    for m in rx.iter() {
                        if let Msg::Error {
                            candidates,
                            reply: Some(r),
                            ..
                        } = m
                        {
                            let _ = r.try_send(candidates);
                        }
                    }
                    return;
                };
                worker.run(&brain, rx);
            })
            .ok();
        Self {
            tx: thread.is_some().then_some(tx),
            filter: Mutex::new(EventFilter::new(o.cfg.events_per_minute)),
            signals,
            clock: o.clock,
            dropped: AtomicU64::new(0),
            thread,
        }
    }

    /// Whether the memory thread is running.
    pub fn is_enabled(&self) -> bool {
        self.tx.is_some()
    }

    /// Feeds one desktop / app event. Never blocks: filtered here, then a
    /// `try_send` (a full queue drops it). `chat.*` events only mark chat
    /// activity (and the safeword); they are never stored.
    pub fn observe(&self, ev: &Event) {
        let Some(tx) = &self.tx else { return };
        let now = (self.clock)();
        if ev.name.starts_with("chat.") {
            store(&self.signals.last_chat, now);
            if ev.name == "chat.safeword" {
                store(&self.signals.last_safeword, now);
            }
            return;
        }
        let admitted = match self.filter.lock() {
            Ok(mut f) => f.admit(ev, now),
            Err(_) => None,
        };
        if let Some(ev) = admitted {
            self.send(tx, Msg::Event(ev));
        }
    }

    /// Marks Ko as busy (fullscreen, presenting) for `secs`.
    pub fn set_busy_for(&self, secs: f64) {
        store(&self.signals.busy_until, (self.clock)() + secs.max(0.0));
    }

    /// Records a failure and asks which safe fixes to try, best first. The
    /// answer arrives on the returned channel (poll it with `try_recv`);
    /// with memory off it is the caller's order, immediately.
    pub fn report_error(&self, text: &str, context: &str, candidates: &[SafeFix]) -> PlanReply {
        let (rtx, rrx) = sync_channel(1);
        match &self.tx {
            None => {
                let _ = rtx.try_send(candidates.to_vec());
            }
            Some(tx) => {
                let m = Msg::Error {
                    text: text.to_string(),
                    context: context.to_string(),
                    candidates: candidates.to_vec(),
                    reply: Some(rtx.clone()),
                };
                if !self.send(tx, m) {
                    let _ = rtx.try_send(candidates.to_vec());
                }
            }
        }
        rrx
    }

    /// Records whether `fix` worked for the error `text`.
    pub fn report_fix(&self, text: &str, fix: SafeFix, worked: bool) {
        if let Some(tx) = &self.tx {
            self.send(
                tx,
                Msg::Fix {
                    text: text.to_string(),
                    fix,
                    worked,
                },
            );
        }
    }

    /// Runs a proactive check now (tests, "what do I usually do?").
    pub fn check_now(&self) {
        if let Some(tx) = &self.tx {
            self.send(tx, Msg::CheckNow);
        }
    }

    /// Events dropped because the queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn send(&self, tx: &SyncSender<Msg>, m: Msg) -> bool {
        match tx.try_send(m) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Stops the thread after it drained the queue (also on drop).
    pub fn shutdown(&mut self) {
        if let Some(tx) = self.tx.take() {
            // Blocking send is fine here: shutdown is not on a UI path.
            let _ = tx.send(Msg::Shutdown);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for MemoryBridge {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct Worker {
    proactive: Proactive,
    interval_s: f64,
    clock: Clock,
    stop_epoch: Arc<dyn Fn() -> u64 + Send + Sync>,
    epoch: u64,
    say: Say,
    presence: Option<PresenceSink>,
    signals: Arc<Signals>,
    /// Last time each app was used (from admitted focus events).
    used: HashMap<String, f64>,
    salt: usize,
}

impl Worker {
    fn run(mut self, brain: &Brain, rx: Receiver<Msg>) {
        let mut stopped_at = f64::NEG_INFINITY;
        let mut next_check = (self.clock)() + self.interval_s;
        let mut next_presence = f64::NEG_INFINITY;
        loop {
            let now = (self.clock)();
            if let Some(p) = &self.presence {
                if now >= next_presence {
                    next_presence = now + PRESENCE_EVERY_S;
                    p(brain.presence_profile(&Ctx::at(now)));
                }
            }
            let wait = (next_check.min(next_presence) - now).clamp(0.05, 60.0);
            let msg = match rx.recv_timeout(Duration::from_secs_f64(wait)) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            };
            let now = (self.clock)();
            let e = (self.stop_epoch)();
            if e != self.epoch {
                self.epoch = e;
                stopped_at = now;
            }
            let check = match msg {
                Some(Msg::Shutdown) => return,
                Some(Msg::Event(ev)) => {
                    if ev.name == "window.opened.app" {
                        if let Some(app) = &ev.detail {
                            self.used.insert(app.clone(), now);
                        }
                    }
                    brain.observe_event(&ev, &Ctx::at(now));
                    false
                }
                Some(Msg::Error {
                    text,
                    context,
                    candidates,
                    reply,
                }) => {
                    let plan = crate::recovery::begin(Some(brain), &text, &context, &candidates);
                    if let Some(r) = reply {
                        let _ = r.try_send(plan);
                    }
                    false
                }
                Some(Msg::Fix { text, fix, worked }) => {
                    let outcome = if worked {
                        FixOutcome::Worked
                    } else {
                        FixOutcome::Failed
                    };
                    if let Err(e) = brain.record_fix(&text, fix.as_str(), outcome, &Ctx::at(now)) {
                        tracing::warn!("memory: record_fix failed: {e}");
                    }
                    false
                }
                Some(Msg::CheckNow) => true,
                None => now >= next_check,
            };
            if check {
                next_check = now + self.interval_s;
                let safeword = now - stopped_at < SAFEWORD_QUIET_S
                    || now - load(&self.signals.last_safeword) < SAFEWORD_QUIET_S;
                let busy = now - load(&self.signals.last_chat) < CHAT_QUIET_S
                    || now < load(&self.signals.busy_until);
                self.check(
                    brain,
                    Moment {
                        now,
                        safeword,
                        busy,
                    },
                );
            }
        }
    }

    fn check(&mut self, brain: &Brain, m: Moment) {
        // Cheap exits first: no DB read while quiet anyway.
        if !self.proactive.enabled || m.safeword || m.busy {
            tracing::debug!(safeword = m.safeword, busy = m.busy, "proactive: quiet");
            return;
        }
        let preds = brain.predict(&Ctx::at(m.now), 5);
        let used = &self.used;
        match self.proactive.choose(&preds, m, |kind, subject| {
            kind == "app"
                && used
                    .get(subject)
                    .is_some_and(|t| m.now >= *t && m.now - t < RECENT_USE_S)
        }) {
            Ok(p) => {
                self.salt = self.salt.wrapping_add(1);
                let line = suggestion_line(&p, self.salt);
                tracing::info!(
                    target: "lillith_memory::proactive",
                    kind = %p.kind,
                    confidence = p.confidence,
                    "proactive suggestion"
                );
                (self.say)(line);
            }
            Err(q) => tracing::debug!(?q, "proactive: nothing to say"),
        }
    }
}

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod tests;
