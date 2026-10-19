//! # lillith-sense
//!
//! Everything that turns the state of the machine into [`Event`]s on the bus
//! (PLAN.md §2, FEATURES.md §4). Each monitor runs as a tokio task, applies
//! hysteresis + cooldown, and publishes events whose names match
//! `assets/rules.toml`.
//!
//! | Module | Events | Source |
//! |---|---|---|
//! | [`cpu`] | `cpu.high`, `ram.high`, `app.loading` | `sysinfo` |
//! | [`dbus`] | `battery.low`, `net.*`, `wifi.changed`, `vpn.up`, `hotspot.on`, `usb.*`, `notify.received`, `idle.*` | `zbus` → UPower, NetworkManager, UDisks2, Notifications, ScreenSaver |
//! | [`files`] | `trash.added`, `screenshot.taken`, `archive.created`, `brightness.changed` | `notify` (inotify) |
//! | [`kwin`] | `window.*`, `start_menu.opened`, cursor position | KWin script → `org.lillith.Bridge` |
//! | `bluetooth` (feature) | `bt.paired` | `bluer` |
//! | `pulse` (feature) | `volume.changed` | `libpulse-binding` |
//! | `journal` (feature) | `journal.error`, `app.crash`, `firewall.blocked` | `systemd` |
//! | `clipboard` (feature) | `clipboard.copied` | `wl-clipboard-rs` |
//! | `x11` (feature) | `window.*` on X11 | `x11rb` |

pub mod bus;
pub mod cpu;
pub mod dbus;
pub mod files;
pub mod kwin;

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
