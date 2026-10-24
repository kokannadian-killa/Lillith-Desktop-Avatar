//! The small chat bubble that opens when you click Lillith.
//!
//! An undecorated, always-on-top, transparent `eframe` window (300×380)
//! placed beside her head, in the DemonOS hellforge look (near-black glass,
//! crimson frame, ember accents, a vector hellforge emblem in the corner
//! where a heart emoji used to be). It shows the conversation, has a
//! one-line input (Enter sends), streams her reply as it arrives, and closes
//! on `Esc` or as soon as it loses focus (a click anywhere outside it).
//!
//! When she proposes a gated action (skill write, file write/edit, shell),
//! the shared Soul Contract popup ([`lillith_theme::egui_contract`]) takes
//! over the bubble: **Yes** runs it through
//! [`lillith_core::soul_contract::SoulContract::confirm`] (audited `CONFIRM`),
//! **No** or `Esc` discards it (audited `DENY`). While it is up the chat
//! input is disabled and the bubble stays open on focus loss, so the answer
//! is never skipped by accident. `/contract yes|no` still works as a typed
//! alias.
//!
//! Every change, launch and command she proposes arrives as ONE Soul
//! Contract **plan** ([`lillith_theme::egui_plan`]): every numbered item in
//! full (paths, exact renames, sizes, diffs, delete listings, program, args,
//! cwd, risky). Ko ticks what he allows and presses APPROVE SELECTED, or
//! APPROVE ALL, or DECLINE / `Esc`. Only approved items run, through
//! [`lillith_core::soul_contract::SoulContract::approve_plan`]; if the disk
//! changed meanwhile, the new plan comes up instead. Deletes among the
//! approved items need the typed phrase in the same popup.
//!
//! Deleting is different: the deletion contract
//! ([`lillith_theme::egui_deletion`]) lists EVERY file and folder with sizes
//! and totals, offers Trash (default) or Permanent, and only signs when Ko
//! types the exact phrase and clicks SIGN. A plain yes never deletes. If the
//! files changed meanwhile the new list comes up instead.
//!
//! Tool results and Ko's answers are fed back to her (up to
//! [`MAX_TOOL_CHAIN`] steps) so she can say what happened. On open, a
//! skills folder on the desktop is proposed for import as a plan
//! (`[mind.access] import_desktop_skills`); nothing is copied unless approved.
//!
//! The model call runs on a worker thread (its own tokio runtime) through
//! [`lillith_mind::chat::ChatSession`]. Every step is also reported through
//! [`BubbleOptions::emit`] (`chat.start`, `chat.token`, `chat.end`, …) so
//! the overlay can lip-flap the `talk_loop` clip in sync.
//!
//! Runs as its own process (`lillith chat-bubble`), because the overlay
//! already owns the process's windowing event loop.

use eframe::egui;
use lillith_core::access::deletion::{DeleteMode, DeletionManifest, DeletionSignature};
use lillith_core::access::plan::{ChangePlan, PlanApproval};
use lillith_core::config::{ChatConfig, MindConfig};
use lillith_core::soul_contract::{
    ContractError, ContractPrompt, PendingAction, RequestId, SoulContract,
};
use lillith_mind::capabilities::RESULT_OPEN;
use lillith_mind::chat::{ChatSession, Outcome, MAX_TOOL_CHAIN};
use lillith_mind::llm::Message;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Reports a chat event (`name`, optional detail) to whoever launched us.
pub type Emit = Arc<dyn Fn(&str, Option<&str>) + Send + Sync>;

/// Bubble window size (logical px).
pub const BUBBLE_SIZE: (f32, f32) = (300.0, 380.0);

/// Ignore focus loss this soon after opening (window managers may hand
/// focus over a moment late).
const FOCUS_GRACE: Duration = Duration::from_millis(600);

/// Everything the bubble needs.
#[derive(Clone)]
pub struct BubbleOptions {
    /// Top-left on screen (honoured on X11; Wayland compositors place
    /// normal windows themselves).
    pub pos: Option<(i32, i32)>,
    pub chat: ChatConfig,
    /// Knowledge + skills folders and the skill-authoring gate (`[mind]`).
    pub mind: MindConfig,
    /// The config file in use (`--config`, else the default path). Its
    /// `[llm] system_prompt` applies unless `[chat] system_prompt` is set
    /// (`None` = only the default config path is consulted).
    pub config_path: Option<PathBuf>,
    /// `--no-llm`: answer with an in-character "switched off" line.
    pub offline: bool,
    /// History file (`None` = in-memory only).
    pub history: Option<PathBuf>,
    pub emit: Emit,
    /// Save the first settled frame to this PNG and exit (previews).
    pub screenshot: Option<PathBuf>,
    /// Show a canned sample conversation instead of the history (previews).
    pub demo: bool,
    /// Show a sample Soul Contract popup (previews only: answering it just
    /// dismisses it; nothing is filed, run, or audited).
    pub demo_contract: bool,
}

enum Req {
    Send(String),
    Clear,
    /// Answer the pending Soul Contract (skill, file write, or shell): Yes
    /// (run it) or No (discard).
    Confirm(bool),
    /// Sign the pending deletion with the typed phrase.
    SignDeletion {
        typed: String,
        permanent: bool,
    },
    /// Run exactly these plan items (`typed`/`permanent` sign its deletes).
    ApprovePlan {
        items: Vec<usize>,
        typed: String,
        permanent: bool,
    },
    /// Decline the whole plan.
    DeclinePlan,
}

/// What the plan review shows.
#[derive(Debug, Clone, PartialEq)]
struct PlanPrompt {
    rows: Vec<PlanRow>,
    totals: String,
    note: Option<String>,
}

impl PlanPrompt {
    fn of(plan: &ChangePlan, note: Option<&str>) -> Self {
        Self {
            rows: plan
                .items
                .iter()
                .map(|i| PlanRow {
                    n: i.n,
                    group: i.group.clone(),
                    line: i.line.clone(),
                    fields: i.fields.clone(),
                    detail: i.detail.clone(),
                    delete_entries: i.manifest.as_ref().map_or(0, |m| m.entries.len()),
                    risky: i.risky.is_some(),
                    elevated: i.elevated,
                })
                .collect(),
            totals: plan.totals(None),
            note: note.map(str::to_string),
        }
    }
}

/// What the deletion popup shows.
#[derive(Debug, Clone, PartialEq)]
struct DeletionPrompt {
    listing: String,
    totals: String,
    phrase: String,
    note: Option<String>,
}

impl DeletionPrompt {
    fn of(m: &DeletionManifest, note: Option<&str>) -> Self {
        Self {
            listing: m.listing(),
            totals: m.totals(),
            phrase: m.phrase(),
            note: note.map(str::to_string),
        }
    }
}

enum Ui {
    /// A new reply starts streaming (after a tool result was fed back).
    Begin,
    /// A deletion is waiting for Ko's signature.
    Deletion(DeletionPrompt),
    Token(String),
    Done(Outcome),
    /// A system note to show in the bubble (e.g. a skill proposal/result).
    Note(String),
    /// A gated action is waiting: raise the Soul Contract popup.
    Contract(ContractPrompt),
    /// A plan is waiting for review.
    Plan(PlanPrompt),
}

/// Parses the bubble's Soul Contract answer: `/contract yes` (or `y`) →
/// `Some(true)`, `/contract no` (or `n`) → `Some(false)`, anything else →
/// `None`. `/skill yes|no` is kept as an alias (Phase 1 muscle memory).
fn parse_confirm_command(text: &str) -> Option<bool> {
    let mut parts = text.split_whitespace();
    let cmd = parts.next()?;
    if cmd != "/contract" && cmd != "/skill" {
        return None;
    }
    let answer = parts.next()?.to_ascii_lowercase();
    if parts.next().is_some() {
        return None;
    }
    match answer.as_str() {
        "yes" | "y" => Some(true),
        "no" | "n" => Some(false),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Line {
    me: bool,
    text: String,
}

struct Bubble {
    lines: Vec<Line>,
    /// Her opening line while there's no history yet (picked once per open).
    opener: &'static str,
    streaming: Option<String>,
    input: String,
    tx: Sender<Req>,
    rx: Receiver<Ui>,
    opened: Instant,
    had_focus: bool,
    closing: bool,
    asked_focus: bool,
    emit: Emit,
    screenshot: Option<PathBuf>,
    shot_requested: bool,
    /// The Soul Contract popup currently up (a pending gated action).
    contract: Option<ContractPrompt>,
    /// The popup is the `--demo-contract` sample (answers aren't sent).
    contract_demo: bool,
    /// The deletion contract currently up.
    deletion: Option<DeletionPrompt>,
    deletion_state: DeletionState,
    /// The plan review currently up.
    plan: Option<PlanPrompt>,
    plan_state: PlanState,
}

// lillith-theme's hellforge bubble look (DemonOS palette).
use lillith_theme::bubble::{self as look, BG, CRIMSON, EMBER, HER_TEXT, ME_TEXT, NEON};
use lillith_theme::egui_contract::{soul_contract_popup, ContractChoice, ContractView};
use lillith_theme::egui_deletion::{
    deletion_contract_popup, DeletionChoice, DeletionState, DeletionView,
};
use lillith_theme::egui_plan::{plan_review_popup, PlanChoice, PlanRow, PlanState, PlanView};
use lillith_theme::egui_theme as th;
use lillith_theme::palette as pal;

/// Size of the hellforge emblem in the header (logical px).
pub const EMBLEM_PX: f32 = 30.0;

/// Draws the header emblem (replaces the old 💜 glyph) and returns its rect.
pub fn emblem(ui: &mut egui::Ui, flicker: f32) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(EMBLEM_PX, EMBLEM_PX), egui::Sense::hover());
    th::hellforge_emblem(ui.painter(), rect.center(), EMBLEM_PX, flicker);
    resp
}

/// The sample Soul Contract for previews (`--demo-contract`).
pub fn demo_contract() -> ContractPrompt {
    PendingAction::WriteFile {
        path: PathBuf::from("~/lillith/projects/hellforge/src/main.rs"),
        body: "fn main() {\n    let souls = 666;\n    println!(\"the forge burns with {souls} souls\");\n}\n"
            .into(),
    }
    .prompt()
}

/// Her opening line in an empty chat, from Lily's banter pool.
pub fn opener_line(salt: usize) -> &'static str {
    let pool = lillith_core::banter::CHAT_OPENERS;
    pool[salt % pool.len()]
}

/// A canned exchange for previews (`--demo`).
pub fn demo_lines() -> Vec<(bool, &'static str)> {
    vec![
        (
            false,
            "There you are, Ko. I've been fingering myself waiting. Cock out, I'm starving~ 😈",
        ),
        (true, "missed you 🔥"),
        (
            false,
            "I know. Now feed me, toy. Your cum down my throat, every last drop 💜",
        ),
    ]
}

/// The bubble's chat session (no history yet): `[mind]` attached, and the
/// system prompt resolved against [`BubbleOptions::config_path`] so a custom
/// `--config` file's `[llm] system_prompt` reaches the model.
pub fn new_session(opts: &BubbleOptions) -> ChatSession {
    let mut session = ChatSession::new(&opts.chat, opts.offline);
    if let Some(p) = &opts.config_path {
        session = session.with_config_file(&opts.chat.system_prompt, p);
    }
    session.with_mind(&opts.mind)
}

fn spawn_worker(opts: &BubbleOptions, ctx: egui::Context) -> (Sender<Req>, Receiver<Ui>) {
    let (req_tx, req_rx) = channel::<Req>();
    let (ui_tx, ui_rx) = channel::<Ui>();
    let mut session = new_session(opts);
    if let Some(p) = &opts.history {
        session = session.with_history_file(p.clone());
    }
    let import = !opts.demo && opts.screenshot.is_none();
    let emit = opts.emit.clone();
    std::thread::Builder::new()
        .name("lillith-chat".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!("chat runtime: {e}");
                    return;
                }
            };
            let mut w = Worker {
                session,
                rt,
                ui_tx,
                emit,
                ctx,
                pending: None,
                deletion: None,
                plan: None,
            };
            if import {
                // One-time notes from the app (e.g. the KWin start-up check).
                if let Some(d) = lillith_core::config::data_dir() {
                    for note in lillith_core::notes::take(&d) {
                        w.note(format!("🛠 {note}"));
                    }
                }
                if let Some(note) = w.session.import_desktop_skills() {
                    w.note(format!("📥 {note}"));
                    w.raise_pending();
                }
            }
            while let Ok(req) = req_rx.recv() {
                match req {
                    Req::Clear => w.session.clear(),
                    Req::Send(text) => {
                        let fb = w.turn(&text, false);
                        w.chain(fb);
                    }
                    Req::Confirm(yes) => w.confirm(yes),
                    Req::SignDeletion { typed, permanent } => w.sign(&typed, permanent),
                    Req::ApprovePlan {
                        items,
                        typed,
                        permanent,
                    } => w.approve_plan(&items, &typed, permanent),
                    Req::DeclinePlan => w.decline_plan(),
                }
            }
        })
        .expect("spawn chat worker");
    (req_tx, ui_rx)
}

/// The chat worker's state (lives on its own thread).
struct Worker {
    session: ChatSession,
    rt: tokio::runtime::Runtime,
    ui_tx: Sender<Ui>,
    emit: Emit,
    ctx: egui::Context,
    /// A gated action (not a deletion) awaiting Yes/No.
    pending: Option<PendingAction>,
    /// A deletion awaiting a signature: its contract and request id.
    deletion: Option<(SoulContract, RequestId)>,
    /// A plan awaiting review: its contract and request id.
    plan: Option<(SoulContract, RequestId)>,
}

impl Worker {
    fn ui(&self, m: Ui) {
        let _ = self.ui_tx.send(m);
        self.ctx.request_repaint();
    }

    fn note(&self, text: String) {
        self.ui(Ui::Note(text));
    }

    /// One model turn. Returns the tool result to feed back, if a read-only
    /// tool ran (or was refused); gated actions are raised instead.
    fn turn(&mut self, text: &str, follow_up: bool) -> Option<String> {
        if follow_up {
            self.ui(Ui::Begin);
        }
        (self.emit)("chat.start", None);
        let mut streamed = false;
        let (emit, ui_tx, ctx) = (self.emit.clone(), self.ui_tx.clone(), self.ctx.clone());
        let out = self.rt.block_on(self.session.send(text, |t| {
            streamed = true;
            emit("chat.token", Some(t));
            let _ = ui_tx.send(Ui::Token(t.to_string()));
            ctx.request_repaint();
        }));
        match &out {
            Outcome::Reply(r) => (self.emit)("chat.end", Some(r)),
            Outcome::Fallback { line, error } => {
                // She still says her in-character line on the overlay.
                if !streamed {
                    (self.emit)("chat.token", Some(line));
                }
                (self.emit)("chat.error", Some(error));
            }
        }
        self.ui(Ui::Done(out));
        if let Some(out) = self.session.last_tool_output() {
            // Free tools ran (or a tool was refused).
            self.note(format!("🔎 {}", short(out)));
        }
        // Did she propose changes? Hold them for review; never run them on
        // our own.
        if self.raise_pending() {
            return None;
        }
        self.session.take_tool_feedback()
    }

    /// Raises her pending proposal (plan, deletion or other gated action)
    /// for Ko. Returns whether one was raised.
    fn raise_pending(&mut self) -> bool {
        if let Some(action) = self.session.take_pending() {
            let summary = action.summary();
            (self.emit)("chat.contract_proposed", Some(&summary));
            if let PendingAction::Plan { plan } = &action {
                let mut sc = self.session.soul_contract();
                let prompt = PlanPrompt::of(plan, None);
                let id = sc.request(action.clone());
                self.plan = Some((sc, id));
                self.note(format!(
                    "📜 Soul Contract plan: {summary}. Nothing has happened yet. Check every step, then approve all, pick the ones you allow, or decline."
                ));
                self.ui(Ui::Plan(prompt));
            } else if let PendingAction::DeletePaths { manifest } = &action {
                let mut sc = self.session.soul_contract();
                let prompt = DeletionPrompt::of(manifest, None);
                let id = sc.request(action.clone());
                self.deletion = Some((sc, id));
                self.note(format!(
                    "📜 Soul Contract: she wants to {summary}. Every item is listed in the popup; type the phrase and sign, or refuse."
                ));
                self.ui(Ui::Deletion(prompt));
            } else {
                self.note(format!(
                    "📜 Soul Contract: she wants to {summary}. Sign or refuse it (or type /contract yes|no)."
                ));
                self.ui(Ui::Contract(action.prompt()));
                self.pending = Some(action);
            }
            return true;
        }
        false
    }

    fn plan_prompt(&self, note: Option<&str>) -> Option<PlanPrompt> {
        let (sc, id) = self.plan.as_ref()?;
        sc.plan(*id).map(|p| PlanPrompt::of(p, note))
    }

    /// Ko approved `items` in the review: run exactly those (deletes among
    /// them signed with `typed`).
    fn approve_plan(&mut self, items: &[usize], typed: &str, permanent: bool) {
        let Some((mut sc, id)) = self.plan.take() else {
            self.note("No plan is waiting for review.".into());
            return;
        };
        let Some(plan) = sc.plan(id).cloned() else {
            return;
        };
        let approval = PlanApproval::subset(&plan, items.iter().copied());
        let mut signature = None;
        if let Some(m) = plan.deletion_manifest(approval.selected()) {
            let mode = if permanent {
                DeleteMode::Permanent
            } else {
                DeleteMode::Trash
            };
            match DeletionSignature::sign(&m, typed, mode) {
                Ok(sig) => signature = Some(sig),
                Err(e) => {
                    self.plan = Some((sc, id));
                    self.note(format!("Not signed: {e}. Nothing ran."));
                    if let Some(p) = self.plan_prompt(Some(&e.to_string())) {
                        self.ui(Ui::Plan(p));
                    }
                    return;
                }
            }
        }
        let action = PendingAction::Plan { plan };
        match sc.approve_plan(id, approval, signature) {
            Ok(done) => {
                (self.emit)("chat.contract_confirmed", Some("plan"));
                let out = done.output.unwrap_or_default();
                self.note(format!("✅ Plan:\n{}", short(&out)));
                let fb = ChatSession::answer_feedback(&action, Ok(&out));
                self.chain(Some(fb));
            }
            Err(ContractError::Changed { new_id }) => {
                self.plan = Some((sc, new_id));
                let note = "Things changed on disk since you saw the plan — nothing ran. Here is the new plan.";
                self.note(format!("⚠️ {note}"));
                if let Some(p) = self.plan_prompt(Some(note)) {
                    self.ui(Ui::Plan(p));
                }
            }
            Err(e @ (ContractError::NeedsSignature(_) | ContractError::WrongSet)) => {
                self.plan = Some((sc, id));
                let note = format!("{e} — nothing ran.");
                self.note(note.clone());
                if let Some(p) = self.plan_prompt(Some(&note)) {
                    self.ui(Ui::Plan(p));
                }
            }
            Err(e) => {
                let _ = sc.deny(id);
                (self.emit)("chat.contract_error", Some(&e.to_string()));
                self.note(format!("Couldn't run the plan: {e}"));
                let fb = ChatSession::answer_feedback(&action, Err(&e.to_string()));
                self.chain(Some(fb));
            }
        }
    }

    /// Ko declined the whole plan: nothing runs (audited).
    fn decline_plan(&mut self) {
        let Some((mut sc, id)) = self.plan.take() else {
            return;
        };
        let action = sc.deny(id).ok();
        (self.emit)("chat.contract_denied", None);
        self.note("🚫 Declined; nothing was changed or run.".into());
        if let Some(a) = action {
            let fb = ChatSession::answer_feedback(&a, Err("Ko declined the whole plan"));
            self.chain(Some(fb));
        }
    }

    /// Feeds tool results / answers back to her, a few steps at most.
    fn chain(&mut self, mut fb: Option<String>) {
        for _ in 0..MAX_TOOL_CHAIN {
            let Some(f) = fb.take() else { return };
            if self.pending.is_some() || self.deletion.is_some() || self.plan.is_some() {
                return;
            }
            fb = self.turn(&f, true);
        }
    }

    fn confirm(&mut self, yes: bool) {
        if self.plan.is_some() {
            if yes {
                self.note("✍️ Tick the steps you allow in the plan and press APPROVE (a plain yes isn't an approval).".into());
                if let Some(p) = self.plan_prompt(None) {
                    self.ui(Ui::Plan(p));
                }
            } else {
                self.decline_plan();
            }
            return;
        }
        if self.deletion.is_some() {
            if yes {
                self.note("✍️ Deleting needs your signature: type the phrase in the contract and press SIGN.".into());
                if let Some(p) = self.deletion_prompt(None) {
                    self.ui(Ui::Deletion(p));
                }
            } else {
                let (mut sc, id) = self.deletion.take().expect("checked");
                let action = sc.deny(id).ok();
                (self.emit)("chat.contract_denied", None);
                self.note("🚫 Refused; nothing was deleted.".into());
                if let Some(a) = action {
                    let fb = ChatSession::answer_feedback(&a, Err("Ko refused"));
                    self.chain(Some(fb));
                }
            }
            return;
        }
        let Some(action) = self.pending.take() else {
            self.note("Nothing is waiting for a yes/no right now.".into());
            return;
        };
        let mut sc = self.session.soul_contract();
        let id = sc.request(action.clone());
        let result: Result<String, String> = if yes {
            match sc.confirm(id) {
                Ok(done) => {
                    (self.emit)("chat.contract_confirmed", Some(done.action.kind()));
                    let mut out = String::new();
                    if let Some(p) = &done.path {
                        out.push_str(&format!("{}\n", p.display()));
                    }
                    if let Some(o) = &done.output {
                        out.push_str(o);
                    }
                    self.note(if out.is_empty() {
                        "✅ Done.".into()
                    } else {
                        format!("✅ Done:\n{}", short(&out))
                    });
                    Ok(out)
                }
                Err(e) => {
                    (self.emit)("chat.contract_error", Some(&e.to_string()));
                    self.note(format!("Couldn't do that: {e}"));
                    Err(format!("it failed: {e}"))
                }
            }
        } else {
            let _ = sc.deny(id);
            (self.emit)("chat.contract_denied", None);
            self.note("🚫 Discarded; nothing was changed.".into());
            Err("Ko said no".into())
        };
        let fb = ChatSession::answer_feedback(&action, result.as_deref().map_err(|e| e.as_str()));
        self.chain(Some(fb));
    }

    fn deletion_prompt(&self, note: Option<&str>) -> Option<DeletionPrompt> {
        let (sc, id) = self.deletion.as_ref()?;
        sc.pending()
            .iter()
            .find(|(i, _)| i == id)
            .and_then(|(_, a)| match a {
                PendingAction::DeletePaths { manifest } => Some(DeletionPrompt::of(manifest, note)),
                _ => None,
            })
    }

    /// Ko signed in the popup: check the phrase against the exact list and
    /// delete through the contract (which re-lists first).
    fn sign(&mut self, typed: &str, permanent: bool) {
        let Some((mut sc, id)) = self.deletion.take() else {
            self.note("Nothing is waiting for a signature.".into());
            return;
        };
        let Some(PendingAction::DeletePaths { manifest }) = sc
            .pending()
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, a)| a.clone())
        else {
            return;
        };
        let mode = if permanent {
            DeleteMode::Permanent
        } else {
            DeleteMode::Trash
        };
        let sig = match DeletionSignature::sign(&manifest, typed, mode) {
            Ok(s) => s,
            Err(e) => {
                self.deletion = Some((sc, id));
                self.note(format!("Not signed: {e}."));
                if let Some(p) = self.deletion_prompt(Some(&e.to_string())) {
                    self.ui(Ui::Deletion(p));
                }
                return;
            }
        };
        let action = PendingAction::DeletePaths {
            manifest: manifest.clone(),
        };
        match sc.sign_deletion(id, sig) {
            Ok(done) => {
                (self.emit)("chat.contract_confirmed", Some("delete_paths"));
                let out = done.output.unwrap_or_default();
                self.note(format!("🗑 Done:\n{}", short(&out)));
                let fb = ChatSession::answer_feedback(&action, Ok(&out));
                self.chain(Some(fb));
            }
            Err(ContractError::Changed { new_id }) => {
                self.deletion = Some((sc, new_id));
                let note = "The files changed since you saw the list — nothing was deleted. Here is the new list.";
                self.note(format!("⚠️ {note}"));
                if let Some(p) = self.deletion_prompt(Some(note)) {
                    self.ui(Ui::Deletion(p));
                }
            }
            Err(e) => {
                (self.emit)("chat.contract_error", Some(&e.to_string()));
                self.note(format!("Couldn't delete: {e}"));
                let fb = ChatSession::answer_feedback(&action, Err(&e.to_string()));
                self.chain(Some(fb));
            }
        }
    }
}

/// Long tool output shortened for the bubble (the model gets it all).
fn short(text: &str) -> String {
    const MAX: usize = 1200;
    if text.len() <= MAX {
        return text.to_string();
    }
    let mut end = MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

fn history_lines(opts: &BubbleOptions) -> Vec<Line> {
    let h: Vec<Message> = opts
        .history
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json_lines(&s))
        .unwrap_or_default();
    h.into_iter()
        .filter(|m| m.role == "user" || m.role == "assistant")
        // Tool results fed back to her are not Ko's words.
        .filter(|m| !(m.role == "user" && m.content.starts_with(RESULT_OPEN)))
        .map(|m| Line {
            me: m.role == "user",
            text: m.content,
        })
        .collect()
}

fn serde_json_lines(s: &str) -> Option<Vec<Message>> {
    serde_json::from_str(s).ok()
}

/// What the bubble says when the safeword stops work mid-reply.
const SAFEWORD_STOPPED: &str =
    "Stopped. Every running command is dead and the admin helper is gone.";

/// If `text` is the safeword, stops every running command and the admin
/// helper immediately (no queue) and returns true.
fn stop_now_if_safeword(text: &str) -> bool {
    if !lillith_core::access::kill_switch::is_safeword(text) {
        return false;
    }
    lillith_core::access::kill_switch::stop_all();
    lillith_core::access::admin_broker::shutdown();
    true
}

impl Bubble {
    fn send(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        // The safeword works right now, on the UI thread: the chat worker
        // may be blocked running the very command it has to stop, so a
        // queued "red" would only land after the command ended by itself.
        if stop_now_if_safeword(&text) && self.streaming.is_some() {
            self.lines.push(Line {
                me: true,
                text: text.clone(),
            });
            self.lines.push(Line {
                me: false,
                text: SAFEWORD_STOPPED.to_string(),
            });
            self.input.clear();
            return;
        }
        if self.streaming.is_some() {
            return;
        }
        if text == "/clear" {
            self.lines.clear();
            let _ = self.tx.send(Req::Clear);
        } else if let Some(yes) = parse_confirm_command(&text) {
            // Soul Contract answer: nothing runs unless this is a yes.
            self.answer_contract(yes);
        } else {
            self.lines.push(Line {
                me: true,
                text: text.clone(),
            });
            self.streaming = Some(String::new());
            let _ = self.tx.send(Req::Send(text));
        }
        self.input.clear();
    }

    fn drain(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Ui::Token(t) => {
                    if let Some(s) = &mut self.streaming {
                        s.push_str(&t);
                    }
                }
                Ui::Done(out) => {
                    self.streaming = None;
                    self.lines.push(Line {
                        me: false,
                        text: out.text().to_string(),
                    });
                }
                Ui::Note(text) => self.lines.push(Line { me: false, text }),
                Ui::Begin => self.streaming = Some(String::new()),
                Ui::Deletion(p) => {
                    self.deletion = Some(p);
                    self.deletion_state = DeletionState::default();
                }
                Ui::Contract(prompt) => {
                    self.contract = Some(prompt);
                    self.contract_demo = false;
                }
                Ui::Plan(p) => {
                    self.plan = Some(p);
                    self.plan_state = PlanState::default();
                }
            }
        }
    }

    /// Answers the Soul Contract (popup or typed alias) and takes the popup
    /// down. The worker does the gating: Yes → `confirm` (`CONFIRM`), No →
    /// `deny` (`DENY`); with nothing pending it only says so.
    fn answer_contract(&mut self, yes: bool) {
        let demo = std::mem::take(&mut self.contract_demo);
        self.contract = None;
        if !yes {
            self.deletion = None;
            self.plan = None;
        }
        if demo {
            self.lines.push(Line {
                me: false,
                text: "(preview contract: nothing was filed or run)".into(),
            });
        } else {
            let _ = self.tx.send(Req::Confirm(yes));
        }
    }

    /// Draws the Soul Contract popup if one is pending; routes its answer.
    fn contract_popup(&mut self, ctx: &egui::Context) {
        let Some(p) = &self.contract else {
            return;
        };
        let view = ContractView {
            title: p.title,
            summary: &p.summary,
            target: p.target.as_deref(),
            detail: p.detail.as_deref(),
            busy: false,
        };
        if let Some(choice) =
            soul_contract_popup(ctx, egui::Id::new("lillith-soul-contract"), &view)
        {
            self.answer_contract(choice == ContractChoice::Yes);
        }
    }

    /// Draws the deletion contract if one is pending; routes its answer.
    fn deletion_popup(&mut self, ctx: &egui::Context) {
        let Some(p) = &self.deletion else {
            return;
        };
        let view = DeletionView {
            listing: &p.listing,
            totals: &p.totals,
            phrase: &p.phrase,
            note: p.note.as_deref(),
        };
        match deletion_contract_popup(
            ctx,
            egui::Id::new("lillith-deletion-contract"),
            &view,
            &mut self.deletion_state,
        ) {
            Some(DeletionChoice::Sign { typed, permanent }) => {
                self.deletion = None;
                let _ = self.tx.send(Req::SignDeletion { typed, permanent });
            }
            Some(DeletionChoice::Cancel) => {
                self.deletion = None;
                let _ = self.tx.send(Req::Confirm(false));
            }
            None => {}
        }
    }

    /// Draws the plan review if one is pending; routes its answer.
    fn plan_popup(&mut self, ctx: &egui::Context) {
        let Some(p) = &self.plan else {
            return;
        };
        let view = PlanView {
            rows: &p.rows,
            totals: &p.totals,
            note: p.note.as_deref(),
        };
        match plan_review_popup(
            ctx,
            egui::Id::new("lillith-plan-review"),
            &view,
            &mut self.plan_state,
        ) {
            Some(PlanChoice::Approve {
                items,
                typed,
                permanent,
            }) => {
                self.plan = None;
                let _ = self.tx.send(Req::ApprovePlan {
                    items,
                    typed,
                    permanent,
                });
            }
            Some(PlanChoice::Decline) => {
                self.plan = None;
                let _ = self.tx.send(Req::DeclinePlan);
            }
            None => {}
        }
    }

    fn close(&mut self, ctx: &egui::Context) {
        if !self.closing {
            self.closing = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

fn bubble_line(ui: &mut egui::Ui, me: bool, text: &str) {
    let max_w = ui.available_width() * 0.82;
    let layout = if me {
        egui::Layout::right_to_left(egui::Align::TOP)
    } else {
        egui::Layout::left_to_right(egui::Align::TOP)
    };
    ui.with_layout(layout, |ui| {
        let color = if me { ME_TEXT } else { HER_TEXT };
        look::line_frame(me).show(ui, |ui| {
            ui.set_max_width(max_w);
            let text = lillith_anim::text::sanitize(text);
            ui.add(egui::Label::new(egui::RichText::new(text).color(color)).wrap());
        });
    });
    ui.add_space(4.0);
}

impl eframe::App for Bubble {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.asked_focus {
            // Always-on-top helper windows don't always get focus by
            // themselves; ask once so typing works straight away.
            self.asked_focus = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.drain();
        // Esc, or a click outside (focus lost) closes the bubble, unless a
        // Soul Contract is up: then Esc means No (the popup handles it) and
        // the bubble waits for an answer.
        let (esc, focused) =
            ctx.input(|i| (i.key_pressed(egui::Key::Escape), i.viewport().focused));
        if focused == Some(true) {
            self.had_focus = true;
        }
        let lost = self.had_focus && focused == Some(false) && self.opened.elapsed() > FOCUS_GRACE;
        if self.screenshot.is_none()
            && self.contract.is_none()
            && self.deletion.is_none()
            && self.plan.is_none()
            && (esc || lost)
        {
            self.close(ctx);
        }

        let frame = egui::Frame::new()
            .fill(BG)
            .stroke(egui::Stroke::new(2.0, CRIMSON))
            .corner_radius(egui::CornerRadius::same(look::CORNER + 1))
            .inner_margin(egui::Margin::same(10));
        let t = self.opened.elapsed().as_secs_f32();
        let flicker = 0.5 + 0.5 * (t * 3.1).sin() * (t * 1.7).cos();
        if self.streaming.is_some() || self.screenshot.is_none() {
            ctx.request_repaint_after(Duration::from_millis(80));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(ctx, |ui| {
                let outer = frame.show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    // Header: drag the bubble by it.
                    let header = ui.horizontal(|ui| {
                        emblem(ui, flicker);
                        ui.label(
                            egui::RichText::new("LILLITH")
                                .color(NEON)
                                .strong()
                                .size(16.0),
                        );
                        let status = if self.streaming.is_some() {
                            "forging…"
                        } else {
                            "awake"
                        };
                        ui.label(egui::RichText::new(status).color(EMBER).small());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("×").size(16.0).color(CRIMSON),
                                    )
                                    .frame(false),
                                )
                                .on_hover_text("Close (Esc)")
                                .clicked()
                            {
                                self.close(ctx);
                            }
                        });
                    });
                    let drag = ui.interact(
                        header.response.rect,
                        ui.id().with("drag"),
                        egui::Sense::drag(),
                    );
                    if drag.drag_started() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }
                    // Blade-thin crimson rule with a subtle drip under the header.
                    let y = header.response.rect.bottom() + 3.0;
                    let (x0, x1) = (ui.min_rect().left(), ui.max_rect().right());
                    ui.painter().line_segment(
                        [egui::pos2(x0, y), egui::pos2(x1, y)],
                        egui::Stroke::new(1.0, CRIMSON),
                    );
                    th::blood_drips(ui.painter(), x0 + 40.0, x1 - 60.0, y, 7, 9.0, 46.0);
                    ui.add_space(10.0);
                    let input_h = 34.0;
                    egui::ScrollArea::vertical()
                        .max_height(ui.available_height() - input_h)
                        .stick_to_bottom(true)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if self.lines.is_empty() && self.streaming.is_none() {
                                bubble_line(ui, false, self.opener);
                            }
                            for l in &self.lines {
                                bubble_line(ui, l.me, &l.text);
                            }
                            if let Some(s) = &self.streaming {
                                let t = if s.is_empty() { "…" } else { s.as_str() };
                                bubble_line(ui, false, t);
                            }
                        });
                    ui.add_space(4.0);
                    let edit = egui::TextEdit::singleline(&mut self.input)
                        .hint_text("Speak, mortal…")
                        .desired_width(f32::INFINITY)
                        .margin(egui::Margin::symmetric(8, 6));
                    let free = self.streaming.is_none()
                        && self.contract.is_none()
                        && self.deletion.is_none()
                        && self.plan.is_none();
                    let resp = ui.add_enabled(free, edit);
                    if free && resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        self.send();
                        resp.request_focus();
                    } else if !resp.has_focus() && free && !self.closing {
                        resp.request_focus();
                    }
                });
                // Faint neon edge around the whole box.
                th::neon_rect(
                    ui.painter(),
                    outer.response.rect.shrink(1.0),
                    pal::BLOOD_NEON.alpha(120),
                    1.0,
                    3.0,
                );
            });
        self.contract_popup(ctx);
        self.deletion_popup(ctx);
        self.plan_popup(ctx);
        self.screenshot(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.screenshot.is_some() {
            return;
        }
        (self.emit)("chat.closed", None);
    }
}

impl Bubble {
    /// `--screenshot`: after the first frames settle, save one and exit.
    fn screenshot(&mut self, ctx: &egui::Context) {
        let Some(path) = self.screenshot.clone() else {
            return;
        };
        if !self.shot_requested && self.opened.elapsed() > Duration::from_millis(1200) {
            self.shot_requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        ctx.request_repaint_after(Duration::from_millis(50));
        let mut img = None;
        ctx.input(|i| {
            for e in &i.raw.events {
                if let egui::Event::Screenshot { image, .. } = e {
                    img = Some(image.clone());
                }
            }
        });
        if let Some(img) = img {
            let [w, h] = img.size;
            let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            match image::RgbaImage::from_raw(w as u32, h as u32, bytes).map(|im| im.save(&path)) {
                Some(Ok(())) => {
                    tracing::info!(path = %path.display(), "chat bubble screenshot saved")
                }
                Some(Err(e)) => tracing::warn!("screenshot {}: {e}", path.display()),
                None => tracing::warn!("screenshot: bad image size"),
            }
            self.screenshot = None;
            self.close(ctx);
        }
    }
}

/// egui's default fonts plus current Noto Emoji (bundled, SIL OFL; covers
/// emoji newer than egui's copy, e.g. 🥰 🥺 🫶) ahead of the old emoji
/// fonts, and DejaVu Sans from the system (if installed) as a last resort:
/// no more boxes for emoji.
pub fn bubble_fonts() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "lillith-noto-emoji".into(),
        std::sync::Arc::new(egui::FontData::from_static(lillith_anim::text::NOTO_EMOJI)),
    );
    let system = lillith_anim::text::SYSTEM_FALLBACKS
        .iter()
        .find_map(|p| std::fs::read(p).ok());
    if let Some(bytes) = system {
        fonts.font_data.insert(
            "lillith-system-fallback".into(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
    }
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let list = fonts.families.entry(family).or_default();
        let at = 1.min(list.len());
        list.insert(at, "lillith-noto-emoji".into());
        if fonts.font_data.contains_key("lillith-system-fallback") {
            list.push("lillith-system-fallback".into());
        }
    }
    fonts
}

/// Opens the bubble (blocks until it closes).
pub fn run_chat_bubble(opts: BubbleOptions) -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Lillith")
        .with_app_id("lillith-chat")
        .with_inner_size([BUBBLE_SIZE.0, BUBBLE_SIZE.1])
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_taskbar(false)
        .with_always_on_top();
    if let Some((x, y)) = opts.pos {
        viewport = viewport.with_position([x as f32, y as f32]);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "lillith-chat",
        options,
        Box::new(move |cc| {
            look::apply(&cc.egui_ctx);
            cc.egui_ctx.set_fonts(bubble_fonts());
            let (tx, rx) = spawn_worker(&opts, cc.egui_ctx.clone());
            if opts.screenshot.is_none() {
                (opts.emit)("chat.opened", None);
            }
            let lines = if opts.demo {
                demo_lines()
                    .into_iter()
                    .map(|(me, t)| Line {
                        me,
                        text: t.to_string(),
                    })
                    .collect()
            } else {
                history_lines(&opts)
            };
            // Screenshots stay reproducible with the first opener.
            let salt = if opts.screenshot.is_some() {
                0
            } else {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos() as usize)
                    .unwrap_or(0)
            };
            Ok(Box::new(Bubble {
                lines,
                opener: opener_line(salt),
                streaming: None,
                input: String::new(),
                tx,
                rx,
                opened: Instant::now(),
                had_focus: false,
                closing: false,
                asked_focus: false,
                emit: opts.emit.clone(),
                screenshot: opts.screenshot.clone(),
                shot_requested: false,
                contract: opts.demo_contract.then(demo_contract),
                contract_demo: opts.demo_contract,
                deletion: None,
                deletion_state: DeletionState::default(),
                plan: None,
                plan_state: PlanState::default(),
            }))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safeword_stops_on_the_ui_thread_without_the_worker() {
        let before = lillith_core::access::kill_switch::epoch();
        assert!(!stop_now_if_safeword("I read a red book"));
        assert!(stop_now_if_safeword("Red."));
        assert!(lillith_core::access::kill_switch::stopped_since(before));
    }

    #[test]
    fn confirm_command_parses_only_explicit_answers() {
        // Generic /contract form.
        assert_eq!(parse_confirm_command("/contract yes"), Some(true));
        assert_eq!(parse_confirm_command("/contract Y"), Some(true));
        assert_eq!(parse_confirm_command("  /contract   no "), Some(false));
        assert_eq!(parse_confirm_command("/contract n"), Some(false));
        // /skill kept as an alias.
        assert_eq!(parse_confirm_command("/skill yes"), Some(true));
        assert_eq!(parse_confirm_command("/skill Y"), Some(true));
        assert_eq!(parse_confirm_command("  /skill   no "), Some(false));
        assert_eq!(parse_confirm_command("/skill n"), Some(false));
        // Anything ambiguous is not an answer (and goes to chat instead).
        assert_eq!(parse_confirm_command("/contract"), None);
        assert_eq!(parse_confirm_command("/contract maybe"), None);
        assert_eq!(parse_confirm_command("/skill yes please"), None);
        assert_eq!(parse_confirm_command("contract yes"), None);
        assert_eq!(parse_confirm_command("yes"), None);
    }

    fn test_bubble(contract: Option<ContractPrompt>, demo: bool) -> (Bubble, Receiver<Req>) {
        let (tx, req_rx) = channel();
        let (_ui_tx, rx) = channel();
        let b = Bubble {
            lines: Vec::new(),
            opener: opener_line(0),
            streaming: None,
            input: String::new(),
            tx,
            rx,
            opened: Instant::now(),
            had_focus: false,
            closing: false,
            asked_focus: true,
            emit: Arc::new(|_, _| {}),
            screenshot: None,
            shot_requested: false,
            contract,
            contract_demo: demo,
            deletion: None,
            deletion_state: DeletionState::default(),
            plan: None,
            plan_state: PlanState::default(),
        };
        (b, req_rx)
    }

    fn popup_frame(ctx: &egui::Context, b: &mut Bubble, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(BUBBLE_SIZE.0, BUBBLE_SIZE.1),
            )),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| b.contract_popup(ctx));
    }

    fn esc() -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn escape_on_contract_popup_sends_no() {
        let (mut b, req_rx) = test_bubble(Some(demo_contract()), false);
        let ctx = egui::Context::default();
        ctx.set_fonts(bubble_fonts());
        for _ in 0..3 {
            popup_frame(&ctx, &mut b, vec![]);
        }
        assert!(req_rx.try_recv().is_err(), "nothing answered yet");
        popup_frame(&ctx, &mut b, vec![esc()]);
        assert!(b.contract.is_none());
        assert!(matches!(req_rx.try_recv(), Ok(Req::Confirm(false))));
        assert!(
            !b.closing,
            "Esc answers the contract; it doesn't close the bubble"
        );
    }

    #[test]
    fn typed_alias_answers_and_drops_the_popup() {
        let (mut b, req_rx) = test_bubble(Some(demo_contract()), false);
        b.input = "/contract yes".into();
        b.send();
        assert!(b.contract.is_none());
        assert!(matches!(req_rx.try_recv(), Ok(Req::Confirm(true))));
    }

    #[test]
    fn demo_contract_never_reaches_the_worker() {
        let (mut b, req_rx) = test_bubble(Some(demo_contract()), true);
        b.answer_contract(true);
        assert!(b.contract.is_none());
        assert!(req_rx.try_recv().is_err(), "preview answers aren't sent");
        let p = demo_contract();
        assert_eq!(p.kind, "write_file");
        assert!(p.detail.is_some());
    }

    #[test]
    fn emoji_render_with_bundled_font() {
        let fonts = bubble_fonts();
        assert_eq!(
            fonts.families[&egui::FontFamily::Proportional][1],
            "lillith-noto-emoji"
        );
        let ctx = egui::Context::default();
        ctx.set_fonts(fonts);
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let id = egui::FontId::proportional(14.0);
        ctx.fonts(|f| {
            for c in "😘🥰🥺🫶🫠🤭💕✨❤".chars() {
                assert!(f.has_glyph(&id, c), "no glyph for {c}");
            }
        });
    }

    fn deletion_prompt() -> DeletionPrompt {
        DeletionPrompt {
            listing: "📄 /home/ko/junk.log 3 B".into(),
            totals: "1 file, 3 B".into(),
            phrase: "DELETE 1 ITEM".into(),
            note: None,
        }
    }

    #[test]
    fn deletion_popup_signs_only_with_the_phrase() {
        let (mut b, req_rx) = test_bubble(None, false);
        b.deletion = Some(deletion_prompt());
        let ctx = egui::Context::default();
        ctx.set_fonts(bubble_fonts());
        let run = |b: &mut Bubble, ev: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(BUBBLE_SIZE.0, BUBBLE_SIZE.1),
                )),
                events: ev,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| b.deletion_popup(ctx));
        };
        for _ in 0..3 {
            run(&mut b, vec![]);
        }
        assert!(req_rx.try_recv().is_err());
        // A typed "/contract yes" is not a signature: the popup stays up.
        b.input = "/contract yes".into();
        b.send();
        assert!(matches!(req_rx.try_recv(), Ok(Req::Confirm(true))));
        // Esc refuses.
        run(&mut b, vec![esc()]);
        assert!(b.deletion.is_none());
        assert!(matches!(req_rx.try_recv(), Ok(Req::Confirm(false))));
    }

    #[test]
    fn tool_results_are_hidden_from_the_history_view() {
        let dir = std::env::temp_dir().join(format!("lillith-bubble-hist-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("h.json");
        let msgs = vec![
            Message::user("tidy my downloads"),
            Message::assistant("```tool\nsort ~/Downloads by type\n```"),
            Message::user("[tool result] sort\nmoved 3\n[/tool result]"),
            Message::assistant("Done, darling."),
        ];
        std::fs::write(&path, serde_json::to_string(&msgs).unwrap()).unwrap();
        let opts = BubbleOptions {
            pos: None,
            chat: ChatConfig::default(),
            mind: MindConfig::default(),
            config_path: None,
            offline: true,
            history: Some(path),
            emit: Arc::new(|_, _| {}),
            screenshot: None,
            demo: false,
            demo_contract: false,
        };
        let lines = history_lines(&opts);
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|l| !l.text.starts_with("[tool result]")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn custom_config_path_supplies_llm_system_prompt() {
        let dir =
            std::env::temp_dir().join(format!("lillith-bubble-cfgprompt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("custom.toml");
        std::fs::write(&cfg, "[llm]\nsystem_prompt = \"From the custom config.\"\n").unwrap();
        let mut opts = BubbleOptions {
            pos: None,
            chat: ChatConfig::default(),
            mind: MindConfig::default(),
            config_path: Some(cfg.clone()),
            offline: true,
            history: None,
            emit: Arc::new(|_, _| {}),
            screenshot: None,
            demo: false,
            demo_contract: false,
        };
        assert_eq!(
            new_session(&opts).system_prompt(),
            "From the custom config."
        );
        // [chat] system_prompt still wins over the file's [llm] one.
        opts.chat.system_prompt = "Chat wins.".into();
        assert_eq!(new_session(&opts).system_prompt(), "Chat wins.");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn plan_prompt_for(dir: &std::path::Path) -> (ChangePlan, PlanPrompt) {
        use lillith_core::access::plan::PlanRequest;
        let plan = ChangePlan::build(
            vec![
                PlanRequest::Mkdir {
                    path: dir.join("A"),
                },
                PlanRequest::Mkdir {
                    path: dir.join("B"),
                },
                PlanRequest::Command {
                    command: format!("touch '{}'", dir.join("ran").display()),
                    cwd: None,
                    timeout_s: 5,
                    shell: false,
                },
            ],
            None,
            None,
        )
        .unwrap();
        let p = PlanPrompt::of(&plan, None);
        (plan, p)
    }

    #[test]
    fn plan_review_sends_only_the_ticked_items_and_esc_declines() {
        let dir = std::env::temp_dir().join(format!("lillith-bubble-plan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (_, prompt) = plan_prompt_for(&dir);
        assert_eq!(prompt.rows.len(), 3);
        assert!(
            prompt.rows[2].fields.iter().any(|(k, _)| k == "program"),
            "{:?}",
            prompt.rows[2]
        );
        let (mut b, req_rx) = test_bubble(None, false);
        b.plan = Some(prompt.clone());
        let ctx = egui::Context::default();
        ctx.set_fonts(bubble_fonts());
        let run = |b: &mut Bubble, ev: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(BUBBLE_SIZE.0, BUBBLE_SIZE.1),
                )),
                events: ev,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| b.plan_popup(ctx));
        };
        let click = |b: &mut Bubble, pos: egui::Pos2| {
            run(b, vec![egui::Event::PointerMoved(pos)]);
            for pressed in [true, false] {
                run(
                    b,
                    vec![egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
            }
        };
        let rect = |key: (&str, usize)| {
            ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("lillith-plan-review").with(key)))
        };
        let named = |key: &str| {
            ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("lillith-plan-review").with(key)))
                .expect("laid out")
        };
        for _ in 0..3 {
            run(&mut b, vec![]);
        }
        assert!(req_rx.try_recv().is_err(), "nothing answered yet");
        // While the review is up the input is locked and focus loss doesn't close.
        assert!(b.plan.is_some());
        // Untick item 2, approve the selection.
        if let Some(r) = rect(("row", 2)) {
            click(&mut b, r.center());
        } else {
            b.plan_state.checked.remove(&2);
        }
        click(&mut b, named("some-rect").center());
        match req_rx.try_recv() {
            Ok(Req::ApprovePlan { items, .. }) => assert_eq!(items, vec![1, 3]),
            _ => panic!("expected ApprovePlan"),
        }
        assert!(b.plan.is_none());
        // Esc declines.
        b.plan = Some(prompt);
        b.plan_state = PlanState::default();
        for _ in 0..2 {
            run(&mut b, vec![]);
        }
        run(&mut b, vec![esc()]);
        assert!(matches!(req_rx.try_recv(), Ok(Req::DeclinePlan)));
        assert!(!b.closing);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn worker_runs_only_approved_plan_items_and_decline_runs_nothing() {
        let dir =
            std::env::temp_dir().join(format!("lillith-bubble-worker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (ui_tx, ui_rx) = channel();
        let mut w = Worker {
            session: ChatSession::new(&ChatConfig::default(), true),
            rt: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
            ui_tx,
            emit: Arc::new(|_, _| {}),
            ctx: egui::Context::default(),
            pending: None,
            deletion: None,
            plan: None,
        };
        let file = |w: &mut Worker| {
            let (plan, _) = plan_prompt_for(&dir);
            let mut sc = SoulContract::new(None);
            let id = sc.request(PendingAction::Plan { plan });
            w.plan = Some((sc, id));
        };
        file(&mut w);
        w.decline_plan();
        assert!(!dir.join("A").exists() && !dir.join("B").exists() && !dir.join("ran").exists());
        file(&mut w);
        w.approve_plan(&[2, 3], "", false);
        assert!(!dir.join("A").exists(), "item 1 was not approved");
        assert!(dir.join("B").is_dir());
        assert!(dir.join("ran").exists(), "approved command ran");
        assert!(w.plan.is_none());
        let notes: Vec<String> = ui_rx
            .try_iter()
            .filter_map(|m| match m {
                Ui::Note(t) => Some(t),
                _ => None,
            })
            .collect();
        assert!(notes.iter().any(|n| n.contains("Declined")), "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("✅ Plan")), "{notes:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
