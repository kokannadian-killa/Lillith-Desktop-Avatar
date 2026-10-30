//! About: version, the local-only promise, licences.

use super::{SettingsTab, Shared};
use crate::chrome;
use egui::RichText;
use lillith_theme::egui_theme::{self as th, c32};
use lillith_theme::palette as pal;
use lillith_theme::egui_whisper::info_rune;

pub struct AboutTab;

impl SettingsTab for AboutTab {
    fn id(&self) -> &str {
        "about"
    }
    fn title(&self) -> &str {
        "About"
    }
    fn rune(&self) -> usize {
        3
    }
    fn ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        chrome::section(ui, "ABOUT_LILLITH", Some(env!("CARGO_PKG_VERSION")), |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Lillith")
                        .font(th::carved_font(26.0))
                        .color(c32(pal::TEXT)),
                );
                let _ = info_rune(ui, ui.id().with("about-lillith-i"));
            });
            ui.label(
                RichText::new("DemonOS chrome · PhantomCore / QuantumSec lattice")
                    .small()
                    .color(c32(pal::CYBER_PURPLE)),
            );
            ui.label(RichText::new(lillith_core::menu::about_text()).color(c32(pal::TEXT_DIM)));
            ui.add_space(4.0);
            chrome::row(ui, "config", |ui| {
                ui.monospace(shared.config_path.display().to_string())
            });
        });
        chrome::section(ui, "LOCAL_ONLY_PACT", Some("no telemetry"), |ui| {
            for line in [
                "Lily's brain runs on this computer. Chats and models never leave the disk.",
                "Only loopback (127.0.0.1 / ::1 / localhost) is ever contacted — remote hosts are refused.",
                "No telemetry, no update checks, no downloads. llama-server / ollama only start when you click.",
            ] {
                ui.label(RichText::new(format!("†  {line}")).color(c32(pal::TEXT)));
            }
        });
        chrome::section(ui, "DEMONOS_PHANTOMCORE", Some("branding"), |ui| {
            for line in [
                "DemonOS — blood-red terminal panels, relic chrome, hellforge Soul Contract.",
                "PhantomCore — QuantumSec shield branding (protons, blood seal, local only).",
                "Biotek — data home ~/.biotek (never confuses with upstream ~/.biotekengine).",
            ] {
                ui.label(RichText::new(format!("†  {line}")).color(c32(pal::TEXT)));
            }
        });
        chrome::section(ui, "GRIMOIRE_CREDITS", None, |ui| {
            ui.label(
                RichText::new(lillith_theme::fonts::LICENSE_NOTICE)
                    .small()
                    .color(c32(pal::TEXT_DIM)),
            );
            ui.label(RichText::new("Character art is locked canon (see LICENSE-ASSETS.md); this window only draws chrome.").small().color(c32(pal::TEXT_MUTE)));
        });
    }
}
