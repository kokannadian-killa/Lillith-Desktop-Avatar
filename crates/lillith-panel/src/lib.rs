//! # lillith-panel
//!
//! A normal (decorated) `egui`/`eframe` window, separate from the overlay,
//! so it needs nothing special from Wayland (PLAN.md §1.1, M6/M7):
//!
//! - **Chat** tab: input line + streamed replies (also shown in her bubble).
//! - [`chat_bubble`]: the small click-to-chat window beside her (own process).
//! - **Settings** tab: live-edit `config.toml` (springs, thresholds, size, opacity).
//! - **Poses** tab: browse and play every manifest clip through the animator
//!   (springs, breathing, idle life, walk flip, reactions) — M2, see [`poses`].
//! - **Events** tab: debug injector that fires any `assets/rules.toml` event.
//!
//! `LillithStage.tsx` is the visual reference for the stage/debug layout.

use eframe::egui;
use lillith_core::rules::RuleSet;
use std::path::PathBuf;

pub mod chat_bubble;
pub mod contract_card;
pub mod poses;
pub mod server_card;
pub mod settings;
pub mod voice_mic;

/// Which tab is visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Chat,
    Settings,
    Poses,
    Events,
}

/// Panel state.
pub struct Panel {
    tab: Tab,
    input: String,
    log: Vec<String>,
    rules: RuleSet,
    poses: poses::Poses,
}

impl Panel {
    /// Panel whose Poses tab loads `manifest` (or the default lookup).
    pub fn new(manifest: Option<PathBuf>) -> Self {
        Self {
            tab: Tab::default(),
            input: String::new(),
            log: Vec::new(),
            rules: RuleSet::bundled(),
            poses: poses::Poses::new(manifest),
        }
    }
}

impl Default for Panel {
    fn default() -> Self {
        Self::new(None)
    }
}

impl eframe::App for Panel {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Chat, "Chat");
                ui.selectable_value(&mut self.tab, Tab::Settings, "Settings");
                ui.selectable_value(&mut self.tab, Tab::Poses, "Poses");
                ui.selectable_value(&mut self.tab, Tab::Events, "Events");
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Chat => {
                for line in &self.log {
                    ui.label(line);
                }
                let resp = ui.text_edit_singleline(&mut self.input);
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    // TODO(M7): send to lillith-mind and stream the reply.
                    self.log.push(format!("you: {}", self.input));
                    self.input.clear();
                }
            }
            Tab::Settings => {
                ui.label("TODO(M6): live config.toml editor");
            }
            Tab::Poses => self.poses.ui(ui),
            Tab::Events => {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for rule in &self.rules.rules {
                        if ui.button(&rule.event).clicked() {
                            // TODO(M4): publish on the event bus.
                            self.log
                                .push(format!("inject {} → {}", rule.event, rule.pose));
                        }
                    }
                });
            }
        });
    }
}

/// Opens the panel window (blocks until it is closed).
pub fn run() -> eframe::Result<()> {
    run_with(None)
}

/// Opens the panel with the Poses tab on `manifest`, starting on that tab
/// when a manifest was given.
pub fn run_with(manifest: Option<PathBuf>) -> eframe::Result<()> {
    let start_on_poses = manifest.is_some();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Lillith")
            .with_inner_size([660.0, 580.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Lillith",
        options,
        Box::new(move |_cc| {
            let mut p = Panel::new(manifest);
            if start_on_poses {
                p.tab = Tab::Poses;
            }
            Ok(Box::new(p))
        }),
    )
}
