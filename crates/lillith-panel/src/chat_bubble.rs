//! The small chat bubble that opens when you click Lillith.
//!
//! An undecorated, always-on-top, transparent `eframe` window (300×380)
//! placed beside her head, in the DemonOS hellforge look (near-black glass,
//! crimson frame, ember accents, a vector hellforge emblem in the corner
//! where a heart emoji used to be). It shows the conversation, has a
//! one-line input (Enter sends), streams her reply as it arrives, and closes
//! on `Esc` or as soon as it loses focus (a click anywhere outside it).
//!
//! The model call runs on a worker thread (its own tokio runtime) through
//! [`lillith_mind::chat::ChatSession`]. Every step is also reported through
//! [`BubbleOptions::emit`] (`chat.start`, `chat.token`, `chat.end`, …) so
//! the overlay can lip-flap the `talk_loop` clip in sync.
//!
//! Runs as its own process (`lillith chat-bubble`), because the overlay
//! already owns the process's windowing event loop.

use eframe::egui;
use lillith_core::config::{ChatConfig, MindConfig};
use lillith_core::soul_contract::PendingAction;
use lillith_mind::chat::{ChatSession, Outcome};
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
    /// `--no-llm`: answer with an in-character "switched off" line.
    pub offline: bool,
    /// History file (`None` = in-memory only).
    pub history: Option<PathBuf>,
    pub emit: Emit,
    /// Save the first settled frame to this PNG and exit (previews).
    pub screenshot: Option<PathBuf>,
    /// Show a canned sample conversation instead of the history (previews).
    pub demo: bool,
}

enum Req {
    Send(String),
    Clear,
    /// Answer the pending skill's Soul Contract: Yes (write) or No (discard).
    ConfirmSkill(bool),
}

enum Ui {
    Token(String),
    Done(Outcome),
    /// A system note to show in the bubble (e.g. a skill proposal/result).
    Note(String),
}

/// Parses the bubble's Soul Contract answer: `/skill yes` (or `y`) → `Some(true)`,
/// `/skill no` (or `n`) → `Some(false)`, anything else → `None`.
fn parse_skill_command(text: &str) -> Option<bool> {
    let mut parts = text.split_whitespace();
    if parts.next()? != "/skill" {
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
}

// lillith-theme's hellforge bubble look (DemonOS palette).
use lillith_theme::bubble::{self as look, BG, CRIMSON, EMBER, HER_TEXT, ME_TEXT, NEON};
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

/// A canned exchange for previews (`--demo`).
pub fn demo_lines() -> Vec<(bool, &'static str)> {
    vec![
        (
            false,
            "Mmh, there you are. Took you long enough to come and play with me~ 😈",
        ),
        (true, "missed you 🔥"),
        (
            false,
            "Of course you did. Now come closer and tell me everything ✨",
        ),
    ]
}

fn spawn_worker(opts: &BubbleOptions, ctx: egui::Context) -> (Sender<Req>, Receiver<Ui>) {
    let (req_tx, req_rx) = channel::<Req>();
    let (ui_tx, ui_rx) = channel::<Ui>();
    let mut session = ChatSession::new(&opts.chat, opts.offline).with_mind(&opts.mind);
    if let Some(p) = &opts.history {
        session = session.with_history_file(p.clone());
    }
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
            // The skill the model last proposed, awaiting a Soul Contract Yes/No.
            let mut pending: Option<PendingAction> = None;
            while let Ok(req) = req_rx.recv() {
                match req {
                    Req::Clear => session.clear(),
                    Req::Send(text) => {
                        emit("chat.start", None);
                        let mut streamed = false;
                        let out = rt.block_on(session.send(&text, |t| {
                            streamed = true;
                            emit("chat.token", Some(t));
                            let _ = ui_tx.send(Ui::Token(t.to_string()));
                            ctx.request_repaint();
                        }));
                        match &out {
                            Outcome::Reply(r) => emit("chat.end", Some(r)),
                            Outcome::Fallback { line, error } => {
                                // She still says her in-character line on
                                // the overlay.
                                if !streamed {
                                    emit("chat.token", Some(line));
                                }
                                emit("chat.error", Some(error));
                            }
                        }
                        let _ = ui_tx.send(Ui::Done(out));
                        // Did she propose a skill? Hold it for the gate; never
                        // write it on our own.
                        if let Some(action) = session.pending_skill_action() {
                            let summary = action.summary();
                            pending = Some(action);
                            emit("chat.skill_proposed", Some(&summary));
                            let _ = ui_tx.send(Ui::Note(format!(
                                "📜 Soul Contract: she wants to {summary}. Type /skill yes to save, /skill no to discard."
                            )));
                        }
                        ctx.request_repaint();
                    }
                    Req::ConfirmSkill(yes) => {
                        let Some(action) = pending.take() else {
                            let _ = ui_tx.send(Ui::Note(
                                "No skill is waiting for a yes/no right now.".into(),
                            ));
                            ctx.request_repaint();
                            continue;
                        };
                        let mut sc = session.soul_contract();
                        let id = sc.request(action);
                        let note = if yes {
                            match sc.confirm(id) {
                                Ok(done) => {
                                    let where_ = done
                                        .path
                                        .as_ref()
                                        .map(|p| p.display().to_string())
                                        .unwrap_or_default();
                                    emit("chat.skill_saved", Some(&where_));
                                    format!("✅ Saved to {where_}")
                                }
                                Err(e) => {
                                    emit("chat.skill_error", Some(&e.to_string()));
                                    format!("Couldn't save that skill: {e}")
                                }
                            }
                        } else {
                            let _ = sc.deny(id);
                            emit("chat.skill_denied", None);
                            "🚫 Discarded; nothing was written.".into()
                        };
                        let _ = ui_tx.send(Ui::Note(note));
                        ctx.request_repaint();
                    }
                }
            }
        })
        .expect("spawn chat worker");
    (req_tx, ui_rx)
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
        .map(|m| Line {
            me: m.role == "user",
            text: m.content,
        })
        .collect()
}

fn serde_json_lines(s: &str) -> Option<Vec<Message>> {
    serde_json::from_str(s).ok()
}

impl Bubble {
    fn send(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() || self.streaming.is_some() {
            return;
        }
        if text == "/clear" {
            self.lines.clear();
            let _ = self.tx.send(Req::Clear);
        } else if let Some(yes) = parse_skill_command(&text) {
            // Soul Contract answer: nothing is written unless this is a yes.
            let _ = self.tx.send(Req::ConfirmSkill(yes));
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
            }
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
        // Esc, or a click outside (focus lost) closes the bubble.
        let (esc, focused) =
            ctx.input(|i| (i.key_pressed(egui::Key::Escape), i.viewport().focused));
        if focused == Some(true) {
            self.had_focus = true;
        }
        let lost = self.had_focus && focused == Some(false) && self.opened.elapsed() > FOCUS_GRACE;
        if self.screenshot.is_none() && (esc || lost) {
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
                                bubble_line(
                                    ui,
                                    false,
                                    "Hey you~ finally clicked me. What's on your mind, cutie? 😈",
                                );
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
                    let resp = ui.add_enabled(self.streaming.is_none(), edit);
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        self.send();
                        resp.request_focus();
                    } else if !resp.has_focus() && self.streaming.is_none() && !self.closing {
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
            Ok(Box::new(Bubble {
                lines,
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
            }))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_command_parses_only_explicit_answers() {
        assert_eq!(parse_skill_command("/skill yes"), Some(true));
        assert_eq!(parse_skill_command("/skill Y"), Some(true));
        assert_eq!(parse_skill_command("  /skill   no "), Some(false));
        assert_eq!(parse_skill_command("/skill n"), Some(false));
        // Anything ambiguous is not an answer (and goes to chat instead).
        assert_eq!(parse_skill_command("/skill"), None);
        assert_eq!(parse_skill_command("/skill maybe"), None);
        assert_eq!(parse_skill_command("/skill yes please"), None);
        assert_eq!(parse_skill_command("skill yes"), None);
        assert_eq!(parse_skill_command("yes"), None);
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
}
