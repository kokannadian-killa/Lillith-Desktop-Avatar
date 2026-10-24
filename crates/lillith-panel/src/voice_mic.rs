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

#[derive(Debug, PartialEq)]
pub(crate) enum MicCmd {
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
    /// Opening the mic failed while the button was held: no retry (and no
    /// repeated note / barge-in) until it is released.
    pub held_failed: bool,
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
        // Bubble closed mid push-to-talk: close the mic.
        if self.listening {
            let _ = self.cmd.send(MicCmd::Stop);
        }
    }
}

impl Mic {
    /// From the `[voice]` section (`daemon_socket`, `[voice.stt]`) of the
    /// config file the app runs with (`--config`; `None` = the default
    /// path). `None` when STT is switched off or no local endpoint resolves.
    pub fn from_config(
        config_path: Option<&std::path::Path>,
        repaint: impl Fn() + Send + 'static,
    ) -> Option<Self> {
        Self::from_voice_config(&load_voice_config(config_path), repaint)
    }

    /// [`Mic::from_config`] for an already loaded `[voice]`.
    pub fn from_voice_config(
        cfg: &lillith_voice::VoiceConfig,
        repaint: impl Fn() + Send + 'static,
    ) -> Option<Self> {
        if !cfg.stt.enabled {
            return None;
        }
        let client = DaemonClient::from_config(cfg)?;
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
            held_failed: false,
        }
    }

    /// Mic handle wired to channels instead of a daemon thread (tests).
    #[cfg(test)]
    pub(crate) fn for_test() -> (Self, Receiver<MicCmd>, Sender<MicEvent>) {
        let (cmd, cmd_rx) = std::sync::mpsc::channel();
        let (tx, rx) = std::sync::mpsc::channel();
        let m = Self {
            cmd,
            rx,
            ready: false,
            listening: false,
            busy: false,
            held_failed: false,
        };
        (m, cmd_rx, tx)
    }

    /// The button counts as held (open, opening, or failed while held).
    pub fn held(&self) -> bool {
        self.listening || self.held_failed
    }

    /// Button pressed: open the mic (the daemon stops her voice).
    pub fn press(&mut self) {
        if self.ready && !self.listening && !self.busy && !self.held_failed {
            self.listening = true;
            let _ = self.cmd.send(MicCmd::Start);
        }
    }

    /// Button released: close the mic and wait for the transcript.
    pub fn release(&mut self) {
        if self.held_failed {
            self.held_failed = false;
            return;
        }
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
                    // `listen start` failed with the button still down.
                    self.held_failed |= self.listening;
                    self.listening = false;
                    self.busy = false;
                }
            }
        }
        evs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_mic_is_latched_until_the_button_is_released() {
        let (mut m, cmds, ev) = Mic::for_test();
        ev.send(MicEvent::Ready).unwrap();
        m.poll();
        m.press();
        assert_eq!(cmds.try_recv(), Ok(MicCmd::Start));
        ev.send(MicEvent::Failed("mic: no device".into())).unwrap();
        m.poll();
        assert!(m.held() && !m.listening);
        // Still held: every frame's press is ignored (no retry spam).
        for _ in 0..5 {
            m.press();
        }
        assert!(cmds.try_recv().is_err());
        // Release clears the latch without a `listen stop`.
        m.release();
        assert!(!m.held());
        assert!(cmds.try_recv().is_err());
        m.press();
        assert_eq!(cmds.try_recv(), Ok(MicCmd::Start));
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
