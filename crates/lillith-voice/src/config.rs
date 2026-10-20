//! `[voice]` table of `~/.config/lillith/config.toml`.
//!
//! ```toml
//! [voice]
//! enabled = true            # master switch (legacy top-level `voice_enabled` is the fallback)
//! engine = "auto"           # auto | piper | espeak-ng | spd-say | none
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
//! ```
//!
//! lillith-core's `Config` ignores unknown tables, so this table lives here
//! and is parsed from the same file text with [`VoiceConfig::from_config_toml`].

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Which TTS engine to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EngineChoice {
    /// First installed of piper (needs `model`) → espeak-ng → spd-say → none.
    #[default]
    Auto,
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
    }
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
    fn home_is_expanded() {
        if let Some(home) = std::env::var_os("HOME") {
            let c = VoiceConfig::from_config_toml("[voice]\nmodel = \"~/v/a.onnx\"\n").unwrap();
            assert_eq!(c.model.unwrap(), PathBuf::from(home).join("v/a.onnx"));
        }
    }
}
