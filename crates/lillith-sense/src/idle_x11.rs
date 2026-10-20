//! X11 desktop idle via the MIT-SCREEN-SAVER extension (`XScreenSaverQueryInfo`).
//! Used only in X11 sessions; under Wayland, XWayland would only count input
//! that went to X clients.

use crate::presence::IdleSource;
use x11rb::connection::Connection;
use x11rb::protocol::screensaver::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

/// MIT-SCREEN-SAVER idle source.
pub struct X11Idle {
    conn: RustConnection,
    root: u32,
}

impl std::fmt::Debug for X11Idle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("X11Idle").field("root", &self.root).finish()
    }
}

impl X11Idle {
    pub fn connect() -> Result<Self, String> {
        let (conn, screen) = RustConnection::connect(None).map_err(|e| e.to_string())?;
        let root = conn.setup().roots[screen].root;
        // Fails if the server lacks the extension.
        conn.screensaver_query_version(1, 1)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| format!("MIT-SCREEN-SAVER unavailable: {e}"))?;
        tracing::info!("desktop idle: X11 MIT-SCREEN-SAVER");
        Ok(Self { conn, root })
    }
}

impl IdleSource for X11Idle {
    fn idle_for(&mut self) -> Option<f64> {
        let reply = self
            .conn
            .screensaver_query_info(self.root)
            .ok()?
            .reply()
            .ok()?;
        Some(reply.ms_since_user_input as f64 / 1000.0)
    }

    fn name(&self) -> &'static str {
        "MIT-SCREEN-SAVER"
    }
}
