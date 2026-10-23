//! A chat conversation with Lillith over a local llama-server.
//!
//! [`ChatSession`] keeps the history, builds the request (system prompt +
//! the last `history_turns` turns + the new message), streams the reply and
//! turns every failure into an in-character line so the bubble never shows
//! a raw error. The history can be persisted as JSON between bubble openings.

use crate::fstools::{parse_tool_requests, MindTools, Planned};
use crate::knowledge::{parse_skill_proposal, KnowledgeBase, SkillDraft};
use crate::llm::{ChatOptions, Flavor, LlmClient, LlmError, Message};
use crate::persona;
use lillith_core::access::plan::PlanRequest;
use lillith_core::config::{ChatConfig, MindConfig};
use lillith_core::soul_contract::{PendingAction, SoulContract};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How a turn ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The model answered (the full reply).
    Reply(String),
    /// The model could not answer; `line` is the in-character fallback
    /// (already streamed to the callback), `error` the technical reason.
    Fallback { line: String, error: String },
}

impl Outcome {
    pub fn text(&self) -> &str {
        match self {
            Outcome::Reply(t) => t,
            Outcome::Fallback { line, .. } => line,
        }
    }
}

/// One conversation.
#[derive(Debug)]
pub struct ChatSession {
    client: Option<LlmClient>,
    system_prompt: String,
    opts: ChatOptions,
    history_turns: usize,
    history: Vec<Message>,
    save_path: Option<PathBuf>,
    salt: usize,
    /// Knowledge + skills she may read and inject (Phase 1). `None` until
    /// [`ChatSession::with_mind`] is called.
    knowledge: Option<KnowledgeBase>,
    /// Where confirmed skills are written (the skills folder).
    skills_dir: Option<PathBuf>,
    /// Where skill writes/denials are logged.
    audit_log: Option<PathBuf>,
    /// A skill the model proposed in the last reply, awaiting a Soul Contract
    /// Yes/No. Never written on its own.
    pending_skill: Option<SkillDraft>,
    /// The scoped file/shell tool API (Phase 2). `None` until [`with_mind`]
    /// attaches an enabled `[mind]`.
    ///
    /// [`with_mind`]: ChatSession::with_mind
    tools: Option<MindTools>,
    /// The plan (file changes + program runs, as one
    /// [`PendingAction::Plan`]) the model proposed last reply, awaiting Ko's
    /// review. Never run on its own.
    pending_tool: Option<PendingAction>,
    /// Output from a read tool that ran this turn (reads need no popup), or a
    /// one-line note when a tool was refused.
    last_tool_output: Option<String>,
    /// Which tool produced `last_tool_output`.
    last_tool_op: Option<&'static str>,
}

/// Most tool calls chained after one message of Ko's (each tool result is fed
/// back to her; she may ask for another tool, up to this many).
pub const MAX_TOOL_CHAIN: usize = 3;

/// Most bytes of a tool result fed back to the model.
pub const TOOL_RESULT_MAX: usize = 6000;

/// Most messages kept on disk.
const MAX_SAVED: usize = 200;

impl ChatSession {
    /// Session for `cfg`. `offline` (= `--no-llm`) answers every message
    /// with [`persona::OFFLINE_LINE`] without touching the network.
    pub fn new(cfg: &ChatConfig, offline: bool) -> Self {
        if !offline {
            // Checked here (config read) and again before every request.
            if let Err(e) = crate::llm::local_base(&cfg.endpoint) {
                tracing::warn!(endpoint = %cfg.endpoint, "[chat] endpoint refused: {e}");
            }
        }
        let client = (!offline).then(|| {
            let mut c = LlmClient::with_timeout(
                Flavor::OpenAiCompat,
                cfg.endpoint.clone(),
                cfg.model.clone(),
                Duration::from_secs_f32(cfg.timeout_s.max(1.0)),
            );
            if !cfg.api_key.is_empty() {
                c.api_key = Some(cfg.api_key.clone());
            }
            c
        });
        let system_prompt = persona::resolve_system_prompt(
            &cfg.system_prompt,
            persona::default_llm_system_prompt().as_deref(),
        );
        Self {
            client,
            system_prompt,
            opts: ChatOptions {
                temperature: cfg.temperature,
                max_tokens: cfg.max_tokens.max(1),
            },
            history_turns: cfg.history_turns,
            history: Vec::new(),
            save_path: None,
            salt: std::process::id() as usize,
            knowledge: None,
            skills_dir: None,
            audit_log: None,
            pending_skill: None,
            tools: None,
            pending_tool: None,
            last_tool_output: None,
            last_tool_op: None,
        }
    }

    /// Attaches the knowledge/skills base from `[mind]`: indexes both folders
    /// (creating them) and remembers the skills folder + audit log for gated
    /// skill writes. A disabled config attaches an inert base (no injection,
    /// no authoring).
    pub fn with_mind(mut self, cfg: &MindConfig) -> Self {
        self.knowledge = Some(KnowledgeBase::load(cfg));
        if cfg.enabled {
            self.skills_dir = cfg.resolved_skills_dir();
            self.audit_log = cfg.audit_log_path();
            self.tools = Some(MindTools::from_mind(cfg));
        }
        self
    }

    /// The persona system prompt, with any relevant knowledge/skill context
    /// for `user` appended (within the configured token budget).
    fn system_with_context(&self, user: &str) -> String {
        let mut s = self.system_prompt.clone();
        let has_kb = self
            .knowledge
            .as_ref()
            .map(|kb| kb.is_enabled())
            .unwrap_or(false);
        if let Some(caps) = crate::capabilities::section(self.tools.as_ref(), has_kb) {
            s.push_str("\n\n");
            s.push_str(&caps);
        }
        if let Some(ctx) = self.knowledge.as_ref().and_then(|kb| kb.context_for(user)) {
            s.push_str("\n\n");
            s.push_str(&ctx);
        }
        s
    }

    /// The full system prompt as sent for `user` (persona + the tools section
    /// + any matching knowledge). For tests and `--print-prompt`-style checks.
    pub fn full_system_prompt(&self, user: &str) -> String {
        self.system_with_context(user)
    }

    /// The last tool result as the message to feed back to her (and clears
    /// it), so she can tell Ko what she found. `None` when no tool ran.
    pub fn take_tool_feedback(&mut self) -> Option<String> {
        let out = self.last_tool_output.take()?;
        let op = self.last_tool_op.take().unwrap_or("tool");
        Some(crate::capabilities::result_message(
            op,
            &out,
            TOOL_RESULT_MAX,
        ))
    }

    /// The message that tells her how Ko answered a Soul Contract card.
    pub fn answer_feedback(action: &PendingAction, result: Result<&str, &str>) -> String {
        let text = match result {
            Ok(out) if matches!(action, PendingAction::Plan { .. }) => format!(
                "Ko reviewed my plan ({}). Only what he approved ran:\n{out}",
                action.summary()
            ),
            Ok(out) => format!("Ko said YES to: {}\n{out}", action.summary()),
            Err(why) => format!("{}: {}\nNothing was done.", action.summary(), why),
        };
        crate::capabilities::result_message(action.kind(), &text, TOOL_RESULT_MAX)
    }

    /// Looks for a `skills` / `lillith-skills` folder on Ko's desktop
    /// (`[mind.access] import_desktop_skills`, default on) and, when it has
    /// anything new, files the exact copies as a plan for Ko to review
    /// (see [`pending_action`](Self::pending_action)). Nothing is copied
    /// here. Returns a one-line note when a plan was filed.
    pub fn import_desktop_skills(&mut self) -> Option<String> {
        let tools = self.tools.as_ref()?;
        let a = tools.access();
        if !a.skills || !a.import_desktop_skills || self.pending_tool.is_some() {
            return None;
        }
        let dir = tools.skills_dir()?;
        let mut steps = Vec::new();
        for src in lillith_core::access::skill_import::find_desktop_skills() {
            match lillith_core::access::skill_import::plan_import(&src, dir) {
                Ok(p) => steps.extend(p.copies.into_iter().map(|c| PlanRequest::SkillCopy {
                    from: c.src,
                    to: c.dest,
                    conflict_with: c.conflict_with,
                })),
                Err(e) => tracing::warn!("skills import: {}: {e}", src.display()),
            }
        }
        if steps.is_empty() {
            return None;
        }
        match tools.build_plan(steps) {
            Ok(plan) => {
                let note = format!(
                    "I found skills on your desktop. Here's exactly what I'd copy into my skills folder ({}); approve what you want.",
                    plan.totals(None)
                );
                self.pending_tool = Some(PendingAction::Plan { plan });
                Some(note)
            }
            Err(e) => {
                tracing::warn!("skills import plan: {e}");
                None
            }
        }
    }

    /// Replaces the plan awaiting review (e.g. the re-built plan after the
    /// disk changed: `ContractError::Changed`).
    pub fn set_pending_tool(&mut self, action: Option<PendingAction>) {
        self.pending_tool = action;
    }

    /// A skill the model proposed in its last reply, if any (awaiting Yes/No).
    pub fn pending_skill(&self) -> Option<&SkillDraft> {
        self.pending_skill.as_ref()
    }

    /// Removes and returns the pending skill draft.
    pub fn take_pending_skill(&mut self) -> Option<SkillDraft> {
        self.pending_skill.take()
    }

    /// The skills folder confirmed writes land in.
    pub fn skills_dir(&self) -> Option<&Path> {
        self.skills_dir.as_deref()
    }

    /// The gated [`PendingAction`] for the pending skill, resolved into the
    /// skills folder. `None` if there is no draft or no skills folder.
    pub fn pending_skill_action(&self) -> Option<PendingAction> {
        let draft = self.pending_skill.as_ref()?;
        let dir = self.skills_dir.as_ref()?;
        Some(draft.to_action(dir))
    }

    /// A fresh [`SoulContract`] bound to this session's audit log. The caller
    /// files the pending action, shows a Yes/No prompt, then confirms/denies.
    pub fn soul_contract(&self) -> SoulContract {
        SoulContract::new(self.audit_log.clone())
    }

    /// The scoped tool API, if `[mind]` is enabled.
    pub fn tools(&self) -> Option<&MindTools> {
        self.tools.as_ref()
    }

    /// A mutating tool (write/shell) the model proposed, awaiting Yes/No.
    pub fn pending_tool(&self) -> Option<&PendingAction> {
        self.pending_tool.as_ref()
    }

    /// Output a read tool produced this turn, or a note about a refused tool.
    pub fn last_tool_output(&self) -> Option<&str> {
        self.last_tool_output.as_deref()
    }

    /// The single action currently awaiting a Soul Contract Yes/No: a pending
    /// tool (write/shell) if any, otherwise a pending skill resolved into the
    /// skills folder. This is what a generic `/contract yes|no` acts on.
    pub fn pending_action(&self) -> Option<PendingAction> {
        self.pending_tool
            .clone()
            .or_else(|| self.pending_skill_action())
    }

    /// Removes and returns the action awaiting Yes/No (tool first, then skill).
    pub fn take_pending(&mut self) -> Option<PendingAction> {
        if let Some(a) = self.pending_tool.take() {
            return Some(a);
        }
        let action = self.pending_skill_action();
        if action.is_some() {
            self.pending_skill = None;
        }
        action
    }

    /// Re-resolves the system prompt against the config file at `path`
    /// (for `--config` users): `[chat] system_prompt` (`chat_override`)
    /// still wins, then that file's `[llm] system_prompt`, then the built-in
    /// persona.
    pub fn with_config_file(mut self, chat_override: &str, path: &Path) -> Self {
        self.system_prompt = persona::resolve_system_prompt(
            chat_override,
            persona::llm_system_prompt(path).as_deref(),
        );
        self
    }

    /// Loads (if present) and keeps saving the history at `path`.
    pub fn with_history_file(mut self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        if let Ok(s) = std::fs::read_to_string(&path) {
            match serde_json::from_str::<Vec<Message>>(&s) {
                Ok(h) => self.history = h,
                Err(e) => tracing::warn!("{}: {e}; starting a fresh chat", path.display()),
            }
        }
        self.save_path = Some(path);
        self
    }

    pub fn history(&self) -> &[Message] {
        &self.history
    }

    pub fn clear(&mut self) {
        self.history.clear();
        self.save();
    }

    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    /// The messages sent for a new `user` text.
    pub fn request_messages(&self, user: &str) -> Vec<Message> {
        let keep = self.history_turns.saturating_mul(2);
        let start = self.history.len().saturating_sub(keep);
        let mut m = Vec::with_capacity(keep + 2);
        m.push(Message::system(self.system_with_context(user)));
        m.extend(
            self.history[start..]
                .iter()
                .filter(|h| h.role == "user" || h.role == "assistant")
                .cloned(),
        );
        m.push(Message::user(user));
        m
    }

    /// Sends `user`, streaming the reply through `on_token`. Failures are
    /// streamed too, as one in-character line, and are not stored in the
    /// history (so the next try resends the same context).
    pub async fn send<F: FnMut(&str)>(&mut self, user: &str, mut on_token: F) -> Outcome {
        let user = user.trim();
        let messages = self.request_messages(user);
        self.salt = self.salt.wrapping_add(1);
        let result = match &self.client {
            None => {
                on_token(persona::OFFLINE_LINE);
                return Outcome::Fallback {
                    line: persona::OFFLINE_LINE.into(),
                    error: "chat disabled (--no-llm)".into(),
                };
            }
            Some(c) => c.chat_stream(&messages, self.opts, &mut on_token).await,
        };
        match result {
            Ok(reply) if !reply.trim().is_empty() => {
                let reply = reply.trim().to_string();
                self.history.push(Message::user(user));
                self.history.push(Message::assistant(reply.clone()));
                self.save();
                // A reply may propose a skill; hold it for the Soul Contract
                // gate (never written here).
                self.pending_skill = parse_skill_proposal(&reply);
                // A reply may also ask for a file/shell tool. Reads run now
                // (no popup, but audited); writes/shell are held for the gate.
                self.handle_tool_request(&reply);
                Outcome::Reply(reply)
            }
            Ok(_) => self.fallback(LlmError::Response("empty reply".into()), &mut on_token),
            Err(e) => self.fallback(e, &mut on_token),
        }
    }

    /// Parses every tool block in `reply`. Reads, listings, searches and
    /// "is it installed?" run straight away (audited). Every change and
    /// program run — plus a proposed skill — goes into ONE combined plan, in
    /// the order she wrote them, held for Ko's review. A refused request
    /// becomes a one-line note. Never mutates the machine here.
    fn handle_tool_request(&mut self, reply: &str) {
        self.pending_tool = None;
        self.last_tool_output = None;
        self.last_tool_op = None;
        let Some(tools) = self.tools.as_ref() else {
            return;
        };
        let audit = tools.audit_log().map(|p| p.to_path_buf());
        let mut steps: Vec<PlanRequest> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        let mut first_op = None;
        let reqs = parse_tool_requests(reply);
        let many = reqs.len() > 1;
        for req in reqs {
            let op = req.op();
            first_op.get_or_insert(op);
            let tag = |t: String| if many { format!("[{op}] {t}") } else { t };
            match tools.plan(req) {
                Ok(Planned::Info(text)) => notes.push(tag(text)),
                Ok(Planned::Read(action)) => {
                    let mut sc = SoulContract::new(audit.clone());
                    match sc.run_now(action) {
                        Ok(done) => notes.push(tag(done.output.unwrap_or_default())),
                        Err(e) => notes.push(tag(format!("(tool error: {e})"))),
                    }
                }
                Ok(Planned::Change(s)) => steps.extend(s),
                Err(e) => notes.push(tag(format!("(tool refused: {e})"))),
            }
        }
        // A proposed skill joins the same plan (the skills folder is only
        // ever written through Ko's review).
        if let (Some(draft), Some(dir)) = (&self.pending_skill, &self.skills_dir) {
            if let PendingAction::WriteSkill { path, body } = draft.to_action(dir) {
                steps.push(PlanRequest::Write {
                    path,
                    body,
                    skill: true,
                });
                self.pending_skill = None;
            }
        }
        if !steps.is_empty() {
            match tools.build_plan(steps) {
                Ok(plan) => self.pending_tool = Some(PendingAction::Plan { plan }),
                Err(e) => {
                    first_op.get_or_insert("plan");
                    notes.push(format!("(plan refused: {e}; nothing was done)"))
                }
            }
        }
        if !notes.is_empty() {
            self.last_tool_op = first_op;
            self.last_tool_output = Some(notes.join("\n"));
        }
    }

    fn fallback<F: FnMut(&str)>(&self, e: LlmError, on_token: &mut F) -> Outcome {
        let lines = match &e {
            LlmError::Status { status: 503, .. } => persona::SERVER_LOADING_LINES,
            e if e.is_down() => persona::SERVER_DOWN_LINES,
            _ => persona::SERVER_ERROR_LINES,
        };
        let line = persona::pick(lines, self.salt).to_string();
        tracing::warn!("chat: {e}");
        on_token(&line);
        Outcome::Fallback {
            line,
            error: e.to_string(),
        }
    }

    fn save(&self) {
        let Some(path) = &self.save_path else { return };
        let start = self.history.len().saturating_sub(MAX_SAVED);
        if let Err(e) = write_json(path, &self.history[start..]) {
            tracing::warn!("could not save chat history to {}: {e}", path.display());
        }
    }
}

fn write_json(path: &Path, h: &[Message]) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(h)?)?;
    std::fs::rename(tmp, path)
}

/// `$XDG_DATA_HOME/lillith/chat_history.json`, else
/// `~/.local/share/lillith/chat_history.json`.
/// Windows: `%LOCALAPPDATA%\lillith\chat_history.json`.
pub fn default_history_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    #[cfg(windows)]
    let base = base.or_else(|| {
        std::env::var_os("LOCALAPPDATA")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    });
    let base =
        base.or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("lillith").join("chat_history.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A one-shot fake llama-server: answers each connection with `reply`
    /// (raw HTTP) and hands back the request it got.
    async fn fake_server(
        replies: Vec<String>,
    ) -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            for reply in replies {
                let (mut s, _) = l.accept().await.unwrap();
                let mut req = Vec::new();
                let mut buf = [0u8; 8192];
                // Read headers + body (Content-Length).
                loop {
                    let n = s.read(&mut buf).await.unwrap();
                    req.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&req).to_string();
                    if let Some(h) = text.find("\r\n\r\n") {
                        let len = text[..h]
                            .lines()
                            .find_map(|l| {
                                let l = l.to_ascii_lowercase();
                                l.strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if req.len() >= h + 4 + len {
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                tx.send(String::from_utf8_lossy(&req).to_string()).unwrap();
                s.write_all(reply.as_bytes()).await.unwrap();
                s.shutdown().await.ok();
            }
        });
        (url, rx)
    }

    fn sse_reply(parts: &[&str]) -> String {
        let mut body = String::new();
        for p in parts {
            body.push_str(&format!(
                "data: {}\n\n",
                serde_json::json!({"choices":[{"delta":{"content":p},"finish_reason":null}]})
            ));
        }
        body.push_str("data: [DONE]\n\n");
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn cfg(url: &str) -> ChatConfig {
        ChatConfig {
            endpoint: url.into(),
            history_turns: 1,
            ..ChatConfig::default()
        }
    }

    #[tokio::test]
    async fn streams_and_keeps_history() {
        let (url, mut reqs) = fake_server(vec![
            sse_reply(&["Hey", " cutie", "~"]),
            sse_reply(&["Again", "?"]),
            sse_reply(&["Third"]),
        ])
        .await;
        let mut s = ChatSession::new(&cfg(&url), false);
        let mut toks = Vec::new();
        let out = s.send("hi", |t| toks.push(t.to_string())).await;
        assert_eq!(out, Outcome::Reply("Hey cutie~".into()));
        assert_eq!(toks, ["Hey", " cutie", "~"]);
        let r1 = reqs.recv().await.unwrap();
        assert!(r1.starts_with("POST /v1/chat/completions"));
        assert!(r1.contains("\"stream\":true"));
        // Built-in persona = lillith-setup's LILY_SYSTEM_PROMPT (single source).
        assert!(r1.contains("You are Lily (full name Lillith), an ancient succubus"));
        s.send("again", |_| {}).await;
        s.send("third", |_| {}).await;
        // history_turns = 1: only the last turn goes along.
        let _ = reqs.recv().await.unwrap();
        let r3 = reqs.recv().await.unwrap();
        assert!(r3.contains("\"again\"") && r3.contains("Again?"));
        assert!(!r3.contains("\"hi\""));
        assert_eq!(s.history().len(), 6);
    }

    #[tokio::test]
    async fn server_down_is_in_character() {
        // Bind and drop: nothing listens there any more.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let mut s = ChatSession::new(&cfg(&format!("http://127.0.0.1:{port}")), false);
        let mut shown = String::new();
        let out = s.send("hello?", |t| shown.push_str(t)).await;
        match out {
            Outcome::Fallback { line, error } => {
                assert!(
                    persona::SERVER_DOWN_LINES.contains(&line.as_str()),
                    "{line} / {error}"
                );
                assert_eq!(shown, line);
            }
            o => panic!("expected fallback, got {o:?}"),
        }
        assert!(s.history().is_empty(), "failed turns are not stored");
    }

    #[tokio::test]
    async fn loading_503_and_offline() {
        let body = "{\"error\":{\"code\":503,\"message\":\"Loading model\"}}";
        let (url, _r) = fake_server(vec![format!(
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )])
        .await;
        let mut s = ChatSession::new(&cfg(&url), false);
        let out = s.send("hi", |_| {}).await;
        assert!(
            persona::SERVER_LOADING_LINES.contains(&out.text()),
            "{out:?}"
        );

        let mut off = ChatSession::new(&ChatConfig::default(), true);
        assert_eq!(off.send("hi", |_| {}).await.text(), persona::OFFLINE_LINE);
    }

    #[tokio::test]
    async fn history_file_round_trip() {
        let dir = std::env::temp_dir().join(format!("lillith-chat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("h.json");
        let (url, _r) = fake_server(vec![sse_reply(&["Saved"])]).await;
        let mut s = ChatSession::new(&cfg(&url), false).with_history_file(&path);
        s.send("remember me", |_| {}).await;
        let s2 = ChatSession::new(&cfg(&url), false).with_history_file(&path);
        assert_eq!(s2.history(), s.history());
        assert_eq!(s2.history()[1].content, "Saved");
        let custom = ChatConfig {
            system_prompt: "Be brief.".into(),
            ..ChatConfig::default()
        };
        assert_eq!(
            ChatSession::new(&custom, true).request_messages("x")[0].content,
            "Be brief."
        );
        // [llm] system_prompt from a --config file applies unless [chat] overrides it.
        let cfg_file = dir.join("config.toml");
        std::fs::write(&cfg_file, "[llm]\nsystem_prompt = \"From the wizard.\"\n").unwrap();
        let s3 = ChatSession::new(&ChatConfig::default(), true).with_config_file("", &cfg_file);
        assert_eq!(s3.request_messages("x")[0].content, "From the wizard.");
        let s4 = ChatSession::new(&custom, true).with_config_file(&custom.system_prompt, &cfg_file);
        assert_eq!(s4.request_messages("x")[0].content, "Be brief.");
        assert_eq!(
            ChatSession::new(&ChatConfig::default(), true).request_messages("x")[0].content,
            persona::DEFAULT_SYSTEM_PROMPT
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn with_mind_injects_skill_context_and_gates_authoring() {
        let dir = std::env::temp_dir().join(format!("lillith-chat-mind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mind = lillith_core::config::MindConfig {
            enabled: true,
            knowledge_dir: dir.join("knowledge").to_string_lossy().into_owned(),
            skills_dir: dir.join("skills").to_string_lossy().into_owned(),
            max_context_tokens: 800,
            ..Default::default()
        };
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        std::fs::write(
            dir.join("skills/rebase.md"),
            "# Git rebase\nRun git rebase -i main, then fixup.\n",
        )
        .unwrap();

        // The reply proposes a skill via a fenced ```skill block.
        let reply = "Here you go~\n```skill brew.md\n# Brew\nUse a 1:16 ratio.\n```\nSave it?";
        let (url, mut reqs) = fake_server(vec![sse_reply(&[reply])]).await;
        let mut s = ChatSession::new(&cfg(&url), false).with_mind(&mind);

        // The relevant skill is injected into the system message for this turn.
        let msgs = s.request_messages("how do I rebase in git?");
        assert!(
            msgs[0].content.contains("Skill: Git rebase"),
            "{}",
            msgs[0].content
        );
        // An unrelated query injects nothing extra.
        let none = s.request_messages("what is the weather");
        assert!(!none[0].content.contains("Skill: Git rebase"));

        // Sending turns the proposed skill into a plan item; nothing is
        // written yet.
        let out = s.send("make me a brew skill", |_| {}).await;
        assert!(matches!(out, Outcome::Reply(_)));
        let _ = reqs.recv().await.unwrap();
        let brew = dir.join("skills/brew.md");
        assert!(!brew.exists(), "nothing is written before approval");
        let Some(PendingAction::Plan { plan }) = s.pending_action() else {
            panic!("the skill write is in the plan")
        };
        assert_eq!(plan.len(), 1);
        assert_eq!(plan.items[0].req.op(), "write skill");
        assert!(plan.listing().contains(&brew.display().to_string()));

        // `soul_contract()` is wired to the session's audit log; here we use an
        // explicit temp audit path so the assertions stay hermetic.
        let _ = s.soul_contract();
        let audit_path = dir.join("audit.log");

        // Decline: still not written, audit records it.
        let mut sc = lillith_core::soul_contract::SoulContract::new(Some(audit_path.clone()));
        let id = sc.request(PendingAction::Plan { plan: plan.clone() });
        sc.deny(id).unwrap();
        assert!(!brew.exists());

        // Approve a fresh request: now it is written and audited.
        let mut sc = lillith_core::soul_contract::SoulContract::new(Some(audit_path.clone()));
        let ap = lillith_core::access::plan::PlanApproval::all(&plan);
        let id = sc.request(PendingAction::Plan { plan });
        sc.approve_plan(id, ap, None).unwrap();
        assert_eq!(
            std::fs::read_to_string(&brew).unwrap(),
            "# Brew\nUse a 1:16 ratio."
        );
        let audit = std::fs::read_to_string(&audit_path).unwrap();
        assert!(
            audit.contains("DECLINED") && audit.contains("RESULT-OK"),
            "{audit}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn with_mind_runs_read_tools_and_gates_write_tools() {
        let dir = std::env::temp_dir().join(format!("lillith-chat-tools-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Point the data dir (and thus the audit log) at the temp dir so this
        // test is hermetic. No other mind test reads XDG_DATA_HOME.
        std::env::set_var("XDG_DATA_HOME", &dir);
        let proj = dir.join("Projects");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("notes.md"), "# Notes\nremember the milk TODO\n").unwrap();
        let mind = lillith_core::config::MindConfig {
            enabled: true,
            knowledge_dir: dir.join("knowledge").to_string_lossy().into_owned(),
            skills_dir: dir.join("skills").to_string_lossy().into_owned(),
            max_context_tokens: 800,
            allow_paths: vec![proj.to_string_lossy().into_owned()],
            allow_shell: true,
            ..Default::default()
        };

        // 1) A reply asking to READ an allowlisted file runs now, no popup.
        let read_reply = format!(
            "let me peek~\n```tool\nread {}\n```",
            proj.join("notes.md").display()
        );
        let write_reply = format!(
            "writing~\n```tool\nwrite {}\nhello\n```",
            proj.join("out.txt").display()
        );
        let (url, mut reqs) = fake_server(vec![
            sse_reply(&[&read_reply]),
            sse_reply(&[&write_reply]),
            sse_reply(&["outside~\n```tool\nread /etc/passwd\n```"]),
        ])
        .await;
        let mut s = ChatSession::new(&cfg(&url), false).with_mind(&mind);

        s.send("what's in my notes?", |_| {}).await;
        let _ = reqs.recv().await.unwrap();
        // Read ran now: output captured, nothing pending.
        let out = s.last_tool_output().expect("read produced output");
        assert!(out.contains("remember the milk"), "{out}");
        assert!(s.pending_tool().is_none());
        assert!(s.pending_action().is_none());

        // 2) A reply asking to WRITE becomes a plan (not written until approved).
        s.send("save a file", |_| {}).await;
        let _ = reqs.recv().await.unwrap();
        let out_file = proj.join("out.txt");
        let Some(PendingAction::Plan { plan }) = s.pending_action() else {
            panic!("a write plan is pending")
        };
        assert!(plan.listing().contains(&out_file.display().to_string()));
        assert!(!out_file.exists(), "nothing written before approval");
        // A plain Yes is not enough.
        let mut sc = s.soul_contract();
        let id = sc.request(s.take_pending().unwrap());
        assert!(sc.confirm(id).is_err());
        assert!(!out_file.exists());
        sc.approve_plan(
            id,
            lillith_core::access::plan::PlanApproval::all(&plan),
            None,
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(&out_file).unwrap(), "hello");
        assert!(s.pending_action().is_none(), "cleared after take");

        // 3) A read OUTSIDE the allowlist is refused (noted, not run).
        s.send("read etc passwd", |_| {}).await;
        let _ = reqs.recv().await.unwrap();
        let refused = s.last_tool_output().expect("a refusal note");
        assert!(
            refused.contains("refused") || refused.contains("outside"),
            "{refused}"
        );
        assert!(s.pending_tool().is_none());

        let audit = std::fs::read_to_string(dir.join("lillith/audit.log")).unwrap();
        assert!(audit.contains("AUTO\tread_file"), "{audit}");
        assert!(audit.contains("APPROVED\tplan:write"), "{audit}");
        std::env::remove_var("XDG_DATA_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn system_prompt_lists_tools_when_enabled_and_omits_them_when_disabled() {
        use crate::capabilities::HEADING;
        let dir = std::env::temp_dir().join(format!("lillith-chat-caps-{}", std::process::id()));
        let mk = |enabled: bool, access: lillith_core::config::AccessConfig| {
            lillith_core::config::MindConfig {
                enabled,
                knowledge_dir: dir.join("knowledge").to_string_lossy().into_owned(),
                skills_dir: dir.join("skills").to_string_lossy().into_owned(),
                access,
                ..Default::default()
            }
        };
        let on =
            ChatSession::new(&ChatConfig::default(), true).with_mind(&mk(true, Default::default()));
        let p = on.request_messages("hi")[0].content.clone();
        assert!(
            p.starts_with(persona::DEFAULT_SYSTEM_PROMPT),
            "persona text untouched"
        );
        assert!(p.contains(HEADING));
        for t in [
            "list <folder>",
            "locate <folder>",
            "move <from>",
            "sort <folder>",
            "undo",
            "delete <path>",
            "open <app> -> <file>",
            "installed <name>",
            "run <command>",
            "list_skills",
            "read_skill <name>",
            "import_skills",
            "```skill",
        ] {
            assert!(p.contains(t), "{t} missing from the system prompt");
        }
        assert!(p.contains(&dir.join("skills").display().to_string()));

        let off = lillith_core::config::AccessConfig {
            organize: false,
            programs: false,
            commands: false,
            skills: false,
            ..Default::default()
        };
        let p = ChatSession::new(&ChatConfig::default(), true)
            .with_mind(&mk(true, off))
            .request_messages("hi")[0]
            .content
            .clone();
        assert!(p.contains(HEADING));
        for t in [
            "sort <folder>",
            "open <app>",
            "run <command>",
            "list_skills",
        ] {
            assert!(!p.contains(t), "{t} should be gone when switched off");
        }
        let p = ChatSession::new(&ChatConfig::default(), true)
            .with_mind(&mk(false, Default::default()))
            .request_messages("hi")[0]
            .content
            .clone();
        assert!(!p.contains(HEADING), "mind off: no tools section");
        let p = ChatSession::new(&ChatConfig::default(), true).request_messages("hi")[0]
            .content
            .clone();
        assert!(!p.contains(HEADING));
    }

    #[tokio::test]
    async fn tool_results_flow_back_to_her() {
        let dir = std::env::temp_dir().join(format!("lillith-chat-loop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        std::fs::write(
            dir.join("skills/tidy.md"),
            "# Tidy downloads\nSort by type.\n",
        )
        .unwrap();
        let mind = lillith_core::config::MindConfig {
            enabled: true,
            knowledge_dir: dir.join("knowledge").to_string_lossy().into_owned(),
            skills_dir: dir.join("skills").to_string_lossy().into_owned(),
            ..Default::default()
        };
        let (url, mut reqs) = fake_server(vec![
            sse_reply(&["let me check my skills~\n```tool\nlist_skills\n```"]),
            sse_reply(&["I have a tidy-downloads skill, darling."]),
        ])
        .await;
        let mut s = ChatSession::new(&cfg(&url), false).with_mind(&mind);
        s.send("what skills do you have?", |_| {}).await;
        let _ = reqs.recv().await.unwrap();
        let fb = s.take_tool_feedback().expect("the listing is fed back");
        assert!(fb.starts_with("[tool result] list_skills"), "{fb}");
        assert!(fb.contains("tidy.md — Tidy downloads"), "{fb}");
        assert!(s.take_tool_feedback().is_none(), "taken once");
        let out = s.send(&fb, |_| {}).await;
        assert_eq!(out.text(), "I have a tidy-downloads skill, darling.");
        let r2 = reqs.recv().await.unwrap();
        assert!(
            r2.contains("[tool result] list_skills"),
            "the model saw the result"
        );
        // A refused tool comes back too, so she can explain.
        let action = PendingAction::RunCommand {
            command: "sudo pacman -Syu".into(),
            cwd: None,
            timeout_s: 5,
        };
        let no = ChatSession::answer_feedback(&action, Err("Ko said no"));
        assert!(no.contains("sudo pacman -Syu") && no.contains("Nothing was done"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn non_local_endpoint_is_refused_in_character() {
        for e in [
            "http://203.0.113.7:8080",
            "https://127.0.0.1:8080",
            "http://localhost.evil.com:8080",
        ] {
            let mut s = ChatSession::new(&cfg(e), false);
            let mut shown = String::new();
            match s.send("hi", |t| shown.push_str(t)).await {
                Outcome::Fallback { line, error } => {
                    assert!(error.contains("non-local"), "{e}: {error}");
                    assert!(persona::SERVER_ERROR_LINES.contains(&line.as_str()));
                    assert_eq!(shown, line);
                }
                o => panic!("{e}: expected refusal, got {o:?}"),
            }
            assert!(s.history().is_empty());
        }
    }

    #[tokio::test]
    async fn mixed_requests_become_one_plan_in_order_and_nothing_runs_first() {
        let dir = std::env::temp_dir().join(format!("lillith-chat-mixed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let proj = dir.join("Projects");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("a.txt"), "a").unwrap();
        let mind = lillith_core::config::MindConfig {
            enabled: true,
            knowledge_dir: dir.join("knowledge").to_string_lossy().into_owned(),
            skills_dir: dir.join("skills").to_string_lossy().into_owned(),
            allow_paths: vec![proj.to_string_lossy().into_owned()],
            ..Default::default()
        };
        let reply = format!(
            "on it~\n```tool\nmkdir {p}/Docs\n```\n```tool\nmove {p}/a.txt -> {p}/Docs\n```\n```tool\nlist {p}\n```\n```tool\nrun touch '{p}/made-by-command'\n```\n```tool\nopen sh -> {p}/a.txt\n```",
            p = proj.display()
        );
        let (url, mut reqs) = fake_server(vec![sse_reply(&[&reply])]).await;
        let mut s = ChatSession::new(&cfg(&url), false).with_mind(&mind);
        s.send("tidy and run it", |_| {}).await;
        let _ = reqs.recv().await.unwrap();
        // The listing (a read) ran; the rest is one plan, in her order.
        assert!(s.last_tool_output().unwrap().contains("a.txt"));
        let Some(PendingAction::Plan { plan }) = s.pending_action() else {
            panic!("one combined plan")
        };
        let ops: Vec<&str> = plan.items.iter().map(|i| i.req.op()).collect();
        assert_eq!(ops, ["mkdir", "move", "run", "launch"]);
        assert!(!proj.join("Docs").exists());
        assert!(proj.join("a.txt").exists());
        assert!(
            !proj.join("made-by-command").exists(),
            "no command before approval"
        );
        // Approve only the command: just that runs.
        let mut sc = lillith_core::soul_contract::SoulContract::new(None);
        let ap = lillith_core::access::plan::PlanApproval::subset(&plan, [3]);
        let id = sc.request(s.take_pending().unwrap());
        sc.approve_plan(id, ap, None).unwrap();
        assert!(proj.join("made-by-command").exists());
        assert!(!proj.join("Docs").exists() && proj.join("a.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn desktop_skills_import_is_a_reviewed_plan() {
        let dir = std::env::temp_dir().join(format!("lillith-chat-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let desk = dir.join("Desktop/skills");
        std::fs::create_dir_all(&desk).unwrap();
        std::fs::write(desk.join("new.md"), "# New\n").unwrap();
        std::fs::write(desk.join("same.md"), "# Mine differs\n").unwrap();
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        std::fs::write(dir.join("skills/same.md"), "# Hers\n").unwrap();
        let mind = lillith_core::config::MindConfig {
            enabled: true,
            knowledge_dir: dir.join("knowledge").to_string_lossy().into_owned(),
            skills_dir: dir.join("skills").to_string_lossy().into_owned(),
            ..Default::default()
        };
        let mut s = ChatSession::new(&ChatConfig::default(), true).with_mind(&mind);
        // Plan straight from the folder (find_desktop_skills reads $HOME).
        let tools = s.tools().unwrap().clone();
        let Ok(Planned::Change(steps)) = tools.plan(crate::fstools::ToolRequest::ImportSkills {
            from: Some(desk.display().to_string()),
        }) else {
            panic!("import is a plan")
        };
        let plan = tools.build_plan(steps).unwrap();
        assert_eq!(plan.len(), 2);
        let l = plan.listing();
        assert!(
            l.contains("same (from desktop).md"),
            "exact rename shown: {l}"
        );
        assert!(!dir.join("skills/new.md").exists(), "nothing copied yet");
        s.set_pending_tool(Some(PendingAction::Plan { plan: plan.clone() }));
        let mut sc = lillith_core::soul_contract::SoulContract::new(None);
        let ap = lillith_core::access::plan::PlanApproval::subset(
            &plan,
            plan.items
                .iter()
                .filter(|i| i.line.contains("new.md"))
                .map(|i| i.n),
        );
        let id = sc.request(s.take_pending().unwrap());
        sc.approve_plan(id, ap, None).unwrap();
        assert!(dir.join("skills/new.md").exists());
        assert!(!dir.join("skills/same (from desktop).md").exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("skills/same.md")).unwrap(),
            "# Hers\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
