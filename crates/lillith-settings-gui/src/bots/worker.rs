//! The supervised local worker behind each bot job.
//!
//! [`think`] runs a job's planning on its own thread (recipe and, for the
//! builder bots, the local model on loopback) and reports back over a
//! channel. [`execute`] runs ONE step after its Soul Contract Yes, through
//! a fresh contract (`CONFIRM` audit line) on another thread, streaming
//! output lines. Both stop on the job's own flag ([`Thinking::stop`]) and on
//! the process-wide kill switch (a moved [`kill_switch::epoch`]); a running
//! command is killed by the kill switch itself.

use super::{recipes, vet, BotDef, Plan};
use crate::task::Task;
use lillith_core::access::kill_switch;
use lillith_core::soul_contract::{PendingAction, SoulContract};
use lillith_setup::check::{test_chat_with, ChatTestOptions};
use lillith_setup::config::{Backend, LlmConfig};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

/// What a thinking job reports.
#[derive(Debug, Clone, PartialEq)]
pub enum BotEvent {
    Progress(String),
    /// The vetted plan plus the steps that were refused (with reasons).
    Planned {
        plan: Plan,
        refused: Vec<String>,
    },
    Failed(String),
    Stopped,
}

/// A planning job in flight.
pub struct Thinking {
    rx: Receiver<BotEvent>,
    stop: Arc<AtomicBool>,
}

impl Thinking {
    /// Asks the job to stop (its result is dropped).
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Everything reported since the last call.
    pub fn poll(&self) -> Vec<BotEvent> {
        self.rx.try_iter().collect()
    }
}

/// How the model is reached (`None`: recipes only).
#[derive(Clone)]
pub struct ModelAccess {
    pub cfg: LlmConfig,
    pub timeout: Duration,
}

impl ModelAccess {
    /// The bot's model choice applied to Lillith's `[llm]`: an Ollama
    /// model name replaces `ollama_model`; llama-server serves the model
    /// it has loaded. The address stays the configured loopback one.
    pub fn for_bot(&self, bot: &BotDef) -> LlmConfig {
        let mut c = self.cfg.clone();
        if !bot.model.trim().is_empty() && c.backend == Backend::Ollama {
            c.ollama_model = bot.model.trim().to_string();
        }
        c
    }
}

/// Starts planning `job` for `bot`. `audit` gets one `READ` line for the
/// planning read; `repaint` is poked on every event.
pub fn think(
    bot: BotDef,
    job: String,
    model: Option<ModelAccess>,
    audit: Option<PathBuf>,
    repaint: Option<egui::Context>,
) -> Thinking {
    let (tx, rx) = channel();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let epoch = kill_switch::epoch();
    let _ = std::thread::Builder::new()
        .name(format!("lillith-bot-{}", bot.id))
        .spawn(move || {
            let send = |e: BotEvent| {
                let _ = tx.send(e);
                if let Some(c) = &repaint {
                    c.request_repaint();
                }
            };
            let stopped = || flag.load(Ordering::SeqCst) || kill_switch::stopped_since(epoch);
            let ev = plan_job(&bot, &job, model.as_ref(), audit, &send, &stopped);
            send(if stopped() { BotEvent::Stopped } else { ev });
        });
    Thinking { rx, stop }
}

fn plan_job(
    bot: &BotDef,
    job: &str,
    model: Option<&ModelAccess>,
    audit: Option<PathBuf>,
    send: &dyn Fn(BotEvent),
    stopped: &dyn Fn() -> bool,
) -> BotEvent {
    let Some(base) = bot.resolved_folders().into_iter().next() else {
        return BotEvent::Failed("I have no allowed folder yet: add one in my settings.".into());
    };
    SoulContract::new(audit).record_note(
        "READ",
        &PendingAction::ListDir { path: base.clone() },
        &format!("bot={} planning (read-only)", bot.id),
    );
    let mut plan: Option<Plan> = None;
    if bot.kind.uses_model() && bot.use_model {
        if let Some(m) = model {
            send(BotEvent::Progress(
                "asking the local model (loopback only)…".into(),
            ));
            let opts = ChatTestOptions {
                connect_timeout: Duration::from_secs(2),
                timeout: m.timeout,
                max_tokens: 1500,
                system_prompt: Some(super::system_prompt(bot, &base)),
            };
            match test_chat_with(&m.for_bot(bot), job, &opts) {
                Ok(text) => match super::parse_model_plan(&text, &base) {
                    Some(p) if !p.steps.is_empty() => plan = Some(p),
                    _ => send(BotEvent::Progress(
                        "the model's answer had no usable steps; using my built-in recipe".into(),
                    )),
                },
                Err(e) => send(BotEvent::Progress(format!(
                    "local model not available ({e}); using my built-in recipe"
                ))),
            }
        }
    }
    if stopped() {
        return BotEvent::Stopped;
    }
    let plan = match plan {
        Some(p) => p,
        None => {
            send(BotEvent::Progress(
                "planning with my built-in recipe…".into(),
            ));
            match recipes::plan(bot, job) {
                Ok(p) => p,
                Err(e) => return BotEvent::Failed(e),
            }
        }
    };
    let (plan, refused) = vet(bot, plan);
    BotEvent::Planned { plan, refused }
}

/// One approved step running.
pub struct Exec {
    pub task: Task<Result<String, String>>,
    pub lines: Receiver<String>,
}

/// Runs `action` after its mouse Yes: a fresh contract confirms it (the
/// `CONFIRM` audit line, plus a `BOT_STEP` line naming the bot), streaming
/// build/run output lines.
pub fn execute(
    ctx: &egui::Context,
    bot_id: &str,
    action: PendingAction,
    audit: Option<PathBuf>,
) -> Exec {
    let (tx, rx): (Sender<String>, Receiver<String>) = channel();
    let repaint = ctx.clone();
    let id = bot_id.to_string();
    let task = Task::spawn(ctx, "bot-step", move || {
        let mut sc = SoulContract::new(audit);
        sc.record_note("BOT_STEP", &action, &format!("bot={id}"));
        let rid = sc.request(action);
        sc.confirm_streaming(rid, &mut |line: &str| {
            let _ = tx.send(line.to_string());
            repaint.request_repaint();
        })
        .map(|c| c.output.unwrap_or_default())
        .map_err(|e| e.to_string())
    });
    Exec { task, lines: rx }
}

/// Audits a declined step (`DENY`).
pub fn deny(action: PendingAction, audit: Option<PathBuf>) {
    let mut sc = SoulContract::new(audit);
    let id = sc.request(action);
    let _ = sc.deny(id);
}
