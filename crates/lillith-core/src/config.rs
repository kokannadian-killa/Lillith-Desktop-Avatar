//! User configuration (`~/.config/lillith/config.toml`).
//!
//! Keys mirror README-SP's config.ini names so existing tunables carry over.
//! TODO(M6): live reload from the egui settings panel.
//!
//! The overlay persists her on-screen position in the `[overlay]` table
//! ([`save_overlay`]); that write only replaces that table and keeps every
//! other key in the file.

use crate::spring::SpringParams;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

/// Where the overlay sits (M1). Both backends use the same numbers: the
/// distance in px from the left edge and from the bottom edge of the output
/// (layer-shell margins on Wayland; converted to window x/y on X11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlayPos {
    pub margin_left: i32,
    pub margin_bottom: i32,
    /// Output (monitor) name, e.g. `eDP-1`; `None` = compositor's choice.
    pub output: Option<String>,
}

impl Default for OverlayPos {
    fn default() -> Self {
        Self {
            margin_left: 80,
            margin_bottom: 48,
            output: None,
        }
    }
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
    pub overlay: OverlayPos,
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
            overlay: OverlayPos::default(),
        }
    }
}

impl Config {
    /// Parses a config file; missing keys fall back to defaults.
    pub fn from_toml_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// `$XDG_CONFIG_HOME/lillith/config.toml`, else `~/.config/lillith/config.toml`.
    pub fn default_path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("lillith/config.toml"))
    }

    /// Loads `path`; a missing file gives the defaults.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(s) => Ok(Self::from_toml_str(&s)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }
}

/// Config load/save errors.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config io: {0}")]
    Io(#[from] std::io::Error),
    #[error("config parse: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("config write: {0}")]
    Ser(#[from] toml::ser::Error),
}

/// Writes `pos` into the `[overlay]` table of `path`, creating the file and
/// its directory if needed. Other keys are kept (comments are not).
pub fn save_overlay(path: &Path, pos: &OverlayPos) -> Result<(), ConfigError> {
    let mut table: toml::Table = match std::fs::read_to_string(path) {
        Ok(s) => s.parse()?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(e) => return Err(e.into()),
    };
    table.insert("overlay".into(), toml::Value::try_from(pos)?);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, toml::to_string_pretty(&table)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
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

    #[test]
    fn overlay_position_round_trip_keeps_other_keys() {
        let dir = std::env::temp_dir().join(format!("lillith-cfg-{}", std::process::id()));
        let path = dir.join("sub/config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        // Missing file -> defaults.
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "mode = \"stay\"\nscale = 0.8\n").unwrap();
        let pos = OverlayPos {
            margin_left: 1234,
            margin_bottom: 7,
            output: Some("eDP-1".into()),
        };
        save_overlay(&path, &pos).unwrap();
        let c = Config::load(&path).unwrap();
        assert_eq!(c.overlay, pos);
        assert_eq!(c.mode, MoveMode::Stay);
        assert_eq!(c.scale, 0.8);
        // Second save replaces only the table.
        let pos2 = OverlayPos {
            margin_left: -3,
            ..OverlayPos::default()
        };
        save_overlay(&path, &pos2).unwrap();
        let c = Config::load(&path).unwrap();
        assert_eq!(c.overlay, pos2);
        assert_eq!(c.scale, 0.8);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
