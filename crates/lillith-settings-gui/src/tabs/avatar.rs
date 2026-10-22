//! Avatar: who is on the desktop (Lillith or a custom image), the local
//! importer (background removal, fit to her 400×400 stage), the anchor
//! editor with a live animated preview, and the display knobs.
//!
//! All image / store / motion logic is `lillith_anim::avatar` and
//! `lillith_anim::avatar_edit` (shared with `lillith-panel::settings`);
//! this file only draws it in the relic theme. A save or a pick goes out
//! as the wire's `avatar` line (by id, `lillith` for her own sprites) plus
//! `reload_config`.

use super::knobs::KnobTab;
use super::{SettingsTab, Shared};
use crate::chrome;
use crate::task::Task;
use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke};
use lillith_anim::avatar::{
    import_image, AvatarError, AvatarMeta, AvatarStore, ImportOptions, BASELINE, CANVAS,
};
use lillith_anim::avatar_edit::{self, Preview};
use lillith_core::menu::{MenuAction, MenuPick};
use lillith_theme::egui_theme::{self as th, c32};
use lillith_theme::geom;
use lillith_theme::palette::{self as pal, Rgba};
use std::collections::HashMap;
use std::path::PathBuf;

/// Editor canvas side (px) for the 400 px stage.
const EDIT_SIDE: f32 = 300.0;
/// Live preview side (px).
const PREVIEW_SIDE: u32 = 180;

type Imported = Result<(image::RgbaImage, AvatarMeta), AvatarError>;

/// The anchor editor on one avatar (fresh import or stored).
struct Editor {
    img: image::RgbaImage,
    meta: AvatarMeta,
    /// Not in the store yet.
    new: bool,
    tex: Option<egui::TextureHandle>,
    dragging: Option<&'static str>,
    preview: Preview,
    preview_tex: Option<egui::TextureHandle>,
    last: f64,
}

impl Editor {
    fn new(img: image::RgbaImage, meta: AvatarMeta, new: bool) -> Self {
        let preview = Preview::new(&img, &meta);
        Self {
            img,
            meta,
            new,
            tex: None,
            dragging: None,
            preview,
            preview_tex: None,
            last: 0.0,
        }
    }
}

pub struct AvatarTab {
    knobs: KnobTab,
    store: Option<AvatarStore>,
    avatars: Vec<AvatarMeta>,
    selected: Option<String>,
    import_path: String,
    already_transparent: bool,
    tolerance: f32,
    editor: Option<Editor>,
    importing: Option<Task<Imported>>,
    browsing: Option<Task<Option<PathBuf>>>,
    /// Import requested before the first frame (deep link).
    import_on_open: bool,
    status: Option<(String, Rgba)>,
    thumbs: HashMap<String, egui::TextureHandle>,
}

impl AvatarTab {
    pub fn new(config_path: &std::path::Path) -> Self {
        let mut t = Self {
            knobs: KnobTab::avatar(),
            store: AvatarStore::for_config(Some(config_path)),
            avatars: Vec::new(),
            selected: None,
            import_path: String::new(),
            already_transparent: false,
            tolerance: ImportOptions::default().tolerance,
            editor: None,
            importing: None,
            browsing: None,
            import_on_open: false,
            status: None,
            thumbs: HashMap::new(),
        };
        t.refresh();
        t
    }

    fn refresh(&mut self) {
        if let Some(st) = &self.store {
            self.avatars = st.list();
            self.selected = st.selection().selected;
        }
    }

    fn say(&mut self, text: impl Into<String>, color: Rgba) {
        self.status = Some((text.into(), color));
    }

    fn start_import(&mut self, ctx: &egui::Context) {
        let path = PathBuf::from(self.import_path.trim());
        let opts = ImportOptions {
            already_transparent: self.already_transparent,
            tolerance: self.tolerance,
        };
        self.say("removing the background…", pal::NEON_BLUE);
        self.importing = Some(Task::spawn(ctx, "import", move || {
            import_image(&path, opts)
        }));
    }

    /// Tells the overlay: the avatar on stage, then re-read config.toml
    /// (through the window's save when knob edits are staged).
    fn announce(&mut self, id: Option<String>, shared: &mut Shared) {
        shared.picks_out.push(match &id {
            Some(_) => MenuPick::Avatar(id.clone()),
            None => MenuPick::Action(MenuAction::Avatar(0)),
        });
        if shared.store.pending() > 0 {
            shared.want_save = true; // emits reload_config once written
        } else {
            shared.picks_out.push(MenuPick::ReloadConfig);
        }
    }

    fn select(&mut self, id: Option<String>, shared: &mut Shared) {
        if let Some(st) = &self.store {
            if let Err(e) = st.select(id.as_deref()) {
                self.say(format!("could not save the choice: {e}"), pal::BLOOD_SHEEN);
                return;
            }
        }
        self.selected = id.clone();
        self.say(
            format!("on stage: {}", self.name_of(id.as_deref())),
            pal::OK,
        );
        self.announce(id, shared);
    }

    fn name_of(&self, id: Option<&str>) -> String {
        match id {
            None => "Lillith".into(),
            Some(id) => self
                .avatars
                .iter()
                .find(|m| m.id == id)
                .map_or_else(|| id.to_string(), |m| m.name.clone()),
        }
    }

    /// Saves the editor's avatar and puts it on stage.
    fn commit(&mut self, shared: &mut Shared) -> Result<(), String> {
        let Some(st) = self.store.clone() else {
            return Err("no config directory for avatars".into());
        };
        let Some(ed) = self.editor.as_mut() else {
            return Ok(());
        };
        let id = avatar_edit::commit(&st, &ed.img, &mut ed.meta, &mut ed.new)
            .map_err(|e| format!("could not save the avatar: {e}"))?;
        self.thumbs.remove(&id);
        self.editor = None;
        self.refresh();
        self.select(Some(id), shared);
        Ok(())
    }

    fn open_editor(&mut self, id: &str) {
        let Some(st) = &self.store else { return };
        let res = st.load(id).and_then(|av| {
            image::open(&av.path)
                .map(|img| (img.to_rgba8(), av.meta))
                .map_err(|e| AvatarError::Image(av.path.display().to_string(), e.to_string()))
        });
        match res {
            Ok((img, meta)) => self.editor = Some(Editor::new(img, meta, false)),
            Err(e) => self.say(format!("{e}"), pal::BLOOD_SHEEN),
        }
    }

    fn thumb(&mut self, ctx: &egui::Context, m: &AvatarMeta) -> Option<egui::TextureHandle> {
        if let Some(t) = self.thumbs.get(&m.id) {
            return Some(t.clone());
        }
        let path = avatar_edit::image_path(self.store.as_ref()?, m);
        let img = image::open(path).ok()?.thumbnail(96, 96).to_rgba8();
        let ci = egui::ColorImage::from_rgba_unmultiplied(
            [img.width() as usize, img.height() as usize],
            img.as_raw(),
        );
        let t = ctx.load_texture(format!("thumb-{}", m.id), ci, egui::TextureOptions::LINEAR);
        self.thumbs.insert(m.id.clone(), t.clone());
        Some(t)
    }

    // ------------------------------------------------------------ gallery

    fn gallery(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let mut pick: Option<Option<String>> = None;
        let mut edit: Option<String> = None;
        let ctx = ui.ctx().clone();
        chrome::section(
            ui,
            "VESSEL · WHO_WALKS_YOUR_DESKTOP",
            Some("avatar"),
            |ui| {
                ui.horizontal_wrapped(|ui| {
                    if vessel_card(ui, None, "Lillith", self.selected.is_none(), shared.time)
                        .clicked()
                    {
                        pick = Some(None);
                    }
                    for m in self.avatars.clone() {
                        let on = self.selected.as_deref() == Some(m.id.as_str());
                        let tex = self.thumb(&ctx, &m);
                        ui.vertical(|ui| {
                            if vessel_card(ui, tex.as_ref(), &m.name, on, shared.time).clicked() {
                                pick = Some(Some(m.id.clone()));
                            }
                            if ui
                                .add(
                                    egui::Label::new(
                                        RichText::new("† anchors")
                                            .small()
                                            .color(c32(pal::NEON_BLUE)),
                                    )
                                    .sense(Sense::click()),
                                )
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                edit = Some(m.id.clone());
                            }
                        });
                    }
                });
                chrome::hint(
                ui,
                "Lillith is her full sprite set. Custom images are animated by the 2.5D engine; her own look stays canon.",
            );
            },
        );
        if let Some(p) = pick {
            self.select(p, shared);
        }
        if let Some(id) = edit {
            self.open_editor(&id);
        }
    }

    // ----------------------------------------------------------- importer

    fn importer(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let ctx = ui.ctx().clone();
        chrome::section(ui, "SUMMON · CUSTOM_IMAGE", Some("local import"), |ui| {
            chrome::hint(
                ui,
                "PNG, JPG or WebP. Nothing leaves this computer: a flat background is removed, the image is fitted to her 400×400 stage and the anchors are guessed.",
            );
            ui.add_space(4.0);
            chrome::row(ui, "Image", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.import_path)
                        .hint_text("/path/to/image.png")
                        .desired_width((ui.available_width() - 110.0).max(120.0)),
                );
                if cfg!(feature = "file-dialog")
                    && self.browsing.is_none()
                    && chrome::stone_button(ui, "Browse…").clicked()
                {
                    self.browsing = browse_image(&ctx);
                }
            });
            chrome::row(ui, "Background", |ui| {
                if chrome::rune_check(ui, self.already_transparent, "already transparent").clicked()
                {
                    self.already_transparent = !self.already_transparent;
                }
            });
            ui.add_enabled_ui(!self.already_transparent, |ui| {
                chrome::row(ui, "Tolerance", |ui| {
                    ui.spacing_mut().slider_width =
                        (ui.available_width() - 70.0).clamp(80.0, 240.0);
                    ui.add(egui::Slider::new(&mut self.tolerance, 8.0..=120.0).fixed_decimals(0));
                });
            });
            ui.add_space(4.0);
            let ready = !self.import_path.trim().is_empty() && self.importing.is_none();
            let label = if self.importing.is_some() {
                "Summoning…"
            } else {
                "Import"
            };
            if chrome::blade_button(ui, label, ready).clicked() && ready {
                self.start_import(&ctx);
            }
        });
        let _ = shared;
    }

    // ------------------------------------------------------ anchor editor

    fn editor_ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let ctx = ui.ctx().clone();
        let mut save = false;
        let mut cancel = false;
        let wide = ui.available_width() > 640.0;
        let Some(ed) = self.editor.as_mut() else {
            return;
        };
        let title = format!("ANCHORS · {}", ed.meta.name.to_uppercase());
        let tag = if ed.new { "unsaved import" } else { "stored" };
        chrome::section(ui, &title, Some(tag), |ui| {
            chrome::hint(
                ui,
                "Drag the sigils onto the head, mouth, chest, hip and hair tips. The neon line is the floor her feet stand on.",
            );
            ui.add_space(4.0);
            let time = shared.time;
            if wide {
                ui.horizontal_top(|ui| {
                    anchor_canvas(ui, &ctx, ed);
                    ui.add_space(12.0);
                    ui.vertical(|ui| editor_side(ui, &ctx, ed, time, &mut save, &mut cancel));
                });
            } else {
                anchor_canvas(ui, &ctx, ed);
                editor_side(ui, &ctx, ed, time, &mut save, &mut cancel);
            }
        });
        ctx.request_repaint();
        if save {
            if let Err(e) = self.commit(shared) {
                self.say(e, pal::BLOOD_SHEEN);
            } else if shared.store.pending() > 0 {
                shared.want_save = true;
            }
        } else if cancel {
            self.editor = None;
            self.say("import dropped", pal::TEXT_DIM);
        }
    }
}

/// Live preview, character / jiggle / name, Save & use, Re-guess, Cancel.
fn editor_side(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    ed: &mut Editor,
    time: f64,
    save: &mut bool,
    cancel: &mut bool,
) {
    let dt = (time - ed.last).clamp(0.0, 0.1) as f32;
    ed.last = time;
    if let Some(c) = ed.preview.render(&ed.meta, dt, time as f32, PREVIEW_SIDE) {
        let ci = egui::ColorImage::from_rgba_premultiplied(
            [PREVIEW_SIDE as usize, PREVIEW_SIDE as usize],
            c.data(),
        );
        match &mut ed.preview_tex {
            Some(t) => t.set(ci, egui::TextureOptions::LINEAR),
            None => {
                ed.preview_tex =
                    Some(ctx.load_texture("avatar-preview", ci, egui::TextureOptions::LINEAR))
            }
        }
    }
    let s = PREVIEW_SIDE as f32 + 28.0;
    let (r, _) = ui.allocate_exact_size(vec2(s, s + 20.0), Sense::hover());
    let inner = th::terminal_panel(ui.painter(), r, "LIVE_PREVIEW", None);
    if let Some(t) = &ed.preview_tex {
        let img = Rect::from_center_size(
            inner.center(),
            vec2(PREVIEW_SIDE as f32, PREVIEW_SIDE as f32),
        );
        ui.painter().image(
            t.id(),
            img,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    ui.add_space(4.0);
    if chrome::rune_check(ui, ed.meta.character, "character (jiggle + talk bob)").clicked() {
        ed.meta.character = !ed.meta.character;
    }
    ui.add_enabled_ui(ed.meta.character, |ui| {
        ui.horizontal(|ui| {
            chrome::label_os(ui, "JIGGLE", pal::BLOOD_SHEEN);
            ui.spacing_mut().slider_width = 120.0;
            ui.add(egui::Slider::new(&mut ed.meta.jiggle_strength, 0.0..=1.0).fixed_decimals(2));
        });
    });
    ui.horizontal(|ui| {
        chrome::label_os(ui, "NAME", pal::BLOOD_SHEEN);
        ui.add(egui::TextEdit::singleline(&mut ed.meta.name).desired_width(150.0));
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if chrome::blade_button(ui, "Save & use", true).clicked() {
            *save = true;
        }
    });
    ui.horizontal(|ui| {
        if chrome::stone_button(ui, "Re-guess anchors").clicked() {
            avatar_edit::reguess_anchors(&ed.img, &mut ed.meta);
        }
        if chrome::stone_button(ui, "Cancel").clicked() {
            *cancel = true;
        }
    });
}

/// The 400 px stage at [`EDIT_SIDE`] px on a stone checker, with the
/// floor line and draggable anchor sigils.
fn anchor_canvas(ui: &mut egui::Ui, ctx: &egui::Context, ed: &mut Editor) {
    let k = EDIT_SIDE / CANVAS as f32;
    let tex = ed.tex.get_or_insert_with(|| {
        let ci = egui::ColorImage::from_rgba_unmultiplied(
            [ed.img.width() as usize, ed.img.height() as usize],
            ed.img.as_raw(),
        );
        ctx.load_texture("avatar-edit", ci, egui::TextureOptions::LINEAR)
    });
    let pad = 10.0;
    let (outer, resp) = ui.allocate_exact_size(
        vec2(EDIT_SIDE + pad * 2.0, EDIT_SIDE + pad * 2.0),
        Sense::click_and_drag(),
    );
    let p = ui.painter();
    let frame = th::iron_frame(p, outer, pad, 7);
    let r = Rect::from_center_size(frame.center(), vec2(EDIT_SIDE, EDIT_SIDE));
    p.rect_filled(r, 0.0, c32(Rgba::hex(0x120a0d)));
    let cell = 15.0;
    for y in 0..(EDIT_SIDE / cell) as i32 {
        for x in 0..(EDIT_SIDE / cell) as i32 {
            if (x + y) % 2 == 0 {
                p.rect_filled(
                    Rect::from_min_size(
                        r.min + vec2(x as f32 * cell, y as f32 * cell),
                        vec2(cell, cell),
                    ),
                    0.0,
                    c32(Rgba::hex(0x1d1216)),
                );
            }
        }
    }
    p.image(
        tex.id(),
        r,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    let base_y = r.min.y + BASELINE as f32 * k;
    th::neon_line(
        p,
        &[[r.min.x, base_y], [r.max.x, base_y]],
        pal::NEON_BLUE,
        1.0,
        3.0,
        false,
    );
    let to_screen = |a: (f32, f32)| r.min + vec2(a.0 * k, a.1 * k);
    let to_canvas = |q: Pos2| ((q.x - r.min.x) / k, (q.y - r.min.y) / k);
    if resp.drag_started() {
        if let Some(pos) = resp.interact_pointer_pos() {
            ed.dragging = avatar_edit::nearest_anchor(&ed.meta, to_canvas(pos), 16.0 / k);
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
    let hover = resp
        .hover_pos()
        .and_then(|h| avatar_edit::nearest_anchor(&ed.meta, to_canvas(h), 16.0 / k));
    if hover.is_some() || ed.dragging.is_some() {
        ctx.set_cursor_icon(egui::CursorIcon::Grab);
    }
    for ((name, a), [cr, cg, cb]) in ed
        .meta
        .anchors
        .named()
        .iter()
        .zip(avatar_edit::ANCHOR_COLORS)
    {
        let s = to_screen(*a);
        let col = Rgba::new(cr, cg, cb, 255);
        let hot = hover == Some(*name) || ed.dragging == Some(*name);
        th::glow_blob(
            p,
            s,
            if hot { 16.0 } else { 11.0 },
            col,
            if hot { 1.4 } else { 0.9 },
        );
        let d = if hot { 7.5 } else { 6.0 };
        let diamond = [
            [s.x, s.y - d],
            [s.x + d, s.y],
            [s.x, s.y + d],
            [s.x - d, s.y],
        ];
        th::fill_convex(p, &diamond, c32(pal::VOID));
        th::neon_line(p, &diamond, col, 1.6, 2.5, true);
        p.circle_filled(s, 1.8, Color32::WHITE);
        let label = name.replace('_', " ").to_uppercase();
        // Spread the labels so clustered points (head / mouth / hair) stay
        // readable.
        let (at, align) = match *name {
            "head" => (s + vec2(0.0, -10.0), Align2::CENTER_BOTTOM),
            "mouth" => (s + vec2(0.0, 10.0), Align2::CENTER_TOP),
            "hair_left" => (s + vec2(-10.0, 0.0), Align2::RIGHT_CENTER),
            _ => (s + vec2(10.0, 0.0), Align2::LEFT_CENTER),
        };
        p.text(
            at + vec2(1.0, 1.0),
            align,
            &label,
            FontId::monospace(10.0),
            Color32::BLACK,
        );
        p.text(
            at,
            align,
            &label,
            FontId::monospace(10.0),
            c32(col.lerp(Rgba::hex(0xffffff), 0.35)),
        );
    }
}

/// One gallery card: thumbnail (or her sigil), name, knife highlight and a
/// pink neon edge when on stage.
fn vessel_card(
    ui: &mut egui::Ui,
    tex: Option<&egui::TextureHandle>,
    name: &str,
    on: bool,
    time: f64,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(112.0, 118.0), Sense::click());
    let hover = ui.ctx().animate_bool(resp.id.with("h"), resp.hovered());
    let p = ui.painter();
    let poly = geom::chamfer_rect([rect.left(), rect.top(), rect.right(), rect.bottom()], 9.0);
    th::fill_convex(
        p,
        &poly,
        c32(if on {
            Rgba::hex(0x24060f)
        } else {
            Rgba::hex(0x110709)
        }),
    );
    if hover > 0.0 && !on {
        th::knife_highlight(p, rect.shrink2(vec2(3.0, 4.0)), hover * 0.25);
    }
    let art = Rect::from_min_size(rect.min + vec2(14.0, 10.0), vec2(84.0, 80.0));
    match tex {
        Some(t) => {
            let sz = t.size_vec2();
            let s = (art.width() / sz.x).min(art.height() / sz.y);
            p.image(
                t.id(),
                Rect::from_center_size(art.center(), sz * s),
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        None => {
            let pulse = ((time * 1.6).sin() * 0.5 + 0.5) as f32;
            th::sigil(p, art.center() + vec2(0.0, 2.0), 64.0, pulse, false);
        }
    }
    if on {
        th::neon_line(p, &poly, pal::PINK, 1.4, 4.0, true);
    } else {
        p.add(egui::Shape::closed_line(
            poly.iter().map(|q| pos2(q[0], q[1])).collect(),
            Stroke::new(1.0, c32(pal::BLOOD.lerp(pal::BLOOD_NEON, hover))),
        ));
    }
    let mut label = name.to_string();
    if label.chars().count() > 13 {
        label = label.chars().take(12).collect::<String>() + "…";
    }
    p.text(
        pos2(rect.center().x, rect.bottom() - 15.0),
        Align2::CENTER_CENTER,
        label,
        th::bold_font(13.0),
        c32(if on { Rgba::hex(0xffffff) } else { pal::TEXT }),
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[cfg(feature = "file-dialog")]
fn browse_image(ctx: &egui::Context) -> Option<Task<Option<PathBuf>>> {
    Some(Task::spawn(ctx, "browse-image", || {
        let mut d = rfd::FileDialog::new()
            .set_title("Pick an avatar image")
            .add_filter("Images", &["png", "jpg", "jpeg", "webp"]);
        if let Some(h) = lillith_setup::paths::home_dir() {
            d = d.set_directory(h);
        }
        d.pick_file()
    }))
}

#[cfg(not(feature = "file-dialog"))]
fn browse_image(_ctx: &egui::Context) -> Option<Task<Option<PathBuf>>> {
    None
}

impl SettingsTab for AvatarTab {
    fn id(&self) -> &str {
        "avatar"
    }
    fn title(&self) -> &str {
        "Avatar"
    }
    fn rune(&self) -> usize {
        19
    }
    fn tick(&mut self, ctx: &egui::Context, _shared: &mut Shared) {
        if std::mem::take(&mut self.import_on_open) {
            self.start_import(ctx);
        }
        if let Some(t) = &mut self.browsing {
            if let Some(r) = t.poll() {
                self.browsing = None;
                if let Some(p) = r {
                    self.import_path = p.display().to_string();
                    self.start_import(ctx);
                }
            }
        }
        if let Some(t) = &mut self.importing {
            if let Some(r) = t.poll() {
                self.importing = None;
                match r {
                    Ok((img, meta)) => {
                        let msg = avatar_edit::import_summary(&meta);
                        self.editor = Some(Editor::new(img, meta, true));
                        self.say(msg, pal::OK);
                    }
                    Err(e) => self.say(format!("import failed: {e}"), pal::BLOOD_SHEEN),
                }
            }
        }
    }
    fn busy(&self) -> bool {
        self.importing.is_some() || self.import_on_open
    }
    fn ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        if let Some((text, color)) = &self.status {
            ui.label(
                RichText::new(format!("> {text}"))
                    .monospace()
                    .color(c32(*color)),
            );
            ui.add_space(2.0);
        }
        if self.editor.is_some() {
            self.editor_ui(ui, shared);
        } else {
            self.gallery(ui, shared);
            self.importer(ui, shared);
        }
        ui.add_space(4.0);
        self.knobs.ui(ui, shared);
    }
    fn pending(&self) -> usize {
        // An open editor is an unsaved import / anchor edit.
        self.knobs.pending() + usize::from(self.editor.is_some())
    }
    fn save(&mut self, shared: &mut Shared) -> Result<(), String> {
        if self.editor.is_some() {
            self.commit(shared)?;
        }
        self.knobs.save(shared)
    }
    fn revert(&mut self, shared: &mut Shared) {
        self.editor = None;
        self.refresh();
        self.knobs.revert(shared);
    }
    /// `import:PATH` imports right away; `anchors:ID` opens a stored
    /// avatar in the editor (docs, screenshots, tests).
    fn deep_link(&mut self, arg: &str) {
        if let Some(p) = arg.strip_prefix("import:") {
            self.import_path = p.to_string();
            self.import_on_open = true;
        } else if let Some(id) = arg.strip_prefix("anchors:") {
            self.open_editor(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(dir: &std::path::Path) -> PathBuf {
        let mut img = image::RgbaImage::from_pixel(120, 200, image::Rgba([30, 140, 130, 255]));
        for y in 20..190 {
            for x in 40..80 {
                img.put_pixel(x, y, image::Rgba([240, 120, 170, 255]));
            }
        }
        let p = dir.join("buddy.png");
        img.save(&p).unwrap();
        p
    }

    #[test]
    fn import_edit_save_announces_avatar_and_reload() {
        let dir =
            std::env::temp_dir().join(format!("lillith-settings-avtab-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.toml");
        let mut shared = crate::tabs::tests::shared();
        shared.config_path = cfg.clone();
        shared.store = crate::store::ConfigStore::load(&cfg);
        let mut tab = AvatarTab::new(&cfg);
        tab.deep_link(&format!("import:{}", sample(&dir).display()));
        assert!(tab.busy());
        let ctx = egui::Context::default();
        lillith_theme::egui_theme::install(&ctx);
        // Run frames until the background import lands in the editor.
        for _ in 0..400 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                tab.tick(ctx, &mut shared);
                egui::CentralPanel::default().show(ctx, |ui| tab.ui(ui, &mut shared));
            });
            if tab.editor.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(tab.editor.as_ref().is_some_and(|e| e.new));
        assert_eq!(tab.pending(), 1, "an open import is unsaved");
        // Footer Save commits it, selects it and announces it.
        tab.save(&mut shared).unwrap();
        assert!(tab.editor.is_none());
        assert_eq!(tab.selected.as_deref(), Some("buddy"));
        let lines: Vec<String> = shared
            .picks_out
            .iter()
            .map(lillith_core::menu::pick_line)
            .collect();
        assert_eq!(
            lines,
            [
                r#"{"menu_action":"avatar","value":"buddy"}"#,
                r#"{"menu_action":"reload_config"}"#
            ]
        );
        let st = AvatarStore::new(&dir);
        assert_eq!(st.selection().selected.as_deref(), Some("buddy"));
        // Back to Lillith.
        shared.picks_out.clear();
        tab.select(None, &mut shared);
        assert_eq!(
            lillith_core::menu::pick_line(&shared.picks_out[0]),
            r#"{"menu_action":"avatar","value":"lillith"}"#
        );
        // Re-open the stored one in the editor and render it.
        tab.deep_link("anchors:buddy");
        assert!(tab.editor.as_ref().is_some_and(|e| !e.new));
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| tab.ui(ui, &mut shared));
        });
        let _ = std::fs::remove_dir_all(dir);
    }
}
