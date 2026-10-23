//! Motion ▸ Chest bounce: preset, intensity and softness sliders and a
//! reset, over `config.toml [springs.bounce]` (see `lillith_core::bounce`).
//!
//! Applies live: changes to these two keys (and only these) are written
//! straight away — throttled while a slider is dragged, once more on
//! release — and the overlay is sent `reload_config`, so she bounces
//! differently while you drag. The overlay re-reads config.toml itself;
//! the rest of the window's staged edits still wait for Save.

use super::Shared;
use crate::chrome;
use egui::{RichText, Ui};
use lillith_core::bounce::{BouncePreset, ChestBounce, MAX_INTENSITY};
use lillith_core::menu::MenuPick;
use lillith_theme::egui_theme::c32;
use lillith_theme::palette as pal;

pub const PREFIX: &[&str] = &["springs", "bounce"];
pub const INTENSITY: &[&str] = &["springs", "bounce", "intensity"];
pub const SOFTNESS: &[&str] = &["springs", "bounce", "softness"];
/// Live writes while dragging: at most one per this many seconds.
pub const LIVE_EVERY_S: f64 = 0.12;

/// Current values (staged, else on disk, else the default).
pub fn current(shared: &Shared) -> ChestBounce {
    let d = ChestBounce::default();
    ChestBounce {
        intensity: shared.store.get_f32(INTENSITY).unwrap_or(d.intensity),
        softness: shared.store.get_f32(SOFTNESS).unwrap_or(d.softness),
    }
    .sanitized()
}

/// Stages `b` (rounded to whole percent).
pub fn stage(shared: &mut Shared, b: ChestBounce) {
    let b = b.sanitized();
    shared.store.set_f32(INTENSITY, b.intensity, 2);
    shared.store.set_f32(SOFTNESS, b.softness, 2);
}

/// Writes the staged bounce keys and tells the overlay to reload.
/// `true` if something was written.
pub fn push_live(shared: &mut Shared) -> bool {
    match shared.store.save_only(PREFIX) {
        Ok(true) => {
            shared.picks_out.push(MenuPick::ReloadConfig);
            true
        }
        Ok(false) => false,
        Err(e) => {
            shared.toast(format!("chest bounce not saved: {e}"), pal::BLOOD_SHEEN);
            false
        }
    }
}

/// The section, drawn at the top of the Motion tab.
pub fn section(ui: &mut Ui, shared: &mut Shared) {
    chrome::section(ui, "CHEST · BOUNCE", Some("[springs.bounce]"), |ui| {
        body(ui, shared)
    });
}

fn body(ui: &mut Ui, shared: &mut Shared) {
    let mut b = current(shared);
    let mut changed = false;
    let mut settled = false; // a click / release: write now

    let dot = |shared: &Shared, label: &str| {
        if shared.store.is_pending(INTENSITY) || shared.store.is_pending(SOFTNESS) {
            format!("{label} •")
        } else {
            label.to_string()
        }
    };

    chrome::row(ui, &dot(shared, "Preset"), |ui| {
        let cur = b.preset().map_or("Custom", BouncePreset::label);
        egui::ComboBox::from_id_salt("chest-bounce-preset")
            .selected_text(cur)
            .width(150.0)
            .show_ui(ui, |ui| {
                for p in BouncePreset::ALL {
                    if ui
                        .selectable_label(b.preset() == Some(p), p.label())
                        .clicked()
                    {
                        b = p.values();
                        changed = true;
                        settled = true;
                    }
                }
            });
        ui.add_space(8.0);
        if chrome::stone_button(ui, "Reset")
            .on_hover_text("Back to the default: Extra (sexy), 100 %")
            .clicked()
        {
            b = ChestBounce::default();
            changed = true;
            settled = true;
        }
    });

    let slider_w = |ui: &mut Ui| {
        ui.spacing_mut().slider_width = (ui.available_width() - 90.0).clamp(80.0, 260.0);
    };

    chrome::row(ui, &dot(shared, "Intensity"), |ui| {
        slider_w(ui);
        let mut pct = (b.intensity * 100.0).round();
        let r = ui.add(
            egui::Slider::new(&mut pct, 0.0..=MAX_INTENSITY * 100.0)
                .step_by(5.0)
                .fixed_decimals(0)
                .suffix(" %"),
        );
        if r.changed() {
            b.intensity = pct / 100.0;
            changed = true;
        }
        settled |= r.drag_stopped() || (r.changed() && !r.dragged());
    });
    hint_row(
        ui,
        "0 % = off · 100 % = tuned · over 100 % also adds a light bounce on baked-jiggle poses",
    );

    chrome::row(ui, &dot(shared, "Softness"), |ui| {
        slider_w(ui);
        let mut pct = (b.softness * 100.0).round();
        let r = ui.add(
            egui::Slider::new(&mut pct, 0.0..=100.0)
                .step_by(1.0)
                .fixed_decimals(0)
                .suffix(" %"),
        );
        if r.changed() {
            b.softness = pct / 100.0;
            changed = true;
        }
        settled |= r.drag_stopped() || (r.changed() && !r.dragged());
    });
    let f = b.feel();
    let feel = if b.intensity <= 0.0 {
        "off".to_string()
    } else {
        let w = f.wobbles();
        let wob = match w {
            0 => "settles at once".to_string(),
            1 => "1 wobble".to_string(),
            n => format!("{n} wobbles"),
        };
        let lag = if f.extra > 0.0 {
            " · lags her body"
        } else {
            ""
        };
        format!("firm ↔ soft · {:.1} Hz · {wob}{lag}", f.freq_hz)
    };
    hint_row(ui, &feel);

    if changed {
        stage(shared, b);
    }
    // live apply (throttled while dragging)
    let id = egui::Id::new("chest-bounce-live-write");
    let last: f64 = ui
        .ctx()
        .data(|d| d.get_temp(id))
        .unwrap_or(f64::NEG_INFINITY);
    let pending = shared.store.is_pending(INTENSITY) || shared.store.is_pending(SOFTNESS);
    if pending {
        if settled || shared.time - last >= LIVE_EVERY_S {
            let t = shared.time;
            ui.ctx().data_mut(|d| d.insert_temp(id, t));
            push_live(shared);
        } else {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(LIVE_EVERY_S));
        }
    }
    ui.horizontal(|ui| {
        ui.add_space(142.0);
        ui.label(
            RichText::new("◆ applies live — no Save needed")
                .small()
                .color(c32(pal::NEON_BLUE)),
        );
    });
}

fn hint_row(ui: &mut Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(142.0);
        chrome::hint(ui, text);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ConfigStore;

    fn shared(name: &str) -> (std::path::PathBuf, Shared) {
        let d =
            std::env::temp_dir().join(format!("lillith-bounce-tab-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("config.toml");
        std::fs::write(&p, "mode = \"stay\"\n[springs]\njiggle_gain = 0.6\n").unwrap();
        let s = Shared {
            store: ConfigStore::load(&p),
            config_path: p.clone(),
            runtime: None,
            detecting: false,
            want_redetect: false,
            manifest: None,
            picks_out: Vec::new(),
            toasts: Vec::new(),
            time: 0.0,
            goto_tab: None,
            want_save: false,
            accent_changed: None,
        };
        (p, s)
    }

    #[test]
    fn old_config_shows_the_extra_default() {
        let (_, s) = shared("old");
        assert_eq!(current(&s), ChestBounce::default());
        assert_eq!(current(&s).preset(), Some(BouncePreset::Extra));
    }

    #[test]
    fn live_push_writes_only_bounce_and_reloads() {
        let (p, mut s) = shared("live");
        s.store.set_str(&["mode"], "wander");
        stage(
            &mut s,
            ChestBounce {
                intensity: 2.0,
                softness: 0.4,
            },
        );
        assert!(push_live(&mut s));
        assert_eq!(s.picks_out, vec![MenuPick::ReloadConfig]);
        assert_eq!(s.store.pending(), 1, "mode waits for Save");
        let cfg = lillith_core::config::Config::load(&p).unwrap();
        assert_eq!(cfg.springs.bounce.intensity, 2.0);
        assert_eq!(cfg.springs.bounce.softness, 0.4);
        assert_eq!(cfg.mode, lillith_core::config::MoveMode::Stay);
        assert!(!push_live(&mut s), "nothing new");
    }
}
