//! User configuration (`~/.config/lillith/config.toml`).
//!
//! Keys mirror README-SP's config.ini names so existing tunables carry over.
//! TODO(M6): live reload from the egui settings panel.
//!
//! The overlay persists her on-screen position in the `[overlay]` table
//! ([`save_overlay`]); that write only replaces that table and keeps every
//! other key in the file.

use crate::spring::SpringParams;
use crate::state::BehaviorConfig;
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
    Wander,
    Follow,
    /// Default: she stays where you put her (G toggles wandering).
    #[default]
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

/// System monitor tunables (`[monitor]`, M4). Thresholds default to the
/// `[thresholds]` numbers in `assets/rules.toml` (Python prototype parity);
/// rule `cooldown_s` values still apply on top inside the brain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MonitorConfig {
    /// Master switch (`lillith --no-monitor` overrides it).
    pub enabled: bool,
    /// procfs/sysfs sampling period (s).
    pub poll_s: f32,
    /// Minimum seconds between two emissions of the same event by the
    /// monitor itself (anti-spam; `user.activity` has its own period).
    pub event_cooldown_s: f32,
    /// `cpu.high` when total CPU ≥ this for `cpu_high_sustain_s`.
    pub cpu_high_percent: f32,
    pub cpu_high_sustain_s: f32,
    /// Re-arms once CPU has stayed below this for `cpu_rearm_sustain_s`.
    pub cpu_rearm_percent: f32,
    pub cpu_rearm_sustain_s: f32,
    /// `ram.high` (MemTotal − MemAvailable) ≥ this for `ram_high_sustain_s`.
    pub ram_high_percent: f32,
    pub ram_high_sustain_s: f32,
    pub ram_rearm_percent: f32,
    /// `battery.low` at or below this while discharging.
    pub battery_low_percent: f32,
    /// Re-arms at or above this, or as soon as AC is plugged in.
    pub battery_rearm_percent: f32,
    /// `net.lost` / `net.restored` only after the new state held this long.
    pub net_debounce_s: f32,
    /// `brightness.changed` when the backlight moved at least this many points.
    pub brightness_min_delta_percent: f32,
    /// Watch removable block devices (`usb.added` / `usb.removed`).
    pub usb: bool,
    /// Detect VPN tunnels (tun/wg/ppp interfaces) → `vpn.up`.
    pub vpn: bool,
    /// Desktop-wide idle detection (ext-idle-notify on Wayland, XScreenSaver on X11).
    pub idle: bool,
    /// Input within this many seconds counts as "the user is active".
    pub activity_window_s: f32,
    /// While active, `user.activity` is sent at most this often (keeps her awake).
    pub activity_every_s: f32,
    /// Idle this long = away; the next input sends `user.returned`.
    pub away_after_s: f32,
    /// inotify on Trash / Screenshots / Downloads.
    pub files: bool,
    /// Session-bus notification monitor (`notify.received`, `calendar.reminder`, `update.available`).
    pub notifications: bool,
    /// Serve `org.lillith.Bridge` for the KWin script (`window.*`).
    pub kwin_bridge: bool,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_s: 2.0,
            event_cooldown_s: 20.0,
            cpu_high_percent: 72.0,
            cpu_high_sustain_s: 10.0,
            cpu_rearm_percent: 62.0,
            cpu_rearm_sustain_s: 10.0,
            ram_high_percent: 85.0,
            ram_high_sustain_s: 10.0,
            ram_rearm_percent: 80.0,
            battery_low_percent: 15.0,
            battery_rearm_percent: 20.0,
            net_debounce_s: 4.0,
            brightness_min_delta_percent: 2.0,
            usb: true,
            vpn: true,
            idle: true,
            activity_window_s: 10.0,
            activity_every_s: 30.0,
            away_after_s: 120.0,
            files: true,
            notifications: true,
            kwin_bridge: true,
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
    /// Legacy top-level override of `[behavior] pose_cycle_s` (unset = use
    /// `[behavior]`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pose_cycle_s: Option<f32>,
    /// Draw baked-jiggle `<clip>_jiggle` variants instead of the engine-clean
    /// clips (engine chest/pigtail/hip springs off while one is drawn).
    pub baked_jiggle: bool,
    pub muted: bool,
    pub voice_enabled: bool,
    pub llm_enabled: bool,
    /// `ollama` or `llama-server` (open question, PLAN.md §6 Q4).
    pub llm_backend: String,
    pub llm_url: String,
    pub llm_model: String,
    pub overlay: OverlayPos,
    /// M3 behaviour timings (`[behavior]`).
    pub behavior: BehaviorConfig,
    /// M4 system monitor (`[monitor]`).
    pub monitor: MonitorConfig,
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
            pose_cycle_s: None,
            baked_jiggle: false,
            muted: false,
            voice_enabled: true,
            llm_enabled: true,
            llm_backend: "ollama".into(),
            llm_url: "http://127.0.0.1:11434".into(),
            llm_model: "dolphin3".into(),
            overlay: OverlayPos::default(),
            behavior: BehaviorConfig::default(),
            monitor: MonitorConfig::default(),
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
    save_key(path, "overlay", toml::Value::try_from(pos)?)
}

/// Writes the top-level `scale` of `path` (the menu's Size ▸ pick), keeping
/// the other keys like [`save_overlay`].
pub fn save_scale(path: &Path, scale: f32) -> Result<(), ConfigError> {
    // Round to 2 decimals so 0.55 doesn't land as 0.550000011920929.
    let v = (scale as f64 * 100.0).round() / 100.0;
    save_key(path, "scale", toml::Value::Float(v))
}

/// Sets top-level `key` = `value` in `path` (creating file and directory).
fn save_key(path: &Path, key: &str, value: toml::Value) -> Result<(), ConfigError> {
    let mut table: toml::Table = match std::fs::read_to_string(path) {
        Ok(s) => s.parse()?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(e) => return Err(e.into()),
    };
    table.insert(key.into(), value);
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
    fn save_scale_keeps_other_keys() {
        let dir = std::env::temp_dir().join(format!("lillith-cfg-scale-{}", std::process::id()));
        let p = dir.join("config.toml");
        save_overlay(
            &p,
            &OverlayPos {
                margin_left: 7,
                ..OverlayPos::default()
            },
        )
        .unwrap();
        save_scale(&p, 0.75).unwrap();
        let c = Config::load(&p).unwrap();
        assert_eq!(c.scale, 0.75);
        assert_eq!(c.overlay.margin_left, 7);
        save_scale(&p, 0.45).unwrap();
        assert_eq!(Config::load(&p).unwrap().scale, 0.45);
        let _ = std::fs::remove_dir_all(dir);
    }

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
    fn monitor_table() {
        let c =
            Config::from_toml_str("[monitor]\nenabled = false\ncpu_high_percent = 90.0\n").unwrap();
        assert!(!c.monitor.enabled);
        assert_eq!(c.monitor.cpu_high_percent, 90.0);
        assert_eq!(c.monitor.battery_low_percent, 15.0);
        assert!(c.monitor.cpu_rearm_percent < MonitorConfig::default().cpu_high_percent);
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
