//! # lillith-voice
//!
//! Voice in and out (PLAN.md §3.3, M8), ported from `lillith_tts_engine.py`
//! and `lillith_voice_input.py`:
//!
//! - [`tts`]: subprocess chain piper → espeak-ng (`en+f3 -s145 -p50`) →
//!   spd-say → silent.
//! - [`stt`]: push-to-talk, `pw-record`/`arecord` 5 s → `whisper-cli`;
//!   later `whisper-rs` + `cpal`.
//! - `sfx` (feature `audio`): README_SOUNDS cues via `rodio`.

#[cfg(feature = "audio")]
pub mod sfx;
pub mod stt;
pub mod tts;
