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
//! # optional locomotion metadata (in-place walk/run cycles), one entry per
//! # frame: how far the ground moves under the planted foot when frame i
//! # gives way to frame i+1, in cell px along +x (negative for left-facing
//! # art), so the window can move exactly as far as the feet do:
//! # root_motion = [20.0, 21.0, ...]
//! # contact_frames = [3, 9]   # frames where a foot lands (footfall bounce)
//! # optional event reaction metadata: event = "cpu_busy",
//! # base_clip = "facepalm", base_frames = [0, 1, ...] (one per frame)
//! # rule_events = ["window.opened.folder"]  # every rule event it serves
//! # optional baked-jiggle pairing: the clean clip names its baked sibling
//! # (jiggle_variant = "idle_jiggle"); the sibling has jiggle_baked = true,
//! # jiggle_of = "idle" and the engine spring groups to switch off while it
//! # is drawn: disable_springs = ["chest", "pigtails", "hip"]
//! # baked-WALK clips (video walk/strut/run takes) add the non-spring group
//! # "gait" so the engine gait ([gait] in physics.toml) never drives them:
//! # disable_springs = ["gait"]
//! # optional per-frame attachment points (one [x, y] per frame):
//! # frame_points = { chest_point = [[242, 133], ...], hip_point = [...] }
//! # optional speaking-clip mouth shape per frame (lip-flap):
//! # mouth_sequence = ["closed", "mid", "open", "smile", ...]
//! # optional mouth patches for true lip-sync (speaking clips):
//! # mouth_angle_deg = [0.0, 1.1, ...]   # one per frame
//! # mouth_shape_ms = [200, 260]
//! # [clips.talk_loop.mouth_patches.open]
//! # page = "talk_loop/mouth/mouth_open.png"
//! # centre = [9.0, 5.96]
//! # optional blend-helper metadata (transition clips between two clips):
//! # blend = { from = "idle", to = "wave", from_frame = 9, to_frame = 0,
//! #           blend_t = [0.2, 0.4, 0.6, 0.8], easing = "smoothstep" }
//! # optional MP4 source (alpha-packed H.264, see `crate::video`): played
//! # instead of `page` when present and decodable; `page` stays the fallback.
//! # Frame i of the video is frame i of the clip; all cells share one size.
//! # video = "wake/wake.mp4"
//! frames = [ { x = 0, y = 0, w = 400, h = 400, anchor = [200, 392] } ]
//! ```

use lillith_core::spring::SpringMask;
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

/// Blend-helper metadata: the clip bridges `from` (after `from_frame`) to
/// `to` (before `to_frame`); it plays once and its endpoints are not
/// repeated inside the clip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Blend {
    pub from: String,
    pub to: String,
    pub from_frame: u32,
    pub to_frame: u32,
    /// Interpolation parameter of each in-between frame (informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend_t: Option<Vec<f32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub easing: Option<String>,
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
    /// Optional root motion of an in-place (treadmill) locomotion cycle, one
    /// entry per frame: cell px the character advances along +x when frame
    /// `i` gives way to frame `i + 1` (the last entry wraps to frame 0).
    /// Measured from the planted foot, so moving the window by this much
    /// keeps the feet still on the desktop. Negative for left-facing art.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_motion: Option<Vec<f32>>,
    /// Optional frames on which a foot lands (heel strike) in a locomotion
    /// cycle; footfall bounces fire there instead of at fixed cycle phases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_frames: Option<Vec<usize>>,
    /// Optional system event this clip reacts to (e.g. `cpu_busy`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    /// Optional base clip this clip was composed from (reaction = base + overlay).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_clip: Option<String>,
    /// Optional frame index into `base_clip` for each of this clip's frames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_frames: Option<Vec<u32>>,
    /// Every dotted rule event this clip serves (superset of `event`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rule_events: Vec<String>,
    /// The frames already contain baked secondary motion (jiggle variant).
    #[serde(default, skip_serializing_if = "is_false")]
    pub jiggle_baked: bool,
    /// For a baked-jiggle variant: the engine-clean clip it was baked from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jiggle_of: Option<String>,
    /// For an engine-clean clip: its baked-jiggle sibling, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jiggle_variant: Option<String>,
    /// Engine spring groups switched off while this clip is drawn
    /// (`chest`, `pigtails`, `hip`; see [`SpringMask::group`]), so the
    /// engine adds no motion on top of baked jiggle. `gait`
    /// ([`SpringMask::GAIT_GROUP`]) marks a baked walk: the engine gait is
    /// suppressed while it is drawn (see [`Clip::bakes_gait`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disable_springs: Vec<String>,
    /// Baked-jiggle clips whose frame count differs from their clean clip
    /// (long holds split into settle sub-frames): the clean frame shown in
    /// each of this clip's frames. Its own `frame_durations_ms` add up to the
    /// clean clip's duration. Empty = frame `j` is clean frame `j`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clean_frame_index: Vec<u32>,
    /// Optional per-frame attachment points (cell coords), one entry per
    /// frame, e.g. `chest_point` for the jiggle springs. Clip-level medians
    /// stay in [`Clip::points`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub frame_points: BTreeMap<String, Vec<(i32, i32)>>,
    /// Optional blend-helper metadata (transition between two clips).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend: Option<Blend>,
    /// Speaking clips: the mouth shape baked into each frame
    /// (`closed` / `mid` / `open` / `smile`, one per frame), from the
    /// JSON's `lip_flap.baked_sequence`. Drives the token lip-flap
    /// ([`crate::talk`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mouth_sequence: Vec<String>,
    /// Speaking clips: mouth patch images per shape (`closed`, `mid`,
    /// `open`, `smile`), pasted over the face at the per-frame
    /// `mouth_point` for true lip-sync (from `lip_flap.shapes`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub mouth_patches: BTreeMap<String, MouthPatch>,
    /// Head tilt per frame (degrees, + = clockwise) to rotate the mouth
    /// patch by; one per frame.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mouth_angle_deg: Vec<f32>,
    /// Recommended time between mouth-shape changes `[min, max]` in ms
    /// (`lip_flap.recommended_rate.mouth_shape_change_ms`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mouth_shape_ms: Option<(u32, u32)>,
    /// Optional alpha-packed MP4 (relative to the manifest's directory, or
    /// absolute) the frames are played from; [`Clip::page`] is the fallback
    /// when ffmpeg is missing or the video fails (see [`crate::video`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<String>,
    pub frames: Vec<FrameRect>,
}

impl Clip {
    /// Ground speed (canvas px/s at playback speed 1) the clip's planted
    /// feet are locked to, from the importer's `move_speed_px_s:<n>` tag.
    pub fn move_speed_px_s(&self) -> Option<f32> {
        self.tags
            .iter()
            .find_map(|t| t.strip_prefix("move_speed_px_s:"))
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| v.is_finite() && *v > 0.0)
    }
}

/// One mouth patch image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MouthPatch {
    /// Image path, relative to the manifest's directory (or absolute).
    pub page: String,
    /// Patch pixel that lands on `mouth_point`.
    pub centre: (f32, f32),
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
            root_motion: None,
            contact_frames: None,
            event: None,
            base_clip: None,
            base_frames: None,
            rule_events: Vec::new(),
            jiggle_baked: false,
            jiggle_of: None,
            jiggle_variant: None,
            disable_springs: Vec::new(),
            clean_frame_index: Vec::new(),
            frame_points: BTreeMap::new(),
            blend: None,
            mouth_sequence: Vec::new(),
            mouth_patches: BTreeMap::new(),
            mouth_angle_deg: Vec::new(),
            mouth_shape_ms: None,
            video: None,
            frames: Vec::new(),
        }
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn default_fps() -> f32 {
    12.0
}

impl Clip {
    /// Resolves [`Clip::page`] against the manifest's directory.
    pub fn page_path(&self, manifest_dir: &Path) -> PathBuf {
        manifest_dir.join(&self.page)
    }

    /// Resolves [`Clip::video`] against the manifest's directory.
    pub fn video_path(&self, manifest_dir: &Path) -> Option<PathBuf> {
        self.video.as_ref().map(|v| manifest_dir.join(v))
    }

    /// Cell size shared by every frame (video clips need one size).
    pub fn cell_size(&self) -> (u32, u32) {
        self.frames.first().map_or((0, 0), |f| (f.w, f.h))
    }

    /// Per-frame durations in seconds: `frame_durations_ms` if present,
    /// otherwise `1 / fps` for every frame.
    pub fn frame_durations_s(&self) -> Vec<f32> {
        match &self.frame_durations_ms {
            Some(ms) => ms.iter().map(|&d| d as f32 / 1000.0).collect(),
            None => vec![1.0 / self.fps; self.frames.len().max(1)],
        }
    }

    /// The clean-clip frame shown in frame `j` ([`Clip::clean_frame_index`],
    /// identity for clips without one).
    pub fn clean_frame(&self, j: usize) -> usize {
        match self.clean_frame_index.get(j) {
            Some(&c) => c as usize,
            None => j,
        }
    }

    /// Named attachment point, if the clip defines it.
    pub fn point(&self, name: &str) -> Option<(i32, i32)> {
        self.points.get(name).copied()
    }

    /// Attachment point `name` on `frame`: the per-frame value when the clip
    /// has `frame_points`, else the clip-level [`Clip::points`] value.
    pub fn point_at(&self, name: &str, frame: usize) -> Option<(i32, i32)> {
        self.frame_points
            .get(name)
            .and_then(|v| v.get(frame).copied())
            .or_else(|| self.point(name))
    }

    /// Average ground speed of a root-motion cycle in cell px/s (signed,
    /// +x = right), or `None` without `root_motion`.
    pub fn root_speed_px_s(&self) -> Option<f32> {
        let rm = self.root_motion.as_ref()?;
        let total: f32 = self.frame_durations_s().iter().sum();
        (total > 0.0).then(|| rm.iter().sum::<f32>() / total)
    }

    /// `true` for blend-helper (transition) clips.
    pub fn is_blend(&self) -> bool {
        self.blend.is_some()
    }

    /// `true` if this clip reacts to rule event `event` (`event` or
    /// `rule_events`).
    pub fn serves_event(&self, event: &str) -> bool {
        self.event.as_deref() == Some(event) || self.rule_events.iter().any(|e| e == event)
    }

    /// Engine spring channels to switch off while this clip is drawn
    /// (from [`Clip::disable_springs`]; unknown names are rejected on load).
    pub fn spring_mask(&self) -> SpringMask {
        let mut m = SpringMask::NONE;
        for c in self
            .disable_springs
            .iter()
            .filter_map(|g| SpringMask::group(g))
        {
            m.insert(c);
        }
        m
    }

    /// The frames bake the walk itself (legs, arms, hip bob): its
    /// `disable_springs` names `gait`. The animator then suppresses the
    /// engine gait (`PhysicsDriver::set_gait_suppressed`) while it is drawn.
    pub fn bakes_gait(&self) -> bool {
        SpringMask::names_gait(&self.disable_springs)
    }

    /// Tag test (`allure`, `idle_variant`, `blend`, ...).
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
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
        if !self.mouth_angle_deg.is_empty() && self.mouth_angle_deg.len() != self.frames.len() {
            return Err(bad(format!(
                "mouth_angle_deg has {} entries but clip has {} frames",
                self.mouth_angle_deg.len(),
                self.frames.len()
            )));
        }
        if !self.mouth_sequence.is_empty() && self.mouth_sequence.len() != self.frames.len() {
            return Err(bad(format!(
                "mouth_sequence has {} entries but clip has {} frames",
                self.mouth_sequence.len(),
                self.frames.len()
            )));
        }
        for (pname, v) in &self.frame_points {
            if v.len() != self.frames.len() {
                return Err(bad(format!(
                    "frame_points.{pname} has {} entries but clip has {} frames",
                    v.len(),
                    self.frames.len()
                )));
            }
        }
        if let Some(rm) = &self.root_motion {
            if rm.len() != self.frames.len() {
                return Err(bad(format!(
                    "root_motion has {} entries but clip has {} frames",
                    rm.len(),
                    self.frames.len()
                )));
            }
            if rm.iter().any(|v| !v.is_finite()) {
                return Err(bad("root_motion must be finite".into()));
            }
        }
        if let Some(cf) = &self.contact_frames {
            if let Some(f) = cf.iter().find(|&&f| f >= self.frames.len()) {
                return Err(bad(format!(
                    "contact_frames: frame {f} out of range ({} frames)",
                    self.frames.len()
                )));
            }
        }
        if let Some(b) = &self.blend {
            if b.from.is_empty() || b.to.is_empty() {
                return Err(bad("blend.from / blend.to must be set".into()));
            }
        }
        if let Err(g) = SpringMask::from_groups(&self.disable_springs) {
            return Err(bad(format!("disable_springs: unknown spring group `{g}`")));
        }
        if let Some(v) = &self.video {
            if v.is_empty() {
                return Err(bad("video is empty".into()));
            }
            let (w, h) = self.cell_size();
            if self.frames.iter().any(|f| (f.w, f.h) != (w, h)) {
                return Err(bad("video clips need one cell size for every frame".into()));
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
            // Jiggle pairs must point at existing clips. Their frame counts
            // may differ (the animator times the player for the clip
            // actually drawn) when the jiggle side maps its frames to clean
            // frames (`clean_frame_index`), or when both clips loop.
            if !clip.clean_frame_index.is_empty() {
                let bad = |reason: String| AtlasError::InvalidClip {
                    clip: name.clone(),
                    reason,
                };
                if clip.clean_frame_index.len() != clip.frames.len() {
                    return Err(bad(format!(
                        "clean_frame_index has {} entries but clip has {} frames",
                        clip.clean_frame_index.len(),
                        clip.frames.len()
                    )));
                }
                let Some(clean) = clip.jiggle_of.as_ref().and_then(|c| m.clips.get(c)) else {
                    return Err(bad("clean_frame_index without a valid jiggle_of".into()));
                };
                if let Some(&i) = clip
                    .clean_frame_index
                    .iter()
                    .find(|&&i| i as usize >= clean.frames.len())
                {
                    return Err(bad(format!(
                        "clean_frame_index {i} out of range for `{}` ({} frames)",
                        clip.jiggle_of.as_deref().unwrap_or_default(),
                        clean.frames.len()
                    )));
                }
            }
            for (field, other) in [
                ("jiggle_of", &clip.jiggle_of),
                ("jiggle_variant", &clip.jiggle_variant),
            ] {
                let Some(other) = other else { continue };
                let bad = |reason: String| AtlasError::InvalidClip {
                    clip: name.clone(),
                    reason,
                };
                let Some(o) = m.clips.get(other) else {
                    return Err(bad(format!("{field} `{other}` not in manifest")));
                };
                let both_loop = o.mode == LoopMode::Loop && clip.mode == LoopMode::Loop;
                // The jiggle side of the pair carries the map.
                let jiggle = if field == "jiggle_of" { clip } else { o };
                let mapped = !jiggle.clean_frame_index.is_empty();
                if o.frames.len() != clip.frames.len() && !both_loop && !mapped {
                    return Err(bad(format!(
                        "{field} `{other}` has {} frames, this clip {}",
                        o.frames.len(),
                        clip.frames.len()
                    )));
                }
            }
            // Blend helpers must bridge existing clips at valid frames.
            if let Some(bl) = &clip.blend {
                for (end, cname, frame) in [
                    ("from", &bl.from, bl.from_frame),
                    ("to", &bl.to, bl.to_frame),
                ] {
                    let bad = |reason: String| AtlasError::InvalidClip {
                        clip: name.clone(),
                        reason,
                    };
                    let Some(c) = m.clips.get(cname) else {
                        return Err(bad(format!("blend.{end} `{cname}` not in manifest")));
                    };
                    if frame as usize >= c.frames.len() {
                        return Err(bad(format!(
                            "blend.{end}_frame {frame} out of range for `{cname}` ({} frames)",
                            c.frames.len()
                        )));
                    }
                }
            }
        }
        Ok(m)
    }
}

/// Finds the generated set's `manifest.toml`: `$LILLITH_MANIFEST`, then
/// `./assets/generated/manifest.toml`, (Windows) next to `lillith.exe` and
/// `%LOCALAPPDATA%\lillith\generated`, then
/// `$XDG_DATA_HOME/lillith/generated/manifest.toml` (default
/// `~/.local/share/...`), then (Unix) `<dir>/lillith/generated/manifest.toml`
/// for each `$XDG_DATA_DIRS` entry (default `/usr/local/share:/usr/share`),
/// and last the standard install locations ([`install_manifest_paths`]:
/// `<exe>/../assets/generated` and `/opt/lillith` on Linux,
/// `%LOCALAPPDATA%\Programs\Lillith` on Windows, the app's files dir on
/// Android). Returns the first that exists.
pub fn default_manifest_path() -> Option<PathBuf> {
    manifest_candidates_with(
        &|k| std::env::var_os(k),
        std::env::current_exe().ok().as_deref(),
    )
    .into_iter()
    .find(|p| p.is_file())
}

/// Every place [`default_manifest_path`] looks, in order (`env` and `exe`
/// injectable for tests).
pub fn manifest_candidates_with(
    env: &dyn Fn(&str) -> Option<std::ffi::OsString>,
    exe: Option<&std::path::Path>,
) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = env("LILLITH_MANIFEST").filter(|v| !v.is_empty()) {
        candidates.push(PathBuf::from(p));
    }
    candidates.push(PathBuf::from("assets/generated/manifest.toml"));
    // Windows install: `<install dir>\assets\generated\manifest.toml`
    // next to `lillith.exe`, then `%LOCALAPPDATA%\lillith\generated`.
    #[cfg(windows)]
    {
        if let Some(dir) = exe.and_then(|e| e.parent()) {
            candidates.push(dir.join("assets").join("generated").join("manifest.toml"));
        }
        if let Some(l) = env("LOCALAPPDATA") {
            candidates.push(
                PathBuf::from(l)
                    .join("lillith")
                    .join("generated")
                    .join("manifest.toml"),
            );
        }
    }
    let data = env("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    if let Some(d) = data {
        candidates.push(d.join("lillith/generated/manifest.toml"));
    }
    // System-wide install (the Arch PKGBUILD): `$XDG_DATA_DIRS`, default
    // `/usr/local/share:/usr/share`.
    #[cfg(unix)]
    candidates.extend(
        system_data_dirs(env("XDG_DATA_DIRS"))
            .into_iter()
            .map(|d| d.join("lillith/generated/manifest.toml")),
    );
    candidates.extend(install_manifest_paths(env, exe));
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|p| seen.insert(p.clone()));
    candidates
}

/// The standard install locations' `manifest.toml`, used when nothing
/// else is found and no `--manifest` / `$LILLITH_MANIFEST` is given:
///
/// - the package layout around the binary: `<exe dir>/../assets/generated`
///   (`/opt/lillith/bin/lillith` -> `/opt/lillith/assets/generated`) and
///   `<exe dir>/assets/generated` (portable zip, Windows install);
/// - Linux: `/opt/lillith/assets/generated/manifest.toml` (lillith-bin);
/// - Windows: `%LOCALAPPDATA%\Programs\Lillith\assets\generated\manifest.toml`
///   (the per-user setup);
/// - Android: the app's files dir (`/data/user/0/<pkg>/files/generated`).
pub fn install_manifest_paths(
    env: &dyn Fn(&str) -> Option<std::ffi::OsString>,
    exe: Option<&std::path::Path>,
) -> Vec<PathBuf> {
    let gen = |root: PathBuf| root.join("assets").join("generated").join("manifest.toml");
    let mut v = Vec::new();
    if let Some(dir) = exe.and_then(|e| e.parent()) {
        v.push(gen(dir.to_path_buf()));
        if let Some(up) = dir.parent() {
            v.push(gen(up.to_path_buf()));
        }
    }
    #[cfg(all(unix, not(target_os = "android")))]
    v.push(gen(PathBuf::from(LINUX_INSTALL_DIR)));
    #[cfg(windows)]
    if let Some(l) = env("LOCALAPPDATA") {
        v.push(gen(PathBuf::from(l).join("Programs").join("Lillith")));
    }
    #[cfg(target_os = "android")]
    for base in ["/data/user/0", "/data/data"] {
        v.push(
            PathBuf::from(base)
                .join(ANDROID_PACKAGE)
                .join("files/generated/manifest.toml"),
        );
    }
    let _ = env;
    v
}

/// Where the Arch package (lillith-bin) installs her.
pub const LINUX_INSTALL_DIR: &str = "/opt/lillith";
/// The Android application id (its files dir holds `generated/`).
pub const ANDROID_PACKAGE: &str = "com.kokannadian.lillith";

#[cfg(unix)]
fn system_data_dirs(var: Option<std::ffi::OsString>) -> Vec<PathBuf> {
    let dirs: Vec<PathBuf> = var
        .map(|v| std::env::split_paths(&v).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.is_absolute())
        .collect();
    if dirs.is_empty() {
        vec![
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share"),
        ]
    } else {
        dirs
    }
}

/// Loads one atlas page as RGBA8.
pub fn load_page(path: &Path) -> Result<image::RgbaImage, AtlasError> {
    Ok(image::open(path)?.to_rgba8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn install_locations_are_the_last_resort() {
        let env = |k: &str| match k {
            "HOME" => Some(std::ffi::OsString::from("/home/ko")),
            _ => None,
        };
        let exe = std::path::Path::new("/opt/lillith/bin/lillith");
        let c = manifest_candidates_with(&env, Some(exe));
        let opt = PathBuf::from("/opt/lillith/assets/generated/manifest.toml");
        let user = PathBuf::from("/home/ko/.local/share/lillith/generated/manifest.toml");
        let pu = c.iter().position(|p| *p == user).expect("user data dir");
        let po = c
            .iter()
            .position(|p| *p == opt)
            .expect("/opt/lillith fallback");
        assert!(pu < po, "{c:#?}");
        // No duplicates (exe/.. and the fixed /opt/lillith are the same here).
        assert_eq!(c.iter().filter(|p| **p == opt).count(), 1);
        // $LILLITH_MANIFEST still wins.
        let env2 = |k: &str| (k == "LILLITH_MANIFEST").then(|| "/x/m.toml".into());
        assert_eq!(
            manifest_candidates_with(&env2, None)[0],
            PathBuf::from("/x/m.toml")
        );
    }

    #[test]
    fn package_layout_next_to_the_binary_is_found() {
        // A self-contained install tree: <root>/bin/lillith + <root>/assets/generated.
        let root = std::env::temp_dir().join(format!("lillith-atlas-inst-{}", std::process::id()));
        let gen = root.join("assets/generated");
        std::fs::create_dir_all(&gen).unwrap();
        std::fs::write(gen.join("manifest.toml"), "").unwrap();
        let exe = root.join("bin").join("lillith");
        let none = |_: &str| None;
        let found = install_manifest_paths(&none, Some(&exe))
            .into_iter()
            .find(|p| p.is_file());
        assert_eq!(found, Some(gen.join("manifest.toml")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(unix)]
    fn system_data_dirs_default_and_override() {
        let def = system_data_dirs(None);
        assert_eq!(
            def,
            vec![
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share")
            ]
        );
        assert_eq!(system_data_dirs(Some("".into())), def);
        assert_eq!(
            system_data_dirs(Some("/opt/share:relative:/usr/share".into())),
            vec![PathBuf::from("/opt/share"), PathBuf::from("/usr/share")]
        );
    }

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
    fn allows_self_referencing_base_clip() {
        // New-pose event clips are their own base (base_clip = own name).
        let m = Manifest::from_toml_str(
            r#"
            [clips.event_ram_high]
            page = "r.png"
            mode = "once"
            event = "ram.high"
            base_clip = "event_ram_high"
            base_frames = [0, 1]
            frames = [ { x = 0, y = 0, w = 4, h = 4 }, { x = 4, y = 0, w = 4, h = 4 } ]
            "#,
        )
        .unwrap();
        let c = &m.clips["event_ram_high"];
        assert_eq!(c.base_clip.as_deref(), Some("event_ram_high"));
        // Its own frame count still bounds the indices.
        let err = Manifest::from_toml_str(
            r#"
            [clips.e]
            page = "e.png"
            base_clip = "e"
            base_frames = [2]
            frames = [ { x = 0, y = 0, w = 4, h = 4 } ]
            "#,
        )
        .unwrap_err();
        assert!(matches!(err, AtlasError::InvalidClip { .. }), "{err}");
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
                root_motion: None,
                contact_frames: None,
                event: Some("cpu_busy".into()),
                base_clip: Some("a".into()),
                base_frames: Some(vec![0]),
                rule_events: vec!["cpu.high".into()],
                jiggle_baked: true,
                jiggle_of: Some("a".into()),
                jiggle_variant: Some("a".into()),
                disable_springs: vec!["chest".into(), "pigtails".into(), "hip".into()],
                clean_frame_index: Vec::new(),
                frame_points: [("chest_point".to_string(), vec![(240, 130)])].into(),
                mouth_sequence: vec!["open".into()],
                mouth_patches: [(
                    "open".to_string(),
                    MouthPatch {
                        page: "m/open.png".into(),
                        centre: (9.0, 5.96),
                    },
                )]
                .into(),
                mouth_angle_deg: vec![1.5],
                mouth_shape_ms: Some((200, 260)),
                blend: Some(Blend {
                    from: "a".into(),
                    to: "a".into(),
                    from_frame: 0,
                    to_frame: 0,
                    blend_t: Some(vec![0.5]),
                    easing: Some("smoothstep".into()),
                }),
                video: Some("a/a.mp4".into()),
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

    #[test]
    fn frame_points_and_blend_metadata() {
        let m = Manifest::from_toml_str(
            r#"
            [clips.idle]
            page = "i.png"
            points = { chest_point = [240, 130] }
            frame_points = { chest_point = [[241, 131], [243, 129]] }
            frames = [ { x = 0, y = 0, w = 4, h = 4 }, { x = 4, y = 0, w = 4, h = 4 } ]
            [clips.wave]
            page = "w.png"
            rule_events = ["window.closed"]
            frames = [ { x = 0, y = 0, w = 4, h = 4 } ]
            [clips.blend_idle_to_wave]
            page = "b.png"
            mode = "once"
            blend = { from = "idle", to = "wave", from_frame = 1, to_frame = 0, blend_t = [0.5] }
            frames = [ { x = 0, y = 0, w = 4, h = 4 } ]
            "#,
        )
        .unwrap();
        let idle = &m.clips["idle"];
        assert_eq!(idle.point_at("chest_point", 1), Some((243, 129)));
        assert_eq!(
            idle.point_at("chest_point", 9),
            Some((240, 130)),
            "falls back"
        );
        assert_eq!(idle.point_at("hip_point", 0), None);
        let b = &m.clips["blend_idle_to_wave"];
        assert!(b.is_blend() && !idle.is_blend());
        assert_eq!(b.blend.as_ref().unwrap().to, "wave");
        assert!(m.clips["wave"].serves_event("window.closed"));
        assert!(!m.clips["wave"].serves_event("cpu.high"));
        // Round trip keeps everything.
        let back = Manifest::from_toml_str(&toml::to_string(&m).unwrap()).unwrap();
        assert_eq!(back.clips, m.clips);
    }

    #[test]
    fn rejects_bad_frame_points_and_blends() {
        let base = "[clips.idle]\npage = \"i.png\"\nframes = [ { x = 0, y = 0, w = 1, h = 1 } ]\n";
        for clip in [
            // wrong frame_points length
            "[clips.e]\npage = \"e.png\"\nframe_points = { chest_point = [[1, 1], [2, 2]] }\nframes = [ { x = 0, y = 0, w = 1, h = 1 } ]",
            // unknown blend target
            "[clips.e]\npage = \"e.png\"\nblend = { from = \"idle\", to = \"nope\", from_frame = 0, to_frame = 0 }\nframes = [ { x = 0, y = 0, w = 1, h = 1 } ]",
            // blend frame out of range
            "[clips.e]\npage = \"e.png\"\nblend = { from = \"idle\", to = \"idle\", from_frame = 3, to_frame = 0 }\nframes = [ { x = 0, y = 0, w = 1, h = 1 } ]",
        ] {
            let err = Manifest::from_toml_str(&format!("{base}\n{clip}")).unwrap_err();
            assert!(matches!(err, AtlasError::InvalidClip { .. }), "{err}");
        }
    }

    #[test]
    fn jiggle_pairs_and_disable_springs() {
        use lillith_core::spring::Channel;
        let m = Manifest::from_toml_str(
            r#"
            [clips.idle]
            page = "i.png"
            jiggle_variant = "idle_jiggle"
            frames = [ { x = 0, y = 0, w = 4, h = 4 }, { x = 4, y = 0, w = 4, h = 4 } ]
            [clips.idle_jiggle]
            page = "j.png"
            jiggle_baked = true
            jiggle_of = "idle"
            disable_springs = ["chest", "pigtails", "hip"]
            frames = [ { x = 0, y = 0, w = 4, h = 4 }, { x = 4, y = 0, w = 4, h = 4 } ]
            "#,
        )
        .unwrap();
        let (idle, j) = (&m.clips["idle"], &m.clips["idle_jiggle"]);
        assert!(idle.spring_mask().is_empty() && !idle.jiggle_baked);
        assert!(j.jiggle_baked);
        let mask = j.spring_mask();
        for c in [Channel::Chest, Channel::Pigtail, Channel::RootX] {
            assert!(mask.contains(c), "{c:?}");
        }
        assert!(!mask.contains(Channel::RootY) && !mask.contains(Channel::Angle));

        let base = "[clips.idle]\npage = \"i.png\"\nframes = [ { x = 0, y = 0, w = 1, h = 1 } ]\n";
        for clip in [
            "[clips.e]\npage = \"e.png\"\ndisable_springs = [\"elbow\"]\nframes = [ { x = 0, y = 0, w = 1, h = 1 } ]",
            "[clips.e]\npage = \"e.png\"\njiggle_of = \"nope\"\nframes = [ { x = 0, y = 0, w = 1, h = 1 } ]",
            "[clips.e]\npage = \"e.png\"\nmode = \"once\"\njiggle_variant = \"idle\"\nframes = [ { x = 0, y = 0, w = 1, h = 1 }, { x = 1, y = 0, w = 1, h = 1 } ]",
        ] {
            let err = Manifest::from_toml_str(&format!("{base}\n{clip}")).unwrap_err();
            assert!(matches!(err, AtlasError::InvalidClip { .. }), "{err}");
        }
        // A once jiggle clip of another length needs a valid
        // clean_frame_index on its jiggle side.
        let jig = |cfi: &str| {
            format!(
                "{base}\n[clips.e]\npage = \"e.png\"\nmode = \"once\"\njiggle_baked = true\njiggle_of = \"idle\"\n{cfi}frames = [ {{ x = 0, y = 0, w = 1, h = 1 }}, {{ x = 1, y = 0, w = 1, h = 1 }} ]"
            )
        };
        assert!(Manifest::from_toml_str(&jig("")).is_err());
        assert!(Manifest::from_toml_str(&jig("clean_frame_index = [0, 0]\n")).is_ok());
        assert!(Manifest::from_toml_str(&jig("clean_frame_index = [0, 1]\n")).is_err());
        assert!(Manifest::from_toml_str(&jig("clean_frame_index = [0]\n")).is_err());
        // Two looping clips may differ in length (talk_loop 32 f, its
        // jiggle sibling 36 f).
        let loops = "[clips.e]\npage = \"e.png\"\njiggle_variant = \"idle\"\nframes = [ { x = 0, y = 0, w = 1, h = 1 }, { x = 1, y = 0, w = 1, h = 1 } ]";
        assert!(Manifest::from_toml_str(&format!("{base}\n{loops}")).is_ok());
    }

    #[test]
    fn video_source_field() {
        let m = Manifest::from_toml_str(
            r#"
            [clips.pose]
            page = "pose/pose_sheet.png"
            video = "pose/pose.mp4"
            frames = [ { x = 0, y = 0, w = 4, h = 4 }, { x = 4, y = 0, w = 4, h = 4 } ]
            "#,
        )
        .unwrap();
        let c = &m.clips["pose"];
        assert_eq!(
            c.video_path(Path::new("/a")),
            Some(PathBuf::from("/a/pose/pose.mp4"))
        );
        assert_eq!(c.cell_size(), (4, 4));
        let mixed = "[clips.e]\npage = \"e.png\"\nvideo = \"e.mp4\"\nframes = [ { x = 0, y = 0, w = 4, h = 4 }, { x = 4, y = 0, w = 5, h = 4 } ]";
        assert!(Manifest::from_toml_str(mixed).is_err());
        let empty = "[clips.e]\npage = \"e.png\"\nvideo = \"\"\nframes = [ { x = 0, y = 0, w = 4, h = 4 } ]";
        assert!(Manifest::from_toml_str(empty).is_err());
    }
}
