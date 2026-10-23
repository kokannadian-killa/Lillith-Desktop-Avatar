//! Windows window-stack rules for gravity perching, kept free of Win32
//! calls so they are unit-tested on every platform. The Win32 backend
//! ([`crate::win32`]) enumerates top-level windows (`EnumWindows`, top →
//! bottom), reads each one's DWM frame bounds
//! (`DWMWA_EXTENDED_FRAME_BOUNDS`) and keeps the ones [`keep`] accepts;
//! [`build`] turns them into the bottom → top
//! [`lillith_core::desktop::DeskWindow`] list the KWin bridge would send.

use lillith_core::desktop::{DeskWindow, Rect};

/// `WS_EX_TOOLWINDOW`.
pub const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
/// `WS_EX_APPWINDOW`.
pub const WS_EX_APPWINDOW: u32 = 0x0004_0000;
/// `WS_EX_NOACTIVATE`.
pub const WS_EX_NOACTIVATE: u32 = 0x0800_0000;

/// Shell surfaces that are never perch targets: the taskbar(s), the
/// desktop, Start / search / Action Center hosts, and Lillith's own windows.
pub const IGNORED_CLASSES: &[&str] = &[
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Progman",
    "WorkerW",
    "Windows.UI.Core.CoreWindow",
    "ApplicationFrameInputSinkWindow",
    "NotifyIconOverflowWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "XamlExplorerHostIslandWindow",
    LILLITH_CLASS,
    LILLITH_MENU_CLASS,
];

/// Window class of her overlay window.
pub const LILLITH_CLASS: &str = "LillithOverlay";
/// Window class of her right-click menu.
pub const LILLITH_MENU_CLASS: &str = "LillithMenu";

/// One top-level window as `EnumWindows` reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawWindow {
    /// `HWND` value (stable while the window lives).
    pub hwnd: usize,
    pub class: String,
    pub title_len: usize,
    /// `GWL_EXSTYLE`.
    pub ex_style: u32,
    /// Has an owner window (dialogs, tool palettes).
    pub owned: bool,
    /// `IsWindowVisible`.
    pub visible: bool,
    /// `IsIconic` (minimized).
    pub minimized: bool,
    /// `DWMWA_CLOAKED` (other virtual desktop, suspended UWP app).
    pub cloaked: bool,
    /// DWM frame bounds (no invisible resize borders), global px.
    pub rect: Rect,
}

/// `true` for a visible, normal app window she may stand on.
pub fn keep(w: &RawWindow, screen: Option<Rect>) -> bool {
    if !w.visible || w.minimized || w.cloaked || w.owned || w.title_len == 0 {
        return false;
    }
    if w.rect.w <= 0 || w.rect.h <= 0 {
        return false;
    }
    let app = w.ex_style & WS_EX_APPWINDOW != 0;
    if !app && w.ex_style & (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) != 0 {
        return false;
    }
    if IGNORED_CLASSES
        .iter()
        .any(|c| c.eq_ignore_ascii_case(&w.class))
        || lillith_core::desktop::ignored_class(&w.class)
    {
        return false;
    }
    // Fullscreen (games, video): covers the whole screen.
    if let Some(s) = screen {
        if w.rect.x <= s.x
            && w.rect.y <= s.y
            && w.rect.right() >= s.right()
            && w.rect.bottom() >= s.bottom()
        {
            return false;
        }
    }
    true
}

/// `EnumWindows` order (top → bottom) → kept windows, bottom → top.
pub fn build(top_to_bottom: &[RawWindow], screen: Option<Rect>) -> Vec<DeskWindow> {
    top_to_bottom
        .iter()
        .rev()
        .filter(|w| keep(w, screen))
        .map(|w| DeskWindow {
            id: format!("{:#x}", w.hwnd),
            class: w.class.clone(),
            rect: w.rect,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(hwnd: usize, class: &str, rect: Rect) -> RawWindow {
        RawWindow {
            hwnd,
            class: class.into(),
            title_len: 5,
            ex_style: 0,
            owned: false,
            visible: true,
            minimized: false,
            cloaked: false,
            rect,
        }
    }

    const SCREEN: Rect = Rect::new(0, 0, 1920, 1080);

    #[test]
    fn keeps_normal_windows_bottom_to_top() {
        let top = win(0x10, "Chrome_WidgetWin_1", Rect::new(100, 100, 800, 600));
        let below = win(0x20, "Notepad", Rect::new(300, 400, 500, 300));
        let out = build(&[top, below], Some(SCREEN));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "0x20", "bottom first");
        assert_eq!(out[1].id, "0x10");
        assert_eq!(out[1].rect, Rect::new(100, 100, 800, 600));
    }

    #[test]
    fn drops_shell_hidden_and_special_windows() {
        let r = Rect::new(10, 10, 300, 200);
        let mut v = vec![
            win(1, "Shell_TrayWnd", Rect::new(0, 1040, 1920, 40)),
            win(2, "Progman", SCREEN),
            win(3, LILLITH_CLASS, r),
            win(4, LILLITH_MENU_CLASS, r),
        ];
        let mut hidden = win(5, "A", r);
        hidden.visible = false;
        let mut min = win(6, "A", r);
        min.minimized = true;
        let mut cloaked = win(7, "A", r);
        cloaked.cloaked = true;
        let mut tool = win(8, "A", r);
        tool.ex_style = WS_EX_TOOLWINDOW;
        let mut owned = win(9, "A", r);
        owned.owned = true;
        let mut untitled = win(10, "A", r);
        untitled.title_len = 0;
        let full = win(11, "Game", Rect::new(0, 0, 1920, 1080));
        let empty = win(12, "A", Rect::new(5, 5, 0, 10));
        v.extend([hidden, min, cloaked, tool, owned, untitled, full, empty]);
        assert!(build(&v, Some(SCREEN)).is_empty());
    }

    #[test]
    fn app_window_style_wins_over_tool_window() {
        let mut w = win(1, "A", Rect::new(10, 10, 300, 200));
        w.ex_style = WS_EX_TOOLWINDOW | WS_EX_APPWINDOW;
        assert!(keep(&w, Some(SCREEN)));
    }

    #[test]
    fn maximized_is_not_fullscreen() {
        // Maximized: the work area (taskbar visible), not the whole screen.
        let w = win(1, "A", Rect::new(0, 0, 1920, 1040));
        assert!(keep(&w, Some(SCREEN)));
    }
}
