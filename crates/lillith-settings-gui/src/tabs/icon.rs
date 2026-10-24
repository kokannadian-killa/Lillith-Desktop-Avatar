//! Icon: pick Lillith's app icon from the set in `branding/icons.toml`.
//!
//! A click stages `icon = "<id>"`, saves config.toml and applies the icon
//! to the desktop ([`lillith_core::icons::apply`], on a worker thread):
//! Linux menu entry + desktop shortcut, Windows Start-menu / Desktop
//! shortcuts. This window's own icon changes right away.
//!
//! Windows: re-pointing the shortcuts runs PowerShell, so it needs Ko's
//! Yes on a Soul Contract card first (mouse only: the popup's Yes reacts
//! to a primary click alone). The tab first shows the whole command and
//! script (the popup itself shows only 9 lines), then the card. No, Esc
//! or leaving it unanswered changes nothing; every decision is audited
//! (`PROPOSED` / `CONFIRM` / `DENY`, `card=<id>`).

use super::{SettingsTab, Shared};
use crate::chrome;
use egui::{pos2, vec2, Align2, RichText, Sense};
use lillith_core::icons::{self, IconInfo};
use lillith_core::launch_audit;
use lillith_theme::egui_contract::{soul_contract_popup, ContractChoice, ContractView};
use lillith_theme::egui_theme::{self as th, c32};
use lillith_theme::geom;
use lillith_theme::palette::{self as pal, Rgba};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;

const THUMB: f32 = 96.0;

type Job = mpsc::Receiver<(String, Result<icons::Applied, String>)>;

pub struct IconTab {
    root: Option<PathBuf>,
    thumbs: HashMap<String, Option<egui::TextureHandle>>,
    job: Option<Job>,
    /// The Windows shortcut rewrite waiting for Ko's Yes.
    card: Option<ShortcutCard>,
}

/// A pending Windows shortcut card.
struct ShortcutCard {
    id: String,
    card: String,
    detail: String,
    /// The full command was read: show the popup.
    reviewed: bool,
}

fn audit(tag: &str, card: &str) {
    if let Some(log) = lillith_core::config::audit_log_path() {
        launch_audit::record_to(
            &log,
            tag,
            icons::ICON_CARD_KIND,
            &icons::windows_shortcut_argv(),
            Some(card),
        );
    }
}

impl IconTab {
    pub fn new() -> Self {
        Self {
            root: icons::icons_dir(),
            thumbs: HashMap::new(),
            job: None,
            card: None,
        }
    }

    fn current(shared: &Shared) -> &'static IconInfo {
        icons::resolve(shared.store.get_str(&["icon"]).as_deref())
    }

    fn thumb(&mut self, ctx: &egui::Context, id: &str) -> Option<egui::TextureHandle> {
        if let Some(t) = self.thumbs.get(id) {
            return t.clone();
        }
        let t = self
            .root
            .as_ref()
            .and_then(|r| icons::thumbnail(r, id, THUMB as u32))
            .and_then(|p| image::open(p).ok())
            .map(|img| {
                let img = img.to_rgba8();
                let ci = egui::ColorImage::from_rgba_unmultiplied(
                    [img.width() as usize, img.height() as usize],
                    img.as_raw(),
                );
                ctx.load_texture(format!("icon-{id}"), ci, egui::TextureOptions::LINEAR)
            });
        self.thumbs.insert(id.to_string(), t.clone());
        t
    }

    fn pick(&mut self, ctx: &egui::Context, shared: &mut Shared, info: &'static IconInfo) {
        shared.store.set_str(&["icon"], info.id.clone());
        shared.want_save = true;
        if let Some(icon) = crate::window_icon(&info.id) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Icon(Some(icon)));
        }
        let Some(root) = self.root.clone() else {
            shared.toast(
                "icon saved; icon files not found, so the desktop keeps the old one",
                pal::WARN,
            );
            return;
        };
        if cfg!(windows) {
            let card = lillith_core::card_process::new_card_id("icon");
            audit("PROPOSED", &card);
            self.card = Some(ShortcutCard {
                id: info.id.clone(),
                detail: icons::windows_shortcut_detail(&icons::ico_path(&root, &info.id)),
                card,
                reviewed: false,
            });
            return;
        }
        let (tx, rx) = mpsc::channel();
        let id = info.id.clone();
        std::thread::spawn(move || {
            let r = icons::apply(&root, &id);
            let _ = tx.send((id, r));
        });
        self.job = Some(rx);
    }

    /// The Windows card: full command first, then the mouse-only popup.
    fn shortcut_card(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let Some(c) = &mut self.card else { return };
        if !c.reviewed {
            let mut go_on = false;
            let mut cancel = false;
            chrome::section(
                ui,
                "SOUL CONTRACT · SHORTCUTS",
                Some("needs your Yes"),
                |ui| {
                    chrome::hint(
                        ui,
                        "Changing the Windows shortcuts runs this PowerShell command:",
                    );
                    egui::ScrollArea::both().max_height(260.0).show(ui, |ui| {
                        ui.add(egui::Label::new(RichText::new(&c.detail).monospace()).extend());
                    });
                    ui.horizontal(|ui| {
                        // Only moves on to the card; approves nothing.
                        go_on = chrome::stone_button(ui, "Continue to the contract").clicked();
                        cancel = chrome::stone_button(ui, "Keep the old shortcuts").clicked();
                    });
                },
            );
            if go_on {
                c.reviewed = true;
            }
            if cancel {
                audit("DENY", &c.card);
                self.card = None;
            }
            return;
        }
        let detail = format!("{}\n(the full script was shown before this card)", c.detail);
        let view = ContractView {
            title: "CHANGE SHORTCUTS",
            summary: "re-point the Start-menu / Desktop shortcuts at the new icon (PowerShell)",
            target: Some("Start menu, Desktop, Startup: lillith*.lnk"),
            detail: Some(&detail),
            initiator: lillith_theme::egui_contract::Initiator::You,
            ..Default::default()
        };
        match soul_contract_popup(ui.ctx(), egui::Id::new("icon-soul-contract"), &view) {
            Some(ContractChoice::Yes) => {
                let Some(c) = self.card.take() else { return };
                audit("CONFIRM", &c.card);
                let Some(root) = self.root.clone() else {
                    return;
                };
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let r = icons::apply_windows_shortcuts(&root, &c.id, &c.card);
                    let _ = tx.send((c.id, r));
                });
                self.job = Some(rx);
            }
            Some(ContractChoice::No) => {
                if let Some(c) = self.card.take() {
                    audit("DENY", &c.card);
                }
                shared.toast("icon saved; shortcuts left as they were", pal::WARN);
            }
            None => {}
        }
    }
}

impl Default for IconTab {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsTab for IconTab {
    fn id(&self) -> &str {
        "icon"
    }
    fn title(&self) -> &str {
        "Icon"
    }
    fn rune(&self) -> usize {
        9
    }
    fn busy(&self) -> bool {
        self.job.is_some() || self.card.is_some()
    }
    fn tick(&mut self, _ctx: &egui::Context, shared: &mut Shared) {
        let Some(rx) = &self.job else { return };
        match rx.try_recv() {
            Ok((id, res)) => {
                self.job = None;
                let name = icons::find(&id).map_or(id.as_str(), |i| i.name.as_str());
                match res {
                    Ok(_) => shared.toast(
                        format!("† {name} is her icon now (menu, shortcuts)"),
                        pal::OK,
                    ),
                    Err(e) => shared.toast(format!("⚠ icon: {e}"), pal::BLOOD_SHEEN),
                }
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => self.job = None,
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let ctx = ui.ctx().clone();
        let current = Self::current(shared);
        let mut picked: Option<&'static IconInfo> = None;
        chrome::section(ui, "SIGIL · HER_FACE_ON_YOUR_DESKTOP", Some("icon"), |ui| {
            chrome::hint(
                ui,
                "The icon of her menu entry and desktop / Start-menu shortcuts. Click one to use it.",
            );
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                for info in icons::all() {
                    let tex = self.thumb(&ctx, &info.id);
                    let on = info.id == current.id;
                    let r = icon_card(ui, tex.as_ref(), info, on, shared.time);
                    if r.clicked() && !on {
                        picked = Some(info);
                    }
                }
            });
            ui.add_space(6.0);
            let def = if current.id == icons::DEFAULT_ICON {
                " (default)"
            } else {
                ""
            };
            ui.label(
                RichText::new(format!("current: {}{def}", current.name)).color(c32(pal::TEXT_DIM)),
            );
            if self.root.is_none() {
                ui.label(
                    RichText::new("Icon files not found (set LILLITH_ICONS_DIR or reinstall).")
                        .small()
                        .color(c32(pal::WARN)),
                );
            }
            #[cfg(target_os = "windows")]
            chrome::hint(
                ui,
                "Windows: the shortcuts change; lillith.exe itself keeps Phantomcore Red.",
            );
        });
        if let Some(info) = picked {
            if self.card.is_none() {
                self.pick(&ctx, shared, info);
            }
        }
        self.shortcut_card(ui, shared);
    }
}

/// A chamfered card with the icon and its name.
fn icon_card(
    ui: &mut egui::Ui,
    tex: Option<&egui::TextureHandle>,
    info: &IconInfo,
    on: bool,
    time: f64,
) -> egui::Response {
    let size = vec2(THUMB + 24.0, THUMB + 40.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hover = ui.ctx().animate_bool(resp.id.with("h"), resp.hovered());
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
    match tex {
        Some(t) => {
            p.image(
                t.id(),
                img,
                egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        None => {
            p.rect_filled(img, 18.0, c32(pal::IRON));
        }
    }
    let tc = if on {
        Rgba::hex(0xffffff)
    } else {
        pal::TEXT_DIM.lerp(Rgba::hex(0xffffff), hover)
    };
    p.text(
        pos2(rect.center().x, rect.bottom() - 15.0),
        Align2::CENTER_CENTER,
        &info.name,
        th::bold_font(12.0),
        c32(tc),
    );
    resp.on_hover_text(format!("{} ({})", info.name, info.id))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}
