//! Settings window (`lillith settings`, own process): live-edits
//! `config.toml` and tells the overlay (`settings.changed`) to re-apply it.
//!
//! Sections: **Animation** (playback / talk speed, smooth frames, avatar
//! pace), **Jiggle** (baked / engine springs / off, spring gains, flirty
//! preset), **Chat** (endpoint, model, temperature, system prompt, overlay
//! speech), **Window** (always-on-top, click-through, autostart) and
//! **Avatar** (Lillith or a custom image: local import with background
//! removal, anchor editor with draggable points, live animated preview).
//!
//! Changes are saved ~0.3 s after the last edit (atomic write, unknown
//! keys kept) and announced on stdout; the chat settings apply from the
//! next bubble.

use crate::chat_bubble::Emit;
use eframe::egui;
use lillith_anim::avatar::{import_image, AvatarMeta, AvatarStore, ImportOptions, CANVAS};
use lillith_anim::avatar_edit::{self, Preview};
use lillith_core::config::{autostart_dir, set_autostart, Config, JiggleMode, Springs};
use lillith_theme::bubble::{self as theme, CYAN, PINK, PINK_SOFT};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

/// Save this long after the last edit.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(300);
/// Settings window size.
pub const SETTINGS_SIZE: (f32, f32) = (680.0, 560.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Animation,
    Jiggle,
    Chat,
    Window,
    Avatar,
}

impl Section {
    pub const ALL: [Section; 5] = [
        Section::Animation,
        Section::Jiggle,
        Section::Chat,
        Section::Window,
        Section::Avatar,
    ];

    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "jiggle" => Section::Jiggle,
            "chat" => Section::Chat,
            "window" => Section::Window,
            "avatar" => Section::Avatar,
            _ => Section::Animation,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Section::Animation => "🎞  Animation",
            Section::Jiggle => "💗  Jiggle",
            Section::Chat => "💬  Chat",
            Section::Window => "🪟  Window",
            Section::Avatar => "🖼  Avatar",
        }
    }
}

/// Everything the window needs.
#[derive(Clone)]
pub struct SettingsOptions {
    pub config_path: PathBuf,
    pub section: Option<String>,
    pub avatars: Option<AvatarStore>,
    /// Pre-fill the import path (and import it right away): tests and
    /// screenshots.
    pub import: Option<PathBuf>,
    pub emit: Emit,
}

/// The anchor editor for an imported (or stored) avatar.
pub struct Editor {
    pub img: image::RgbaImage,
    pub meta: AvatarMeta,
    /// Not saved yet (fresh import).
    pub new: bool,
    tex: Option<egui::TextureHandle>,
    dragging: Option<&'static str>,
    preview: Preview,
    preview_tex: Option<egui::TextureHandle>,
    last: Instant,
}

impl Editor {
    pub fn new(img: image::RgbaImage, meta: AvatarMeta, new: bool) -> Self {
        let preview = Preview::new(&img, &meta);
        Self {
            img,
            meta,
            new,
            tex: None,
            dragging: None,
            preview,
            preview_tex: None,
            last: Instant::now(),
        }
    }
}

pub struct Settings {
    pub cfg: Config,
    path: PathBuf,
    pub section: Section,
    dirty_at: Option<Instant>,
    emit: Emit,
    status: String,
    store: Option<AvatarStore>,
    avatars: Vec<AvatarMeta>,
    selected: Option<String>,
    import_path: String,
    already_transparent: bool,
    tolerance: f32,
    pub editor: Option<Editor>,
    picker: Option<Receiver<Option<String>>>,
    thumbs: std::collections::HashMap<String, egui::TextureHandle>,
}

impl Settings {
    pub fn new(opts: SettingsOptions) -> Self {
        let cfg = if opts.config_path.exists() {
            Config::load(&opts.config_path).unwrap_or_default()
        } else {
            Config::default()
        };
        let mut s = Self {
            cfg,
            path: opts.config_path.clone(),
            section: opts
                .section
                .as_deref()
                .map_or(Section::Animation, Section::parse),
            dirty_at: None,
            emit: opts.emit.clone(),
            status: String::new(),
            store: opts.avatars.clone(),
            avatars: Vec::new(),
            selected: None,
            import_path: String::new(),
            already_transparent: false,
            tolerance: ImportOptions::default().tolerance,
            editor: None,
            picker: None,
            thumbs: Default::default(),
        };
        s.refresh_avatars();
        if let Some(p) = opts.import {
            s.import_path = p.display().to_string();
            s.import();
        }
        s
    }

    fn refresh_avatars(&mut self) {
        if let Some(st) = &self.store {
            self.avatars = st.list();
            self.selected = st.selection().selected;
        }
    }

    fn changed(&mut self) {
        self.dirty_at = Some(Instant::now());
    }

    /// Saves now and tells the overlay.
    pub fn save(&mut self) {
        self.dirty_at = None;
        match self.cfg.save(&self.path) {
            Ok(()) => {
                self.status = format!("saved ✓  {}", self.path.display());
                (self.emit)("settings.changed", None);
            }
            Err(e) => self.status = format!("could not save: {e}"),
        }
    }

    fn import(&mut self) {
        let path = PathBuf::from(self.import_path.trim());
        let opts = ImportOptions {
            already_transparent: self.already_transparent,
            tolerance: self.tolerance,
        };
        match import_image(&path, opts) {
            Ok((img, meta)) => {
                self.status = avatar_edit::import_summary(&meta);
                self.editor = Some(Editor::new(img, meta, true));
            }
            Err(e) => self.status = format!("import failed: {e}"),
        }
    }

    fn select_avatar(&mut self, id: Option<String>) {
        if let Some(st) = &self.store {
            if let Err(e) = st.select(id.as_deref()) {
                self.status = format!("could not save the choice: {e}");
                return;
            }
        }
        self.selected = id.clone();
        (self.emit)("avatar.selected", Some(id.as_deref().unwrap_or("lillith")));
        self.status = format!("avatar: {}", id.as_deref().unwrap_or("Lillith"));
    }

    fn save_editor(&mut self) {
        let (Some(st), Some(ed)) = (self.store.clone(), self.editor.as_mut()) else {
            return;
        };
        match avatar_edit::commit(&st, &ed.img, &mut ed.meta, &mut ed.new) {
            Ok(id) => {
                self.refresh_avatars();
                self.select_avatar(Some(id));
            }
            Err(e) => self.status = format!("could not save the avatar: {e}"),
        }
    }

    fn browse(&mut self) {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let tries: [(&str, &[&str]); 2] = [
                (
                    "zenity",
                    &[
                        "--file-selection",
                        "--title=Pick an avatar image",
                        "--file-filter=Images | *.png *.jpg *.jpeg *.webp",
                    ],
                ),
                (
                    "kdialog",
                    &["--getopenfilename", ".", "*.png *.jpg *.jpeg *.webp"],
                ),
            ];
            for (bin, args) in tries {
                let mut c = std::process::Command::new(bin);
                c.args(args);
                lillith_core::launch_audit::record_command("avatar_picker", &c);
                if let Ok(out) = c.output() {
                    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    let _ = tx.send((!s.is_empty()).then_some(s));
                    return;
                }
            }
            let _ = tx.send(None);
        });
        self.picker = Some(rx);
        self.status = "choose an image… (no picker? type the path)".into();
    }

    fn animation(&mut self, ui: &mut egui::Ui) {
        theme::heading(ui, "Pace");
        let mut ch = false;
        let a = &mut self.cfg.anim;
        ch |= ui
            .add(egui::Slider::new(&mut a.playback_speed, 0.3..=1.5).text("playback speed"))
            .on_hover_text("Clip frame timing (1 = as drawn; lower = slower)")
            .changed();
        ch |= ui
            .add(egui::Slider::new(&mut a.talk_speed, 0.3..=1.5).text("talk speed"))
            .on_hover_text("Lip-flap pace while she talks")
            .changed();
        ui.add_space(6.0);
        theme::heading(ui, "Smoothness");
        ch |= ui
            .checkbox(
                &mut a.smooth_frames,
                "Smooth frames (cross-fade between sprite frames)",
            )
            .changed();
        ui.add_enabled_ui(a.smooth_frames, |ui| {
            ch |= ui
                .add(
                    egui::Slider::new(&mut a.crossfade, 0.1..=1.0)
                        .text("cross-fade part of each frame"),
                )
                .changed();
        });
        ch |= ui
            .add(egui::Slider::new(&mut self.cfg.avatar.pace, 0.3..=1.5).text("custom avatar pace"))
            .changed();
        ch |= ui
            .add(
                egui::Slider::new(&mut self.cfg.avatar.render_hz, 30.0..=120.0)
                    .text("custom avatar redraw Hz"),
            )
            .changed();
        if ch {
            self.changed();
        }
    }

    fn jiggle(&mut self, ui: &mut egui::Ui) {
        theme::heading(ui, "Jiggle mode");
        let mut ch = false;
        let mut mode = self.cfg.jiggle();
        ui.horizontal(|ui| {
            for m in JiggleMode::ALL {
                ch |= ui.selectable_value(&mut mode, m, m.label()).changed();
            }
        });
        if ch {
            self.cfg.jiggle_mode = Some(mode);
            self.cfg.baked_jiggle = mode == JiggleMode::Baked;
        }
        ui.label(
            egui::RichText::new(match mode {
                JiggleMode::Baked => {
                    "Drawn jiggle art; engine chest/pigtail springs pause on those clips."
                }
                JiggleMode::Springs => "Clean art with the engine's springs.",
                JiggleMode::Off => "Clean art, chest and pigtails stay still.",
            })
            .color(CYAN)
            .small(),
        );
        ui.add_space(6.0);
        theme::heading(ui, "Spring gains");
        let s = &mut self.cfg.springs;
        ch |= ui
            .add(egui::Slider::new(&mut s.chest_bounce_gain, 0.0..=2.0).text("chest bounce"))
            .changed();
        ch |= ui
            .add(egui::Slider::new(&mut s.pigtail_sway_gain, 0.0..=2.0).text("pigtail sway"))
            .changed();
        ch |= ui
            .add(egui::Slider::new(&mut s.jiggle_gain, 0.0..=2.0).text("jiggle"))
            .changed();
        ui.horizontal(|ui| {
            if ui
                .button("💋 Flirty preset")
                .on_hover_text("Softer chest/pigtail damping, a bit more sway")
                .clicked()
            {
                self.cfg.springs = Springs::flirty();
                ch = true;
            }
            if ui.button("Default").clicked() {
                self.cfg.springs = Springs::default();
                ch = true;
            }
        });
        if ch {
            self.changed();
        }
    }

    fn chat(&mut self, ui: &mut egui::Ui) {
        theme::heading(ui, "Local model");
        let mut ch = false;
        let c = &mut self.cfg.chat;
        egui::Grid::new("chat")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("endpoint");
                ch |= ui
                    .add(egui::TextEdit::singleline(&mut c.endpoint).desired_width(320.0))
                    .changed();
                ui.end_row();
                ui.label("model");
                ch |= ui
                    .add(egui::TextEdit::singleline(&mut c.model).desired_width(320.0))
                    .changed();
                ui.end_row();
                ui.label("temperature");
                ch |= ui
                    .add(egui::Slider::new(&mut c.temperature, 0.0..=1.5))
                    .changed();
                ui.end_row();
            });
        ui.label("system prompt (empty = her built-in persona)");
        ch |= ui
            .add(
                egui::TextEdit::multiline(&mut c.system_prompt)
                    .desired_rows(4)
                    .desired_width(f32::INFINITY),
            )
            .changed();
        ch |= ui
            .checkbox(&mut c.overlay_speech, "Speech bubble over her head")
            .changed();
        ch |= ui
            .checkbox(&mut c.open_on_click, "A click on her opens the chat")
            .changed();
        ui.label(
            egui::RichText::new("Chat changes apply from the next bubble.")
                .color(CYAN)
                .small(),
        );
        if ch {
            self.changed();
        }
    }

    fn window(&mut self, ui: &mut egui::Ui) {
        theme::heading(ui, "Window");
        let mut ch = false;
        ch |= ui
            .checkbox(&mut self.cfg.always_on_top, "Always on top")
            .changed();
        ch |= ui
            .checkbox(
                &mut self.cfg.click_through,
                "Click-through (only her head stays clickable)",
            )
            .changed();
        let was = self.cfg.autostart;
        ch |= ui
            .checkbox(&mut self.cfg.autostart, "Start with the desktop session")
            .changed();
        if was != self.cfg.autostart {
            let r = match (autostart_dir(), std::env::current_exe()) {
                (Some(d), Ok(exe)) => {
                    set_autostart(&d, &exe, self.cfg.autostart).map_err(|e| e.to_string())
                }
                _ => Err("no autostart directory".into()),
            };
            if let Err(e) = r {
                self.status = format!("autostart: {e}");
            }
        }
        if ch {
            self.changed();
        }
    }

    fn thumb(&mut self, ctx: &egui::Context, m: &AvatarMeta) -> Option<egui::TextureHandle> {
        if let Some(t) = self.thumbs.get(&m.id) {
            return Some(t.clone());
        }
        let st = self.store.as_ref()?;
        let path = avatar_edit::image_path(st, m);
        let img = image::open(path).ok()?.thumbnail(64, 64).to_rgba8();
        let ci = egui::ColorImage::from_rgba_unmultiplied(
            [img.width() as usize, img.height() as usize],
            img.as_raw(),
        );
        let t = ctx.load_texture(format!("thumb-{}", m.id), ci, egui::TextureOptions::LINEAR);
        self.thumbs.insert(m.id.clone(), t.clone());
        Some(t)
    }

    fn avatar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if let Some(rx) = &self.picker {
            if let Ok(r) = rx.try_recv() {
                self.picker = None;
                if let Some(p) = r {
                    self.import_path = p;
                    self.import();
                }
            }
        }
        if self.editor.is_some() {
            self.anchor_editor(ui, ctx);
            return;
        }
        theme::heading(ui, "Who's on your desktop?");
        let mut pick: Option<Option<String>> = None;
        let mut edit: Option<String> = None;
        ui.horizontal_wrapped(|ui| {
            let on = self.selected.is_none();
            if ui
                .add(
                    egui::Button::new(egui::RichText::new("💜\nLillith").size(15.0))
                        .selected(on)
                        .min_size(egui::vec2(96.0, 84.0)),
                )
                .on_hover_text("Her full animated sprite set (default)")
                .clicked()
            {
                pick = Some(None);
            }
            for m in self.avatars.clone() {
                let on = self.selected.as_deref() == Some(m.id.as_str());
                ui.vertical(|ui| {
                    let r = match self.thumb(ctx, &m) {
                        Some(t) => ui.add(
                            egui::Button::image_and_text(
                                egui::Image::new(&t).fit_to_exact_size(egui::vec2(56.0, 56.0)),
                                m.name.clone(),
                            )
                            .selected(on)
                            .min_size(egui::vec2(96.0, 84.0)),
                        ),
                        None => ui.add(
                            egui::Button::new(&m.name)
                                .selected(on)
                                .min_size(egui::vec2(96.0, 84.0)),
                        ),
                    };
                    if r.clicked() {
                        pick = Some(Some(m.id.clone()));
                    }
                    if ui.small_button("✏ anchors").clicked() {
                        edit = Some(m.id.clone());
                    }
                });
            }
        });
        if let Some(p) = pick {
            self.select_avatar(p);
        }
        if let Some(id) = edit {
            if let Some(st) = &self.store {
                match st.load(&id) {
                    Ok(av) => match image::open(&av.path) {
                        Ok(img) => self.editor = Some(Editor::new(img.to_rgba8(), av.meta, false)),
                        Err(e) => self.status = format!("{e}"),
                    },
                    Err(e) => self.status = format!("{e}"),
                }
            }
        }
        ui.add_space(8.0);
        theme::heading(ui, "Custom image…");
        ui.label(
            egui::RichText::new("PNG, JPG or WebP. Everything stays on this computer: a flat background is removed, the image is fitted to her 400×400 stage and animated by the 2.5D engine.")
                .color(PINK_SOFT)
                .small(),
        );
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.import_path)
                    .hint_text("/path/to/image.png")
                    .desired_width(330.0),
            );
            if ui.button("Browse…").clicked() {
                self.browse();
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.already_transparent, "Already transparent");
            ui.add_enabled(
                !self.already_transparent,
                egui::Slider::new(&mut self.tolerance, 8.0..=120.0).text("background tolerance"),
            );
        });
        if ui
            .add_enabled(
                !self.import_path.trim().is_empty(),
                egui::Button::new(egui::RichText::new("✨ Import").strong()).fill(PINK),
            )
            .clicked()
        {
            self.import();
        }
    }

    fn anchor_editor(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(ed) = self.editor.as_mut() else {
            return;
        };
        theme::heading(ui, &format!("Anchors · {}", ed.meta.name));
        ui.label(
            egui::RichText::new("Drag the dots onto her head, mouth, chest, hip and hair tips.")
                .color(CYAN)
                .small(),
        );
        let mut save = false;
        let mut cancel = false;
        ui.horizontal_top(|ui| {
            // Image with draggable anchor points (300 px = the 400 px canvas).
            let side = 300.0;
            let k = side / CANVAS as f32;
            let tex = ed.tex.get_or_insert_with(|| {
                let ci = egui::ColorImage::from_rgba_unmultiplied(
                    [ed.img.width() as usize, ed.img.height() as usize],
                    ed.img.as_raw(),
                );
                ctx.load_texture("avatar-edit", ci, egui::TextureOptions::LINEAR)
            });
            let (resp, painter) =
                ui.allocate_painter(egui::vec2(side, side), egui::Sense::click_and_drag());
            let r = resp.rect;
            painter.rect_filled(r, 8.0, egui::Color32::from_rgb(52, 30, 66));
            // Checker so transparency is visible.
            let cell = 15.0;
            for y in 0..(side / cell) as i32 {
                for x in 0..(side / cell) as i32 {
                    if (x + y) % 2 == 0 {
                        painter.rect_filled(
                            egui::Rect::from_min_size(
                                r.min + egui::vec2(x as f32 * cell, y as f32 * cell),
                                egui::vec2(cell, cell),
                            ),
                            0.0,
                            egui::Color32::from_rgb(62, 38, 78),
                        );
                    }
                }
            }
            painter.image(
                tex.id(),
                r,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            let base_y = r.min.y + lillith_anim::avatar::BASELINE as f32 * k;
            painter.line_segment(
                [egui::pos2(r.min.x, base_y), egui::pos2(r.max.x, base_y)],
                egui::Stroke::new(1.0, CYAN),
            );
            let colors =
                avatar_edit::ANCHOR_COLORS.map(|[r, g, b]| egui::Color32::from_rgb(r, g, b));
            let to_screen = |p: (f32, f32)| r.min + egui::vec2(p.0 * k, p.1 * k);
            let to_canvas = |p: egui::Pos2| ((p.x - r.min.x) / k, (p.y - r.min.y) / k);
            if resp.drag_started() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    ed.dragging = avatar_edit::nearest_anchor(&ed.meta, to_canvas(pos), 14.0 / k);
                }
            }
            if resp.dragged() {
                if let (Some(n), Some(pos)) = (ed.dragging, resp.interact_pointer_pos()) {
                    avatar_edit::move_anchor(&mut ed.meta, n, to_canvas(pos));
                }
            }
            if resp.drag_stopped() {
                ed.dragging = None;
            }
            for ((name, p), c) in ed.meta.anchors.named().iter().zip(colors) {
                let s = to_screen(*p);
                painter.circle(s, 6.0, c, egui::Stroke::new(2.0, egui::Color32::WHITE));
                painter.text(
                    s + egui::vec2(9.0, -8.0),
                    egui::Align2::LEFT_TOP,
                    name.replace('_', " "),
                    egui::FontId::proportional(11.0),
                    egui::Color32::WHITE,
                );
            }
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("live preview").color(PINK_SOFT).small());
                // Animated preview (continuous, same engine as the overlay).
                let now = Instant::now();
                let dt = (now - ed.last).as_secs_f32().min(0.1);
                ed.last = now;
                if let Some(c) = ed
                    .preview
                    .render(&ed.meta, dt, START.elapsed().as_secs_f32(), 200)
                {
                    let ci = egui::ColorImage::from_rgba_premultiplied([200, 200], c.data());
                    match &mut ed.preview_tex {
                        Some(t) => t.set(ci, egui::TextureOptions::LINEAR),
                        None => {
                            ed.preview_tex = Some(ctx.load_texture(
                                "avatar-preview",
                                ci,
                                egui::TextureOptions::LINEAR,
                            ))
                        }
                    }
                }
                if let Some(t) = &ed.preview_tex {
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(52, 30, 66))
                        .corner_radius(8)
                        .show(ui, |ui| {
                            ui.add(egui::Image::new(t).fit_to_exact_size(egui::vec2(200.0, 200.0)));
                        });
                }
                ui.checkbox(&mut ed.meta.character, "Character (jiggle + talk bob)");
                ui.add_enabled(
                    ed.meta.character,
                    egui::Slider::new(&mut ed.meta.jiggle_strength, 0.0..=1.0).text("jiggle"),
                );
                ui.horizontal(|ui| {
                    ui.label("name");
                    ui.add(egui::TextEdit::singleline(&mut ed.meta.name).desired_width(120.0));
                });
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(egui::RichText::new("💾 Save & use").strong())
                                .fill(PINK),
                        )
                        .clicked()
                    {
                        save = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
                if ui.button("Re-guess anchors").clicked() {
                    avatar_edit::reguess_anchors(&ed.img, &mut ed.meta);
                }
            });
        });
        ctx.request_repaint();
        if save {
            self.save_editor();
            self.editor = None;
        } else if cancel {
            self.editor = None;
        }
    }
}

static START: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);

impl eframe::App for Settings {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.dirty_at.is_some_and(|t| t.elapsed() >= SAVE_DEBOUNCE) {
            self.save();
        }
        if self.dirty_at.is_some() || self.picker.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        egui::TopBottomPanel::top("title").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("💜 Lillith · Settings")
                        .color(PINK)
                        .strong()
                        .size(18.0),
                );
                ui.label(
                    egui::RichText::new("changes apply live~")
                        .color(CYAN)
                        .small(),
                );
            });
            ui.add_space(4.0);
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.label(egui::RichText::new(&self.status).color(PINK_SOFT).small());
        });
        egui::SidePanel::left("sections")
            .resizable(false)
            .exact_width(130.0)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                for s in Section::ALL {
                    let on = self.section == s;
                    if ui
                        .add_sized([118.0, 30.0], egui::Button::new(s.label()).selected(on))
                        .clicked()
                    {
                        self.section = s;
                    }
                }
            });
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| match self.section {
                Section::Animation => self.animation(ui),
                Section::Jiggle => self.jiggle(ui),
                Section::Chat => self.chat(ui),
                Section::Window => self.window(ui),
                Section::Avatar => self.avatar(ui, ctx),
            });
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.dirty_at.is_some() {
            self.save();
        }
        (self.emit)("settings.closed", None);
    }
}

/// Opens Settings (blocks until closed).
pub fn run_settings(opts: SettingsOptions) -> eframe::Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_title("Lillith · Settings")
        .with_app_id("lillith-settings")
        .with_inner_size([SETTINGS_SIZE.0, SETTINGS_SIZE.1])
        .with_min_inner_size([480.0, 400.0]);
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "lillith-settings",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            cc.egui_ctx.set_fonts(crate::chat_bubble::bubble_fonts());
            Ok(Box::new(Settings::new(opts)))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn edits_save_and_notify() {
        let dir = std::env::temp_dir().join(format!("lillith-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        let l = log.clone();
        let emit: Emit = Arc::new(move |n, _| l.lock().unwrap().push(n.to_string()));
        let mut s = Settings::new(SettingsOptions {
            config_path: path.clone(),
            section: Some("jiggle".into()),
            avatars: Some(AvatarStore::new(&dir)),
            import: None,
            emit,
        });
        assert_eq!(s.section, Section::Jiggle);
        s.cfg.anim.playback_speed = 0.55;
        s.cfg.springs = Springs::flirty();
        s.cfg.jiggle_mode = Some(JiggleMode::Baked);
        s.save();
        let back = Config::load(&path).unwrap();
        assert!((back.anim.playback_speed - 0.55).abs() < 1e-6);
        assert_eq!(back.jiggle(), JiggleMode::Baked);
        assert_eq!(log.lock().unwrap().as_slice(), ["settings.changed"]);
        // Every section renders.
        let ctx = egui::Context::default();
        for sec in Section::ALL {
            s.section = sec;
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| match s.section {
                    Section::Animation => s.animation(ui),
                    Section::Jiggle => s.jiggle(ui),
                    Section::Chat => s.chat(ui),
                    Section::Window => s.window(ui),
                    Section::Avatar => s.avatar(ui, ctx),
                });
            });
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn import_edit_save_selects_avatar() {
        let dir = std::env::temp_dir().join(format!("lillith-settings-av-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A flat-background "character".
        let mut img = image::RgbaImage::from_pixel(120, 200, image::Rgba([30, 140, 130, 255]));
        for y in 20..190 {
            for x in 40..80 {
                img.put_pixel(x, y, image::Rgba([240, 120, 170, 255]));
            }
        }
        let src = dir.join("buddy.png");
        img.save(&src).unwrap();
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        let l = log.clone();
        let emit: Emit =
            Arc::new(move |n, d| l.lock().unwrap().push(format!("{n}:{}", d.unwrap_or(""))));
        let mut s = Settings::new(SettingsOptions {
            config_path: dir.join("config.toml"),
            section: Some("avatar".into()),
            avatars: Some(AvatarStore::new(&dir)),
            import: Some(src),
            emit,
        });
        let ed = s.editor.as_ref().expect("imported into the editor");
        assert!(ed.new && !ed.meta.already_transparent);
        // Editor renders (with the live preview).
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| s.avatar(ui, ctx));
        });
        s.editor.as_mut().unwrap().meta.anchors.head.1 += 5.0;
        s.save_editor();
        assert_eq!(s.selected.as_deref(), Some("buddy"));
        assert!(log
            .lock()
            .unwrap()
            .contains(&"avatar.selected:buddy".to_string()));
        let st = AvatarStore::new(&dir);
        assert_eq!(st.selection().selected.as_deref(), Some("buddy"));
        assert_eq!(st.list().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
