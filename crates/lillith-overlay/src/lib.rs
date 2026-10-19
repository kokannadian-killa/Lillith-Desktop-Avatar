//! # lillith-overlay
//!
//! The window Lillith lives in (PLAN.md §1.1, §1.3):
//!
//! - **Wayland (default on KDE Plasma 6):** a `zwlr_layer_shell_v1` surface on
//!   the `Overlay` layer via `smithay-client-toolkit`, anchored bottom+left,
//!   exclusive zone −1, keyboard interactivity `OnDemand`. Moved by updating
//!   margins. Click-through by setting the `wl_surface` input region to the
//!   current frame's alpha mask ([`input_region`]). Feature `wayland`.
//! - **X11 / XWayland fallback:** `winit` 0.30 transparent window with
//!   `WindowLevel::AlwaysOnTop` and `set_cursor_hittest()` toggled by the
//!   hit-test, drawn with `softbuffer`. Feature `x11`.
//! - Speech bubbles ([`bubble`]), drag, walk/patrol as margin animation.
//!
//! Rendering is CPU-side (`tiny-skia`) at 30 Hz; `wgpu` only if effects need it.

pub mod backend;
pub mod bubble;
pub mod input_region;

#[cfg(feature = "wayland")]
pub mod wayland;

#[cfg(feature = "x11")]
pub mod x11;

pub use backend::Backend;
