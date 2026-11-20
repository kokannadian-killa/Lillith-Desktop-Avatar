//! Colours, as straight (non-premultiplied) sRGB RGBA.
//!
//! Dark obsidian/iron base, crimson blood, electric-blue sigil neon, violet
//! accents, and Lillith pink kept only for the selected/hovered item.

/// Straight-alpha sRGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    /// `0xRRGGBB`, opaque.
    pub const fn hex(rgb: u32) -> Self {
        Self {
            r: (rgb >> 16) as u8,
            g: (rgb >> 8) as u8,
            b: rgb as u8,
            a: 255,
        }
    }

    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Same colour, alpha `a` (0-255).
    pub const fn alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    /// Same colour with alpha multiplied by `f` (0..=1).
    pub fn fade(self, f: f32) -> Self {
        Self {
            a: (self.a as f32 * f.clamp(0.0, 1.0)).round() as u8,
            ..self
        }
    }

    /// Linear blend towards `o` (t = 0 → self, 1 → o), per channel.
    pub fn lerp(self, o: Rgba, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Self {
            r: m(self.r, o.r),
            g: m(self.g, o.g),
            b: m(self.b, o.b),
            a: m(self.a, o.a),
        }
    }

    pub const fn to_array(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// Premultiplied RGBA (what tiny-skia `PremultipliedColorU8` and egui
    /// `Color32` store).
    pub fn premultiplied(self) -> [u8; 4] {
        let f = self.a as f32 / 255.0;
        let m = |c: u8| (c as f32 * f).round() as u8;
        [m(self.r), m(self.g), m(self.b), self.a]
    }

    /// WCAG relative luminance (alpha ignored).
    pub fn luminance(self) -> f32 {
        fn lin(c: u8) -> f32 {
            let c = c as f32 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }

    /// WCAG contrast ratio between two opaque colours (1..=21).
    pub fn contrast(self, o: Rgba) -> f32 {
        let (a, b) = (self.luminance(), o.luminance());
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }
}

// ---- base: obsidian, iron, stone -------------------------------------------
/// Deepest background (window void).
pub const VOID: Rgba = Rgba::hex(0x050003);
/// Obsidian panel base.
pub const OBSIDIAN: Rgba = Rgba::hex(0x0c080b);
pub const OBSIDIAN_HI: Rgba = Rgba::hex(0x171015);
/// Black iron frame.
pub const IRON: Rgba = Rgba::hex(0x1b1619);
pub const IRON_MID: Rgba = Rgba::hex(0x2c2429);
pub const IRON_HI: Rgba = Rgba::hex(0x4a3f46);
/// Edge highlight on carved iron/stone.
pub const IRON_EDGE: Rgba = Rgba::hex(0x6e6168);
/// Carved stone (tabs, plaques).
pub const STONE: Rgba = Rgba::hex(0x221b1f);
pub const STONE_HI: Rgba = Rgba::hex(0x3b3036);
/// Bone/ivory for skull ornaments.
pub const BONE: Rgba = Rgba::hex(0xbdb2a3);
pub const BONE_SHADOW: Rgba = Rgba::hex(0x5b5148);

// ---- blood -------------------------------------------------------------------
pub const BLOOD_DRIED: Rgba = Rgba::hex(0x3a0006);
pub const BLOOD_DEEP: Rgba = Rgba::hex(0x5c000b);
pub const BLOOD: Rgba = Rgba::hex(0x8a0010);
pub const CRIMSON: Rgba = Rgba::hex(0xc4001a);
/// Fresh blood / neon red (terminal borders, grid, glitch labels).
pub const BLOOD_NEON: Rgba = Rgba::hex(0xff1a2e);
/// Wet highlight on a drip.
pub const BLOOD_SHEEN: Rgba = Rgba::hex(0xff6b78);

// ---- DemonOS terminal --------------------------------------------------------
/// Translucent red terminal fill.
pub const TERMINAL_FILL: Rgba = Rgba::new(0x4a, 0x00, 0x08, 0xb0);
pub const TERMINAL_FILL_HI: Rgba = Rgba::new(0x6a, 0x00, 0x0e, 0xc0);
/// Grid lines over terminal panels.
/// (egui blends in linear light, so small alphas already read clearly.)
pub const TERMINAL_GRID: Rgba = Rgba::new(0xff, 0x1a, 0x2e, 0x06);
/// Terminal text.
pub const TERMINAL_TEXT: Rgba = Rgba::hex(0xffb3ba);

// ---- neon --------------------------------------------------------------------
/// Electric-blue sigil neon.
pub const NEON_BLUE: Rgba = Rgba::hex(0x2fb4ff);
pub const NEON_BLUE_CORE: Rgba = Rgba::hex(0xcdf2ff);
/// Violet / purple neon accents.
pub const VIOLET: Rgba = Rgba::hex(0xa24dff);
pub const VIOLET_DEEP: Rgba = Rgba::hex(0x4b1a8c);
/// Lillith pink: the selected / hovered item only.
pub const PINK: Rgba = Rgba::hex(0xff2bd6);
/// Ember glow in stone cracks.
pub const EMBER: Rgba = Rgba::hex(0xff5a1a);
pub const EMBER_CORE: Rgba = Rgba::hex(0xffc06a);

// ---- text --------------------------------------------------------------------
pub const TEXT: Rgba = Rgba::hex(0xf3e4e6);
pub const TEXT_DIM: Rgba = Rgba::hex(0xc9a3a8);
pub const TEXT_MUTE: Rgba = Rgba::hex(0x94767c);

// ---- cyberpunk / QuantumSec (additive palette; existing colours untouched) ------
/// Void black (QuantumSec awakening / cyber panels).
pub const CYBER_VOID: Rgba = Rgba::hex(0x0a0a0f);
/// Crimson accent (#ff3e6c).
pub const CYBER_CRIMSON: Rgba = Rgba::hex(0xff3e6c);
/// Cyber cyan (#00f0ff).
pub const CYBER_CYAN: Rgba = Rgba::hex(0x00f0ff);
/// Phantom purple (#b829f7).
pub const CYBER_PURPLE: Rgba = Rgba::hex(0xb829f7);
/// Soft cyan for body text on void.
pub const CYBER_TEXT: Rgba = Rgba::hex(0xd6fbff);
/// Dim cyan/gray secondary text.
pub const CYBER_TEXT_DIM: Rgba = Rgba::hex(0x7aa8b0);

// ---- status ------------------------------------------------------------------
pub const OK: Rgba = Rgba::hex(0x39ff88);
pub const WARN: Rgba = Rgba::hex(0xffb02e);
pub const BAD: Rgba = BLOOD_NEON;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_and_alpha() {
        let c = Rgba::hex(0x123456);
        assert_eq!(c.to_array(), [0x12, 0x34, 0x56, 255]);
        assert_eq!(c.alpha(7).a, 7);
        assert_eq!(c.fade(0.5).a, 128);
        assert_eq!(
            Rgba::new(200, 100, 50, 128).premultiplied(),
            [100, 50, 25, 128]
        );
        assert_eq!(VOID.lerp(TEXT, 0.0), VOID);
        assert_eq!(VOID.lerp(TEXT, 1.0), TEXT);
    }

    #[test]
    fn text_stays_readable() {
        // WCAG AA (4.5:1) for body text on every surface we put text on.
        for bg in [VOID, OBSIDIAN, OBSIDIAN_HI, IRON, STONE, BLOOD_DEEP] {
            assert!(
                TEXT.contrast(bg) >= 7.0,
                "TEXT on {bg:?}: {}",
                TEXT.contrast(bg)
            );
            assert!(
                TEXT_DIM.contrast(bg) >= 4.5,
                "TEXT_DIM on {bg:?}: {}",
                TEXT_DIM.contrast(bg)
            );
        }
        assert!(TERMINAL_TEXT.contrast(BLOOD_DEEP) >= 4.5);
        assert!(TEXT_MUTE.contrast(VOID) >= 4.5);
    }

    #[test]
    fn cyberpunk_palette_hexes() {
        assert_eq!(CYBER_VOID.to_array(), [0x0a, 0x0a, 0x0f, 255]);
        assert_eq!(CYBER_CRIMSON.to_array(), [0xff, 0x3e, 0x6c, 255]);
        assert_eq!(CYBER_CYAN.to_array(), [0x00, 0xf0, 0xff, 255]);
        assert_eq!(CYBER_PURPLE.to_array(), [0xb8, 0x29, 0xf7, 255]);
        assert!(CYBER_TEXT.contrast(CYBER_VOID) >= 4.5);
    }

    #[test]
    fn contrast_is_symmetric_and_bounded() {
        let c = Rgba::hex(0xffffff).contrast(Rgba::hex(0x000000));
        assert!((c - 21.0).abs() < 0.01);
        assert_eq!(TEXT.contrast(VOID), VOID.contrast(TEXT));
    }
}
