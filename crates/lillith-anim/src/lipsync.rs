//! Audio-driven lip-sync for the speaking clip (`talk_loop`).
//!
//! The voice daemon (`lillith-voice`'s `voiced`) streams JSON lines over a
//! local socket (see `docs/LIPSYNC.md`). The app layer owns the socket; it
//! hands every line to [`AudioLipSync::feed_line`] together with a local
//! monotonic clock in milliseconds, and calls [`AudioLipSync::tick`] once per
//! frame. This module does no I/O and reads no clock: it is a pure state
//! machine, so it is fully deterministic under test.
//!
//! Per utterance (`utt`) and chunk (`chunk`) the daemon sends, ahead of
//! playback, a loudness envelope (and optionally visemes), then
//! `chunk_start` when the audio really reaches the speakers. Envelope and
//! viseme times are relative to that moment; it is mapped to the *local*
//! receive time (the sender's `mono_ms` is kept for diagnostics only, since
//! monotonic clocks are not comparable across processes).
//!
//! Shapes:
//! - envelope only: `rms < 0.08` closed, `0.08..=0.35` mid, `> 0.35` open,
//!   with ±[`HYSTERESIS`] bands so the mouth doesn't chatter on a boundary;
//! - visemes present: the viseme wins, but a quiet envelope
//!   (`rms <` [`VISEME_GATE`]) still closes the mouth;
//! - every shape is held at least [`hold_ms`] (the token lip-flap's 85 ms
//!   `FLAP_S`) before the next one shows;
//! - `utt_end`: once the last chunk has played, the
//!   [`END_SMILE_S`](crate::talk::END_SMILE_S) smile, then idle;
//! - `stop` (barge-in): closed at once, no smile, the utterance is dropped.
//!
//! [`TalkMouth`] puts this in front of the text-driven
//! [`LipFlap`](crate::talk::LipFlap): audio shapes win while audio is
//! playing, the lip-flap is the fallback (voice off, daemon not running).

use crate::talk::{LipFlap, Mouth, END_SMILE_S, FLAP_S};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Subscribe request the app writes to the voice daemon socket (one line,
/// newline-terminated) to receive the lip-sync events.
pub const SUBSCRIBE_LINE: &str = r#"{"op":"subscribe","topics":["lipsync"]}"#;
/// Linux/macOS socket path, relative to `$XDG_RUNTIME_DIR`.
pub const SOCKET_REL_PATH: &str = "lillith-voice/voiced.sock";
/// Windows named pipe.
pub const WINDOWS_PIPE: &str = r"\\.\pipe\lillith-voice";

/// Envelope at or above this (plus hysteresis) opens the mouth half-way.
pub const CLOSED_MAX: f32 = 0.08;
/// Envelope above this (plus hysteresis) opens the mouth fully.
pub const OPEN_MIN: f32 = 0.35;
/// Half-width of the hysteresis band around [`CLOSED_MAX`] and [`OPEN_MIN`].
pub const HYSTERESIS: f32 = 0.02;
/// With visemes, an envelope below this closes the mouth anyway.
pub const VISEME_GATE: f32 = 0.05;
/// A chunk whose `chunk_start` hasn't arrived this long (ms) after its
/// first data is dropped.
pub const LATE_START_MS: u64 = 2000;
/// After a chunk's data runs out (and no `chunk_end` came), it keeps
/// "playing" closed this long (ms) to absorb device jitter.
pub const CHUNK_GRACE_MS: u64 = 200;
/// Speaking but nothing playing or arriving for this long (ms) without an
/// `utt_end`: treated as ended (daemon died mid-utterance).
pub const IDLE_END_MS: u64 = 3000;
/// Most chunks kept per utterance (oldest dropped first).
const MAX_CHUNKS: usize = 64;
/// Most points/items kept per chunk.
const MAX_POINTS: usize = 20_000;
/// Stopped/finished utterance ids remembered (late events ignored).
const MAX_DEAD: usize = 32;

/// Minimum time (ms) a shape is shown: the token lip-flap's `FLAP_S`.
pub fn hold_ms() -> u64 {
    (FLAP_S * 1000.0).round() as u64
}

/// End smile length (ms).
fn end_smile_ms() -> u64 {
    (END_SMILE_S * 1000.0).round() as u64
}

/// A line that could not be understood.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LipSyncError {
    #[error("lip-sync line is not valid JSON: {0}")]
    Json(String),
    #[error("malformed lip-sync `{ev}` event: {why}")]
    Malformed { ev: String, why: String },
}

/// One event from the voice daemon's `lipsync` topic.
#[derive(Debug, Clone, PartialEq)]
pub enum LipEvent {
    /// Loudness envelope: `(t_ms, rms 0..=1)` relative to `chunk_start`.
    Envelope {
        utt: String,
        chunk: u64,
        fps: f32,
        pts: Vec<(u32, f32)>,
    },
    /// Visemes: `(t_ms, dur_ms, shape)` relative to `chunk_start`.
    Visemes {
        utt: String,
        chunk: u64,
        items: Vec<(u32, u32, Mouth)>,
    },
    /// The chunk's audio started at the output.
    ChunkStart {
        utt: String,
        chunk: u64,
        mono_ms: Option<u64>,
    },
    ChunkEnd {
        utt: String,
        chunk: u64,
    },
    /// Last chunk sent: smile once it has played.
    UttEnd {
        utt: String,
    },
    /// Barge-in / interrupt: close now, drop the utterance.
    Stop {
        utt: String,
    },
}

impl LipEvent {
    pub fn utt(&self) -> &str {
        match self {
            LipEvent::Envelope { utt, .. }
            | LipEvent::Visemes { utt, .. }
            | LipEvent::ChunkStart { utt, .. }
            | LipEvent::ChunkEnd { utt, .. }
            | LipEvent::UttEnd { utt }
            | LipEvent::Stop { utt } => utt,
        }
    }

    /// Parses one JSON line. `Ok(None)` for blank lines, lines without an
    /// `ev` (acks, other topics) and unknown events; unknown fields are
    /// ignored. Bad JSON or a known event with missing/mistyped fields is
    /// an error.
    pub fn parse(line: &str) -> Result<Option<Self>, LipSyncError> {
        let line = line.trim();
        if line.is_empty() {
            return Ok(None);
        }
        let v: Value = serde_json::from_str(line).map_err(|e| LipSyncError::Json(e.to_string()))?;
        let Some(obj) = v.as_object() else {
            return Err(LipSyncError::Malformed {
                ev: String::new(),
                why: "not a JSON object".into(),
            });
        };
        let Some(ev) = obj.get("ev").and_then(Value::as_str) else {
            return Ok(None);
        };
        let bad = |why: &str| LipSyncError::Malformed {
            ev: ev.to_string(),
            why: why.to_string(),
        };
        let known = matches!(
            ev,
            "envelope" | "visemes" | "chunk_start" | "chunk_end" | "utt_end" | "stop"
        );
        if !known {
            return Ok(None);
        }
        let utt = match obj.get("utt") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => return Err(bad("missing `utt`")),
        };
        let chunk = || {
            obj.get("chunk")
                .and_then(Value::as_u64)
                .ok_or_else(|| bad("missing or negative `chunk`"))
        };
        let ms = |v: &Value| -> Option<u32> {
            let f = v.as_f64()?;
            (f.is_finite() && f >= 0.0).then(|| f.min(u32::MAX as f64) as u32)
        };
        Ok(Some(match ev {
            "envelope" => {
                let fps = obj
                    .get("fps")
                    .and_then(Value::as_f64)
                    .filter(|f| f.is_finite() && *f > 0.0)
                    .unwrap_or(30.0) as f32;
                let arr = obj
                    .get("pts")
                    .and_then(Value::as_array)
                    .ok_or_else(|| bad("missing `pts` array"))?;
                let mut pts = Vec::with_capacity(arr.len().min(MAX_POINTS));
                for p in arr.iter().take(MAX_POINTS) {
                    let pair = p.as_array().filter(|a| a.len() >= 2);
                    let (Some(t), Some(r)) = (
                        pair.and_then(|a| ms(&a[0])),
                        pair.and_then(|a| a[1].as_f64()),
                    ) else {
                        return Err(bad("`pts` entries must be [t_ms, rms]"));
                    };
                    let r = if r.is_finite() {
                        r.clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    pts.push((t, r as f32));
                }
                pts.sort_by_key(|p| p.0);
                LipEvent::Envelope {
                    utt,
                    chunk: chunk()?,
                    fps,
                    pts,
                }
            }
            "visemes" => {
                let arr = obj
                    .get("items")
                    .and_then(Value::as_array)
                    .ok_or_else(|| bad("missing `items` array"))?;
                let mut items = Vec::with_capacity(arr.len().min(MAX_POINTS));
                for it in arr.iter().take(MAX_POINTS) {
                    let a = it.as_array().filter(|a| a.len() >= 3);
                    let (Some(t), Some(d), Some(s)) = (
                        a.and_then(|a| ms(&a[0])),
                        a.and_then(|a| ms(&a[1])),
                        a.and_then(|a| a[2].as_str()),
                    ) else {
                        return Err(bad("`items` entries must be [t_ms, dur_ms, shape]"));
                    };
                    // Unknown shape names (future visemes) are skipped.
                    if let Some(m) = Mouth::parse(s.trim()) {
                        items.push((t, d, m));
                    }
                }
                items.sort_by_key(|i| i.0);
                LipEvent::Visemes {
                    utt,
                    chunk: chunk()?,
                    items,
                }
            }
            "chunk_start" => LipEvent::ChunkStart {
                utt,
                chunk: chunk()?,
                mono_ms: obj.get("mono_ms").and_then(Value::as_u64),
            },
            "chunk_end" => LipEvent::ChunkEnd {
                utt,
                chunk: chunk()?,
            },
            "utt_end" => LipEvent::UttEnd { utt },
            _ => LipEvent::Stop { utt },
        }))
    }
}

/// Envelope-only shape after `prev`, with hysteresis.
pub fn envelope_shape(prev: Mouth, rms: f32) -> Mouth {
    let up_mid = CLOSED_MAX + HYSTERESIS;
    let down_closed = CLOSED_MAX - HYSTERESIS;
    let up_open = OPEN_MIN + HYSTERESIS;
    let down_mid = OPEN_MIN - HYSTERESIS;
    match prev {
        Mouth::Closed | Mouth::Smile => {
            if rms > up_open {
                Mouth::Open
            } else if rms >= up_mid {
                Mouth::Mid
            } else {
                Mouth::Closed
            }
        }
        Mouth::Mid => {
            if rms > up_open {
                Mouth::Open
            } else if rms < down_closed {
                Mouth::Closed
            } else {
                Mouth::Mid
            }
        }
        Mouth::Open => {
            if rms < down_closed {
                Mouth::Closed
            } else if rms < down_mid {
                Mouth::Mid
            } else {
                Mouth::Open
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
struct Chunk {
    env: Option<Vec<(u32, f32)>>,
    frame_ms: u32,
    vis: Option<Vec<(u32, u32, Mouth)>>,
    /// Local time the first envelope/visemes arrived.
    data_at: Option<u64>,
    /// Local time `chunk_start` arrived.
    start: Option<u64>,
    sender_mono: Option<u64>,
    ended_at: Option<u64>,
}

impl Chunk {
    /// Length of the data (ms after start); `None` before any data.
    fn data_len(&self) -> Option<u64> {
        let e = self
            .env
            .as_ref()
            .and_then(|e| e.last())
            .map(|p| p.0 as u64 + self.frame_ms as u64);
        let v = self
            .vis
            .as_ref()
            .and_then(|v| v.iter().map(|i| i.0 as u64 + i.1 as u64).max());
        e.max(v)
    }

    /// Local time the chunk stops playing (requires `start`).
    fn end(&self, start: u64) -> u64 {
        if let Some(e) = self.ended_at.filter(|e| *e >= start) {
            return e;
        }
        match self.data_len() {
            Some(len) => start + len + CHUNK_GRACE_MS,
            None => start + LATE_START_MS,
        }
    }

    fn rms_at(&self, t: u64) -> Option<f32> {
        let env = self.env.as_ref()?;
        let i = env.partition_point(|p| (p.0 as u64) <= t);
        if i == 0 {
            return Some(0.0);
        }
        let (pt, r) = env[i - 1];
        // Past the end of the envelope: silence.
        if t > pt as u64 + 2 * self.frame_ms.max(1) as u64 && i == env.len() {
            return Some(0.0);
        }
        Some(r)
    }

    fn viseme_at(&self, t: u64) -> Option<Option<Mouth>> {
        let vis = self.vis.as_ref().filter(|v| !v.is_empty())?;
        Some(
            vis.iter()
                .rev()
                .find(|i| (i.0 as u64) <= t && t < i.0 as u64 + i.1 as u64)
                .map(|i| i.2),
        )
    }
}

#[derive(Debug, Clone)]
struct Utt {
    id: String,
    chunks: BTreeMap<u64, Chunk>,
    /// Chunks already played (late events for them are ignored).
    done: BTreeSet<u64>,
    /// A chunk has started: the mouth is audio-driven.
    speaking: bool,
    finishing: bool,
    smile_from: Option<u64>,
    last_activity: u64,
    env_level: Mouth,
    shown: Mouth,
    /// When `shown` last changed (`None`: the first change is free).
    shown_since: Option<u64>,
}

impl Utt {
    fn new(id: String, now: u64) -> Self {
        Self {
            id,
            chunks: BTreeMap::new(),
            done: BTreeSet::new(),
            speaking: false,
            finishing: false,
            smile_from: None,
            last_activity: now,
            env_level: Mouth::Closed,
            shown: Mouth::Closed,
            shown_since: None,
        }
    }

    fn chunk(&mut self, idx: u64) -> Option<&mut Chunk> {
        if self.done.contains(&idx) {
            return None;
        }
        if !self.chunks.contains_key(&idx) && self.chunks.len() >= MAX_CHUNKS {
            let oldest = *self.chunks.keys().next()?;
            self.chunks.remove(&oldest);
            self.done.insert(oldest);
        }
        Some(self.chunks.entry(idx).or_default())
    }
}

/// Audio-driven mouth: feed it the daemon's events, tick it every frame.
#[derive(Debug, Clone, Default)]
pub struct AudioLipSync {
    utt: Option<Utt>,
    dead: VecDeque<String>,
    /// One `Closed` to return on the next tick (after `stop`).
    flash: Option<Mouth>,
    dropped_late: u64,
    last_sender_mono: Option<u64>,
}

impl AudioLipSync {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses and applies one line from the socket. Blank, unknown and
    /// non-`ev` lines are ignored (`Ok`); malformed ones are errors and
    /// leave the state untouched.
    pub fn feed_line(&mut self, line: &str, now_ms: u64) -> Result<(), LipSyncError> {
        if let Some(ev) = LipEvent::parse(line)? {
            self.feed(ev, now_ms);
        }
        Ok(())
    }

    fn is_dead(&self, id: &str) -> bool {
        self.dead.iter().any(|d| d == id)
    }

    fn kill(&mut self, id: String) {
        if !self.is_dead(&id) {
            if self.dead.len() >= MAX_DEAD {
                self.dead.pop_front();
            }
            self.dead.push_back(id);
        }
    }

    /// Ends the current utterance (no smile).
    fn drop_current(&mut self) {
        if let Some(u) = self.utt.take() {
            self.kill(u.id);
        }
    }

    /// Applies one event at local time `now_ms`.
    pub fn feed(&mut self, ev: LipEvent, now_ms: u64) {
        if self.is_dead(ev.utt()) {
            return;
        }
        let current = self.utt.as_ref().map(|u| u.id.as_str());
        if current != Some(ev.utt()) {
            match ev {
                // Ending something that isn't playing: just remember it.
                LipEvent::Stop { utt } | LipEvent::UttEnd { utt } => {
                    self.kill(utt);
                    return;
                }
                _ => {
                    // A new utterance supersedes the old one.
                    self.drop_current();
                    self.utt = Some(Utt::new(ev.utt().to_string(), now_ms));
                }
            }
        }
        if let LipEvent::Stop { .. } = ev {
            let was_speaking = self.utt.as_ref().is_some_and(|u| u.speaking);
            self.drop_current();
            if was_speaking {
                self.flash = Some(Mouth::Closed);
            }
            return;
        }
        let Some(u) = self.utt.as_mut() else { return };
        u.last_activity = u.last_activity.max(now_ms);
        match ev {
            LipEvent::Envelope {
                chunk, fps, pts, ..
            } => {
                if let Some(c) = u.chunk(chunk) {
                    c.frame_ms = (1000.0 / fps.max(1.0)).round() as u32;
                    c.env = Some(pts);
                    c.data_at.get_or_insert(now_ms);
                }
            }
            LipEvent::Visemes { chunk, items, .. } => {
                if let Some(c) = u.chunk(chunk) {
                    c.vis = Some(items);
                    c.data_at.get_or_insert(now_ms);
                }
            }
            LipEvent::ChunkStart { chunk, mono_ms, .. } => {
                if let Some(c) = u.chunk(chunk) {
                    if c.start.is_none() {
                        c.start = Some(now_ms);
                        c.sender_mono = mono_ms;
                        u.speaking = true;
                        if mono_ms.is_some() {
                            self.last_sender_mono = mono_ms;
                        }
                    }
                }
            }
            LipEvent::ChunkEnd { chunk, .. } => {
                if let Some(c) = u.chunks.get_mut(&chunk) {
                    c.ended_at.get_or_insert(now_ms);
                }
            }
            LipEvent::UttEnd { .. } => u.finishing = true,
            LipEvent::Stop { .. } => {}
        }
    }

    /// `true` while audio drives the mouth: from the first `chunk_start`
    /// of an utterance until its end smile is over (or `stop`).
    pub fn is_active(&self) -> bool {
        self.utt.as_ref().is_some_and(|u| u.speaking)
    }

    /// Current utterance id (also before its first chunk plays).
    pub fn utt(&self) -> Option<&str> {
        self.utt.as_ref().map(|u| u.id.as_str())
    }

    /// Chunks dropped because their `chunk_start` never came.
    pub fn dropped_late(&self) -> u64 {
        self.dropped_late
    }

    /// Sender `mono_ms` of the latest `chunk_start` (diagnostics only).
    pub fn last_sender_mono_ms(&self) -> Option<u64> {
        self.last_sender_mono
    }

    /// Clears everything (voice disabled, daemon disconnected).
    pub fn reset(&mut self) {
        self.drop_current();
        self.flash = None;
    }

    /// Shape to draw at local time `now_ms`; `None` = audio isn't driving
    /// the mouth (let the lip-flap / normal animation play).
    pub fn tick(&mut self, now_ms: u64) -> Option<Mouth> {
        if let Some(m) = self.flash.take() {
            return Some(m);
        }
        let u = self.utt.as_mut()?;
        // Drop chunks whose chunk_start is overdue.
        let before = u.chunks.len();
        let late: Vec<u64> = u
            .chunks
            .iter()
            .filter(|(_, c)| {
                c.start.is_none()
                    && c.data_at
                        .is_some_and(|d| now_ms.saturating_sub(d) > LATE_START_MS)
            })
            .map(|(k, _)| *k)
            .collect();
        for k in late {
            u.chunks.remove(&k);
            u.done.insert(k);
        }
        self.dropped_late += (before - u.chunks.len()) as u64;
        // Retire played chunks; find the playing one (latest start).
        let finished: Vec<u64> = u
            .chunks
            .iter()
            .filter(|(_, c)| c.start.is_some_and(|s| now_ms >= c.end(s)))
            .map(|(k, _)| *k)
            .collect();
        for k in finished {
            u.chunks.remove(&k);
            u.done.insert(k);
        }
        let playing = u
            .chunks
            .values()
            .filter(|c| c.start.is_some_and(|s| s <= now_ms))
            .max_by_key(|c| c.start)
            .cloned();
        if playing.is_some() {
            u.last_activity = u.last_activity.max(now_ms);
        }
        // End smile.
        if let Some(from) = u.smile_from {
            if now_ms.saturating_sub(from) >= end_smile_ms() {
                self.drop_current();
                return None;
            }
            return Some(Mouth::Smile);
        }
        if !u.speaking {
            if u.finishing {
                // Ended before anything played.
                self.drop_current();
            }
            return None;
        }
        let desired = match &playing {
            Some(c) => {
                let t = now_ms.saturating_sub(c.start.unwrap_or(now_ms));
                let rms = c.rms_at(t);
                if let Some(r) = rms {
                    u.env_level = envelope_shape(u.env_level, r);
                }
                match c.viseme_at(t) {
                    Some(v) => {
                        if rms.is_some_and(|r| r < VISEME_GATE) {
                            Mouth::Closed
                        } else {
                            v.unwrap_or(Mouth::Closed)
                        }
                    }
                    None if rms.is_some() => u.env_level,
                    None => Mouth::Closed,
                }
            }
            None => {
                let idle = now_ms.saturating_sub(u.last_activity) >= IDLE_END_MS;
                if u.finishing || idle {
                    u.chunks.retain(|_, c| c.start.is_some());
                    u.smile_from = Some(now_ms);
                    return Some(Mouth::Smile);
                }
                u.env_level = Mouth::Closed;
                Mouth::Closed
            }
        };
        let held = u
            .shown_since
            .is_none_or(|t| now_ms.saturating_sub(t) >= hold_ms());
        if desired != u.shown && held {
            u.shown = desired;
            u.shown_since = Some(now_ms);
        }
        Some(u.shown)
    }
}

/// The speaking mouth: audio lip-sync when the voice daemon is playing,
/// the text-token [`LipFlap`] otherwise.
#[derive(Debug, Clone, Default)]
pub struct TalkMouth {
    pub flap: LipFlap,
    pub audio: AudioLipSync,
}

impl TalkMouth {
    pub fn new(flap: LipFlap) -> Self {
        Self {
            flap,
            audio: AudioLipSync::new(),
        }
    }

    /// Advances both (`dt` s for the lip-flap, `now_ms` local monotonic for
    /// audio) and returns the shape to draw: audio first, else lip-flap.
    pub fn tick(&mut self, dt: f32, now_ms: u64) -> Option<Mouth> {
        let flap = self.flap.tick(dt);
        self.audio.tick(now_ms).or(flap)
    }

    /// Audio is driving the mouth right now.
    pub fn audio_driven(&self) -> bool {
        self.audio.is_active()
    }

    /// Either source is active.
    pub fn active(&self) -> bool {
        self.audio.is_active() || self.flap.active()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(utt: &str, chunk: u64, pts: &[(u32, f32)]) -> String {
        let p: Vec<String> = pts.iter().map(|(t, r)| format!("[{t},{r}]")).collect();
        format!(
            r#"{{"ev":"envelope","utt":"{utt}","chunk":{chunk},"fps":30,"pts":[{}]}}"#,
            p.join(",")
        )
    }
    fn vis(utt: &str, chunk: u64, items: &[(u32, u32, &str)]) -> String {
        let p: Vec<String> = items
            .iter()
            .map(|(t, d, s)| format!(r#"[{t},{d},"{s}"]"#))
            .collect();
        format!(
            r#"{{"ev":"visemes","utt":"{utt}","chunk":{chunk},"items":[{}]}}"#,
            p.join(",")
        )
    }
    fn start(utt: &str, chunk: u64) -> String {
        format!(r#"{{"ev":"chunk_start","utt":"{utt}","chunk":{chunk},"mono_ms":987654}}"#)
    }
    fn end(utt: &str, chunk: u64) -> String {
        format!(r#"{{"ev":"chunk_end","utt":"{utt}","chunk":{chunk}}}"#)
    }
    fn utt_end(utt: &str) -> String {
        format!(r#"{{"ev":"utt_end","utt":"{utt}"}}"#)
    }
    fn stop(utt: &str) -> String {
        format!(r#"{{"ev":"stop","utt":"{utt}"}}"#)
    }

    /// Flat envelope: `rms` from 0 to `len_ms`, every 33 ms.
    fn flat(len_ms: u32, rms: f32) -> Vec<(u32, f32)> {
        (0..=len_ms / 33).map(|i| (i * 33, rms)).collect()
    }

    fn feed(a: &mut AudioLipSync, line: &str, now: u64) {
        a.feed_line(line, now).unwrap();
    }

    #[test]
    fn subscribe_line_is_valid_json() {
        let v: Value = serde_json::from_str(SUBSCRIBE_LINE).unwrap();
        assert_eq!(v["op"], "subscribe");
        assert_eq!(v["topics"][0], "lipsync");
        assert_eq!(hold_ms(), 85);
    }

    #[test]
    fn thresholds_and_hysteresis() {
        use Mouth::*;
        // Plain thresholds from rest.
        assert_eq!(envelope_shape(Closed, 0.0), Closed);
        assert_eq!(envelope_shape(Closed, 0.2), Mid);
        assert_eq!(envelope_shape(Closed, 0.6), Open);
        // Bands: closed needs 0.10 to open, mid holds down to 0.06.
        assert_eq!(envelope_shape(Closed, 0.09), Closed);
        assert_eq!(envelope_shape(Closed, 0.10), Mid);
        assert_eq!(envelope_shape(Mid, 0.07), Mid);
        assert_eq!(envelope_shape(Mid, 0.05), Closed);
        // Open needs > 0.37, holds down to 0.33.
        assert_eq!(envelope_shape(Mid, 0.36), Mid);
        assert_eq!(envelope_shape(Mid, 0.38), Open);
        assert_eq!(envelope_shape(Open, 0.34), Open);
        assert_eq!(envelope_shape(Open, 0.30), Mid);
        assert_eq!(envelope_shape(Open, 0.02), Closed);

        // Through the consumer: samples 200 ms apart (well past the hold).
        let pts = [
            (0, 0.0),
            (200, 0.09),
            (400, 0.12),
            (600, 0.07),
            (800, 0.05),
            (1000, 0.36),
            (1200, 0.40),
            (1400, 0.34),
            (1600, 0.30),
            (1800, 0.30),
        ];
        let mut a = AudioLipSync::new();
        feed(&mut a, &env("u1", 0, &pts), 0);
        feed(&mut a, &start("u1", 0), 1000);
        assert!(a.is_active());
        let got: Vec<Mouth> = pts[..9]
            .iter()
            .map(|p| a.tick(1000 + p.0 as u64 + 100).unwrap())
            .collect();
        assert_eq!(
            got,
            [Closed, Closed, Mid, Mid, Closed, Mid, Open, Open, Mid]
        );
        assert_eq!(a.last_sender_mono_ms(), Some(987654));
    }

    #[test]
    fn minimum_hold_prevents_chatter() {
        // Envelope flips every 33 ms between silent and loud.
        let pts: Vec<(u32, f32)> = (0..60)
            .map(|i| (i * 33, if i % 2 == 0 { 0.0 } else { 0.6 }))
            .collect();
        let mut a = AudioLipSync::new();
        feed(&mut a, &env("u1", 0, &pts), 0);
        feed(&mut a, &start("u1", 0), 0);
        let mut last = None;
        let mut changed_at = 0u64;
        let mut changes = 0;
        for now in (0..1900).step_by(5) {
            let m = a.tick(now).unwrap();
            if last.is_some() && Some(m) != last {
                if changes > 0 {
                    assert!(
                        now - changed_at >= 85,
                        "changed after {} ms",
                        now - changed_at
                    );
                }
                changed_at = now;
                changes += 1;
            }
            last = Some(m);
        }
        assert!(changes > 5, "still moves: {changes}");
    }

    #[test]
    fn visemes_win_and_envelope_gates_them() {
        let mut a = AudioLipSync::new();
        // Loud throughout except a quiet gap 400..600.
        let mut pts = flat(1000, 0.2);
        for p in &mut pts {
            if (400..600).contains(&p.0) {
                p.1 = 0.03;
            }
        }
        feed(&mut a, &env("u1", 0, &pts), 0);
        feed(
            &mut a,
            &vis(
                "u1",
                0,
                &[(0, 200, "open"), (200, 200, "smile"), (400, 300, "open")],
            ),
            0,
        );
        feed(&mut a, &start("u1", 0), 100);
        // Envelope alone says mid; the viseme says open and wins.
        assert_eq!(a.tick(100 + 100), Some(Mouth::Open));
        assert_eq!(a.tick(100 + 300), Some(Mouth::Smile));
        // Viseme says open but the envelope is near-silent: closed.
        assert_eq!(a.tick(100 + 500), Some(Mouth::Closed));
        assert_eq!(a.tick(100 + 650), Some(Mouth::Open));
        // Gap after the last viseme: closed.
        assert_eq!(a.tick(100 + 800), Some(Mouth::Closed));
    }

    #[test]
    fn missing_visemes_and_missing_envelope() {
        // Envelope only.
        let mut a = AudioLipSync::new();
        feed(&mut a, &env("u1", 0, &flat(600, 0.5)), 0);
        feed(&mut a, &start("u1", 0), 0);
        assert_eq!(a.tick(100), Some(Mouth::Open));
        // Visemes only: no gate.
        let mut b = AudioLipSync::new();
        feed(&mut b, &vis("u2", 0, &[(0, 300, "mid")]), 0);
        feed(&mut b, &start("u2", 0), 0);
        assert_eq!(b.tick(100), Some(Mouth::Mid));
        // Started but no data yet: closed (speaking), not None.
        let mut c = AudioLipSync::new();
        feed(&mut c, &start("u3", 0), 0);
        assert_eq!(c.tick(50), Some(Mouth::Closed));
        // ...and data arriving late still applies (t relative to start).
        feed(&mut c, &env("u3", 0, &flat(800, 0.5)), 100);
        assert_eq!(c.tick(300), Some(Mouth::Open));
    }

    #[test]
    fn out_of_order_events() {
        let mut a = AudioLipSync::new();
        // Chunk 1's data before chunk 0's; chunk 0 starts before its data.
        feed(&mut a, &env("u1", 1, &flat(500, 0.5)), 0);
        feed(&mut a, &start("u1", 0), 10);
        feed(&mut a, &env("u1", 0, &flat(500, 0.2)), 20);
        assert_eq!(a.tick(200), Some(Mouth::Mid));
        // chunk_end for 0 arrives, then 1 starts.
        feed(&mut a, &end("u1", 0), 520);
        feed(&mut a, &start("u1", 1), 530);
        assert_eq!(a.tick(700), Some(Mouth::Open));
        // A duplicate / stale chunk_start for 0 changes nothing.
        feed(&mut a, &start("u1", 0), 710);
        assert_eq!(a.tick(800), Some(Mouth::Open));
        // chunk_end before chunk_start: falls back to data length.
        let mut b = AudioLipSync::new();
        feed(&mut b, &env("u2", 0, &flat(300, 0.5)), 0);
        feed(&mut b, &end("u2", 0), 1);
        feed(&mut b, &start("u2", 0), 50);
        assert_eq!(b.tick(150), Some(Mouth::Open));
    }

    #[test]
    fn late_chunk_start_is_dropped_after_2s() {
        let mut a = AudioLipSync::new();
        feed(&mut a, &env("u1", 0, &flat(500, 0.5)), 0);
        feed(&mut a, &env("u1", 1, &flat(500, 0.5)), 0);
        assert_eq!(a.tick(1999), None, "not speaking yet");
        feed(&mut a, &start("u1", 0), 2000);
        assert_eq!(a.tick(2000), Some(Mouth::Open));
        assert_eq!(a.tick(2100), Some(Mouth::Open));
        // Chunk 1 never started within 2 s of its data: dropped.
        assert_eq!(a.dropped_late(), 1);
        // Its chunk_start arriving now is ignored.
        feed(&mut a, &start("u1", 1), 2800);
        assert_eq!(a.tick(2900), Some(Mouth::Closed));
        assert_eq!(a.dropped_late(), 1);
    }

    #[test]
    fn stop_closes_immediately_without_smile() {
        let mut a = AudioLipSync::new();
        feed(&mut a, &env("u1", 0, &flat(2000, 0.6)), 0);
        feed(&mut a, &env("u1", 1, &flat(2000, 0.6)), 0);
        feed(&mut a, &start("u1", 0), 0);
        assert_eq!(a.tick(100), Some(Mouth::Open));
        feed(&mut a, &stop("u1"), 120);
        assert!(!a.is_active());
        // Closed right away (no 85 ms hold), then hand back.
        assert_eq!(a.tick(121), Some(Mouth::Closed));
        assert_eq!(a.tick(150), None);
        // The stopped utterance's pending data and late events are ignored.
        feed(&mut a, &start("u1", 1), 200);
        feed(&mut a, &utt_end("u1"), 210);
        assert_eq!(a.tick(300), None);
        assert!(a.utt().is_none());
        // Stop for an unknown utt is harmless.
        feed(&mut a, &stop("zzz"), 400);
        assert_eq!(a.tick(400), None);
    }

    #[test]
    fn utt_end_holds_smile_1_5s() {
        let mut a = AudioLipSync::new();
        feed(&mut a, &env("u1", 0, &flat(500, 0.5)), 0);
        feed(&mut a, &start("u1", 0), 0);
        feed(&mut a, &end("u1", 0), 500);
        feed(&mut a, &utt_end("u1"), 500);
        assert_eq!(a.tick(400), Some(Mouth::Open));
        // Audio over: smile for 1.5 s.
        assert_eq!(a.tick(510), Some(Mouth::Smile));
        assert_eq!(a.tick(1500), Some(Mouth::Smile));
        assert_eq!(a.tick(2009), Some(Mouth::Smile));
        assert_eq!(a.tick(2010), None);
        assert!(!a.is_active());
        // utt_end before the last chunk finished: smile waits for it.
        let mut b = AudioLipSync::new();
        feed(&mut b, &env("u2", 0, &flat(1000, 0.5)), 0);
        feed(&mut b, &start("u2", 0), 0);
        feed(&mut b, &utt_end("u2"), 100);
        assert_eq!(b.tick(500), Some(Mouth::Open));
        // Data ends at ~1023 ms + grace.
        assert_eq!(b.tick(1300), Some(Mouth::Smile));
        // utt_end with nothing played: just idle.
        let mut c = AudioLipSync::new();
        feed(&mut c, &env("u3", 0, &flat(100, 0.5)), 0);
        feed(&mut c, &utt_end("u3"), 10);
        assert_eq!(c.tick(20), None);
        assert!(c.utt().is_none());
    }

    #[test]
    fn idle_without_utt_end_ends_with_smile() {
        let mut a = AudioLipSync::new();
        feed(&mut a, &env("u1", 0, &flat(300, 0.5)), 0);
        feed(&mut a, &start("u1", 0), 0);
        feed(&mut a, &end("u1", 0), 300);
        assert_eq!(a.tick(1000), Some(Mouth::Closed), "between chunks");
        assert_eq!(a.tick(3400), Some(Mouth::Smile));
        assert_eq!(a.tick(5000), None);
    }

    #[test]
    fn malformed_lines_are_errors_not_panics() {
        let mut a = AudioLipSync::new();
        assert!(matches!(
            a.feed_line("{nope", 0),
            Err(LipSyncError::Json(_))
        ));
        assert!(matches!(
            a.feed_line("[1,2]", 0),
            Err(LipSyncError::Malformed { .. })
        ));
        for bad in [
            r#"{"ev":"envelope","utt":"u1","chunk":0}"#,
            r#"{"ev":"envelope","utt":"u1","chunk":0,"pts":[[1]]}"#,
            r#"{"ev":"envelope","utt":"u1","chunk":-1,"pts":[]}"#,
            r#"{"ev":"envelope","chunk":0,"pts":[]}"#,
            r#"{"ev":"visemes","utt":"u1","chunk":0,"items":[[0,"x","open"]]}"#,
            r#"{"ev":"chunk_start","utt":"u1"}"#,
            r#"{"ev":"stop"}"#,
        ] {
            assert!(
                matches!(a.feed_line(bad, 0), Err(LipSyncError::Malformed { .. })),
                "{bad}"
            );
        }
        assert!(a.utt().is_none(), "errors leave state untouched");
        assert_eq!(a.tick(10), None);
    }

    #[test]
    fn unknown_events_and_fields_are_ignored() {
        let mut a = AudioLipSync::new();
        feed(&mut a, "", 0);
        feed(&mut a, r#"{"ok":true,"op":"subscribe"}"#, 0);
        feed(
            &mut a,
            r#"{"ev":"phonemes","utt":"u1","chunk":0,"x":[1]}"#,
            0,
        );
        assert!(a.utt().is_none());
        // Extra fields and unknown viseme names are tolerated.
        feed(
            &mut a,
            r#"{"ev":"visemes","utt":"u1","chunk":0,"v":2,"items":[[0,100,"fv"],[0,300,"open"]]}"#,
            0,
        );
        feed(
            &mut a,
            r#"{"ev":"chunk_start","utt":"u1","chunk":0,"mono_ms":5,"device":"x"}"#,
            0,
        );
        assert_eq!(a.tick(50), Some(Mouth::Open));
    }

    #[test]
    fn new_utt_supersedes_old() {
        let mut a = AudioLipSync::new();
        feed(&mut a, &env("u1", 0, &flat(3000, 0.6)), 0);
        feed(&mut a, &start("u1", 0), 0);
        assert_eq!(a.tick(100), Some(Mouth::Open));
        feed(&mut a, &env("u2", 0, &flat(3000, 0.2)), 200);
        assert_eq!(a.utt(), Some("u2"));
        assert_eq!(a.tick(250), None, "u2 hasn't started");
        feed(&mut a, &start("u2", 0), 300);
        assert_eq!(a.tick(400), Some(Mouth::Mid));
        // Late u1 events are ignored.
        feed(&mut a, &env("u1", 1, &flat(3000, 0.9)), 450);
        feed(&mut a, &start("u1", 1), 460);
        feed(&mut a, &utt_end("u1"), 470);
        assert_eq!(a.utt(), Some("u2"));
        assert_eq!(a.tick(600), Some(Mouth::Mid));
    }

    #[test]
    fn talk_mouth_prefers_audio_then_falls_back_to_flap() {
        let mut t = TalkMouth::new(LipFlap::default());
        assert_eq!(t.tick(0.03, 0), None);
        t.flap.start();
        t.flap.on_token("...");
        assert_eq!(t.tick(0.03, 30), Some(Mouth::Closed));
        assert!(!t.audio_driven());
        t.audio
            .feed_line(&env("u1", 0, &flat(500, 0.6)), 40)
            .unwrap();
        t.audio.feed_line(&start("u1", 0), 40).unwrap();
        assert_eq!(t.tick(0.03, 140), Some(Mouth::Open));
        assert!(t.audio_driven() && t.active());
        t.audio.feed_line(&stop("u1"), 150).unwrap();
        assert_eq!(t.tick(0.03, 160), Some(Mouth::Closed));
        // Back to the lip-flap (still active, resting closed).
        assert_eq!(t.tick(0.03, 190), Some(Mouth::Closed));
        assert!(!t.audio_driven());
    }
}
