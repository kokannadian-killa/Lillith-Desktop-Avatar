//! Companion: the right-click menu, 1:1, generated from
//! `lillith_core::menu::build_menu`.

use super::{SettingsTab, Shared};
use crate::chrome;
use crate::companion::{self as model, Persisted};
use crate::store::ConfigStore;
use egui::{pos2, vec2, Align2, RichText, Sense};
use lillith_core::menu::{build_menu, Check, MenuAction, MenuItem, MenuState};
use lillith_theme::egui_theme::{self as th, c32};
use lillith_theme::palette::{self as pal, Rgba};

pub struct CompanionTab {
    state: MenuState,
    log: Vec<String>,
}

impl CompanionTab {
    pub fn new(store: &ConfigStore) -> Self {
        Self { state: model::state_from_store(store), log: Vec::new() }
    }

    fn pick(&mut self, a: MenuAction, shared: &mut Shared) {
        model::apply(&mut self.state, a);
        let persisted = model::persist(&mut shared.store, &self.state, a);
        match a {
            MenuAction::OpenSettings => {
                // we *are* the settings window: open the raw file instead
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
            MenuAction::About => shared.goto_tab = Some("about".into()),
            MenuAction::Quit => shared.toast("quit sent to the host (Lillith reads it if she launched this window)", pal::WARN),
            _ => {}
        }
        self.log.push(format!(
            "{} {}",
            model::action_line(a),
            if persisted == Persisted::Staged { "→ config (pending save)" } else { "→ host" }
        ));
        if self.log.len() > 6 {
            self.log.remove(0);
        }
        shared.actions_out.push(a);
    }
}

fn rune_for(label: &str) -> usize {
    label.bytes().fold(7usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize))
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
        ("CORRUPTION_LEVEL", format!("{}%", (state.scale * 100.0).round() as i32)),
        ("RUNE_KEY", format!("{} · {}", model::mode_key(state.mode).to_uppercase(), if state.muted { "MUTE" } else { "VOICE" })),
    ];
    for (i, (k, v)) in lines.iter().enumerate() {
        let y = ly + i as f32 * 15.0;
        th::glitch_text(p, pos2(inner.left() + 4.0, y), Align2::LEFT_TOP, k, th::display_font(9.5), pal::BLOOD_SHEEN);
        p.text(pos2(inner.right() - 4.0, y), Align2::RIGHT_TOP, v, egui::FontId::monospace(11.0), c32(pal::TERMINAL_TEXT));
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
    fn ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let menu = build_menu(&self.state);
        let wide = ui.available_width() > 700.0;
        let mut picked: Option<MenuAction> = None;

        let body = |ui: &mut egui::Ui, picked: &mut Option<MenuAction>, log: &[String]| {
            let mut toggles = Vec::new();
            let mut rites = Vec::new();
            for item in &menu.items {
                match item {
                    MenuItem::Submenu { label, items } => {
                        let title = match label.as_str() {
                            "Mode" => "MODE · MOVEMENT".to_string(),
                            "Mood" => "MOOD · SOUL_SYNTAX".to_string(),
                            "Size" => "SIZE · VESSEL".to_string(),
                            l => l.to_uppercase(),
                        };
                        chrome::section(ui, &title, Some("radio"), |ui| {
                            ui.horizontal_wrapped(|ui| {
                                for it in items {
                                    if let MenuItem::Leaf { label, action, check, enabled } = it {
                                        let sel = matches!(check, Check::Radio(true) | Check::Checkbox(true));
                                        let r = ui.add_enabled_ui(*enabled, |ui| chrome::choice(ui, label, sel, rune_for(label))).inner;
                                        if r.clicked() {
                                            *picked = Some(*action);
                                        }
                                    }
                                }
                            });
                        });
                    }
                    MenuItem::Leaf { check: Check::Checkbox(on), label, action, .. } => toggles.push((label.clone(), *on, *action)),
                    MenuItem::Leaf { label, action, .. } => rites.push((label.clone(), *action)),
                    MenuItem::Separator => {}
                }
            }
            if !toggles.is_empty() {
                chrome::section(ui, "WARDS", Some("toggle"), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for (label, on, action) in &toggles {
                            if chrome::rune_check(ui, *on, label).clicked() {
                                *picked = Some(*action);
                            }
                            ui.add_space(14.0);
                        }
                    });
                });
            }
            if !rites.is_empty() {
                chrome::section(ui, "RITES", Some("actions"), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for (label, action) in &rites {
                            let text = match action {
                                MenuAction::OpenSettings => "Open config.toml".to_string(),
                                MenuAction::Quit => "☠ Quit Lillith".to_string(),
                                _ => label.clone(),
                            };
                            if chrome::stone_button(ui, &text).clicked() {
                                *picked = Some(*action);
                            }
                        }
                    });
                    chrome::hint(ui, "Mode, size, mute, monitor and position are saved to config.toml; mood, about and quit go to the running overlay.");
                });
            }
            chrome::section(ui, "RITE_LOG", Some("stdout"), |ui| {
                if log.is_empty() {
                    ui.label(RichText::new("> awaiting your command_").monospace().color(c32(pal::TEXT_MUTE)));
                }
                for l in log {
                    ui.label(RichText::new(format!("> {l}")).monospace().size(11.5).color(c32(pal::TERMINAL_TEXT)));
                }
            });
        };

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
                    body(ui, &mut picked, &self.log);
                });
            });
        } else {
            body(ui, &mut picked, &self.log);
        }
        if let Some(a) = picked {
            self.pick(a, shared);
        }
        let _ = Rgba::hex(0);
    }
    fn revert(&mut self, shared: &mut Shared) {
        self.state = model::state_from_store(&shared.store);
    }
}
