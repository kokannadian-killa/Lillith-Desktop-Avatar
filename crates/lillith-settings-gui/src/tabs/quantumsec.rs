//! QuantumSec tab: shield status, awakening dialog, blood-seal lockdown.

use super::{SettingsTab, Shared};
use crate::chrome;
use egui::{RichText, Ui};
use lillith_quantumsec::consent::{AwakeningState, ConsentLog};
use lillith_quantumsec::lockdown::LockdownProposal;
use lillith_quantumsec::shield::QuantumSecShield;
use lillith_quantumsec::status::{ShieldStatus, StatusLevel};
use lillith_theme::egui_awakening::{quantumsec_awakening_popup, AwakeningChoice};
use lillith_theme::egui_blood_seal::lockdown_blood_seal;
use lillith_theme::egui_contract::ContractChoice;
use lillith_theme::egui_theme::{self as th, c32};
use lillith_theme::palette as pal;
use std::path::PathBuf;

pub struct QuantumSecTab {
    show_awakening: bool,
    awakening_checked: bool,
    pending: Option<LockdownProposal>,
    last_status: Option<ShieldStatus>,
    biotek: Option<PathBuf>,
}

impl QuantumSecTab {
    pub fn new() -> Self {
        Self {
            show_awakening: false,
            awakening_checked: false,
            pending: None,
            last_status: None,
            biotek: lillith_setup::biotek::biotek_home(),
        }
    }

    fn status_path(&self) -> Option<PathBuf> {
        self.biotek
            .as_ref()
            .map(|h| h.join("quantumsec").join("status.json"))
    }

    fn refresh_status(&mut self) {
        if let Some(p) = self.status_path() {
            self.last_status = QuantumSecShield::load_status_file(&p);
        }
    }

    fn ensure_awakening_flag(&mut self) {
        if self.awakening_checked {
            return;
        }
        self.awakening_checked = true;
        let Some(home) = &self.biotek else {
            return;
        };
        let path = home.join("config").join("awakening.json");
        let state = AwakeningState::load(&path);
        if !state.shown {
            self.show_awakening = true;
        }
    }

    fn mark_awakening_shown(&mut self) {
        self.show_awakening = false;
        let Some(home) = &self.biotek else {
            return;
        };
        let path = home.join("config").join("awakening.json");
        let mut state = AwakeningState::load(&path);
        state.mark_shown();
        let _ = state.save(&path);
    }
}

impl Default for QuantumSecTab {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsTab for QuantumSecTab {
    fn id(&self) -> &str {
        "quantumsec"
    }
    fn title(&self) -> &str {
        "QuantumSec"
    }
    fn rune(&self) -> usize {
        17
    }

    fn tick(&mut self, ctx: &egui::Context, _shared: &mut Shared) {
        self.ensure_awakening_flag();
        self.refresh_status();
        if self.show_awakening {
            if let Some(c) =
                quantumsec_awakening_popup(ctx, egui::Id::new("qsec-awakening"))
            {
                match c {
                    AwakeningChoice::Continue | AwakeningChoice::Dismiss => {
                        self.mark_awakening_shown();
                    }
                }
            }
            ctx.request_repaint();
        }
        if let Some(p) = self.pending.clone() {
            if let Some(choice) = lockdown_blood_seal(
                ctx,
                egui::Id::new("qsec-blood-seal"),
                &p.summary,
                &p.detail_block(),
                p.local_simulation,
            ) {
                let home = self.biotek.clone();
                match choice {
                    ContractChoice::Yes => {
                        if let Some(h) = home {
                            if let Ok(log) = ConsentLog::open_in(h.join("consent")) {
                                if let Ok(mut tok) = log.approve(&p) {
                                    // Execute only the local-simulation proposal from the UI.
                                    let _ = lillith_quantumsec::lockdown::execute_with_token(
                                        &p, &mut tok,
                                    );
                                }
                            }
                        }
                        self.pending = None;
                    }
                    ContractChoice::No => {
                        if let Some(h) = home {
                            if let Ok(log) = ConsentLog::open_in(h.join("consent")) {
                                let _ = log.deny(&p);
                            }
                        }
                        self.pending = None;
                    }
                }
            }
            ctx.request_repaint();
        }
    }

    fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        self.refresh_status();

        chrome::section(ui, "QUANTUMSEC_SHIELD", Some("local protons"), |ui| {
            ui.label(
                RichText::new("QuantumSec")
                    .font(th::carved_font(22.0))
                    .color(c32(pal::CYBER_CRIMSON)),
            );
            ui.label(
                RichText::new("Protons watch this machine only. Red team = local simulation.")
                    .color(c32(pal::CYBER_TEXT_DIM)),
            );
            ui.add_space(6.0);
            match &self.last_status {
                Some(st) => {
                    let col = match st.level {
                        StatusLevel::Nominal => pal::OK,
                        StatusLevel::Elevated => pal::WARN,
                        StatusLevel::Critical => pal::CYBER_CRIMSON,
                        StatusLevel::Paused => pal::TEXT_MUTE,
                    };
                    ui.label(
                        RichText::new(format!("STATUS  {}", st.level.label()))
                            .font(th::display_font(14.0))
                            .color(c32(col)),
                    );
                    ui.monospace(&st.summary);
                    ui.add_space(4.0);
                    for p in &st.protons {
                        let line = match &p.last {
                            Some(r) => format!("† {} — {}", p.id.label(), r.summary),
                            None => format!("† {} — (no reading)", p.id.label()),
                        };
                        let resp = ui.label(RichText::new(line).color(c32(pal::CYBER_TEXT)));
                        resp.on_hover_text(p.id.whisper());
                    }
                }
                None => {
                    ui.label(
                        RichText::new("No status yet. Start Lillith (or `lillith quantumsec`) to tick the shield.")
                            .color(c32(pal::TEXT_MUTE)),
                    );
                }
            }
            if let Some(p) = self.status_path() {
                chrome::row(ui, "status.json", |ui| {
                    ui.monospace(p.display().to_string())
                });
            }
        });

        chrome::section(ui, "BLOOD_SEAL", Some("consent gate"), |ui| {
            ui.label(
                RichText::new(
                    "Every lockdown or bot deploy shows the exact command. Yes is mouse-only.",
                )
                .color(c32(pal::TEXT_DIM)),
            );
            ui.add_space(4.0);
            if chrome::blade_button(ui, "Propose LOCAL SIMULATION lockdown", true).clicked() {
                let p = LockdownProposal::local_simulation_default();
                self.pending = Some(p);
                shared.toast(
                    "Blood seal opened — review the exact commands",
                    pal::CYBER_CRIMSON,
                );
            }
            if chrome::stone_button(ui, "Show awakening again").clicked() {
                self.show_awakening = true;
                if let Some(home) = &self.biotek {
                    let path = home.join("config").join("awakening.json");
                    let s = AwakeningState::default();
                    let _ = s.save(&path); // reset so next dismiss re-marks
                }
            }
        });

        chrome::section(ui, "WHISPER_FROM_THE_CORE", Some("info"), |ui| {
            for id in lillith_quantumsec::ProtonId::ALL {
                ui.label(
                    RichText::new(format!("⛧ {} — {}", id.label(), id.whisper()))
                        .small()
                        .color(c32(pal::CYBER_TEXT_DIM)),
                );
            }
        });
    }
}
