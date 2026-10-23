//! Icon: pick Lillith's app icon from the set in `branding/icons.toml`.
//!
//! A click stages `icon = "<id>"`, saves config.toml and applies the icon
//! to the desktop ([`lillith_core::icons::apply`], on a worker thread):
//! Linux menu entry + desktop shortcut, Windows Start-menu / Desktop
//! shortcuts. This window's own icon changes right away.

use super::{SettingsTab, Shared};
use crate::chrome;
use egui::{pos2, vec2, Align2, RichText, Sense};
use lillith_core::icons::{self, IconInfo};
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
}

impl IconTab {
    pub fn new() -> Self {
        Self {
            root: icons::icons_dir(),
            thumbs: HashMap::new(),
            job: None,
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
        let (tx, rx) = mpsc::channel();
        let id = info.id.clone();
        std::thread::spawn(move || {
            let r = icons::apply(&root, &id);
            let _ = tx.send((id, r));
        });
        self.job = Some(rx);
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
        self.job.is_some()
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
            self.pick(&ctx, shared, info);
        }
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
