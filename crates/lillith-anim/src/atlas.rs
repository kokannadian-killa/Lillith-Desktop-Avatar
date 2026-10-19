//! Atlas + manifest loading.
//!
//! `manifest.toml` maps clip names to frame rectangles on atlas pages. It is
//! written by `tools/slice-sheets` (hand-cut source art) and by
//! `tools/import-manifest` (generated-style `manifest.json` sets).
//!
//! ```toml
//! [clips.wake]
//! page = "wake/wake_sheet.png"   # relative to the manifest's directory
//! fps = 6.0                      # fallback timing
//! mode = "once"                  # "loop" (default) | "once"
//! frame_durations_ms = [450, 350, 160]  # optional, one per frame; overrides fps
//! baseline_y = 392               # optional: ground line inside each cell
//! scale = 0.54                   # optional: character scale baked into the art
//! tags = ["canonical"]
//! points = { beam_origin = [306, 78] }  # optional named points
//! # optional seat metadata (sit_swing): seat_y = 230, seat_x_range = [136, 279],
//! # feet_lowest_y = 364
//! # optional event reaction metadata: event = "cpu_busy",
//! # base_clip = "facepalm", base_frames = [0, 1, ...] (one per frame)
//! frames = [ { x = 0, y = 0, w = 400, h = 400, anchor = [200, 392] } ]
//! ```

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Errors loading assets.
#[derive(Debug, thiserror::Error)]
pub enum AtlasError {
    #[error("manifest: {0}")]
    Manifest(#[from] toml::de::Error),
    #[error("image: {0}")]
    Image(#[from] image::ImageError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("clip `{clip}`: {reason}")]
    InvalidClip { clip: String, reason: String },
}

/// One frame rectangle inside an atlas page.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FrameRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Feet anchor relative to the frame's top-left, so playback doesn't jitter.
    #[serde(default)]
    pub anchor: (i32, i32),
}

/// Playback mode of a clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoopMode {
    #[default]
    Loop,
    Once,
}

/// A named clip: an ordered list of frames on one atlas page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    /// Atlas page, relative to the manifest's directory (or absolute).
    pub page: String,
    /// Fixed playback rate, used when `frame_durations_ms` is absent.
    #[serde(default = "default_fps")]
    pub fps: f32,
    #[serde(default)]
    pub mode: LoopMode,
    /// Optional per-frame hold times in milliseconds (one per frame).
    /// When present the player uses these instead of `fps`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_durations_ms: Option<Vec<u32>>,
    /// Optional ground line (y inside each cell) the art was normalized to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_y: Option<i32>,
    /// Optional character scale baked into the art (informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Optional named attachment points in cell coordinates (same space as
    /// the feet anchor), e.g. `beam_origin` for a cast or `grab_point` (the
    /// spot to pin to the cursor while she is dragged).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub points: BTreeMap<String, (i32, i32)>,
    /// Optional seat line (y in cell coords) for seated clips such as
    /// `sit_swing`: the engine puts the edge she sits on (e.g. a window top) here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat_y: Option<i32>,
    /// Optional horizontal extent `[x0, x1]` of the seat contact, cell coords.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat_x_range: Option<(i32, i32)>,
    /// Optional lowest y the feet reach (for clips whose feet are not on
    /// `baseline_y`, e.g. legs dangling below a seat).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feet_lowest_y: Option<i32>,
    /// Optional system event this clip reacts to (e.g. `cpu_busy`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    /// Optional base clip this clip was composed from (reaction = base + overlay).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_clip: Option<String>,
    /// Optional frame index into `base_clip` for each of this clip's frames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_frames: Option<Vec<u32>>,
    pub frames: Vec<FrameRect>,
}

impl Default for Clip {
    fn default() -> Self {
        Self {
            page: String::new(),
            fps: default_fps(),
            mode: LoopMode::default(),
            frame_durations_ms: None,
            baseline_y: None,
            scale: None,
            tags: Vec::new(),
            points: BTreeMap::new(),
            seat_y: None,
            seat_x_range: None,
            feet_lowest_y: None,
            event: None,
            base_clip: None,
            base_frames: None,
            frames: Vec::new(),
        }
    }
}

fn default_fps() -> f32 {
    12.0
}

impl Clip {
    /// Resolves [`Clip::page`] against the manifest's directory.
    pub fn page_path(&self, manifest_dir: &Path) -> PathBuf {
        manifest_dir.join(&self.page)
    }

    /// Per-frame durations in seconds: `frame_durations_ms` if present,
    /// otherwise `1 / fps` for every frame.
    pub fn frame_durations_s(&self) -> Vec<f32> {
        match &self.frame_durations_ms {
            Some(ms) => ms.iter().map(|&d| d as f32 / 1000.0).collect(),
            None => vec![1.0 / self.fps; self.frames.len().max(1)],
        }
    }

    /// Named attachment point, if the clip defines it.
    pub fn point(&self, name: &str) -> Option<(i32, i32)> {
        self.points.get(name).copied()
    }

    /// Total length of one pass through the clip, in seconds.
    pub fn duration_s(&self) -> f32 {
        self.frame_durations_s().iter().sum()
    }

    /// Checks internal consistency (non-empty, sane timing).
    pub fn validate(&self, name: &str) -> Result<(), AtlasError> {
        let bad = |reason: String| AtlasError::InvalidClip {
            clip: name.to_owned(),
            reason,
        };
        if self.frames.is_empty() {
            return Err(bad("no frames".into()));
        }
        if let Some(ms) = &self.frame_durations_ms {
            if ms.len() != self.frames.len() {
                return Err(bad(format!(
                    "frame_durations_ms has {} entries but clip has {} frames",
                    ms.len(),
                    self.frames.len()
                )));
            }
            if ms.contains(&0) {
                return Err(bad("frame_durations_ms contains 0".into()));
            }
        } else if !(self.fps.is_finite() && self.fps > 0.0) {
            return Err(bad(format!("fps must be > 0 (got {})", self.fps)));
        }
        if let Some(bf) = &self.base_frames {
            if bf.len() != self.frames.len() {
                return Err(bad(format!(
                    "base_frames has {} entries but clip has {} frames",
                    bf.len(),
                    self.frames.len()
                )));
            }
        }
        if let Some((x0, x1)) = self.seat_x_range {
            if x0 > x1 {
                return Err(bad(format!("seat_x_range [{x0}, {x1}] is reversed")));
            }
        }
        Ok(())
    }
}

/// `manifest.toml`: clip name → clip.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub clips: BTreeMap<String, Clip>,
}

impl Manifest {
    /// Reads and validates `manifest.toml` from disk.
    pub fn load(path: &Path) -> Result<Self, AtlasError> {
        Self::from_toml_str(&std::fs::read_to_string(path)?)
    }

    /// Parses and validates a manifest from TOML text.
    pub fn from_toml_str(s: &str) -> Result<Self, AtlasError> {
        let m: Self = toml::from_str(s)?;
        for (name, clip) in &m.clips {
            clip.validate(name)?;
            // Composed clips must point at an existing base clip and frames.
            if let Some(base) = &clip.base_clip {
                let bad = |reason: String| AtlasError::InvalidClip {
                    clip: name.clone(),
                    reason,
                };
                let Some(b) = m.clips.get(base) else {
                    return Err(bad(format!("base_clip `{base}` not in manifest")));
                };
                if let Some(&i) = clip
                    .base_frames
                    .iter()
                    .flatten()
                    .find(|&&i| i as usize >= b.frames.len())
                {
                    return Err(bad(format!(
                        "base_frames index {i} out of range for `{base}` ({} frames)",
                        b.frames.len()
                    )));
                }
            }
        }
        Ok(m)
    }
}

/// Loads one atlas page as RGBA8.
pub fn load_page(path: &Path) -> Result<image::RgbaImage, AtlasError> {
    Ok(image::open(path)?.to_rgba8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manifest() {
        let m = Manifest::from_toml_str(
            r#"
            [clips.walk_left]
            page = "walk.png"
            fps = 12.0
            frames = [ { x = 0, y = 0, w = 240, h = 336, anchor = [120, 330] } ]
            "#,
        )
        .unwrap();
        let c = &m.clips["walk_left"];
        assert_eq!(c.frames[0].anchor, (120, 330));
        assert_eq!(c.mode, LoopMode::Loop);
        assert_eq!(c.frame_durations_ms, None);
        assert!((c.duration_s() - 1.0 / 12.0).abs() < 1e-6);
    }

    #[test]
    fn parses_per_frame_durations_and_extras() {
        let m = Manifest::from_toml_str(
            r#"
            [clips.wake]
            page = "wake/wake_sheet.png"
            fps = 6.0
            mode = "once"
            frame_durations_ms = [450, 350]
            baseline_y = 392
            scale = 0.54
            points = { grab_point = [200, 76] }
            frames = [
              { x = 0, y = 0, w = 400, h = 400, anchor = [200, 392] },
              { x = 400, y = 0, w = 400, h = 400, anchor = [200, 392] },
            ]
            "#,
        )
        .unwrap();
        let c = &m.clips["wake"];
        assert_eq!(c.mode, LoopMode::Once);
        assert_eq!(c.baseline_y, Some(392));
        assert_eq!(c.point("grab_point"), Some((200, 76)));
        assert_eq!(c.point("beam_origin"), None);
        assert!((c.duration_s() - 0.8).abs() < 1e-6);
        assert_eq!(
            c.page_path(Path::new("/a")),
            PathBuf::from("/a/wake/wake_sheet.png")
        );
    }

    #[test]
    fn rejects_mismatched_durations() {
        let err = Manifest::from_toml_str(
            r#"
            [clips.bad]
            page = "x.png"
            frame_durations_ms = [100, 100]
            frames = [ { x = 0, y = 0, w = 1, h = 1 } ]
            "#,
        )
        .unwrap_err();
        assert!(matches!(err, AtlasError::InvalidClip { .. }), "{err}");
    }

    #[test]
    fn parses_seat_and_event_metadata() {
        let m = Manifest::from_toml_str(
            r#"
            [clips.facepalm]
            page = "f.png"
            frames = [ { x = 0, y = 0, w = 4, h = 4 }, { x = 4, y = 0, w = 4, h = 4 } ]
            [clips.event_cpu_busy]
            page = "e.png"
            mode = "once"
            event = "cpu_busy"
            base_clip = "facepalm"
            base_frames = [1, 0]
            frames = [ { x = 0, y = 0, w = 4, h = 4 }, { x = 4, y = 0, w = 4, h = 4 } ]
            [clips.sit_swing]
            page = "s.png"
            seat_y = 230
            seat_x_range = [136, 279]
            feet_lowest_y = 364
            frames = [ { x = 0, y = 0, w = 4, h = 4 } ]
            "#,
        )
        .unwrap();
        let e = &m.clips["event_cpu_busy"];
        assert_eq!(e.event.as_deref(), Some("cpu_busy"));
        assert_eq!(e.base_clip.as_deref(), Some("facepalm"));
        assert_eq!(e.base_frames.as_deref(), Some(&[1, 0][..]));
        let s = &m.clips["sit_swing"];
        assert_eq!(s.seat_y, Some(230));
        assert_eq!(s.seat_x_range, Some((136, 279)));
        assert_eq!(s.feet_lowest_y, Some(364));
        assert!(s.points.is_empty());
        assert_eq!(m.clips["facepalm"].event, None);
    }

    #[test]
    fn rejects_bad_base_references() {
        let base = r#"
            [clips.b]
            page = "b.png"
            frames = [ { x = 0, y = 0, w = 1, h = 1 } ]
            "#;
        for clip in [
            // unknown base clip
            "[clips.e]\npage = \"e.png\"\nbase_clip = \"nope\"\nframes = [ { x = 0, y = 0, w = 1, h = 1 } ]",
            // index out of range
            "[clips.e]\npage = \"e.png\"\nbase_clip = \"b\"\nbase_frames = [3]\nframes = [ { x = 0, y = 0, w = 1, h = 1 } ]",
            // wrong length
            "[clips.e]\npage = \"e.png\"\nbase_clip = \"b\"\nbase_frames = [0, 0]\nframes = [ { x = 0, y = 0, w = 1, h = 1 } ]",
        ] {
            let err = Manifest::from_toml_str(&format!("{base}\n{clip}")).unwrap_err();
            assert!(matches!(err, AtlasError::InvalidClip { .. }), "{err}");
        }
    }

    #[test]
    fn round_trips_through_toml() {
        let mut m = Manifest::default();
        m.clips.insert(
            "a".into(),
            Clip {
                page: "a.png".into(),
                fps: 8.0,
                mode: LoopMode::Once,
                frame_durations_ms: Some(vec![100]),
                baseline_y: Some(392),
                scale: Some(0.5),
                tags: vec!["t".into()],
                points: [("beam_origin".to_string(), (306, 78))].into(),
                seat_y: Some(230),
                seat_x_range: Some((136, 279)),
                feet_lowest_y: Some(364),
                event: Some("cpu_busy".into()),
                base_clip: Some("a".into()),
                base_frames: Some(vec![0]),
                frames: vec![FrameRect {
                    x: 0,
                    y: 0,
                    w: 4,
                    h: 4,
                    anchor: (2, 3),
                }],
            },
        );
        let s = toml::to_string(&m).unwrap();
        let back = Manifest::from_toml_str(&s).unwrap();
        assert_eq!(back.clips, m.clips);
    }
}
