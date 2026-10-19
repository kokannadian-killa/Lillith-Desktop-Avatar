//! Commands emitted by the core towards the overlay, voice and mind.

use crate::mood::Mood;
use serde::{Deserialize, Serialize};

/// An instruction produced by the behaviour core.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// Show a pose or clip, optionally followed by another one.
    PlayPose {
        pose: String,
        then: Option<String>,
        gesture: Option<String>,
    },
    /// Show a speech bubble (and speak it if voice is enabled).
    Say(String),
    /// Change mood.
    SetMood(Mood),
    /// Play a sound-effect cue (README_SOUNDS name).
    PlaySfx(String),
    /// Move the avatar to a screen position (layer-shell margins on Wayland).
    MoveTo { x: i32, y: i32 },
    /// Apply a physics impulse to a spring channel.
    Impulse { channel: String, strength: f32 },
}
