//! # lillith-voice
//!
//! Voice in and out (PLAN.md §3.3, M8), ported from `lillith_tts_engine.py`
//! and `lillith_voice_input.py`:
//!
//! - [`config`]: the `[voice]` config table ([`VoiceConfig`]).
//! - [`lines`]: [`LineSource`] (event + mood → line) and the PG
//!   [`DefaultLines`] table.
//! - `engine` (feature `voice`): Piper → aplay/paplay, espeak-ng, spd-say
//!   subprocess engines, detected once at start-up; silent no-op otherwise.
//! - `speech` (feature `voice`): [`Voice`], the handle the app owns: cooldown
//!   gate, bounded drop-oldest queue, interrupt, runtime mute, and one speech
//!   worker thread (never the render thread).
//! - [`tts`]: the older async fallback chain (kept for the CLI).
//! - [`stt`]: push-to-talk, `pw-record`/`arecord` 5 s → `whisper-cli`;
//!   later `whisper-rs` + `cpal`.
//! - `sfx` (feature `audio`): README_SOUNDS cues via `rodio`.
//!
//! Hook (overlay `Controller`, see HANDOFF.md):
//!
//! ```ignore
//! let voice = lillith_voice::Voice::from_config(VoiceConfig::load(&path)?);
//! // every batch of brain actions the overlay applies:
//! voice.on_actions(&actions);            // Action::Say → speak, Action::Muted → mute
//! // optional, for events whose rule has no line:
//! voice.on_event(&ev, brain.mood(), &actions);
//! ```

pub mod config;
#[cfg(feature = "voice")]
pub mod engine;
pub mod lines;
#[cfg(feature = "audio")]
pub mod sfx;
#[cfg(feature = "voice")]
pub mod speech;
pub mod stt;
pub mod tts;

pub use config::{EngineChoice, PlayerChoice, VoiceConfig};
pub use lines::{DefaultLines, LineSource, NoLines};
#[cfg(feature = "voice")]
pub use speech::{Dropped, Gate, Stats, Urgency, Voice};
