//! "A system interaction is open": Soul Contract / deletion cards, the
//! settings window and its setup wizard, file dialogs, the console,
//! installs. While one is open Lillith makes no attention bids and draws
//! no lonely poses ([`crate::loneliness`]), and the time doesn't count as
//! attention either.
//!
//! Two ways to say so, both RAII ([`SystemUi`]; dropping it ends it):
//!
//! - in-process: a counter (cards spawned by this process, dialogs);
//! - across processes ([`SystemUi::enter_shared`]): an empty marker file
//!   `<runtime>/lillith/system-ui/<pid>-<tag>`, removed on drop. Readers
//!   ignore markers whose process is gone (a crash leaves no stuck state).
//!
//! [`active`] is cheap (one small directory listing); the overlay polls it
//! about once a second.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static OPEN: AtomicUsize = AtomicUsize::new(0);

/// Marker directory: `$XDG_RUNTIME_DIR/lillith/system-ui`, else the temp
/// dir. `LILLITH_SYSTEM_UI_DIR` overrides it (tests).
pub fn marker_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("LILLITH_SYSTEM_UI_DIR") {
        return PathBuf::from(d);
    }
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(std::env::temp_dir)
        .join("lillith")
        .join("system-ui")
}

/// A system interaction held open until dropped.
#[derive(Debug)]
pub struct SystemUi {
    marker: Option<PathBuf>,
}

impl SystemUi {
    /// In this process only.
    pub fn enter() -> Self {
        OPEN.fetch_add(1, Ordering::SeqCst);
        Self { marker: None }
    }

    /// Visible to other Lillith processes too (the overlay). `tag` names
    /// it (`settings`, `card`, …; only `[a-z0-9_-]` kept). If the marker
    /// can't be written it still counts in-process.
    pub fn enter_shared(tag: &str) -> Self {
        let mut g = Self::enter();
        let tag: String = tag
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .take(32)
            .collect();
        let dir = marker_dir();
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let path = dir.join(format!("{}-{n}-{tag}", std::process::id()));
        if std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&path, b"").is_ok() {
            g.marker = Some(path);
        }
        g
    }
}

static NEXT: AtomicUsize = AtomicUsize::new(0);

impl Drop for SystemUi {
    fn drop(&mut self) {
        OPEN.fetch_sub(1, Ordering::SeqCst);
        if let Some(p) = self.marker.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Open in this process.
pub fn active_here() -> bool {
    OPEN.load(Ordering::SeqCst) > 0
}

fn pid_alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        // No cheap check: trust the marker (it is removed on drop).
        let _ = pid;
        true
    }
}

/// Any system interaction open, here or in another Lillith process.
pub fn active() -> bool {
    active_here() || shared_active(&marker_dir())
}

/// Live markers in `dir` (stale ones of dead processes are ignored).
pub fn shared_active(dir: &std::path::Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    rd.flatten().any(|e| {
        e.file_name()
            .to_str()
            .and_then(|n| n.split('-').next())
            .and_then(|p| p.parse::<u32>().ok())
            .is_some_and(pid_alive)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guards_count_and_markers_clear() {
        let dir = std::env::temp_dir().join(format!("lillith-sysui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!shared_active(&dir));
        // A stale marker of a process that is gone is ignored.
        std::fs::write(dir.join("4000000000-0-card"), b"").unwrap();
        #[cfg(target_os = "linux")]
        assert!(!shared_active(&dir));
        std::fs::remove_file(dir.join("4000000000-0-card")).unwrap();
        // Our own marker counts while held.
        let mine = dir.join(format!("{}-x-settings", std::process::id()));
        std::fs::write(&mine, b"").unwrap();
        assert!(shared_active(&dir));
        std::fs::remove_file(&mine).unwrap();
        assert!(!shared_active(&dir));
        {
            let _g = SystemUi::enter();
            assert!(active_here());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
