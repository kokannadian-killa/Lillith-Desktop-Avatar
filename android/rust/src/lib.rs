//! # lillith-android
//!
//! The Android build of Lillith reuses the desktop brain
//! ([`lillith_core::state::Brain`]), animator and clip set
//! ([`lillith_anim`]) unchanged. This crate is the thin, platform-neutral
//! [`engine::Engine`] that the Android overlay service drives once per
//! display frame, plus (only on `target_os = "android"`) the JNI entry
//! points in `jni_api`.
//!
//! - [`engine`]: brain + animator + render + window placement, touch
//!   gestures (tap, long-press, drag) and gravity to the bottom of the
//!   screen.
//! - [`pages`]: an LRU, byte-budgeted atlas page cache (phones can't keep
//!   every decoded sheet resident like the desktop `PageCache` does).
//! - [`fall`]: the drop → fall → bounce → land physics.
//!
//! The local LLM is not here: it is llama.cpp behind a small C JNI shim
//! (`android/llm`), fully on device. No crate in the Android build opens a
//! network socket.

pub mod engine;
pub mod fall;
pub mod pages;

#[cfg(target_os = "android")]
mod jni_api;

/// The system prompt the on-device chat uses (same persona as desktop).
pub fn system_prompt() -> &'static str {
    lillith_setup::modelfile::LILY_SYSTEM_PROMPT
}
