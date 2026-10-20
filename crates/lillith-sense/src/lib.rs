//! # lillith-sense
//!
//! Everything that turns the state of the machine into [`Event`]s for the
//! brain (PLAN.md §2, FEATURES.md §4). Event names match `assets/rules.toml`.
//!
//! M4 entry point: [`service::spawn`] starts the monitors configured by
//! `[monitor]` in config.toml and hands every event to a [`Sink`].
//!
//! | Module | Events | Source | Status |
//! |---|---|---|---|
//! | [`monitor`] (+ [`procfs`], [`sysfs`], [`detect`]) | `cpu.high`, `ram.high`, `battery.low`, `net.lost`, `net.restored`, `vpn.up`, `brightness.changed`, `usb.added`, `usb.removed` | `/proc/stat`, `/proc/meminfo`, `/proc/net/route`, `/sys/class/{power_supply,net,backlight}`, `/sys/block` polling | M4 ✔ |
//! | [`presence`] (+ `idle_wayland`, `idle_x11`) | `user.activity`, `user.returned` | `ext-idle-notify-v1` (feature `wayland-idle`) / MIT-SCREEN-SAVER (feature `x11`) | M4 ✔ |
//! | [`notifications`] | `notify.received`, `calendar.reminder`, `update.available` | session bus `BecomeMonitor` on `org.freedesktop.Notifications.Notify` | M4 ✔ |
//! | [`files`] | `trash.added`, `screenshot.taken`, `archive.created` | `notify` (inotify) | M4 ✔ |
//! | [`kwin`] | `window.*`, `start_menu.opened`, cursor position | KWin script → `org.lillith.Bridge` | TODO(M5), hook only |
//! | [`dbus`] | `wifi.changed`, `hotspot.on` | NetworkManager | later |
//! | `bluetooth` (feature) | `bt.paired` | `bluer` | later |
//! | `pulse` (feature) | `volume.changed` | `libpulse-binding` | later |
//! | `journal` (feature) | `journal.error`, `app.crash`, `firewall.blocked` | `systemd` | later |
//! | `clipboard` (feature) | `clipboard.copied` | `wl-clipboard-rs` | later |
//! | `x11` (feature) | desktop idle on X11 (window list later) | `x11rb` | M4 ✔ (idle) |

pub mod bus;
pub mod dbus;
pub mod detect;
pub mod files;
pub mod kwin;
pub mod monitor;
pub mod notifications;
pub mod presence;
pub mod procfs;
pub mod service;
pub mod sysfs;

#[cfg(feature = "wayland-idle")]
pub mod idle_wayland;
#[cfg(feature = "x11")]
pub mod idle_x11;

#[cfg(feature = "bluetooth")]
pub mod bluetooth;
#[cfg(feature = "clipboard")]
pub mod clipboard;
#[cfg(feature = "journal")]
pub mod journal;
#[cfg(feature = "pulse")]
pub mod pulse;
#[cfg(feature = "x11")]
pub mod x11;

pub use bus::EventBus;
pub use lillith_core::Event;
pub use monitor::{Monitor, Sample};
pub use service::{spawn, MonitorHandle};

/// Where monitors deliver events (the app forwards them to the overlay's
/// brain; tests collect them). Must be cheap and non-blocking.
pub type Sink = std::sync::Arc<dyn Fn(Event) + Send + Sync>;
