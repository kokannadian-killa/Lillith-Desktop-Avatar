//! Companion: the right-click menu, 1:1, generated from
//! `lillith_core::menu::build_menu`.
//!
//! Avatar ▸ Outfit is drawn inline as a row of thumbnail cards (Auto, Off,
//! then every outfit from `assets/seasons.toml` that has clips in the set;
//! outfits whose clips haven't landed yet are not shown). A pick stages
//! `seasonal_outfit`, saves, and goes to the overlay live.

use super::{SettingsTab, Shared};
use crate::chrome;
use crate::companion::{self as model, Persisted};
use egui::{pos2, vec2, Align2, RichText, Sense};
use lillith_anim::atlas::Manifest;
use lillith_anim::avatar::AvatarStore;
use lillith_core::menu::{
    build_menu, Check, Menu, MenuAction, MenuItem, MenuState, OUTFIT_AUTO, OUTFIT_OFF,
};
use lillith_core::seasons::{Outfit, Seasons};
use lillith_theme::egui_theme::{self as th, c32};
use lillith_theme::egui_whisper::info_rune;
use lillith_theme::geom;
use lillith_theme::palette::{self as pal, Rgba};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

/// Outfit thumbnails are 128×128 PNGs; shown at the Icon tab's size.
const THUMB: f32 = 96.0;
/// The plum the outfit thumbnails are drawn on (and the placeholder).
const PLUM: Rgba = Rgba::hex(0x3a1438);

/// The animation set and its seasons, parsed off the UI thread.
pub struct LoadedSet {
    manifest: Manifest,
    seasons: Seasons,
    /// Repo root (thumbnail paths in seasons.toml are relative to it).
    root: Option<PathBuf>,
}

impl LoadedSet {
    pub fn load(manifest: &Path) -> Option<Self> {
        // Built-in set plus Ko's imported clips (add-only).
        let m = lillith_anim::user_clips::load_with_user_clips(manifest).ok()?;
        let seasons = Seasons::load_or_bundled(Seasons::path_for_manifest(manifest).as_deref());
        // <root>/assets/generated/manifest.toml
        let root = manifest
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        Some(Self {
            manifest: m,
            seasons,
            root,
        })
    }

    fn worn(&self, setting: &str) -> Option<&Outfit> {
        lillith_anim::catalog::worn_outfit(
            &self.seasons,
            &self.manifest,
            lillith_core::seasons::today(),
            setting,
        )
    }
}

pub struct CompanionTab {
    state: MenuState,
    log: Vec<String>,
    /// The animation set + seasons, parsed off the UI thread (the
    /// manifest is big).
    set_rx: Option<Receiver<Option<LoadedSet>>>,
    set: Option<LoadedSet>,
    thumbs: HashMap<String, Option<egui::TextureHandle>>,
    avatars: Option<AvatarStore>,
    avatars_checked: f64,
}

impl CompanionTab {
    pub fn new(shared: &Shared) -> Self {
        let avatars = AvatarStore::for_config(Some(&shared.config_path));
        let mut state = model::state_from_store(&shared.store);
        let outfit = std::mem::take(&mut state.extras.outfit);
        state.extras = model::extras_for(Vec::new(), avatars.as_ref());
        state.extras.outfit = outfit;
        state.extras.speaker = Some(model::speaker_from_store(&shared.store));
        state.extras.lonely = Some(model::lonely_from_store(&shared.store));
        let set_rx = shared.manifest.clone().map(|path| {
            let (tx, rx) = channel();
            std::thread::spawn(move || {
                let _ = tx.send(LoadedSet::load(&path));
            });
            rx
        });
        Self {
            state,
            log: Vec::new(),
            set_rx,
            set: None,
            thumbs: HashMap::new(),
            avatars,
            avatars_checked: 0.0,
        }
    }

    fn set_loaded(&mut self, set: Option<LoadedSet>) {
        self.set = set;
        if let Some(set) = &self.set {
            self.state.extras.outfits = model::outfits_of(&set.manifest, &set.seasons);
        }
        self.refresh_outfit();
    }

    /// Worn outfit label + Poses (with its seasonal clips) for the
    /// current setting and today's date.
    fn refresh_outfit(&mut self) {
        let Some(set) = &self.set else {
            return;
        };
        let worn = set.worn(&self.state.extras.outfit);
        self.state.extras.outfit_active = worn.map(|o| o.label().to_string());
        self.state.extras.poses = model::poses_for(&set.manifest, worn);
        // A Poses page may now be past the end of its category.
        if let Some(p) = self.state.extras.page {
            let ok = self
                .state
                .extras
                .poses
                .get(p.group as usize)
                .is_some_and(|g| (p.page as usize) * lillith_core::menu::POSE_PAGE < g.clips.len());
            if !ok {
                self.state.extras.page = None;
            }
        }
    }

    fn thumb(&mut self, ctx: &egui::Context, id: &str) -> Option<egui::TextureHandle> {
        if let Some(t) = self.thumbs.get(id) {
            return t.clone();
        }
        let set = self.set.as_ref()?;
        let rel = &set.seasons.outfit(id)?.thumbnail;
        let t = set
            .root
            .as_ref()
            .filter(|_| !rel.trim().is_empty())
            .map(|r| r.join(rel))
            .and_then(|p| image::open(p).ok())
            .map(|img| {
                let img = img
                    .resize(128, 128, image::imageops::FilterType::Triangle)
                    .to_rgba8();
                let ci = egui::ColorImage::from_rgba_unmultiplied(
                    [img.width() as usize, img.height() as usize],
                    img.as_raw(),
                );
                ctx.load_texture(format!("outfit-{id}"), ci, egui::TextureOptions::LINEAR)
            });
        self.thumbs.insert(id.to_string(), t.clone());
        t
    }

    fn refresh_avatars(&mut self) {
        if let Some(st) = &self.avatars {
            let (recent, sel) = model::avatars_of(st);
            self.state.extras.avatars = recent;
            self.state.extras.avatar = sel;
        }
    }

    fn pick(&mut self, a: MenuAction, shared: &mut Shared) {
        if model::is_local(a) {
            // Poses paging: shown inline here, nothing to send.
            model::apply(&mut self.state, a);
            return;
        }
        // Resolve names against the lists the chips were drawn from.
        let wire = model::to_pick(a, &self.state.extras);
        model::apply(&mut self.state, a);
        let persisted = model::persist(&mut shared.store, &self.state, a);
        let mut send = true;
        match a {
            MenuAction::Outfit(_) => {
                shared.want_save = true;
                self.refresh_outfit();
            }
            MenuAction::ChooseIcon => {
                // we are the settings window: go to the Icon tab
                send = false;
                shared.goto_tab = Some("icon".into());
            }
            MenuAction::OpenSettings => {
                // we *are* the settings window: open the raw file instead
                send = false;
                let path = shared.config_path.clone();
                if !path.exists() {
                    if let Some(d) = path.parent() {
                        let _ = std::fs::create_dir_all(d);
                    }
                    let _ = std::fs::write(&path, lillith_core::menu::settings_template());
                }
                match lillith_core::menu::spawn_open_settings(
                    &path,
                    lillith_core::config::audit_log_path(),
                ) {
                    Ok(_) => shared.toast("opening config.toml in your editor", pal::NEON_BLUE),
                    Err(e) => shared.toast(format!("xdg-open failed: {e}"), pal::BLOOD_SHEEN),
                }
            }
            MenuAction::ImportAvatar => {
                // The importer lives in this window's Avatar tab.
                send = false;
                shared.goto_tab = Some("avatar".into());
            }
            MenuAction::Avatar(_) => {
                // Remember the choice like the overlay does, so it sticks
                // even when she isn't running right now.
                if let Some(st) = &self.avatars {
                    let id = self.state.extras.avatar.clone();
                    if let Err(e) = st.select(id.as_deref()) {
                        shared.toast(format!("avatar choice not saved: {e}"), pal::BLOOD_SHEEN);
                    }
                    shared.accent_changed = Some(st.accent(id.as_deref()));
                }
            }
            MenuAction::About => shared.goto_tab = Some("about".into()),
            MenuAction::Quit => shared.toast(
                "quit sent to the host (Lillith reads it if she launched this window)",
                pal::WARN,
            ),
            _ => {}
        }
        let route = if !send {
            "→ here"
        } else if persisted == Persisted::Staged {
            "→ host + config (pending save)"
        } else {
            "→ host"
        };
        self.log
            .push(format!("{} {route}", model::pick_line(&wire)));
        if self.log.len() > 6 {
            self.log.remove(0);
        }
        if send {
            shared.picks_out.push(wire);
        }
    }
}

fn rune_for(label: &str) -> usize {
    label
        .bytes()
        .fold(7usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize))
}

fn emblem(ui: &mut egui::Ui, shared: &Shared, state: &MenuState, h: f32) {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let p = ui.painter();
    let inner = th::terminal_panel(p, r, "SIGIL_CORE", Some("v1.0"));
    let pulse = ((shared.time * 1.6).sin() * 0.5 + 0.5) as f32;
    let s = (inner.height() * 0.66).min(inner.width() * 0.9);
    let c = pos2(inner.center().x, inner.top() + inner.height() * 0.44);
    th::sigil(p, c, s, pulse, true);
    let ly = inner.bottom() - 46.0;
    let lines = [
        ("SOUL_SYNTAX", format!("{:?}", state.mood).to_uppercase()),
        (
            "CORRUPTION_LEVEL",
            format!("{}%", (state.scale * 100.0).round() as i32),
        ),
        (
            "RUNE_KEY",
            format!(
                "{} · {}",
                model::mode_key(state.mode).to_uppercase(),
                if state.muted { "MUTE" } else { "VOICE" }
            ),
        ),
    ];
    for (i, (k, v)) in lines.iter().enumerate() {
        let y = ly + i as f32 * 15.0;
        th::glitch_text(
            p,
            pos2(inner.left() + 4.0, y),
            Align2::LEFT_TOP,
            k,
            th::display_font(9.5),
            pal::BLOOD_SHEEN,
        );
        p.text(
            pos2(inner.right() - 4.0, y),
            Align2::RIGHT_TOP,
            v,
            egui::FontId::monospace(11.0),
            c32(pal::TERMINAL_TEXT),
        );
    }
}

/// Chip text for a menu label (glyphs the relic fonts lack are dropped).
fn chip_text(label: &str) -> String {
    label.replace(" ✨", "").replace('…', "").trim().to_string()
}

/// One submenu as a row of chips; returns the picked action.
fn chips(ui: &mut egui::Ui, items: &[MenuItem], active: Option<MenuAction>) -> Option<MenuAction> {
    let mut picked = None;
    // Wrap by hand: inside the two-column layout the wrapped row's max
    // rect is unbounded, so egui never breaks the line itself.
    let left = ui.cursor().min.x;
    let right = left + ui.available_width();
    ui.horizontal_wrapped(|ui| {
        for it in items {
            if let MenuItem::Leaf { label, .. } = it {
                let w = chrome::choice_width(ui, &chip_text(label)) + ui.spacing().item_spacing.x;
                if ui.cursor().min.x > left + 1.0 && ui.cursor().min.x + w > right {
                    ui.end_row();
                }
            }
            match it {
                MenuItem::Leaf {
                    label,
                    action,
                    check,
                    enabled,
                } => {
                    if !*enabled && matches!(action, MenuAction::MenuBack) {
                        continue; // a page's disabled title row
                    }
                    if matches!(action, MenuAction::OutfitPage) {
                        continue; // drawn as the thumbnail row instead
                    }
                    let sel = matches!(check, Check::Radio(true) | Check::Checkbox(true))
                        || Some(*action) == active;
                    let text = chip_text(label);
                    // Not `add_enabled_ui`: its child scope stops the row
                    // from wrapping.
                    let r = if *enabled {
                        chrome::choice(ui, &text, sel, rune_for(&text))
                    } else {
                        ui.add_enabled_ui(false, |ui| {
                            chrome::choice(ui, &text, sel, rune_for(&text))
                        })
                        .inner
                    };
                    if r.clicked() && *enabled {
                        picked = Some(*action);
                    }
                }
                MenuItem::Separator => {}
                MenuItem::Submenu { .. } => {}
            }
        }
    });
    picked
}

fn section_title(label: &str) -> (String, &'static str) {
    match label {
        "Poses" => ("POSES · SHOW_OFF".into(), "pose"),
        "Avatar" => ("AVATAR · SKIN".into(), "avatar"),
        "Resize" => ("RESIZE · VESSEL".into(), "set_size"),
        "Mode" => ("MODE · MOVEMENT".into(), "radio"),
        "Mood" => ("MOOD · SOUL_SYNTAX".into(), "radio"),
        l => (l.to_uppercase(), "radio"),
    }
}

impl CompanionTab {
    /// Auto / Off / every listed outfit as thumbnail cards.
    fn outfit_row(&self, ui: &mut egui::Ui, time: f64) -> Option<MenuAction> {
        let x = &self.state.extras;
        let cur = x.outfit_index();
        let worn = x.outfit_active.clone();
        let mut picked = None;
        let left = ui.cursor().min.x;
        let right = left + ui.available_width();
        let w = THUMB + 24.0 + ui.spacing().item_spacing.x;
        ui.horizontal_wrapped(|ui| {
            let mut card = |ui: &mut egui::Ui, n: u8, c: Card| {
                if ui.cursor().min.x > left + 1.0 && ui.cursor().min.x + w > right {
                    ui.end_row();
                }
                let enabled = c.enabled;
                if outfit_card(ui, c, cur == Some(n), time).clicked() && enabled {
                    picked = Some(MenuAction::Outfit(n));
                }
            };
            card(
                ui,
                OUTFIT_AUTO,
                Card {
                    tex: None,
                    glyph: "☾",
                    title: "Auto".into(),
                    sub: worn.clone().unwrap_or_else(|| "by date".into()),
                    tip: "Auto: the outfit for today's date (base outfit outside every season)"
                        .into(),
                    enabled: true,
                },
            );
            card(
                ui,
                OUTFIT_OFF,
                Card {
                    tex: None,
                    glyph: "†",
                    title: "Off".into(),
                    sub: "base outfit".into(),
                    tip: "Off: always her base outfit".into(),
                    enabled: true,
                },
            );
            for (i, o) in x.outfits.iter().enumerate() {
                let tex = self.thumbs.get(&o.id).cloned().flatten();
                card(
                    ui,
                    i as u8 + 2,
                    Card {
                        tex,
                        glyph: "",
                        title: o.label.clone(),
                        sub: if o.available {
                            String::new()
                        } else {
                            "coming soon".into()
                        },
                        tip: if o.available {
                            format!("{} ({})", o.label, o.id)
                        } else {
                            format!("{} ({}): coming soon, no clips yet", o.label, o.id)
                        },
                        enabled: o.available,
                    },
                );
            }
        });
        picked
    }

    fn body(
        &self,
        ui: &mut egui::Ui,
        main: &Menu,
        page: Option<&Menu>,
        shared_time: f64,
    ) -> Option<MenuAction> {
        let mut picked: Option<MenuAction> = None;
        let mut summons = Vec::new();
        let mut toggles = Vec::new();
        let mut rites = Vec::new();
        let mut seps = 0;
        let flush_summons = |ui: &mut egui::Ui,
                             summons: &mut Vec<(String, MenuAction, bool)>,
                             picked: &mut Option<MenuAction>| {
            if summons.is_empty() {
                return;
            }
            chrome::section(ui, "SUMMONS", Some("live"), |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (label, action, enabled) in summons.iter() {
                        let r = if *enabled {
                            chrome::stone_button(ui, label)
                        } else {
                            ui.add_enabled_ui(false, |ui| chrome::stone_button(ui, label))
                                .inner
                        };
                        if r.clicked() && *enabled {
                            *picked = Some(*action);
                        }
                    }
                });
            });
            summons.clear();
        };
        for item in &main.items {
            match item {
                MenuItem::Submenu { label, items } => {
                    let (title, tag) = section_title(label);
                    let active_page = self.state.extras.page.map(MenuAction::PosePage);
                    chrome::section(ui, &title, Some(tag), |ui| {
                        if let Some(a) = chips(ui, items, active_page) {
                            picked = Some(a);
                        }
                        if label == "Poses" {
                            match page {
                                Some(pm) => {
                                    let group = pm.items.iter().find_map(|it| match it {
                                        MenuItem::Leaf {
                                            label,
                                            enabled: false,
                                            ..
                                        } => Some(label.clone()),
                                        _ => None,
                                    });
                                    chrome::divider(ui);
                                    chrome::label_os(
                                        ui,
                                        &format!("† {}", group.unwrap_or_default().to_uppercase()),
                                        pal::BLOOD_SHEEN,
                                    );
                                    if let Some(a) = chips(ui, &pm.items, None) {
                                        picked = Some(a);
                                    }
                                }
                                None => {
                                    chrome::hint(ui, "Pick a category to see its poses; a pose plays on her right away.");
                                }
                            }
                        }
                        if label == "Avatar" {
                            chrome::hint(ui, "Custom image opens the importer in the Avatar tab.");
                            if !self.state.extras.outfits.is_empty() {
                                chrome::divider(ui);
                                chrome::label_os(ui, "† OUTFIT · SEASON", pal::BLOOD_SHEEN);
                                if let Some(a) = self.outfit_row(ui, shared_time) {
                                    picked = Some(a);
                                }
                            }
                        }
                    });
                }
                MenuItem::Leaf {
                    check: Check::Checkbox(on),
                    label,
                    action,
                    ..
                } => toggles.push((label.clone(), *on, *action)),
                MenuItem::Leaf {
                    label,
                    action,
                    enabled,
                    ..
                } => {
                    let text = match action {
                        MenuAction::OpenSettings => "Open config.toml".to_string(),
                        MenuAction::Quit => "☠ Quit Lillith".to_string(),
                        _ => label.clone(),
                    };
                    if seps == 0 {
                        summons.push((text, *action, *enabled));
                    } else {
                        rites.push((text, *action, *enabled));
                    }
                }
                MenuItem::Separator => {
                    if seps == 0 {
                        flush_summons(ui, &mut summons, &mut picked);
                    }
                    seps += 1;
                }
            }
        }
        flush_summons(ui, &mut summons, &mut picked);
        if !toggles.is_empty() {
            chrome::section(ui, "WARDS", Some("toggle"), |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (label, on, action) in &toggles {
                        if chrome::rune_check(ui, *on, label).clicked() {
                            picked = Some(*action);
                        }
                        ui.add_space(14.0);
                    }
                });
            });
        }
        if !rites.is_empty() {
            chrome::section(ui, "RITES", Some("actions"), |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (label, action, enabled) in &rites {
                        let r = if *enabled {
                            chrome::stone_button(ui, label)
                        } else {
                            ui.add_enabled_ui(false, |ui| chrome::stone_button(ui, label))
                                .inner
                        };
                        if r.clicked() && *enabled {
                            picked = Some(*action);
                        }
                    }
                });
                chrome::hint(ui, "Mode, mute, monitor, position and outfit are saved to config.toml. Size, mood, poses, avatar, chat and the rest go live to the running overlay as menu_action lines (she owns size).");
            });
        }
        chrome::section(ui, "RITE_LOG", Some("stdout"), |ui| {
            if self.log.is_empty() {
                ui.label(
                    RichText::new("> awaiting your command_")
                        .monospace()
                        .color(c32(pal::TEXT_MUTE)),
                );
            }
            for l in &self.log {
                ui.label(
                    RichText::new(format!("> {l}"))
                        .monospace()
                        .size(11.5)
                        .color(c32(pal::TERMINAL_TEXT)),
                );
            }
        });
        picked
    }
}

impl SettingsTab for CompanionTab {
    fn id(&self) -> &str {
        "companion"
    }
    fn title(&self) -> &str {
        "Companion"
    }
    fn rune(&self) -> usize {
        13
    }
    fn tick(&mut self, _ctx: &egui::Context, shared: &mut Shared) {
        if let Some(rx) = &self.set_rx {
            if let Ok(set) = rx.try_recv() {
                self.set_rx = None;
                self.set_loaded(set);
            }
        }
        // The Avatar tab (or the overlay) may have changed the selection.
        if shared.time - self.avatars_checked > 1.5 {
            self.avatars_checked = shared.time;
            self.refresh_avatars();
        }
    }
    fn busy(&self) -> bool {
        self.set_rx.is_some()
    }
    /// `--step poses` / `poses/N`: open Poses category N (docs, screenshots).
    fn deep_link(&mut self, arg: &str) {
        if let Some(rest) = arg.strip_prefix("poses") {
            let group = rest.trim_start_matches('/').parse().unwrap_or(0);
            self.state.extras.page = Some(lillith_core::menu::MenuPage { group, page: 0 });
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let _ = info_rune(ui, ui.id().with("companion-info"));
        let ids: Vec<String> = self
            .state
            .extras
            .outfits
            .iter()
            .map(|o| o.id.clone())
            .collect();
        for id in ids {
            self.thumb(ui.ctx(), &id);
        }
        let mut main_state = self.state.clone();
        main_state.extras.page = None;
        main_state.extras.outfit_page = false;
        let main = build_menu(&main_state);
        let page = self.state.extras.page.map(|_| build_menu(&self.state));
        let wide = ui.available_width() > 700.0;
        let mut picked: Option<MenuAction> = None;
        if wide {
            let total = ui.available_width();
            let left_w = (total * 0.34).clamp(240.0, 340.0);
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(left_w);
                    emblem(ui, shared, &self.state, 400.0);
                });
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.set_width(total - left_w - 14.0);
                    picked = self.body(ui, &main, page.as_ref(), shared.time);
                });
            });
        } else {
            picked = self.body(ui, &main, page.as_ref(), shared.time);
        }
        if let Some(a) = picked {
            self.pick(a, shared);
        }
    }
    fn revert(&mut self, shared: &mut Shared) {
        let extras = std::mem::take(&mut self.state.extras);
        self.state = model::state_from_store(&shared.store);
        self.state.extras = extras;
        self.state.extras.speaker = Some(model::speaker_from_store(&shared.store));
        self.state.extras.lonely = Some(model::lonely_from_store(&shared.store));
        self.state.extras.outfit = model::outfit_from_store(&shared.store);
        self.refresh_outfit();
    }
}

/// One outfit card's content.
struct Card {
    tex: Option<egui::TextureHandle>,
    /// Drawn on the plum tile when there is no thumbnail (Auto / Off).
    glyph: &'static str,
    title: String,
    sub: String,
    tip: String,
    enabled: bool,
}

/// A chamfered card like the Icon tab's: thumbnail on plum, name, and a
/// sub line ("coming soon" for outfits without clips yet).
fn outfit_card(ui: &mut egui::Ui, c: Card, on: bool, time: f64) -> egui::Response {
    let size = vec2(THUMB + 24.0, THUMB + 52.0);
    let sense = if c.enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let hover = if c.enabled {
        ui.ctx().animate_bool(resp.id.with("h"), resp.hovered())
    } else {
        0.0
    };
    let p = ui.painter();
    let poly = geom::chamfer_rect([rect.left(), rect.top(), rect.right(), rect.bottom()], 8.0);
    th::fill_convex(
        p,
        &poly,
        c32(if on {
            Rgba::hex(0x2a0510)
        } else {
            Rgba::hex(0x12060a)
        }),
    );
    if on {
        let pulse = 0.75 + 0.25 * ((time * 2.0).sin() as f32);
        th::neon_line(p, &poly, pal::PINK, 1.4, 3.5 * pulse, true);
    } else {
        th::knife_highlight(p, rect.shrink2(vec2(3.0, 4.0)), hover * 0.55);
        p.add(egui::Shape::closed_line(
            poly.iter().map(|q| pos2(q[0], q[1])).collect(),
            egui::Stroke::new(1.0, c32(pal::BLOOD.lerp(pal::BLOOD_NEON, hover))),
        ));
    }
    let img = egui::Rect::from_min_size(
        pos2(rect.center().x - THUMB * 0.5, rect.top() + 10.0),
        vec2(THUMB, THUMB),
    );
    let dim = if c.enabled { 1.0 } else { 0.38 };
    p.rect_filled(img, 10.0, c32(PLUM.lerp(Rgba::hex(0x000000), 1.0 - dim)));
    match &c.tex {
        Some(t) => {
            p.image(
                t.id(),
                img,
                egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                egui::Color32::from_gray((255.0 * dim) as u8),
            );
        }
        None => {
            let g = if c.glyph.is_empty() { "?" } else { c.glyph };
            p.text(
                img.center(),
                Align2::CENTER_CENTER,
                g,
                th::display_font(34.0),
                c32(pal::PINK.lerp(Rgba::hex(0x000000), 1.0 - dim)),
            );
        }
    }
    let tc = if on {
        Rgba::hex(0xffffff)
    } else if c.enabled {
        pal::TEXT_DIM.lerp(Rgba::hex(0xffffff), hover)
    } else {
        pal::TEXT_MUTE
    };
    let fit = |s: &str, n: usize| -> String {
        if s.chars().count() > n {
            format!("{}…", s.chars().take(n - 1).collect::<String>())
        } else {
            s.to_string()
        }
    };
    // "Micro Witch (Halloween)" → name over season.
    let (name, season) = match c.title.split_once(" (") {
        Some((n, s)) => (n.to_string(), s.trim_end_matches(')').to_string()),
        None => (c.title.clone(), String::new()),
    };
    p.text(
        pos2(rect.center().x, rect.bottom() - 32.0),
        Align2::CENTER_CENTER,
        fit(&name, 16),
        th::bold_font(12.0),
        c32(tc),
    );
    let sub = if c.sub.is_empty() {
        season
    } else {
        c.sub.clone()
    };
    p.text(
        pos2(rect.center().x, rect.bottom() - 15.0),
        Align2::CENTER_CENTER,
        fit(&sub, 18),
        egui::FontId::monospace(10.5),
        c32(if c.enabled {
            pal::BLOOD_SHEEN
        } else {
            pal::TEXT_MUTE
        }),
    );
    let resp = resp.on_hover_text(c.tip);
    if c.enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lillith_core::menu::PoseGroup;

    #[test]
    fn renders_main_menu_and_a_poses_page() {
        let mut shared = crate::tabs::tests::shared();
        let mut tab = CompanionTab::new(&shared);
        tab.state.extras.poses = vec![PoseGroup {
            label: "Gestures".into(),
            clips: vec![("wave".into(), "Wave".into())],
        }];
        tab.pick(
            MenuAction::PosePage(lillith_core::menu::MenuPage::default()),
            &mut shared,
        );
        assert!(shared.picks_out.is_empty(), "paging stays local");
        let ctx = egui::Context::default();
        lillith_theme::egui_theme::install(&ctx);
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| tab.ui(ui, &mut shared));
            });
        }
        tab.pick(MenuAction::Pose(0), &mut shared);
        tab.pick(MenuAction::ShowOff, &mut shared);
        tab.pick(MenuAction::ImportAvatar, &mut shared);
        let lines: Vec<String> = shared.picks_out.iter().map(model::pick_line).collect();
        assert_eq!(
            lines,
            [
                r#"{"menu_action":"pose","value":"wave"}"#,
                r#"{"menu_action":"show_off"}"#
            ]
        );
        assert_eq!(shared.goto_tab.as_deref(), Some("avatar"));
    }

    #[test]
    fn outfit_row_picks_persist_and_go_live() {
        let mut shared = crate::tabs::tests::shared();
        let mut tab = CompanionTab::new(&shared);
        let clip = |n: &str, tags: &str| {
            format!(
                "[clips.{n}]\npage = \"{n}.png\"\nfps = 8.0\ntags = [{tags}]\n[[clips.{n}.frames]]\nx = 0\ny = 0\nw = 400\nh = 400\nanchor = [200, 392]\n"
            )
        };
        let text = clip("blow_kiss", "")
            + &clip(
                "halloween_hat_tip_wink",
                "\"seasonal\", \"season:halloween\", \"outfit:halloween\"",
            );
        tab.set_loaded(Some(LoadedSet {
            manifest: Manifest::from_toml_str(&text).unwrap(),
            seasons: Seasons::bundled(),
            root: None,
        }));
        assert_eq!(tab.state.extras.outfits.len(), 1);
        let ctx = egui::Context::default();
        lillith_theme::egui_theme::install(&ctx);
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| tab.ui(ui, &mut shared));
            });
        }
        tab.pick(MenuAction::OutfitPage, &mut shared);
        assert!(shared.picks_out.is_empty(), "the page is local");
        tab.pick(MenuAction::Outfit(2), &mut shared);
        assert_eq!(tab.state.extras.outfit, "halloween");
        assert_eq!(
            tab.state.extras.outfit_active.as_deref(),
            Some("Micro Witch (Halloween)")
        );
        let has_witch = |t: &CompanionTab| {
            t.state
                .extras
                .poses
                .iter()
                .any(|g| g.clips.iter().any(|c| c.0 == "halloween_hat_tip_wink"))
        };
        assert!(has_witch(&tab));
        assert_eq!(
            shared.store.get_str(&["seasonal_outfit"]).as_deref(),
            Some("halloween")
        );
        assert!(shared.want_save);
        tab.pick(MenuAction::Outfit(OUTFIT_OFF), &mut shared);
        assert!(!has_witch(&tab) && tab.state.extras.outfit_active.is_none());
        let lines: Vec<String> = shared.picks_out.iter().map(model::pick_line).collect();
        assert_eq!(
            lines,
            [
                r#"{"menu_action":"seasonal_outfit","value":"halloween"}"#,
                r#"{"menu_action":"seasonal_outfit","value":"off"}"#
            ]
        );
    }
}
