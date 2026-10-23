//! Table-driven tabs (Motion, Avatar, Voice): each knob is a config.toml
//! key with a range, rounding and a default taken from the core config.

use super::{SettingsTab, Shared};
use crate::chrome;
use egui::RichText;
use lillith_core::config::Config;
use lillith_theme::egui_theme::c32;
use lillith_theme::palette as pal;

#[derive(Clone)]
pub enum Kind {
    /// Slider `min..=max`, rounded to `decimals`, with a unit suffix.
    Float {
        min: f32,
        max: f32,
        decimals: u32,
        unit: &'static str,
        default: fn(&Config) -> f32,
    },
    Int {
        min: i64,
        max: i64,
        unit: &'static str,
        default: i64,
    },
    Bool {
        default: fn(&Config) -> bool,
    },
    /// One of a fixed set of strings.
    Choice {
        options: &'static [&'static str],
        default: &'static str,
    },
    /// Free text (paths).
    Text {
        hint: &'static str,
    },
}

#[derive(Clone)]
pub struct Knob {
    pub path: &'static [&'static str],
    pub label: &'static str,
    pub help: &'static str,
    pub kind: Kind,
}

pub struct Group {
    pub title: &'static str,
    pub tag: &'static str,
    pub knobs: Vec<Knob>,
}

pub struct KnobTab {
    id: &'static str,
    title: &'static str,
    rune: usize,
    intro: &'static str,
    groups: Vec<Group>,
    text_buf: std::collections::HashMap<String, String>,
}

#[allow(clippy::too_many_arguments)]
const fn f(
    path: &'static [&'static str],
    label: &'static str,
    help: &'static str,
    min: f32,
    max: f32,
    decimals: u32,
    unit: &'static str,
    default: fn(&Config) -> f32,
) -> Knob {
    Knob {
        path,
        label,
        help,
        kind: Kind::Float {
            min,
            max,
            decimals,
            unit,
            default,
        },
    }
}

impl KnobTab {
    pub fn motion() -> Self {
        Self {
            id: "motion",
            title: "Motion",
            rune: 18,
            intro: "How she moves on the desktop. Restart Lillith (or let the overlay reload) to apply.",
            groups: vec![
                Group {
                    title: "GAIT · WANDERING",
                    tag: "[behavior]",
                    knobs: vec![
                        f(&["behavior", "wander_speed_px_s"], "Glide speed", "Walk speed for clips without root motion", 10.0, 400.0, 0, "px/s", |c| c.behavior.wander_speed_px_s),
                        f(&["behavior", "wander_min_step_px"], "Shortest walk", "Ignore walks shorter than this", 0.0, 400.0, 0, "px", |c| c.behavior.wander_min_step_px),
                        Knob { path: &["behavior", "walk_root_motion"], label: "Feet-locked walk", help: "Move by the clip's measured root motion", kind: Kind::Bool { default: |c| c.behavior.walk_root_motion } },
                    ],
                },
                Group {
                    title: "IDLE · SLUMBER",
                    tag: "[behavior]",
                    knobs: vec![
                        f(&["behavior", "pose_cycle_s"], "Pose cycle", "Automatic pose change while idle", 5.0, 180.0, 0, "s", |c| c.behavior.pose_cycle_s),
                        f(&["behavior", "sleepy_after_s"], "Drowsy after", "No interaction → drowsy", 30.0, 3600.0, 0, "s", |c| c.behavior.sleepy_after_s),
                        f(&["behavior", "asleep_after_s"], "Asleep after", "No interaction → asleep", 60.0, 7200.0, 0, "s", |c| c.behavior.asleep_after_s),
                    ],
                },
                Group {
                    title: "SPRINGS · SECONDARY_MOTION",
                    tag: "[springs]",
                    knobs: vec![
                        f(&["springs", "jiggle_gain"], "Secondary motion", "Overall spring follow-through (tasteful)", 0.0, 1.5, 2, "×", |c| c.springs.jiggle_gain),
                        f(&["springs", "pigtail_sway_gain"], "Pigtail sway", "Hair follow-through", 0.0, 1.5, 2, "×", |c| c.springs.pigtail_sway_gain),
                    ],
                },
                Group {
                    title: "TIMING · ENGINE",
                    tag: "top-level",
                    knobs: vec![
                        f(&["sprite_fps"], "Sprite fps", "Animation frame rate", 4.0, 30.0, 0, "fps", |c| c.sprite_fps),
                        f(&["render_hz"], "Render rate", "Overlay redraw rate", 10.0, 60.0, 0, "Hz", |c| c.render_hz),
                        f(&["physics_hz"], "Physics rate", "Spring integration rate", 30.0, 240.0, 0, "Hz", |c| c.physics_hz),
                    ],
                },
            ],
            text_buf: Default::default(),
        }
    }

    pub fn avatar() -> Self {
        Self {
            id: "avatar",
            title: "Avatar",
            rune: 19,
            intro: "Her look is locked canon (black top, magenta shorts, pink thigh-highs, neon horns, spade tail). These settings change how she's shown, never what she wears.",
            groups: vec![
                Group {
                    title: "VESSEL",
                    tag: "top-level",
                    knobs: vec![
                        // No size knob: the overlay owns `scale` (Companion ▸ Resize sends set_size / scale_step).
                        f(&["opacity"], "Opacity", "Whole-avatar opacity", 0.2, 1.0, 2, "", |c| c.opacity),
                        Knob { path: &["baked_jiggle"], label: "Baked motion clips", help: "Use the pre-baked secondary-motion clip variants", kind: Kind::Bool { default: |c| c.baked_jiggle } },
                    ],
                },
                Group {
                    title: "CUSTOM_AVATAR · MOTION",
                    tag: "[avatar]",
                    knobs: vec![
                        f(&["avatar", "pace"], "Pace", "Breathing / sway / hop speed of a custom image", 0.3, 1.5, 2, "×", |c| c.avatar.pace),
                        f(&["avatar", "render_hz"], "Redraw rate", "Custom image redraw rate", 30.0, 120.0, 0, "Hz", |c| c.avatar.render_hz),
                    ],
                },
                Group {
                    title: "POSITION",
                    tag: "[overlay]",
                    knobs: vec![
                        Knob { path: &["overlay", "margin_left"], label: "From left", help: "Distance from the left screen edge", kind: Kind::Int { min: 0, max: 8000, unit: "px", default: 80 } },
                        Knob { path: &["overlay", "margin_bottom"], label: "From bottom", help: "Distance from the bottom screen edge", kind: Kind::Int { min: 0, max: 5000, unit: "px", default: 48 } },
                        Knob { path: &["overlay", "output"], label: "Monitor", help: "Output name, e.g. eDP-1 (empty = compositor's choice)", kind: Kind::Text { hint: "eDP-1" } },
                    ],
                },
            ],
            text_buf: Default::default(),
        }
    }

    pub fn voice() -> Self {
        Self {
            id: "voice",
            title: "Voice",
            rune: 2,
            intro: "Text-to-speech, all local (Piper → espeak-ng → spd-say).",
            groups: vec![
                Group {
                    title: "VOICE · ENGINE",
                    tag: "[voice]",
                    knobs: vec![
                        Knob {
                            path: &["voice", "enabled"],
                            label: "Speak",
                            help: "Master switch",
                            kind: Kind::Bool {
                                default: |c| c.voice_enabled,
                            },
                        },
                        Knob {
                            path: &["voice", "mute"],
                            label: "Muted",
                            help: "Same as Mute in the right-click menu",
                            kind: Kind::Bool {
                                default: |c| c.muted,
                            },
                        },
                        Knob {
                            path: &["voice", "engine"],
                            label: "Engine",
                            help: "auto = first installed",
                            kind: Kind::Choice {
                                options: &["auto", "piper", "espeak-ng", "spd-say", "none"],
                                default: "auto",
                            },
                        },
                        Knob {
                            path: &["voice", "model"],
                            label: "Piper model",
                            help: "Path to a Piper .onnx voice",
                            kind: Kind::Text {
                                hint: "~/.local/share/piper/voice.onnx",
                            },
                        },
                    ],
                },
                Group {
                    title: "VOICE · CADENCE",
                    tag: "[voice]",
                    knobs: vec![
                        f(
                            &["voice", "rate"],
                            "Rate",
                            "Speaking speed",
                            0.5,
                            2.0,
                            2,
                            "×",
                            |_| 1.0,
                        ),
                        f(&["voice", "volume"], "Volume", "", 0.0, 1.0, 2, "", |_| 0.8),
                        f(
                            &["voice", "cooldown_s"],
                            "Cooldown",
                            "Quiet time between lines",
                            0.0,
                            120.0,
                            0,
                            "s",
                            |_| 8.0,
                        ),
                        Knob {
                            path: &["voice", "max_chars"],
                            label: "Max length",
                            help: "Longer lines are cut",
                            kind: Kind::Int {
                                min: 40,
                                max: 1000,
                                unit: "chars",
                                default: 240,
                            },
                        },
                    ],
                },
            ],
            text_buf: Default::default(),
        }
    }

    fn knob_ui(&mut self, ui: &mut egui::Ui, k: &Knob, shared: &mut Shared, defaults: &Config) {
        let st = &mut shared.store;
        let pending = st.is_pending(k.path);
        let label = if pending {
            format!("{} •", k.label)
        } else {
            k.label.to_string()
        };
        chrome::row(ui, &label, |ui| match &k.kind {
            Kind::Float {
                min,
                max,
                decimals,
                unit,
                default,
            } => {
                let mut v = st.get_f32(k.path).unwrap_or_else(|| default(defaults));
                ui.spacing_mut().slider_width = (ui.available_width() - 90.0).clamp(80.0, 240.0);
                let r = ui.add(
                    egui::Slider::new(&mut v, *min..=*max)
                        .fixed_decimals(*decimals as usize)
                        .suffix(if unit.is_empty() {
                            String::new()
                        } else {
                            format!(" {unit}")
                        }),
                );
                if r.changed() {
                    st.set_f32(k.path, v, *decimals);
                }
            }
            Kind::Int {
                min,
                max,
                unit,
                default,
            } => {
                let mut v = st.get_i64(k.path).unwrap_or(*default);
                if ui
                    .add(
                        egui::DragValue::new(&mut v)
                            .range(*min..=*max)
                            .suffix(format!(" {unit}")),
                    )
                    .changed()
                {
                    st.set_i64(k.path, v);
                }
            }
            Kind::Bool { default } => {
                let v = st.get_bool(k.path).unwrap_or_else(|| default(defaults));
                if chrome::rune_check(ui, v, if v { "on" } else { "off" }).clicked() {
                    st.set_bool(k.path, !v);
                }
            }
            Kind::Choice { options, default } => {
                let cur = st.get_str(k.path).unwrap_or_else(|| default.to_string());
                egui::ComboBox::from_id_salt(k.path)
                    .selected_text(cur.clone())
                    .show_ui(ui, |ui| {
                        for o in *options {
                            if ui.selectable_label(cur == *o, *o).clicked() {
                                st.set_str(k.path, *o);
                            }
                        }
                    });
            }
            Kind::Text { hint } => {
                let key = k.path.join(".");
                let buf = self
                    .text_buf
                    .entry(key)
                    .or_insert_with(|| st.get_str(k.path).unwrap_or_default());
                if ui
                    .add(
                        egui::TextEdit::singleline(buf)
                            .hint_text(*hint)
                            .desired_width(260.0),
                    )
                    .changed()
                {
                    st.set_str(k.path, buf.trim());
                }
            }
        });
        if !k.help.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(142.0);
                chrome::hint(ui, k.help);
            });
        }
    }
}

impl SettingsTab for KnobTab {
    fn id(&self) -> &str {
        self.id
    }
    fn title(&self) -> &str {
        self.title
    }
    fn rune(&self) -> usize {
        self.rune
    }
    fn ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let defaults = Config::default();
        ui.label(RichText::new(self.intro).color(c32(pal::TEXT_DIM)));
        ui.add_space(6.0);
        if self.id == "motion" {
            super::bounce::section(ui, shared);
        }
        let groups = std::mem::take(&mut self.groups);
        let two = ui.available_width() > 820.0 && groups.len() > 1;
        if two {
            let half = (groups.len() + 1) / 2;
            ui.columns(2, |cols| {
                for (i, g) in groups.iter().enumerate() {
                    let ui = &mut cols[if i < half { 0 } else { 1 }];
                    chrome::section(ui, g.title, Some(g.tag), |ui| {
                        for k in &g.knobs {
                            self.knob_ui(ui, k, shared, &defaults);
                        }
                    });
                }
            });
        } else {
            for g in &groups {
                chrome::section(ui, g.title, Some(g.tag), |ui| {
                    for k in &g.knobs {
                        self.knob_ui(ui, k, shared, &defaults);
                    }
                });
            }
        }
        self.groups = groups;
    }
    fn revert(&mut self, _shared: &mut Shared) {
        self.text_buf.clear();
    }
}
