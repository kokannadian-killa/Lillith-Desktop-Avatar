//! D-Bus monitors via `zbus`. TODO(M4).
//!
//! - UPower `org.freedesktop.UPower` → `battery.low` (≤ `battery_low_percent`)
//! - NetworkManager `org.freedesktop.NetworkManager` → `net.lost`,
//!   `net.restored`, `wifi.changed`, `vpn.up`, `hotspot.on`
//! - UDisks2 `org.freedesktop.UDisks2` → `usb.added`, `usb.removed`
//! - Notifications monitor (`org.freedesktop.Notifications.Notify`, session
//!   bus, `BecomeMonitor`) → `notify.received`, `calendar.reminder`
//! - `org.freedesktop.ScreenSaver` / ext-idle-notify → `idle.long`,
//!   `idle.asleep`, `user.returned`

/// Well-known bus names we watch.
pub const UPOWER: &str = "org.freedesktop.UPower";
pub const NETWORK_MANAGER: &str = "org.freedesktop.NetworkManager";
pub const UDISKS2: &str = "org.freedesktop.UDisks2";
pub const NOTIFICATIONS: &str = "org.freedesktop.Notifications";
pub const SCREENSAVER: &str = "org.freedesktop.ScreenSaver";

/// Connects to the system bus (UPower, NetworkManager, UDisks2).
pub async fn system_bus() -> zbus::Result<zbus::Connection> {
    zbus::Connection::system().await
}

/// Connects to the session bus (Notifications, ScreenSaver, KWin bridge).
pub async fn session_bus() -> zbus::Result<zbus::Connection> {
    zbus::Connection::session().await
}
