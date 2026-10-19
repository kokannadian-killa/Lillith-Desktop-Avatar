//! Backend selection: `--backend x11` or no `WAYLAND_DISPLAY` → X11 fallback.

use std::str::FromStr;

/// Which windowing backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// wlr-layer-shell overlay (KWin implements it).
    Wayland,
    /// winit/X11 (or XWayland) fallback.
    X11,
}

impl FromStr for Backend {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "wayland" => Ok(Self::Wayland),
            "x11" | "xwayland" => Ok(Self::X11),
            other => Err(format!("unknown backend {other} (expected wayland|x11)")),
        }
    }
}

impl Backend {
    /// Picks a backend from an explicit flag, else from `WAYLAND_DISPLAY`.
    pub fn detect(forced: Option<Backend>, wayland_display: Option<&str>) -> Backend {
        match forced {
            Some(b) => b,
            None if wayland_display.is_some_and(|d| !d.is_empty()) => Backend::Wayland,
            None => Backend::X11,
        }
    }

    /// Picks a backend using the real environment.
    pub fn detect_env(forced: Option<Backend>) -> Backend {
        let wd = std::env::var("WAYLAND_DISPLAY").ok();
        Self::detect(forced, wd.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection() {
        assert_eq!(Backend::detect(None, Some("wayland-0")), Backend::Wayland);
        assert_eq!(Backend::detect(None, None), Backend::X11);
        assert_eq!(
            Backend::detect(Some(Backend::X11), Some("wayland-0")),
            Backend::X11
        );
        assert_eq!("X11".parse::<Backend>(), Ok(Backend::X11));
    }
}
