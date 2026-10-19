//! inotify watchers via `notify`. TODO(M4).
//!
//! - `~/.local/share/Trash/files` → `trash.added`
//! - `~/Pictures/Screenshots` → `screenshot.taken`
//! - `~/Downloads`, `~` (new `.zip`/`.tar*`) → `archive.created`
//! - `/sys/class/backlight/*/actual_brightness` → `brightness.changed`

use std::path::Path;

/// Maps a newly created file to an event name, if any.
pub fn classify_new_file(path: &Path) -> Option<&'static str> {
    let s = path.to_string_lossy();
    if s.contains("/.local/share/Trash/") {
        Some("trash.added")
    } else if s.contains("/Screenshots/") {
        Some("screenshot.taken")
    } else if [
        ".zip", ".tar", ".tar.gz", ".tgz", ".tar.xz", ".tar.zst", ".7z",
    ]
    .iter()
    .any(|ext| s.ends_with(ext))
    {
        Some("archive.created")
    } else {
        None
    }
}

/// Re-export so callers build watchers with the same version.
pub use notify;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies() {
        assert_eq!(
            classify_new_file(Path::new("/home/ko/.local/share/Trash/files/a.txt")),
            Some("trash.added")
        );
        assert_eq!(
            classify_new_file(Path::new("/home/ko/Downloads/x.tar.zst")),
            Some("archive.created")
        );
        assert_eq!(classify_new_file(Path::new("/home/ko/notes.md")), None);
    }
}
