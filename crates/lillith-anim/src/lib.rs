//! # lillith-anim
//!
//! The sprite/animation engine (PLAN.md §1.2, §4):
//!
//! - [`atlas`]: loads the atlases + `manifest.toml` produced by `tools/slice-sheets`
//!   or `tools/import-manifest` (optional per-frame durations, baseline, scale).
//! - [`animator`]: composes base clip + idle life + reaction queue + springs (M2).
//! - [`render`]: CPU compositing of an animator frame (spring transform, flip).
//! - [`preview`]: headless clip preview (contact sheets / frame dumps).
//! - [`player`]: clip player (fixed fps or per-frame durations, loop/once, feet anchor).
//! - [`gestures`]: procedural gestures ported from `aether_animator.py`
//!   (easing/tweens become spring targets and impulses).
//! - [`talk`]: token-driven lip-flap for the `talk_loop` clip (chat replies).
//! - [`video`]: alpha-packed MP4 clip source (local ffmpeg, LRU cache, PNG fallback).
//! - [`particles`]: cyan/magenta/purple/gold/red particle bursts (TODO, M9).
//!
//! Frames are composited on the CPU with `tiny-skia` into a [`Canvas`].

pub mod animator;
pub mod atlas;
pub mod avatar;
pub mod avatar_edit;
pub mod catalog;
pub mod gestures;
pub mod particles;
pub mod player;
pub mod preview;
pub mod render;
pub mod talk;
pub mod text;
pub mod video;

/// CPU render target shared with the overlay (premultiplied RGBA).
pub type Canvas = tiny_skia::Pixmap;
