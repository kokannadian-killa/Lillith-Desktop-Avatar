//! Particle bursts (README-SP palette). TODO(M9).

use glam::Vec2;

/// A single particle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Particle {
    pub pos: Vec2,
    pub vel: Vec2,
    pub life_s: f32,
    pub rgba: [u8; 4],
}

/// Named particle colours from README-SP.
pub const PALETTE: &[(&str, [u8; 4])] = &[
    ("cyan", [0x5f, 0xd0, 0xe6, 0xff]),
    ("magenta", [0xe2, 0x3d, 0x8a, 0xff]),
    ("purple", [0xc0, 0x26, 0xd3, 0xff]),
    ("gold", [0xff, 0xc8, 0x57, 0xff]),
    ("red", [0xe0, 0x30, 0x30, 0xff]),
    ("soft", [0xf8, 0xee, 0xf4, 0xff]),
];
