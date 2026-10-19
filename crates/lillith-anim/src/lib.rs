//! # lillith-anim
//!
//! The sprite/animation engine (PLAN.md §1.2, §4):
//!
//! - [`atlas`]: loads the atlases + `manifest.toml` produced by `tools/slice-sheets`
//!   or `tools/import-manifest` (optional per-frame durations, baseline, scale).
//! - [`player`]: clip player (fixed fps or per-frame durations, loop/once, feet anchor).
//! - [`gestures`]: procedural gestures ported from `aether_animator.py`
//!   (easing/tweens become spring targets and impulses).
//! - [`particles`]: cyan/magenta/purple/gold/red particle bursts (TODO, M9).
//!
//! Frames are composited on the CPU with `tiny-skia` into a [`Canvas`].

pub mod atlas;
pub mod gestures;
pub mod particles;
pub mod player;

/// CPU render target shared with the overlay (premultiplied RGBA).
pub type Canvas = tiny_skia::Pixmap;
