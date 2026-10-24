//! Text-to-speech engine names. Speaking itself goes through
//! [`crate::engine`] (argv-safe, `--`-terminated). The old `speak()` here,
//! which ran `espeak-ng … <text>` / `spd-say -w <text>` with the raw text
//! as argv and had no callers, was removed (legacy-launches #31–32).

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
