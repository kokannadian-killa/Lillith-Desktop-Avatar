//! Click-to-chat wiring for the binary.
//!
//! - [`Launcher`]: the overlay's [`lillith_overlay::ChatHook`]. Spawns
//!   `lillith chat-bubble --x … --y …` (a separate process, because the
//!   overlay owns this process's windowing event loop), reads the bubble's
//!   stdout (JSON lines `{"event": "chat.token", "detail": "…"}`) and forwards
//!   them as [`Event`]s into the overlay feed so `talk_loop` lip-flaps in sync.
//!   Clicking her while the bubble is open closes it.
//! - [`ServerGuard`]: the optional self-spawned llama-server
//!   (`[chat.server]`), stopped when the app exits (also on Ctrl-C/SIGTERM).
//! - [`bubble_main`] / [`cli_chat`]: the `chat-bubble` and `chat` subcommands.

use anyhow::Result;
use lillith_core::config::ChatConfig;
use lillith_core::Event;
use lillith_mind::chat::{default_history_path, ChatSession, Outcome};
use lillith_mind::server::{already_running, LlamaServer};
use lillith_overlay::{ChatAnchor, ChatHook};
use lillith_panel::chat_bubble::{run_chat_bubble, BubbleOptions, Emit, BUBBLE_SIZE};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A click this soon after the bubble closed was the click that closed it
/// (focus loss): don't reopen.
const REOPEN_GUARD: Duration = Duration::from_millis(900);

/// One JSON line of the bubble → overlay protocol.
pub fn encode(name: &str, detail: Option<&str>) -> String {
    serde_json::json!({ "event": name, "detail": detail }).to_string()
}

/// Parses a protocol line (`None` for anything else, e.g. stray logs).
pub fn decode(line: &str) -> Option<Event> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let name = v.get("event")?.as_str()?;
    if !name.starts_with("chat.") && !lillith_overlay::ui::is_ui_event(name) {
        return None;
    }
    Some(match v.get("detail").and_then(|d| d.as_str()) {
        Some(d) => Event::with_detail(name, d),
        None => Event::new(name),
    })
}

#[derive(Default)]
struct State {
    child: Option<Child>,
    closed_at: Option<Instant>,
}

/// Spawns and tracks the bubble process.
#[derive(Clone)]
pub struct Launcher {
    state: Arc<Mutex<State>>,
    feed: Sender<Event>,
    config: Option<PathBuf>,
    offline: bool,
}

impl Launcher {
    pub fn new(feed: Sender<Event>, config: Option<PathBuf>, offline: bool) -> Self {
        Self {
            state: Arc::default(),
            feed,
            config,
            offline,
        }
    }

    pub fn hook(&self) -> ChatHook {
        let me = self.clone();
        ChatHook::new(move |a| me.toggle(a))
    }

    fn toggle(&self, a: ChatAnchor) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = &mut st.child {
            if matches!(c.try_wait(), Ok(None)) {
                tracing::debug!("chat bubble open; closing it");
                let _ = c.kill();
                let _ = c.wait();
                st.child = None;
                st.closed_at = Some(Instant::now());
                return;
            }
            st.child = None;
        }
        if st.closed_at.is_some_and(|t| t.elapsed() < REOPEN_GUARD) {
            return;
        }
        match self.spawn(a) {
            Ok(c) => st.child = Some(c),
            Err(e) => tracing::warn!("could not open the chat bubble: {e}"),
        }
    }

    fn spawn(&self, a: ChatAnchor) -> Result<Child> {
        let exe = std::env::current_exe()?;
        let mut cmd = Command::new(exe);
        // Top-level flags go before the subcommand.
        if let Some(c) = &self.config {
            cmd.arg("--config").arg(c);
        }
        cmd.arg("chat-bubble");
        if let Some((x, y)) = a.bubble_pos(BUBBLE_SIZE.0 as u32, BUBBLE_SIZE.1 as u32) {
            cmd.args(["--x", &x.to_string(), "--y", &y.to_string()]);
        }
        if self.offline {
            cmd.arg("--offline");
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = cmd.spawn()?;
        let out = child.stdout.take().expect("piped stdout");
        let feed = self.feed.clone();
        let state = self.state.clone();
        std::thread::Builder::new()
            .name("lillith-chat-feed".into())
            .spawn(move || {
                let mut closed = false;
                for line in std::io::BufReader::new(out).lines() {
                    let Ok(line) = line else { break };
                    if let Some(ev) = decode(&line) {
                        closed |= ev.name == "chat.closed";
                        if feed.send(ev).is_err() {
                            return;
                        }
                    }
                }
                // Crashed or killed without saying goodbye: stop talking.
                if !closed {
                    let _ = feed.send(Event::new("chat.closed"));
                }
                if let Ok(mut st) = state.lock() {
                    st.closed_at = Some(Instant::now());
                }
            })?;
        tracing::info!(pid = child.id(), "chat bubble opened");
        Ok(child)
    }

    /// Closes the bubble (app exit).
    pub fn close(&self) {
        if let Ok(mut st) = self.state.lock() {
            if let Some(mut c) = st.child.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
}

/// The self-spawned llama-server, shared with the signal handler.
#[derive(Clone, Default)]
pub struct ServerGuard(Arc<Mutex<Option<LlamaServer>>>);

impl ServerGuard {
    /// Starts llama-server when `[chat.server]` is configured and nothing
    /// answers at the endpoint yet. Readiness is logged in the background;
    /// until then the bubble answers with an in-character "waking up" line.
    pub async fn start(cfg: &ChatConfig) -> Self {
        let g = Self::default();
        if !cfg.enabled || !cfg.server.should_spawn() {
            return g;
        }
        if already_running(&cfg.endpoint).await {
            tracing::info!(endpoint = %cfg.endpoint, "llama-server already running; not spawning");
            return g;
        }
        match LlamaServer::spawn(cfg) {
            Ok(s) => {
                tracing::info!(pid = ?s.pid(), endpoint = %cfg.endpoint, "llama-server spawned");
                *g.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
                let endpoint = cfg.endpoint.clone();
                let timeout = Duration::from_secs_f32(cfg.server.startup_timeout_s.max(1.0));
                tokio::spawn(async move {
                    let t0 = Instant::now();
                    while t0.elapsed() < timeout {
                        let c = lillith_mind::llm::LlmClient::with_timeout(
                            lillith_mind::llm::Flavor::OpenAiCompat,
                            endpoint.as_str(),
                            "",
                            Duration::from_secs(2),
                        );
                        if let Ok(true) = c.health().await {
                            tracing::info!(
                                after_s = t0.elapsed().as_secs_f32(),
                                "llama-server ready"
                            );
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                    tracing::warn!("llama-server not ready after {timeout:?}");
                });
                let sig = g.clone();
                tokio::spawn(async move {
                    wait_for_signal().await;
                    tracing::info!("signal: stopping llama-server");
                    sig.stop();
                    std::process::exit(130);
                });
            }
            Err(e) => tracing::warn!("not starting llama-server: {e}"),
        }
        g
    }

    pub fn stop(&self) {
        if let Some(mut s) = self.0.lock().unwrap_or_else(|e| e.into_inner()).take() {
            s.shutdown();
        }
    }
}

impl Drop for ServerGuard {
    /// Dropping the app's guard (normal exit) stops the server. The signal
    /// handler holds the only other clone and stops it itself.
    fn drop(&mut self) {
        self.stop();
    }
}

async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn history_path(cfg: &ChatConfig) -> Option<PathBuf> {
    cfg.save_history.then(default_history_path).flatten()
}

/// `lillith chat-bubble`: the bubble window; events go to stdout.
/// Emit that writes protocol lines to stdout (child windows → overlay).
pub fn stdout_emit() -> Emit {
    let stdout = Arc::new(Mutex::new(std::io::stdout()));
    Arc::new(move |name, detail| {
        if let Ok(mut o) = stdout.lock() {
            let _ = writeln!(o, "{}", encode(name, detail));
            let _ = o.flush();
        }
    })
}

pub fn bubble_main(cfg: ChatConfig, pos: Option<(i32, i32)>, offline: bool) -> Result<()> {
    let emit = stdout_emit();
    let opts = BubbleOptions {
        pos,
        history: history_path(&cfg),
        chat: cfg,
        offline,
        emit,
    };
    run_chat_bubble(opts).map_err(|e| anyhow::anyhow!("chat bubble: {e}"))
}

/// `lillith chat [MESSAGE]`: headless chat through the same client code
/// (one message, or a prompt loop on stdin). Streams to stdout.
pub async fn cli_chat(
    cfg: ChatConfig,
    message: Option<String>,
    spawn_server: bool,
    no_history: bool,
) -> Result<()> {
    let _server = if spawn_server {
        let mut c = cfg.clone();
        c.enabled = true;
        let g = ServerGuard::start(&c).await;
        // Wait for the model before the first message.
        let client = lillith_mind::llm::LlmClient::with_timeout(
            lillith_mind::llm::Flavor::OpenAiCompat,
            cfg.endpoint.as_str(),
            "",
            Duration::from_secs(2),
        );
        let t0 = Instant::now();
        let limit = Duration::from_secs_f32(cfg.server.startup_timeout_s.max(1.0));
        while !matches!(client.health().await, Ok(true)) && t0.elapsed() < limit {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        Some(g)
    } else {
        None
    };
    let mut session = ChatSession::new(&cfg, false);
    if !no_history {
        if let Some(p) = history_path(&cfg) {
            session = session.with_history_file(p);
        }
    }
    match message {
        Some(m) => {
            if let Outcome::Fallback { .. } = ask(&mut session, m).await {
                anyhow::bail!("no reply from {}", cfg.endpoint);
            }
        }
        None => {
            let stdin = std::io::stdin();
            loop {
                print!("you: ");
                let _ = std::io::stdout().flush();
                let mut line = String::new();
                if stdin.lock().read_line(&mut line)? == 0 {
                    break;
                }
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                if line == "/quit" {
                    break;
                }
                ask(&mut session, line).await;
            }
        }
    }
    Ok(())
}

async fn ask(session: &mut ChatSession, text: String) -> Outcome {
    print!("Lillith: ");
    let _ = std::io::stdout().flush();
    let out = session
        .send(&text, |t| {
            print!("{t}");
            let _ = std::io::stdout().flush();
        })
        .await;
    println!();
    if let Outcome::Fallback { error, .. } = &out {
        eprintln!("(fallback line; reason: {error})");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_round_trip() {
        let l = encode("chat.token", Some("hey \"cutie\"\n~"));
        let ev = decode(&l).unwrap();
        assert_eq!(ev.name, "chat.token");
        assert_eq!(ev.detail.as_deref(), Some("hey \"cutie\"\n~"));
        assert_eq!(decode(&encode("chat.start", None)).unwrap().detail, None);
        assert!(decode("random log line").is_none());
        assert!(
            decode("{\"event\":\"cpu.high\"}").is_none(),
            "only chat.* and settings/avatar events pass"
        );
        assert_eq!(
            decode(&encode("settings.changed", None)).unwrap().name,
            "settings.changed"
        );
        assert!(decode(&encode("menu.pose", Some("wave"))).is_none());
    }
}
