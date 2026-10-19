//! Sound effects (README_SOUNDS, 19 cues; no audio files exist yet). TODO(M9).

pub use rodio;

/// Cue names referenced by `assets/rules.toml`.
pub const CUES: &[&str] = &[
    "alert",
    "battery",
    "disconnect",
    "soft",
    "wifi",
    "vpn",
    "click",
    "hack",
    "clean",
    "help",
    "hover",
    "peek",
    "drag",
    "tail",
    "sleep",
    "error",
];
