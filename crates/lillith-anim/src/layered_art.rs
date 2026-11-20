//! Region layer art (`lillith.layered_art.v1`, docs/LAYERED_ART.md): a clip
//! split into a base sheet plus movable layers (covered chest, each
//! pigtail) that the layered jiggle ([`lillith_core::jiggle`]) moves
//! independently instead of squashing the whole frame.
//!
//! A clip directory holds `<clip>.layers.json` next to the shipped sheet;
//! [`LayeredArt::discover`] finds it by its `source_sheet` (so aliases that
//! share the sheet, e.g. `idle` → `idle_a`, pick it up too). No manifest
//! change is needed.
//!
//! Binding (per layer `spring` / `spring_sign`):
//!
//! * `chest` → vertical translation by the chest jiggle (× sign);
//! * `pigtails` → a swing (rotation about the layer's per-frame root
//!   pivot): the body-driven sway (same direction for both) plus a
//!   mirrored flap (× sign) that opens and closes with the chest bounce
//!   ([`PIGTAIL_FLAP_PER_CHEST`]); the tip travels the jiggle offset;
//! * anything else / `movable = false` → drawn unmoved.
//!
//! **Seam-safe limits.** Every move is clamped to the layer's
//! `max_offset_px` slack *and* to the range the pilot fill really holds
//! without opening the underbust / pigtail-root fringe
//! ([`CHEST_SAFE_UP_PX`], [`CHEST_SAFE_DOWN_PX`], [`PIGTAIL_SAFE_DEG`],
//! measured on all 72 `idle_a` frames with tools/layered-art/
//! preview_layers.py: 0 crack px at these caps, combined; the declared
//! 10 / 16 px slack itself cracks from about 2 px of slide). The
//! part of the jiggle past that range is not lost: the animator folds it
//! into the whole-sprite squash/skew ([`Layer::safe_px`]).

use crate::atlas::Clip;
use lillith_core::jiggle::{JiggleOffsets, Region};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Schema string this reader understands.
pub const SCHEMA: &str = "lillith.layered_art.v1";
/// Pigtail flap (px of mirrored spread) per px of chest bounce.
pub const PIGTAIL_FLAP_PER_CHEST: f32 = 0.6;
/// Chest layer lift (px) the pilot fill holds without a visible gap.
pub const CHEST_SAFE_UP_PX: f32 = 1.0;
/// Chest layer drop (px) the pilot fill holds without a visible gap.
pub const CHEST_SAFE_DOWN_PX: f32 = 1.5;
/// Pigtail swing (degrees about the root) the pilot fill holds.
pub const PIGTAIL_SAFE_DEG: f32 = 2.5;
/// Root-to-tip lever (art px) that turns a pigtail offset into a swing.
pub const PIGTAIL_LEVER_PX: f32 = 110.0;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LayerSpec {
    pub id: String,
    #[serde(default)]
    pub sheet: Option<String>,
    #[serde(default)]
    pub z: i32,
    #[serde(default)]
    pub movable: bool,
    #[serde(default)]
    pub spring: Option<String>,
    #[serde(default = "one")]
    pub spring_sign: f32,
    #[serde(default)]
    pub max_offset_px: Option<f32>,
    /// Per-frame pivot name (`pivots.<name>`).
    #[serde(default)]
    pub pivot: Option<String>,
}

fn one() -> f32 {
    1.0
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct FrameHints {
    #[serde(default)]
    pub pivots: BTreeMap<String, [f32; 2]>,
    #[serde(default)]
    pub torso_vel_px_s: Option<[f32; 2]>,
    #[serde(default)]
    pub torso_acc_px_s2: Option<[f32; 2]>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct Raw {
    schema: String,
    clip: String,
    #[serde(default)]
    source_sheet: Option<String>,
    canvas: [u32; 2],
    frame_count: usize,
    #[serde(default)]
    layout: Option<String>,
    #[serde(default)]
    fill_slack_px: BTreeMap<String, f32>,
    layers: Vec<LayerSpec>,
    #[serde(default)]
    frames: Vec<FrameHints>,
}

/// Why a layers file was not used.
#[derive(Debug, thiserror::Error)]
pub enum LayeredArtError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(String),
}

/// What a movable layer follows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Binding {
    Static,
    Chest { sign: f32 },
    Pigtail { sign: f32 },
}

/// One layer ready to draw: sheet (relative to the manifest directory),
/// binding and slack.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub id: String,
    pub sheet: String,
    pub binding: Binding,
    /// Max |offset| px (the authored slack).
    pub slack: f32,
    /// Per-frame pivot name.
    pub pivot: Option<String>,
}

/// How a layer is moved this frame (cell px / degrees, about `pivot`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LayerMove {
    pub dx: f32,
    pub dy: f32,
    pub angle_deg: f32,
}

impl Layer {
    /// Region whose jiggle moves this layer (`None` = static).
    pub fn region(&self) -> Option<Region> {
        match self.binding {
            Binding::Static => None,
            Binding::Chest { .. } => Some(Region::Chest),
            Binding::Pigtail { .. } => Some(Region::Pigtails),
        }
    }

    /// Largest |region offset| px this layer shows; the rest of the
    /// jiggle is folded into the whole-sprite transform.
    pub fn safe_px(&self) -> f32 {
        match self.binding {
            Binding::Static => 0.0,
            Binding::Chest { .. } => CHEST_SAFE_DOWN_PX.max(CHEST_SAFE_UP_PX).min(self.slack),
            Binding::Pigtail { .. } => {
                (PIGTAIL_LEVER_PX * PIGTAIL_SAFE_DEG.to_radians().tan()).min(self.slack)
            }
        }
    }

    /// The move for jiggle `j`, clamped to the slack and the seam-safe range.
    pub fn motion(&self, j: &JiggleOffsets) -> LayerMove {
        let fin = |v: f32| if v.is_finite() { v } else { 0.0 };
        match self.binding {
            Binding::Static => LayerMove::default(),
            Binding::Chest { sign } => {
                let lo = -CHEST_SAFE_UP_PX.min(self.slack);
                let hi = CHEST_SAFE_DOWN_PX.min(self.slack);
                LayerMove {
                    dy: fin(sign * j.chest).clamp(lo, hi),
                    ..Default::default()
                }
            }
            Binding::Pigtail { sign } => {
                let tip = fin(j.pigtails + sign * PIGTAIL_FLAP_PER_CHEST * fin(j.chest))
                    .clamp(-self.slack, self.slack);
                // y-down: a clockwise (+) turn swings a hanging tip left.
                let deg = (-tip / PIGTAIL_LEVER_PX).atan().to_degrees();
                LayerMove {
                    angle_deg: deg.clamp(-PIGTAIL_SAFE_DEG, PIGTAIL_SAFE_DEG),
                    ..Default::default()
                }
            }
        }
    }
}

/// A clip's layered art (z-ordered, bottom first).
#[derive(Debug, Clone, PartialEq)]
pub struct LayeredArt {
    pub clip: String,
    pub cell: (u32, u32),
    pub frame_count: usize,
    pub layers: Vec<Layer>,
    pub frames: Vec<FrameHints>,
}

impl LayeredArt {
    /// Parses a layers file; `dir_rel` is its directory relative to the
    /// manifest directory (sheet paths are made manifest-relative).
    pub fn from_json_str(s: &str, dir_rel: &str) -> Result<Self, LayeredArtError> {
        let r: Raw = serde_json::from_str(s)?;
        let bad = |m: String| Err(LayeredArtError::Invalid(m));
        if r.schema != SCHEMA {
            return bad(format!("schema `{}` (want {SCHEMA})", r.schema));
        }
        if r.layout.as_deref().is_some_and(|l| l != "horizontal_strip") {
            return bad(format!("layout {:?} unsupported", r.layout));
        }
        if r.frame_count == 0 || r.canvas[0] == 0 || r.canvas[1] == 0 {
            return bad("empty canvas or no frames".into());
        }
        let mut specs = r.layers.clone();
        specs.sort_by_key(|l| l.z);
        let mut layers = Vec::new();
        for l in specs {
            let Some(sheet) = l.sheet.clone() else {
                return bad(format!("layer `{}` has no sheet", l.id));
            };
            if sheet.contains("..") || Path::new(&sheet).is_absolute() {
                return bad(format!("layer `{}` sheet escapes the clip dir", l.id));
            }
            let sign = if l.spring_sign.is_finite() && l.spring_sign < 0.0 {
                -1.0
            } else {
                1.0
            };
            let binding = match (l.movable, l.spring.as_deref()) {
                (true, Some("chest")) => Binding::Chest { sign },
                (true, Some("pigtails" | "pigtail" | "pig")) => Binding::Pigtail { sign },
                _ => Binding::Static,
            };
            let slack = l
                .max_offset_px
                .or_else(|| r.fill_slack_px.get(&l.id).copied())
                .filter(|v| v.is_finite() && *v >= 0.0)
                .unwrap_or(0.0);
            let sheet = if dir_rel.is_empty() {
                sheet
            } else {
                format!("{}/{sheet}", dir_rel.trim_end_matches('/'))
            };
            layers.push(Layer {
                pivot: l.pivot.clone().or_else(|| Some(l.id.clone())),
                id: l.id,
                sheet,
                binding,
                slack,
            });
        }
        if !layers.iter().any(|l| l.binding == Binding::Static) {
            return bad("no static base layer".into());
        }
        let _ = r.source_sheet;
        Ok(Self {
            clip: r.clip,
            cell: (r.canvas[0], r.canvas[1]),
            frame_count: r.frame_count,
            layers,
            frames: r.frames,
        })
    }

    /// Finds the layers file for `clip` under `manifest_dir`: a
    /// `*.layers.json` in the clip page's directory whose `source_sheet` is
    /// that page, with a matching frame count and cell size.
    pub fn discover(manifest_dir: &Path, clip: &Clip) -> Option<Self> {
        let page = Path::new(&clip.page);
        let page_file = page.file_name()?.to_str()?;
        let dir_rel = page.parent().map(Path::to_path_buf).unwrap_or_default();
        let dir = manifest_dir.join(&dir_rel);
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(".layers.json"))
            })
            .collect();
        files.sort();
        let rel = dir_rel.to_string_lossy().replace('\\', "/");
        for f in files {
            let Ok(text) = std::fs::read_to_string(&f) else {
                continue;
            };
            let src: Option<String> = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v.get("source_sheet")?.as_str().map(str::to_string));
            if src.as_deref() != Some(page_file) {
                continue;
            }
            match Self::from_json_str(&text, &rel) {
                Ok(a) if a.fits(clip) => return Some(a),
                Ok(_) => tracing::warn!(file = %f.display(), "layered art does not fit its clip"),
                Err(e) => tracing::warn!(file = %f.display(), error = %e, "layered art skipped"),
            }
        }
        None
    }

    /// Same frame count and cell size as `clip`.
    pub fn fits(&self, clip: &Clip) -> bool {
        self.frame_count == clip.frames.len() && clip.frames.iter().all(|r| (r.w, r.h) == self.cell)
    }

    /// Cell rect of frame `i` in a layer sheet (horizontal strip).
    pub fn cell_rect(&self, i: usize) -> (u32, u32, u32, u32) {
        let i = i.min(self.frame_count - 1) as u32;
        (i * self.cell.0, 0, self.cell.0, self.cell.1)
    }

    /// Regions moved by a layer here, with the px each layer shows (the
    /// animator folds only the rest into the whole-sprite transform).
    pub fn regions(&self) -> Vec<(Region, f32)> {
        let mut v: Vec<(Region, f32)> = Vec::new();
        for l in &self.layers {
            if let Some(r) = l.region() {
                match v.iter_mut().find(|(q, _)| *q == r) {
                    Some(e) => e.1 = e.1.min(l.safe_px()),
                    None => v.push((r, l.safe_px())),
                }
            }
        }
        v
    }

    /// Pivot `name` of frame `i` (cell px), if the art has it.
    pub fn pivot(&self, i: usize, name: &str) -> Option<(f32, f32)> {
        let p = self.frames.get(i)?.pivots.get(name)?;
        (p[0].is_finite() && p[1].is_finite()).then_some((p[0], p[1]))
    }

    /// The art's own torso velocity at frame `i` (px/s, +y down), if hinted.
    pub fn torso_velocity(&self, i: usize) -> Option<(f32, f32)> {
        let v = self.frames.get(i)?.torso_vel_px_s?;
        (v[0].is_finite() && v[1].is_finite()).then_some((v[0], v[1]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"{
      "schema": "lillith.layered_art.v1", "clip": "idle_a",
      "source_sheet": "idle_a_sheet.png", "canvas": [400, 400], "frame_count": 2,
      "layout": "horizontal_strip",
      "fill_slack_px": {"chest": 10, "pigtail_left": 16, "pigtail_right": 16},
      "layers": [
        {"id": "pigtail_right", "sheet": "layers/pigtail_r_sheet.png", "z": 3, "movable": true, "spring": "pigtails", "spring_sign": 1.0, "max_offset_px": 16},
        {"id": "base", "sheet": "layers/base_sheet.png", "z": 0, "movable": false, "spring": null},
        {"id": "chest", "sheet": "layers/chest_sheet.png", "z": 1, "movable": true, "spring": "chest", "spring_sign": 1.0, "max_offset_px": 10},
        {"id": "pigtail_left", "sheet": "layers/pigtail_l_sheet.png", "z": 2, "movable": true, "spring": "pigtails", "spring_sign": -1.0}
      ],
      "frames": [{"pivots": {"chest": [193.9, 134.1]}, "torso_vel_px_s": [3.0, 11.0]}, {}]
    }"#;

    #[test]
    fn parses_z_order_bindings_and_slack() {
        let a = LayeredArt::from_json_str(DOC, "idle_a").unwrap();
        let ids: Vec<_> = a.layers.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["base", "chest", "pigtail_left", "pigtail_right"]);
        assert_eq!(a.layers[0].binding, Binding::Static);
        assert_eq!(a.layers[0].sheet, "idle_a/layers/base_sheet.png");
        assert_eq!(a.layers[1].binding, Binding::Chest { sign: 1.0 });
        assert_eq!(a.layers[1].slack, 10.0);
        assert_eq!(a.layers[2].binding, Binding::Pigtail { sign: -1.0 });
        assert_eq!(a.layers[2].slack, 16.0, "fill_slack_px fallback");
        let regs = a.regions();
        assert_eq!(regs.len(), 2);
        assert_eq!(regs[0], (Region::Chest, CHEST_SAFE_DOWN_PX));
        assert_eq!(regs[1].0, Region::Pigtails);
        assert!(regs[1].1 > 3.0 && regs[1].1 < 16.0);
        assert_eq!(a.layers[2].pivot.as_deref(), Some("pigtail_left"));
        assert_eq!(a.pivot(0, "chest"), Some((193.9, 134.1)));
        assert_eq!(a.pivot(1, "chest"), None);
        assert_eq!(a.cell_rect(1), (400, 0, 400, 400));
        assert_eq!(a.cell_rect(9), (400, 0, 400, 400));
        assert_eq!(a.torso_velocity(0), Some((3.0, 11.0)));
        assert_eq!(a.torso_velocity(1), None);
    }

    #[test]
    fn moves_follow_bindings_and_stay_seam_safe() {
        let a = LayeredArt::from_json_str(DOC, "").unwrap();
        let j = JiggleOffsets {
            chest: 1.5,
            hips: 0.0,
            pigtails: 3.0,
            tail: 0.0,
        };
        assert_eq!(a.layers[0].motion(&j), LayerMove::default());
        assert_eq!(a.layers[1].motion(&j).dy, 1.5);
        // sway 3 + mirrored flap 0.6*1.5: left tip 2.1, right tip 3.9 px
        let (l, r) = (a.layers[2].motion(&j), a.layers[3].motion(&j));
        let tip = |m: LayerMove| -(m.angle_deg.to_radians().tan()) * PIGTAIL_LEVER_PX;
        assert!((tip(l) - 2.1).abs() < 1e-3 && (tip(r) - 3.9).abs() < 1e-3);
        assert_eq!((l.dx, l.dy), (0.0, 0.0), "pigtails swing, never slide");
        let huge = JiggleOffsets {
            chest: 999.0,
            hips: 0.0,
            pigtails: -999.0,
            tail: f32::NAN,
        };
        assert_eq!(a.layers[1].motion(&huge).dy, CHEST_SAFE_DOWN_PX);
        let up = JiggleOffsets {
            chest: -999.0,
            ..huge
        };
        assert_eq!(a.layers[1].motion(&up).dy, -CHEST_SAFE_UP_PX);
        for l in &a.layers[2..] {
            assert!(l.motion(&huge).angle_deg.abs() <= PIGTAIL_SAFE_DEG);
            assert!(l.motion(&up).angle_deg.is_finite());
        }
    }

    #[test]
    fn rejects_bad_files() {
        let bad = |s: String| LayeredArt::from_json_str(&s, "").is_err();
        assert!(bad(DOC.replace("layered_art.v1", "layered_art.v9")));
        assert!(bad(DOC.replace("horizontal_strip", "grid")));
        assert!(bad(DOC.replace("layers/base_sheet.png", "../../etc/x.png")));
        assert!(bad(DOC
            .replace(
                "\"movable\": false",
                "\"movable\": true, \"spring\": \"chest\""
            )
            .replace(
                "\"spring\": \"chest\", \"spring\": null",
                "\"spring\": \"chest\""
            )));
        assert!(bad("{}".into()));
    }

    #[test]
    fn discover_matches_the_page_and_frame_count() {
        let root = std::env::temp_dir().join(format!("lillith-layers-{}", std::process::id()));
        let dir = root.join("idle_a");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("idle_a.layers.json"), DOC).unwrap();
        let m = crate::atlas::Manifest::from_toml_str(
            r#"
            [clips.idle]
            page = "idle_a/idle_a_sheet.png"
            frames = [ { x = 0, y = 0, w = 400, h = 400 }, { x = 400, y = 0, w = 400, h = 400 } ]
            [clips.other]
            page = "idle_a/other_sheet.png"
            frames = [ { x = 0, y = 0, w = 400, h = 400 }, { x = 400, y = 0, w = 400, h = 400 } ]
            [clips.short]
            page = "idle_a/idle_a_sheet.png"
            frames = [ { x = 0, y = 0, w = 400, h = 400 } ]
            "#,
        )
        .unwrap();
        let a = LayeredArt::discover(&root, &m.clips["idle"]).expect("alias finds it");
        assert_eq!(a.layers[0].sheet, "idle_a/layers/base_sheet.png");
        assert!(LayeredArt::discover(&root, &m.clips["other"]).is_none());
        assert!(LayeredArt::discover(&root, &m.clips["short"]).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn bundled_idle_a_pilot_parses_and_stays_seam_safe() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/generated");
        let json = std::fs::read_to_string(dir.join("idle_a/idle_a.layers.json"))
            .expect("art/layered-chest-pigtail-1 pilot (idle_a.layers.json)");
        let a = LayeredArt::from_json_str(&json, "idle_a").unwrap();
        assert_eq!((a.frame_count, a.cell), (72, (400, 400)));
        let ids: Vec<&str> = a.layers.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["base", "chest", "pigtail_left", "pigtail_right"]);
        assert_eq!(a.layers[0].binding, Binding::Static);
        assert_eq!(a.layers[1].binding, Binding::Chest { sign: 1.0 });
        assert_eq!(a.layers[2].binding, Binding::Pigtail { sign: -1.0 });
        assert_eq!(a.layers[3].binding, Binding::Pigtail { sign: 1.0 });
        assert_eq!(a.layers[1].sheet, "idle_a/layers/chest_sheet.png");
        assert_eq!((a.layers[1].slack, a.layers[2].slack), (10.0, 16.0));
        let regs: Vec<Region> = a.regions().into_iter().map(|(r, _)| r).collect();
        assert_eq!(
            regs,
            [Region::Chest, Region::Pigtails],
            "no hips/tail layers yet"
        );
        for i in 0..72 {
            for l in &a.layers[1..] {
                assert!(
                    a.pivot(i, l.pivot.as_deref().unwrap()).is_some(),
                    "{} {i}",
                    l.id
                );
            }
            assert!(a.torso_velocity(i).is_some());
        }
        let max = JiggleOffsets {
            chest: 60.0,
            hips: 0.0,
            pigtails: -90.0,
            tail: 0.0,
        };
        for l in &a.layers {
            let m = l.motion(&max);
            assert!(m.dy <= CHEST_SAFE_DOWN_PX && m.dy >= -CHEST_SAFE_UP_PX);
            assert!(m.angle_deg.abs() <= PIGTAIL_SAFE_DEG && m.dx == 0.0);
        }
    }
}
