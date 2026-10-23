//! The avatar-matched accent: the one hue that follows the active avatar
//! (hover blade, sigil, checks, focus rings). The demon base (black,
//! iron, blood red) never changes.
//!
//! Lillith's magenta is the canonical default ([`Accent::LILLITH`]). A
//! custom avatar uses the colour stored in its `avatar.json` if any
//! ([`Accent::from_hex`]), else one derived from its image
//! ([`Accent::derive`]: the dominant saturated hue, clamped so it stays a
//! readable neon on obsidian).

use crate::palette::{self as pal, Rgba};

/// Accent triple.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Accent {
    /// Neon colour (edges, sigil, radio fill).
    pub base: Rgba,
    /// Hot core (thin bright lines on top of `base`).
    pub hot: Rgba,
    /// Dark fill (blade body behind white text).
    pub deep: Rgba,
}

/// Minimum WCAG contrast of `base` against [`pal::OBSIDIAN`].
pub const MIN_CONTRAST: f32 = 5.0;
/// Minimum contrast of white text on `deep`.
pub const MIN_TEXT_CONTRAST: f32 = 7.0;

impl Accent {
    /// Lillith's magenta (her art's dominant hue), the canonical look.
    pub const LILLITH: Accent = Accent {
        base: pal::PINK,
        hot: Rgba::hex(0xffb8f0),
        deep: Rgba::hex(0x5e0a4f),
    };

    /// An accent built around `c` (any colour): hue kept, saturation and
    /// value clamped to a neon that reads on obsidian, plus its hot core
    /// and a dark body that keeps white text legible. Near-grey input
    /// keeps its hue but is pushed to a usable saturation.
    pub fn from_color(c: Rgba) -> Accent {
        let (h, s, _) = rgb_to_hsv(c);
        let s = s.clamp(0.62, 0.92);
        let mut base = hsv_to_rgb(h, s, 1.0);
        let white = Rgba::hex(0xffffff);
        let mut t = 0.0;
        while base.contrast(pal::OBSIDIAN) < MIN_CONTRAST && t < 1.0 {
            t += 0.05;
            base = hsv_to_rgb(h, s, 1.0).lerp(white, t);
        }
        let hot = base.lerp(white, 0.62);
        let mut v = 0.42;
        let mut deep = hsv_to_rgb(h, (s + 0.05).min(1.0), v);
        while white.contrast(deep) < MIN_TEXT_CONTRAST && v > 0.1 {
            v -= 0.03;
            deep = hsv_to_rgb(h, (s + 0.05).min(1.0), v);
        }
        Accent { base, hot, deep }
    }

    /// `"#rrggbb"` / `"rrggbb"` (as stored in `avatar.json`).
    pub fn from_hex(s: &str) -> Option<Accent> {
        let s = s.trim().trim_start_matches('#');
        if s.len() != 6 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(s, 16)
            .ok()
            .map(|v| Accent::from_color(Rgba::hex(v)))
    }

    /// The dominant saturated hue of an RGBA image (straight or
    /// premultiplied: only near-opaque pixels count), as an accent;
    /// `None` when the image has no clearly coloured pixels (greyscale,
    /// empty) so the caller keeps [`Accent::LILLITH`].
    pub fn derive(rgba: &[u8]) -> Option<Accent> {
        const BINS: usize = 36;
        let mut w = [0f32; BINS];
        let mut hx = [0f32; BINS];
        let mut hy = [0f32; BINS];
        let mut sat = [0f32; BINS];
        let mut total = 0usize;
        for px in rgba.chunks_exact(4).step_by(1) {
            if px[3] < 224 {
                continue;
            }
            total += 1;
            let (h, s, v) = rgb_to_hsv(Rgba::new(px[0], px[1], px[2], 255));
            if s < 0.35 || v < 0.25 {
                continue;
            }
            let k = s * s * v; // favour vivid colours over skin / beige
            let b = ((h / 360.0) * BINS as f32) as usize % BINS;
            let r = h.to_radians();
            w[b] += k;
            hx[b] += r.cos() * k;
            hy[b] += r.sin() * k;
            sat[b] += s * k;
        }
        // Smooth over neighbours so a hue split across two bins still wins.
        let score = |i: usize| w[(i + BINS - 1) % BINS] * 0.5 + w[i] + w[(i + 1) % BINS] * 0.5;
        let best = (0..BINS).max_by(|&a, &b| score(a).total_cmp(&score(b)))?;
        let picked: f32 = (0..BINS).map(|i| w[i]).sum::<f32>();
        if total == 0 || w[best] <= 0.0 || picked < total as f32 * 0.01 {
            return None;
        }
        let (mut x, mut y, mut ws, mut ss) = (0.0, 0.0, 0.0, 0.0);
        for i in [(best + BINS - 1) % BINS, best, (best + 1) % BINS] {
            x += hx[i];
            y += hy[i];
            ws += w[i];
            ss += sat[i];
        }
        let h = y.atan2(x).to_degrees().rem_euclid(360.0);
        Some(Accent::from_color(hsv_to_rgb(h, ss / ws, 1.0)))
    }

    /// `"#rrggbb"` of `base`.
    pub fn to_hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.base.r, self.base.g, self.base.b)
    }
}

impl Default for Accent {
    fn default() -> Self {
        Self::LILLITH
    }
}

/// Hue (degrees), saturation, value (0..=1).
pub fn rgb_to_hsv(c: Rgba) -> (f32, f32, f32) {
    let (r, g, b) = (c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= f32::EPSILON {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max <= 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Rgba {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let q = |f: f32| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Rgba::new(q(r), q(g), q(b), 255)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(px: &[(Rgba, usize)]) -> Vec<u8> {
        px.iter()
            .flat_map(|(c, n)| std::iter::repeat_n(c.to_array(), *n))
            .flatten()
            .collect()
    }

    #[test]
    fn lillith_is_the_default_and_readable() {
        assert_eq!(Accent::default(), Accent::LILLITH);
        let a = Accent::LILLITH;
        assert!(a.base.contrast(pal::OBSIDIAN) >= MIN_CONTRAST);
        assert!(Rgba::hex(0xffffff).contrast(a.deep) >= MIN_TEXT_CONTRAST);
    }

    #[test]
    fn hex_round_trip_and_rejects_junk() {
        let a = Accent::from_hex("#33ccff").unwrap();
        let (h, _, _) = rgb_to_hsv(a.base);
        assert!((h - 195.0).abs() < 6.0, "{h}");
        assert!(Accent::from_hex("33ccff").is_some());
        assert!(Accent::from_hex("#33ccf").is_none());
        assert!(Accent::from_hex("nope!!").is_none());
    }

    #[test]
    fn every_hue_is_clamped_for_contrast() {
        for h in (0..360).step_by(15) {
            for c in [
                hsv_to_rgb(h as f32, 1.0, 1.0),
                hsv_to_rgb(h as f32, 0.2, 0.2),
            ] {
                let a = Accent::from_color(c);
                assert!(a.base.contrast(pal::OBSIDIAN) >= MIN_CONTRAST, "{h}");
                assert!(
                    Rgba::hex(0xffffff).contrast(a.deep) >= MIN_TEXT_CONTRAST - 0.5,
                    "{h}"
                );
            }
        }
        // pure blue gets lifted
        let a = Accent::from_color(Rgba::hex(0x0000ff));
        assert!(a.base.contrast(pal::OBSIDIAN) >= MIN_CONTRAST);
    }

    #[test]
    fn derive_picks_the_dominant_saturated_hue() {
        // lots of skin + grey, a smaller but vivid teal outfit
        let px = img(&[
            (Rgba::hex(0xf1c7a8), 4000),
            (Rgba::hex(0x808080), 3000),
            (Rgba::hex(0x00c8b4), 1500),
            (Rgba::hex(0xff2020).alpha(10), 5000), // transparent: ignored
        ]);
        let a = Accent::derive(&px).unwrap();
        let (h, _, _) = rgb_to_hsv(a.base);
        assert!((h - 174.0).abs() < 8.0, "{h}");
        // greyscale → none (caller keeps Lillith)
        assert!(Accent::derive(&img(&[(Rgba::hex(0x777777), 100)])).is_none());
        assert!(Accent::derive(&[]).is_none());
    }

    #[test]
    fn derive_on_magenta_is_close_to_lillith() {
        let px = img(&[(Rgba::hex(0xe8288f), 800), (Rgba::hex(0x111111), 2000)]);
        let (h, _, _) = rgb_to_hsv(Accent::derive(&px).unwrap().base);
        let (hl, _, _) = rgb_to_hsv(Accent::LILLITH.base);
        assert!((h - hl).abs() < 25.0, "{h} vs {hl}");
    }
}
