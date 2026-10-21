//! The small chat bubble that opens when you click Lillith.
//!
//! An undecorated, always-on-top, transparent `eframe` window (300×380)
//! placed beside her head. It shows the conversation as cute bubbles, has a
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
use lillith_core::config::ChatConfig;
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
    /// `--no-llm`: answer with an in-character "switched off" line.
    pub offline: bool,
    /// History file (`None` = in-memory only).
    pub history: Option<PathBuf>,
    pub emit: Emit,
}

enum Req {
    Send(String),
    Clear,
}

enum Ui {
    Token(String),
    Done(Outcome),
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
}

// Palette (canonical Lillith: pink, magenta, violet, cyan neon on black):
// lillith-theme's bubble look.
use lillith_theme::bubble::{
    BG, CYAN, HER_TEXT, PINK, PINK_SOFT as HER_FILL, PLUM, VIOLET as ME_FILL,
};

fn spawn_worker(opts: &BubbleOptions, ctx: egui::Context) -> (Sender<Req>, Receiver<Ui>) {
    let (req_tx, req_rx) = channel::<Req>();
    let (ui_tx, ui_rx) = channel::<Ui>();
    let mut session = ChatSession::new(&opts.chat, opts.offline);
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
        let (fill, color) = if me {
            (ME_FILL, egui::Color32::WHITE)
        } else {
            (HER_FILL, HER_TEXT)
        };
        egui::Frame::new()
            .fill(fill)
            .corner_radius(egui::CornerRadius {
                nw: 12,
                ne: 12,
                sw: if me { 12 } else { 3 },
                se: if me { 3 } else { 12 },
            })
            .inner_margin(egui::Margin::symmetric(9, 6))
            .show(ui, |ui| {
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
        if esc || (self.had_focus && focused == Some(false) && self.opened.elapsed() > FOCUS_GRACE)
        {
            self.close(ctx);
        }

        let frame = egui::Frame::new()
            .fill(BG)
            .stroke(egui::Stroke::new(2.0, PINK))
            .corner_radius(egui::CornerRadius::same(18))
            .inner_margin(egui::Margin::same(10));
        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(ctx, |ui| {
                frame.show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    // Header: drag the bubble by it.
                    let header = ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("💜 Lillith")
                                .color(PINK)
                                .strong()
                                .size(16.0),
                        );
                        let status = if self.streaming.is_some() {
                            "typing…"
                        } else {
                            "online~"
                        };
                        ui.label(egui::RichText::new(status).color(CYAN).small());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("×").size(16.0).color(PINK),
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
                    ui.add_space(4.0);
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
                        .hint_text("Say something cute…")
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
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        (self.emit)("chat.closed", None);
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
            let mut style = (*cc.egui_ctx.style()).clone();
            style.visuals = egui::Visuals::dark();
            style.visuals.extreme_bg_color = PLUM;
            style.visuals.selection.bg_fill = PINK;
            style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(10);
            style.visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(10);
            style.visuals.widgets.active.corner_radius = egui::CornerRadius::same(10);
            cc.egui_ctx.set_style(style);
            cc.egui_ctx.set_fonts(bubble_fonts());
            let (tx, rx) = spawn_worker(&opts, cc.egui_ctx.clone());
            (opts.emit)("chat.opened", None);
            Ok(Box::new(Bubble {
                lines: history_lines(&opts),
                streaming: None,
                input: String::new(),
                tx,
                rx,
                opened: Instant::now(),
                had_focus: false,
                closing: false,
                asked_focus: false,
                emit: opts.emit.clone(),
            }))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
