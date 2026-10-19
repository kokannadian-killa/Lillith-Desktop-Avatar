//! winit/softbuffer fallback for X11 and XWayland. TODO(M1).
//!
//! `transparent(true)`, `with_decorations(false)`,
//! `WindowLevel::AlwaysOnTop`, `set_cursor_hittest()` toggled from
//! [`crate::input_region::hit_test`] on every pointer move.

pub use softbuffer;
pub use winit;

/// Placeholder for the X11 window state.
#[derive(Debug, Default)]
pub struct X11Overlay {
    pub x: i32,
    pub y: i32,
}
