//! Pose Builder: import Ko's own videos as poses.
//!
//! Drop a video (`.mp4 .gif .webm .mov`, any size; only its path is used)
//! or pick one with Browse. **Start import** raises a Soul Contract card
//! showing the exact pipeline command (mouse-click Yes only; audited
//! `CONFIRM` / `DENY`). The pipeline (`tools/video-pipeline/user_import.py`,
//! see [`crate::video_import`]) runs as a child process into
//! `<data>/lillith/import-staging/<job>/`, streaming a progress bar; Cancel
//! signals it (SIGTERM / CTRL_BREAK) and removes the staging folder.
//!
//! Each proposed clip gets a card: its preview (PNG, or ▶ Play decodes the
//! MP4 in-app), the QA result, an editable name and Approve / Reject.
//! Approve is a file write, so it has its own Soul Contract card; it copies
//! the clip add-only into `<data>/lillith/user-clips/` and adds it to
//! `user_manifest.toml` ([`lillith_anim::user_clips::install_clip`]: a
//! taken name gets a `_2` suffix, nothing is ever overwritten). The overlay
//! and the Poses lists merge those with the built-in set.

use super::{SettingsTab, Shared};
use crate::chrome;
use crate::task::Task;
use crate::video_import::{self as vi, Candidate, Event, ImportCmd, Job, JobMsg, Outcome};
use egui::RichText;
use lillith_anim::user_clips;
use lillith_core::soul_contract::{PendingAction, SoulContract};
use lillith_theme::egui_contract::{soul_contract_popup, ContractChoice, ContractView, Initiator};
use lillith_theme::egui_theme::c32;
use lillith_theme::palette as pal;
use std::path::{Path, PathBuf};

/// Popup id of the tab's Soul Contract card.
const CARD_ID: &str = "import-soul-contract";
/// Largest preview PNG shown (bytes).
const MAX_PREVIEW_BYTES: u64 = 32 * 1024 * 1024;
/// Largest in-app playback (decoded RGBA bytes).
const MAX_PLAY_BYTES: u64 = 768 * 1024 * 1024;
/// Log lines kept.
const MAX_LOG: usize = 200;

/// Tool paths for a run (found on PATH / bundled, or set by tests).
#[derive(Debug, Clone)]
struct Tools {
    python: PathBuf,
    script: PathBuf,
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
}

/// The decision waiting on the card.
enum Pending {
    Start {
        cmd: ImportCmd,
        action: PendingAction,
    },
    Approve {
        card: usize,
        name: String,
        action: PendingAction,
    },
}

impl Pending {
    fn action(&self) -> &PendingAction {
        match self {
            Pending::Start { action, .. } | Pending::Approve { action, .. } => action,
        }
    }
    /// The audit-line note.
    fn note(&self) -> String {
        match self {
            Pending::Start { cmd, .. } => format!("pose builder import {}", file_name(&cmd.input)),
            Pending::Approve { name, .. } => format!("pose builder add clip {name}"),
        }
    }
    fn summary(&self) -> String {
        match self {
            Pending::Start { cmd, .. } => format!(
                "Import {} into Pose Builder (staging: {})",
                file_name(&cmd.input),
                cmd.out.display()
            ),
            Pending::Approve { name, .. } => {
                format!("Add the clip \"{name}\" to your poses (add-only)")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum CardState {
    Open,
    Approved(String),
    Rejected,
}

type Frames = Result<Vec<egui::ColorImage>, String>;

struct Card {
    c: Candidate,
    /// The candidate's files, confined to the job's staging folder.
    mp4: Option<PathBuf>,
    png: Option<PathBuf>,
    name: String,
    state: CardState,
    tex: Option<egui::TextureHandle>,
    tex_failed: bool,
    decoding: Option<Task<Frames>>,
    frames: Vec<egui::TextureHandle>,
    play_from: f64,
    note: String,
}

pub struct ImportTab {
    /// The chosen video (only its path).
    input: Option<PathBuf>,
    /// Typed path (when there is no file dialog, or to paste one).
    input_buf: String,
    browsing: Option<Task<Option<PathBuf>>>,
    pending: Option<Pending>,
    job: Option<Job>,
    /// The current job's staging folder (kept while cards are open).
    staging: Option<PathBuf>,
    stage: String,
    pct: f32,
    msg: String,
    error: Option<String>,
    cards: Vec<Card>,
    log: Vec<String>,
    audit: Option<PathBuf>,
    staging_root: Option<PathBuf>,
    clips_dir: Option<PathBuf>,
    /// Test hook: fixed tool paths instead of searching.
    tools_override: Option<Tools>,
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

impl ImportTab {
    pub fn new() -> Self {
        Self {
            input: None,
            input_buf: String::new(),
            browsing: None,
            pending: None,
            job: None,
            staging: None,
            stage: String::new(),
            pct: 0.0,
            msg: String::new(),
            error: None,
            cards: Vec::new(),
            log: Vec::new(),
            audit: lillith_core::config::audit_log_path(),
            staging_root: vi::staging_root(),
            clips_dir: user_clips::user_clips_dir(),
            tools_override: None,
        }
    }

    fn log_line(&mut self, l: impl Into<String>) {
        self.log.push(l.into());
        let n = self.log.len();
        if n > MAX_LOG {
            self.log.drain(..n - MAX_LOG);
        }
    }

    /// Sets the video to import (from a drop, Browse or the text field).
    fn choose(&mut self, p: PathBuf, shared: &mut Shared) {
        if !vi::accepted(&p) {
            shared.toast(
                format!(
                    "{} isn't a video (use .mp4, .gif, .webm or .mov)",
                    file_name(&p)
                ),
                pal::WARN,
            );
            return;
        }
        if !p.is_file() {
            shared.toast(format!("{} doesn't exist", p.display()), pal::BAD);
            return;
        }
        self.input_buf = p.display().to_string();
        self.input = Some(p);
        self.error = None;
    }

    fn tools(&self) -> Result<Tools, String> {
        if let Some(t) = &self.tools_override {
            return Ok(t.clone());
        }
        let (python, ffmpeg, ffprobe, script) = (
            vi::find_python(),
            vi::find_ffmpeg(),
            vi::find_ffprobe(),
            vi::find_script(),
        );
        if let Some(m) = vi::missing_tools(
            python.as_deref(),
            ffmpeg.as_deref(),
            ffprobe.as_deref(),
            script.as_deref(),
        ) {
            return Err(m);
        }
        Ok(Tools {
            python: python.unwrap_or_default(),
            script: script.unwrap_or_default(),
            ffmpeg: ffmpeg.unwrap_or_default(),
            ffprobe: ffprobe.unwrap_or_default(),
        })
    }

    /// **Start import**: builds the command and raises its card.
    fn propose_start(&mut self) {
        if self.job.is_some() {
            return;
        }
        let Some(input) = self.input.clone() else {
            self.error = Some("Drop a video here or pick one with Browse first.".into());
            return;
        };
        let tools = match self.tools() {
            Ok(t) => t,
            Err(m) => {
                self.error = Some(m);
                return;
            }
        };
        let Some(root) = self.staging_root.clone() else {
            self.error = Some("No data folder (set $HOME / %LOCALAPPDATA%).".into());
            return;
        };
        let cmd = ImportCmd {
            python: tools.python,
            script: tools.script,
            input,
            out: root.join(vi::new_job_id()),
            max_ram_mb: vi::DEFAULT_MAX_RAM_MB,
            ffmpeg: tools.ffmpeg,
            ffprobe: tools.ffprobe,
        };
        let action = PendingAction::RunCommand {
            command: cmd.display(),
            cwd: None,
            timeout_s: 0,
        };
        self.error = None;
        self.pending = Some(Pending::Start { cmd, action });
    }

    /// **Approve** on a card: raises its write card.
    fn propose_approve(&mut self, i: usize) {
        let Some(dir) = self.clips_dir.clone() else {
            self.error = Some("No data folder for user clips.".into());
            return;
        };
        let Some(card) = self.cards.get(i).filter(|c| c.state == CardState::Open) else {
            return;
        };
        let (Some(mp4), Some(png)) = (&card.mp4, &card.png) else {
            self.error = Some("This clip's files are missing from staging.".into());
            return;
        };
        let base = user_clips::sanitize_name(&card.name);
        let body = format!(
            "Copies (add-only; a taken name gets a _2, _3… suffix, nothing is overwritten):\n  {}\n  {}\ninto {}\nand adds [clips.{base}] ({} frames @ {} fps, tag \"user\") to {}",
            mp4.display(),
            png.display(),
            dir.join(&base).display(),
            card.c.frames,
            card.c.fps,
            dir.join(user_clips::USER_MANIFEST).display(),
        );
        let action = PendingAction::WriteFile {
            path: dir.join(user_clips::USER_MANIFEST),
            body,
        };
        self.pending = Some(Pending::Approve {
            card: i,
            name: base,
            action,
        });
    }

    fn with_view<R>(&self, f: impl FnOnce(&ContractView<'_>) -> R) -> Option<R> {
        let p = self.pending.as_ref()?;
        let prompt = p.action().prompt();
        let summary = p.summary();
        let view = ContractView {
            title: prompt.title,
            summary: &summary,
            target: prompt.target.as_deref(),
            detail: prompt.detail.as_deref(),
            busy: false,
            // Every card here comes from Ko's own button press.
            initiator: Initiator::You,
            scroll_detail: prompt.scrolls(),
        };
        Some(f(&view))
    }

    /// Card **Yes** (mouse only): audit `CONFIRM`, then do it.
    fn confirm(&mut self, ctx: &egui::Context) {
        let Some(p) = self.pending.take() else {
            return;
        };
        SoulContract::new(self.audit.clone()).record_note("CONFIRM", p.action(), &p.note());
        match p {
            Pending::Start { cmd, .. } => self.start(cmd, ctx),
            Pending::Approve { card, .. } => self.approve(card),
        }
    }

    /// Card **No** / Esc: audit `DENY`, nothing happens.
    fn deny(&mut self) {
        let Some(p) = self.pending.take() else {
            return;
        };
        SoulContract::new(self.audit.clone()).record_note("DENY", p.action(), &p.note());
        self.log_line(format!("✕ denied: {}", p.summary()));
    }

    fn start(&mut self, cmd: ImportCmd, ctx: &egui::Context) {
        self.clear_cards();
        let root = cmd.out.parent().map(Path::to_path_buf).unwrap_or_default();
        let job = cmd
            .out
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let staging = match vi::create_staging(&root, &job) {
            Ok(d) => d,
            Err(e) => {
                self.error = Some(format!("Couldn't create {}: {e}", cmd.out.display()));
                return;
            }
        };
        let repaint = ctx.clone();
        match Job::spawn(cmd.command(), staging.clone(), move || {
            repaint.request_repaint()
        }) {
            Ok(j) => {
                self.log_line(format!("▶ import started: {}", file_name(&cmd.input)));
                self.job = Some(j);
                self.staging = Some(staging);
                self.stage = "starting".into();
                self.pct = 0.0;
                self.msg.clear();
                self.error = None;
            }
            Err(e) => {
                let _ = vi::remove_staging(&root, &staging);
                self.error = Some(format!(
                    "Couldn't start the import ({}): {e}",
                    cmd.python.display()
                ));
            }
        }
    }

    fn cancel(&mut self) {
        if let Some(j) = &mut self.job {
            j.cancel();
            self.stage = "cancelling".into();
            self.log_line("■ cancel sent");
        }
    }

    fn on_event(&mut self, e: Event) {
        match e {
            Event::Progress { stage, pct, msg } => {
                self.stage = stage;
                self.pct = pct;
                self.msg = msg;
            }
            Event::Candidate(c) => {
                let staging = self.staging.clone().unwrap_or_default();
                let mp4 = vi::resolve_in_staging(&staging, &c.mp4);
                let png = vi::resolve_in_staging(&staging, &c.preview_png);
                if mp4.is_none() || png.is_none() {
                    self.log_line(format!(
                        "⚠ candidate {} points outside staging or at a missing file; ignored",
                        c.id
                    ));
                    return;
                }
                let name = if c.suggested_name.trim().is_empty() {
                    c.id.clone()
                } else {
                    c.suggested_name.clone()
                };
                self.cards.push(Card {
                    c,
                    mp4,
                    png,
                    name,
                    state: CardState::Open,
                    tex: None,
                    tex_failed: false,
                    decoding: None,
                    frames: Vec::new(),
                    play_from: 0.0,
                    note: String::new(),
                });
            }
            Event::Done { candidates } => {
                self.log_line(format!("✓ pipeline done: {candidates} candidate(s)"));
            }
            Event::Error { msg } => {
                self.log_line(format!("✕ {msg}"));
                self.error = Some(msg);
            }
            Event::Unknown(l) | Event::Malformed(l) => self.log_line(l),
        }
    }

    fn on_exit(&mut self, o: Outcome) {
        self.job = None;
        match o {
            Outcome::Ok => {
                self.stage = "done".into();
                self.pct = 100.0;
                if self.cards.is_empty() {
                    if self.error.is_none() {
                        self.error = Some("The import finished but found no usable clip.".into());
                    }
                    self.drop_staging();
                }
            }
            Outcome::Cancelled => {
                self.log_line("■ import cancelled; staging removed");
                self.stage = "cancelled".into();
                self.clear_cards();
            }
            Outcome::Failed(m) => {
                self.log_line(format!("✕ {m}"));
                if self.error.is_none() {
                    self.error = Some(m);
                }
                self.stage = "failed".into();
                self.clear_cards();
            }
        }
    }

    fn drop_staging(&mut self) {
        if let (Some(root), Some(dir)) = (self.staging_root.clone(), self.staging.take()) {
            if let Err(e) = vi::remove_staging(&root, &dir) {
                self.log_line(format!("⚠ couldn't remove {}: {e}", dir.display()));
            }
        }
    }

    fn clear_cards(&mut self) {
        self.cards.clear();
        self.drop_staging();
    }

    /// Removes the staging folder once every card is decided.
    fn maybe_finish(&mut self) {
        if self.job.is_none()
            && !self.cards.is_empty()
            && self.cards.iter().all(|c| c.state != CardState::Open)
        {
            self.drop_staging();
        }
    }

    fn approve(&mut self, i: usize) {
        let (Some(dir), Some(card)) = (self.clips_dir.clone(), self.cards.get(i)) else {
            return;
        };
        let (Some(mp4), Some(png)) = (card.mp4.clone(), card.png.clone()) else {
            return;
        };
        let builtin = builtin_names();
        let r = user_clips::install_clip(
            &dir,
            &user_clips::NewClip {
                name: &card.name,
                mp4: &mp4,
                preview_png: &png,
                frames: card.c.frames,
                fps: card.c.fps,
            },
            &|n: &str| builtin.contains(n),
        );
        match r {
            Ok(name) => {
                self.log_line(format!("✓ added \"{name}\" to {}", dir.display()));
                let card = &mut self.cards[i];
                card.state = CardState::Approved(name);
                for f in [&mp4, &png] {
                    let _ = std::fs::remove_file(f);
                }
                self.maybe_finish();
            }
            Err(e) => {
                self.cards[i].note = format!("✕ {e}");
                self.log_line(format!("✕ couldn't add the clip: {e}"));
            }
        }
    }

    fn reject(&mut self, i: usize) {
        let Some(card) = self.cards.get_mut(i) else {
            return;
        };
        for f in [card.mp4.take(), card.png.take()].into_iter().flatten() {
            let _ = std::fs::remove_file(f);
        }
        card.state = CardState::Rejected;
        card.frames.clear();
        card.tex = None;
        self.maybe_finish();
    }

    fn play(&mut self, i: usize, ctx: &egui::Context) {
        let Some(card) = self.cards.get_mut(i) else {
            return;
        };
        let (Some(mp4), Some(png)) = (card.mp4.clone(), card.png.clone()) else {
            return;
        };
        let Some(ffmpeg) = self
            .tools_override
            .as_ref()
            .map(|t| t.ffmpeg.clone())
            .or_else(vi::find_ffmpeg)
        else {
            card.note = "✕ ffmpeg wasn't found, so the clip can't play here".into();
            return;
        };
        let Ok((w, h)) = image::image_dimensions(&png) else {
            card.note = "✕ unreadable preview".into();
            return;
        };
        let bytes = w as u64 * h as u64 * 4 * card.c.frames as u64;
        if bytes > MAX_PLAY_BYTES {
            card.note = format!(
                "too large to play here ({} MB); the preview image shows it",
                bytes / (1024 * 1024)
            );
            return;
        }
        card.note.clear();
        card.decoding = Some(Task::spawn(ctx, "import-play", move || {
            lillith_anim::video::decode_all(&ffmpeg, &mp4, w, h)
                .map_err(|e| e.to_string())
                .map(|frames| {
                    frames
                        .iter()
                        .map(|f| {
                            let img = lillith_anim::render::rgba_from_pixmap(f);
                            egui::ColorImage::from_rgba_unmultiplied(
                                [img.width() as usize, img.height() as usize],
                                img.as_raw(),
                            )
                        })
                        .collect()
                })
        }));
    }

    fn drain_job(&mut self) {
        let msgs = match &mut self.job {
            Some(j) => j.poll(),
            None => return,
        };
        for m in msgs {
            match m {
                JobMsg::Event(e) => self.on_event(e),
                JobMsg::Exited(o) => self.on_exit(o),
            }
        }
    }

    fn drop_zone(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let (hovering, dropped) = ui.ctx().input(|i| {
            (
                !i.raw.hovered_files.is_empty(),
                i.raw
                    .dropped_files
                    .iter()
                    .filter_map(|f| f.path.clone())
                    .collect::<Vec<_>>(),
            )
        });
        let idle = self.job.is_none();
        if idle {
            if let Some(p) = dropped.into_iter().next() {
                self.choose(p, shared);
            }
        }
        let stroke = if hovering && idle {
            pal::BLOOD_NEON
        } else {
            pal::IRON_EDGE
        };
        egui::Frame::new()
            .fill(c32(if hovering {
                pal::TERMINAL_FILL
            } else {
                pal::OBSIDIAN
            }))
            .stroke(egui::Stroke::new(
                if hovering { 2.0 } else { 1.0 },
                c32(stroke),
            ))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    let head = if hovering && idle {
                        "Drop it here".to_string()
                    } else if let Some(p) = &self.input {
                        format!("🎞 {}", file_name(p))
                    } else {
                        "Drag a video here  (.mp4 .gif .webm .mov, any size)".to_string()
                    };
                    ui.label(RichText::new(head).size(16.0).color(c32(pal::TEXT)));
                    if let Some(p) = &self.input {
                        let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                        chrome::hint(
                            ui,
                            &format!("{} · {:.1} MB", p.display(), size as f64 / 1_048_576.0),
                        );
                    }
                });
            });
    }

    fn progress(&mut self, ui: &mut egui::Ui) {
        let cancelling = self.job.as_ref().is_some_and(Job::cancelling);
        ui.add(
            egui::ProgressBar::new(self.pct / 100.0)
                .text(format!("{} · {:.0}%", self.stage, self.pct))
                .animate(self.job.is_some()),
        );
        if !self.msg.is_empty() {
            chrome::hint(ui, &self.msg);
        }
        if self.job.is_some()
            && chrome::blade_button(
                ui,
                if cancelling {
                    "Cancelling…"
                } else {
                    "■ Cancel"
                },
                !cancelling,
            )
            .clicked()
        {
            self.cancel();
        }
    }

    fn card_ui(&mut self, i: usize, ui: &mut egui::Ui) {
        let time = ui.input(|i| i.time);
        let ctx = ui.ctx().clone();
        let mut act: Option<&str> = None;
        let card = &mut self.cards[i];
        if let Some(t) = &mut card.decoding {
            if let Some(r) = t.poll() {
                card.decoding = None;
                match r {
                    Ok(imgs) => {
                        card.frames = imgs
                            .into_iter()
                            .enumerate()
                            .map(|(n, img)| {
                                ctx.load_texture(
                                    format!("import-{}-{n}", card.c.id),
                                    img,
                                    Default::default(),
                                )
                            })
                            .collect();
                        card.play_from = time;
                    }
                    Err(e) => card.note = format!("✕ {e}"),
                }
            }
        }
        if card.tex.is_none() && !card.tex_failed && card.state == CardState::Open {
            card.tex = card
                .png
                .as_ref()
                .and_then(|p| load_preview(&ctx, p, &card.c.id));
            card.tex_failed = card.tex.is_none();
        }
        egui::Frame::new()
            .fill(c32(pal::IRON))
            .stroke(egui::Stroke::new(1.0, c32(pal::IRON_HI)))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let tex = if card.frames.is_empty() {
                        card.tex.as_ref()
                    } else {
                        let n = ((time - card.play_from) * card.c.fps as f64) as usize
                            % card.frames.len();
                        ctx.request_repaint();
                        card.frames.get(n)
                    };
                    match tex {
                        Some(t) => {
                            let s = t.size_vec2();
                            let k = (160.0 / s.y.max(1.0)).min(200.0 / s.x.max(1.0));
                            ui.image((t.id(), s * k));
                        }
                        None => {
                            ui.label(RichText::new("no preview").color(c32(pal::TEXT_MUTE)));
                        }
                    }
                    ui.vertical(|ui| {
                        let (qa_text, qa_col) = if card.c.qa.pass {
                            ("QA pass", pal::OK)
                        } else {
                            ("QA flagged", pal::WARN)
                        };
                        ui.horizontal(|ui| {
                            chrome::chip(ui, qa_text, qa_col, true);
                            chrome::hint(
                                ui,
                                &format!(
                                    "{} frames @ {} fps · white edge {} · fringe {}",
                                    card.c.frames,
                                    card.c.fps,
                                    or_dash(&card.c.qa.white_edge),
                                    or_dash(&card.c.qa.fringe)
                                ),
                            );
                        });
                        match &card.state {
                            CardState::Open => {
                                ui.horizontal(|ui| {
                                    ui.label("Name");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut card.name)
                                            .desired_width(200.0),
                                    );
                                });
                                chrome::hint(
                                    ui,
                                    &format!(
                                        "saved as \"{}\" (a _2 suffix if taken)",
                                        user_clips::sanitize_name(&card.name)
                                    ),
                                );
                                ui.horizontal(|ui| {
                                    if card.frames.is_empty() && card.decoding.is_none() {
                                        if chrome::stone_button(ui, "▶ Play").clicked() {
                                            act = Some("play");
                                        }
                                    } else if card.decoding.is_some() {
                                        chrome::hint(ui, "decoding…");
                                    } else if chrome::stone_button(ui, "■ Stop").clicked() {
                                        card.frames.clear();
                                    }
                                    if chrome::blade_button(ui, "✓ Approve", true).clicked() {
                                        act = Some("approve");
                                    }
                                    if chrome::stone_button(ui, "✕ Reject").clicked() {
                                        act = Some("reject");
                                    }
                                });
                            }
                            CardState::Approved(n) => {
                                ui.label(
                                    RichText::new(format!("✓ added as \"{n}\""))
                                        .color(c32(pal::OK)),
                                );
                            }
                            CardState::Rejected => {
                                ui.label(
                                    RichText::new("✕ rejected (deleted)")
                                        .color(c32(pal::TEXT_MUTE)),
                                );
                            }
                        }
                        if !card.note.is_empty() {
                            chrome::hint(ui, &card.note);
                        }
                    });
                });
            });
        match act {
            Some("play") => self.play(i, &ctx),
            Some("approve") => self.propose_approve(i),
            Some("reject") => self.reject(i),
            _ => {}
        }
    }
}

impl Default for ImportTab {
    fn default() -> Self {
        Self::new()
    }
}

fn or_dash(s: &str) -> &str {
    if s.is_empty() {
        "–"
    } else {
        s
    }
}

/// Names in the built-in animation set (never replaced by a user clip).
fn builtin_names() -> std::collections::BTreeSet<String> {
    lillith_anim::atlas::default_manifest_path()
        .and_then(|p| lillith_anim::atlas::Manifest::load(&p).ok())
        .map(|m| m.clips.into_keys().collect())
        .unwrap_or_default()
}

fn load_preview(ctx: &egui::Context, p: &Path, id: &str) -> Option<egui::TextureHandle> {
    if std::fs::metadata(p).ok()?.len() > MAX_PREVIEW_BYTES {
        return None;
    }
    let img = image::open(p).ok()?.to_rgba8();
    let ci = egui::ColorImage::from_rgba_unmultiplied(
        [img.width() as usize, img.height() as usize],
        img.as_raw(),
    );
    Some(ctx.load_texture(format!("import-preview-{id}"), ci, Default::default()))
}

#[cfg(feature = "file-dialog")]
fn browse_video(ctx: &egui::Context) -> Option<Task<Option<PathBuf>>> {
    Some(Task::spawn(ctx, "browse-video", || {
        let mut d = rfd::FileDialog::new()
            .set_title("Pick a video to import")
            .add_filter("Videos", vi::ACCEPTED_EXT);
        if let Some(h) = lillith_setup::paths::home_dir() {
            d = d.set_directory(h);
        }
        d.pick_file()
    }))
}

#[cfg(not(feature = "file-dialog"))]
fn browse_video(_ctx: &egui::Context) -> Option<Task<Option<PathBuf>>> {
    None
}

impl SettingsTab for ImportTab {
    fn id(&self) -> &str {
        "import"
    }
    fn title(&self) -> &str {
        "Pose Builder"
    }
    fn rune(&self) -> usize {
        20
    }
    fn busy(&self) -> bool {
        self.job.is_some() || self.cards.iter().any(|c| c.decoding.is_some())
    }

    fn tick(&mut self, _ctx: &egui::Context, shared: &mut Shared) {
        self.drain_job();
        if let Some(t) = &mut self.browsing {
            if let Some(r) = t.poll() {
                self.browsing = None;
                if let Some(p) = r {
                    self.choose(p, shared);
                }
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        ui.label(
            RichText::new(
                "Turn your own videos into poses. The video stays where it is (only its path \
                 is used); the pipeline cuts her out into clips you approve one by one. \
                 Approved clips are added next to the built-in ones, never replacing any.",
            )
            .color(c32(pal::TEXT_DIM)),
        );
        ui.add_space(6.0);
        let ctx = ui.ctx().clone();
        let idle = self.job.is_none();

        chrome::section(ui, "IMPORT VIDEO", Some("Soul Contract gated"), |ui| {
            self.drop_zone(ui, shared);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let r = ui.add_enabled(
                    idle,
                    egui::TextEdit::singleline(&mut self.input_buf)
                        .hint_text("or paste a path to a video")
                        .desired_width(320.0),
                );
                if r.lost_focus() && !self.input_buf.trim().is_empty() {
                    let p = PathBuf::from(self.input_buf.trim());
                    if self.input.as_deref() != Some(p.as_path()) {
                        self.choose(p, shared);
                    }
                }
                if cfg!(feature = "file-dialog")
                    && idle
                    && self.browsing.is_none()
                    && chrome::stone_button(ui, "Browse…").clicked()
                {
                    self.browsing = browse_video(&ctx);
                }
                if chrome::blade_button(ui, "✦ Start import", idle && self.input.is_some())
                    .clicked()
                {
                    self.propose_start();
                }
            });
            if self.job.is_some() || !self.stage.is_empty() {
                ui.add_space(6.0);
                self.progress(ui);
            }
            if let Some(e) = &self.error {
                ui.add_space(4.0);
                ui.label(RichText::new(e).color(c32(pal::BAD)));
            }
        });

        if !self.cards.is_empty() {
            chrome::section(ui, "CLIPS", Some("approve to add"), |ui| {
                for i in 0..self.cards.len() {
                    self.card_ui(i, ui);
                    ui.add_space(6.0);
                }
            });
        }

        let choice = self.with_view(|v| soul_contract_popup(ui.ctx(), egui::Id::new(CARD_ID), v));
        match choice.flatten() {
            Some(ContractChoice::Yes) => self.confirm(ui.ctx()),
            Some(ContractChoice::No) => self.deny(),
            _ => {}
        }

        chrome::section(ui, "LOG", Some("local only"), |ui| {
            if self.log.is_empty() {
                chrome::hint(ui, "no output yet");
            } else {
                egui::ScrollArea::vertical()
                    .max_height(160.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        chrome::code_block(ui, &self.log.join("\n"));
                    });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::shared;
    use super::*;
    use std::time::{Duration, Instant};

    fn tmp(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("lillith-import-tab-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A tab pointed at `dir` (staging, clips, audit) with `script` as the
    /// pipeline.
    fn tab_in(dir: &Path, script: &Path) -> ImportTab {
        let mut t = ImportTab::new();
        t.audit = Some(dir.join("audit.log"));
        t.staging_root = Some(dir.join("import-staging"));
        t.clips_dir = Some(dir.join("user-clips"));
        t.tools_override = Some(Tools {
            python: vi::find_python().unwrap_or_else(|| "python3".into()),
            script: script.to_path_buf(),
            ffmpeg: "ffmpeg".into(),
            ffprobe: "ffprobe".into(),
        });
        t
    }

    fn frame(ctx: &egui::Context, tab: &mut ImportTab, s: &mut Shared, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 800.0),
            )),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            tab.tick(ctx, s);
            egui::CentralPanel::default().show(ctx, |ui| tab.ui(ui, s));
        });
    }

    fn pump(tab: &mut ImportTab, until: impl Fn(&ImportTab) -> bool) {
        let end = Instant::now() + Duration::from_secs(20);
        while !until(tab) {
            assert!(Instant::now() < end, "timed out; log: {:?}", tab.log);
            tab.drain_job();
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn only_videos_are_accepted() {
        let d = tmp("accept");
        let mut s = shared();
        let mut t = tab_in(&d, Path::new("/none"));
        std::fs::write(d.join("a.png"), b"x").unwrap();
        t.choose(d.join("a.png"), &mut s);
        assert!(t.input.is_none());
        t.choose(d.join("missing.mp4"), &mut s);
        assert!(t.input.is_none());
        std::fs::write(d.join("clip.MOV"), b"x").unwrap();
        t.choose(d.join("clip.MOV"), &mut s);
        assert_eq!(t.input, Some(d.join("clip.MOV")));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn start_needs_a_card_and_escape_denies_with_audit() {
        let d = tmp("deny");
        std::fs::write(d.join("v.mp4"), b"x").unwrap();
        let mut s = shared();
        let mut t = tab_in(&d, &d.join("user_import.py"));
        t.input = Some(d.join("v.mp4"));
        t.propose_start();
        assert!(matches!(t.pending, Some(Pending::Start { .. })));
        let ctx = egui::Context::default();
        lillith_theme::egui_theme::install(&ctx);
        for _ in 0..3 {
            frame(&ctx, &mut t, &mut s, vec![]);
        }
        assert!(ctx
            .read_response(egui::Id::new(CARD_ID).with("yes"))
            .is_some());
        assert!(t.job.is_none(), "nothing runs before Yes");
        // Enter never approves; Escape denies.
        let key = |k| egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&ctx, &mut t, &mut s, vec![key(egui::Key::Enter)]);
        assert!(t.job.is_none() && t.pending.is_some());
        frame(&ctx, &mut t, &mut s, vec![key(egui::Key::Escape)]);
        assert!(t.pending.is_none() && t.job.is_none());
        let log = std::fs::read_to_string(d.join("audit.log")).unwrap_or_default();
        assert!(
            log.contains("DENY") && log.contains("pose builder import v.mp4"),
            "{log}"
        );
        assert!(!log.contains("CONFIRM"), "{log}");
        assert!(
            !d.join("import-staging").exists()
                || std::fs::read_dir(d.join("import-staging"))
                    .unwrap()
                    .next()
                    .is_none()
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn missing_tools_show_a_clear_message() {
        let mut t = ImportTab::new();
        t.input = Some("/x.mp4".into());
        t.staging_root = Some(std::env::temp_dir());
        // With a real search this may or may not find things; force a miss.
        assert!(vi::missing_tools(None, None, None, None)
            .unwrap()
            .contains("Python"));
        t.tools_override = None;
        t.propose_start();
        assert!(t.pending.is_some() || t.error.is_some());
    }

    /// A stub pipeline honouring the contract (stands in for
    /// tools/video-pipeline/user_import.py).
    fn stub_script(dir: &Path, png: &Path, hang: bool) -> PathBuf {
        let p = dir.join("user_import.py");
        let body = format!(
            r#"import json, os, shutil, signal, sys, time
a = sys.argv
out = a[a.index("--out") + 1]
assert "--json-progress" in a and "--ffmpeg" in a and "--ffprobe" in a
signal.signal(signal.SIGTERM if os.name != "nt" else signal.SIGBREAK, lambda *_: sys.exit(130))
def say(**k): print(json.dumps(k), flush=True)
say(event="progress", stage="probe", pct=10, msg="probing")
print("a stray line", flush=True)
if {hang}:
    while True: time.sleep(0.05)
for n in ("wave", "idle"):
    os.makedirs(os.path.join(out, n), exist_ok=True)
    open(os.path.join(out, n, n + ".mp4"), "wb").write(b"mp4")
    shutil.copy({png:?}, os.path.join(out, n, n + ".png"))
    say(event="candidate", id=n, mp4=n + "/" + n + ".mp4", preview_png=n + "/" + n + ".png",
        frames=4, fps=12, qa={{"pass": True, "white_edge": 0.0, "fringe": 0.1}}, suggested_name=n)
say(event="candidate", id="evil", mp4="../../etc/passwd", preview_png="x.png", frames=1, fps=1, qa={{}})
say(event="done", candidates=2)
"#,
            hang = if hang { "True" } else { "False" },
            png = png.display().to_string(),
        );
        std::fs::write(&p, body).unwrap();
        p
    }

    fn start_now(t: &mut ImportTab) {
        t.propose_start();
        let ctx = egui::Context::default();
        t.confirm(&ctx);
        assert!(t.job.is_some(), "{:?}", t.error);
    }

    #[test]
    fn full_run_approve_and_reject() {
        if vi::find_python().is_none() {
            return;
        }
        let d = tmp("run");
        let png = d.join("p.png");
        image::RgbaImage::new(8, 16).save(&png).unwrap();
        std::fs::write(d.join("v.mp4"), b"x").unwrap();
        let script = stub_script(&d, &png, false);
        let mut t = tab_in(&d, &script);
        t.input = Some(d.join("v.mp4"));
        start_now(&mut t);
        let staging = t.staging.clone().unwrap();
        assert!(staging.starts_with(d.join("import-staging")));
        pump(&mut t, |t| t.job.is_none());
        assert_eq!(t.stage, "done");
        assert_eq!(
            t.cards.len(),
            2,
            "the outside-staging candidate is ignored: {:?}",
            t.log
        );
        assert!(t.log.iter().any(|l| l == "a stray line"));
        // Approve "wave" (renamed), reject "idle".
        t.cards[0].name = "My Wave".into();
        t.propose_approve(0);
        assert!(matches!(t.pending, Some(Pending::Approve { .. })));
        t.confirm(&egui::Context::default());
        assert_eq!(t.cards[0].state, CardState::Approved("my_wave".into()));
        let clips = d.join("user-clips");
        assert!(clips.join("my_wave/my_wave.mp4").is_file());
        let m = user_clips::load_user_manifest(&clips).unwrap();
        assert!(m.clips.contains_key("my_wave"));
        assert!(staging.exists(), "kept while a card is open");
        t.reject(1);
        assert_eq!(t.cards[1].state, CardState::Rejected);
        assert!(!staging.exists(), "all decided: staging removed");
        let log = std::fs::read_to_string(d.join("audit.log")).unwrap();
        assert_eq!(log.matches("CONFIRM").count(), 2, "{log}");
        // A second import of the same name is suffixed, never overwritten.
        start_now(&mut t);
        pump(&mut t, |t| t.job.is_none());
        t.cards[0].name = "my wave".into();
        t.propose_approve(0);
        t.confirm(&egui::Context::default());
        assert_eq!(t.cards[0].state, CardState::Approved("my_wave_2".into()));
        t.reject(1);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn cancel_stops_the_run_and_removes_staging() {
        if vi::find_python().is_none() {
            return;
        }
        let d = tmp("cancel");
        let png = d.join("p.png");
        image::RgbaImage::new(8, 16).save(&png).unwrap();
        std::fs::write(d.join("v.mp4"), b"x").unwrap();
        let script = stub_script(&d, &png, true);
        let mut t = tab_in(&d, &script);
        t.input = Some(d.join("v.mp4"));
        start_now(&mut t);
        let staging = t.staging.clone().unwrap();
        pump(&mut t, |t| t.stage == "probe");
        assert_eq!(t.pct, 10.0);
        t.cancel();
        pump(&mut t, |t| t.job.is_none());
        assert_eq!(t.stage, "cancelled");
        assert!(!staging.exists());
        assert!(t.cards.is_empty());
        let _ = std::fs::remove_dir_all(d);
    }
}
