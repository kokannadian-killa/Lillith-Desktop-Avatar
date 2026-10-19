//! User configuration (`~/.config/lillith/config.toml`).
//!
//! Keys mirror README-SP's config.ini names so existing tunables carry over.
//! TODO(M6): live reload from the egui settings panel.

use crate::spring::SpringParams;
use serde::{Deserialize, Serialize};

/// Spring tunables with README-SP defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Springs {
    pub chest: SpringParams,
    pub pig: SpringParams,
    pub root: SpringParams,
    pub angle: SpringParams,
    pub scale: SpringParams,
    pub jiggle_gain: f32,
    pub chest_bounce_gain: f32,
    pub pigtail_sway_gain: f32,
}

impl Default for Springs {
    fn default() -> Self {
        let p = |stiffness, damping| SpringParams { stiffness, damping };
        Self {
            chest: p(65.0, 9.5),
            pig: p(32.0, 6.5),
            root: p(48.0, 10.0),
            angle: p(42.0, 9.0),
            scale: p(55.0, 11.0),
            jiggle_gain: 0.6,
            chest_bounce_gain: 1.0,
            pigtail_sway_gain: 0.9,
        }
    }
}

/// Movement mode (GH#1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MoveMode {
    #[default]
    Wander,
    Follow,
    Stay,
}

/// Top-level config.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub springs: Springs,
    pub mode: MoveMode,
    pub scale: f32,
    pub opacity: f32,
    pub physics_hz: f32,
    pub render_hz: f32,
    pub sprite_fps: f32,
    pub pose_cycle_s: f32,
    pub muted: bool,
    pub voice_enabled: bool,
    pub llm_enabled: bool,
    /// `ollama` or `llama-server` (open question, PLAN.md §6 Q4).
    pub llm_backend: String,
    pub llm_url: String,
    pub llm_model: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            springs: Springs::default(),
            mode: MoveMode::default(),
            scale: 1.0,
            opacity: 1.0,
            physics_hz: 120.0,
            render_hz: 30.0,
            sprite_fps: 12.0,
            pose_cycle_s: 16.0,
            muted: false,
            voice_enabled: true,
            llm_enabled: true,
            llm_backend: "ollama".into(),
            llm_url: "http://127.0.0.1:11434".into(),
            llm_model: "dolphin3".into(),
        }
    }
}

impl Config {
    /// Parses a config file; missing keys fall back to defaults.
    pub fn from_toml_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        let c = Config::from_toml_str("").unwrap();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn partial_override() {
        let c = Config::from_toml_str(
            "mode = \"stay\"\n[springs.chest]\nstiffness = 70.0\ndamping = 9.0\n",
        )
        .unwrap();
        assert_eq!(c.mode, MoveMode::Stay);
        assert_eq!(c.springs.chest.stiffness, 70.0);
        assert_eq!(c.springs.pig.stiffness, 32.0);
    }
}
