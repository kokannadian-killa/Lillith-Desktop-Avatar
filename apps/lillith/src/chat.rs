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
use lillith_core::access::deletion::{DeleteMode, DeletionSignature};
use lillith_core::config::{ChatConfig, MindConfig};
use lillith_core::soul_contract::{ContractError, PendingAction, RequestId, SoulContract};
use lillith_core::Event;
use lillith_mind::chat::{default_history_path, ChatSession, Outcome, MAX_TOOL_CHAIN};
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

/// A chat session for `lillith chat`: `[mind]` attached, and the system
/// prompt resolved against `config_path` (`--config` or the default path),
/// so a custom config file's `[llm] system_prompt` applies too.
fn new_session(
    cfg: &ChatConfig,
    mind: &MindConfig,
    config_path: Option<&std::path::Path>,
) -> ChatSession {
    let mut session = ChatSession::new(cfg, false);
    if let Some(p) = config_path {
        session = session.with_config_file(&cfg.system_prompt, p);
    }
    session.with_mind(mind)
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
    config_path: Option<PathBuf>,
    pos: Option<(i32, i32)>,
    offline: bool,
    screenshot: Option<PathBuf>,
    demo: bool,
    demo_contract: bool,
) -> Result<()> {
    let emit = stdout_emit();
    let opts = BubbleOptions {
        pos,
        history: if demo { None } else { history_path(&cfg) },
        chat: cfg,
        mind,
        config_path,
        offline,
        emit,
        screenshot,
        demo,
        demo_contract,
    };
    run_chat_bubble(opts).map_err(|e| anyhow::anyhow!("chat bubble: {e}"))
}

/// `lillith chat [MESSAGE]`: headless chat through the same client code
/// (one message, or a prompt loop on stdin). Streams to stdout.
pub async fn cli_chat(
    cfg: ChatConfig,
    mind: MindConfig,
    config_path: Option<PathBuf>,
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
    let mut session = new_session(&cfg, &mind, config_path.as_deref());
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
/// file write, organize batch, command…), ask Yes/No on stdin. Only an
/// explicit yes (`y`, `yes`, or `/contract yes`; `/skill yes` is an alias)
/// runs it (audited); anything else discards it (also audited). A deletion
/// prints the full list and needs the typed phrase instead (see
/// [`sign_on_console`]). Returns the message to feed back to her (the tool
/// result or Ko's answer), if any.
fn offer_pending(session: &mut ChatSession, stdin: &mut dyn BufRead) -> Result<Option<String>> {
    let Some(action) = session.take_pending() else {
        return Ok(session.take_tool_feedback());
    };
    let mut contract = session.soul_contract();
    if matches!(action, PendingAction::Plan { .. }) {
        let id = contract.request(action.clone());
        let res = review_plan(&mut contract, id, stdin, &mut std::io::stdout())?;
        return Ok(Some(ChatSession::answer_feedback(
            &action,
            res.as_deref().map_err(|e| e.as_str()),
        )));
    }
    if matches!(action, PendingAction::DeletePaths { .. }) {
        let id = contract.request(action.clone());
        let res = sign_on_console(&mut contract, id, stdin)?;
        return Ok(Some(ChatSession::answer_feedback(
            &action,
            res.as_deref().map_err(|e| e.as_str()),
        )));
    }
    let prompt = action.prompt();
    if let Some(d) = &prompt.detail {
        println!("{d}");
    }
    print!(
        "[soul contract] She wants to {}. Allow? [y/N] ",
        action.summary()
    );
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    stdin.read_line(&mut answer)?;
    let id = contract.request(action.clone());
    let res = if is_yes(&answer) {
        match contract.confirm(id) {
            Ok(done) => {
                let mut out = String::new();
                if let Some(p) = &done.path {
                    out.push_str(&format!("done: {}\n", p.display()));
                }
                if let Some(o) = &done.output {
                    out.push_str(o);
                }
                println!("[soul contract] {out}");
                Ok(out)
            }
            Err(e) => {
                println!("[soul contract] failed: {e}");
                Err(format!("it failed: {e}"))
            }
        }
    } else {
        contract.deny(id)?;
        println!("[soul contract] discarded; nothing changed.");
        Err("Ko said no".to_string())
    };
    Ok(Some(ChatSession::answer_feedback(
        &action,
        res.as_deref().map_err(|e| e.as_str()),
    )))
}

/// The console plan review: print every numbered item in full, then Ko
/// types `all`, a selection like `1,3-5`, or `none` (empty also declines).
/// If the selection includes deletes, he picks Trash/permanent and types the
/// phrase. If the disk changed, the new plan is shown and he is asked again.
pub(crate) fn review_plan_on_console(
    contract: &mut SoulContract,
    id: RequestId,
    stdin: &std::io::Stdin,
) -> Result<Result<String, String>> {
    let mut input = stdin.lock();
    review_plan(contract, id, &mut input, &mut std::io::stdout())
}

/// [`review_plan_on_console`] over any reader / writer (tested).
pub(crate) fn review_plan(
    contract: &mut SoulContract,
    mut id: RequestId,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<Result<String, String>> {
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
        write!(
            out,
            "Approve which? `all`, numbers like `1,3-5`, or `none`: "
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
        if let Some(manifest) = plan.deletion_manifest(&selected) {
            writeln!(
                out,
                "The approved items include deleting:\n{}",
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
            write!(out, "To sign, type exactly: {}\n> ", manifest.phrase())?;
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

/// A deletion on the console: print EVERY file and folder (full paths,
/// sizes, totals), then Ko types `t` (Trash, default) or `permanent`, then
/// the exact phrase. If the files changed meanwhile, the new list is shown
/// and he is asked again. Anything else cancels (audited as DENY).
fn sign_on_console(
    contract: &mut SoulContract,
    mut id: RequestId,
    stdin: &mut dyn BufRead,
) -> Result<Result<String, String>> {
    loop {
        let Some(PendingAction::DeletePaths { manifest }) = contract
            .pending()
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, a)| a.clone())
        else {
            return Ok(Err("the request vanished".into()));
        };
        println!("[soul contract] She wants to DELETE these — every file and folder:");
        println!("{}", manifest.listing());
        print!("Where to? [T]rash (default) / type `permanent` to erase for good / `n` cancels: ");
        let _ = std::io::stdout().flush();
        let mut mode = String::new();
        stdin.read_line(&mut mode)?;
        let mode = match mode.trim().to_ascii_lowercase().as_str() {
            "" | "t" | "trash" => DeleteMode::Trash,
            "permanent" => DeleteMode::Permanent,
            _ => {
                contract.deny(id)?;
                println!("[soul contract] cancelled; nothing deleted.");
                return Ok(Err("Ko cancelled the deletion".into()));
            }
        };
        print!("To sign, type exactly: {}\n> ", manifest.phrase());
        let _ = std::io::stdout().flush();
        let mut typed = String::new();
        stdin.read_line(&mut typed)?;
        let Ok(sig) = DeletionSignature::sign(&manifest, &typed, mode) else {
            contract.deny(id)?;
            println!("[soul contract] not signed; nothing deleted.");
            return Ok(Err("Ko did not sign".into()));
        };
        match contract.sign_deletion(id, sig) {
            Ok(done) => {
                let out = done.output.unwrap_or_default();
                println!("{out}");
                return Ok(Ok(out));
            }
            Err(ContractError::Changed { new_id }) => {
                println!("[soul contract] the files changed since the list; here is the new one.");
                id = new_id;
            }
            Err(e) => {
                println!("[soul contract] {e}");
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

/// An explicit yes at the console prompt.
fn is_yes(answer: &str) -> bool {
    let a = answer.trim().to_ascii_lowercase();
    let a = a
        .strip_prefix("/contract")
        .or_else(|| a.strip_prefix("/skill"))
        .unwrap_or(&a)
        .trim();
    matches!(a, "y" | "yes")
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
        let s = new_session(&full.chat, &full.mind, Some(&path));
        assert_eq!(s.system_prompt(), "Custom-path persona.");
        let mut chat = full.chat.clone();
        chat.system_prompt = "Chat wins.".into();
        let s = new_session(&chat, &full.mind, Some(&path));
        assert_eq!(s.system_prompt(), "Chat wins.");
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
        let r = review_plan(&mut sc, id, &mut input, &mut out).unwrap();
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

    #[test]
    fn console_yes_is_explicit() {
        for y in ["y", "YES\n", " /contract yes ", "/skill y"] {
            assert!(is_yes(y), "{y:?}");
        }
        for n in ["", "n", "no", "/contract no", "yep", "/contract", "sure"] {
            assert!(!is_yes(n), "{n:?}");
        }
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
        let r = review_plan(&mut sc, id, &mut "none\n".as_bytes(), &mut out).unwrap();
        assert!(r.is_err());
        let shown = String::from_utf8(out).unwrap();
        assert!(
            shown.contains("REVIEW THE PLAN") && shown.contains("b.txt"),
            "{shown}"
        );
        assert!(!dir.join("A").exists() && !dir.join("b.txt").exists());
        // `1,3` runs just those.
        let id = sc.request(PendingAction::Plan {
            plan: ChangePlan::build(reqs(), None, None).unwrap(),
        });
        let mut out = Vec::new();
        let r = review_plan(&mut sc, id, &mut "1,3\n".as_bytes(), &mut out).unwrap();
        assert!(r.is_ok(), "{r:?}");
        assert!(dir.join("A").is_dir() && dir.join("C").is_dir());
        assert!(!dir.join("b.txt").exists(), "item 2 was not chosen");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
