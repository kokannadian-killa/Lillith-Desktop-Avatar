//! Layer-shell overlay on KDE Plasma 6 Wayland. TODO(M1).
//!
//! Plan:
//! 1. Connect with `wayland_client::Connection::connect_to_env()`.
//! 2. Bind compositor, `wl_shm`, `zwlr_layer_shell_v1`, seat via sctk.
//! 3. Create a layer surface: `Layer::Overlay`, anchor `BOTTOM | LEFT`,
//!    exclusive zone −1, `KeyboardInteractivity::OnDemand`.
//! 4. Draw the `tiny-skia` canvas into an `wl_shm` slot pool at 30 Hz.
//! 5. On each frame change set the input region from the alpha runs.
//! 6. Pointer grab + margin updates for drag; walk animates margins.

pub use smithay_client_toolkit as sctk;
pub use wayland_client;

/// Placeholder for the layer-shell window state.
#[derive(Debug, Default)]
pub struct LayerOverlay {
    pub margin_left: i32,
    pub margin_bottom: i32,
}
