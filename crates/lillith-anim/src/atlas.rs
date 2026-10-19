//! Atlas + manifest loading. TODO(M0/M2): match the slice-sheets output format.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Errors loading assets.
#[derive(Debug, thiserror::Error)]
pub enum AtlasError {
    #[error("manifest: {0}")]
    Manifest(#[from] toml::de::Error),
    #[error("image: {0}")]
    Image(#[from] image::ImageError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// One frame rectangle inside an atlas page.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoopMode {
    #[default]
    Loop,
    Once,
}

/// A named clip: an ordered list of frames on one atlas page.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Clip {
    pub page: String,
    pub frames: Vec<FrameRect>,
    #[serde(default = "default_fps")]
    pub fps: f32,
    #[serde(default)]
    pub mode: LoopMode,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_fps() -> f32 {
    12.0
}

/// `manifest.toml`: clip name → clip.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub clips: BTreeMap<String, Clip>,
}

impl Manifest {
    /// Reads `manifest.toml` from disk.
    pub fn load(path: &Path) -> Result<Self, AtlasError> {
        Ok(toml::from_str(&std::fs::read_to_string(path)?)?)
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
        let m: Manifest = toml::from_str(
            r#"
            [clips.walk_left]
            page = "walk.png"
            fps = 12.0
            frames = [ { x = 0, y = 0, w = 240, h = 336, anchor = [120, 330] } ]
            "#,
        )
        .unwrap();
        assert_eq!(m.clips["walk_left"].frames[0].anchor, (120, 330));
        assert_eq!(m.clips["walk_left"].mode, LoopMode::Loop);
    }
}
