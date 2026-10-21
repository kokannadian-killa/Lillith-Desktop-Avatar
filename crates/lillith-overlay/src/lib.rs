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
//!   `WindowLevel::AlwaysOnTop`, click-through via the X Shape input shape
//!   set to the same mask, drawn with `softbuffer`. Feature `x11`.
//! - [`stage`]: the M2 animator rendered into the overlay canvas + mask;
//!   [`drag`]: click/drag and `(left, bottom)` placement; [`run`](mod@run): backend
//!   dispatch, the shared [`run::Controller`] and position persistence.
//! - [`chat`]: click-to-chat (single-click gate, bubble hook, `talk_loop`
//!   lip-flap driven by streamed `chat.*` events).
//! - [`ui`]: right-click menu, Settings, minimize-to-peek and resize, as
//!   commands from the app's menu/settings windows.
//! - Speech bubbles ([`bubble`], M3), walk/patrol as margin animation (M3).
//!
//! Rendering is CPU-side (`tiny-skia`) at 30 Hz; `wgpu` only if effects need it.

pub mod backend;
pub mod bubble;
pub mod chat;
pub mod drag;
pub mod input_region;
pub mod menu_popup;
pub mod pixels;
pub mod run;
pub mod settings_window;
pub mod speech;
pub mod stage;
pub mod ui;
pub mod wl_geom;

#[cfg(feature = "wayland")]
pub mod wayland;

#[cfg(feature = "x11")]
pub mod x11;

pub use backend::Backend;
pub use chat::{ChatAnchor, ChatHook};
pub use run::{run, ActionSink, EventFeed, OverlayError, OverlayOptions, SinkHandle};
