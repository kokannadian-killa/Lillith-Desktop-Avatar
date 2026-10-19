//! KWin bridge receiver. TODO(M5).
//!
//! `kwin/lillith-bridge` (a KWin script) calls `callDBus` on
//! [`BUS_NAME`] / [`OBJECT_PATH`] for window added/removed/activated/
//! minimized, frame geometry and polled cursor position. This module will
//! serve that interface with `zbus` and publish `window.*` events.

/// D-Bus name owned by Lillith for the KWin script to call.
pub const BUS_NAME: &str = "org.lillith.Bridge";
/// Object path served.
pub const OBJECT_PATH: &str = "/org/lillith/Bridge";

/// Window categories (copied from `aether_gesture_map.py` app lists). TODO: full lists.
pub fn categorize(resource_class: &str) -> &'static str {
    let c = resource_class.to_ascii_lowercase();
    if ["dolphin", "nautilus", "thunar", "nemo"]
        .iter()
        .any(|k| c.contains(k))
    {
        "folder"
    } else if [
        "firefox",
        "chromium",
        "chrome",
        "brave",
        "vivaldi",
        "librewolf",
    ]
    .iter()
    .any(|k| c.contains(k))
    {
        "browser"
    } else if [
        "konsole",
        "alacritty",
        "kitty",
        "foot",
        "wezterm",
        "terminal",
    ]
    .iter()
    .any(|k| c.contains(k))
    {
        "terminal"
    } else if ["vlc", "mpv", "spotify", "elisa", "haruna"]
        .iter()
        .any(|k| c.contains(k))
    {
        "media"
    } else {
        "app"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_have_rules() {
        let rules = lillith_core::rules::RuleSet::bundled();
        for class in ["dolphin", "firefox", "konsole", "mpv", "kate"] {
            let ev = format!("window.opened.{}", categorize(class));
            assert!(rules.find(&ev).is_some(), "no rule for {ev}");
        }
    }
}
