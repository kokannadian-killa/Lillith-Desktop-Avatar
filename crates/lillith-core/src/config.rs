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

impl Springs {
    /// The art's "flirty" spring preset (`spring_proposal.json`
    /// `mood_presets.flirty`, TASTEFUL_CHANGE): chest and pigtail damping a
    /// little lower (one more soft rebound), pigtail sway +0.05; stiffness,
    /// the other gains and clamps unchanged.
    pub fn flirty() -> Self {
        let mut s = Self::default();
        s.chest.damping = 8.5;
        s.pig.damping = 5.8;
        s.pigtail_sway_gain = 0.95;
        s
    }
}

/// How her secondary motion (chest, pigtails, hip) is produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JiggleMode {
    /// Baked-jiggle `<clip>_jiggle` art (engine chest/pigtail/hip springs
    /// off while one is drawn).
    Baked,
    /// Engine-clean clips with the engine springs.
    #[default]
    Springs,
    /// Engine-clean clips, chest/pigtail springs quiet (gains 0).
    Off,
}

impl JiggleMode {
    pub const ALL: [JiggleMode; 3] = [JiggleMode::Baked, JiggleMode::Springs, JiggleMode::Off];

    pub fn label(self) -> &'static str {
        match self {
            JiggleMode::Baked => "Baked jiggle",
            JiggleMode::Springs => "Engine springs",
            JiggleMode::Off => "Off",
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

/// Optional local `llama-server` the app starts itself (`[chat.server]`).
/// Both `binary` and `model_path` must be set; otherwise nothing is spawned
/// and the app only talks to whatever already serves [`ChatConfig::endpoint`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlamaServerConfig {
    /// Path to the `llama-server` executable (empty = don't spawn).
    pub binary: String,
    /// Path to the `.gguf` model file (empty = don't spawn).
    pub model_path: String,
    /// Context window (`-c`).
    pub ctx_size: u32,
    /// CPU threads (`-t`); 0 = llama-server's default.
    pub threads: u32,
    /// Layers to offload to the GPU (`-ngl`); `None` = llama-server's default.
    pub gpu_layers: Option<i32>,
    /// Extra command-line arguments, passed through verbatim.
    pub extra_args: Vec<String>,
    /// How long to wait for `/health` after spawning (s).
    pub startup_timeout_s: f32,
}

impl Default for LlamaServerConfig {
    fn default() -> Self {
        Self {
            binary: String::new(),
            model_path: String::new(),
            ctx_size: 4096,
            threads: 0,
            gpu_layers: None,
            extra_args: Vec::new(),
            startup_timeout_s: 90.0,
        }
    }
}

impl LlamaServerConfig {
    /// `true` when both paths are set, i.e. the app should spawn the server.
    pub fn should_spawn(&self) -> bool {
        !self.binary.trim().is_empty() && !self.model_path.trim().is_empty()
    }
}

/// Click-to-chat with a local llama.cpp `llama-server` (`[chat]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatConfig {
    /// Master switch for the chat bubble.
    pub enabled: bool,
    /// A plain left click on her opens the bubble (`Y`/`Enter` always do).
    pub open_on_click: bool,
    /// Base URL of the OpenAI-compatible server (`/v1/chat/completions`
    /// is appended).
    pub endpoint: String,
    /// `model` field of the request (llama-server serves whatever it
    /// loaded and ignores it; other OpenAI-compatible servers need it).
    pub model: String,
    /// System prompt; empty = Lillith's built-in persona.
    pub system_prompt: String,
    pub temperature: f32,
    /// Reply length cap (`max_tokens`).
    pub max_tokens: u32,
    /// How many past user/assistant turns are sent with each message.
    pub history_turns: usize,
    /// Whole-request timeout (s); the connect timeout is fixed at 3 s.
    pub timeout_s: f32,
    /// Keep the conversation across bubble openings and restarts
    /// (`~/.local/share/lillith/chat_history.json`).
    pub save_history: bool,
    /// Optional `Authorization: Bearer` key (llama-server `--api-key`).
    pub api_key: String,
    /// Show her replies in a small speech bubble over her head on the
    /// overlay (the overlay window grows upward to make room).
    pub overlay_speech: bool,
    /// Draw the baked-jiggle talk clips (`talk_loop*_jiggle`) while she
    /// speaks when baked-jiggle mode is on (`baked_jiggle` /
    /// `--baked-jiggle`); `false` keeps the engine-clean talk clips.
    pub talk_jiggle: bool,
    /// Optional self-spawned server.
    pub server: LlamaServerConfig,
}

impl Default for ChatConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            open_on_click: true,
            endpoint: "http://127.0.0.1:8080".into(),
            model: "local".into(),
            system_prompt: String::new(),
            // Matches lillith_setup::modelfile::LILY_TEMPERATURE (pinned by a
            // test in lillith-mind, which depends on both crates).
            temperature: 0.85,
            max_tokens: 320,
            history_turns: 12,
            timeout_s: 120.0,
            save_history: true,
            api_key: String::new(),
            overlay_speech: true,
            talk_jiggle: true,
            server: LlamaServerConfig::default(),
        }
    }
}

/// Animation pacing (`[anim]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimConfig {
    /// Global clip playback speed (frame timing only; physics and timers
    /// stay real-time). 1.0 = as authored, 0.7 = 30% slower.
    pub playback_speed: f32,
    /// Lip-flap pace while she talks: 1.0 = the original fast pace; the
    /// default 0.62 changes mouth shapes ~38% less often and holds the
    /// end-of-reply smile ~1.5 s.
    pub talk_speed: f32,
    /// Smooth mode for her sprite clips: cross-fade into the next frame
    /// toward the end of each frame instead of hard steps.
    pub smooth_frames: bool,
    /// Fraction (0..1) of each frame spent cross-fading (smooth mode).
    pub crossfade: f32,
}

impl Default for AnimConfig {
    fn default() -> Self {
        Self {
            playback_speed: 0.7,
            talk_speed: 0.62,
            smooth_frames: true,
            crossfade: 0.5,
        }
    }
}

/// `[avatar]`: custom (single-image) avatars, animated procedurally.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AvatarConfig {
    /// Motion pace of a custom avatar (1 = default; lower = slower).
    pub pace: f32,
    /// Redraw rate (Hz) while a custom avatar is shown: it moves every
    /// frame, so a higher rate looks smoother (at some CPU cost).
    pub render_hz: f32,
}

impl Default for AvatarConfig {
    fn default() -> Self {
        Self {
            pace: 0.8,
            render_hz: 60.0,
        }
    }
}

/// Overlay scale used when config `scale` is left at 1.0 (its default):
/// ~220 px tall from the 400 px cells (Ko's choice).
pub const DEFAULT_OVERLAY_SCALE: f32 = 0.55;

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
    /// Superseded by `jiggle_mode` when that is set.
    pub baked_jiggle: bool,
    /// `baked` / `springs` / `off` (unset = from `baked_jiggle`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jiggle_mode: Option<JiggleMode>,
    /// Keep the overlay above other windows.
    pub always_on_top: bool,
    /// Clicks pass through her body to the desktop; only her head stays
    /// clickable (a grab handle for the menu and dragging).
    pub click_through: bool,
    /// Start Lillith with the desktop session (XDG autostart entry).
    pub autostart: bool,
    pub avatar: AvatarConfig,
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
    /// Click-to-chat with a local llama-server (`[chat]`).
    pub chat: ChatConfig,
    /// Animation pacing (`[anim]`).
    pub anim: AnimConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            springs: Springs::default(),
            mode: MoveMode::default(),
            // A 300 px tall window from the 400 px cells.
            scale: 1.0,
            opacity: 1.0,
            physics_hz: 120.0,
            render_hz: 30.0,
            sprite_fps: 12.0,
            pose_cycle_s: None,
            baked_jiggle: false,
            jiggle_mode: None,
            always_on_top: true,
            click_through: false,
            autostart: false,
            avatar: AvatarConfig::default(),
            muted: false,
            voice_enabled: true,
            llm_enabled: true,
            llm_backend: "ollama".into(),
            llm_url: "http://127.0.0.1:11434".into(),
            llm_model: "dolphin3".into(),
            overlay: OverlayPos::default(),
            behavior: BehaviorConfig::default(),
            monitor: MonitorConfig::default(),
            chat: ChatConfig::default(),
            anim: AnimConfig::default(),
        }
    }
}

impl Config {
    /// The overlay scale this config asks for: `scale = 1.0` (the default)
    /// means [`DEFAULT_OVERLAY_SCALE`]; any other positive value is used
    /// as is.
    pub fn overlay_scale(&self) -> f32 {
        let s = self.scale;
        if (s - 1.0).abs() < 1e-6 || !s.is_finite() || s <= 0.0 {
            DEFAULT_OVERLAY_SCALE
        } else {
            s
        }
    }

    /// Effective jiggle mode (`jiggle_mode`, else from `baked_jiggle`).
    pub fn jiggle(&self) -> JiggleMode {
        self.jiggle_mode.unwrap_or(if self.baked_jiggle {
            JiggleMode::Baked
        } else {
            JiggleMode::Springs
        })
    }

    /// Writes the whole config to `path` (atomically), keeping unknown
    /// top-level keys/tables already in the file.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let me = toml::Value::try_from(self)?;
        update_config(path, |table| {
            if let toml::Value::Table(t) = me {
                for (k, v) in t {
                    table.insert(k, v);
                }
            }
            if self.jiggle_mode.is_none() {
                table.remove("jiggle_mode");
            }
        })
    }

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
    let v = toml::Value::try_from(pos)?;
    update_config(path, |t| {
        t.insert("overlay".into(), v);
    })
}

/// Writes the top-level `scale` of `path` (the menu's Resize pick), keeping
/// the other keys.
pub fn save_scale(path: &Path, scale: f32) -> Result<(), ConfigError> {
    // Round to 2 decimals so 0.55 doesn't land as 0.550000011920929.
    let v = (scale as f64 * 100.0).round() / 100.0;
    update_config(path, |t| {
        t.insert("scale".into(), toml::Value::Float(v));
    })
}

/// Read-modify-write of the config file as a TOML table (atomic rename;
/// creates the file and directory; comments are not kept).
pub fn update_config(path: &Path, f: impl FnOnce(&mut toml::Table)) -> Result<(), ConfigError> {
    let mut table: toml::Table = match std::fs::read_to_string(path) {
        Ok(s) => s.parse()?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(e) => return Err(e.into()),
    };
    f(&mut table);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, toml::to_string_pretty(&table)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// `$XDG_CONFIG_HOME/autostart` (else `~/.config/autostart`).
pub fn autostart_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|d| d.join("autostart"))
}

/// Writes or removes the XDG autostart entry (`lillith.desktop` in `dir`)
/// that starts `exe` with the desktop session.
pub fn set_autostart(
    dir: &std::path::Path,
    exe: &std::path::Path,
    on: bool,
) -> std::io::Result<()> {
    let file = dir.join("lillith.desktop");
    if on {
        std::fs::create_dir_all(dir)?;
        std::fs::write(
            &file,
            format!(
                "[Desktop Entry]\nType=Application\nName=Lillith\nComment=Desktop companion\nExec={}\nX-GNOME-Autostart-enabled=true\nTerminal=false\n",
                exe.display()
            ),
        )
    } else {
        match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_one_means_the_055_default() {
        let mut c = Config::default();
        assert_eq!(c.scale, 1.0);
        assert_eq!(c.overlay_scale(), DEFAULT_OVERLAY_SCALE);
        assert_eq!(DEFAULT_OVERLAY_SCALE, 0.55);
        for (s, want) in [(0.75, 0.75), (1.25, 1.25), (0.45, 0.45)] {
            c.scale = s;
            assert_eq!(c.overlay_scale(), want);
        }
        for bad in [0.0, -1.0, f32::NAN] {
            c.scale = bad;
            assert_eq!(c.overlay_scale(), DEFAULT_OVERLAY_SCALE);
        }
    }

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
    fn chat_table() {
        let c = Config::from_toml_str(
            "[chat]\nendpoint = \"http://127.0.0.1:9090\"\ntemperature = 0.5\n\
             [chat.server]\nbinary = \"/opt/llama-server\"\nmodel_path = \"/m/q.gguf\"\n",
        )
        .unwrap();
        assert_eq!(c.chat.endpoint, "http://127.0.0.1:9090");
        assert_eq!(c.chat.temperature, 0.5);
        assert_eq!(c.chat.history_turns, 12);
        assert!(c.chat.server.should_spawn());
        assert!(!ChatConfig::default().server.should_spawn());
        let a = Config::from_toml_str("[anim]\ntalk_speed = 1.0\n")
            .unwrap()
            .anim;
        assert_eq!((a.talk_speed, a.playback_speed), (1.0, 0.7));
        let d = ChatConfig::default();
        assert!(d.overlay_speech && d.talk_jiggle);
        let c = Config::from_toml_str("[chat]\noverlay_speech = false\ntalk_jiggle = false\n")
            .unwrap()
            .chat;
        assert!(!c.overlay_speech && !c.talk_jiggle);
        // Serialises (tables after plain keys).
        assert!(toml::to_string(&Config::default()).is_ok());
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

    #[test]
    fn autostart_entry() {
        let d = std::env::temp_dir().join(format!("lillith-autostart-{}", std::process::id()));
        set_autostart(&d, Path::new("/usr/bin/lillith"), true).unwrap();
        let s = std::fs::read_to_string(d.join("lillith.desktop")).unwrap();
        assert!(s.contains("Exec=/usr/bin/lillith"));
        set_autostart(&d, Path::new("/usr/bin/lillith"), false).unwrap();
        assert!(!d.join("lillith.desktop").exists());
        set_autostart(&d, Path::new("/usr/bin/lillith"), false).unwrap();
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn settings_fields_round_trip() {
        let mut c = Config::default();
        assert!(c.anim.smooth_frames && c.always_on_top && !c.click_through);
        assert_eq!(c.jiggle(), JiggleMode::Springs);
        c.baked_jiggle = true;
        assert_eq!(c.jiggle(), JiggleMode::Baked);
        c.jiggle_mode = Some(JiggleMode::Off);
        c.springs = Springs::flirty();
        c.avatar.pace = 0.6;
        let d = std::env::temp_dir().join(format!("lillith-cfg-save-{}", std::process::id()));
        let p = d.join("config.toml");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(&p, "custom_key = 3\n").unwrap();
        c.save(&p).unwrap();
        let back = Config::load(&p).unwrap();
        assert_eq!(back.jiggle(), JiggleMode::Off);
        assert_eq!(back.springs, Springs::flirty());
        assert!((back.avatar.pace - 0.6).abs() < 1e-6);
        assert!(std::fs::read_to_string(&p).unwrap().contains("custom_key"));
        save_scale(&p, 1.3).unwrap();
        assert!((Config::load(&p).unwrap().scale - 1.3).abs() < 1e-6);
        let _ = std::fs::remove_dir_all(d);
    }
}
