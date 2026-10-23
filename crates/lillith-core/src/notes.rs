//! One-time notes for Ko, queued by the app (e.g. the KWin pre-start check)
//! and shown the next time a chat opens (bubble or `lillith chat`).
//!
//! Stored as `<data>/chat-notes.txt`, one note per paragraph (blank-line
//! separated). [`take`] reads and removes the file, so each note is shown
//! once.

use std::io::Write;
use std::path::Path;

/// File name inside the data folder.
pub const FILE: &str = "chat-notes.txt";

/// Queues `text` (one paragraph) for the next chat.
pub fn push(data_dir: &Path, text: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data_dir.join(FILE))?;
    writeln!(f, "{}\n", text.trim().replace("\n\n", "\n"))
}

/// Takes every queued note (oldest first) and clears the queue.
pub fn take(data_dir: &Path) -> Vec<String> {
    let path = data_dir.join(FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let _ = std::fs::remove_file(&path);
    text.split("\n\n")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_are_shown_once_in_order() {
        let d = std::env::temp_dir().join(format!("lillith-notes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        assert!(take(&d).is_empty());
        push(&d, "first\nline two").unwrap();
        push(&d, "second").unwrap();
        assert_eq!(take(&d), vec!["first\nline two", "second"]);
        assert!(take(&d).is_empty(), "taken once");
        let _ = std::fs::remove_dir_all(&d);
    }
}
