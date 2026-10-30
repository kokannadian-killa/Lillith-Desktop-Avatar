//! Wires the desktop to the memory bridge ([`lillith_memory::MemoryBridge`]):
//! a feed tee (every monitor / chat event is also offered to memory, without
//! ever blocking the sender), proactive lines back into the overlay feed as
//! `memory.suggest`, and the voice's error recovery ladder.

use lillith_core::Event;
use lillith_memory::{MemoryBridge, SafeFix};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Starts the bridge for `[memory]`; proactive lines go into `feed` as
/// [`lillith_memory::bridge::SUGGEST_EVENT`] (shown by the normal speech
/// bubble rule, dropped while the monitor is paused).
pub fn start(cfg: &lillith_core::config::MemoryConfig, feed: Sender<Event>) -> Arc<MemoryBridge> {
    let presence_feed = std::sync::Mutex::new(feed.clone());
    let feed = std::sync::Mutex::new(feed);
    let mut opts = lillith_memory::bridge::BridgeOptions::new(
        cfg,
        Box::new(move |line| {
            if let Ok(f) = feed.lock() {
                let _ = f.send(Event::with_detail(
                    lillith_memory::bridge::SUGGEST_EVENT,
                    line,
                ));
            }
        }),
    );
    // Ko's "usually around" hours (read-only from memory events) for the
    // overlay's loneliness time-of-day factor.
    opts.presence = Some(Box::new(move |p| {
        if let Ok(f) = presence_feed.lock() {
            let _ = f.send(presence_event(&p));
        }
    }));
    let bridge = MemoryBridge::spawn_with(opts);
    tracing::info!(
        on = bridge.is_enabled(),
        proactive = cfg.enabled && cfg.proactive,
        "memory bridge"
    );
    Arc::new(bridge)
}

/// The `lonely.presence` overlay event for a presence profile.
pub fn presence_event(p: &[f32; 8]) -> Event {
    Event::with_detail(
        lillith_memory::bridge::PRESENCE_EVENT,
        lillith_memory::bridge::presence_detail(p),
    )
}

/// A sender that forwards everything to `feed` and offers each event to
/// memory on the way (filtering happens in [`MemoryBridge::observe`], which
/// never blocks). Memory off: `feed` itself, no thread.
pub fn tee(feed: Sender<Event>, bridge: &Arc<MemoryBridge>) -> Sender<Event> {
    if !bridge.is_enabled() {
        return feed;
    }
    let (tx, rx) = std::sync::mpsc::channel::<Event>();
    let b = bridge.clone();
    let out = feed.clone();
    let spawned = std::thread::Builder::new()
        .name("lillith-memory-tee".into())
        .spawn(move || {
            for ev in rx {
                b.observe(&ev);
                if out.send(ev).is_err() {
                    return;
                }
            }
        });
    match spawned {
        Ok(_) => tx,
        Err(e) => {
            tracing::warn!("memory tee: {e}; memory will not see desktop events");
            feed
        }
    }
}

/// The voice recovery steps' effect on the sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceStep {
    /// Speak normally.
    Speak,
    /// Backoff: drop spoken lines for now (text bubbles still show).
    Hold,
    /// Turn speakers off for this run (not saved): text only.
    TextOnly,
}

/// How long the voice rests after an engine error before it tries again.
pub const VOICE_BACKOFF: Duration = Duration::from_secs(30);
const PLAN_WAIT: Duration = Duration::from_secs(2);
const CANDIDATES: [SafeFix; 2] = [SafeFix::RetryBackoff, SafeFix::TextOnly];

#[derive(Debug)]
enum St {
    Idle,
    Planning(Receiver<Vec<SafeFix>>, Instant),
    Backoff(Vec<SafeFix>, Instant),
    /// Retrying: errors / spoken counters when the retry began.
    Trying(Vec<SafeFix>, u64, u64),
    TextOnly,
}

/// Engine errors → `record_error("voice: engine error (<engine>)")`, then
/// the remembered order of [`SafeFix::RetryBackoff`] (rest
/// [`VOICE_BACKOFF`], then speak again: worked when a line is spoken
/// without a new error) and [`SafeFix::TextOnly`] (speakers off for this
/// run; always "worked"). Each outcome goes back to memory.
#[derive(Debug)]
pub struct VoiceRecovery {
    signature: String,
    errors: u64,
    spoken: u64,
    st: St,
}

impl VoiceRecovery {
    pub fn new(engine: &str, errors: u64, spoken: u64) -> Self {
        Self {
            signature: format!("voice: engine error ({engine})"),
            errors,
            spoken,
            st: St::Idle,
        }
    }

    /// The voice switched engines at runtime (e.g. the daemon came up):
    /// later errors are remembered under the live engine's name.
    pub fn set_engine(&mut self, engine: &str) {
        self.signature = format!("voice: engine error ({engine})");
    }

    /// Speakers were switched on again by Ko: start over.
    pub fn reset(&mut self, errors: u64, spoken: u64) {
        self.errors = errors;
        self.spoken = spoken;
        self.st = St::Idle;
    }

    /// Advances with the voice's current counters.
    pub fn poll(
        &mut self,
        errors: u64,
        spoken: u64,
        now: Instant,
        mem: &MemoryBridge,
    ) -> VoiceStep {
        let new_error = errors > self.errors;
        self.errors = errors;
        self.spoken = spoken;
        loop {
            let st = std::mem::replace(&mut self.st, St::Idle);
            let (next, step) = match st {
                St::Idle if new_error => {
                    tracing::warn!(signature = %self.signature, "voice failed; recovering");
                    let rx = mem.report_error(&self.signature, "voice", &CANDIDATES);
                    (St::Planning(rx, now), None)
                }
                St::Idle => (St::Idle, Some(VoiceStep::Speak)),
                St::Planning(rx, since) => match rx.try_recv() {
                    Ok(plan) => (self.apply(plan, now, mem), None),
                    Err(TryRecvError::Empty) if now.duration_since(since) < PLAN_WAIT => {
                        (St::Planning(rx, since), Some(VoiceStep::Hold))
                    }
                    Err(_) => (self.apply(CANDIDATES.to_vec(), now, mem), None),
                },
                St::Backoff(plan, until) if now < until => {
                    (St::Backoff(plan, until), Some(VoiceStep::Hold))
                }
                St::Backoff(plan, _) => (
                    St::Trying(plan, self.errors, self.spoken),
                    Some(VoiceStep::Speak),
                ),
                St::Trying(mut plan, e0, s0) => {
                    if self.errors > e0 {
                        mem.report_fix(&self.signature, SafeFix::RetryBackoff, false);
                        plan.retain(|f| *f != SafeFix::RetryBackoff);
                        (self.apply(plan, now, mem), None)
                    } else if self.spoken > s0 {
                        mem.report_fix(&self.signature, SafeFix::RetryBackoff, true);
                        tracing::info!("voice recovered after a rest");
                        (St::Idle, Some(VoiceStep::Speak))
                    } else {
                        (St::Trying(plan, e0, s0), Some(VoiceStep::Speak))
                    }
                }
                St::TextOnly => (St::TextOnly, Some(VoiceStep::TextOnly)),
            };
            self.st = next;
            if let Some(s) = step {
                return s;
            }
        }
    }

    fn apply(&self, plan: Vec<SafeFix>, now: Instant, mem: &MemoryBridge) -> St {
        match plan.first() {
            Some(SafeFix::RetryBackoff) => St::Backoff(plan, now + VOICE_BACKOFF),
            Some(SafeFix::TextOnly) => {
                tracing::warn!("voice keeps failing: speakers off for this session (text only)");
                mem.report_fix(&self.signature, SafeFix::TextOnly, true);
                St::TextOnly
            }
            // Nothing (or nothing safe) left: give up quietly, keep speaking.
            _ => St::Idle,
        }
    }
}

/// Records a failure that has no automatic fix (setup, llama-server spawn,
/// sensor crashes come in through the feed) so it is remembered and its
/// recurrences counted.
pub fn record_failure(mem: &MemoryBridge, text: &str, context: &str) {
    let _ = mem.report_error(text, context, &[]);
}

/// `lillith setup` failed: remember it (opens the memory DB directly; the
/// overlay is not running). Memory off: nothing.
pub fn record_setup_failure(cfg: &lillith_core::config::MemoryConfig, err: &str) {
    if !cfg.enabled {
        return;
    }
    match lillith_memory::Brain::open(cfg) {
        Ok(b) => {
            let text: String = format!("setup: {err}").chars().take(200).collect();
            let _ = lillith_memory::recovery::begin(Some(&b), &text, "setup", &[]);
        }
        Err(e) => tracing::debug!("setup failure not remembered: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lillith_core::config::MemoryConfig;
    use lillith_memory::bridge::BridgeOptions;

    fn mem() -> MemoryBridge {
        let cfg = MemoryConfig::default();
        let mut o = BridgeOptions::new(&cfg, Box::new(|_| {}));
        let c = cfg.clone();
        o.open = Box::new(move || lillith_memory::Brain::open_in_memory(&c).ok());
        MemoryBridge::spawn_with(o)
    }

    fn settle(r: &mut VoiceRecovery, e: u64, s: u64, now: Instant, m: &MemoryBridge) -> VoiceStep {
        // The plan arrives from the memory thread: poll until it is applied.
        for _ in 0..100 {
            let step = r.poll(e, s, now, m);
            if !matches!(r.st, St::Planning(..)) {
                return step;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("no plan");
    }

    #[test]
    fn presence_event_is_the_one_the_overlay_reads() {
        let p = [0.0, 0.0, 1.0, 4.0, 2.0, 6.0, 9.0, 3.0];
        let ev = presence_event(&p);
        assert_eq!(ev.name, lillith_overlay::run::LONELY_PRESENCE);
        assert_eq!(
            lillith_overlay::run::parse_presence(ev.detail.as_deref()),
            Some(p)
        );
    }

    #[test]
    fn voice_rests_then_learns_that_text_only_is_what_works() {
        let m = mem();
        let t = Instant::now();
        let mut r = VoiceRecovery::new("piper", 0, 0);
        assert_eq!(r.poll(0, 0, t, &m), VoiceStep::Speak);
        // An engine error: rest first.
        assert_eq!(settle(&mut r, 1, 0, t, &m), VoiceStep::Hold);
        assert_eq!(
            r.poll(1, 0, t + Duration::from_secs(5), &m),
            VoiceStep::Hold
        );
        let t1 = t + VOICE_BACKOFF + Duration::from_secs(1);
        assert_eq!(r.poll(1, 0, t1, &m), VoiceStep::Speak);
        // It fails again: text only.
        assert_eq!(r.poll(2, 0, t1, &m), VoiceStep::TextOnly);
        assert_eq!(r.poll(2, 0, t1, &m), VoiceStep::TextOnly);
        // Next session: memory says text-only worked, retry failed.
        let mut r = VoiceRecovery::new("piper", 0, 0);
        assert_eq!(settle(&mut r, 1, 0, t, &m), VoiceStep::TextOnly);
    }

    #[test]
    fn a_successful_retry_is_remembered_first() {
        let m = mem();
        let t = Instant::now();
        let mut r = VoiceRecovery::new("espeak", 3, 10);
        assert_eq!(settle(&mut r, 4, 10, t, &m), VoiceStep::Hold);
        let t1 = t + VOICE_BACKOFF;
        assert_eq!(r.poll(4, 10, t1, &m), VoiceStep::Speak);
        assert_eq!(r.poll(4, 11, t1, &m), VoiceStep::Speak);
        assert!(matches!(r.st, St::Idle));
        // Back to normal; a later error rests again (retry is the known fix).
        assert_eq!(settle(&mut r, 5, 11, t1, &m), VoiceStep::Hold);
    }

    #[test]
    fn set_engine_tracks_the_live_engine_name() {
        let mut r = VoiceRecovery::new("espeak-ng", 0, 0);
        assert_eq!(r.signature, "voice: engine error (espeak-ng)");
        r.set_engine("voiced");
        assert_eq!(r.signature, "voice: engine error (voiced)");
        // Only the name changes; the ladder still starts idle.
        assert!(matches!(r.st, St::Idle));
    }

    #[test]
    fn memory_off_still_recovers_in_the_default_order() {
        let m = MemoryBridge::disabled();
        let t = Instant::now();
        let mut r = VoiceRecovery::new("piper", 0, 0);
        assert_eq!(settle(&mut r, 1, 0, t, &m), VoiceStep::Hold);
        let t1 = t + VOICE_BACKOFF;
        assert_eq!(r.poll(1, 0, t1, &m), VoiceStep::Speak);
        assert_eq!(r.poll(2, 0, t1, &m), VoiceStep::TextOnly);
    }

    #[test]
    fn tee_forwards_everything_and_memory_off_is_a_plain_feed() {
        let off = Arc::new(MemoryBridge::disabled());
        let (tx, rx) = std::sync::mpsc::channel();
        let t = tee(tx, &off);
        t.send(Event::new("cpu.high")).unwrap();
        assert_eq!(rx.recv().unwrap().name, "cpu.high");

        let on = Arc::new(mem());
        let (tx, rx) = std::sync::mpsc::channel();
        let t = tee(tx, &on);
        for n in ["window.focused", "chat.user", "cpu.high"] {
            t.send(Event::with_detail(n, "x")).unwrap();
        }
        let got: Vec<String> = (0..3)
            .map(|_| rx.recv_timeout(Duration::from_secs(2)).unwrap().name)
            .collect();
        assert_eq!(got, ["window.focused", "chat.user", "cpu.high"]);
    }
}
