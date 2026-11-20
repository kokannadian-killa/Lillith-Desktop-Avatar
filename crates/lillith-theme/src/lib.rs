//! # lillith-theme
//!
//! The shared look of Lillith's chrome (settings window, right-click menu):
//! an *ancient demon relic* (obsidian, black iron, carved runes, skull and
//! horn ornaments, claw grips, cracked stone with ember glow) crossed with
//! *DemonOS* (blood-red translucent terminal panels on a red grid, glitchy
//! labels, electric-blue neon sigil, violet accents, blood drips, a knife
//! blade on the hovered / selected item).
//!
//! Layers, so a non-egui renderer (the tiny-skia overlay popup) can reuse
//! everything but the egui glue:
//!
//! * [`palette`] — colour constants as plain RGBA (`[u8; 4]`-like).
//! * [`geom`] — toolkit-free shapes in `f32` coordinates: glow layers,
//!   blood drips, blade outline, rune strokes, skull parts, horns, the
//!   horns-and-spade-tail sigil, claw talons, stone cracks.
//! * [`accent`] — the avatar-matched accent (Lillith magenta by default,
//!   from `avatar.json` or derived from the avatar image).
//! * [`fonts`] — bundled OFL fonts as `&'static [u8]` (licenses in `fonts/`).
//! * `egui_theme` (feature `egui`) — `install(ctx)` for fonts + Visuals,
//!   and Painter helpers built on [`geom`].
//! * `egui_awakening` / `egui_blood_seal` (feature `egui`) — QuantumSec
//!   early-usage popup and the blood-seal consent gate (Soul Contract).
//! * `bubble` (feature `egui`) — the chat bubble's plum-glass look
//!   (colours, Visuals, frame), used by `lillith-panel`.
//!
//! The character art is locked canon and is never drawn here: the sigil
//! is an abstract horns + spade-tail emblem.

pub mod accent;
pub mod fonts;
pub mod geom;
pub mod palette;

#[cfg(feature = "egui")]
pub mod bubble;
#[cfg(feature = "egui")]
pub mod egui_awakening;
#[cfg(feature = "egui")]
pub mod egui_blood_seal;
#[cfg(feature = "egui")]
pub mod egui_contract;
#[cfg(feature = "egui")]
pub mod egui_deletion;
#[cfg(feature = "egui")]
pub mod egui_plan;
#[cfg(feature = "egui")]
pub mod egui_theme;
#[cfg(feature = "egui")]
pub mod egui_whisper;

pub use accent::Accent;
pub use palette::Rgba;
