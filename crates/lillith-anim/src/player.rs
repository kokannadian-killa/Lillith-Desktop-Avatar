//! Clip player: advances frame indices at the clip's fps, or by optional
//! per-frame durations when the clip provides them.

use crate::atlas::{Clip, LoopMode};

/// Plays one clip of `frame_count` frames.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipPlayer {
    pub frame_count: usize,
    pub fps: f32,
    pub mode: LoopMode,
    /// Cumulative end time (seconds) of each frame when per-frame durations
    /// are in use; `None` means fixed `fps`.
    frame_ends: Option<Vec<f32>>,
    elapsed: f32,
}

impl ClipPlayer {
    /// Fixed-rate player (every frame lasts `1 / fps`).
    pub fn new(frame_count: usize, fps: f32, mode: LoopMode) -> Self {
        Self {
            frame_count: frame_count.max(1),
            fps,
            mode,
            frame_ends: None,
            elapsed: 0.0,
        }
    }

    /// Player with explicit per-frame durations in milliseconds. Falls back to
    /// fixed `fps` if `durations_ms` is empty or contains a zero.
    pub fn with_durations_ms(durations_ms: &[u32], fps: f32, mode: LoopMode) -> Self {
        let mut p = Self::new(durations_ms.len(), fps, mode);
        if !durations_ms.is_empty() && !durations_ms.contains(&0) {
            let mut t = 0.0;
            p.frame_ends = Some(
                durations_ms
                    .iter()
                    .map(|&d| {
                        t += d as f32 / 1000.0;
                        t
                    })
                    .collect(),
            );
        }
        p
    }

    /// Builds a player for `clip`, using its per-frame durations if present
    /// (and consistent with its frame count), else its fps.
    pub fn for_clip(clip: &Clip) -> Self {
        match &clip.frame_durations_ms {
            Some(ms) if ms.len() == clip.frames.len() => {
                Self::with_durations_ms(ms, clip.fps, clip.mode)
            }
            _ => Self::new(clip.frames.len(), clip.fps, clip.mode),
        }
    }

    /// Restarts from the first frame.
    pub fn reset(&mut self) {
        self.elapsed = 0.0;
    }

    pub fn advance(&mut self, dt: f32) {
        self.elapsed += dt;
    }

    /// Length of one pass through the clip, in seconds.
    pub fn duration_s(&self) -> f32 {
        match &self.frame_ends {
            Some(ends) => *ends.last().unwrap_or(&0.0),
            None => self.frame_count as f32 / self.fps,
        }
    }

    /// Current frame index.
    pub fn frame(&self) -> usize {
        match &self.frame_ends {
            None => {
                let raw = (self.elapsed * self.fps) as usize;
                match self.mode {
                    LoopMode::Loop => raw % self.frame_count,
                    LoopMode::Once => raw.min(self.frame_count - 1),
                }
            }
            Some(ends) => {
                let total = self.duration_s();
                let t = match self.mode {
                    LoopMode::Loop if total > 0.0 => self.elapsed.rem_euclid(total),
                    _ => self.elapsed,
                };
                ends.iter()
                    .position(|&end| t < end)
                    .unwrap_or(self.frame_count - 1)
            }
        }
    }

    /// `true` once a `Once` clip has shown its last frame for its full time.
    pub fn finished(&self) -> bool {
        if self.mode != LoopMode::Once {
            return false;
        }
        match &self.frame_ends {
            None => (self.elapsed * self.fps) as usize >= self.frame_count,
            Some(_) => self.elapsed >= self.duration_s(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atlas::FrameRect;

    #[test]
    fn loops_and_stops() {
        let mut p = ClipPlayer::new(4, 12.0, LoopMode::Loop);
        p.advance(5.0 / 12.0 + 0.001);
        assert_eq!(p.frame(), 1);
        let mut o = ClipPlayer::new(4, 12.0, LoopMode::Once);
        o.advance(1.0);
        assert_eq!(o.frame(), 3);
        assert!(o.finished());
    }

    const WAKE_MS: [u32; 10] = [450, 350, 160, 260, 450, 140, 220, 450, 200, 600];

    #[test]
    fn per_frame_durations_once() {
        let mut p = ClipPlayer::with_durations_ms(&WAKE_MS, 6.0, LoopMode::Once);
        assert!((p.duration_s() - 3.28).abs() < 1e-4);
        assert_eq!(p.frame(), 0);
        p.advance(0.449);
        assert_eq!(p.frame(), 0);
        p.advance(0.002); // 0.451
        assert_eq!(p.frame(), 1);
        p.advance(0.360); // 0.811 > 0.800
        assert_eq!(p.frame(), 2);
        p.advance(3.27 - 0.811);
        assert_eq!(p.frame(), 9);
        assert!(!p.finished());
        p.advance(0.02); // 3.29
        assert!(p.finished());
        p.advance(10.0);
        assert_eq!(p.frame(), 9);
    }

    #[test]
    fn per_frame_durations_loop() {
        let mut p = ClipPlayer::with_durations_ms(&[100, 300], 6.0, LoopMode::Loop);
        p.advance(0.35);
        assert_eq!(p.frame(), 1);
        p.advance(0.1); // 0.45 -> 0.05 into second pass
        assert_eq!(p.frame(), 0);
        assert!(!p.finished());
    }

    #[test]
    fn for_clip_uses_durations_or_fps() {
        let frame = FrameRect {
            x: 0,
            y: 0,
            w: 1,
            h: 1,
            anchor: (0, 0),
        };
        let mut clip = Clip {
            page: "p.png".into(),
            fps: 6.0,
            mode: LoopMode::Once,
            frame_durations_ms: None,
            baseline_y: None,
            scale: None,
            tags: vec![],
            points: Default::default(),
            frames: vec![frame; 10],
        };
        let flat = ClipPlayer::for_clip(&clip);
        assert!((flat.duration_s() - 10.0 / 6.0).abs() < 1e-4);
        clip.frame_durations_ms = Some(WAKE_MS.to_vec());
        let timed = ClipPlayer::for_clip(&clip);
        assert!((timed.duration_s() - 3.28).abs() < 1e-4);
    }
}
