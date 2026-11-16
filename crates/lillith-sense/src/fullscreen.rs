//! Fullscreen ("busy") detection shared by the window sources: the KWin
//! bridge on Linux ([`crate::kwin`]) and the foreground-window poller on
//! Windows ([`crate::win_focus`]).
//!
//! [`FullscreenTracker`] turns "the focused window is / is not fullscreen"
//! observations into transitions: [`FULLSCREEN_EVENT`] when Ko's focused
//! window goes fullscreen (detail: the window class / exe id only, never a
//! caption) and [`FULLSCREEN_LEFT_EVENT`] when that ends (focus moves to a
//! normal window, the window leaves fullscreen or closes). The app hands
//! both to the memory bridge's busy hook (`MemoryBridge::set_busy_for`), so
//! proactive lines stay quiet while he games, watches or presents. The
//! memory filter drops both events: nothing about them is learned.

use lillith_core::Event;

/// The focused window went fullscreen (detail: class / exe id).
pub const FULLSCREEN_EVENT: &str = "window.fullscreen";
/// The fullscreen window lost focus, left fullscreen or closed.
pub const FULLSCREEN_LEFT_EVENT: &str = "window.fullscreen_left";

/// Which window (if any) is the focused fullscreen one.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FullscreenTracker {
    /// The focused fullscreen window's id.
    active: Option<String>,
}

impl FullscreenTracker {
    /// The id of the focused fullscreen window, if any.
    pub fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    /// Window `id` (class `class`) has focus and is (`on`) / is not
    /// fullscreen. Emits only on a change.
    pub fn focused(&mut self, id: &str, class: &str, on: bool) -> Option<Event> {
        if on && !id.is_empty() {
            if self.active.as_deref() == Some(id) {
                return None;
            }
            self.active = Some(id.to_string());
            Some(Event::with_detail(FULLSCREEN_EVENT, class))
        } else {
            self.clear()
        }
    }

    /// Window `id` went away (closed / minimized).
    pub fn gone(&mut self, id: &str) -> Option<Event> {
        if self.active.as_deref() == Some(id) {
            self.clear()
        } else {
            None
        }
    }

    /// No focused fullscreen window any more.
    pub fn clear(&mut self) -> Option<Event> {
        self.active
            .take()
            .map(|_| Event::new(FULLSCREEN_LEFT_EVENT))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[Option<Event>]) -> Vec<&str> {
        v.iter().flatten().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn transitions_only() {
        let mut t = FullscreenTracker::default();
        let got = [
            t.focused("a", "konsole", false),
            t.focused("g", "steam_app_1", true),
            t.focused("g", "steam_app_1", true),
            t.focused("v", "mpv", true),
            t.focused("a", "konsole", false),
            t.focused("a", "konsole", false),
        ];
        assert_eq!(
            names(&got),
            [FULLSCREEN_EVENT, FULLSCREEN_EVENT, FULLSCREEN_LEFT_EVENT]
        );
        assert_eq!(
            got[1].as_ref().unwrap().detail.as_deref(),
            Some("steam_app_1")
        );
        assert_eq!(t.active(), None);
    }

    #[test]
    fn closing_the_fullscreen_window_ends_it_and_others_do_not() {
        let mut t = FullscreenTracker::default();
        assert!(t.focused("g", "game", true).is_some());
        assert!(t.gone("other").is_none());
        assert_eq!(t.active(), Some("g"));
        assert_eq!(t.gone("g").unwrap().name, FULLSCREEN_LEFT_EVENT);
        assert!(t.clear().is_none());
        // No id: never "fullscreen".
        assert!(t.focused("", "x", true).is_none());
    }
}
