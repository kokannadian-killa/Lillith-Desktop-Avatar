//! Clip player: advances frame indices at the clip's fps.

use crate::atlas::LoopMode;

/// Plays one clip of `frame_count` frames.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipPlayer {
    pub frame_count: usize,
    pub fps: f32,
    pub mode: LoopMode,
    elapsed: f32,
}

impl ClipPlayer {
    pub fn new(frame_count: usize, fps: f32, mode: LoopMode) -> Self {
        Self {
            frame_count: frame_count.max(1),
            fps,
            mode,
            elapsed: 0.0,
        }
    }

    pub fn advance(&mut self, dt: f32) {
        self.elapsed += dt;
    }

    /// Current frame index.
    pub fn frame(&self) -> usize {
        let raw = (self.elapsed * self.fps) as usize;
        match self.mode {
            LoopMode::Loop => raw % self.frame_count,
            LoopMode::Once => raw.min(self.frame_count - 1),
        }
    }

    /// `true` once a `Once` clip has shown its last frame for a full frame time.
    pub fn finished(&self) -> bool {
        self.mode == LoopMode::Once && (self.elapsed * self.fps) as usize >= self.frame_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
