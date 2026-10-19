//! Poses tab: the M2 clip browser. Lists every manifest clip, plays it
//! through the [`Animator`] (springs, breathing, optional idle life) or steps
//! raw frames, and can fire a clip as a reaction over idle.

use eframe::egui;
use lillith_anim::animator::{Animator, AnimatorConfig, Dir};
use lillith_anim::atlas::{default_manifest_path, Manifest};
use lillith_anim::render::{draw_cell, render_frame, rgba_from_pixmap, PageCache};
use lillith_anim::Canvas;
use lillith_core::spring::{Channel, Transform};
use std::path::PathBuf;

const VIEW: u32 = 400;

/// Clip browser state.
pub struct Poses {
    pub manifest_path: Option<PathBuf>,
    anim: Option<Animator>,
    pages: Option<PageCache>,
    error: Option<String>,
    selected: Option<String>,
    filter: String,
    playing: bool,
    idle_life: bool,
    manual_frame: usize,
    texture: Option<egui::TextureHandle>,
}

impl Poses {
    pub fn new(manifest_path: Option<PathBuf>) -> Self {
        let mut p = Self {
            manifest_path: manifest_path.or_else(default_manifest_path),
            anim: None,
            pages: None,
            error: None,
            selected: None,
            filter: String::new(),
            playing: true,
            idle_life: false,
            manual_frame: 0,
            texture: None,
        };
        p.reload();
        p
    }

    /// (Re)loads the manifest.
    pub fn reload(&mut self) {
        self.anim = None;
        self.error = None;
        let Some(path) = self.manifest_path.clone() else {
            self.error = Some(
                "no manifest.toml (set LILLITH_MANIFEST or pass `lillith panel --manifest …`)"
                    .into(),
            );
            return;
        };
        match Manifest::load(&path)
            .map_err(|e| e.to_string())
            .and_then(|m| {
                Animator::new(
                    m,
                    AnimatorConfig {
                        idle_enabled: self.idle_life,
                        ..AnimatorConfig::default()
                    },
                )
                .map_err(|e| e.to_string())
            }) {
            Ok(a) => {
                self.selected = Some(a.base().to_string());
                self.pages = Some(PageCache::new(
                    path.parent().map(PathBuf::from).unwrap_or_default(),
                ));
                self.anim = Some(a);
            }
            Err(e) => self.error = Some(format!("{}: {e}", path.display())),
        }
    }

    /// Number of clips loaded (0 if none).
    pub fn clip_count(&self) -> usize {
        self.anim.as_ref().map_or(0, |a| a.manifest().clips.len())
    }

    fn select(&mut self, name: &str) {
        self.selected = Some(name.to_string());
        self.manual_frame = 0;
        if let Some(a) = &mut self.anim {
            a.clear_overlays();
            let _ = a.set_base(name, false);
        }
    }

    fn render(&mut self) -> Option<Canvas> {
        let (anim, pages) = (self.anim.as_ref()?, self.pages.as_mut()?);
        if self.playing {
            let f = anim.current();
            let a = f.rect.anchor;
            render_frame(pages, &f, VIEW, VIEW, (a.0 as f32, a.1 as f32), 1.0).ok()
        } else {
            let name = self.selected.as_ref()?;
            let clip = anim.manifest().clips.get(name)?;
            let i = self.manual_frame.min(clip.frames.len() - 1);
            let cell = pages.cell(clip, i).ok()?;
            let a = clip.frames[i].anchor;
            let mut c = Canvas::new(VIEW, VIEW)?;
            draw_cell(
                &mut c,
                &cell,
                a,
                (a.0 as f32, a.1 as f32),
                false,
                &Transform::IDENTITY,
                1.0,
            );
            Some(c)
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        if let Some(e) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, e);
            if ui.button("Retry").clicked() {
                self.reload();
            }
            return;
        }
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        if self.playing {
            if let Some(a) = &mut self.anim {
                a.tick(dt);
            }
            ui.ctx().request_repaint();
        }

        let names: Vec<String> = self
            .anim
            .as_ref()
            .map(|a| a.clip_names().map(String::from).collect())
            .unwrap_or_default();
        let mut pick = None;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(190.0);
                ui.label(format!("{} clips", names.len()));
                ui.text_edit_singleline(&mut self.filter);
                egui::ScrollArea::vertical()
                    .id_salt("clips")
                    .show(ui, |ui| {
                        for n in names.iter().filter(|n| n.contains(self.filter.as_str())) {
                            let sel = self.selected.as_deref() == Some(n.as_str());
                            if ui.selectable_label(sel, n).clicked() {
                                pick = Some(n.clone());
                            }
                        }
                    });
            });
            ui.vertical(|ui| self.preview_ui(ui));
        });
        if let Some(n) = pick {
            self.select(&n);
        }
    }

    fn preview_ui(&mut self, ui: &mut egui::Ui) {
        let frames = self
            .selected
            .as_ref()
            .and_then(|n| self.anim.as_ref()?.manifest().clips.get(n))
            .map_or(1, |c| c.frames.len());
        ui.horizontal(|ui| {
            if ui
                .button(if self.playing {
                    "⏸ Pause"
                } else {
                    "▶ Play"
                })
                .clicked()
            {
                self.playing = !self.playing;
            }
            ui.add_enabled_ui(!self.playing, |ui| {
                if ui.button("◀").clicked() {
                    self.manual_frame = (self.manual_frame + frames - 1) % frames;
                }
                ui.label(format!("{}/{}", self.manual_frame + 1, frames));
                if ui.button("▶").clicked() {
                    self.manual_frame = (self.manual_frame + 1) % frames;
                }
            });
            if ui.checkbox(&mut self.idle_life, "idle life").changed() {
                if let Some(a) = &mut self.anim {
                    a.set_idle_enabled(self.idle_life);
                }
            }
        });
        ui.horizontal(|ui| {
            let sel = self.selected.clone();
            if let Some(a) = &mut self.anim {
                if ui.button("As reaction").clicked() {
                    if let Some(n) = &sel {
                        let idle = a.manifest().clips.contains_key("idle");
                        if idle {
                            let _ = a.set_base("idle", false);
                        }
                        let _ = a.react(n, 1);
                    }
                }
                if ui.button("Jiggle").clicked() {
                    a.physics_mut().impulse(Channel::Chest, 70.0);
                    a.physics_mut().impulse(Channel::Pigtail, 90.0);
                    a.physics_mut().impulse(Channel::RootY, -80.0);
                }
                if ui.button("◀ Walk").clicked() {
                    let _ = a.walk(Dir::Left, false);
                }
                if ui.button("Walk ▶").clicked() {
                    let _ = a.walk(Dir::Right, false);
                }
                if ui.button("Stand").clicked() {
                    a.stand();
                }
            }
        });
        if let Some(c) = self.render() {
            let img = rgba_from_pixmap(&c);
            let ci = egui::ColorImage::from_rgba_unmultiplied(
                [img.width() as usize, img.height() as usize],
                img.as_raw(),
            );
            match &mut self.texture {
                Some(t) => t.set(ci, egui::TextureOptions::LINEAR),
                None => {
                    self.texture = Some(ui.ctx().load_texture(
                        "pose",
                        ci,
                        egui::TextureOptions::LINEAR,
                    ))
                }
            }
        }
        if let Some(t) = &self.texture {
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(VIEW as f32, VIEW as f32), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, 4.0, egui::Color32::from_rgb(72, 72, 80));
            ui.painter().image(
                t.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        if let Some(a) = &self.anim {
            let f = a.current();
            let t = f.transform;
            ui.monospace(format!(
                "{:?} {} #{}{}\nscale {:.4} chest {:+.2} pig {:+.2} y {:+.2} angle {:+.3}",
                f.layer,
                f.clip_name,
                f.frame,
                if f.flip { " (flipped)" } else { "" },
                t.scale,
                t.chest_offset,
                t.pigtail_offset,
                t.y,
                t.angle
            ));
        }
    }
}
