//! Motion ▸ Jiggle: overall multiplier, per-region intensity (chest, hips,
//! pigtails, tail), spring layers and build-up over `config.toml
//! [springs.jiggle]` (see `lillith_core::jiggle`).
//!
//! Applies live like the chest bounce section: only these keys are written
//! (throttled while dragging) and the overlay is sent `reload_config`.

use super::Shared;
use crate::chrome;
use egui::{RichText, Ui};
use lillith_core::jiggle::{JiggleSettings, Region, MAX_LAYERS, MAX_MULTIPLIER, MAX_REGION};
use lillith_core::menu::MenuPick;
use lillith_theme::egui_theme::c32;
use lillith_theme::palette as pal;

pub const PREFIX: &[&str] = &["springs", "jiggle"];
pub const ENABLED: &[&str] = &["springs", "jiggle", "enabled"];
pub const MULTIPLIER: &[&str] = &["springs", "jiggle", "multiplier"];
pub const LAYERS: &[&str] = &["springs", "jiggle", "layers"];
pub const BUILD_UP: &[&str] = &["springs", "jiggle", "build_up"];
pub const ZOOM_FOLLOW: &[&str] = &["springs", "jiggle", "zoom_follow"];
/// Live writes while dragging: at most one per this many seconds.
pub const LIVE_EVERY_S: f64 = 0.12;

fn region_key(r: Region) -> [&'static str; 3] {
    ["springs", "jiggle", r.name()]
}

fn keys() -> Vec<Vec<&'static str>> {
    let mut k: Vec<Vec<&str>> = [ENABLED, MULTIPLIER, LAYERS, BUILD_UP, ZOOM_FOLLOW]
        .iter()
        .map(|k| k.to_vec())
        .collect();
    k.extend(Region::ALL.map(|r| region_key(r).to_vec()));
    k
}

/// Current values (staged, else on disk, else the default).
pub fn current(shared: &Shared) -> JiggleSettings {
    let d = JiggleSettings::default();
    let st = &shared.store;
    let mut s = JiggleSettings {
        enabled: st.get_bool(ENABLED).unwrap_or(d.enabled),
        multiplier: st.get_f32(MULTIPLIER).unwrap_or(d.multiplier),
        layers: st
            .get_i64(LAYERS)
            .map_or(d.layers, |v| v.clamp(1, MAX_LAYERS as i64) as u8),
        build_up: st.get_f32(BUILD_UP).unwrap_or(d.build_up),
        zoom_follow: st.get_bool(ZOOM_FOLLOW).unwrap_or(d.zoom_follow),
        ..d
    };
    for r in Region::ALL {
        s.set_region(r, st.get_f32(&region_key(r)).unwrap_or(d.region(r)));
    }
    s.sanitized()
}

/// Stages `s` (sanitized, two decimals).
pub fn stage(shared: &mut Shared, s: JiggleSettings) {
    let s = s.sanitized();
    let st = &mut shared.store;
    st.set_bool(ENABLED, s.enabled);
    st.set_f32(MULTIPLIER, s.multiplier, 2);
    st.set_i64(LAYERS, i64::from(s.layers));
    st.set_f32(BUILD_UP, s.build_up, 2);
    st.set_bool(ZOOM_FOLLOW, s.zoom_follow);
    for r in Region::ALL {
        st.set_f32(&region_key(r), s.region(r), 2);
    }
}

fn pending(shared: &Shared) -> bool {
    keys().iter().any(|k| shared.store.is_pending(k))
}

/// Writes the staged jiggle keys and tells the overlay to reload.
pub fn push_live(shared: &mut Shared) -> bool {
    match shared.store.save_only(PREFIX) {
        Ok(true) => {
            shared.picks_out.push(MenuPick::ReloadConfig);
            true
        }
        Ok(false) => false,
        Err(e) => {
            shared.toast(format!("jiggle not saved: {e}"), pal::BLOOD_SHEEN);
            false
        }
    }
}

/// The section, drawn under Chest bounce on the Motion tab.
pub fn section(ui: &mut Ui, shared: &mut Shared) {
    chrome::section(ui, "JIGGLE · LAYERED", Some("[springs.jiggle]"), |ui| {
        body(ui, shared)
    });
}

fn pct_slider(ui: &mut Ui, v: &mut f32, max: f32, changed: &mut bool, settled: &mut bool) {
    ui.spacing_mut().slider_width = (ui.available_width() - 90.0).clamp(80.0, 260.0);
    let mut pct = (*v * 100.0).round();
    let r = ui.add(
        egui::Slider::new(&mut pct, 0.0..=max * 100.0)
            .step_by(5.0)
            .fixed_decimals(0)
            .suffix(" %"),
    );
    if r.changed() {
        *v = pct / 100.0;
        *changed = true;
    }
    *settled |= r.drag_stopped() || (r.changed() && !r.dragged());
}

fn body(ui: &mut Ui, shared: &mut Shared) {
    let mut s = current(shared);
    let mut changed = false;
    let mut settled = false;

    chrome::row(ui, "Jiggle", |ui| {
        if ui.checkbox(&mut s.enabled, "on").changed() {
            changed = true;
            settled = true;
        }
        ui.add_space(8.0);
        if chrome::stone_button(ui, "Reset")
            .on_hover_text("Back to the default: 150 % overall, every region 120 %")
            .clicked()
        {
            s = JiggleSettings::default();
            changed = true;
            settled = true;
        }
    });
    chrome::row(ui, "Overall", |ui| {
        pct_slider(
            ui,
            &mut s.multiplier,
            MAX_MULTIPLIER,
            &mut changed,
            &mut settled,
        )
    });
    for r in Region::ALL {
        let mut v = s.region(r);
        chrome::row(ui, r.label(), |ui| {
            pct_slider(ui, &mut v, MAX_REGION, &mut changed, &mut settled)
        });
        s.set_region(r, v);
    }
    chrome::row(ui, "Build-up", |ui| {
        pct_slider(ui, &mut s.build_up, 1.0, &mut changed, &mut settled)
    });
    hint_row(
        ui,
        "how much bouncing, hanging and running builds the jiggle up",
    );
    chrome::row(ui, "Layers", |ui| {
        let mut n = i32::from(s.layers);
        let r = ui.add(egui::Slider::new(&mut n, 1..=MAX_LAYERS as i32));
        if r.changed() {
            s.layers = n.clamp(1, MAX_LAYERS as i32) as u8;
            changed = true;
        }
        settled |= r.drag_stopped() || (r.changed() && !r.dragged());
    });
    chrome::row(ui, "Zoom-ins", |ui| {
        if ui
            .checkbox(&mut s.zoom_follow, "scale with close-up framing")
            .changed()
        {
            changed = true;
            settled = true;
        }
    });
    hint_row(ui, "caps keep every region attached at any setting");

    if changed {
        stage(shared, s);
    }
    let id = egui::Id::new("jiggle-live-write");
    let last: f64 = ui
        .ctx()
        .data(|d| d.get_temp(id))
        .unwrap_or(f64::NEG_INFINITY);
    if pending(shared) {
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
            std::env::temp_dir().join(format!("lillith-jiggle-tab-{}-{name}", std::process::id()));
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
    fn old_config_shows_the_defaults() {
        let (_, s) = shared("old");
        assert_eq!(current(&s), JiggleSettings::default());
    }

    #[test]
    fn live_push_writes_only_jiggle_and_reloads() {
        let (p, mut s) = shared("live");
        s.store.set_str(&["mode"], "wander");
        let mut j = JiggleSettings {
            multiplier: 2.5,
            layers: 2,
            build_up: 0.3,
            ..Default::default()
        };
        j.set_region(Region::Tail, 1.75);
        j.set_region(Region::Chest, 9.0); // clamped
        stage(&mut s, j);
        assert!(push_live(&mut s));
        assert_eq!(s.picks_out, vec![MenuPick::ReloadConfig]);
        assert_eq!(s.store.pending(), 1, "mode waits for Save");
        let cfg = lillith_core::config::Config::load(&p).unwrap();
        let g = cfg.springs.jiggle;
        assert_eq!(g.multiplier, 2.5);
        assert_eq!(g.layers, 2);
        assert_eq!(g.build_up, 0.3);
        assert_eq!(g.tail, 1.75);
        assert_eq!(g.chest, MAX_REGION);
        assert_eq!(cfg.mode, lillith_core::config::MoveMode::Stay);
        assert_eq!(current(&s), g);
    }
}
