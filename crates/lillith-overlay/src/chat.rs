//! Click-to-chat glue on the overlay side (windowing-system independent).
//!
//! - [`ClickGate`]: a plain left click on her opens the chat bubble, but only
//!   once the double-click window has passed without a second click, so
//!   double clicks keep their own reaction and a drag never opens it.
//! - [`ChatHook`]: how the overlay asks the app to open the bubble; it gets a
//!   [`ChatAnchor`] (her window on screen) to place the bubble next to her.
//! - [`Talk`]: the speaking state fed by `chat.*` events (reply streaming):
//!   switches the base to `talk_loop` (alternating with `talk_loop_flirty`;
//!   drawn as their `_jiggle` siblings in baked-jiggle mode), reveals the
//!   reply at a speaking pace for the overlay speech bubble, drives the
//!   mouth from the revealed text (mouth patches pasted at the per-frame
//!   mouth point, or frames whose baked mouth matches when the clip has no
//!   patches), and returns her to idle afterwards.
//!
//! Chat events arrive on the same [`crate::EventFeed`] as the system monitor:
//!
//! | event        | detail         | meaning                          |
//! |--------------|----------------|----------------------------------|
//! | `chat.opened`| –              | bubble window opened             |
//! | `chat.start` | –              | a reply started streaming        |
//! | `chat.token` | text delta     | next piece of the reply          |
//! | `chat.end`   | full reply     | reply complete                   |
//! | `chat.error` | technical text | reply failed (fallback line shown) |
//! | `chat.closed`| –              | bubble window closed             |

use crate::speech::SpeechView;
use lillith_anim::animator::Animator;
use lillith_anim::lipsync::TalkMouth;
use lillith_anim::talk::{
    LipFlap, Mouth, MouthMap, DEFAULT_TALK_SPEED, TALK_CLIP, TALK_CLIP_FLIRTY,
};

/// Mouth-shape rate (ms per shape) when a clip has patches but no
/// recommendation (the art's `slow4` value).
pub const DEFAULT_MOUTH_SHAPE_MS: (u32, u32) = (200, 260);
use std::sync::Arc;

pub const CHAT_OPENED: &str = "chat.opened";
pub const CHAT_START: &str = "chat.start";
pub const CHAT_TOKEN: &str = "chat.token";
pub const CHAT_END: &str = "chat.end";
pub const CHAT_ERROR: &str = "chat.error";
pub const CHAT_CLOSED: &str = "chat.closed";

/// A stream with no chat event for this long is treated as finished.
pub const TALK_WATCHDOG_S: f32 = 30.0;

/// Where she is on screen when the bubble is requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatAnchor {
    /// Her window: top-left x/y on the output, width, height (px). `x`/`y`
    /// are only meaningful when `screen` is known.
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    /// Output size, if the backend reported it.
    pub screen: Option<(u32, u32)>,
}

impl ChatAnchor {
    /// Top-left for a `bw`×`bh` bubble: beside her head on the side with
    /// more room, clamped to the screen.
    pub fn bubble_pos(&self, bw: u32, bh: u32) -> Option<(i32, i32)> {
        let (sw, sh) = self.screen?;
        let (bw, bh) = (bw as i32, bh as i32);
        let (sw, sh) = (sw as i32, sh as i32);
        // Her head is roughly the upper third of the cell, centred.
        let head_y = self.y + self.h as i32 / 6;
        let right = self.x + (self.w as i32 * 5) / 8;
        let left = self.x + (self.w as i32 * 3) / 8 - bw;
        let x = if right + bw <= sw || right + bw - sw < -left {
            right
        } else {
            left
        };
        let y = head_y - bh / 2;
        Some((x.clamp(0, (sw - bw).max(0)), y.clamp(0, (sh - bh).max(0))))
    }
}

/// Callback that opens the chat bubble (implemented by the app).
#[derive(Clone)]
pub struct ChatHook(pub Arc<dyn Fn(ChatAnchor) + Send + Sync>);

impl ChatHook {
    pub fn new(f: impl Fn(ChatAnchor) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }
    pub fn open(&self, a: ChatAnchor) {
        (self.0)(a)
    }
}

impl std::fmt::Debug for ChatHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ChatHook")
    }
}

/// Single-click detector that waits out the double-click window.
#[derive(Debug, Clone, Default)]
pub struct ClickGate {
    pending: Option<f32>,
}

impl ClickGate {
    /// A left click (press + release without drag) at time `t`.
    pub fn click(&mut self, t: f32, double_s: f32) {
        match self.pending {
            Some(t0) if t - t0 <= double_s => self.pending = None, // a double click
            _ => self.pending = Some(t),
        }
    }

    /// Forget a pending click (drag started, she fell asleep…).
    pub fn cancel(&mut self) {
        self.pending = None;
    }

    /// `true` once, when a single click has survived the double-click window.
    pub fn poll(&mut self, t: f32, double_s: f32) -> bool {
        match self.pending {
            Some(t0) if t - t0 > double_s => {
                self.pending = None;
                true
            }
            _ => false,
        }
    }
}

/// Characters per second her speech is revealed at [`DEFAULT_TALK_SPEED`]
/// (scales with `talk_speed`).
pub const REVEAL_CPS: f32 = 12.0;
/// Most unrevealed text (s): a faster stream reveals faster so the bubble
/// never trails the reply by more than this.
pub const REVEAL_MAX_BACKLOG_S: f32 = 4.0;
/// Speech stays fully visible this long after she finished (s)…
pub const SPEECH_HOLD_S: f32 = 3.5;
/// …then fades out over this long (s).
pub const SPEECH_FADE_S: f32 = 0.6;

/// Her reply as the speech bubble shows it: the text so far, revealed at
/// a speaking pace, held, then faded.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Speech {
    /// Everything received so far.
    pub text: String,
    /// Characters of `text` revealed.
    revealed: usize,
    reveal_acc: f32,
    /// The stream ended.
    done: bool,
    /// Seconds since done and fully revealed.
    hold: f32,
}

impl Speech {
    /// The revealed part of the text.
    pub fn revealed_text(&self) -> &str {
        match self.text.char_indices().nth(self.revealed) {
            Some((i, _)) => &self.text[..i],
            None => &self.text,
        }
    }

    pub fn fully_revealed(&self) -> bool {
        self.revealed >= self.text.chars().count()
    }

    /// Opacity: 1 until [`SPEECH_HOLD_S`] after the end, then fading.
    pub fn alpha(&self) -> f32 {
        if !(self.done && self.fully_revealed()) {
            return 1.0;
        }
        (1.0 - (self.hold - SPEECH_HOLD_S).max(0.0) / SPEECH_FADE_S).clamp(0.0, 1.0)
    }

    /// Gone (faded out).
    pub fn finished(&self) -> bool {
        self.done && self.fully_revealed() && self.hold >= SPEECH_HOLD_S + SPEECH_FADE_S
    }

    pub fn view(&self) -> SpeechView {
        SpeechView {
            text: self.revealed_text().to_string(),
            alpha: self.alpha(),
        }
    }

    /// Reveals more text at `cps` (faster when far behind); returns the
    /// newly revealed piece.
    fn advance(&mut self, dt: f32, cps: f32) -> String {
        let total = self.text.chars().count();
        let backlog = total.saturating_sub(self.revealed);
        if backlog == 0 {
            self.reveal_acc = 0.0;
            if self.done {
                self.hold += dt;
            }
            return String::new();
        }
        let rate = cps.max(backlog as f32 / REVEAL_MAX_BACKLOG_S);
        self.reveal_acc += rate * dt;
        let n = (self.reveal_acc.floor() as usize).min(backlog);
        self.reveal_acc -= n as f32;
        let piece: String = self.text.chars().skip(self.revealed).take(n).collect();
        self.revealed += n;
        piece
    }
}

/// Speaking state.
#[derive(Debug, Clone, Default)]
pub struct Talk {
    /// Lip-flap from the text, or audio lip-sync from the voice daemon.
    mouth: TalkMouth,
    /// Local monotonic clock (ms, summed `dt`) for audio lip-sync.
    clock_ms: u64,
    /// Fallback lip-flap (no mouth patches): frames by baked mouth.
    frames: Option<MouthMap>,
    /// Mouth patches are pasted over the playing body (true lip-sync).
    patches: bool,
    /// Speaking clip of the current reply (`talk_loop` / `talk_loop_flirty`).
    clip: Option<String>,
    /// Replies so far (alternates plain and flirty talk clips).
    replies: u32,
    /// The reply as it streams (and for a while after): what her overlay
    /// speech bubble shows. Its revealed text drives the lip-flap.
    pub speech: Option<Speech>,
    since_event: f32,
    streaming: bool,
    /// Stream over; the lip-flap finishes once all text is revealed.
    finish_pending: bool,
    /// The talk clip was set as base by us (so we restore idle after).
    took_base: bool,
    /// Mouth shape this frame (drives a custom avatar's talk bob).
    pub mouth_now: Option<Mouth>,
    /// `speech` is a one-off line ([`Talk::show_line`]), not a chat reply.
    one_off: bool,
}

impl Talk {
    /// Talk state with lip-flap at `speed` (`[anim] talk_speed`).
    pub fn with_speed(speed: f32) -> Self {
        Self {
            mouth: TalkMouth::new(LipFlap::with_speed(speed)),
            ..Self::default()
        }
    }

    /// `true` while she is speaking (stream + end smile).
    pub fn active(&self) -> bool {
        self.mouth.active()
    }

    /// Feeds one voice-daemon lip-sync event line (drives her mouth from
    /// the audio while it plays). Malformed lines are ignored.
    pub fn feed_lipsync(&mut self, line: &str) {
        if let Err(e) = self.mouth.audio.feed_line(line, self.clock_ms) {
            tracing::debug!("lipsync line ignored: {e}");
        }
    }

    /// Drops audio lip-sync (daemon gone, barge-in, safeword).
    pub fn reset_lipsync(&mut self) {
        self.mouth.audio.reset();
    }

    pub fn speed(&self) -> f32 {
        self.mouth.flap.speed()
    }

    /// Lip-flap pace (`[anim] talk_speed`, live from Settings).
    pub fn set_speed(&mut self, speed: f32) {
        self.mouth.flap.set_speed(speed);
    }

    /// `true` if mouth patches drive her mouth (else baked-frame picking).
    pub fn uses_patches(&self) -> bool {
        self.patches
    }

    /// Characters per second the speech is revealed at.
    fn reveal_cps(&self) -> f32 {
        REVEAL_CPS * self.mouth.flap.speed() / DEFAULT_TALK_SPEED
    }

    /// Applies a `chat.*` event. Returns `false` for other names.
    pub fn event(&mut self, name: &str, detail: Option<&str>, anim: &mut Animator) -> bool {
        if name != CHAT_TOKEN {
            tracing::debug!(event = name, "chat");
        }
        match name {
            CHAT_START => {
                self.mouth.flap.start();
                self.streaming = true;
                self.finish_pending = false;
                self.since_event = 0.0;
                self.speech = Some(Speech::default());
                self.one_off = false;
                self.replies = self.replies.wrapping_add(1);
                self.take_talk_base(anim);
            }
            CHAT_TOKEN => {
                if !self.streaming {
                    // Token without start (e.g. the bubble restarted): start now.
                    self.event(CHAT_START, None, anim);
                }
                self.since_event = 0.0;
                if let Some(s) = &mut self.speech {
                    s.text.push_str(detail.unwrap_or(""));
                }
            }
            CHAT_END | CHAT_ERROR => self.end(detail.filter(|_| name == CHAT_END)),
            CHAT_CLOSED => {
                // Bubble gone mid-stream: wrap up.
                if self.streaming {
                    self.end(None);
                }
            }
            CHAT_OPENED => {}
            _ => return false,
        }
        true
    }

    /// Shows a one-off line (banter, About) in the speech bubble: revealed
    /// at the speaking pace, held, faded; no lip-flap. Ignored while a chat
    /// reply streams or is still on screen. Returns `true` if shown.
    pub fn show_line(&mut self, line: &str) -> bool {
        let line = line.trim();
        let reply_on_screen = self.speech.is_some() && !self.one_off;
        if line.is_empty() || self.streaming || self.finish_pending || reply_on_screen {
            return false;
        }
        self.one_off = true;
        self.speech = Some(Speech {
            text: line.to_string(),
            done: true,
            ..Speech::default()
        });
        true
    }

    /// Sets the talk clip of this reply as base (plain/flirty alternate)
    /// and sets up its mouth; idle if she has no talk clip.
    fn take_talk_base(&mut self, anim: &mut Animator) {
        let flirty_turn = self.replies % 2 == 0;
        let pick = [
            if flirty_turn {
                TALK_CLIP_FLIRTY
            } else {
                TALK_CLIP
            },
            TALK_CLIP,
            TALK_CLIP_FLIRTY,
        ]
        .into_iter()
        .find(|c| anim.has_clip(c));
        anim.set_mouth(None);
        // Whatever little reaction was playing (hover, idle fidget)
        // stops: she turns to talk.
        if pick.is_some() {
            anim.clear_overlays();
        }
        match pick {
            Some(c) => {
                tracing::debug!(clip = c, "talking");
                let _ = anim.set_base(c, false);
                self.took_base = true;
                self.setup_mouth(c, anim);
                self.clip = Some(c.to_string());
            }
            None => {
                self.took_base = false;
                self.frames = None;
                self.patches = false;
                self.mouth.flap.set_shape_ms(None);
                self.clip = None;
            }
        }
    }

    /// Patch lip-sync if the drawn talk clip (or its clean sibling) has
    /// mouth patches, else frame picking on the drawn clip.
    fn setup_mouth(&mut self, base: &str, anim: &Animator) {
        let clips = &anim.manifest().clips;
        let drawn = anim.drawn_clip(base);
        let (Some(d), clean) = (clips.get(drawn), clips.get(base)) else {
            return;
        };
        self.patches = lillith_anim::animator::mouth_draw(d, clean, 0, Mouth::Open).is_some();
        if self.patches {
            let ms = d
                .mouth_shape_ms
                .or(clean.and_then(|c| c.mouth_shape_ms))
                .unwrap_or(DEFAULT_MOUTH_SHAPE_MS);
            self.mouth.flap.set_shape_ms(Some(ms));
            self.frames = None;
        } else {
            self.mouth.flap.set_shape_ms(None);
            self.frames = MouthMap::for_clip(d).or_else(|| clean.and_then(MouthMap::for_clip));
        }
        tracing::debug!(
            clip = drawn,
            patches = self.patches,
            shape_s = ?self.mouth.flap.shape_s(),
            "lip-sync"
        );
    }

    fn end(&mut self, full: Option<&str>) {
        self.streaming = false;
        self.finish_pending = true;
        if let Some(s) = &mut self.speech {
            if let Some(full) = full.filter(|f| !f.is_empty()) {
                if full.starts_with(s.revealed_text()) {
                    s.text = full.to_string();
                }
            }
            s.done = true;
        } else {
            self.mouth.flap.finish();
            self.finish_pending = false;
        }
    }

    /// Advances speech reveal, lip-flap and the bubble timer; drives the
    /// animator.
    pub fn tick(&mut self, dt: f32, anim: &mut Animator) {
        if self.streaming {
            self.since_event += dt;
            if self.since_event > TALK_WATCHDOG_S {
                tracing::debug!("chat stream went quiet; ending talk");
                self.end(None);
            }
        }
        let cps = self.reveal_cps();
        if let Some(s) = &mut self.speech {
            let piece = s.advance(dt, cps);
            if !piece.is_empty() && self.mouth.flap.active() {
                self.mouth.flap.on_token(&piece);
            }
            if self.finish_pending && s.fully_revealed() {
                self.mouth.flap.finish();
                self.finish_pending = false;
            }
            if s.finished() {
                self.speech = None;
            }
        }
        self.clock_ms = self
            .clock_ms
            .saturating_add((dt.max(0.0) * 1000.0).round() as u64);
        let was = self.mouth.active();
        let mouth = self.mouth.tick(dt, self.clock_ms);
        // The voice usually starts after the text finished: take the talk
        // clip again while the audio drives her mouth.
        if self.mouth.audio_driven() && self.clip.is_none() {
            self.take_talk_base(anim);
        }
        self.mouth_now = mouth;
        let on_talk_base = self.clip.as_deref().is_some_and(|c| anim.base() == c);
        match mouth {
            Some(m) if on_talk_base => {
                if self.patches {
                    anim.set_mouth(Some(m));
                } else {
                    let natural = anim.base_frame();
                    let pin = self.frames.as_ref().and_then(|f| f.frame_for(m, natural));
                    anim.pin_base_frame(pin);
                }
            }
            Some(_) => {}
            None if was && !self.mouth.active() => {
                if on_talk_base {
                    anim.pin_base_frame(None);
                    anim.set_mouth(None);
                    if self.took_base {
                        anim.stand();
                    }
                }
                self.took_base = false;
                self.clip = None;
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_click_waits_out_double_click() {
        let mut g = ClickGate::default();
        g.click(1.0, 0.35);
        assert!(!g.poll(1.2, 0.35));
        assert!(g.poll(1.4, 0.35));
        assert!(!g.poll(1.5, 0.35), "fires once");
        // Double click: no chat.
        g.click(2.0, 0.35);
        g.click(2.2, 0.35);
        assert!(!g.poll(3.0, 0.35));
        // Cancelled (drag).
        g.click(4.0, 0.35);
        g.cancel();
        assert!(!g.poll(5.0, 0.35));
    }

    #[test]
    fn daemon_lipsync_lines_drive_her_mouth_and_talk_clip() {
        use lillith_anim::animator::AnimatorConfig;
        let mut toml = String::new();
        for name in ["idle", TALK_CLIP] {
            toml += &format!(
                "[clips.{name}]\npage = \"{name}.png\"\nfps = 10.0\nmode = \"loop\"\nframes = ["
            );
            for i in 0..4 {
                toml += &format!(
                    "{{ x = {}, y = 0, w = 400, h = 400, anchor = [200, 392] }},",
                    i * 400
                );
            }
            toml += "]\n";
        }
        let manifest = lillith_anim::atlas::Manifest::from_toml_str(&toml).unwrap();
        let cfg = AnimatorConfig {
            idle_enabled: false,
            ..AnimatorConfig::default()
        };
        let mut anim = Animator::new(manifest, cfg).unwrap();
        let mut talk = Talk::with_speed(DEFAULT_TALK_SPEED);
        talk.tick(0.016, &mut anim);
        assert!(!talk.active());
        // No chat stream at all: the voice alone moves her mouth.
        let pts: Vec<String> = (0..=60).map(|i| format!("[{},0.6]", i * 33)).collect();
        talk.feed_lipsync(&format!(
            r#"{{"ev":"envelope","utt":7,"chunk":0,"fps":30,"pts":[{}]}}"#,
            pts.join(",")
        ));
        talk.feed_lipsync(r#"{"ev":"chunk_start","utt":7,"chunk":0,"mono_ms":12345}"#);
        talk.feed_lipsync("not json");
        let mut opened = false;
        for _ in 0..20 {
            talk.tick(0.016, &mut anim);
            opened |= talk.mouth_now == Some(Mouth::Open);
        }
        assert!(talk.active(), "audio drives the mouth");
        assert!(opened, "loud envelope opens the mouth");
        assert_eq!(anim.base(), TALK_CLIP, "talk clip taken for the voice");
        // Barge-in / stop: mouth closes and she goes back to idle.
        talk.feed_lipsync(r#"{"ev":"stop","utt":7}"#);
        for _ in 0..5 {
            talk.tick(0.016, &mut anim);
        }
        assert!(!talk.active());
        assert_eq!(talk.mouth_now, None);
        assert_ne!(anim.base(), TALK_CLIP);
        // reset_lipsync (daemon gone) also ends it.
        talk.feed_lipsync(r#"{"ev":"chunk_start","utt":8,"chunk":0,"mono_ms":1}"#);
        talk.tick(0.016, &mut anim);
        assert!(talk.active());
        talk.reset_lipsync();
        talk.tick(0.016, &mut anim);
        assert!(!talk.active());
    }

    #[test]
    fn bubble_goes_beside_her_and_stays_on_screen() {
        let a = ChatAnchor {
            x: 100,
            y: 500,
            w: 300,
            h: 300,
            screen: Some((1920, 1080)),
        };
        let (x, y) = a.bubble_pos(300, 360).unwrap();
        assert_eq!(x, 287, "right of her head");
        assert!(y >= 0 && y + 360 <= 1080);
        // Near the right edge: flips to her left.
        let r = ChatAnchor { x: 1700, ..a };
        let (x, _) = r.bubble_pos(300, 360).unwrap();
        assert!(x + 300 <= 1700 + 113, "x = {x}");
        assert_eq!(ChatAnchor { screen: None, ..a }.bubble_pos(300, 360), None);
    }
}
