//! inotify watchers via `notify` (non-recursive, so the cost is a few
//! watch descriptors):
//!
//! - `~/.local/share/Trash/files` → `trash.added`
//! - `~/Pictures/Screenshots` (Spectacle's Plasma 6 default) → `screenshot.taken`
//! - `~/Downloads` (new `.zip`/`.tar*`/`.7z`, including browser `.part` renames) → `archive.created`
//!
//! Only file *names* are looked at; contents are never read. Directories
//! that do not exist are skipped (logged once), never created. Brightness
//! is polled from sysfs by [`crate::monitor`] instead (sysfs attributes do
//! not emit inotify events).

use crate::detect::Cooldowns;
use crate::Sink;
use lillith_core::Event;
use notify::event::{CreateKind, EventKind, ModifyKind, RenameMode};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

/// Re-export so callers build watchers with the same version.
pub use notify;

/// Directories watched below `home`.
pub fn watch_dirs(home: &Path) -> Vec<PathBuf> {
    [
        ".local/share/Trash/files",
        "Pictures/Screenshots",
        "Downloads",
    ]
    .iter()
    .map(|d| home.join(d))
    .collect()
}

/// The path a filesystem event introduces, if it is a new file.
pub fn new_file_path(ev: &notify::Event) -> Option<&Path> {
    match ev.kind {
        EventKind::Create(CreateKind::File | CreateKind::Any) => ev.paths.first(),
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => ev.paths.first(),
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => ev.paths.get(1),
        _ => None,
    }
    .map(PathBuf::as_path)
}

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

/// Event for a filesystem notification (detail = the file name).
pub fn event_for(ev: &notify::Event) -> Option<Event> {
    let path = new_file_path(ev)?;
    let name = classify_new_file(path)?;
    let file = path.file_name()?.to_string_lossy().into_owned();
    Some(Event::with_detail(name, file))
}

/// Starts the watcher; dropping the returned watcher stops it. The same
/// event name is delivered at most once per `cooldown_s` (a batch of files
/// moved to the trash is one reaction).
pub fn watch(home: &Path, sink: Sink, cooldown_s: f64) -> notify::Result<RecommendedWatcher> {
    let start = Instant::now();
    let cooldowns = Mutex::new(Cooldowns::default());
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(ev) = res else { return };
        let Some(e) = event_for(&ev) else { return };
        let now = start.elapsed().as_secs_f64();
        let ready = cooldowns
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .ready(&e.name, now, cooldown_s);
        if ready {
            sink(e);
        }
    })?;
    let mut watched = 0;
    for dir in watch_dirs(home) {
        if !dir.is_dir() {
            tracing::info!(dir = %dir.display(), "not watched (missing)");
            continue;
        }
        match w.watch(&dir, RecursiveMode::NonRecursive) {
            Ok(()) => watched += 1,
            Err(e) => tracing::warn!(dir = %dir.display(), "inotify: {e}"),
        }
    }
    tracing::info!(
        dirs = watched,
        "file watcher (trash / screenshots / downloads)"
    );
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::RemoveKind;

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

    #[test]
    fn notify_events_map_to_rules() {
        let shot = notify::Event::new(EventKind::Create(CreateKind::File))
            .add_path("/home/ko/Pictures/Screenshots/Screenshot_20261019.png".into());
        assert_eq!(
            event_for(&shot),
            Some(Event::with_detail(
                "screenshot.taken",
                "Screenshot_20261019.png"
            ))
        );
        // Browser download: `.part` renamed to the final name.
        let dl = notify::Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path("/home/ko/Downloads/a.zip.part".into())
            .add_path("/home/ko/Downloads/a.zip".into());
        assert_eq!(event_for(&dl).unwrap().name, "archive.created");
        let part = notify::Event::new(EventKind::Create(CreateKind::File))
            .add_path("/home/ko/Downloads/a.zip.part".into());
        assert_eq!(event_for(&part), None, "partial download");
        let modify = notify::Event::new(EventKind::Modify(ModifyKind::Any))
            .add_path("/home/ko/Downloads/a.zip".into());
        assert_eq!(event_for(&modify), None);
        let gone = notify::Event::new(EventKind::Remove(RemoveKind::File))
            .add_path("/home/ko/.local/share/Trash/files/x".into());
        assert_eq!(event_for(&gone), None);
        assert_eq!(watch_dirs(Path::new("/h")).len(), 3);
        let rules = lillith_core::rules::RuleSet::bundled();
        for e in ["trash.added", "screenshot.taken", "archive.created"] {
            assert!(rules.find(e).is_some(), "{e}");
        }
    }

    #[test]
    fn watcher_reports_new_files_with_cooldown() {
        let home = std::env::temp_dir().join(format!("lillith-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let shots = home.join("Pictures/Screenshots");
        let trash = home.join(".local/share/Trash/files");
        std::fs::create_dir_all(&shots).unwrap();
        std::fs::create_dir_all(&trash).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let sink: Sink = std::sync::Arc::new(move |e| {
            let _ = tx.send(e);
        });
        let _w = watch(&home, sink, 60.0).unwrap();
        let wait = std::time::Duration::from_secs(5);
        std::fs::write(shots.join("s.png"), b"x").unwrap();
        assert_eq!(rx.recv_timeout(wait).unwrap().name, "screenshot.taken");
        std::fs::write(trash.join("a.txt"), b"x").unwrap();
        std::fs::write(trash.join("b.txt"), b"x").unwrap();
        assert_eq!(rx.recv_timeout(wait).unwrap().name, "trash.added");
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(300))
                .is_err(),
            "second trash file within the cooldown"
        );
        let _ = std::fs::remove_dir_all(&home);
    }
}
