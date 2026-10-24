//! `[voice]` table of `~/.config/lillith/config.toml`.
//!
//! ```toml
//! [voice]
//! enabled = true            # master switch (legacy top-level `voice_enabled` is the fallback)
//! engine = "auto"           # auto | daemon | piper | espeak-ng | spd-say | none
//! model = "~/.local/share/lillith/piper/en_US-amy-medium.onnx"   # piper voice model (.onnx)
//! voice = "female1"         # spd-say voice type / espeak-ng voice (optional)
//! rate = 1.0                # 0.5 .. 2.0, 1.0 = engine default speed
//! volume = 0.8              # 0.0 .. 1.0
//! mute = false              # start muted (legacy top-level `muted` is the fallback)
//! cooldown_s = 8.0          # min seconds between two lines (anti-chatter)
//! repeat_cooldown_s = 90.0  # the same line again only after this long
//! queue = 3                 # lines waiting at most (oldest dropped when full)
//! max_chars = 240           # longer lines are cut at a word boundary
//! player = "auto"           # piper playback: auto | aplay | paplay
//! daemon_socket = "/run/user/1000/lillith-voice/voiced.sock"  # local voice daemon (optional)
//! preset = "lillith"        # daemon voice preset (falls back to "p1")
//!
//! [voice.stt]
//! enabled = true            # bubble mic button (push-to-talk via the daemon)
//! timeout_s = 20.0          # longest wait for a transcript
//! ```
//!
//! `engine = "daemon"` (alias `voiced`) uses the local voice daemon
//! (`lillith-voice-kit/PROTOCOL.md`); `auto` also tries it first. Either
//! way, when the socket is missing or does not answer `ping`, the
//! subprocess chain below is used instead.
//!
//! lillith-core's `Config` ignores unknown tables, so this table lives here
//! and is parsed from the same file text with [`VoiceConfig::from_config_toml`].

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Which TTS engine to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EngineChoice {
    /// The local voice daemon if it answers `ping`, else the first installed
    /// of piper (needs `model`) → espeak-ng → spd-say → none.
    #[default]
    Auto,
    /// The local voice daemon (`voiced`, Unix socket / named pipe only);
    /// falls back like `auto` when it is not running.
    #[serde(alias = "voiced")]
    Daemon,
    Piper,
    #[serde(alias = "espeak", alias = "espeak_ng")]
    EspeakNg,
    #[serde(alias = "spd_say", alias = "speech-dispatcher")]
    SpdSay,
    /// Never speak (lines are still accepted and dropped quietly).
    None,
}

/// Raw-audio player for piper output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayerChoice {
    /// `aplay` (ALSA), else `paplay` (PulseAudio / PipeWire-pulse).
    #[default]
    Auto,
    Aplay,
    Paplay,
}

/// Default daemon voice preset (Ko's cloned voice).
pub const DEFAULT_PRESET: &str = "lillith";
/// Preset used when the daemon has no [`DEFAULT_PRESET`].
pub const FALLBACK_PRESET: &str = "p1";

/// `[voice.stt]`: the chat bubble's push-to-talk mic (through the daemon).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SttConfig {
    /// Show the mic button (only when the daemon answers).
    pub enabled: bool,
    /// Longest wait for a transcript after the button is released (s).
    pub timeout_s: f32,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_s: 20.0,
        }
    }
}

/// The `[voice]` table. Every key is optional.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    pub enabled: bool,
    pub engine: EngineChoice,
    /// Piper `.onnx` voice model (its `.onnx.json` sits next to it). `~/` is expanded.
    pub model: Option<PathBuf>,
    /// spd-say voice type (`female1`, …) or espeak-ng voice (`en+f3`).
    pub voice: Option<String>,
    /// Speaking speed multiplier (1.0 = engine default), clamped to 0.5..=2.0.
    pub rate: f32,
    /// 0..=1.
    pub volume: f32,
    /// Start muted.
    pub mute: bool,
    pub cooldown_s: f32,
    pub repeat_cooldown_s: f32,
    /// Bounded queue length (≥ 1).
    pub queue: usize,
    pub max_chars: usize,
    pub player: PlayerChoice,
    /// Voice daemon socket (Unix) / pipe (Windows). `None`:
    /// `$LILLITH_VOICE_SOCKET`, else `$XDG_RUNTIME_DIR/lillith-voice/voiced.sock`
    /// (Windows: `$LILLITH_VOICE_PIPE`, else `\\.\pipe\lillith-voice`).
    pub daemon_socket: Option<PathBuf>,
    /// Daemon voice preset ([`DEFAULT_PRESET`], else [`FALLBACK_PRESET`]).
    pub preset: Option<String>,
    pub stt: SttConfig,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            engine: EngineChoice::Auto,
            model: None,
            voice: None,
            rate: 1.0,
            volume: 0.8,
            mute: false,
            cooldown_s: 8.0,
            repeat_cooldown_s: 90.0,
            queue: 3,
            max_chars: 240,
            player: PlayerChoice::Auto,
            daemon_socket: None,
            preset: None,
            stt: SttConfig::default(),
        }
    }
}

impl VoiceConfig {
    /// Parses the `[voice]` table out of a whole config.toml text. A missing
    /// table gives the defaults; the legacy top-level `voice_enabled` and
    /// `muted` keys fill `enabled` / `mute` when the table does not set them.
    pub fn from_config_toml(text: &str) -> Result<Self, toml::de::Error> {
        let root: toml::Table = text.parse()?;
        let mut table = match root.get("voice") {
            Some(toml::Value::Table(t)) => t.clone(),
            _ => toml::Table::new(),
        };
        if !table.contains_key("enabled") {
            if let Some(v @ toml::Value::Boolean(_)) = root.get("voice_enabled") {
                table.insert("enabled".into(), v.clone());
            }
        }
        if !table.contains_key("mute") {
            if let Some(v @ toml::Value::Boolean(_)) = root.get("muted") {
                table.insert("mute".into(), v.clone());
            }
        }
        let mut cfg: VoiceConfig = toml::Value::Table(table).try_into()?;
        cfg.sanitize();
        Ok(cfg)
    }

    /// Reads `path` (a missing file gives the defaults).
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(s) => Self::from_config_toml(&s).map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Clamps numbers into range and expands `~/` in `model`.
    pub fn sanitize(&mut self) {
        let fin = |v: f32, d: f32| if v.is_finite() { v } else { d };
        self.rate = fin(self.rate, 1.0).clamp(0.5, 2.0);
        self.volume = fin(self.volume, 0.8).clamp(0.0, 1.0);
        self.cooldown_s = fin(self.cooldown_s, 8.0).max(0.0);
        self.repeat_cooldown_s = fin(self.repeat_cooldown_s, 90.0).max(0.0);
        self.queue = self.queue.clamp(1, 32);
        self.max_chars = self.max_chars.clamp(16, 2000);
        if let Some(m) = &self.model {
            self.model = Some(expand_home(m));
        }
        if let Some(m) = &self.daemon_socket {
            self.daemon_socket = Some(expand_home(m));
        }
        self.stt.timeout_s = fin(self.stt.timeout_s, 20.0).clamp(1.0, 120.0);
    }

    /// The daemon preset to ask for first.
    pub fn preset(&self) -> &str {
        self.preset
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .unwrap_or(DEFAULT_PRESET)
    }
}

/// Persists the "Speak out loud (speakers)" switch as `[voice] enabled`
/// in `path` (other keys kept; creates the file if missing).
pub fn save_enabled(path: &Path, on: bool) -> Result<(), lillith_core::config::ConfigError> {
    lillith_core::config::update_config(path, |t| {
        let entry = t
            .entry("voice")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if !entry.is_table() {
            *entry = toml::Value::Table(toml::Table::new());
        }
        if let Some(v) = entry.as_table_mut() {
            v.insert("enabled".into(), toml::Value::Boolean(on));
        }
    })
}

/// `~/x` → `$HOME/x`.
pub fn expand_home(p: &Path) -> PathBuf {
    match (p.strip_prefix("~"), std::env::var_os("HOME")) {
        (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => p.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_table_gives_defaults() {
        let c = VoiceConfig::from_config_toml("scale = 0.55\n").unwrap();
        assert_eq!(c, VoiceConfig::default());
        assert!(c.enabled && !c.mute);
        assert_eq!(c.engine, EngineChoice::Auto);
    }

    #[test]
    fn full_table_parses() {
        let c = VoiceConfig::from_config_toml(
            "[voice]\nenabled = false\nengine = \"spd-say\"\nmodel = \"/m/amy.onnx\"\n\
             voice = \"female2\"\nrate = 1.25\nvolume = 0.5\nmute = true\ncooldown_s = 3.0\n\
             queue = 5\nplayer = \"paplay\"\n",
        )
        .unwrap();
        assert!(!c.enabled && c.mute);
        assert_eq!(c.engine, EngineChoice::SpdSay);
        assert_eq!(c.model.as_deref(), Some(Path::new("/m/amy.onnx")));
        assert_eq!(c.voice.as_deref(), Some("female2"));
        assert_eq!(
            (c.rate, c.volume, c.cooldown_s, c.queue),
            (1.25, 0.5, 3.0, 5)
        );
        assert_eq!(c.player, PlayerChoice::Paplay);
        assert_eq!(c.repeat_cooldown_s, 90.0, "unset keys keep defaults");
    }

    #[test]
    fn engine_names_and_aliases() {
        for (s, e) in [
            ("auto", EngineChoice::Auto),
            ("piper", EngineChoice::Piper),
            ("espeak-ng", EngineChoice::EspeakNg),
            ("espeak", EngineChoice::EspeakNg),
            ("spd-say", EngineChoice::SpdSay),
            ("none", EngineChoice::None),
            ("daemon", EngineChoice::Daemon),
            ("voiced", EngineChoice::Daemon),
        ] {
            let c = VoiceConfig::from_config_toml(&format!("[voice]\nengine = \"{s}\"\n")).unwrap();
            assert_eq!(c.engine, e, "{s}");
        }
        assert!(VoiceConfig::from_config_toml("[voice]\nengine = \"festival\"\n").is_err());
    }

    #[test]
    fn legacy_keys_and_clamping() {
        let c = VoiceConfig::from_config_toml("voice_enabled = false\nmuted = true\n").unwrap();
        assert!(!c.enabled && c.mute);
        // The table wins over the legacy keys.
        let c = VoiceConfig::from_config_toml("muted = true\n[voice]\nmute = false\n").unwrap();
        assert!(!c.mute);
        let c = VoiceConfig::from_config_toml("[voice]\nrate = 9.0\nvolume = -1.0\nqueue = 0\n")
            .unwrap();
        assert_eq!((c.rate, c.volume, c.queue), (2.0, 0.0, 1));
    }

    #[test]
    fn save_enabled_round_trips_and_keeps_other_keys() {
        let dir = std::env::temp_dir().join(format!("lillith-voice-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("config.toml");
        save_enabled(&path, false).unwrap();
        assert!(
            !VoiceConfig::load(&path).unwrap().enabled,
            "creates the file"
        );
        std::fs::write(
            &path,
            "scale = 0.6\n[voice]\nengine = \"espeak-ng\"\nenabled = false\n",
        )
        .unwrap();
        save_enabled(&path, true).unwrap();
        let c = VoiceConfig::load(&path).unwrap();
        assert!(c.enabled);
        assert_eq!(c.engine, EngineChoice::EspeakNg);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("scale = 0.6"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn daemon_socket_preset_and_stt_parse() {
        let c = VoiceConfig::default();
        assert_eq!(c.preset(), "lillith");
        assert!(c.daemon_socket.is_none() && c.stt.enabled);
        let c = VoiceConfig::from_config_toml(
            "[voice]\nengine = \"daemon\"\ndaemon_socket = \"/run/user/1000/v.sock\"\n\
             preset = \"p1\"\n[voice.stt]\nenabled = false\ntimeout_s = 500.0\n",
        )
        .unwrap();
        assert_eq!(c.engine, EngineChoice::Daemon);
        assert_eq!(
            c.daemon_socket.as_deref(),
            Some(Path::new("/run/user/1000/v.sock"))
        );
        assert_eq!(c.preset(), "p1");
        assert!(!c.stt.enabled);
        assert_eq!(c.stt.timeout_s, 120.0, "clamped");
    }

    #[test]
    fn home_is_expanded() {
        if let Some(home) = std::env::var_os("HOME") {
            let c = VoiceConfig::from_config_toml("[voice]\nmodel = \"~/v/a.onnx\"\n").unwrap();
            assert_eq!(c.model.unwrap(), PathBuf::from(home).join("v/a.onnx"));
        }
    }
}
