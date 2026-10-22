//! Companion: the right-click menu, 1:1, generated from
//! `lillith_core::menu::build_menu`.

use super::{SettingsTab, Shared};
use crate::chrome;
use crate::companion::{self as model, Persisted};
use egui::{pos2, vec2, Align2, RichText, Sense};
use lillith_anim::atlas::Manifest;
use lillith_anim::avatar::AvatarStore;
use lillith_core::menu::{build_menu, Check, Menu, MenuAction, MenuItem, MenuState, PoseGroup};
use lillith_theme::egui_theme::{self as th, c32};
use lillith_theme::palette as pal;
use std::sync::mpsc::{channel, Receiver};

pub struct CompanionTab {
    state: MenuState,
    log: Vec<String>,
    /// Poses ▸ categories, parsed off the UI thread (the manifest is big).
    poses_rx: Option<Receiver<Vec<PoseGroup>>>,
    avatars: Option<AvatarStore>,
    avatars_checked: f64,
}

impl CompanionTab {
    pub fn new(shared: &Shared) -> Self {
        let avatars = AvatarStore::for_config(Some(&shared.config_path));
        let mut state = model::state_from_store(&shared.store);
        state.extras = model::extras_for(Vec::new(), avatars.as_ref());
        let poses_rx = shared.manifest.clone().map(|path| {
            let (tx, rx) = channel();
            std::thread::spawn(move || {
                let poses = Manifest::load(&path)
                    .map(|m| model::poses_of(&m))
                    .unwrap_or_default();
                let _ = tx.send(poses);
            });
            rx
        });
        Self {
            state,
            log: Vec::new(),
            poses_rx,
            avatars,
            avatars_checked: 0.0,
        }
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
                match lillith_core::menu::open_settings_command(&path).spawn() {
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
    fn body(&self, ui: &mut egui::Ui, main: &Menu, page: Option<&Menu>) -> Option<MenuAction> {
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
                chrome::hint(ui, "Mode, mute, monitor and position are saved to config.toml. Size, mood, poses, avatar, chat and the rest go live to the running overlay as menu_action lines (she owns size).");
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
        if let Some(rx) = &self.poses_rx {
            if let Ok(p) = rx.try_recv() {
                self.state.extras.poses = p;
                self.poses_rx = None;
            }
        }
        // The Avatar tab (or the overlay) may have changed the selection.
        if shared.time - self.avatars_checked > 1.5 {
            self.avatars_checked = shared.time;
            self.refresh_avatars();
        }
    }
    fn busy(&self) -> bool {
        self.poses_rx.is_some()
    }
    /// `--step poses` / `poses/N`: open Poses category N (docs, screenshots).
    fn deep_link(&mut self, arg: &str) {
        if let Some(rest) = arg.strip_prefix("poses") {
            let group = rest.trim_start_matches('/').parse().unwrap_or(0);
            self.state.extras.page = Some(lillith_core::menu::MenuPage { group, page: 0 });
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let mut main_state = self.state.clone();
        main_state.extras.page = None;
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
                    picked = self.body(ui, &main, page.as_ref());
                });
            });
        } else {
            picked = self.body(ui, &main, page.as_ref());
        }
        if let Some(a) = picked {
            self.pick(a, shared);
        }
    }
    fn revert(&mut self, shared: &mut Shared) {
        let extras = std::mem::take(&mut self.state.extras);
        self.state = model::state_from_store(&shared.store);
        self.state.extras = extras;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
