//! Her speech bubble drawn on the overlay canvas, above her head: a small
//! pink rounded box with a tail pointing at her head, up to
//! [`MAX_LINES`] word-wrapped lines showing the newest text.
//!
//! The overlay window grows upward by [`SpeechLayout::pad`] for it (her feet
//! stay put; see [`crate::stage::Stage::with_speech_area`]). The bubble is
//! drawn after the click mask is computed, so it never catches clicks.

use lillith_anim::text::{last_lines, sanitize, Fonts};
use lillith_anim::Canvas;
use tiny_skia::{FillRule, Paint, PathBuilder, Stroke, Transform};

/// Lines shown at once (older lines scroll away).
pub const MAX_LINES: usize = 3;
/// Where the chat window's left edge sits, as a fraction of the overlay
/// width (see `ChatAnchor::bubble_pos`).
pub const CHAT_WINDOW_X: f32 = 5.0 / 8.0;
/// Text size (px) at overlay scale 0.75.
pub const BASE_FONT_PX: f32 = 13.0;

const FILL: [u8; 3] = [255, 231, 243];
const STROKE: [u8; 4] = [236, 72, 153, 255];
const INK: [u8; 4] = [74, 20, 64, 255];

/// What the bubble shows: the revealed text and its opacity (fade-out).
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechView {
    pub text: String,
    pub alpha: f32,
}

/// Bubble metrics for an overlay `width` px wide at sprite `scale`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeechLayout {
    pub font_px: f32,
    pub line_h: f32,
    pub pad_x: f32,
    pub pad_y: f32,
    pub tail_h: f32,
    pub box_w: f32,
    /// The bubble stays left of this x (px): the chat window opens at 5/8
    /// of the overlay width, right of her head, and would cover it.
    pub max_right: f32,
    /// Extra px above the sprite cell reserved for the bubble.
    pub pad: u32,
}

impl SpeechLayout {
    pub fn new(fonts: &Fonts, width: u32, scale: f32) -> Self {
        let k = (scale / 0.75).clamp(0.6, 3.0);
        let font_px = (BASE_FONT_PX * k).max(10.0);
        let line_h = fonts.line_height(font_px).ceil();
        let pad_x = (8.0 * k).round();
        let pad_y = (5.0 * k).round();
        let tail_h = (9.0 * k).round();
        let max_right = (width as f32 * CHAT_WINDOW_X - 3.0).max(63.0);
        let box_w = (max_right - 3.0).min(250.0 * k).max(60.0);
        let pad = (MAX_LINES as f32 * line_h + 2.0 * pad_y + tail_h + 3.0).ceil() as u32;
        Self {
            font_px,
            line_h,
            pad_x,
            pad_y,
            tail_h,
            box_w,
            max_right,
            pad,
        }
    }

    /// The (at most [`MAX_LINES`]) lines `text` shows as.
    pub fn lines(&self, fonts: &Fonts, text: &str) -> Vec<String> {
        let clean = sanitize(text.trim());
        let all = fonts.wrap(&clean, self.font_px, self.box_w - 2.0 * self.pad_x);
        last_lines(&all, MAX_LINES).to_vec()
    }
}

fn paint(c: [u8; 4], alpha: f32) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c[0], c[1], c[2], (c[3] as f32 * alpha).round() as u8);
    p.anti_alias = true;
    p
}

/// Draws the bubble into `canvas` (top `l.pad` px), centred on `head_x`
/// (clamped on screen) with the tail tip at `(head_x, tip_y)`.
pub fn draw_speech(
    canvas: &mut Canvas,
    fonts: &Fonts,
    l: &SpeechLayout,
    lines: &[String],
    head_x: f32,
    tip_y: f32,
    alpha: f32,
) {
    if lines.iter().all(|s| s.is_empty()) || alpha <= 0.01 {
        return;
    }
    let alpha = alpha.clamp(0.0, 1.0);
    let cw = canvas.width() as f32;
    let widest = lines
        .iter()
        .map(|s| fonts.measure(s, l.font_px))
        .fold(0.0f32, f32::max);
    let bw = (widest + 2.0 * l.pad_x).clamp(48.0, l.box_w).ceil();
    let bh = lines.len() as f32 * l.line_h + 2.0 * l.pad_y;
    let x0 = clampf(head_x - bw / 2.0, 3.0, l.max_right.min(cw - 3.0) - bw);
    let y1 = l.pad as f32 - l.tail_h - 1.0;
    let y0 = y1 - bh;
    let r = (bh / 2.0).min(10.0 * l.font_px / BASE_FONT_PX);
    let rect = rounded_rect(x0, y0, bw, bh, r);
    // Tail: base inside the box, tip at the head.
    let tx = clampf(head_x, x0 + r + 4.0, x0 + bw - r - 4.0);
    let half = (5.0 * l.font_px / BASE_FONT_PX).max(3.0);
    let tail = {
        let mut pb = PathBuilder::new();
        pb.move_to(tx - half, y1 - 2.0);
        pb.line_to(head_x.clamp(tx - 3.0 * half, tx + 3.0 * half), tip_y);
        pb.line_to(tx + half, y1 - 2.0);
        pb.close();
        pb.finish()
    };
    let stroke = Stroke {
        width: 3.0 * l.font_px / BASE_FONT_PX,
        ..Stroke::default()
    };
    let id = Transform::identity();
    if let (Some(rect), Some(tail)) = (rect, tail) {
        // Strokes first, fills on top: the fills hide the inner half of the
        // outline and the seam where the tail meets the box.
        canvas.stroke_path(&rect, &paint(STROKE, alpha), &stroke, id, None);
        canvas.stroke_path(&tail, &paint(STROKE, alpha), &stroke, id, None);
        let fill = paint([FILL[0], FILL[1], FILL[2], 255], alpha);
        canvas.fill_path(&rect, &fill, FillRule::Winding, id, None);
        canvas.fill_path(&tail, &fill, FillRule::Winding, id, None);
    }
    let asc = fonts.ascent(l.font_px);
    for (i, line) in lines.iter().enumerate() {
        let lw = fonts.measure(line, l.font_px);
        let x = x0 + ((bw - lw) / 2.0).max(l.pad_x);
        let base = y0 + l.pad_y + i as f32 * l.line_h + asc;
        fonts.draw(canvas, line, x, base, l.font_px, INK, alpha);
    }
}

/// `v.clamp(lo, hi)`, or the midpoint when the range is empty.
fn clampf(v: f32, lo: f32, hi: f32) -> f32 {
    if lo > hi {
        (lo + hi) / 2.0
    } else {
        v.clamp(lo, hi)
    }
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_keeps_three_newest_lines_and_draws_inside_pad() {
        let f = Fonts::bundled();
        let l = SpeechLayout::new(&f, 300, 0.75);
        assert!(l.pad > 3 * l.line_h as u32 && l.pad < 120, "pad {}", l.pad);
        let text = "Mmm, hi there cutie~ I was just stretching my tail and thinking \
                    about you. Want to tell me about your day? 🥰💕";
        let lines = l.lines(&f, text);
        assert_eq!(lines.len(), MAX_LINES);
        assert!(lines.last().unwrap().ends_with("🥰💕"), "{lines:?}");
        let h = 300 + l.pad;
        let mut c = Canvas::new(300, h).unwrap();
        draw_speech(&mut c, &f, &l, &lines, 150.0, l.pad as f32 + 4.0, 1.0);
        let ink: Vec<usize> = c
            .data()
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, p)| p[3] > 0)
            .map(|(i, _)| i / 300)
            .collect();
        assert!(!ink.is_empty());
        // Stays clear of the chat window (opens at 5/8 of the width).
        let max_x = c
            .data()
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, p)| p[3] > 0)
            .map(|(i, _)| i % 300)
            .max()
            .unwrap() as f32;
        assert!(max_x <= 300.0 * CHAT_WINDOW_X + 2.0, "x {max_x}");
        let max_y = *ink.iter().max().unwrap() as u32;
        // Only the tail tip (+ its outline) reaches past the pad.
        assert!(max_y <= l.pad + 10, "bubble spills onto her: y {max_y}");
        // Faded out: nothing drawn.
        let mut c2 = Canvas::new(300, h).unwrap();
        draw_speech(&mut c2, &f, &l, &lines, 150.0, l.pad as f32, 0.0);
        assert!(c2.data().iter().all(|&b| b == 0));
    }
}
