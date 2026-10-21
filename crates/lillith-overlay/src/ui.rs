//! The overlay-side commands behind the right-click menu.
//!
//! The menu itself is the overlay's own popup ([`crate::menu_popup`]) over
//! the [`lillith_core::menu`] model; its picks become a [`Command`] here
//! (play a pose, show off, chat, minimize to peek, resize, avatar, open
//! the `lillith-settings` window). These event lines on the
//! [`crate::EventFeed`] map to commands as well:
//!
//! | event              | detail              | command                     |
//! |--------------------|---------------------|-----------------------------|
//! | `settings.changed` |                     | reload config.toml live     |
//! | `avatar.selected`  | id / `lillith`      | switch avatar               |

/// Sprite scale of the minimized "peek" icon.
pub const PEEK_SCALE: f32 = 0.2;
/// Smallest / largest live scale (the menu's Resize range).
pub const SCALE_RANGE: (f32, f32) = lillith_core::menu::SCALE_RANGE;

/// An overlay command (from a menu pick or the Settings window).
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Pose(String),
    ShowOff,
    Chat,
    Minimize,
    Scale {
        scale: f32,
        save: bool,
    },
    /// Open the `lillith-settings` window ([`crate::settings_window`]).
    Settings,
    /// `None` = Lillith.
    Avatar(Option<String>),
    ConfigChanged,
    Quit,
}

impl Command {
    /// Parses a `settings.*` / `avatar.*` event from the Settings window.
    pub fn from_event(name: &str, detail: Option<&str>) -> Option<Self> {
        let avatar = || match detail.map(str::trim) {
            None | Some("") | Some("lillith") => None,
            Some(id) => Some(id.to_string()),
        };
        Some(match name {
            "avatar.selected" => Command::Avatar(avatar()),
            "settings.changed" => Command::ConfigChanged,
            _ => return None,
        })
    }
}

/// `true` for events this module handles.
pub fn is_ui_event(name: &str) -> bool {
    name.starts_with("settings.") || name.starts_with("avatar.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_settings_events() {
        assert_eq!(
            Command::from_event("avatar.selected", Some("lillith")),
            Some(Command::Avatar(None))
        );
        assert_eq!(
            Command::from_event("avatar.selected", Some("cat")),
            Some(Command::Avatar(Some("cat".into())))
        );
        assert_eq!(
            Command::from_event("settings.changed", None),
            Some(Command::ConfigChanged)
        );
        assert_eq!(Command::from_event("settings.nope", None), None);
        assert!(is_ui_event("settings.changed") && !is_ui_event("chat.token"));
        assert!(!is_ui_event("menu.pose"));
    }
}
