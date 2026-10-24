//! Talk button for the chat bubble through Lillith's local voice daemon
//! (`listen action:start|stop`, protocol v2). Loopback only: the daemon is
//! a Unix socket / named pipe on this machine.
//!
//! The button is a **toggle**, because every press needs its own Yes on the
//! daemon's mic card (no session grant) and clicking that card means the
//! button is no longer held:
//! 1. first click: `listen start` (the card shows; the mic opens on Yes)
//!    ([`MicState::Waiting`], then [`MicState::Listening`]);
//! 2. second click (or the safeword / closing the bubble): `listen stop`.
//!    While the card is still up this cancels it (the daemon closes it,
//!    audits `CANCELLED`, and the mic never opens for that press); once
//!    listening it closes the mic and returns the transcript.
//!
//! `listen stop` and the safeword `stop` never wait behind a pending
//! `listen start`: start, stop and hush each run on their own thread. At
//! most one `listen start` is in flight.
//!
//! What a transcript may do is decided by
//! [`lillith_voice::daemon::route_transcript`]: the safeword stops
//! everything, other words only fill the chat input, and nothing heard can
//! ever answer a Soul Contract.

use lillith_voice::daemon::{DaemonClient, Transcript};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

/// What the mic threads report back to the bubble.
#[derive(Debug, Clone, PartialEq)]
pub enum MicEvent {
    /// The daemon answered the probe: show the mic button.
    Ready,
    /// No daemon (or STT off): no button.
    Unavailable,
    /// `listen start` succeeded: the card was answered Yes and the mic is
    /// open (the daemon already cut her voice: barge-in).
    Listening,
    /// `listen start` ended without opening the mic (card No / timeout /
    /// closed, no card UI, daemon error). Not reported as a failure when the
    /// press was already cancelled.
    StartFailed(String),
    /// `listen stop` returned: what was heard (empty when it only cancelled
    /// the card).
    Heard(Transcript),
    /// A request failed (shown as a short note).
    Failed(String),
}

#[derive(Debug, PartialEq)]
pub(crate) enum MicCmd {
    Start,
    Stop,
    /// Stop her voice (`stop` with this reason).
    Hush(String),
}

/// The talk button's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicState {
    /// Nothing open: a click starts a press.
    Idle,
    /// `listen start` sent: the daemon's mic card is waiting for Ko's Yes.
    /// A click cancels it.
    Waiting,
    /// The mic is open: a click stops it and transcribes.
    Listening,
    /// `listen stop` sent: waiting for the transcript (or the cancel).
    Busy,
}

/// The bubble's handle on the mic threads.
pub struct Mic {
    cmd: Sender<MicCmd>,
    rx: Receiver<MicEvent>,
    /// The daemon answered: the button shows.
    pub ready: bool,
    /// Where the press is.
    pub state: MicState,
    /// A `listen start` reply is still outstanding (no new start until it
    /// comes back, even after a cancel).
    start_inflight: bool,
    /// The `listen stop` replies still outstanding, oldest first (they come
    /// back in order: one stop thread); `true` = throw that transcript away
    /// (the safeword closed the mic, or it opened after its press ended).
    stops: std::collections::VecDeque<bool>,
}

impl std::fmt::Debug for Mic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mic")
            .field("ready", &self.ready)
            .field("state", &self.state)
            .field("start_inflight", &self.start_inflight)
            .field("stops", &self.stops)
            .finish()
    }
}

/// `[voice]` of `config_path` (else the default config path); defaults when
/// the file is missing or broken.
pub fn load_voice_config(config_path: Option<&std::path::Path>) -> lillith_voice::VoiceConfig {
    config_path
        .map(std::path::Path::to_path_buf)
        .or_else(lillith_core::config::Config::default_path)
        .filter(|p| p.exists())
        .and_then(|p| match lillith_voice::VoiceConfig::load(&p) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("[voice] in {}: {e}", p.display());
                None
            }
        })
        .unwrap_or_default()
}

impl Drop for Mic {
    fn drop(&mut self) {
        // Bubble closed mid press: cancel the card / close the mic.
        if matches!(self.state, MicState::Waiting | MicState::Listening) {
            let _ = self.cmd.send(MicCmd::Stop);
        }
    }
}

/// One worker thread serving `rx` with `f`, one request after another.
fn worker<T: Send + 'static>(name: &str, rx: Receiver<T>, f: impl Fn(T) + Send + 'static) {
    let spawned = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            while let Ok(x) = rx.recv() {
                f(x);
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("{name} thread: {e}");
    }
}

impl Mic {
    /// From the `[voice]` section (`daemon_socket`, `[voice.stt]`) of the
    /// config file the app runs with (`--config`; `None` = the default
    /// path). `None` when STT is switched off or no local endpoint resolves.
    pub fn from_config(
        config_path: Option<&std::path::Path>,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Option<Self> {
        Self::from_voice_config(&load_voice_config(config_path), repaint)
    }

    /// [`Mic::from_config`] for an already loaded `[voice]`.
    pub fn from_voice_config(
        cfg: &lillith_voice::VoiceConfig,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Option<Self> {
        if !cfg.stt.enabled {
            return None;
        }
        let client = DaemonClient::from_config(cfg)?;
        let timeout = Duration::from_secs_f32(cfg.stt.timeout_s);
        Some(Self::spawn(client, timeout, repaint))
    }

    /// Starts the mic threads: a router (pings `client`, then hands each
    /// command to its own worker at once, never blocking), and one worker
    /// each for `listen start`, `listen stop` and `stop` (hush), so a stop
    /// is never queued behind a start that waits for the card.
    pub fn spawn(
        client: DaemonClient,
        timeout: Duration,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (cmd, cmd_rx) = channel::<MicCmd>();
        let (tx, rx) = channel::<MicEvent>();
        let repaint = std::sync::Arc::new(repaint);
        let sender = move |tx: &Sender<MicEvent>| {
            let (tx, repaint) = (tx.clone(), std::sync::Arc::clone(&repaint));
            move |e: MicEvent| {
                let _ = tx.send(e);
                repaint();
            }
        };
        let (send_r, send_a, send_o) = (sender(&tx), sender(&tx), sender(&tx));
        let spawned = std::thread::Builder::new()
            .name("lillith-mic".into())
            .spawn(move || {
                match client.ping() {
                    Ok(_) => send_r(MicEvent::Ready),
                    Err(e) => {
                        tracing::debug!("voice daemon not available for the talk button: {e}");
                        send_r(MicEvent::Unavailable);
                        return;
                    }
                }
                let (start_tx, start_rx) = channel::<()>();
                let (stop_tx, stop_rx) = channel::<()>();
                let (hush_tx, hush_rx) = channel::<String>();
                let c = client.clone();
                worker("lillith-mic-start", start_rx, move |()| {
                    match c.listen_start() {
                        Ok(()) => send_a(MicEvent::Listening),
                        Err(e) => send_a(MicEvent::StartFailed(format!("mic: {e}"))),
                    }
                });
                let c = client.clone();
                worker("lillith-mic-stop", stop_rx, move |()| {
                    match c.listen_stop(timeout) {
                        Ok(t) => send_o(MicEvent::Heard(t)),
                        Err(e) => send_o(MicEvent::Failed(format!("mic: {e}"))),
                    }
                });
                let c = client;
                worker("lillith-mic-hush", hush_rx, move |reason: String| {
                    if let Err(e) = c.stop(Some(&reason)) {
                        tracing::debug!("voice daemon stop: {e}");
                    }
                });
                // Forwarding never blocks, so a stop overtakes a start that
                // is still waiting for the card.
                while let Ok(m) = cmd_rx.recv() {
                    let _ = match m {
                        MicCmd::Start => start_tx.send(()),
                        MicCmd::Stop => stop_tx.send(()),
                        MicCmd::Hush(r) => {
                            hush_tx.send(r).map_err(|_| std::sync::mpsc::SendError(()))
                        }
                    };
                }
            });
        if let Err(e) = spawned {
            tracing::warn!("mic thread: {e}");
        }
        Self::with_channels(cmd, rx)
    }

    fn with_channels(cmd: Sender<MicCmd>, rx: Receiver<MicEvent>) -> Self {
        Self {
            cmd,
            rx,
            ready: false,
            state: MicState::Idle,
            start_inflight: false,
            stops: std::collections::VecDeque::new(),
        }
    }

    /// Mic handle wired to channels instead of a daemon thread (tests).
    #[cfg(test)]
    pub(crate) fn for_test() -> (Self, Receiver<MicCmd>, Sender<MicEvent>) {
        let (cmd, cmd_rx) = std::sync::mpsc::channel();
        let (tx, rx) = std::sync::mpsc::channel();
        (Self::with_channels(cmd, rx), cmd_rx, tx)
    }

    /// A press is under way (card waiting or mic open).
    pub fn active(&self) -> bool {
        matches!(self.state, MicState::Waiting | MicState::Listening)
    }

    /// Whether a click would start a new press now.
    pub fn can_start(&self) -> bool {
        self.ready && self.state == MicState::Idle && !self.start_inflight && self.stops.is_empty()
    }

    /// The talk button was clicked: start a press when idle, else stop the
    /// current one (cancel its card, or close the mic). Returns whether a
    /// new press started (the bubble then signals barge-in).
    pub fn toggle(&mut self) -> bool {
        match self.state {
            MicState::Idle if self.can_start() => {
                self.state = MicState::Waiting;
                self.start_inflight = true;
                let _ = self.cmd.send(MicCmd::Start);
                true
            }
            MicState::Waiting | MicState::Listening => {
                self.stop();
                false
            }
            _ => false,
        }
    }

    /// Ends the press now: `listen stop` (a waiting card is cancelled, an
    /// open mic closes). Never queued behind a pending start.
    pub fn stop(&mut self) {
        self.stop_as(false);
    }

    fn stop_as(&mut self, discard: bool) {
        if self.active() {
            self.state = MicState::Busy;
            self.send_stop(discard);
        }
    }

    fn send_stop(&mut self, discard: bool) {
        self.stops.push_back(discard);
        let _ = self.cmd.send(MicCmd::Stop);
    }

    /// The safeword: her voice stops (`stop` with `reason`, which also makes
    /// the daemon close any mic card and drop unused mic tokens), and an
    /// open mic or waiting card is closed too; what it heard is dropped.
    pub fn hush(&mut self, reason: &str) {
        let _ = self.cmd.send(MicCmd::Hush(reason.to_string()));
        // A transcript already on its way is dropped as well.
        self.stops.iter_mut().for_each(|d| *d = true);
        self.stop_as(true);
    }

    /// Events since the last frame (never blocks). A transcript dropped by
    /// the safeword and the failure of an already-cancelled start are not
    /// returned.
    pub fn poll(&mut self) -> Vec<MicEvent> {
        let mut out = Vec::new();
        for e in self.rx.try_iter().collect::<Vec<_>>() {
            match &e {
                MicEvent::Ready => self.ready = true,
                MicEvent::Unavailable => {
                    self.ready = false;
                    self.state = MicState::Idle;
                    self.start_inflight = false;
                    self.stops.clear();
                }
                MicEvent::Listening => {
                    self.start_inflight = false;
                    if self.state == MicState::Waiting {
                        self.state = MicState::Listening;
                    } else {
                        // Opened after the press was already stopped (an
                        // older daemon): close it again, the mic always
                        // stops. Nothing it hears is used.
                        self.send_stop(true);
                        continue;
                    }
                }
                MicEvent::StartFailed(_) => {
                    self.start_inflight = false;
                    if self.state != MicState::Waiting {
                        continue; // cancelled by us: not a failure
                    }
                    self.state = MicState::Idle;
                }
                MicEvent::Heard(_) | MicEvent::Failed(_) => {
                    let discard = self.stops.pop_front().unwrap_or(false);
                    if self.state == MicState::Busy && self.stops.is_empty() {
                        self.state = MicState::Idle;
                    }
                    if discard {
                        continue;
                    }
                }
            }
            out.push(e);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> (Mic, Receiver<MicCmd>, Sender<MicEvent>) {
        let (mut m, cmds, ev) = Mic::for_test();
        ev.send(MicEvent::Ready).unwrap();
        m.poll();
        (m, cmds, ev)
    }

    #[test]
    fn the_button_toggles_and_one_card_is_one_press() {
        let (mut m, cmds, ev) = ready();
        assert!(m.toggle(), "click 1 starts a press");
        assert_eq!(cmds.try_recv(), Ok(MicCmd::Start));
        assert_eq!(m.state, MicState::Waiting);
        ev.send(MicEvent::Listening).unwrap();
        m.poll();
        assert_eq!(m.state, MicState::Listening);
        assert!(!m.toggle(), "click 2 stops");
        assert_eq!(cmds.try_recv(), Ok(MicCmd::Stop));
        assert_eq!(m.state, MicState::Busy);
        let heard = Transcript {
            text: "hello".into(),
            ..Default::default()
        };
        ev.send(MicEvent::Heard(heard.clone())).unwrap();
        assert_eq!(m.poll(), [MicEvent::Heard(heard)]);
        assert_eq!(m.state, MicState::Idle);
        // The next press is a new start (a new card on the daemon side).
        assert!(m.toggle());
        assert_eq!(cmds.try_recv(), Ok(MicCmd::Start));
    }

    #[test]
    fn no_second_start_while_one_is_pending() {
        let (mut m, cmds, ev) = ready();
        m.toggle();
        assert_eq!(cmds.try_recv(), Ok(MicCmd::Start));
        // Click again while the card waits: a stop (cancel), never a 2nd start.
        m.toggle();
        assert_eq!(cmds.try_recv(), Ok(MicCmd::Stop));
        for _ in 0..5 {
            m.toggle();
        }
        assert!(cmds.try_recv().is_err(), "busy: nothing sent");
        // The cancel came back, but the start's reply hasn't: still no start.
        ev.send(MicEvent::Heard(Transcript::default())).unwrap();
        m.poll();
        assert_eq!(m.state, MicState::Idle);
        assert!(!m.can_start());
        assert!(!m.toggle());
        assert!(cmds.try_recv().is_err());
        // The cancelled start's refusal is not reported as a failure.
        ev.send(MicEvent::StartFailed("mic: released".into()))
            .unwrap();
        assert!(m.poll().is_empty());
        assert!(m.toggle(), "now a new press may start");
        assert_eq!(cmds.try_recv(), Ok(MicCmd::Start));
    }

    #[test]
    fn a_refused_card_ends_the_press_with_a_note() {
        let (mut m, cmds, ev) = ready();
        m.toggle();
        assert_eq!(cmds.try_recv(), Ok(MicCmd::Start));
        ev.send(MicEvent::StartFailed("mic: Ko said no".into()))
            .unwrap();
        assert_eq!(m.poll(), [MicEvent::StartFailed("mic: Ko said no".into())]);
        assert_eq!(m.state, MicState::Idle);
        assert!(
            cmds.try_recv().is_err(),
            "nothing to stop: the mic never opened"
        );
    }

    #[test]
    fn the_safeword_always_closes_the_mic_and_drops_what_it_heard() {
        for open in [false, true] {
            let (mut m, cmds, ev) = ready();
            m.toggle();
            assert_eq!(cmds.try_recv(), Ok(MicCmd::Start));
            if open {
                ev.send(MicEvent::Listening).unwrap();
                m.poll();
            }
            m.hush("safeword");
            assert_eq!(cmds.try_recv(), Ok(MicCmd::Hush("safeword".into())));
            assert_eq!(cmds.try_recv(), Ok(MicCmd::Stop), "open={open}");
            ev.send(MicEvent::Heard(Transcript {
                text: "secret".into(),
                ..Default::default()
            }))
            .unwrap();
            assert!(m.poll().is_empty(), "dropped");
            assert_eq!(m.state, MicState::Idle);
        }
        // Idle: only her voice is stopped.
        let (mut m, cmds, _ev) = ready();
        m.hush("safeword");
        assert_eq!(cmds.try_recv(), Ok(MicCmd::Hush("safeword".into())));
        assert!(cmds.try_recv().is_err());
    }

    #[test]
    fn a_mic_that_opens_after_its_press_was_stopped_is_closed_again() {
        let (mut m, cmds, ev) = ready();
        m.toggle();
        m.toggle();
        assert_eq!(cmds.try_recv(), Ok(MicCmd::Start));
        assert_eq!(cmds.try_recv(), Ok(MicCmd::Stop));
        ev.send(MicEvent::Heard(Transcript::default())).unwrap();
        ev.send(MicEvent::Listening).unwrap(); // an older daemon opened it late
        m.poll();
        assert_eq!(cmds.try_recv(), Ok(MicCmd::Stop), "closed again");
        ev.send(MicEvent::Heard(Transcript {
            text: "late".into(),
            ..Default::default()
        }))
        .unwrap();
        assert!(m.poll().is_empty(), "nothing it heard is used");
        assert!(m.can_start());
    }

    #[test]
    fn a_late_open_before_the_cancel_returns_still_drops_its_transcript() {
        let (mut m, cmds, ev) = ready();
        m.toggle();
        m.toggle();
        assert_eq!(cmds.try_recv(), Ok(MicCmd::Start));
        assert_eq!(cmds.try_recv(), Ok(MicCmd::Stop));
        ev.send(MicEvent::Listening).unwrap(); // opened before the cancel landed
        m.poll();
        assert_eq!(cmds.try_recv(), Ok(MicCmd::Stop), "closed again");
        assert_eq!(m.state, MicState::Busy);
        ev.send(MicEvent::Heard(Transcript::default())).unwrap();
        assert_eq!(m.poll(), [MicEvent::Heard(Transcript::default())]);
        assert_eq!(m.state, MicState::Busy, "one stop still out");
        assert!(!m.can_start());
        ev.send(MicEvent::Heard(Transcript {
            text: "late".into(),
            ..Default::default()
        }))
        .unwrap();
        assert!(m.poll().is_empty(), "the late mic's words are dropped");
        assert_eq!(m.state, MicState::Idle);
        assert!(m.can_start());
    }

    #[test]
    fn dropping_the_bubble_mid_press_stops() {
        let (mut m, cmds, _ev) = ready();
        m.toggle();
        assert_eq!(cmds.try_recv(), Ok(MicCmd::Start));
        drop(m);
        assert_eq!(cmds.try_recv(), Ok(MicCmd::Stop));
    }

    /// A fake voice daemon on a Unix socket: `listen start` holds its reply
    /// like a mic card waiting for Ko (until a `listen stop` arrives, or
    /// answers Yes at once when `yes`).
    #[cfg(unix)]
    mod fake_daemon {
        use super::*;
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;
        use std::sync::{Arc, Condvar, Mutex};
        use std::time::Instant;

        #[derive(Default)]
        pub struct Log {
            /// (command/action, when it arrived)
            pub seen: Vec<(String, Instant)>,
            pub stopped: bool,
            pub start_replied: Option<Instant>,
        }

        pub struct Fake {
            pub dir: std::path::PathBuf,
            pub log: Arc<(Mutex<Log>, Condvar)>,
        }

        impl Drop for Fake {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }

        pub fn start(tag: &str, yes: bool) -> (Fake, DaemonClient) {
            let dir =
                std::env::temp_dir().join(format!("lillith-mic-fake-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let sock = dir.join("voiced.sock");
            let listener = UnixListener::bind(&sock).unwrap();
            let log: Arc<(Mutex<Log>, Condvar)> = Arc::default();
            let l = Arc::clone(&log);
            std::thread::spawn(move || {
                for s in listener.incoming() {
                    let Ok(mut s) = s else { break };
                    let l = Arc::clone(&l);
                    std::thread::spawn(move || {
                        let mut line = String::new();
                        if BufReader::new(&s).read_line(&mut line).is_err() {
                            return;
                        }
                        let v: serde_json::Value =
                            serde_json::from_str(line.trim()).unwrap_or_default();
                        let cmd = v.get("cmd").and_then(|c| c.as_str()).unwrap_or("");
                        let act = v.get("action").and_then(|c| c.as_str()).unwrap_or("");
                        let what = format!("{cmd} {act}").trim().to_string();
                        let (m, cv) = &*l;
                        m.lock().unwrap().seen.push((what.clone(), Instant::now()));
                        cv.notify_all();
                        let reply = match what.as_str() {
                            "ping" => r#"{"ok":true,"protocol":2}"#.to_string(),
                            "listen start" if yes => {
                                m.lock().unwrap().start_replied = Some(Instant::now());
                                r#"{"ok":true,"listening":true}"#.to_string()
                            }
                            "listen start" => {
                                // The card is up: wait for the release.
                                let g = m.lock().unwrap();
                                let (mut g, _) = cv
                                    .wait_timeout_while(g, Duration::from_secs(5), |g| !g.stopped)
                                    .unwrap();
                                g.start_replied = Some(Instant::now());
                                r#"{"ok":false,"released":true,"need_card":true,"error":"released"}"#
                                    .to_string()
                            }
                            "listen stop" => {
                                m.lock().unwrap().stopped = true;
                                cv.notify_all();
                                if yes {
                                    r#"{"ok":true,"text":"hello there"}"#.to_string()
                                } else {
                                    r#"{"ok":true,"text":"","cancelled":true}"#.to_string()
                                }
                            }
                            _ => r#"{"ok":true}"#.to_string(),
                        };
                        let _ = s.write_all(format!("{reply}\n").as_bytes());
                    });
                }
            });
            let client = DaemonClient::new(&sock).unwrap();
            (Fake { dir, log }, client)
        }

        pub fn wait_for(f: &Fake, what: &str) -> Instant {
            let (m, cv) = &*f.log;
            let g = m.lock().unwrap();
            let (g, _) = cv
                .wait_timeout_while(g, Duration::from_secs(5), |g| {
                    !g.seen.iter().any(|(w, _)| w == what)
                })
                .unwrap();
            g.seen
                .iter()
                .find(|(w, _)| w == what)
                .map(|(_, t)| *t)
                .unwrap_or_else(|| panic!("the fake daemon never got {what}"))
        }

        pub fn poll_until(m: &mut Mic, ok: impl Fn(&Mic) -> bool) -> Vec<MicEvent> {
            let mut evs = Vec::new();
            let end = Instant::now() + Duration::from_secs(5);
            while Instant::now() < end {
                evs.extend(m.poll());
                if ok(m) {
                    return evs;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            panic!("timed out: {m:?} {evs:?}");
        }

        pub fn count(f: &Fake, what: &str) -> usize {
            f.log
                .0
                .lock()
                .unwrap()
                .seen
                .iter()
                .filter(|(w, _)| w == what)
                .count()
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_stop_during_a_pending_card_is_delivered_at_once() {
        use fake_daemon::*;
        let (f, client) = start("pending", false);
        let mut m = Mic::spawn(client, Duration::from_secs(5), || {});
        poll_until(&mut m, |m| m.ready);
        assert!(m.toggle());
        wait_for(&f, "listen start");
        std::thread::sleep(Duration::from_millis(100)); // the card is up
        let clicked = std::time::Instant::now();
        m.toggle(); // click 2: cancel
        let got = wait_for(&f, "listen stop");
        let lag = got.duration_since(clicked);
        assert!(lag < Duration::from_millis(500), "stop took {lag:?}");
        let replied = f.log.0.lock().unwrap().start_replied;
        assert!(
            replied.is_none_or(|t| t >= got),
            "the stop was not queued behind the pending start"
        );
        let evs = poll_until(&mut m, |m| m.can_start());
        assert!(
            !evs.iter()
                .any(|e| matches!(e, MicEvent::StartFailed(_) | MicEvent::Failed(_))),
            "a cancel is not a failure: {evs:?}"
        );
        assert_eq!(count(&f, "listen start"), 1);
        assert_eq!(count(&f, "listen stop"), 1);
    }

    #[cfg(unix)]
    #[test]
    fn toggle_press_two_stops_and_returns_the_transcript() {
        use fake_daemon::*;
        let (f, client) = start("toggle", true);
        let mut m = Mic::spawn(client, Duration::from_secs(5), || {});
        poll_until(&mut m, |m| m.ready);
        m.toggle();
        poll_until(&mut m, |m| m.state == MicState::Listening);
        m.toggle();
        let evs = poll_until(&mut m, |m| m.state == MicState::Idle);
        assert!(
            evs.iter()
                .any(|e| matches!(e, MicEvent::Heard(t) if t.text == "hello there")),
            "{evs:?}"
        );
        assert_eq!(count(&f, "listen start"), 1);
        assert_eq!(count(&f, "listen stop"), 1);
    }

    #[cfg(unix)]
    #[test]
    fn no_second_listen_start_reaches_the_daemon_while_one_is_pending() {
        use fake_daemon::*;
        let (f, client) = start("single", false);
        let mut m = Mic::spawn(client, Duration::from_secs(5), || {});
        poll_until(&mut m, |m| m.ready);
        m.toggle();
        wait_for(&f, "listen start");
        for _ in 0..10 {
            m.poll();
            if m.state == MicState::Waiting {
                assert!(!m.can_start());
            }
        }
        m.toggle(); // cancel
        for _ in 0..10 {
            m.toggle(); // ignored until the pending start is answered
        }
        poll_until(&mut m, |m| m.can_start());
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(count(&f, "listen start"), 1);
    }

    #[test]
    fn the_config_the_app_runs_with_is_used() {
        let dir = std::env::temp_dir().join(format!("lillith-mic-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("custom.toml");
        std::fs::write(
            &p,
            "[voice]\ndaemon_socket = \"/run/custom/voiced.sock\"\n[voice.stt]\nenabled = false\n",
        )
        .unwrap();
        let v = load_voice_config(Some(&p));
        assert_eq!(
            v.daemon_socket.as_deref(),
            Some(std::path::Path::new("/run/custom/voiced.sock"))
        );
        assert!(!v.stt.enabled);
        // STT off in that file: no mic, whatever the default config says.
        assert!(Mic::from_voice_config(&v, || {}).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
