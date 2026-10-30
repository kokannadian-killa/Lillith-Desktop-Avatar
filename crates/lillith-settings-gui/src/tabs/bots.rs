//! Bots: Lillith's local helper bots, laid out like a bots dashboard.
//!
//! Left: the roster (avatar, name, one-line role, status), **+ New bot**
//! (from a template), **Stop all** (the kill switch) and the dream-learning
//! status hook. Right: the picked bot's pane: its chat / task log where Ko
//! types a job and sees progress, results and the files it produced, or its
//! settings (name, description, allowed folders, model, on/off).
//!
//! Every step a bot proposes waits for a mouse click on the shared Soul
//! Contract card ([`soul_contract_popup`]; keyboard, typed or spoken yes
//! never approves) and is audited (`CONFIRM` / `DENY` / `BOT_STEP`). The
//! engine is [`crate::bots`].

use super::{SettingsTab, Shared};
use crate::bots::worker::{self, BotEvent, Exec, ModelAccess, Thinking};
use crate::bots::{
    self, BotDef, BotKind, BotStatus, DreamStatus, DreamStatusSource, NoDream, Step,
};
use crate::chrome;
use egui::{pos2, vec2, Align, Align2, FontId, Layout, RichText, Sense, Stroke};
use lillith_core::access::kill_switch;
use lillith_core::soul_contract::{PendingAction, SoulContract};
use lillith_theme::egui_contract::{soul_contract_popup, ContractChoice, ContractView, Initiator};
use lillith_theme::egui_theme::c32;
use lillith_theme::palette as pal;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

/// Popup id of the tab's Soul Contract card.
pub const CARD_ID: &str = "bots-soul-contract";
/// Chat lines kept per bot.
const MAX_CHAT: usize = 400;
/// Width of the roster column.
const ROSTER_W: f32 = 260.0;

/// Who said a chat line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Who {
    Ko,
    Bot,
    System,
}

/// A bot's live state (not saved).
#[derive(Default)]
struct Run {
    status: BotStatus,
    chat: Vec<(Who, String)>,
    files: Vec<PathBuf>,
    thinking: Option<Thinking>,
    queue: VecDeque<Step>,
    /// Steps in the current plan (for "step k of n").
    plan_len: usize,
    exec: Option<(Exec, Step)>,
    input: String,
}

impl Run {
    fn say(&mut self, who: Who, text: impl Into<String>) {
        self.chat.push((who, text.into()));
        if self.chat.len() > MAX_CHAT {
            let n = self.chat.len() - MAX_CHAT;
            self.chat.drain(..n);
        }
    }
    fn busy(&self) -> bool {
        self.thinking.is_some() || self.exec.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Chat,
    Settings,
    NewBot,
}

pub struct BotsTab {
    roster: Vec<BotDef>,
    /// The roster as saved (for `pending` / `revert`).
    saved: Vec<BotDef>,
    runs: Vec<Run>,
    selected: usize,
    view: View,
    roster_path: PathBuf,
    load_error: Option<String>,
    audit: Option<PathBuf>,
    /// Organize undo journal (`<data>/lillith/organize-journal`).
    journal: PathBuf,
    model: Option<ModelAccess>,
    dream: Box<dyn DreamStatusSource>,
    new_kind: BotKind,
    new_name: String,
    folder_buf: String,
    kill_epoch: u64,
    ctx: Option<egui::Context>,
}

impl BotsTab {
    pub fn new(shared: &Shared) -> Self {
        let roster_path = bots::roster_path(&shared.config_path);
        let (roster, load_error) = match bots::load_roster(&roster_path) {
            Ok(r) => (r, None),
            Err(e) => (bots::stock_roster(), Some(e)),
        };
        let data = lillith_core::config::data_dir();
        let model = lillith_setup::config::load(&shared.config_path)
            .ok()
            .map(|cfg| ModelAccess {
                cfg,
                timeout: Duration::from_secs(180),
            });
        let runs = roster.iter().map(|b| off_run(b)).collect();
        Self {
            saved: roster.clone(),
            roster,
            runs,
            selected: 0,
            view: View::Chat,
            roster_path,
            load_error,
            audit: data.as_ref().map(|d| d.join("audit.log")),
            journal: data
                .map(|d| d.join("organize-journal"))
                .unwrap_or_else(|| std::env::temp_dir().join("lillith-organize-journal")),
            model,
            dream: Box::new(NoDream),
            new_kind: BotKind::FileOrganizer,
            new_name: String::new(),
            folder_buf: String::new(),
            kill_epoch: kill_switch::epoch(),
            ctx: None,
        }
    }

    /// Plugs in Memory Engine's dream-session status (read-only).
    pub fn set_dream_source(&mut self, src: Box<dyn DreamStatusSource>) {
        self.dream = src;
    }

    /// The roster (as edited).
    pub fn roster(&self) -> &[BotDef] {
        &self.roster
    }

    /// Status of bot `i`.
    pub fn status(&self, i: usize) -> BotStatus {
        self.runs.get(i).map(|r| r.status).unwrap_or_default()
    }

    /// The dream line shown in the roster column.
    pub fn dream_line(&self) -> String {
        dream_text(self.dream.status())
    }

    fn select(&mut self, i: usize) {
        if i < self.roster.len() {
            self.selected = i;
            self.folder_buf.clear();
        }
    }

    /// Gives bot `i` the job in its input box.
    fn submit(&mut self, i: usize) {
        let Some(bot) = self.roster.get(i).cloned() else {
            return;
        };
        let audit = self.audit.clone();
        let model = self.model.clone();
        let ctx = self.ctx.clone();
        let r = &mut self.runs[i];
        let job: String = r.input.trim().chars().take(bots::MAX_JOB_CHARS).collect();
        if job.is_empty() || r.busy() || !r.queue.is_empty() {
            return;
        }
        if !bot.enabled {
            r.say(Who::System, format!("{} is off (Settings ▸ on).", bot.name));
            return;
        }
        r.input.clear();
        r.say(Who::Ko, job.clone());
        r.status = BotStatus::Working;
        r.thinking = Some(worker::think(bot, job, model, audit, ctx));
    }

    /// Stops bot `i`: drops its plan, ends its thinking and (through the
    /// kill switch) a running step.
    fn stop(&mut self, i: usize) {
        let audit = self.audit.clone();
        let journal = self.journal.clone();
        let Some(r) = self.runs.get_mut(i) else {
            return;
        };
        if let Some(t) = r.thinking.take() {
            t.stop();
        }
        if let Some(s) = r.queue.front() {
            SoulContract::new(audit).record_note(
                "CANCELLED",
                &s.action(&journal),
                &format!("bot={} stopped", self.roster[i].id),
            );
        }
        r.queue.clear();
        if r.exec.is_some() {
            // A running command only stops through the kill switch.
            let _ = kill_switch::stop_all();
            self.kill_epoch = kill_switch::epoch();
            for (j, o) in self.runs.iter_mut().enumerate() {
                if j != i && o.busy() {
                    o.say(Who::System, "stopped by the kill switch");
                }
            }
        }
        let r = &mut self.runs[i];
        r.say(Who::System, "■ stopped");
        r.status = BotStatus::Stopped;
    }

    /// The kill switch moved (Ctrl-C, Stop all, another stop): every bot
    /// drops its work.
    fn on_kill_switch(&mut self) {
        for r in &mut self.runs {
            let active = r.busy() || !r.queue.is_empty();
            if let Some(t) = r.thinking.take() {
                t.stop();
            }
            r.queue.clear();
            if active {
                r.status = BotStatus::Stopped;
                r.say(Who::System, "■ stopped by the kill switch");
            }
        }
    }

    /// Mouse Yes on the card: run the front step of bot `i`.
    fn approve(&mut self, i: usize, ctx: &egui::Context) {
        let audit = self.audit.clone();
        let journal = self.journal.clone();
        let id = self.roster[i].id.clone();
        let r = &mut self.runs[i];
        if r.exec.is_some() {
            return;
        }
        let Some(step) = r.queue.pop_front() else {
            return;
        };
        let action = step.action(&journal);
        r.say(Who::System, format!("▶ {}", action.summary()));
        r.status = BotStatus::Working;
        r.exec = Some((worker::execute(ctx, &id, action, audit), step));
    }

    /// No on the card: audit `DENY`, skip the step.
    fn decline(&mut self, i: usize) {
        let audit = self.audit.clone();
        let journal = self.journal.clone();
        let r = &mut self.runs[i];
        let Some(step) = r.queue.pop_front() else {
            return;
        };
        let action = step.action(&journal);
        r.say(Who::System, format!("✕ declined: {}", action.summary()));
        worker::deny(action, audit);
        if r.queue.is_empty() {
            r.status = BotStatus::Done;
            r.say(Who::Bot, "Okay, I left it there.");
        }
    }

    /// Moves worker results into the bots' state.
    fn poll(&mut self) {
        for i in 0..self.runs.len() {
            let name = self.roster[i].name.clone();
            let pose = self.roster[i].kind == BotKind::PoseBuilder;
            let r = &mut self.runs[i];
            let events = r.thinking.as_ref().map(|t| t.poll()).unwrap_or_default();
            for ev in events {
                match ev {
                    BotEvent::Progress(p) => r.say(Who::System, format!("· {p}")),
                    BotEvent::Planned { plan, refused } => {
                        r.thinking = None;
                        if !plan.say.is_empty() {
                            r.say(Who::Bot, plan.say);
                        }
                        for n in plan.notes.iter().take(40) {
                            r.say(Who::System, format!("• {n}"));
                        }
                        for x in refused {
                            r.say(Who::System, format!("✕ {x}"));
                        }
                        r.plan_len = plan.steps.len();
                        r.queue = plan.steps.into();
                        if r.queue.is_empty() {
                            r.status = BotStatus::Done;
                        } else {
                            r.status = BotStatus::NeedsApproval;
                            r.say(
                                Who::System,
                                format!(
                                    "{} step(s) waiting for your Yes on the card (mouse click only)",
                                    r.queue.len()
                                ),
                            );
                        }
                    }
                    BotEvent::Failed(e) => {
                        r.thinking = None;
                        r.say(Who::Bot, e);
                        r.status = BotStatus::Failed;
                    }
                    BotEvent::Stopped => {
                        r.thinking = None;
                        r.status = BotStatus::Stopped;
                    }
                }
            }
            let mut finished = None;
            if let Some((ex, _)) = &mut r.exec {
                let lines: Vec<String> = ex.lines.try_iter().collect();
                let res = ex.task.poll();
                for l in lines {
                    r.chat.push((Who::System, l));
                }
                finished = res;
            }
            if let Some(res) = finished {
                let (_, step) = r.exec.take().expect("exec");
                match res {
                    Ok(out) => {
                        for l in out.lines() {
                            if pose {
                                if let crate::video_import::Event::Candidate(c) =
                                    crate::video_import::parse_line(l)
                                {
                                    r.files.push(c.mp4.clone());
                                    r.say(
                                        Who::System,
                                        format!("★ pose candidate: {}", c.suggested_name),
                                    );
                                }
                            }
                        }
                        let tail: String = out
                            .chars()
                            .rev()
                            .take(1500)
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                            .collect();
                        if !tail.trim().is_empty() && !pose {
                            r.say(Who::System, tail.trim().to_string());
                        }
                        if let Some(p) = step.produces() {
                            if !r.files.contains(&p) {
                                r.files.push(p);
                            }
                        }
                        if r.queue.is_empty() {
                            r.status = BotStatus::Done;
                            r.say(
                                Who::Bot,
                                if pose {
                                    "Done. Approve the poses you like in Pose Builder.".to_string()
                                } else {
                                    format!("Done. — {name}")
                                },
                            );
                        } else {
                            r.status = BotStatus::NeedsApproval;
                        }
                    }
                    Err(e) => {
                        r.say(Who::System, format!("✕ {e}"));
                        r.queue.clear();
                        r.status = BotStatus::Failed;
                    }
                }
            }
            let _ = &r.chat;
            if r.chat.len() > MAX_CHAT {
                let n = r.chat.len() - MAX_CHAT;
                r.chat.drain(..n);
            }
        }
    }

    /// The front step of bot `i` as a pending action plus card text.
    fn card(&self, i: usize) -> Option<(PendingAction, String)> {
        let r = self.runs.get(i)?;
        if r.exec.is_some() || r.thinking.is_some() {
            return None;
        }
        let step = r.queue.front()?;
        let bot = &self.roster[i];
        let action = step.action(&self.journal);
        let k = r.plan_len.saturating_sub(r.queue.len()) + 1;
        let prompt = action.prompt();
        let mut detail = format!(
            "{} ({}) proposes step {k} of {}.\n",
            bot.name,
            bot.kind.role(),
            r.plan_len.max(k)
        );
        if let Some(d) = prompt.detail {
            detail.push('\n');
            detail.push_str(&d);
        }
        Some((action, detail))
    }

    fn roster_ui(&mut self, ui: &mut egui::Ui) {
        for i in 0..self.roster.len() {
            let w = ui.available_width();
            let (rect, resp) = ui.allocate_exact_size(vec2(w, 56.0), Sense::click());
            let sel = i == self.selected && self.view != View::NewBot;
            let p = ui.painter();
            if sel || resp.hovered() {
                p.rect_filled(rect, 6.0, c32(if sel { pal::STONE_HI } else { pal::STONE }));
            }
            if sel {
                p.rect_stroke(
                    rect,
                    6.0,
                    Stroke::new(1.0, c32(pal::BLOOD_NEON.alpha(140))),
                    egui::StrokeKind::Inside,
                );
            }
            let b = &self.roster[i];
            let st = if b.enabled {
                self.runs[i].status
            } else {
                BotStatus::Off
            };
            avatar(p, pos2(rect.left() + 26.0, rect.center().y), 18.0, b);
            p.text(
                pos2(rect.left() + 52.0, rect.top() + 9.0),
                Align2::LEFT_TOP,
                &b.name,
                FontId::proportional(15.0),
                c32(pal::TEXT),
            );
            p.text(
                pos2(rect.left() + 52.0, rect.top() + 29.0),
                Align2::LEFT_TOP,
                b.kind.role(),
                FontId::proportional(11.5),
                c32(pal::TEXT_DIM),
            );
            let dot = pos2(rect.right() - 12.0, rect.top() + 16.0);
            p.circle_filled(dot, 4.0, c32(st.color()));
            p.text(
                pos2(rect.right() - 22.0, rect.top() + 16.0),
                Align2::RIGHT_CENTER,
                st.label(),
                FontId::monospace(10.5),
                c32(st.color()),
            );
            if resp.clicked() {
                self.select(i);
                if self.view == View::NewBot {
                    self.view = View::Chat;
                }
            }
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if chrome::stone_button(ui, "+ New bot").clicked() {
                self.view = View::NewBot;
                self.new_name = bots::new_bot(self.new_kind, &self.roster).name;
            }
            if chrome::stone_button(ui, "⏻ Stop all").clicked() {
                let _ = kill_switch::stop_all();
            }
        });
        chrome::hint(ui, "Stop all = the kill switch (Ctrl-C does the same).");
        ui.add_space(6.0);
        let line = self.dream_line();
        chrome::section(ui, "DREAM LEARNING", Some("Tor · Ko-gated"), |ui| {
            ui.label(RichText::new(line).small().color(c32(pal::TEXT_DIM)));
        });
    }

    fn chat_ui(&mut self, ui: &mut egui::Ui) {
        let i = self.selected;
        let bot = self.roster[i].clone();
        let st = if bot.enabled {
            self.runs[i].status
        } else {
            BotStatus::Off
        };
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(52.0, 52.0), Sense::hover());
            avatar(ui.painter(), rect.center(), 24.0, &bot);
            ui.vertical(|ui| {
                ui.label(RichText::new(&bot.name).size(20.0).color(c32(pal::TEXT)));
                ui.label(RichText::new(bot.kind.role()).color(c32(pal::TEXT_DIM)));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if chrome::stone_button(ui, "⚙ Settings").clicked() {
                    self.view = View::Settings;
                }
                let r = &self.runs[i];
                if (r.busy() || !r.queue.is_empty()) && chrome::stone_button(ui, "■ Stop").clicked()
                {
                    self.stop(i);
                }
                chrome::chip(ui, st.label(), st.color(), true);
            });
        });
        chrome::hint(ui, &bot.description);
        ui.add_space(4.0);
        let files = self.runs[i].files.clone();
        let files_h = if files.is_empty() { 0.0 } else { 90.0 };
        let chat_h = (ui.available_height() - 150.0 - files_h).max(120.0);
        chrome::section(ui, "TASK LOG", Some("local only"), |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("bots-chat", &bot.id))
                .max_height(chat_h)
                .stick_to_bottom(true)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let r = &self.runs[i];
                    if r.chat.is_empty() {
                        chrome::hint(
                            ui,
                            &format!(
                                "Give {} a job below. Every change it proposes waits for your click on the Soul Contract card.",
                                bot.name
                            ),
                        );
                    }
                    for (who, text) in &r.chat {
                        match who {
                            Who::Ko => {
                                ui.label(
                                    RichText::new(format!("You: {text}")).color(c32(pal::BONE)),
                                );
                            }
                            Who::Bot => {
                                ui.label(
                                    RichText::new(format!("{}: {text}", bot.name))
                                        .color(c32(bot.kind.color())),
                                );
                            }
                            Who::System => {
                                ui.label(
                                    RichText::new(text)
                                        .monospace()
                                        .small()
                                        .color(c32(pal::TEXT_DIM)),
                                );
                            }
                        }
                    }
                });
        });
        if !files.is_empty() {
            chrome::section(ui, "FILES PRODUCED", None, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(("bots-files", &bot.id))
                    .max_height(60.0)
                    .show(ui, |ui| {
                        for f in &files {
                            ui.label(
                                RichText::new(f.display().to_string())
                                    .monospace()
                                    .small()
                                    .color(c32(pal::TEXT)),
                            );
                        }
                    });
            });
        }
        let ready = bot.enabled && !self.runs[i].busy() && self.runs[i].queue.is_empty();
        ui.horizontal(|ui| {
            let w = ui.available_width() - 130.0;
            ui.add(
                egui::TextEdit::multiline(&mut self.runs[i].input)
                    .hint_text(if bot.enabled {
                        format!("Give {} a job…", bot.name)
                    } else {
                        format!("{} is off — turn it on in Settings", bot.name)
                    })
                    .desired_rows(2)
                    .desired_width(w.max(120.0)),
            );
            let has_text = !self.runs[i].input.trim().is_empty();
            if chrome::blade_button(ui, "Send job", ready && has_text).clicked() {
                self.submit(i);
            }
        });
        if bot.kind == BotKind::PoseBuilder {
            chrome::hint(
                ui,
                "Candidates land in Pose Builder's staging area: approve them in the Pose Builder tab.",
            );
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let i = self.selected;
        chrome::section(ui, "BOT SETTINGS", Some(bots::ROSTER_FILE), |ui| {
            let b = &mut self.roster[i];
            chrome::row(ui, "Name", |ui| {
                ui.add(egui::TextEdit::singleline(&mut b.name).desired_width(240.0));
            });
            chrome::row(ui, "Role", |ui| {
                ui.label(RichText::new(b.kind.role()).color(c32(pal::TEXT_DIM)));
            });
            chrome::row(ui, "Description", |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut b.description)
                        .desired_rows(2)
                        .desired_width(420.0),
                );
            });
            chrome::row(ui, "On", |ui| {
                if chrome::rune_check(ui, b.enabled, "bot is on").clicked() {
                    b.enabled = !b.enabled;
                }
            });
            chrome::row(ui, "Model", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut b.model)
                        .hint_text("Lillith's local model")
                        .desired_width(240.0),
                );
            });
            if b.kind.uses_model() {
                chrome::row(ui, "Plan with model", |ui| {
                    if chrome::rune_check(ui, b.use_model, "let the local model plan jobs")
                        .clicked()
                    {
                        b.use_model = !b.use_model;
                    }
                });
            }
            chrome::hint(
                ui,
                "Model: an Ollama model name, or empty for Lillith's model. llama-server serves the model it has loaded. Always loopback (127.0.0.1), never online.",
            );
            ui.add_space(6.0);
            chrome::label_os(ui, "ALLOWED FOLDERS", pal::TEXT_DIM);
            let mut remove = None;
            for (k, f) in b.folders.iter().enumerate() {
                ui.horizontal(|ui| {
                    if chrome::stone_button(ui, "✕").clicked() {
                        remove = Some(k);
                    }
                    ui.label(RichText::new(f).monospace().color(c32(pal::TEXT)));
                });
            }
            if let Some(k) = remove {
                b.folders.remove(k);
            }
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.folder_buf)
                        .hint_text("~/Projects/my-folder")
                        .desired_width(300.0),
                );
                if chrome::stone_button(ui, "Add folder").clicked() {
                    let f = self.folder_buf.trim().to_string();
                    if bots::expand_home(&f).is_some() && !b.folders.contains(&f) {
                        b.folders.push(f);
                        self.folder_buf.clear();
                    } else {
                        shared.toast("Use an absolute path or ~/…", pal::BLOOD_NEON);
                    }
                }
            });
            chrome::hint(
                ui,
                "The bot only reads and proposes changes inside these folders. Relative paths go in the first one.",
            );
        });
        ui.horizontal(|ui| {
            if chrome::stone_button(ui, "← Back to tasks").clicked() {
                self.view = View::Chat;
            }
            chrome::hint(ui, "Save (bottom bar) writes bots.toml.");
        });
    }

    fn new_bot_ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        chrome::section(ui, "NEW BOT", Some("from a template"), |ui| {
            for (k, kind) in BotKind::ALL.into_iter().enumerate() {
                if chrome::choice(ui, kind.role(), self.new_kind == kind, k + 3).clicked() {
                    self.new_kind = kind;
                }
                chrome::hint(ui, kind.blurb());
            }
            ui.add_space(6.0);
            chrome::row(ui, "Name", |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.new_name).desired_width(240.0));
            });
            if chrome::blade_button(ui, "Create bot", !self.new_name.trim().is_empty()).clicked() {
                let mut b = bots::new_bot(self.new_kind, &self.roster);
                b.name = self.new_name.trim().to_string();
                self.roster.push(b.clone());
                self.runs.push(off_run(&b));
                self.selected = self.roster.len() - 1;
                self.view = View::Settings;
                shared.toast(
                    format!("{} created — set its folders, then Save", b.name),
                    pal::VIOLET,
                );
            }
        });
        if chrome::stone_button(ui, "← Cancel").clicked() {
            self.view = View::Chat;
        }
    }
}

fn off_run(b: &BotDef) -> Run {
    Run {
        status: if b.enabled {
            BotStatus::Idle
        } else {
            BotStatus::Off
        },
        ..Default::default()
    }
}

/// The dream-learning status line.
pub fn dream_text(s: Option<DreamStatus>) -> String {
    match s {
        None => "No dream session. Online research only happens in a dream session you approve (Tor, mouse Yes); bots never go online.".into(),
        Some(d) => format!(
            "Session {}: {} · {} min left · {}/{} jobs{}",
            d.session,
            d.state,
            d.minutes_left,
            d.jobs_done,
            d.jobs_total,
            if d.note.is_empty() {
                String::new()
            } else {
                format!(" · {}", d.note)
            }
        ),
    }
}

/// A round avatar: ring in the kind's colour and the name's initial.
fn avatar(p: &egui::Painter, c: egui::Pos2, r: f32, b: &BotDef) {
    let col = if b.enabled {
        b.kind.color()
    } else {
        pal::TEXT_MUTE
    };
    p.circle_filled(c, r, c32(pal::OBSIDIAN_HI));
    p.circle_filled(c, r, c32(col.alpha(40)));
    p.circle_stroke(c, r, Stroke::new(1.6, c32(col)));
    let initial: String = b.name.chars().take(1).collect::<String>().to_uppercase();
    p.text(
        c,
        Align2::CENTER_CENTER,
        initial,
        FontId::proportional(r * 1.05),
        c32(col),
    );
}

impl SettingsTab for BotsTab {
    fn id(&self) -> &str {
        "bots"
    }
    fn title(&self) -> &str {
        "Bots"
    }
    fn rune(&self) -> usize {
        13
    }
    fn busy(&self) -> bool {
        self.runs.iter().any(Run::busy)
    }
    fn pending(&self) -> usize {
        usize::from(self.roster != self.saved)
    }
    fn save(&mut self, _shared: &mut Shared) -> Result<(), String> {
        if self.roster == self.saved {
            return Ok(());
        }
        bots::save_roster(&self.roster_path, &self.roster)?;
        self.saved = self.roster.clone();
        self.load_error = None;
        Ok(())
    }
    fn revert(&mut self, _shared: &mut Shared) {
        self.roster = self.saved.clone();
        self.runs.truncate(self.roster.len());
        self.selected = self.selected.min(self.roster.len().saturating_sub(1));
        if self.view == View::NewBot {
            self.view = View::Chat;
        }
    }
    fn deep_link(&mut self, arg: &str) {
        match arg {
            "new" => self.view = View::NewBot,
            "settings" => self.view = View::Settings,
            a => {
                if let Some(i) = self
                    .roster
                    .iter()
                    .position(|b| b.id == a || b.name.eq_ignore_ascii_case(a))
                {
                    self.select(i);
                }
            }
        }
    }

    fn tick(&mut self, ctx: &egui::Context, _shared: &mut Shared) {
        if self.ctx.is_none() {
            self.ctx = Some(ctx.clone());
        }
        let e = kill_switch::epoch();
        if e != self.kill_epoch {
            self.kill_epoch = e;
            self.on_kill_switch();
        }
        self.poll();
        if self.busy() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        if self.ctx.is_none() {
            self.ctx = Some(ui.ctx().clone());
        }
        ui.label(
            RichText::new(
                "Local helper bots. They plan on this computer (local model on loopback, no internet) \
                 and every change, command or launch waits for your mouse click on the Soul Contract card.",
            )
            .color(c32(pal::TEXT_DIM)),
        );
        if let Some(e) = &self.load_error {
            ui.label(
                RichText::new(format!("bots.toml: {e} (showing the stock bots)"))
                    .small()
                    .color(c32(pal::BLOOD_NEON)),
            );
        }
        ui.add_space(4.0);
        let h = ui.available_height().max(320.0);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(vec2(ROSTER_W, h), Layout::top_down(Align::Min), |ui| {
                ui.set_width(ROSTER_W);
                egui::ScrollArea::vertical()
                    .id_salt("bots-roster")
                    .max_height(h)
                    .show(ui, |ui| self.roster_ui(ui));
            });
            ui.add_space(10.0);
            let w = ui.available_width();
            ui.allocate_ui_with_layout(vec2(w, h), Layout::top_down(Align::Min), |ui| {
                ui.set_width(w);
                if self.roster.is_empty() {
                    chrome::hint(ui, "No bots yet: + New bot.");
                    return;
                }
                match self.view {
                    View::Chat => self.chat_ui(ui),
                    View::Settings => self.settings_ui(ui, shared),
                    View::NewBot => self.new_bot_ui(ui, shared),
                }
            });
        });

        // The card for the picked bot's next step (mouse-only Yes).
        if let Some((action, detail)) = self.card(self.selected) {
            let prompt = action.prompt();
            let summary = action.summary();
            let view = ContractView {
                title: prompt.title,
                summary: &summary,
                target: prompt.target.as_deref(),
                detail: Some(&detail),
                busy: false,
                initiator: Initiator::Her,
                scroll_detail: true,
            };
            match soul_contract_popup(ui.ctx(), egui::Id::new(CARD_ID), &view) {
                Some(ContractChoice::Yes) => {
                    let ctx = ui.ctx().clone();
                    self.approve(self.selected, &ctx);
                }
                Some(ContractChoice::No) => self.decline(self.selected),
                None => {}
            }
        }
    }
}

#[cfg(test)]
mod tests;
