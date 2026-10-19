//! Push-to-talk speech input. TODO(M8).
//!
//! v1: record 5 s with `pw-record` (PipeWire) or `arecord`, then run
//! `whisper-cli -m <model> -f <wav> -otxt`. v2: `whisper-rs` + `cpal`.

/// Recorder binaries in preference order.
pub const RECORDERS: &[&str] = &["pw-record", "arecord"];
/// Transcriber binaries in preference order.
pub const TRANSCRIBERS: &[&str] = &["whisper-cli", "whisper.cpp", "whisper"];
/// Push-to-talk recording length in seconds.
pub const RECORD_SECONDS: u32 = 5;
