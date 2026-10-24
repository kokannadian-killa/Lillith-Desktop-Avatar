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
//!   Starting it is a program launch, so the overlay never does it on its
//!   own: the bubble raises the "Restart local model server?" Soul Contract
//!   card and only a click on its Yes (audited `CONFIRM`) makes the bubble
//!   emit [`SERVER_APPROVED_EVENT`], on which [`Launcher`] spawns it.
//! - [`bubble_main`] / [`cli_chat`]: the `chat-bubble` and `chat` subcommands.

use anyhow::Result;
use lillith_core::access::deletion::{DeleteMode, DeletionSignature};
use lillith_core::card_process::CardText;
use lillith_core::config::{ChatConfig, MemoryConfig, MindConfig};
use lillith_core::soul_contract::{ContractError, PendingAction, RequestId, SoulContract};
use lillith_core::Event;
use lillith_mind::chat::{default_history_path, ChatSession, Outcome, MAX_TOOL_CHAIN};
use lillith_mind::server::{already_running, LlamaServer};
use lillith_overlay::{ChatAnchor, ChatHook};
use lillith_panel::chat_bubble::{
    run_chat_bubble, BubbleOptions, Emit, BUBBLE_SIZE, SERVER_APPROVED_EVENT, SERVER_CARD_EVENT,
};
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
    /// `[chat.server]` the bubble may ask for (after a clicked Yes).
    server: Option<ApprovedServer>,
    /// The Soul Contract audit log: a bubble that dies with the
    /// model-server card up gets its `CANCELLED` written here.
    audit_log: Option<PathBuf>,
}

/// Audit note when the overlay finds the bubble gone with a card up.
pub const BUBBLE_GONE_NOTE: &str = "chat bubble gone before an answer (killed)";

/// What [`Launcher`] needs to start `[chat.server]` on the bubble's
/// [`SERVER_APPROVED_EVENT`].
#[derive(Clone)]
struct ApprovedServer {
    handle: ServerHandle,
    cfg: ChatConfig,
    memory: Arc<lillith_memory::MemoryBridge>,
}

impl Launcher {
    pub fn new(feed: Sender<Event>, config: Option<PathBuf>, offline: bool) -> Self {
        Self {
            state: Arc::default(),
            feed,
            config,
            offline,
            server: None,
            audit_log: None,
        }
    }

    /// Where a killed bubble's open model-server card is closed.
    pub fn with_audit_log(mut self, audit_log: Option<PathBuf>) -> Self {
        self.audit_log = audit_log;
        self
    }

    /// The app owns `[chat.server]`: the bubble is told so (`--app-server`)
    /// and its approval event starts the server through `handle`.
    pub fn with_server(
        mut self,
        handle: ServerHandle,
        cfg: ChatConfig,
        memory: Arc<lillith_memory::MemoryBridge>,
    ) -> Self {
        self.server = Some(ApprovedServer {
            handle,
            cfg,
            memory,
        });
        self
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
        if self.server.is_some() {
            cmd.arg("--app-server");
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = cmd.spawn()?;
        let out = child.stdout.take().expect("piped stdout");
        let feed = self.feed.clone();
        let state = self.state.clone();
        let server = self.server.clone();
        let audit_log = self.audit_log.clone();
        std::thread::Builder::new()
            .name("lillith-chat-feed".into())
            .spawn(move || {
                pump(out, &feed, server.as_ref(), audit_log);
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

/// Forwards the bubble's stdout into the overlay feed until it ends. The
/// model-server card protocol is handled here, not forwarded: an approval
/// starts the server; a card id is remembered so that, if the bubble ends
/// (crash, SIGKILL) with that card still open in the audit log, `CANCELLED`
/// is appended for it (nothing when the bubble already recorded a decision).
fn pump(
    out: impl std::io::Read,
    feed: &Sender<Event>,
    server: Option<&ApprovedServer>,
    audit_log: Option<PathBuf>,
) {
    let mut closed = false;
    let mut card: Option<String> = None;
    for line in std::io::BufReader::new(out).lines() {
        let Ok(line) = line else { break };
        let Some(ev) = decode(&line) else { continue };
        if ev.name == SERVER_APPROVED_EVENT {
            // Sent after a clicked Yes on the card (audited CONFIRM in the
            // bubble); re-checked here against the audit log and config.
            if let Some(s) = server {
                start_approved(s, ev.detail.as_deref(), audit_log.clone());
            }
            continue;
        }
        if ev.name == SERVER_CARD_EVENT {
            card = ev.detail.clone();
            continue;
        }
        closed |= ev.name == "chat.closed";
        if feed.send(ev).is_err() {
            break;
        }
    }
    if let Some(id) = card {
        if SoulContract::new(audit_log).cancel_open_model_server_card(&id, BUBBLE_GONE_NOTE) {
            tracing::info!(card = %id, "chat bubble gone with the model-server card up: CANCELLED");
        }
    }
    // Crashed or killed without saying goodbye: stop talking.
    if !closed {
        let _ = feed.send(Event::new("chat.closed"));
    }
}

/// Checks a `chat.server_approved` detail before anything starts: it must
/// name a card and the argv it showed; that argv must equal what
/// `[chat.server]` would run now (`display_argv`); and the audit log must
/// hold the card's `CONFIRM` and no earlier use. Then `USED` is appended
/// (one card, one launch) and `(card, binary)` returned. Every refusal is
/// audited `SPAWN_REFUSED` when the card is known.
fn approve_spawn(
    detail: Option<&str>,
    cfg: &ChatConfig,
    audit_log: Option<PathBuf>,
) -> Result<(String, String), String> {
    let (card, argv) = detail
        .and_then(lillith_panel::chat_bubble::parse_approval_detail)
        .ok_or("the approval names no card and command")?;
    let sc = SoulContract::new(audit_log);
    let bin = argv.first().cloned().unwrap_or_default();
    if lillith_mind::server::display_argv(cfg).as_ref() != Some(&argv) {
        let why = "the approved command is not what [chat.server] would run";
        sc.refuse_model_server_card(&card, &bin, why);
        return Err(why.into());
    }
    sc.claim_model_server_approval(&card, &bin)?;
    Ok((card, bin))
}

/// Starts `[chat.server]` for an approval that passes [`approve_spawn`];
/// a failed spawn is audited `SPAWN_FAILED` so the bubble stops waiting.
fn start_approved(s: &ApprovedServer, detail: Option<&str>, audit_log: Option<PathBuf>) {
    match approve_spawn(detail, &s.cfg, audit_log.clone()) {
        Ok((card, bin)) => {
            if !s.handle.spawn_approved(&s.cfg, Some(&s.memory)) {
                SoulContract::new(audit_log).record_model_server_failure(
                    &card,
                    &bin,
                    "llama-server could not be started",
                );
            }
        }
        Err(e) => tracing::warn!("not starting llama-server: {e}"),
    }
}

/// The self-spawned llama-server, shared with the signal handler.
#[derive(Clone, Default)]
pub struct ServerGuard(Arc<Mutex<Option<LlamaServer>>>);

impl ServerGuard {
    /// Prepares `[chat.server]` for the overlay. Nothing is spawned here:
    /// starting llama-server is a program launch, so it waits for a clicked
    /// Yes on the bubble's Soul Contract card (then
    /// [`ServerHandle::spawn_approved`]). Ctrl-C / SIGTERM still stop a
    /// server started that way.
    pub async fn start(cfg: &ChatConfig) -> Self {
        let g = Self::default();
        if !cfg.enabled || !cfg.server.should_spawn() {
            return g;
        }
        if already_running(&cfg.endpoint).await {
            tracing::info!(endpoint = %cfg.endpoint, "llama-server already running; not spawning");
        } else {
            tracing::info!(
                endpoint = %cfg.endpoint,
                "llama-server not started: [chat.server] starts only after a clicked Yes on the Soul Contract card (open the chat)"
            );
        }
        g.stop_on_signal();
        g
    }

    /// `lillith chat --spawn-server`: the flag only asks; starting is a
    /// program launch, so it waits for a click on Yes on the Soul Contract
    /// card. Without a display (or when the card window can't open) it is
    /// refused (`NO_UI`) and the terminal gets the manual start command; a
    /// typed yes never counts (see [`crate::server_consent`]). `true` when it was
    /// started. A server already answering needs no card.
    pub async fn start_cli(cfg: &ChatConfig, mind: &MindConfig) -> (Self, bool) {
        let g = Self::default();
        if !cfg.server.should_spawn() {
            return (g, false);
        }
        if already_running(&cfg.endpoint).await {
            return (g, true);
        }
        let log = mind.audit_log_path();
        // A refused extra arg is rejected up front (one ARG_REFUSED line):
        // no card is shown for a start that would fail after the Yes.
        let bin = lillith_setup::launch::resolve_program(&cfg.server.binary);
        if let Err(e) = lillith_mind::server::check_extra_args_audited(
            &cfg.server.extra_args,
            &bin.display().to_string(),
            log.clone(),
        ) {
            println!(
                "[chat.server] {e}\nNot starting the local model server and no Soul Contract card shown \
                 (logged as ARG_REFUSED). Remove it from [chat.server] extra_args in config.toml \
                 (see \"Migrating extra_args\" in docs/CHAT.md)."
            );
            return (g, false);
        }
        let Some(argv) = lillith_mind::server::display_argv(cfg) else {
            return (g, false);
        };
        // Cards a dead process left open (its pid is gone) are closed
        // first; a live overlay's card is left alone.
        SoulContract::new(log.clone())
            .close_orphaned_model_server_cards("orphaned PROPOSED closed at startup");
        let action = PendingAction::start_model_server(argv, false);
        let kept = action.clone();
        let log2 = log.clone();
        let display = crate::server_consent::display_available();
        let decision = match tokio::task::spawn_blocking(move || {
            crate::server_consent::decide(
                &mut SoulContract::new(log2),
                action,
                display,
                crate::server_consent::show_card,
            )
        })
        .await
        {
            Ok(d) => d,
            Err(e) => {
                // The prompt panicked: close the card (PROPOSED stays
                // open otherwise) and start nothing.
                SoulContract::new(log).record_note(
                    "CANCELLED",
                    &kept,
                    &format!("consent prompt panicked: {e}"),
                );
                crate::server_consent::Decision::Card(lillith_mind::chat::CardChoice::Cancelled)
            }
        };
        println!("[soul contract] {}", decision.describe());
        if let crate::server_consent::Decision::NoUi { .. } = decision {
            if let PendingAction::StartModelServer { argv, .. } = &kept {
                println!("{}", crate::server_consent::manual_start_hint(argv));
            }
        }
        if !decision.approved() {
            return (g, false);
        }
        let ok = g.handle().spawn_approved(cfg, None);
        g.stop_on_signal();
        (g, ok)
    }

    /// The spawn side, shared with [`Launcher`] (no `Drop`: only the
    /// guard stops the server).
    pub fn handle(&self) -> ServerHandle {
        ServerHandle(self.0.clone())
    }

    fn stop_on_signal(&self) {
        let sig = self.clone();
        tokio::spawn(async move {
            wait_for_signal().await;
            // Ctrl-C / SIGTERM: kill running commands (their own
            // process group never sees the terminal's Ctrl-C) and
            // the admin helper first.
            let r = lillith_core::access::kill_switch::stop_all();
            println!("\n[safeword] {}", r.chat_line());
            tracing::info!("signal: stopping llama-server");
            sig.stop();
            std::process::exit(130);
        });
    }

    pub fn stop(&self) {
        if let Some(mut s) = self.0.lock().unwrap_or_else(|e| e.into_inner()).take() {
            s.shutdown();
        }
    }
}

/// Starts the guarded server once it was approved (see [`ServerGuard`]).
#[derive(Clone)]
pub struct ServerHandle(Arc<Mutex<Option<LlamaServer>>>);

impl ServerHandle {
    /// Spawns `[chat.server]` after an approval (a clicked Yes on the card;
    /// a typed yes never counts). No-op when the one it started is
    /// still running. Spawn failures are remembered in `memory`.
    pub fn spawn_approved(
        &self,
        cfg: &ChatConfig,
        memory: Option<&lillith_memory::MemoryBridge>,
    ) -> bool {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if slot.as_mut().is_some_and(LlamaServer::is_running) {
            return true;
        }
        if let Some(mut old) = slot.take() {
            old.shutdown();
        }
        match LlamaServer::spawn(cfg) {
            Ok(s) => {
                tracing::info!(pid = ?s.pid(), endpoint = %cfg.endpoint, "llama-server spawned (approved)");
                *slot = Some(s);
                true
            }
            Err(e) => {
                tracing::warn!("not starting llama-server: {e}");
                if let Some(m) = memory {
                    crate::memory_wiring::record_failure(
                        m,
                        "llama-server: spawn failed",
                        "chat.server",
                    );
                }
                false
            }
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

/// A chat session for `lillith chat`: `[mind]` attached, and the system
/// prompt resolved against `config_path` (`--config` or the default path),
/// so a custom config file's `[llm] system_prompt` applies too.
fn new_session(
    cfg: &ChatConfig,
    mind: &MindConfig,
    memory: &MemoryConfig,
    config_path: Option<&std::path::Path>,
) -> ChatSession {
    let mut session = ChatSession::new(cfg, false);
    if let Some(p) = config_path {
        session = session.with_config_file(&cfg.system_prompt, p);
    }
    session.with_mind(mind).with_memory(memory)
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

#[allow(clippy::too_many_arguments)]
pub fn bubble_main(
    cfg: ChatConfig,
    mind: MindConfig,
    memory: MemoryConfig,
    config_path: Option<PathBuf>,
    pos: Option<(i32, i32)>,
    offline: bool,
    screenshot: Option<PathBuf>,
    demo: bool,
    demo_contract: bool,
    app_server: bool,
) -> Result<()> {
    let emit = stdout_emit();
    let opts = BubbleOptions {
        pos,
        history: if demo { None } else { history_path(&cfg) },
        chat: cfg,
        mind,
        memory,
        config_path,
        offline,
        emit,
        screenshot,
        demo,
        demo_contract,
        app_server,
    };
    run_chat_bubble(opts).map_err(|e| anyhow::anyhow!("chat bubble: {e}"))
}

/// `lillith chat [MESSAGE]`: headless chat through the same client code
/// (one message, or a prompt loop on stdin). Streams to stdout.
pub async fn cli_chat(
    cfg: ChatConfig,
    mind: MindConfig,
    memory: MemoryConfig,
    config_path: Option<PathBuf>,
    message: Option<String>,
    spawn_server: bool,
    no_history: bool,
) -> Result<()> {
    let _server = if spawn_server {
        let mut c = cfg.clone();
        c.enabled = true;
        let (g, started) = ServerGuard::start_cli(&c, &mind).await;
        // Declined / no UI: chat goes on and falls back to text-only.
        if started {
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
        }
        Some(g)
    } else {
        None
    };
    let mut session = new_session(&cfg, &mind, &memory, config_path.as_deref());
    if !no_history {
        if let Some(p) = history_path(&cfg) {
            session = session.with_history_file(p);
        }
    }
    if let Some(d) = lillith_core::config::data_dir() {
        for note in lillith_core::notes::take(&d) {
            println!("[lillith] {note}");
        }
    }
    let skills_note = session.import_desktop_skills();
    match message {
        Some(m) => {
            spawn_ctrl_c_kill();
            if let Some(note) = skills_note {
                // One-shot mode can't review; drop the plan, copy nothing.
                let _ = session.take_pending();
                println!("[skills] {note} — not done (run `lillith chat` or `lillith import-skills` to review).");
            }
            if let Outcome::Fallback { .. } = ask(&mut session, m).await {
                anyhow::bail!("no reply from {}", cfg.endpoint);
            }
            // One-shot mode can't ask; never write or run without an answer.
            if let Some(action) = session.pending_action() {
                println!(
                    "[soul contract] she proposed to {} — not done (run `lillith chat` interactively to confirm).",
                    action.summary()
                );
            } else if let Some(fb) = session.take_tool_feedback() {
                println!("{fb}");
                // Let her tell what she found (read-only results only).
                let _ = ask(&mut session, fb).await;
            }
        }
        None => {
            // stdin is read on its own thread so "red" / "OOC: stop" fires
            // the kill switch even while an approved command runs inline.
            let mut stdin = ConsoleLines::new(spawn_console_reader(
                std::io::BufReader::new(std::io::stdin()),
                console_safeword,
            ));
            spawn_ctrl_c_kill();
            if let Some(note) = skills_note {
                println!("[skills] {note}");
                // Review it now; the answer is not sent to her.
                let _ = offer_pending(&mut session, &mut stdin)?;
            }
            loop {
                print!("you: ");
                let _ = std::io::stdout().flush();
                let mut line = String::new();
                if stdin.read_line(&mut line)? == 0 {
                    break;
                }
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                if line == "/quit" {
                    break;
                }
                if lillith_core::access::kill_switch::is_safeword(&line) {
                    // The reader thread already pulled the kill switch and
                    // said so; drop anything still waiting for a Yes.
                    while session.take_pending().is_some() {}
                    continue;
                }
                ask(&mut session, line).await;
                // Tool results (and Ko's answers) go back to her so she can
                // say what happened; she may chain a few tools.
                for _ in 0..MAX_TOOL_CHAIN {
                    let Some(feedback) = offer_pending(&mut session, &mut stdin)? else {
                        break;
                    };
                    ask(&mut session, feedback).await;
                }
                if let Some(fb) = offer_pending(&mut session, &mut stdin)? {
                    println!("{fb}");
                }
            }
        }
    }
    Ok(())
}

/// The console Soul Contract: if her last reply proposed a mutation (skill,
/// file write, organize batch, command…), it runs only after a mouse click
/// on Yes on the Soul Contract card (see [`offer_on_card`]); a typed yes
/// never counts and stdin is not read for it. Without a display nothing
/// runs (`NO_UI`) and the console says how to do it by hand. A plan or a
/// deletion is reviewed on the console first, but typing there only PICKS
/// (items, Trash/permanent, the list phrase); the run itself needs the
/// same card's clicked Yes (see [`review_plan`] and [`sign_on_console`]).
/// Returns the message to feed back to her (the tool result or Ko's
/// answer), if any.
fn offer_pending(session: &mut ChatSession, stdin: &mut dyn BufRead) -> Result<Option<String>> {
    offer_pending_with(
        session,
        stdin,
        crate::contract_consent::display_available(),
        crate::contract_consent::show_card,
    )
}

/// The card hook: shows a card and returns how it was answered (`None`: it
/// could not open).
type CardFn<'a> = dyn FnMut(&CardText) -> Option<lillith_mind::chat::CardChoice> + 'a;

/// [`offer_pending`] with the display check and the card injectable (tests).
fn offer_pending_with(
    session: &mut ChatSession,
    stdin: &mut dyn BufRead,
    display: bool,
    mut card: impl FnMut(&CardText) -> Option<lillith_mind::chat::CardChoice>,
) -> Result<Option<String>> {
    let Some(action) = session.take_pending() else {
        return Ok(session.take_tool_feedback());
    };
    let mut contract = session.soul_contract();
    if matches!(action, PendingAction::Plan { .. }) {
        let id = contract.request(action.clone());
        let res = review_plan(
            &mut contract,
            id,
            stdin,
            &mut std::io::stdout(),
            display,
            &mut card,
        )?;
        return Ok(Some(ChatSession::answer_feedback(
            &action,
            res.as_deref().map_err(|e| e.as_str()),
        )));
    }
    if matches!(action, PendingAction::DeletePaths { .. }) {
        let id = contract.request(action.clone());
        let res = sign_on_console(
            &mut contract,
            id,
            stdin,
            &mut std::io::stdout(),
            display,
            &mut card,
        )?;
        return Ok(Some(ChatSession::answer_feedback(
            &action,
            res.as_deref().map_err(|e| e.as_str()),
        )));
    }
    Ok(Some(offer_on_card(&mut contract, action, display, card)))
}

/// Everything but plans and deletions: the mouse-only Soul Contract card
/// (`lillith contract-card`), never a typed answer; nothing is read from
/// stdin. `PROPOSED` is audited before the card, then the outcome. No card:
/// `NO_UI`, nothing runs, and the console says how to do it by hand.
/// Returns the feedback for her.
fn offer_on_card(
    contract: &mut SoulContract,
    action: PendingAction,
    display: bool,
    card: impl FnOnce(&CardText) -> Option<lillith_mind::chat::CardChoice>,
) -> String {
    let prompt = action.prompt();
    if let Some(d) = &prompt.detail {
        println!("{d}");
    }
    println!(
        "[soul contract] She wants to {}. Answer on the Soul Contract card (a click on Yes; a typed yes never counts).",
        action.summary()
    );
    let kept = action.clone();
    let (decision, res) = crate::contract_consent::offer(contract, action, display, card);
    match (&res, decision.refusal()) {
        (Ok(out), _) => println!("[soul contract] {out}"),
        (Err(e), None) => println!("[soul contract] failed: {e}"),
        (Err(_), Some(why)) => {
            println!("[soul contract] {why}; discarded, nothing changed.");
            if let crate::contract_consent::Decision::NoUi { .. } = decision {
                println!("{}", crate::contract_consent::manual_hint(&kept));
            }
        }
    }
    ChatSession::answer_feedback(&kept, res.as_deref().map_err(|e| e.as_str()))
}

/// What a plan or deletion card's answer means for the console: `Ok(())`
/// only for a clicked Yes. Anything else takes request `id` out of
/// `contract` without running it (No → `DENY`; no answer → `TIMEOUT`;
/// closed or the safeword → `CANCELLED`; no card → `NO_UI`), prints why
/// (and `hint` for `NO_UI`) and returns the refusal for her.
fn settle_card(
    contract: &mut SoulContract,
    id: RequestId,
    decision: crate::contract_consent::Decision,
    hint: &str,
    out: &mut dyn Write,
) -> Result<std::result::Result<(), String>> {
    use crate::contract_consent::Decision;
    use lillith_core::soul_contract::Answer;
    use lillith_mind::chat::CardChoice;
    let (tag, note) = match decision {
        Decision::Card(CardChoice::Clicked(Answer::Yes)) => return Ok(Ok(())),
        Decision::Card(CardChoice::Clicked(Answer::No)) => {
            contract.deny(id)?;
            writeln!(
                out,
                "[soul contract] declined on the card; nothing changed."
            )?;
            return Ok(Err("Ko said no on the card, so nothing ran".into()));
        }
        Decision::Card(CardChoice::TimedOut) => ("TIMEOUT", "no answer on the card; nothing ran"),
        Decision::Card(CardChoice::Cancelled) => ("CANCELLED", "the card was closed; nothing ran"),
        Decision::NoUi { display: false } => (
            "NO_UI",
            "no display for the card; a typed answer never approves; nothing ran",
        ),
        Decision::NoUi { display: true } => (
            "NO_UI",
            "card window could not open; a typed answer never approves; nothing ran",
        ),
    };
    let _ = contract.withdraw(id, tag, note);
    writeln!(
        out,
        "[soul contract] {}; discarded, nothing changed.",
        decision.refusal().unwrap_or(note)
    )?;
    if matches!(decision, Decision::NoUi { .. }) {
        writeln!(out, "{hint}")?;
    }
    Ok(Err(note.to_string()))
}

/// The console plan review: print every numbered item in full, then Ko
/// types `all`, a selection like `1,3-5`, or `none` (empty also declines).
/// If the selection includes deletes, he picks Trash/permanent and types the
/// phrase. Typing only PICKS: the chosen items run only after a clicked Yes
/// on the Soul Contract card. Without a display nothing is asked and
/// nothing runs (`NO_UI`). If the disk changed, the new plan is shown and
/// he is asked again.
pub(crate) fn review_plan_on_console(
    contract: &mut SoulContract,
    id: RequestId,
    stdin: &std::io::Stdin,
) -> Result<Result<String, String>> {
    let mut input = stdin.lock();
    review_plan(
        contract,
        id,
        &mut input,
        &mut std::io::stdout(),
        crate::contract_consent::display_available(),
        &mut crate::contract_consent::show_card,
    )
}

/// How Ko can do a plan himself when no card could be shown.
fn plan_hint() -> String {
    "Nothing ran and nothing changed. To do this by hand, carry out the numbered items \
     listed above yourself.\n\
     Or run `lillith chat` (or `lillith import-skills`) in a desktop session, where the \
     Soul Contract card can open (only a mouse click on its Yes runs the plan)."
        .to_string()
}

/// [`review_plan_on_console`] over any reader / writer and card (tested).
pub(crate) fn review_plan(
    contract: &mut SoulContract,
    mut id: RequestId,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    display: bool,
    card: &mut CardFn<'_>,
) -> Result<Result<String, String>> {
    use crate::contract_consent::{ask_card, Decision};
    use lillith_core::access::plan::{parse_selection, PlanApproval};
    loop {
        let Some(plan) = contract.plan(id).cloned() else {
            return Ok(Err("the request vanished".into()));
        };
        writeln!(
            out,
            "[soul contract] REVIEW THE PLAN — nothing has happened yet:"
        )?;
        writeln!(out, "{}", plan.listing())?;
        if !display {
            // Nothing to click: don't ask for picks that could never run.
            let d = Decision::NoUi { display: false };
            return Ok(settle_card(contract, id, d, &plan_hint(), out)?.map(|()| String::new()));
        }
        write!(
            out,
            "Pick which? `all`, numbers like `1,3-5`, or `none` (then confirm with a click on Yes on the Soul Contract card): "
        )?;
        out.flush()?;
        let mut line = String::new();
        input.read_line(&mut line)?;
        let selected = match parse_selection(&line, plan.len()) {
            Ok(Some(s)) if !s.is_empty() => s,
            Ok(_) => {
                contract.deny(id)?;
                writeln!(out, "[soul contract] declined; nothing changed.")?;
                return Ok(Err("Ko declined the whole plan".into()));
            }
            Err(e) => {
                writeln!(out, "[soul contract] {e}; declined, nothing changed.")?;
                contract.deny(id)?;
                return Ok(Err("Ko declined the whole plan".into()));
            }
        };
        let approval = PlanApproval::subset(&plan, selected.iter().copied());
        let mut signature = None;
        let mut deleting = String::new();
        if let Some(manifest) = plan.deletion_manifest(&selected) {
            writeln!(
                out,
                "The picked items include deleting:\n{}",
                manifest.listing()
            )?;
            write!(out, "Where to? [T]rash (default) / type `permanent` to erase for good / `n` cancels everything: ")?;
            out.flush()?;
            let mut mode = String::new();
            input.read_line(&mut mode)?;
            let mode = match mode.trim().to_ascii_lowercase().as_str() {
                "" | "t" | "trash" => DeleteMode::Trash,
                "permanent" => DeleteMode::Permanent,
                _ => {
                    contract.deny(id)?;
                    writeln!(out, "[soul contract] cancelled; nothing changed.")?;
                    return Ok(Err("Ko cancelled the plan".into()));
                }
            };
            write!(
                out,
                "To sign the list, type exactly: {}\n> ",
                manifest.phrase()
            )?;
            out.flush()?;
            let mut typed = String::new();
            input.read_line(&mut typed)?;
            match DeletionSignature::sign(&manifest, &typed, mode) {
                Ok(sig) => signature = Some(sig),
                Err(_) => {
                    contract.deny(id)?;
                    writeln!(out, "[soul contract] not signed; nothing changed.")?;
                    return Ok(Err("Ko did not sign, so nothing ran".into()));
                }
            }
            deleting = format!("\n\nDeleting ({}):\n{}", mode.as_str(), manifest.listing());
        }
        let picked: Vec<String> = plan
            .items
            .iter()
            .filter(|it| selected.contains(&it.n))
            .map(|it| format!("{}. {}", it.n, it.line))
            .collect();
        let numbers: Vec<String> = selected.iter().map(usize::to_string).collect();
        let text = CardText {
            title: "REVIEW THE PLAN".into(),
            summary: format!(
                "Run plan item(s) {}: {}",
                numbers.join(", "),
                plan.totals(Some(&selected))
            ),
            target: String::new(),
            detail: format!(
                "{}{deleting}\n\nOnly a click on Yes runs these; typing or saying yes doesn't.",
                picked.join("\n")
            ),
        };
        writeln!(
            out,
            "[soul contract] Picked item(s) {}. Nothing runs until you click Yes on the Soul Contract card (a typed yes never counts).",
            numbers.join(", ")
        )?;
        out.flush()?;
        let decision = ask_card(true, &text, &mut *card);
        if let Err(why) = settle_card(contract, id, decision, &plan_hint(), out)? {
            return Ok(Err(why));
        }
        match contract.approve_plan(id, approval, signature) {
            Ok(done) => {
                let text = done.output.unwrap_or_default();
                writeln!(out, "{text}")?;
                return Ok(Ok(text));
            }
            Err(ContractError::Changed { new_id }) => {
                writeln!(
                    out,
                    "[soul contract] things changed on disk since the plan; here is the new one."
                )?;
                id = new_id;
            }
            Err(e) => {
                writeln!(out, "[soul contract] {e}")?;
                let _ = contract.deny(id);
                return Ok(Err(e.to_string()));
            }
        }
    }
}

/// `'path'` for a POSIX shell.
fn sh_quote(p: &std::path::Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', r"'\''"))
}

/// How Ko can delete the listed roots himself when no card could be shown.
fn deletion_hint(roots: &[PathBuf]) -> String {
    let list: Vec<String> = roots.iter().map(|r| sh_quote(r)).collect();
    format!(
        "Nothing was deleted. To do this by hand, move them to the Trash in your file manager, \
         or in a terminal:\n  $ gio trash {}\n\
         Or run `lillith chat` in a desktop session, where the Soul Contract card can open \
         (only a mouse click on its Yes deletes).",
        list.join(" ")
    )
}

/// A deletion on the console: print EVERY file and folder (full paths,
/// sizes, totals), then Ko types `t` (Trash, default) or `permanent`, then
/// the exact list phrase. Typing only PICKS and signs the list: nothing is
/// deleted until a clicked Yes on the Soul Contract card. Without a display
/// nothing is asked and nothing is deleted (`NO_UI`, with by-hand steps).
/// If the files changed meanwhile, the new list is shown and he is asked
/// again. Anything else cancels (audited as DENY).
fn sign_on_console(
    contract: &mut SoulContract,
    mut id: RequestId,
    stdin: &mut dyn BufRead,
    out: &mut dyn Write,
    display: bool,
    card: &mut CardFn<'_>,
) -> Result<Result<String, String>> {
    use crate::contract_consent::{ask_card, Decision};
    loop {
        let Some(PendingAction::DeletePaths { manifest }) = contract
            .pending()
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, a)| a.clone())
        else {
            return Ok(Err("the request vanished".into()));
        };
        writeln!(
            out,
            "[soul contract] She wants to DELETE these — every file and folder:"
        )?;
        writeln!(out, "{}", manifest.listing())?;
        let hint = deletion_hint(&manifest.roots);
        if !display {
            let d = Decision::NoUi { display: false };
            return Ok(settle_card(contract, id, d, &hint, out)?.map(|()| String::new()));
        }
        write!(
            out,
            "Where to? [T]rash (default) / type `permanent` to erase for good / `n` cancels: "
        )?;
        out.flush()?;
        let mut mode = String::new();
        stdin.read_line(&mut mode)?;
        let mode = match mode.trim().to_ascii_lowercase().as_str() {
            "" | "t" | "trash" => DeleteMode::Trash,
            "permanent" => DeleteMode::Permanent,
            _ => {
                contract.deny(id)?;
                writeln!(out, "[soul contract] cancelled; nothing deleted.")?;
                return Ok(Err("Ko cancelled the deletion".into()));
            }
        };
        write!(
            out,
            "To sign the list, type exactly: {}\n> ",
            manifest.phrase()
        )?;
        out.flush()?;
        let mut typed = String::new();
        stdin.read_line(&mut typed)?;
        let Ok(sig) = DeletionSignature::sign(&manifest, &typed, mode) else {
            contract.deny(id)?;
            writeln!(out, "[soul contract] not signed; nothing deleted.")?;
            return Ok(Err("Ko did not sign".into()));
        };
        writeln!(
            out,
            "[soul contract] List signed. Nothing is deleted until you click Yes on the Soul Contract card (a typed yes never counts)."
        )?;
        out.flush()?;
        let text = CardText {
            title: "DELETE — SIGN THE LIST".into(),
            summary: format!(
                "Delete {} ({})",
                manifest.totals(),
                match mode {
                    DeleteMode::Trash => "to the Trash",
                    DeleteMode::Permanent => "PERMANENTLY",
                }
            ),
            target: manifest
                .roots
                .first()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            detail: format!(
                "{}\n\nOnly a click on Yes deletes; typing or saying yes doesn't.",
                manifest.listing()
            ),
        };
        let decision = ask_card(true, &text, &mut *card);
        if let Err(why) = settle_card(contract, id, decision, &hint, out)? {
            return Ok(Err(why));
        }
        match contract.sign_deletion(id, sig) {
            Ok(done) => {
                let text = done.output.unwrap_or_default();
                writeln!(out, "{text}")?;
                return Ok(Ok(text));
            }
            Err(ContractError::Changed { new_id }) => {
                writeln!(
                    out,
                    "[soul contract] the files changed since the list; here is the new one."
                )?;
                id = new_id;
            }
            Err(e) => {
                writeln!(out, "[soul contract] {e}")?;
                return Ok(Err(e.to_string()));
            }
        }
    }
}

/// Reads `input` line by line on its own thread and forwards every line.
/// A safeword line also calls `on_safeword` right away, before the chat loop
/// gets to it — so it works while an approved command runs inline.
pub(crate) fn spawn_console_reader<R, F>(
    input: R,
    on_safeword: F,
) -> std::sync::mpsc::Receiver<String>
where
    R: BufRead + Send + 'static,
    F: Fn() + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut input = input;
        loop {
            let mut line = String::new();
            match input.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if lillith_core::access::kill_switch::is_safeword(&line) {
                        on_safeword();
                    }
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

/// The console safeword: pull the kill switch (commands and the admin
/// helper die out-of-band) and say truthfully what happened.
fn console_safeword() {
    let r = lillith_core::access::kill_switch::stop_all();
    println!("\n[safeword] {}", r.chat_line());
}

/// Ctrl-C in `lillith chat`: the kill switch first (command children run in
/// their own process group and never see the terminal's Ctrl-C), then exit.
fn spawn_ctrl_c_kill() {
    tokio::spawn(async {
        if tokio::signal::ctrl_c().await.is_ok() {
            console_safeword();
            std::process::exit(130);
        }
    });
}

/// A [`BufRead`] over the console reader thread's lines.
pub(crate) struct ConsoleLines {
    rx: std::sync::mpsc::Receiver<String>,
    buf: Vec<u8>,
    pos: usize,
}

impl ConsoleLines {
    pub(crate) fn new(rx: std::sync::mpsc::Receiver<String>) -> Self {
        Self {
            rx,
            buf: Vec::new(),
            pos: 0,
        }
    }
}

impl std::io::Read for ConsoleLines {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = {
            let b = self.fill_buf()?;
            let n = b.len().min(out.len());
            out[..n].copy_from_slice(&b[..n]);
            n
        };
        self.consume(n);
        Ok(n)
    }
}

impl BufRead for ConsoleLines {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.pos >= self.buf.len() {
            self.pos = 0;
            // EOF (reader thread gone) reads as an empty buffer.
            self.buf = self.rx.recv().map(String::into_bytes).unwrap_or_default();
        }
        Ok(&self.buf[self.pos..])
    }

    fn consume(&mut self, n: usize) {
        self.pos = (self.pos + n).min(self.buf.len());
    }
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
    fn custom_config_path_system_prompt_reaches_cli_chat_session() {
        let dir =
            std::env::temp_dir().join(format!("lillith-cli-cfgprompt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("custom.toml");
        std::fs::write(
            &path,
            "[chat]\nenabled = true\n[llm]\nsystem_prompt = \"Custom-path persona.\"\n",
        )
        .unwrap();
        // Load it the way main.rs does for `--config <path>`.
        let full = lillith_core::config::Config::load(&path).unwrap();
        let s = new_session(
            &full.chat,
            &full.mind,
            &MemoryConfig::disabled(),
            Some(&path),
        );
        assert_eq!(s.system_prompt(), "Custom-path persona.");
        let mut chat = full.chat.clone();
        chat.system_prompt = "Chat wins.".into();
        let s = new_session(&chat, &full.mind, &MemoryConfig::disabled(), Some(&path));
        assert_eq!(s.system_prompt(), "Chat wins.");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_killed_bubble_with_a_card_up_is_cancelled_by_the_overlay() {
        let dir = std::env::temp_dir().join(format!("lillith-pump-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("audit.log");
        let sc = SoulContract::new(Some(log.clone()));
        let card = |answer: Option<&str>| {
            let a = PendingAction::start_model_server(vec!["/opt/llama-server".into()], true);
            sc.record("PROPOSED", &a);
            if let Some(d) = answer {
                sc.record(d, &a);
            }
            a.card_id().unwrap().to_string()
        };
        let cancelled = |id: &str| {
            std::fs::read_to_string(&log)
                .unwrap()
                .lines()
                .filter(|l| l.contains("\tCANCELLED\t") && l.contains(&format!("card={id}")))
                .count()
        };
        let feed = |id: &str, said_goodbye: bool| {
            let mut out = format!(
                "{}\n{}\n",
                encode("chat.opened", None),
                encode(SERVER_CARD_EVENT, Some(id))
            );
            if said_goodbye {
                out.push_str(&encode("chat.closed", None));
                out.push('\n');
            }
            let (tx, rx) = std::sync::mpsc::channel();
            pump(out.as_bytes(), &tx, None, Some(log.clone()));
            rx.try_iter().map(|e| e.name).collect::<Vec<_>>()
        };

        // SIGKILLed with the card up (no goodbye, nothing recorded).
        let killed = card(None);
        let names = feed(&killed, false);
        assert_eq!(
            names,
            ["chat.opened", "chat.closed"],
            "card id not forwarded"
        );
        assert_eq!(cancelled(&killed), 1);
        assert!(std::fs::read_to_string(&log)
            .unwrap()
            .contains(&format!("card={killed}\t{BUBBLE_GONE_NOTE}\n")));

        // Answered (or cancelled by the bubble's own close handler) before
        // it went away: nothing more is written.
        for d in ["DENY", "CANCELLED", "CONFIRM"] {
            let id = card(Some(d));
            let before = std::fs::read_to_string(&log).unwrap();
            feed(&id, d != "CONFIRM");
            assert_eq!(std::fs::read_to_string(&log).unwrap(), before, "{d}");
        }
        assert!(sc.open_model_server_cards().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_overlay_starts_only_an_unused_confirmed_card_with_the_same_command() {
        use lillith_panel::chat_bubble::approval_detail;
        let dir = std::env::temp_dir().join(format!("lillith-claim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("audit.log");
        let mut cfg = ChatConfig {
            endpoint: "http://127.0.0.1:8080".into(),
            ..ChatConfig::default()
        };
        cfg.server.binary = "/opt/llama/llama-server".into();
        cfg.server.model_path = "/opt/llama/m.gguf".into();
        let argv = lillith_mind::server::display_argv(&cfg).unwrap();
        let file = |confirm: bool| {
            let mut sc = SoulContract::new(Some(log.clone()));
            let a = PendingAction::start_model_server(argv.clone(), true);
            sc.record("PROPOSED", &a);
            let card = a.card_id().unwrap().to_string();
            if confirm {
                let id = sc.request(a);
                sc.confirm(id).unwrap();
            }
            card
        };
        let ask = |card: &str, argv: &[String]| {
            approve_spawn(Some(&approval_detail(card, argv)), &cfg, Some(log.clone()))
        };
        let count = |d: &str| {
            std::fs::read_to_string(&log)
                .unwrap()
                .matches(&format!("\t{d}\tstart_model_server\t"))
                .count()
        };

        // An approval that names no card starts nothing.
        assert!(approve_spawn(None, &cfg, Some(log.clone())).is_err());
        assert!(approve_spawn(Some("{}"), &cfg, Some(log.clone())).is_err());
        // Missing confirm: the card was only proposed.
        let proposed = file(false);
        let e = ask(&proposed, &argv).unwrap_err();
        assert!(e.contains("no clicked Yes"), "{e}");
        // Mismatch: a confirmed card, but not the command [chat.server] runs.
        let confirmed = file(true);
        let mut other = argv.clone();
        other.extend(["--host".into(), "0.0.0.0".into()]);
        let e = ask(&confirmed, &other).unwrap_err();
        assert!(e.contains("not what [chat.server] would run"), "{e}");
        // ...which used it up.
        assert!(ask(&confirmed, &argv).unwrap_err().contains("already used"));
        // The real thing: once.
        let good = file(true);
        assert_eq!(ask(&good, &argv), Ok((good.clone(), argv[0].clone())));
        // Reuse: the same card can't launch twice.
        let e = ask(&good, &argv).unwrap_err();
        assert!(e.contains("already used"), "{e}");
        assert_eq!((count("USED"), count("SPAWN_REFUSED")), (1, 4));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The overlay never starts `[chat.server]` on its own; the approved
    /// path (clicked Yes → `chat.server_approved`) does.
    #[cfg(unix)]
    #[tokio::test]
    async fn startup_never_spawns_the_server_until_approved() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("lillith-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ran = dir.join("ran");
        let bin = dir.join("fake-llama-server");
        std::fs::write(
            &bin,
            format!("#!/bin/sh\necho \"$@\" > '{}'\n", ran.display()),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.join("m.gguf"), b"").unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut cfg = ChatConfig {
            endpoint: format!("http://127.0.0.1:{port}"),
            ..ChatConfig::default()
        };
        cfg.server.binary = bin.display().to_string();
        cfg.server.model_path = dir.join("m.gguf").display().to_string();
        let g = ServerGuard::start(&cfg).await;
        std::thread::sleep(Duration::from_millis(300));
        assert!(!ran.exists(), "startup must not launch llama-server");
        // The bubble's approval event is part of the protocol.
        let ev = decode(&encode(SERVER_APPROVED_EVENT, None)).unwrap();
        assert_eq!(ev.name, SERVER_APPROVED_EVENT);
        assert!(g.handle().spawn_approved(&cfg, None));
        let t0 = Instant::now();
        while !ran.exists() && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(ran.exists(), "the approved start runs it");
        g.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

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

    #[test]
    fn console_reader_fires_the_safeword_before_the_chat_loop_reads() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static FIRED: AtomicUsize = AtomicUsize::new(0);
        let rx = spawn_console_reader(
            std::io::Cursor::new(b"hello\nred\nOOC: stop\nI read a red book\n".to_vec()),
            || {
                FIRED.fetch_add(1, Ordering::SeqCst);
            },
        );
        // Nobody reads the channel yet (the chat loop is "busy running a
        // command"); the reader thread must still pull the switch.
        let t0 = Instant::now();
        while FIRED.load(Ordering::SeqCst) < 2 && t0.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(FIRED.load(Ordering::SeqCst), 2, "red and OOC: stop only");
        // Every line still reaches the chat loop, in order.
        let mut lines = ConsoleLines::new(rx);
        let mut got = Vec::new();
        loop {
            let mut l = String::new();
            if lines.read_line(&mut l).unwrap() == 0 {
                break;
            }
            got.push(l.trim().to_string());
        }
        assert_eq!(got, ["hello", "red", "OOC: stop", "I read a red book"]);
    }

    #[test]
    fn console_lines_drive_the_plan_review_and_red_declines_it() {
        use lillith_core::access::plan::{ChangePlan, PlanRequest};
        let dir = std::env::temp_dir().join(format!("lillith-console-red-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut sc = SoulContract::new(None);
        let id = sc.request(PendingAction::Plan {
            plan: ChangePlan::build(
                vec![PlanRequest::Mkdir {
                    path: dir.join("A"),
                }],
                None,
                None,
            )
            .unwrap(),
        });
        let mut input = ConsoleLines::new(spawn_console_reader(
            std::io::Cursor::new(b"red\n".to_vec()),
            || {},
        ));
        let mut out = Vec::new();
        let r = review_plan(&mut sc, id, &mut input, &mut out, true, &mut |_| {
            panic!("the safeword must not reach the card")
        })
        .unwrap();
        assert!(r.is_err(), "the safeword is never an approval");
        assert!(!dir.join("A").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn console_safeword_kills_an_inline_command() {
        use lillith_core::access::commands::{run_child, was_stopped};
        // The "chat loop" is blocked running an approved command inline.
        let t = std::thread::spawn(|| {
            let mut c = std::process::Command::new("sh");
            c.arg("-c").arg("sleep 20");
            run_child(c, Duration::from_secs(30), 4096)
        });
        std::thread::sleep(Duration::from_millis(300));
        let started = Instant::now();
        // Ko types "red" on the console meanwhile.
        let _rx = spawn_console_reader(std::io::Cursor::new(b"red\n".to_vec()), || {
            lillith_core::access::kill_switch::stop_all();
        });
        let out = t.join().unwrap().unwrap();
        assert!(was_stopped(&out), "{out}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// A reader that fails the test if anything reads it.
    struct NoRead;

    impl std::io::Read for NoRead {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            panic!("the console approval read stdin");
        }
    }

    impl BufRead for NoRead {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            panic!("the console approval read stdin");
        }
        fn consume(&mut self, _: usize) {}
    }

    #[test]
    fn typed_y_on_stdin_never_approves_a_tool_action() {
        let dir = std::env::temp_dir().join(format!("lillith-console-y-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("out.txt");
        let action = || PendingAction::WriteFile {
            path: target.clone(),
            body: "hello".into(),
        };
        let cfg = ChatConfig::default();
        // `y`, `yes` and `Y` waiting on stdin, with and without a display
        // (the card can't open in a test either way).
        for display in [false, true] {
            for typed in ["y\n", "yes\n", "Y\n", "/contract yes\n"] {
                let mut session = ChatSession::new(&cfg, true);
                session.set_pending_tool(Some(action()));
                let mut stdin = std::io::Cursor::new(typed.as_bytes().to_vec());
                let mut contract = session.soul_contract();
                let a = session.take_pending().expect("pending");
                let fb = offer_on_card(&mut contract, a, display, |_| None);
                assert!(
                    !target.exists(),
                    "typed {typed:?} (display {display}) ran it"
                );
                assert!(!fb.is_empty());
                // Nothing was read: the typed yes is still there, unused.
                let mut left = String::new();
                stdin.read_line(&mut left).unwrap();
                assert_eq!(left, typed);
            }
        }
        // offer_pending never touches stdin for a tool action either.
        let mut session = ChatSession::new(&cfg, true);
        for display in [false, true] {
            session.set_pending_tool(Some(action()));
            let fb = offer_pending_with(&mut session, &mut NoRead, display, |_| None).unwrap();
            assert!(fb.is_some() && !target.exists());
        }
        // The same action does run after a clicked Yes (so "nothing ran"
        // above means refused, not broken).
        let mut contract = SoulContract::new(None);
        offer_on_card(&mut contract, action(), true, |_| {
            Some(lillith_mind::chat::CardChoice::Clicked(
                lillith_core::soul_contract::Answer::Yes,
            ))
        });
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn console_plan_review_runs_only_the_chosen_items_or_nothing() {
        use lillith_core::access::plan::{ChangePlan, PlanRequest};
        let dir = std::env::temp_dir().join(format!("lillith-console-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let reqs = || {
            vec![
                PlanRequest::Mkdir {
                    path: dir.join("A"),
                },
                PlanRequest::Write {
                    path: dir.join("b.txt"),
                    body: "b".into(),
                    skill: false,
                },
                PlanRequest::Mkdir {
                    path: dir.join("C"),
                },
            ]
        };
        // `none` declines: nothing written.
        let mut sc = SoulContract::new(None);
        let id = sc.request(PendingAction::Plan {
            plan: ChangePlan::build(reqs(), None, None).unwrap(),
        });
        let mut out = Vec::new();
        let r = review_plan(
            &mut sc,
            id,
            &mut "none\n".as_bytes(),
            &mut out,
            true,
            &mut |_| panic!("`none` must not reach the card"),
        )
        .unwrap();
        assert!(r.is_err());
        let shown = String::from_utf8(out).unwrap();
        assert!(
            shown.contains("REVIEW THE PLAN") && shown.contains("b.txt"),
            "{shown}"
        );
        assert!(!dir.join("A").exists() && !dir.join("b.txt").exists());
        // `1,3` runs just those, after the card's clicked Yes.
        let id = sc.request(PendingAction::Plan {
            plan: ChangePlan::build(reqs(), None, None).unwrap(),
        });
        let mut out = Vec::new();
        let r = review_plan(
            &mut sc,
            id,
            &mut "1,3\n".as_bytes(),
            &mut out,
            true,
            &mut |_| Some(clicked_yes()),
        )
        .unwrap();
        assert!(r.is_ok(), "{r:?}");
        assert!(dir.join("A").is_dir() && dir.join("C").is_dir());
        assert!(!dir.join("b.txt").exists(), "item 2 was not chosen");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn clicked_yes() -> lillith_mind::chat::CardChoice {
        lillith_mind::chat::CardChoice::Clicked(lillith_core::soul_contract::Answer::Yes)
    }

    /// Every way a card can end without a clicked Yes, plus "no card".
    fn not_yes() -> Vec<(&'static str, Option<lillith_mind::chat::CardChoice>)> {
        use lillith_core::soul_contract::Answer;
        use lillith_mind::chat::CardChoice;
        vec![
            ("DENY", Some(CardChoice::Clicked(Answer::No))),
            ("TIMEOUT", Some(CardChoice::TimedOut)),
            ("CANCELLED", Some(CardChoice::Cancelled)),
            ("NO_UI", None),
        ]
    }

    struct TmpDir(PathBuf);

    impl TmpDir {
        fn new(tag: &str) -> Self {
            let d = std::env::temp_dir().join(format!(
                "lillith-console2-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Self(d)
        }

        fn audit(&self) -> PathBuf {
            self.0.join("data").join("audit.log")
        }

        fn tags(&self) -> Vec<String> {
            std::fs::read_to_string(self.audit())
                .unwrap_or_default()
                .lines()
                .filter_map(|l| l.split('\t').nth(1).map(String::from))
                .collect()
        }

        fn contract(&self) -> SoulContract {
            std::fs::create_dir_all(self.0.join("data")).unwrap();
            SoulContract::new(Some(self.audit())).with_trash_dir(self.0.join("trash"))
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn victims(t: &TmpDir) -> PendingAction {
        use lillith_core::access::deletion::DeletionManifest;
        let a = t.0.join("victim-a.txt");
        let b = t.0.join("victim-b.txt");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        PendingAction::DeletePaths {
            manifest: DeletionManifest::build(&[a, b], Some(&t.0.join("data"))).unwrap(),
        }
    }

    fn victims_there(t: &TmpDir) -> bool {
        t.0.join("victim-a.txt").exists() && t.0.join("victim-b.txt").exists()
    }

    #[test]
    fn a_typed_phrase_never_deletes_only_the_cards_yes_does() {
        let typed = "t\nDELETE 2 ITEMS\n";
        // With a display: the typed phrase only signs the list; No / Esc /
        // timeout / closed (or the safeword) / no window delete nothing.
        for (tag, choice) in not_yes() {
            let t = TmpDir::new(&format!("del-{tag}"));
            let mut sc = t.contract();
            let id = sc.request(victims(&t));
            let mut out = Vec::new();
            let mut shown = 0;
            let r = sign_on_console(
                &mut sc,
                id,
                &mut typed.as_bytes(),
                &mut out,
                true,
                &mut |text| {
                    shown += 1;
                    assert!(text.detail.contains("victim-a.txt"), "{text:?}");
                    assert!(text.summary.contains("Trash"), "{text:?}");
                    choice
                },
            )
            .unwrap();
            assert_eq!(shown, 1, "{tag}: the card was asked once");
            assert!(r.is_err(), "{tag}: {r:?}");
            assert!(victims_there(&t), "{tag}: something was deleted");
            assert!(sc.pending().is_empty(), "{tag}: still pending");
            assert_eq!(t.tags(), [tag], "{tag}");
            let shown = String::from_utf8(out).unwrap();
            assert!(shown.contains("victim-b.txt"), "{shown}");
            assert_eq!(shown.contains("gio trash"), tag == "NO_UI", "{shown}");
        }
        // Without a display: nothing is asked, read or deleted (NO_UI), and
        // the console says how to do it by hand.
        for typed in [typed, "y\n", "yes\n", "DELETE 2 ITEMS\n"] {
            let t = TmpDir::new("del-nodisplay");
            let mut sc = t.contract();
            let id = sc.request(victims(&t));
            let mut out = Vec::new();
            let mut stdin = typed.as_bytes();
            let r = sign_on_console(&mut sc, id, &mut stdin, &mut out, false, &mut |_| {
                panic!("no display: no card")
            })
            .unwrap();
            assert!(r.is_err() && victims_there(&t));
            assert_eq!(stdin, typed.as_bytes(), "stdin was read");
            assert_eq!(t.tags(), ["NO_UI"]);
            let shown = String::from_utf8(out).unwrap();
            assert!(
                shown.contains("Nothing was deleted") && shown.contains("$ gio trash '"),
                "{shown}"
            );
        }
        // A typed y / wrong phrase never reaches the card and deletes nothing.
        for typed in ["y\ny\n", "t\nyes\n", "yes\n"] {
            let t = TmpDir::new("del-typo");
            let mut sc = t.contract();
            let id = sc.request(victims(&t));
            let r = sign_on_console(
                &mut sc,
                id,
                &mut typed.as_bytes(),
                &mut Vec::new(),
                true,
                &mut |_| panic!("an unsigned list must not reach the card"),
            )
            .unwrap();
            assert!(r.is_err() && victims_there(&t), "{typed:?}");
            assert_eq!(t.tags(), ["DENY"]);
        }
        // The signed list plus a clicked Yes deletes exactly the list.
        let t = TmpDir::new("del-yes");
        let mut sc = t.contract();
        let id = sc.request(victims(&t));
        let r = sign_on_console(
            &mut sc,
            id,
            &mut typed.as_bytes(),
            &mut Vec::new(),
            true,
            &mut |_| Some(clicked_yes()),
        )
        .unwrap();
        assert!(r.is_ok(), "{r:?}");
        assert!(!t.0.join("victim-a.txt").exists() && !t.0.join("victim-b.txt").exists());
        assert_eq!(t.tags(), ["CONFIRM-SIGNED-TRASH"]);
    }

    #[test]
    fn a_typed_selection_only_picks_plan_items_the_cards_yes_runs_them() {
        use lillith_core::access::plan::{ChangePlan, PlanRequest};
        let plan = |t: &TmpDir| PendingAction::Plan {
            plan: ChangePlan::build(
                (1..=5)
                    .map(|i| PlanRequest::Mkdir {
                        path: t.0.join(format!("D{i}")),
                    })
                    .collect(),
                None,
                None,
            )
            .unwrap(),
        };
        let made = |t: &TmpDir| -> Vec<usize> {
            (1..=5)
                .filter(|i| t.0.join(format!("D{i}")).is_dir())
                .collect()
        };
        // `1,3-5`, `all` and a typed `y` (= all) only pick: no card Yes =
        // nothing runs.
        for typed in ["1,3-5\n", "all\n", "y\n"] {
            for (tag, choice) in not_yes() {
                let t = TmpDir::new(&format!("plan-{tag}"));
                let mut sc = t.contract();
                let id = sc.request(plan(&t));
                let mut out = Vec::new();
                let mut seen = None;
                let r = review_plan(
                    &mut sc,
                    id,
                    &mut typed.as_bytes(),
                    &mut out,
                    true,
                    &mut |text| {
                        seen = Some(text.clone());
                        choice
                    },
                )
                .unwrap();
                assert!(r.is_err(), "{typed:?} {tag}");
                assert!(made(&t).is_empty(), "{typed:?} {tag}: ran {:?}", made(&t));
                assert!(sc.pending().is_empty());
                let seen = seen.expect("the card was asked");
                if typed == "1,3-5\n" {
                    assert!(seen.summary.contains("1, 3, 4, 5"), "{seen:?}");
                    assert!(!seen.detail.contains("D2"), "{seen:?}");
                }
                let tags = t.tags();
                assert!(tags.iter().any(|x| x == tag), "{tag}: {tags:?}");
                assert!(
                    !tags
                        .iter()
                        .any(|x| x == "APPROVED" || x.starts_with("RESULT")),
                    "{tags:?}"
                );
                let shown = String::from_utf8(out).unwrap();
                assert_eq!(shown.contains("by hand"), tag == "NO_UI", "{shown}");
            }
        }
        // No display: nothing is read, asked or run; by-hand steps printed.
        for typed in ["all\n", "1,3-5\n", "y\n"] {
            let t = TmpDir::new("plan-nodisplay");
            let mut sc = t.contract();
            let id = sc.request(plan(&t));
            let mut out = Vec::new();
            let mut stdin = typed.as_bytes();
            let r = review_plan(&mut sc, id, &mut stdin, &mut out, false, &mut |_| {
                panic!("no display: no card")
            })
            .unwrap();
            assert!(r.is_err() && made(&t).is_empty());
            assert_eq!(stdin, typed.as_bytes(), "stdin was read");
            assert!(t.tags().iter().any(|x| x == "NO_UI"), "{:?}", t.tags());
            let shown = String::from_utf8(out).unwrap();
            assert!(shown.contains("by hand"), "{shown}");
        }
        // `1,3-5` then a clicked Yes runs exactly those.
        let t = TmpDir::new("plan-yes");
        let mut sc = t.contract();
        let id = sc.request(plan(&t));
        let r = review_plan(
            &mut sc,
            id,
            &mut "1,3-5\n".as_bytes(),
            &mut Vec::new(),
            true,
            &mut |_| Some(clicked_yes()),
        )
        .unwrap();
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(made(&t), [1, 3, 4, 5]);
    }

    #[test]
    fn a_plans_deletes_need_the_signed_list_and_the_cards_yes() {
        use lillith_core::access::plan::{ChangePlan, PlanRequest};
        let t = TmpDir::new("plan-del");
        let victim = t.0.join("victim.txt");
        std::fs::write(&victim, "v").unwrap();
        let data = t.0.join("data");
        std::fs::create_dir_all(&data).unwrap();
        let mk = || PendingAction::Plan {
            plan: ChangePlan::build(
                vec![PlanRequest::Delete {
                    path: victim.clone(),
                }],
                Some(&data),
                None,
            )
            .unwrap(),
        };
        for (tag, choice) in not_yes() {
            let mut sc = t.contract();
            let id = sc.request(mk());
            let r = review_plan(
                &mut sc,
                id,
                &mut "all\nt\nDELETE 1 ITEM\n".as_bytes(),
                &mut Vec::new(),
                true,
                &mut |text| {
                    assert!(text.detail.contains("victim.txt"), "{text:?}");
                    choice
                },
            )
            .unwrap();
            assert!(r.is_err() && victim.exists(), "{tag}");
        }
        let mut sc = t.contract();
        let id = sc.request(mk());
        let r = review_plan(
            &mut sc,
            id,
            &mut "all\nt\nDELETE 1 ITEM\n".as_bytes(),
            &mut Vec::new(),
            true,
            &mut |_| Some(clicked_yes()),
        )
        .unwrap();
        assert!(r.is_ok(), "{r:?}");
        assert!(!victim.exists());
    }

    #[test]
    fn a_console_tool_action_audits_proposed_then_its_outcome() {
        let t = TmpDir::new("proposed");
        let target = t.0.join("out.txt");
        let mut sc = t.contract();
        let audit = t.audit();
        let fb = offer_on_card(
            &mut sc,
            PendingAction::WriteFile {
                path: target.clone(),
                body: "hi".into(),
            },
            true,
            |_| {
                let now = std::fs::read_to_string(&audit).unwrap_or_default();
                assert_eq!(now.lines().count(), 1, "{now}");
                assert!(now.contains("\tPROPOSED\twrite_file\t"), "{now}");
                Some(clicked_yes())
            },
        );
        assert!(!fb.is_empty() && target.exists());
        assert_eq!(t.tags(), ["PROPOSED", "CONFIRM"]);
        for (tag, choice) in not_yes() {
            let t = TmpDir::new(&format!("proposed-{tag}"));
            let mut sc = t.contract();
            let target = t.0.join("out.txt");
            offer_on_card(
                &mut sc,
                PendingAction::WriteFile {
                    path: target.clone(),
                    body: "hi".into(),
                },
                true,
                |_| choice,
            );
            assert!(!target.exists());
            assert_eq!(t.tags(), ["PROPOSED", tag]);
        }
    }
}
