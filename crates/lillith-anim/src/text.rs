//! Small CPU text renderer for the overlay speech bubble (`ab_glyph`), with
//! a per-character font fallback chain so emoji never draw as boxes:
//!
//! 1. Ubuntu Light (egui's default UI font, `epaint_default_fonts`, UFL),
//! 2. Noto Emoji (monochrome, current Unicode coverage, bundled in
//!    `fonts/NotoEmoji-Regular.ttf`, SIL OFL 1.1, see `fonts/OFL-NotoEmoji.txt`),
//! 3. egui's emoji-icon font (MIT),
//! 4. DejaVu Sans from the system, if installed (wide symbol coverage).
//!
//! Characters no font has are skipped, never drawn as tofu. [`sanitize`]
//! drops invisible emoji modifiers (variation selectors, ZWJ, skin tones)
//! first: a ZWJ family then draws as its parts, which beats a box.

use crate::Canvas;
use ab_glyph::{point, Font, FontArc, PxScale, ScaleFont};

/// Noto Emoji (monochrome outline font, SIL OFL 1.1). Also used by the egui
/// chat bubble as a fallback font.
pub const NOTO_EMOJI: &[u8] = include_bytes!("../fonts/NotoEmoji-Regular.ttf");
/// License text for [`NOTO_EMOJI`].
pub const NOTO_EMOJI_LICENSE: &str = include_str!("../fonts/OFL-NotoEmoji.txt");
/// System fonts tried last (any that exist are added to the chain).
pub const SYSTEM_FALLBACKS: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
];

/// Removes characters that only modify a neighbouring emoji and that
/// outline fonts don't draw (they would show as boxes): variation
/// selectors, zero-width joiner, skin-tone modifiers, tag characters,
/// keycap combiner; also other control chars except `\n`/`\t`.
pub fn sanitize(s: &str) -> String {
    s.chars()
        .filter(|&c| {
            let u = c as u32;
            !(matches!(u, 0xFE00..=0xFE0F | 0x200D | 0x200B | 0x20E3 | 0x1F3FB..=0x1F3FF)
                || (0xE0000..=0xE007F).contains(&u)
                || (0xE0100..=0xE01EF).contains(&u)
                || (c.is_control() && c != '\n' && c != '\t'))
        })
        .collect()
}

/// Fallback font chain.
#[derive(Clone)]
pub struct Fonts {
    faces: Vec<FontArc>,
}

impl std::fmt::Debug for Fonts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fonts")
            .field("faces", &self.faces.len())
            .finish()
    }
}

impl Default for Fonts {
    fn default() -> Self {
        Self::new()
    }
}

impl Fonts {
    /// The bundled chain plus any [`SYSTEM_FALLBACKS`] present.
    pub fn new() -> Self {
        let mut f = Self::bundled();
        for p in SYSTEM_FALLBACKS {
            if let Some(face) = std::fs::read(p)
                .ok()
                .and_then(|b| FontArc::try_from_vec(b).ok())
            {
                f.faces.push(face);
                break;
            }
        }
        f
    }

    /// Only the fonts compiled into the binary (deterministic, for tests).
    pub fn bundled() -> Self {
        let faces = [
            epaint_default_fonts::UBUNTU_LIGHT,
            NOTO_EMOJI,
            epaint_default_fonts::EMOJI_ICON,
        ]
        .into_iter()
        .filter_map(|b| FontArc::try_from_slice(b).ok())
        .collect();
        Self { faces }
    }

    /// Index of the first font with a glyph for `c`.
    pub fn face_for(&self, c: char) -> Option<usize> {
        self.faces.iter().position(|f| f.glyph_id(c).0 != 0)
    }

    /// `true` if some font draws `c`.
    pub fn covers(&self, c: char) -> bool {
        self.face_for(c).is_some()
    }

    fn advance(&self, c: char, px: f32) -> f32 {
        match self.face_for(c) {
            Some(i) => {
                let f = &self.faces[i];
                f.as_scaled(PxScale::from(px)).h_advance(f.glyph_id(c))
            }
            None if c == ' ' => px * 0.25,
            None => 0.0,
        }
    }

    /// Width (px) of `s` on one line.
    pub fn measure(&self, s: &str, px: f32) -> f32 {
        s.chars().map(|c| self.advance(c, px)).sum()
    }

    /// Line height (px) at size `px`.
    pub fn line_height(&self, px: f32) -> f32 {
        self.faces
            .first()
            .map(|f| {
                let s = f.as_scaled(PxScale::from(px));
                s.ascent() - s.descent() + s.line_gap()
            })
            .unwrap_or(px * 1.2)
    }

    /// Ascent (px) of the primary font.
    pub fn ascent(&self, px: f32) -> f32 {
        self.faces
            .first()
            .map(|f| f.as_scaled(PxScale::from(px)).ascent())
            .unwrap_or(px * 0.8)
    }

    /// Word-wraps `text` to `max_w` px (explicit `\n` breaks lines; words
    /// longer than a line are split).
    pub fn wrap(&self, text: &str, px: f32, max_w: f32) -> Vec<String> {
        let mut lines = Vec::new();
        for para in text.split('\n') {
            let mut line = String::new();
            let mut w = 0.0;
            for word in para.split_whitespace() {
                let ww = self.measure(word, px);
                let sp = if line.is_empty() {
                    0.0
                } else {
                    self.advance(' ', px)
                };
                if w + sp + ww <= max_w {
                    if !line.is_empty() {
                        line.push(' ');
                    }
                    line.push_str(word);
                    w += sp + ww;
                    continue;
                }
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                    w = 0.0;
                }
                if ww <= max_w {
                    line.push_str(word);
                    w = ww;
                    continue;
                }
                // Longer than a line: hard-split.
                for c in word.chars() {
                    let cw = self.advance(c, px);
                    if w + cw > max_w && !line.is_empty() {
                        lines.push(std::mem::take(&mut line));
                        w = 0.0;
                    }
                    line.push(c);
                    w += cw;
                }
            }
            lines.push(line);
        }
        // Drop trailing empty lines (a reply ending in "\n").
        while lines.len() > 1 && lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines
    }

    /// Draws `s` with its baseline starting at `(x, baseline)` in
    /// premultiplied `rgba` (straight colour) at opacity `alpha`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        canvas: &mut Canvas,
        s: &str,
        x: f32,
        baseline: f32,
        px: f32,
        rgba: [u8; 4],
        alpha: f32,
    ) {
        let (cw, ch) = (canvas.width() as i32, canvas.height() as i32);
        let data = canvas.data_mut();
        let mut pen = x;
        for c in s.chars() {
            let Some(i) = self.face_for(c) else {
                pen += self.advance(c, px);
                continue;
            };
            let f = &self.faces[i];
            let id = f.glyph_id(c);
            let g = id.with_scale_and_position(PxScale::from(px), point(pen, baseline));
            pen += f.as_scaled(PxScale::from(px)).h_advance(id);
            let Some(og) = f.outline_glyph(g) else {
                continue;
            };
            let b = og.px_bounds();
            let a0 = rgba[3] as f32 / 255.0 * alpha.clamp(0.0, 1.0);
            og.draw(|gx, gy, cov| {
                let px_ = b.min.x as i32 + gx as i32;
                let py_ = b.min.y as i32 + gy as i32;
                if px_ < 0 || py_ < 0 || px_ >= cw || py_ >= ch {
                    return;
                }
                let a = (cov.clamp(0.0, 1.0) * a0).clamp(0.0, 1.0);
                if a <= 0.0 {
                    return;
                }
                let k = ((py_ * cw + px_) * 4) as usize;
                let inv = 1.0 - a;
                for (j, col) in rgba[..3].iter().enumerate() {
                    let v = *col as f32 * a + data[k + j] as f32 * inv;
                    data[k + j] = v.round().clamp(0.0, 255.0) as u8;
                }
                let v = 255.0 * a + data[k + 3] as f32 * inv;
                data[k + 3] = v.round().clamp(0.0, 255.0) as u8;
            });
        }
    }
}

/// The last `n` of `lines` (auto-scroll to the newest text).
pub fn last_lines(lines: &[String], n: usize) -> &[String] {
    &lines[lines.len().saturating_sub(n)..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_have_glyphs_and_modifiers_are_dropped() {
        let f = Fonts::bundled();
        for c in "Hey cutie~ 😘💋🥰😉😏✨💕❤🫶🫠🥺😈🔥👀😊😳🤭💖🌸 ♥ … ’".chars()
        {
            assert!(
                c == ' ' || f.covers(c),
                "no glyph for {c:?} U+{:04X}",
                c as u32
            );
        }
        assert_eq!(sanitize("❤️ hi 👋🏽\u{200D}x\u{0007}\n"), "❤ hi 👋x\n");
    }

    #[test]
    fn wraps_and_keeps_last_lines() {
        let f = Fonts::bundled();
        let lines = f.wrap(
            "one two three four five six seven eight nine ten",
            14.0,
            80.0,
        );
        assert!(lines.len() >= 3, "{lines:?}");
        for l in &lines {
            assert!(f.measure(l, 14.0) <= 80.0 + 0.01, "{l:?} too wide");
        }
        assert_eq!(
            lines.join(" "),
            "one two three four five six seven eight nine ten"
        );
        let tail = last_lines(&lines, 3);
        assert_eq!(tail.len(), 3);
        assert_eq!(tail.last(), lines.last());
        // A word longer than a line is split, explicit newlines kept.
        let l = f.wrap("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nb", 14.0, 60.0);
        assert!(l.len() >= 3 && l.last().unwrap() == "b", "{l:?}");
    }

    #[test]
    fn draws_ink() {
        let f = Fonts::bundled();
        let mut c = Canvas::new(120, 30).unwrap();
        f.draw(&mut c, "Hi 🥰", 2.0, 20.0, 16.0, [40, 10, 30, 255], 1.0);
        let ink = c.data().chunks_exact(4).filter(|p| p[3] > 128).count();
        assert!(ink > 30, "ink {ink}");
        // The emoji half has ink too (no skipped glyph).
        let right = c
            .data()
            .chunks_exact(4)
            .enumerate()
            .filter(|(i, p)| i % 120 > 25 && p[3] > 128)
            .count();
        assert!(right > 10, "emoji ink {right}");
    }
}
