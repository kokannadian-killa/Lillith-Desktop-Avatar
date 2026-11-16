//! Windows window-focus source (the KWin bridge's job on Linux): polls the
//! foreground window about once a second and emits
//!
//! - `window.focused` with the app's exe id only (`firefox`, `steam`: the
//!   file stem, lowercase; never the caption), when focus moves to another
//!   app's window, for the memory bridge's app habits (its own
//!   `window.focused` debounce applies, the same as on KDE);
//! - [`crate::fullscreen::FULLSCREEN_EVENT`] /
//!   [`crate::fullscreen::FULLSCREEN_LEFT_EVENT`] when the focused window
//!   covers its whole monitor (games, video, slides) or stops doing so.
//!
//! [`WinFocus`] is the pure state machine (tested on every platform); the
//! Win32 sampler and the poll task are `cfg(windows)`. Shell surfaces
//! (taskbar, desktop, Start) and her own windows are ignored. No window
//! titles, no hooks into other processes, read-only queries only.

use crate::fullscreen::FullscreenTracker;
use lillith_core::Event;

/// One look at the foreground window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Foreground {
    /// `HWND` value.
    pub hwnd: usize,
    /// Window class (`GetClassNameW`).
    pub class: String,
    /// The owning process's exe file name (`firefox.exe`), when readable.
    pub exe: Option<String>,
    /// The window covers its whole monitor.
    pub covers_monitor: bool,
}

/// Classes that are the shell or Lillith herself, never "an app".
const SHELL_CLASSES: &[&str] = &[
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Progman",
    "WorkerW",
    "Windows.UI.Core.CoreWindow",
    "NotifyIconOverflowWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "XamlExplorerHostIslandWindow",
    "LillithOverlay",
    "LillithMenu",
];

/// The app id for an exe file name: the lowercase file stem (`Steam.exe`
/// → `steam`), ASCII-safe, at most 48 chars; `None` for Lillith herself
/// or nothing usable.
pub fn app_id(exe: &str) -> Option<String> {
    let name = exe.rsplit(['\\', '/']).next().unwrap_or(exe);
    let stem = name
        .strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".EXE"))
        .unwrap_or(name);
    let id: String = stem
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .take(48)
        .collect::<String>()
        .to_ascii_lowercase();
    (!id.is_empty() && id != "lillith").then_some(id)
}

/// The pure foreground-window mapper.
#[derive(Debug, Default, Clone)]
pub struct WinFocus {
    last_hwnd: Option<usize>,
    last_app: Option<String>,
    fullscreen: FullscreenTracker,
}

impl WinFocus {
    /// One poll: `None` when there is no foreground window.
    pub fn step(&mut self, fg: Option<&Foreground>) -> Vec<Event> {
        let mut out = Vec::new();
        let Some(fg) = fg else {
            out.extend(self.fullscreen.clear());
            self.last_hwnd = None;
            return out;
        };
        let shell = SHELL_CLASSES
            .iter()
            .any(|c| c.eq_ignore_ascii_case(&fg.class));
        let app = if shell {
            None
        } else {
            fg.exe.as_deref().and_then(app_id)
        };
        let id = format!("{:#x}", fg.hwnd);
        match &app {
            // The desktop "covers" the monitor too: shell and unknown
            // windows are never fullscreen apps.
            Some(a) => out.extend(self.fullscreen.focused(&id, a, fg.covers_monitor)),
            None => out.extend(self.fullscreen.clear()),
        }
        if self.last_hwnd != Some(fg.hwnd) {
            self.last_hwnd = Some(fg.hwnd);
            if let Some(a) = app {
                if self.last_app.as_deref() != Some(a.as_str()) {
                    out.push(Event::with_detail("window.focused", a.clone()));
                    self.last_app = Some(a);
                }
            }
        }
        out
    }
}

#[cfg(windows)]
mod win32 {
    use super::Foreground;
    use windows_sys::Win32::Foundation::{CloseHandle, HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId,
    };

    /// Reads the foreground window (read-only Win32 queries).
    pub fn sample() -> Option<Foreground> {
        // SAFETY: read-only queries on the foreground window handle and a
        // process handle this function opens and closes; a window that
        // vanished meanwhile just makes them fail.
        unsafe {
            let h: HWND = GetForegroundWindow();
            if h.is_null() {
                return None;
            }
            let mut buf = [0u16; 128];
            let n = GetClassNameW(h, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
            let class = String::from_utf16_lossy(&buf[..n]);
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, &mut pid);
            let mut exe = None;
            if pid != 0 {
                let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                if !p.is_null() {
                    let mut path = [0u16; 512];
                    let mut len = path.len() as u32;
                    if QueryFullProcessImageNameW(
                        p,
                        PROCESS_NAME_WIN32,
                        path.as_mut_ptr(),
                        &mut len,
                    ) != 0
                    {
                        exe = Some(String::from_utf16_lossy(&path[..len as usize]));
                    }
                    CloseHandle(p);
                }
            }
            let mut r: RECT = std::mem::zeroed();
            let mut covers = false;
            if GetWindowRect(h, &mut r) != 0 {
                let m = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
                let mut mi: MONITORINFO = std::mem::zeroed();
                mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
                if !m.is_null() && GetMonitorInfoW(m, &mut mi) != 0 {
                    let s = mi.rcMonitor;
                    covers = r.left <= s.left
                        && r.top <= s.top
                        && r.right >= s.right
                        && r.bottom >= s.bottom;
                }
            }
            Some(Foreground {
                hwnd: h as usize,
                class,
                exe,
                covers_monitor: covers,
            })
        }
    }
}

/// Polls the foreground window about once a second until the task is
/// dropped.
#[cfg(windows)]
pub async fn run(sink: crate::Sink) {
    let mut f = WinFocus::default();
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        tick.tick().await;
        let fg = win32::sample();
        for ev in f.step(fg.as_ref()) {
            sink(ev);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fullscreen::{FULLSCREEN_EVENT, FULLSCREEN_LEFT_EVENT};

    fn fg(hwnd: usize, class: &str, exe: &str, full: bool) -> Foreground {
        Foreground {
            hwnd,
            class: class.into(),
            exe: Some(format!("C:\\Program Files\\X\\{exe}")),
            covers_monitor: full,
        }
    }

    fn names(v: &[Event]) -> Vec<(String, Option<String>)> {
        v.iter()
            .map(|e| (e.name.clone(), e.detail.clone()))
            .collect()
    }

    #[test]
    fn app_ids_are_exe_stems_only() {
        assert_eq!(
            app_id("C:\\Games\\Steam\\Steam.exe").as_deref(),
            Some("steam")
        );
        assert_eq!(app_id("firefox.exe").as_deref(), Some("firefox"));
        assert_eq!(app_id("C:\\x\\Lillith.exe"), None);
        assert_eq!(app_id(""), None);
    }

    #[test]
    fn focus_changes_emit_the_app_id_once_and_never_a_caption() {
        let mut w = WinFocus::default();
        let ff = fg(1, "MozillaWindowClass", "firefox.exe", false);
        let e = w.step(Some(&ff));
        assert_eq!(
            names(&e),
            [("window.focused".to_string(), Some("firefox".to_string()))]
        );
        assert!(w.step(Some(&ff)).is_empty(), "same window: nothing");
        // Another firefox window: still the same app, nothing.
        assert!(w
            .step(Some(&fg(2, "MozillaWindowClass", "firefox.exe", false)))
            .is_empty());
        // The taskbar / her own window: ignored.
        assert!(w
            .step(Some(&fg(3, "Shell_TrayWnd", "explorer.exe", false)))
            .is_empty());
        assert!(w
            .step(Some(&fg(4, "LillithOverlay", "lillith.exe", false)))
            .is_empty());
        let e = w.step(Some(&fg(5, "Notepad", "notepad.exe", false)));
        assert_eq!(e[0].detail.as_deref(), Some("notepad"));
    }

    #[test]
    fn a_fullscreen_foreground_app_is_busy_until_it_is_not() {
        let mut w = WinFocus::default();
        let game = fg(9, "UnityWndClass", "game.exe", true);
        let e = w.step(Some(&game));
        assert_eq!(e[0].name, FULLSCREEN_EVENT);
        assert_eq!(e[0].detail.as_deref(), Some("game"));
        assert_eq!(e[1].name, "window.focused");
        assert!(w.step(Some(&game)).is_empty());
        // Windowed again.
        let e = w.step(Some(&fg(9, "UnityWndClass", "game.exe", false)));
        assert_eq!(names(&e), [(FULLSCREEN_LEFT_EVENT.to_string(), None)]);
        // The desktop covers the monitor but is not a fullscreen app.
        assert!(w
            .step(Some(&fg(10, "Progman", "explorer.exe", true)))
            .iter()
            .all(|e| e.name != FULLSCREEN_EVENT));
        // Fullscreen, then no foreground window (locked): it ends.
        w.step(Some(&game));
        assert_eq!(w.step(None)[0].name, FULLSCREEN_LEFT_EVENT);
    }
}
