//! QuantumSec tab: shield, teams, bots, personalization, awakening, blood seal.

use super::{SettingsTab, Shared};
use crate::chrome;
use egui::{RichText, Ui};
use lillith_quantumsec::consent::{AwakeningState, ConsentLog, ConsentRecord};
use lillith_quantumsec::lockdown::LockdownProposal;
use lillith_quantumsec::registry::{self, BotEntry, BotLane};
use lillith_quantumsec::report::{DeviceProfile, REPORT_KEEP};
use lillith_quantumsec::shield::QuantumSecShield;
use lillith_quantumsec::status::{ShieldStatus, StatusLevel};
use lillith_quantumsec::teams::{self, TeamColor, TeamItem};
use lillith_quantumsec::units;
use lillith_theme::egui_awakening::{quantumsec_awakening_popup, AwakeningChoice};
use lillith_theme::egui_blood_seal::lockdown_blood_seal;
use lillith_theme::egui_contract::ContractChoice;
use lillith_theme::egui_theme::{self as th, c32};
use lillith_theme::egui_whisper::{info_rune, whisper_from_the_core};
use lillith_theme::palette as pal;
use std::path::PathBuf;

pub struct QuantumSecTab {
    show_awakening: bool,
    awakening_checked: bool,
    pending: Option<LockdownProposal>,
    last_status: Option<ShieldStatus>,
    biotek: Option<PathBuf>,
    catalog: Vec<TeamItem>,
    bots: Vec<BotEntry>,
    whisper: Option<(String, String)>,
    last_report: Option<PathBuf>,
    /// Screenshot / deep-link: put CONSENT_LOG at top.
    focus_consent: bool,
    consent_rows: Vec<ConsentRecord>,
    last_refresh: f64,
}

impl QuantumSecTab {
    pub fn new() -> Self {
        let biotek = lillith_setup::biotek::biotek_home();
        let catalog = biotek
            .as_ref()
            .map(|h| teams::load_catalog(h))
            .unwrap_or_else(teams::default_catalog);
        let bots = biotek
            .as_ref()
            .map(|h| registry::load_registry(h))
            .unwrap_or_else(registry::default_registry);
        Self {
            show_awakening: false,
            awakening_checked: false,
            pending: None,
            last_status: None,
            biotek,
            catalog,
            bots,
            whisper: None,
            last_report: None,
            focus_consent: false,
            consent_rows: Vec::new(),
            last_refresh: 0.0,
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

    fn refresh_consent(&mut self) {
        self.consent_rows.clear();
        if let Some(h) = &self.biotek {
            if let Ok(log) = ConsentLog::open_in(h.join("consent")) {
                if let Ok(rows) = log.list_recent(24) {
                    self.consent_rows = rows;
                }
            }
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

    fn open_whisper(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.whisper = Some((title.into(), body.into()));
    }

    fn draw_consent_section(&mut self, ui: &mut Ui, shared: &mut Shared) {
        chrome::section(ui, "CONSENT_LOG", Some("viewer · rotation"), |ui| {
            ui.label(
                RichText::new(
                    "Append-only under ~/.biotek/consent/. Daily files rotate (keep 30).",
                )
                .color(c32(pal::TEXT_DIM)),
            );
            if chrome::stone_button(ui, "Refresh consent log").clicked() {
                self.refresh_consent();
            }
            if chrome::stone_button(ui, "Rotate now (keep 30)").clicked() {
                if let Some(h) = &self.biotek {
                    if let Ok(log) = ConsentLog::open_in(h.join("consent")) {
                        match log.rotate_keep(30) {
                            Ok(n) => shared
                                .toast(format!("Removed {n} old consent files"), pal::CYBER_CYAN),
                            Err(e) => {
                                shared.toast(format!("Rotate failed: {e}"), pal::CYBER_CRIMSON)
                            }
                        }
                    }
                }
            }
            ui.add_space(4.0);
            if self.consent_rows.is_empty() {
                ui.label(RichText::new("(empty)").color(c32(pal::TEXT_MUTE)));
            } else {
                let rows = self.consent_rows.clone();
                for rec in rows.iter().take(12) {
                    let line = format!(
                        "† {} · {} · {} — {}",
                        rec.ts, rec.decision, rec.kind, rec.summary
                    );
                    ui.label(RichText::new(line).small().color(c32(pal::CYBER_TEXT_DIM)));
                }
            }
        });
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

    fn deep_link(&mut self, arg: &str) {
        match arg {
            "awakening" | "awaken" => {
                self.show_awakening = true;
                self.awakening_checked = true;
            }
            "blood-seal" | "blood_seal" | "lockdown" => {
                self.pending = Some(LockdownProposal::local_simulation_default());
                self.awakening_checked = true;
                self.show_awakening = false;
            }
            "tab" | "" => {
                self.awakening_checked = true;
                self.show_awakening = false;
            }
            "whisper" => {
                self.awakening_checked = true;
                self.show_awakening = false;
                self.open_whisper(
                    "WHISPER FROM THE CORE",
                    "Info popups sit beside every QuantumSec control. Tooltips stay; this modal goes deeper. Esc or DISMISS closes.",
                );
            }
            "teams" => {
                self.awakening_checked = true;
                self.show_awakening = false;
            }
            "consent" | "consent-log" => {
                self.awakening_checked = true;
                self.show_awakening = false;
                self.focus_consent = true;
                self.refresh_consent();
            }
            "unit-enable" | "systemd" | "enable-unit" => {
                self.awakening_checked = true;
                self.show_awakening = false;
                let hint = PathBuf::from("/usr/lib/systemd/user/lillith-quantumsec.service");
                self.pending = Some(units::enable_quantumsec_unit_proposal(&hint));
            }
            _ => {}
        }
    }

    fn tick(&mut self, ctx: &egui::Context, _shared: &mut Shared) {
        self.ensure_awakening_flag();
        let now = ctx.input(|i| i.time);
        if now - self.last_refresh > 2.0 {
            self.refresh_status();
            self.refresh_consent();
            self.last_refresh = now;
            ctx.request_repaint_after(std::time::Duration::from_secs(2));
        }
        if self.show_awakening {
            if let Some(c) = quantumsec_awakening_popup(ctx, egui::Id::new("qsec-awakening")) {
                match c {
                    AwakeningChoice::Continue | AwakeningChoice::Dismiss => {
                        self.mark_awakening_shown();
                    }
                }
            }
            ctx.request_repaint();
        }
        if let Some((title, body)) = self.whisper.clone() {
            if whisper_from_the_core(ctx, egui::Id::new("qsec-whisper"), &title, &body) {
                self.whisper = None;
            }
            ctx.request_repaint();
        }
        if let Some(p) = self.pending.clone() {
            if let Some(choice) = lockdown_blood_seal(
                ctx,
                egui::Id::new("qsec-blood-seal"),
                &p.summary,
                &p.detail_block(),
                p.seal_banner(),
            ) {
                let home = self.biotek.clone();
                match choice {
                    ContractChoice::Yes => {
                        if let Some(h) = home {
                            if let Ok(log) = ConsentLog::open_in(h.join("consent")) {
                                if let Ok(mut tok) = log.approve(&p) {
                                    let nonce = tok.nonce.clone();
                                    match lillith_quantumsec::lockdown::execute_with_token(
                                        &p, &mut tok,
                                    ) {
                                        Ok(outs) => {
                                            let codes: Vec<String> = outs
                                                .iter()
                                                .map(|o| {
                                                    o.status
                                                        .code()
                                                        .map(|c| c.to_string())
                                                        .unwrap_or_else(|| "signal".into())
                                                })
                                                .collect();
                                            let summary = if p.proposal_only {
                                                format!(
                                                    "acknowledged proposal (not run); cmds={}",
                                                    p.commands.len()
                                                )
                                            } else {
                                                format!(
                                                    "executed {} cmds; exit=[{}]",
                                                    outs.len(),
                                                    codes.join(",")
                                                )
                                            };
                                            let _ = log.record(
                                                "lockdown_result",
                                                &p.id,
                                                lillith_quantumsec::consent::ConsentDecision::Approve,
                                                &summary,
                                                &p.commands,
                                                &nonce,
                                            );
                                        }
                                        Err(e) => {
                                            let _ = log.record(
                                                "lockdown_result",
                                                &p.id,
                                                lillith_quantumsec::consent::ConsentDecision::Deny,
                                                &format!("execute failed: {e}"),
                                                &p.commands,
                                                &nonce,
                                            );
                                        }
                                    }
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
        let catalog = self.catalog.clone();
        let bots = self.bots.clone();

        if self.focus_consent {
            self.draw_consent_section(ui, shared);
            ui.add_space(10.0);
        }

        chrome::section(ui, "QUANTUMSEC_SHIELD", Some("local protons"), |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("QuantumSec")
                        .font(th::carved_font(22.0))
                        .color(c32(pal::CYBER_CRIMSON)),
                );
                if info_rune(ui, ui.id().with("shield-i")) {
                    self.open_whisper(
                        "Shield",
                        "WHISPER FROM THE CORE — Protons watch this machine only. Idle polling stays light. Red team = local simulation.",
                    );
                }
            });
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
                    let proton_rows: Vec<_> = st
                        .protons
                        .iter()
                        .map(|p| {
                            let line = match &p.last {
                                Some(r) => format!("† {} — {}", p.id.label(), r.summary),
                                None => format!("† {} — (no reading)", p.id.label()),
                            };
                            (p.id.label(), p.id.whisper(), line)
                        })
                        .collect();
                    for (label, whisper, line) in proton_rows {
                        ui.horizontal(|ui| {
                            let resp = ui.label(RichText::new(line).color(c32(pal::CYBER_TEXT)));
                            resp.on_hover_text(whisper);
                            if info_rune(ui, ui.id().with(("proton-i", label))) {
                                self.open_whisper(label, whisper);
                            }
                        });
                    }
                }
                None => {
                    ui.label(
                        RichText::new(
                            "No status yet. Start Lillith (or `lillith quantumsec`) to tick the shield.",
                        )
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

        chrome::section(ui, "TEAMS", Some("red · blue · purple"), |ui| {
            ui.label(
                RichText::new("Every exercise shows exact commands. Red = LOCAL SIMULATION ONLY.")
                    .color(c32(pal::TEXT_DIM)),
            );
            for color in TeamColor::ALL {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(color.label())
                            .font(th::display_font(13.0))
                            .color(c32(match color {
                                TeamColor::Red => pal::CYBER_CRIMSON,
                                TeamColor::Blue => pal::CYBER_CYAN,
                                TeamColor::Purple => pal::CYBER_PURPLE,
                            })),
                    );
                    let tip = ui.label(RichText::new("·").color(c32(pal::TEXT_MUTE)));
                    tip.on_hover_text(color.tooltip());
                    if info_rune(ui, ui.id().with(("team-i", color.label()))) {
                        self.open_whisper(color.label(), color.whisper());
                    }
                });
                for item in catalog.iter().filter(|i| i.team == color) {
                    ui.horizontal(|ui| {
                        let resp = ui.label(
                            RichText::new(format!("  ▸ {}", item.title))
                                .color(c32(pal::CYBER_TEXT)),
                        );
                        resp.on_hover_text(&item.tooltip);
                        if info_rune(ui, ui.id().with(("item-i", item.id.as_str()))) {
                            self.open_whisper(&item.title, &item.whisper);
                        }
                        if chrome::blade_button(ui, "Propose…", true).clicked() {
                            self.pending = Some(item.as_proposal());
                            shared.toast(format!("Blood seal: {}", item.title), pal::CYBER_CRIMSON);
                        }
                    });
                }
            }
        });

        chrome::section(ui, "BOT_REGISTRY", Some("main + anon swarm"), |ui| {
            ui.label(
                RichText::new(
                    "Deploy proposes exact commands (Python BotStore / Tor socks5h). Blood seal required.",
                )
                .color(c32(pal::TEXT_DIM)),
            );
            for lane in [BotLane::Main, BotLane::AnonSwarm] {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(lane.label())
                            .font(th::display_font(12.0))
                            .color(c32(if lane == BotLane::AnonSwarm {
                                pal::CYBER_PURPLE
                            } else {
                                pal::CYBER_CYAN
                            })),
                    );
                    ui.label(
                        RichText::new(lane.tooltip())
                            .small()
                            .color(c32(pal::TEXT_MUTE)),
                    );
                    if info_rune(ui, ui.id().with(("lane-i", lane.label()))) {
                        self.open_whisper(lane.label(), lane.whisper());
                    }
                });
                for bot in bots.iter().filter(|b| b.lane == lane) {
                    ui.horizontal(|ui| {
                        let resp = ui.label(
                            RichText::new(format!("  ▸ {} ({})", bot.title, bot.role))
                                .color(c32(pal::CYBER_TEXT)),
                        );
                        resp.on_hover_text(&bot.tooltip);
                        if info_rune(ui, ui.id().with(("bot-i", bot.id.as_str()))) {
                            self.open_whisper(&bot.title, &bot.whisper);
                        }
                        if chrome::blade_button(ui, "Deploy…", true).clicked() {
                            if let Some(h) = &self.biotek {
                                self.pending = Some(bot.deploy_proposal(h));
                                shared.toast(
                                    format!("Blood seal: deploy {}", bot.id),
                                    pal::CYBER_PURPLE,
                                );
                            }
                        }
                    });
                }
            }
        });

        chrome::section(ui, "PERSONALIZATION", Some("reports"), |ui| {
            ui.label(
                RichText::new(
                    format!("Device profile → Markdown in ~/.biotek/reports (keep {REPORT_KEEP}). Fish helpers shown; seal to acknowledge."),
                )
                .color(c32(pal::TEXT_DIM)),
            );
            if chrome::blade_button(ui, "Generate personalization report", true).clicked() {
                if let Some(h) = &self.biotek {
                    let profile = DeviceProfile::collect(h);
                    match profile.save_markdown(h) {
                        Ok(path) => {
                            self.last_report = Some(path.clone());
                            shared.toast(format!("Report → {}", path.display()), pal::CYBER_CYAN);
                        }
                        Err(e) => shared.toast(format!("Report failed: {e}"), pal::CYBER_CRIMSON),
                    }
                }
            }
            if chrome::stone_button(ui, "Propose Fish helpers (blood seal)").clicked() {
                if let Some(h) = &self.biotek {
                    let profile = DeviceProfile::collect(h);
                    self.pending = Some(profile.fish_apply_proposal());
                }
            }
            if let Some(p) = &self.last_report {
                ui.monospace(p.display().to_string());
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
                    let _ = s.save(&path);
                }
            }
        });

        if !self.focus_consent {
            self.draw_consent_section(ui, shared);
        }

        chrome::section(ui, "SYSTEMD_USER", Some("blood-seal enable"), |ui| {
            ui.label(
                RichText::new(
                    "lillith-quantumsec.service is never auto-enabled. Propose enable/disable via blood seal — exact real systemctl commands run only after SIGN IN BLOOD.",
                )
                .color(c32(pal::TEXT_DIM)),
            );
            if chrome::blade_button(ui, "Propose enable user unit…", true).clicked() {
                let hint = PathBuf::from("/usr/lib/systemd/user/lillith-quantumsec.service");
                self.pending = Some(units::enable_quantumsec_unit_proposal(&hint));
                shared.toast("Blood seal: enable QuantumSec unit", pal::CYBER_CRIMSON);
            }
            if chrome::stone_button(ui, "Propose disable user unit…").clicked() {
                self.pending = Some(units::disable_quantumsec_unit_proposal());
            }
        });

        chrome::section(ui, "WHISPER_FROM_THE_CORE", Some("info"), |ui| {
            for id in lillith_quantumsec::ProtonId::ALL {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("⚠ {} — {}", id.label(), id.whisper()))
                            .small()
                            .color(c32(pal::CYBER_TEXT_DIM)),
                    );
                    if info_rune(ui, ui.id().with(("wlist", id.label()))) {
                        self.open_whisper(id.label(), id.whisper());
                    }
                });
            }
        });
    }
}
