//! Token-driven lip-flap for the speaking clip (`talk_loop`).
//!
//! While a chat reply streams in, every text delta schedules a few mouth
//! shapes (vowels open the mouth, consonants half-open it, punctuation
//! closes it, `~`/emoji/`<3` give a smile). [`LipFlap::tick`] returns the
//! shape to draw; [`MouthFrames`] maps shapes to frames of the clip, and the
//! overlay pins that frame on the base ([`crate::animator::Animator::pin_base_frame`]).
//!
//! The queue holds at most [`MAX_QUEUE_S`] of speech: the mouth follows
//! the stream at a speaking pace and never lags far behind it.

use crate::atlas::Clip;
use std::collections::VecDeque;

/// Name of the speaking clip in the manifest.
pub const TALK_CLIP: &str = "talk_loop";
/// Flirtier speaking double (alternates with [`TALK_CLIP`] when present).
pub const TALK_CLIP_FLIRTY: &str = "talk_loop_flirty";

/// How far (frames) to look from the playing frame for one whose baked
/// mouth matches the wanted shape; small, so the body barely jumps.
const TRACK_SEARCH: usize = 3;

/// How a speaking clip shows a mouth shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MouthMap {
    /// Short clip: one frame per shape, pinned.
    Fixed(MouthFrames),
    /// Full speaking loop with a baked shape per frame
    /// ([`Clip::mouth_sequence`]): the body keeps playing, and when the
    /// playing frame has the wrong mouth the nearest frame (±3) with the
    /// right one is drawn instead.
    Track(Vec<Option<Mouth>>),
}

impl MouthMap {
    pub fn for_clip(clip: &Clip) -> Option<Self> {
        if !clip.mouth_sequence.is_empty() && clip.mouth_sequence.len() == clip.frames.len() {
            let t: Vec<Option<Mouth>> = clip
                .mouth_sequence
                .iter()
                .map(|s| Mouth::parse(s.trim().trim_start_matches("mouth_")))
                .collect();
            if t.iter().any(Option::is_some) {
                return Some(MouthMap::Track(t));
            }
        }
        MouthFrames::for_clip(clip).map(MouthMap::Fixed)
    }

    /// Frame to pin for shape `m` while the player is on `natural`
    /// (`None` = draw `natural` as is).
    pub fn frame_for(&self, m: Mouth, natural: usize) -> Option<usize> {
        match self {
            MouthMap::Fixed(f) => Some(f.frame(m)),
            MouthMap::Track(t) => {
                let n = t.len();
                if n == 0 {
                    return None;
                }
                let natural = natural % n;
                // Acceptable substitutes, best first.
                let wants: &[Mouth] = match m {
                    Mouth::Closed => &[Mouth::Closed, Mouth::Smile],
                    Mouth::Mid => &[Mouth::Mid, Mouth::Open],
                    Mouth::Open => &[Mouth::Open, Mouth::Mid],
                    Mouth::Smile => &[Mouth::Smile, Mouth::Closed],
                };
                for w in wants {
                    if t[natural] == Some(*w) {
                        return None;
                    }
                    for d in 1..=TRACK_SEARCH.min(n / 2) {
                        for g in [(natural + d) % n, (natural + n - d) % n] {
                            if t[g] == Some(*w) {
                                return Some(g);
                            }
                        }
                    }
                }
                None
            }
        }
    }
}

/// A mouth shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mouth {
    Closed,
    Mid,
    Open,
    Smile,
}

impl Mouth {
    /// Manifest name (`closed`, `mid`, `open`, `smile`).
    pub fn name(self) -> &'static str {
        match self {
            Mouth::Closed => "closed",
            Mouth::Mid => "mid",
            Mouth::Open => "open",
            Mouth::Smile => "smile",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "closed" => Some(Mouth::Closed),
            "mid" => Some(Mouth::Mid),
            "open" => Some(Mouth::Open),
            "smile" => Some(Mouth::Smile),
            _ => None,
        }
    }
}

/// Frame index of each mouth shape inside the talk clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouthFrames {
    pub closed: usize,
    pub mid: usize,
    pub open: usize,
    pub smile: usize,
}

impl MouthFrames {
    /// Mapping for `clip`:
    ///
    /// 1. explicit tags `mouth_closed:0`, `mouth_mid:1`, `mouth_open:2`,
    ///    `mouth_smile:3` (also `mouth_open=2`), missing shapes fall back to
    ///    the nearest one;
    /// 2. otherwise, for clips of 1–4 frames, the order
    ///    `[closed, mid, open, smile]`;
    /// 3. otherwise `None`: a longer untagged clip is a full talking loop
    ///    and just plays while she speaks.
    pub fn for_clip(clip: &Clip) -> Option<Self> {
        let n = clip.frames.len();
        if n == 0 {
            return None;
        }
        let mut tagged: [Option<usize>; 4] = [None; 4];
        for t in &clip.tags {
            let Some(rest) = t.strip_prefix("mouth_") else {
                continue;
            };
            let Some((shape, idx)) = rest.split_once([':', '=']) else {
                continue;
            };
            let (Some(m), Ok(i)) = (Mouth::parse(shape), idx.trim().parse::<usize>()) else {
                continue;
            };
            if i < n {
                tagged[m as usize] = Some(i);
            }
        }
        if tagged.iter().any(Option::is_some) {
            let [c, m, o, s] = tagged;
            let closed = c.or(s).or(m).or(o).unwrap_or(0);
            let open = o.or(m).unwrap_or(closed);
            let mid = m.or(o).unwrap_or(closed);
            let smile = s.unwrap_or(closed);
            return Some(Self {
                closed,
                mid,
                open,
                smile,
            });
        }
        let at = |i: usize| i.min(n - 1);
        match n {
            1 => Some(Self {
                closed: 0,
                mid: 0,
                open: 0,
                smile: 0,
            }),
            2 => Some(Self {
                closed: 0,
                mid: 1,
                open: 1,
                smile: 0,
            }),
            3 => Some(Self {
                closed: 0,
                mid: 1,
                open: 2,
                smile: 0,
            }),
            4 => Some(Self {
                closed: 0,
                mid: 1,
                open: 2,
                smile: at(3),
            }),
            _ => None,
        }
    }

    pub fn frame(&self, m: Mouth) -> usize {
        match m {
            Mouth::Closed => self.closed,
            Mouth::Mid => self.mid,
            Mouth::Open => self.open,
            Mouth::Smile => self.smile,
        }
    }
}

/// Seconds per scheduled shape at `talk_speed = 1.0` (the original, fast
/// pace). Actual durations are these divided by the speed.
pub(crate) const FLAP_S: f32 = 0.085;
const PAUSE_S: f32 = 0.16;
const SMILE_S: f32 = 0.30;
/// Default `talk_speed`: mouth shapes change ~38% less often than at 1.0
/// (she isn't in a rush).
pub const DEFAULT_TALK_SPEED: f32 = 0.62;
/// Smile held after the reply ends at [`DEFAULT_TALK_SPEED`] (s); scales
/// with the speed like the other shapes.
pub const END_SMILE_S: f32 = 1.5;
/// Most speech (s) waiting in the queue. Local models stream a sentence in
/// well under a second, so she keeps "saying" it at speaking pace for a
/// moment after the text is in, but never more than this behind the stream
/// (older shapes are dropped).
pub const MAX_QUEUE_S: f32 = 4.0;

/// Lip-flap state machine.
#[derive(Debug, Clone)]
pub struct LipFlap {
    queue: VecDeque<(Mouth, f32)>,
    current: Option<(Mouth, f32)>,
    finishing: bool,
    end_left: f32,
    active: bool,
    speed: f32,
    /// Patch mode ([`LipFlap::set_shape_ms`]): seconds per mouth shape.
    shape_s: Option<f32>,
    /// Patch mode: previous char was a vowel / next syllable is `Mid`.
    in_vowel: bool,
    alt: bool,
}

impl Default for LipFlap {
    fn default() -> Self {
        Self::with_speed(DEFAULT_TALK_SPEED)
    }
}

impl LipFlap {
    /// Lip-flap at `speed` (1.0 = original pace, [`DEFAULT_TALK_SPEED`] =
    /// default; clamped to 0.1..=4).
    pub fn with_speed(speed: f32) -> Self {
        Self {
            queue: VecDeque::new(),
            current: None,
            finishing: false,
            end_left: 0.0,
            active: false,
            speed: if speed.is_finite() {
                speed.clamp(0.1, 4.0)
            } else {
                DEFAULT_TALK_SPEED
            },
            shape_s: None,
            in_vowel: false,
            alt: false,
        }
    }

    /// Patch mode (mouth patches pasted over a playing body): one shape
    /// per syllable, each lasting the art's recommended
    /// `mouth_shape_change_ms` (mean of the range, tuned for
    /// [`DEFAULT_TALK_SPEED`]) scaled by `talk_speed`. `None` = the
    /// per-letter frame-pinning pace.
    pub fn set_shape_ms(&mut self, ms: Option<(u32, u32)>) {
        self.shape_s = ms.map(|(a, b)| {
            let mean = (a.min(b) + a.max(b)) as f32 / 2000.0;
            (mean.max(0.02) * DEFAULT_TALK_SPEED / self.speed).clamp(0.05, 1.5)
        });
    }

    /// Seconds per shape in patch mode.
    pub fn shape_s(&self) -> Option<f32> {
        self.shape_s
    }

    pub fn speed(&self) -> f32 {
        self.speed
    }

    /// Changes the pace (Settings, live); takes effect from the next reply.
    pub fn set_speed(&mut self, speed: f32) {
        if speed.is_finite() {
            self.speed = speed.clamp(0.1, 4.0);
        }
    }

    /// Seconds a shape of base length `base_s` lasts at this speed.
    fn dur(&self, base_s: f32) -> f32 {
        base_s / self.speed
    }

    /// Seconds the end smile lasts at this speed.
    pub fn end_smile_s(&self) -> f32 {
        END_SMILE_S * DEFAULT_TALK_SPEED / self.speed
    }

    /// Starts a reply.
    pub fn start(&mut self) {
        *self = Self {
            active: true,
            shape_s: self.shape_s,
            ..Self::with_speed(self.speed)
        };
    }

    /// `true` from [`Self::start`] until the end smile is over.
    pub fn active(&self) -> bool {
        self.active
    }

    /// Schedules shapes for a text delta.
    pub fn on_token(&mut self, text: &str) {
        if !self.active {
            self.start();
        }
        if let Some(s) = self.shape_s {
            self.on_token_syllables(text, s);
            return;
        }
        let mut last: Option<Mouth> = None;
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            let shape = if c == '~' || (c == '<' && chars.peek() == Some(&'3')) || is_emoji(c) {
                Some((Mouth::Smile, self.dur(SMILE_S)))
            } else if matches!(c, '.' | '!' | '?' | ',' | ';' | ':' | '…' | '\n') {
                Some((Mouth::Closed, self.dur(PAUSE_S)))
            } else if "aeiouyAEIOUYäöüáéíóú".contains(c) {
                Some((Mouth::Open, self.dur(FLAP_S)))
            } else if c.is_alphanumeric() {
                Some((Mouth::Mid, self.dur(FLAP_S)))
            } else {
                None
            };
            let Some((m, d)) = shape else { continue };
            // Collapse runs (a word's consonants are one half-open beat).
            if last == Some(m) && m != Mouth::Closed {
                continue;
            }
            last = Some(m);
            self.queue.push_back((m, d));
            if m == Mouth::Open || m == Mouth::Mid {
                // A short close between beats makes it read as talking.
                self.queue
                    .push_back((Mouth::Closed, self.dur(FLAP_S * 0.5)));
                last = Some(Mouth::Closed);
            }
        }
        while self.queue.iter().map(|q| q.1).sum::<f32>() > MAX_QUEUE_S {
            self.queue.pop_front();
        }
    }

    /// Patch mode: open on each vowel group (alternating open / mid), close
    /// on punctuation, smile on `~`/`<3`/emoji; consonants and spaces add
    /// nothing (the mouth rests closed between queued shapes). Text split
    /// across calls schedules the same shapes.
    fn on_token_syllables(&mut self, text: &str, s: f32) {
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            let vowel = "aeiouyAEIOUYäöüáéíóú".contains(c);
            let shape = if c == '~' || (c == '<' && chars.peek() == Some(&'3')) || is_emoji(c) {
                Some((Mouth::Smile, s * 1.6))
            } else if matches!(c, '.' | '!' | '?' | ',' | ';' | ':' | '…' | '\n') {
                Some((Mouth::Closed, s * 1.4))
            } else if vowel && !self.in_vowel {
                self.alt = !self.alt;
                Some((if self.alt { Mouth::Open } else { Mouth::Mid }, s))
            } else {
                None
            };
            self.in_vowel = vowel;
            let Some((m, d)) = shape else { continue };
            if m != Mouth::Open && m != Mouth::Mid && self.queue.back().map(|q| q.0) == Some(m) {
                continue;
            }
            self.queue.push_back((m, d));
        }
        while self.queue.iter().map(|q| q.1).sum::<f32>() > MAX_QUEUE_S {
            self.queue.pop_front();
        }
    }

    /// Shapes waiting (s), excluding the one showing.
    pub fn queued_s(&self) -> f32 {
        self.queue.iter().map(|q| q.1).sum()
    }

    /// The reply is complete: drain, smile, then stop.
    pub fn finish(&mut self) {
        if self.active {
            self.finishing = true;
            self.end_left = self.end_smile_s();
        }
    }

    /// Stops at once (bubble closed, error).
    pub fn stop(&mut self) {
        *self = Self::with_speed(self.speed);
    }

    /// Advances by `dt` and returns the shape to draw (`None` when inactive).
    pub fn tick(&mut self, dt: f32) -> Option<Mouth> {
        if !self.active {
            return None;
        }
        let mut dt = dt;
        loop {
            match &mut self.current {
                Some((_, left)) if *left > dt => {
                    *left -= dt;
                    break;
                }
                Some((_, left)) => {
                    dt -= *left;
                    self.current = self.queue.pop_front();
                }
                None => {
                    self.current = self.queue.pop_front();
                    if self.current.is_none() {
                        break;
                    }
                }
            }
        }
        if let Some((m, _)) = self.current {
            return Some(m);
        }
        if self.finishing {
            self.end_left -= dt;
            if self.end_left <= 0.0 {
                self.stop();
                return None;
            }
            return Some(Mouth::Smile);
        }
        // Waiting for the next token: mouth at rest.
        Some(Mouth::Closed)
    }
}

fn is_emoji(c: char) -> bool {
    matches!(c as u32, 0x1F300..=0x1FAFF | 0x2600..=0x27BF)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atlas::FrameRect;

    fn clip(n: usize, tags: &[&str]) -> Clip {
        Clip {
            page: "t.png".into(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            severity: 0,
            system: false,
            frames: vec![
                FrameRect {
                    x: 0,
                    y: 0,
                    w: 4,
                    h: 4,
                    anchor: (2, 4)
                };
                n
            ],
            ..Clip::default()
        }
    }

    #[test]
    fn baked_track_picks_nearby_matching_frames() {
        let mut c = clip(8, &[]);
        c.mouth_sequence = [
            "closed", "mid", "open", "mid", "smile", "closed", "open", "mid",
        ]
        .map(String::from)
        .to_vec();
        let m = MouthMap::for_clip(&c).unwrap();
        assert!(matches!(m, MouthMap::Track(_)));
        assert_eq!(m.frame_for(Mouth::Open, 2), None, "already open: play on");
        assert_eq!(m.frame_for(Mouth::Open, 1), Some(2));
        let g = m.frame_for(Mouth::Closed, 3).unwrap();
        assert!(c.mouth_sequence[g] == "closed" || c.mouth_sequence[g] == "smile");
        assert_eq!(m.frame_for(Mouth::Smile, 4), None);
        // Untagged short clips keep the fixed mapping.
        assert!(matches!(
            MouthMap::for_clip(&clip(4, &[])),
            Some(MouthMap::Fixed(_))
        ));
    }

    #[test]
    fn mouth_frame_mapping() {
        let m = MouthFrames::for_clip(&clip(4, &[])).unwrap();
        assert_eq!((m.closed, m.mid, m.open, m.smile), (0, 1, 2, 3));
        let t = MouthFrames::for_clip(&clip(
            6,
            &["mouth_open:5", "mouth_closed=1", "mouth_smile:0", "loop"],
        ))
        .unwrap();
        assert_eq!((t.closed, t.mid, t.open, t.smile), (1, 5, 5, 0));
        // Long untagged loops just play.
        assert_eq!(MouthFrames::for_clip(&clip(8, &[])), None);
        // Out-of-range tags are ignored.
        assert_eq!(
            MouthFrames::for_clip(&clip(2, &["mouth_open:9"]))
                .unwrap()
                .open,
            1
        );
    }

    fn run(f: &mut LipFlap, secs: f32) -> Vec<Mouth> {
        let mut v = Vec::new();
        let mut t = 0.0;
        while t < secs {
            if let Some(m) = f.tick(1.0 / 30.0) {
                if v.last() != Some(&m) {
                    v.push(m);
                }
            }
            t += 1.0 / 30.0;
        }
        v
    }

    #[test]
    fn flaps_while_streaming_then_smiles_and_stops() {
        let mut f = LipFlap::default();
        assert_eq!(f.tick(0.1), None);
        f.start();
        f.on_token("Hey cutie");
        let shapes = run(&mut f, 1.5);
        assert!(
            shapes.contains(&Mouth::Open) && shapes.contains(&Mouth::Closed),
            "{shapes:?}"
        );
        // No tokens: rests closed but stays active.
        assert_eq!(f.tick(0.5), Some(Mouth::Closed));
        assert!(f.active());
        f.on_token("~");
        assert_eq!(f.tick(0.01), Some(Mouth::Smile));
        f.finish();
        let tail = run(&mut f, 3.0);
        assert_eq!(tail.last(), Some(&Mouth::Smile));
        assert!(!f.active());
        assert_eq!(f.tick(0.1), None);
    }

    #[test]
    fn default_pace_is_slower_and_smile_holds_1_5s() {
        fn shapes_in(f: &mut LipFlap, secs: f32) -> usize {
            f.start();
            f.on_token("hello there");
            let (mut n, mut last, mut t) = (0, None, 0.0);
            while t < secs {
                let m = f.tick(0.01);
                if m != last {
                    n += 1;
                    last = m;
                }
                t += 0.01;
            }
            n
        }
        let slow = shapes_in(&mut LipFlap::default(), 0.8);
        let fast = shapes_in(&mut LipFlap::with_speed(1.0), 0.8);
        assert!(
            (slow as f32) <= fast as f32 * 0.7,
            "default changes shape less often: {slow} vs {fast}"
        );
        let mut f = LipFlap::default();
        assert!((f.end_smile_s() - 1.5).abs() < 1e-4);
        f.start();
        f.finish();
        assert_eq!(f.tick(1.4), Some(Mouth::Smile));
        assert_eq!(f.tick(0.2), None);
    }

    #[test]
    fn queue_never_lags_far_behind() {
        let mut f = LipFlap::default();
        f.start();
        f.on_token(&"banana ".repeat(50));
        assert!(f.queue.iter().map(|q| q.1).sum::<f32>() <= MAX_QUEUE_S + 1e-3);
        f.finish();
        // Bounded drain + end smile.
        let mut t = 0.0;
        while f.active() {
            f.tick(1.0 / 30.0);
            t += 1.0 / 30.0;
            assert!(t < MAX_QUEUE_S + f.end_smile_s() + 0.5, "lip-flap must end");
        }
    }

    #[test]
    fn patch_mode_uses_recommended_shape_rate() {
        let mut f = LipFlap::with_speed(DEFAULT_TALK_SPEED);
        f.set_shape_ms(Some((200, 260)));
        assert!((f.shape_s().unwrap() - 0.23).abs() < 1e-4);
        // Faster talk_speed -> shorter shapes.
        let mut fast = LipFlap::with_speed(DEFAULT_TALK_SPEED * 2.0);
        fast.set_shape_ms(Some((200, 260)));
        assert!((fast.shape_s().unwrap() - 0.115).abs() < 1e-4);
        // One shape per vowel group, alternating open/mid; punctuation
        // closes; split tokens schedule the same as one.
        f.start();
        f.on_token("hello the");
        f.on_token("re, cutie~");
        let mut g = LipFlap::with_speed(DEFAULT_TALK_SPEED);
        g.set_shape_ms(Some((200, 260)));
        g.start();
        g.on_token("hello there, cutie~");
        assert!((f.queued_s() - g.queued_s()).abs() < 1e-4);
        // he-llo the-re , cu-tie ~ : 6 syllables + close + smile.
        let expect = 6.0 * 0.23 + 0.23 * 1.4 + 0.23 * 1.6;
        assert!((g.queued_s() - expect).abs() < 1e-3, "{}", g.queued_s());
        let seq = run(&mut g, 3.0);
        assert_eq!(seq[0], Mouth::Open);
        assert!(seq.contains(&Mouth::Mid) && seq.contains(&Mouth::Smile));
        // Shapes last ~230 ms: ~4-5 changes per second at most.
        let mut h = LipFlap::with_speed(DEFAULT_TALK_SPEED);
        h.set_shape_ms(Some((200, 260)));
        h.start();
        h.on_token("banana banana banana banana banana");
        let changes = run(&mut h, 1.0).len();
        assert!((3..=6).contains(&changes), "{changes} changes in 1 s");
    }
}
