//! Text-to-speech fallback chain. TODO(M8): piper model path, playback via rodio.

use tokio::process::Command;

/// TTS engines in preference order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Piper,
    EspeakNg,
    SpdSay,
}

/// The default chain (same as the Python module).
pub const CHAIN: &[Engine] = &[Engine::Piper, Engine::EspeakNg, Engine::SpdSay];

impl Engine {
    /// Binary name looked up on `$PATH`.
    pub fn binary(self) -> &'static str {
        match self {
            Engine::Piper => "piper",
            Engine::EspeakNg => "espeak-ng",
            Engine::SpdSay => "spd-say",
        }
    }
}

/// Speaks `text` with the first engine that works. Returns the engine used,
/// or `None` (silent) if none is installed.
pub async fn speak(text: &str) -> Option<Engine> {
    // Never read emoji / emoticons aloud; nothing speakable → silent.
    let spoken = crate::speech_text::speech_text(text);
    if spoken.is_empty() {
        return None;
    }
    let text = spoken.as_str();
    for &engine in CHAIN {
        let status = match engine {
            // TODO(M8): piper needs a voice model and pipes raw audio to a player.
            Engine::Piper => continue,
            Engine::EspeakNg => {
                Command::new("espeak-ng")
                    .args(["-v", "en+f3", "-s", "145", "-p", "50", text])
                    .status()
                    .await
            }
            Engine::SpdSay => Command::new("spd-say").args(["-w", text]).status().await,
        };
        if matches!(status, Ok(s) if s.success()) {
            return Some(engine);
        }
        tracing::debug!(engine = engine.binary(), "tts engine unavailable");
    }
    None
}
