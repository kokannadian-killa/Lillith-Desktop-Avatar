//! Push-to-talk for the chat bubble through Lillith's local voice daemon
//! (`listen action:start|stop`, protocol v2). Loopback only: the daemon is
//! a Unix socket / named pipe on this machine.
//!
//! What a transcript may do is decided by
//! [`lillith_voice::daemon::route_transcript`]: the safeword stops
//! everything, other words only fill the chat input, and nothing heard can
//! ever answer a Soul Contract.

use lillith_voice::daemon::{DaemonClient, Transcript};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

/// What the mic thread reports back to the bubble.
#[derive(Debug, Clone, PartialEq)]
pub enum MicEvent {
    /// The daemon answered the probe: show the mic button.
    Ready,
    /// No daemon (or STT off): no button.
    Unavailable,
    /// The mic is open (the daemon already cut her voice: barge-in).
    Listening,
    /// Push-to-talk released: what was heard.
    Heard(Transcript),
    /// A request failed (shown as a short note).
    Failed(String),
}

enum MicCmd {
    Start,
    Stop,
    /// Stop her voice (`stop` with this reason).
    Hush(String),
}

/// The bubble's handle on the mic thread. Requests run one after another
/// on that thread, so a quick press/release never races.
pub struct Mic {
    cmd: Sender<MicCmd>,
    rx: Receiver<MicEvent>,
    /// The daemon answered: the button shows.
    pub ready: bool,
    /// Button held (mic open or opening).
    pub listening: bool,
    /// Waiting for a transcript.
    pub busy: bool,
}

impl std::fmt::Debug for Mic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mic")
            .field("ready", &self.ready)
            .field("listening", &self.listening)
            .field("busy", &self.busy)
            .finish()
    }
}

impl Drop for Mic {
    fn drop(&mut self) {
        // Bubble closed mid push-to-talk: close the mic.
        if self.listening {
            let _ = self.cmd.send(MicCmd::Stop);
        }
    }
}

impl Mic {
    /// From config.toml `[voice]` (`daemon_socket`, `[voice.stt]`).
    /// `None` when STT is switched off or no local endpoint resolves.
    pub fn from_config(repaint: impl Fn() + Send + 'static) -> Option<Self> {
        let cfg = lillith_core::config::Config::default_path()
            .filter(|p| p.exists())
            .and_then(|p| lillith_voice::VoiceConfig::load(&p).ok())
            .unwrap_or_default();
        if !cfg.stt.enabled {
            return None;
        }
        let client = DaemonClient::from_config(&cfg)?;
        let timeout = Duration::from_secs_f32(cfg.stt.timeout_s);
        Some(Self::spawn(client, timeout, repaint))
    }

    /// Starts the mic thread: pings `client`, then serves press/release.
    pub fn spawn(
        client: DaemonClient,
        timeout: Duration,
        repaint: impl Fn() + Send + 'static,
    ) -> Self {
        let (cmd, cmd_rx) = channel::<MicCmd>();
        let (tx, rx) = channel::<MicEvent>();
        let spawned = std::thread::Builder::new()
            .name("lillith-mic".into())
            .spawn(move || {
                let send = |e: MicEvent| {
                    let _ = tx.send(e);
                    repaint();
                };
                match client.ping() {
                    Ok(_) => send(MicEvent::Ready),
                    Err(e) => {
                        tracing::debug!("voice daemon not available for push-to-talk: {e}");
                        send(MicEvent::Unavailable);
                        return;
                    }
                }
                while let Ok(c) = cmd_rx.recv() {
                    match c {
                        MicCmd::Start => match client.listen_start() {
                            Ok(()) => send(MicEvent::Listening),
                            Err(e) => send(MicEvent::Failed(format!("mic: {e}"))),
                        },
                        MicCmd::Stop => match client.listen_stop(timeout) {
                            Ok(t) => send(MicEvent::Heard(t)),
                            Err(e) => send(MicEvent::Failed(format!("mic: {e}"))),
                        },
                        MicCmd::Hush(reason) => {
                            if let Err(e) = client.stop(Some(&reason)) {
                                tracing::debug!("voice daemon stop: {e}");
                            }
                        }
                    }
                }
            });
        if let Err(e) = spawned {
            tracing::warn!("mic thread: {e}");
        }
        Self {
            cmd,
            rx,
            ready: false,
            listening: false,
            busy: false,
        }
    }

    /// Button pressed: open the mic (the daemon stops her voice).
    pub fn press(&mut self) {
        if self.ready && !self.listening && !self.busy {
            self.listening = true;
            let _ = self.cmd.send(MicCmd::Start);
        }
    }

    /// Button released: close the mic and wait for the transcript.
    pub fn release(&mut self) {
        if self.listening {
            self.listening = false;
            self.busy = true;
            let _ = self.cmd.send(MicCmd::Stop);
        }
    }

    /// Asks the daemon to stop speaking (`reason`: `safeword`, …).
    pub fn hush(&self, reason: &str) {
        let _ = self.cmd.send(MicCmd::Hush(reason.to_string()));
    }

    /// Events since the last frame (never blocks).
    pub fn poll(&mut self) -> Vec<MicEvent> {
        let evs: Vec<MicEvent> = self.rx.try_iter().collect();
        for e in &evs {
            match e {
                MicEvent::Ready => self.ready = true,
                MicEvent::Unavailable => {
                    self.ready = false;
                    self.listening = false;
                    self.busy = false;
                }
                MicEvent::Listening => {}
                MicEvent::Heard(_) => self.busy = false,
                MicEvent::Failed(_) => {
                    self.listening = false;
                    self.busy = false;
                }
            }
        }
        evs
    }
}
