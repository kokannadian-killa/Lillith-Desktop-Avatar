//! The Knowledge panel's API (P3, docs/SELF_LEARNING.md "Knowledge panel"):
//! status, browse, search, edit, forget and recent activity for the notes
//! under `knowledge/`, plus the panel's Off switch.
//!
//! - Reads never write. Every write holds `knowledge/.lock` (the learner's
//!   single-writer lock) and is audited (`LEARN\tedit|forget\t<topic>\tnote=<id>`,
//!   never the note text).
//! - [`edit`] makes the note Ko's (`origin: ko`): the learner never merges,
//!   decays or prunes those. Text carrying the safeword is refused.
//! - [`forget`] deletes the file and its memory index row + graph node
//!   (any note; Ko asked). Nothing else in the memory DB is touched.
//! - [`set_switch`]: Off writes `<runtime dir>/learn-off`, which the
//!   running learner thread checks every tick and stops on
//!   ([`crate::runner`]); On removes it. The setting itself is
//!   `[learn] enabled` in config.toml.
//!
//! Everything is local files; no network.

use crate::store::{Note, NoteStore};
use lillith_core::config::Config;
use std::path::{Path, PathBuf};

/// The Off marker's file name under the runtime dir.
pub const OFF_MARKER: &str = "learn-off";
/// Longest title an edit may set.
pub const MAX_TITLE: usize = 200;
/// Most audit-log bytes read for [`activity`] (its tail).
const ACTIVITY_TAIL_BYTES: u64 = 256 * 1024;

/// One note as the panel lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Path relative to the knowledge folder (`rust/n-….md`).
    pub rel: String,
    pub id: String,
    pub title: String,
    pub topic: String,
    pub kind: String,
    /// `learned` | `ko`.
    pub origin: String,
    pub updated: f64,
    pub confidence: f64,
    pub body: String,
    /// Where a web / dream note came from.
    pub url: Option<String>,
}

impl Entry {
    fn of(rel: String, n: Note) -> Self {
        Self {
            rel,
            id: n.id,
            title: n.title,
            topic: n.topic,
            kind: n.kind,
            origin: n.origin,
            updated: n.updated,
            confidence: n.confidence,
            url: n.web.map(|w| w.url),
            body: n.body,
        }
    }
}

/// The panel's status line.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Status {
    /// `[learn] enabled` and not switched off at runtime.
    pub learning: bool,
    pub memory: bool,
    pub folder: Option<PathBuf>,
    pub notes: usize,
    /// Notes Ko wrote or edited (`origin: ko`).
    pub ko_notes: usize,
    /// `(topic, notes)`, topic order.
    pub by_topic: Vec<(String, usize)>,
    pub online: bool,
    pub practice: bool,
    pub share_socket: bool,
}

fn store(cfg: &Config) -> Result<NoteStore, String> {
    let dir = cfg
        .mind
        .resolved_knowledge_dir()
        .ok_or("no knowledge folder")?;
    NoteStore::open(&dir, &cfg.learn.topics, cfg.learn.allow_create_topics)
        .map_err(|e| format!("knowledge: {e}"))
}

/// Every note, newest first. Learning off or memory off: still listed
/// (Ko can always read and forget what is there).
pub fn list(cfg: &Config) -> Result<Vec<Entry>, String> {
    let dir = cfg
        .mind
        .resolved_knowledge_dir()
        .ok_or("no knowledge folder")?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut v: Vec<Entry> = store(cfg)?
        .read_all()
        .into_iter()
        .map(|(rel, n)| Entry::of(rel, n))
        .collect();
    v.sort_by(|a, b| b.updated.total_cmp(&a.updated).then(a.rel.cmp(&b.rel)));
    Ok(v)
}

/// The status for `entries` (from [`list`]).
pub fn status(cfg: &Config, entries: &[Entry]) -> Status {
    let mut by_topic: Vec<(String, usize)> = Vec::new();
    for e in entries {
        match by_topic.iter_mut().find(|(t, _)| *t == e.topic) {
            Some((_, n)) => *n += 1,
            None => by_topic.push((e.topic.clone(), 1)),
        }
    }
    by_topic.sort();
    Status {
        learning: cfg.learn.enabled && !switched_off(),
        memory: cfg.memory.enabled,
        folder: cfg.mind.resolved_knowledge_dir(),
        notes: entries.len(),
        ko_notes: entries.iter().filter(|e| e.origin == "ko").count(),
        by_topic,
        online: cfg.learn.online,
        practice: cfg.learn.practice,
        share_socket: cfg.learn.share_socket,
    }
}

/// The entries matching every word of `query` (case-insensitive, in the
/// title, topic, kind, body or source address). Empty: all.
pub fn search<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    entries
        .iter()
        .filter(|e| {
            let hay = format!(
                "{}\n{}\n{}\n{}\n{}",
                e.title,
                e.topic,
                e.kind,
                e.body,
                e.url.as_deref().unwrap_or("")
            )
            .to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect()
}

fn brain(cfg: &Config) -> Option<lillith_memory::Brain> {
    if !cfg.memory.enabled {
        return None;
    }
    lillith_memory::Brain::open(&cfg.memory)
        .map_err(|e| tracing::warn!("knowledge: memory unavailable ({e})"))
        .ok()
}

/// Ko's edit of the note at `rel`: new title and body, `origin: ko`, the
/// update time now; the memory index row follows. Audited.
pub fn edit(
    cfg: &Config,
    rel: &str,
    title: &str,
    body: &str,
    audit_log: Option<&Path>,
) -> Result<Entry, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("a note needs a title".into());
    }
    if title.chars().count() > MAX_TITLE {
        return Err(format!("titles are at most {MAX_TITLE} characters"));
    }
    if body.len() > crate::MAX_NOTE_BYTES {
        return Err(format!("notes are at most {} bytes", crate::MAX_NOTE_BYTES));
    }
    if lillith_memory::mentions_safeword(title) || lillith_memory::mentions_safeword(body) {
        return Err("text with the safeword is never stored".into());
    }
    let st = store(cfg)?;
    let mut n = st.read(rel).ok_or("no such note")?;
    let lock = match st.lock() {
        Ok(Some(l)) => l,
        Ok(None) => return Err("the learner is writing; try again in a moment".into()),
        Err(e) => return Err(format!("lock: {e}")),
    };
    n.title = title.to_string();
    n.body = body.trim_end().to_string();
    n.origin = "ko".into();
    n.updated = crate::now_s();
    let topic = rel.split('/').next().unwrap_or(&n.topic).to_string();
    n.topic = topic.clone();
    let path = st.write(&lock, &n).map_err(|e| format!("write: {e}"))?;
    let rel = st.rel(&path);
    if let Some(b) = brain(cfg) {
        let row = crate::learner::row_of(&n, &rel, &topic);
        if let Err(e) = b.upsert_note(&row, &n.body, &lillith_memory::Ctx::now()) {
            tracing::warn!("knowledge: index after edit: {e}");
        }
    }
    crate::audit::record(audit_log, "edit", &topic, &n.id);
    Ok(Entry::of(rel, n))
}

/// Forgets the note at `rel`: its file and its memory index row and graph
/// node. Returns its title. Audited (the id only).
pub fn forget(cfg: &Config, rel: &str, audit_log: Option<&Path>) -> Result<String, String> {
    let st = store(cfg)?;
    let n = st.read(rel).ok_or("no such note")?;
    let path = st.root().join(rel);
    let lock = match st.lock() {
        Ok(Some(l)) => l,
        Ok(None) => return Err("the learner is writing; try again in a moment".into()),
        Err(e) => return Err(format!("lock: {e}")),
    };
    st.remove(&lock, &path)
        .map_err(|e| format!("remove: {e}"))?;
    drop(lock);
    if let Some(b) = brain(cfg) {
        if let Err(e) = b.forget_note(&n.id) {
            tracing::warn!("knowledge: forget index row: {e}");
        }
    }
    let topic = rel.split('/').next().unwrap_or("");
    crate::audit::record(audit_log, "forget", topic, &n.id);
    Ok(n.title)
}

/// The newest `max` learning lines of the audit log (idle learner, online
/// trips, practice runs, dream sessions, helper workers), newest last.
pub fn activity(audit_log: Option<&Path>, max: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Some(p) = audit_log else {
        return Vec::new();
    };
    let Ok(mut f) = std::fs::File::open(p) else {
        return Vec::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(ACTIVITY_TAIL_BYTES)));
    let mut bytes = Vec::new();
    if f.read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let learning = |l: &str| {
        let mut cols = l.split('\t');
        let _ts = cols.next();
        let kind = cols.next().unwrap_or("");
        let what = cols.next().unwrap_or("");
        matches!(kind, "LEARN" | "DREAM" | "WORKER")
            || ["online_research", "sandbox_run", "dream_session"].contains(&what)
    };
    let mut v: Vec<String> = text
        .lines()
        .filter(|l| learning(l))
        .map(|l| l.replace('\t', "  "))
        .collect();
    let skip = v.len().saturating_sub(max);
    v.drain(..skip);
    v
}

/// `<runtime dir>/learn-off`.
pub fn off_marker_path() -> Option<PathBuf> {
    lillith_core::llm_gate::runtime_dir().map(|d| d.join(OFF_MARKER))
}

/// Whether the panel switched learning off for the running learner.
pub fn switched_off() -> bool {
    off_marker_path().is_some_and(|p| switched_off_at(&p))
}

/// [`switched_off`] for an explicit marker path.
pub fn switched_off_at(marker: &Path) -> bool {
    marker.exists()
}

/// The panel's On / Off switch for the running learner (the config value
/// is saved separately). Off: the learner thread stops on its next tick.
pub fn set_switch(on: bool) -> Result<(), String> {
    let p = off_marker_path().ok_or("no runtime folder")?;
    set_switch_at(&p, on)
}

/// [`set_switch`] for an explicit marker path.
pub fn set_switch_at(marker: &Path, on: bool) -> Result<(), String> {
    if on {
        match std::fs::remove_file(marker) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    } else {
        if let Some(d) = marker.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(marker, b"off\n").map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::WebSource;

    struct Dir(PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn setup(name: &str) -> (Dir, Config) {
        let d = std::env::temp_dir().join(format!(
            "lillith-knowledge-{name}-{}-{}",
            std::process::id(),
            crate::now_s() as u64
        ));
        std::fs::create_dir_all(&d).unwrap();
        let mut cfg = Config::default();
        cfg.mind.knowledge_dir = d.join("knowledge").to_string_lossy().into_owned();
        cfg.memory.db_path = d.join("memory.db").to_string_lossy().into_owned();
        (Dir(d), cfg)
    }

    fn note(id: &str, topic: &str, title: &str, body: &str, updated: f64) -> Note {
        Note {
            id: id.into(),
            title: title.into(),
            topic: topic.into(),
            kind: "fact".into(),
            source_type: "man".into(),
            source_ref: "man:ls".into(),
            source_hash: "fnv1a:0".into(),
            created: updated,
            updated,
            confidence: 0.6,
            quiz_asked: 0,
            quiz_passed: 0,
            links: vec![],
            origin: "learned".into(),
            body: body.into(),
            web: None,
        }
    }

    fn put(cfg: &Config, n: &Note) -> String {
        let st = store(cfg).unwrap();
        let lock = st.lock().unwrap().unwrap();
        let p = st.write(&lock, n).unwrap();
        st.rel(&p)
    }

    #[test]
    fn lists_newest_first_searches_and_counts() {
        let (_d, cfg) = setup("list");
        let topic = cfg.learn.topics[0].clone();
        put(
            &cfg,
            &note(
                "n-000000000001",
                &topic,
                "ls flags",
                "ls -l lists long.",
                10.0,
            ),
        );
        let mut web = note(
            "n-000000000002",
            &topic,
            "Traits",
            "Rust traits share behaviour.",
            20.0,
        );
        web.web = Some(WebSource {
            url: "https://doc.rust-lang.org/book/".into(),
            trip_id: "t-1".into(),
        });
        put(&cfg, &web);
        let all = list(&cfg).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].title, "Traits", "newest first");
        assert_eq!(search(&all, "").len(), 2);
        assert_eq!(search(&all, "RUST share")[0].id, "n-000000000002");
        assert_eq!(search(&all, "rust-lang").len(), 1, "source address");
        assert!(search(&all, "nothing here").is_empty());
        let st = status(&cfg, &all);
        assert_eq!((st.notes, st.ko_notes), (2, 0));
        assert_eq!(st.by_topic, vec![(topic, 2)]);
    }

    #[test]
    fn edit_makes_the_note_kos_and_refuses_the_safeword() {
        let (d, cfg) = setup("edit");
        let topic = cfg.learn.topics[0].clone();
        let rel = put(
            &cfg,
            &note("n-000000000003", &topic, "old", "old body", 5.0),
        );
        let log = d.0.join("audit.log");
        let e = edit(&cfg, &rel, "  New title ", "Fixed body.\n", Some(&log)).unwrap();
        assert_eq!((e.title.as_str(), e.origin.as_str()), ("New title", "ko"));
        assert!(e.updated > 5.0);
        let back = store(&cfg).unwrap().read(&rel).unwrap();
        assert_eq!(
            (back.origin.as_str(), back.body.as_str()),
            ("ko", "Fixed body.")
        );
        // The memory index row follows the edit.
        let b = lillith_memory::Brain::open(&cfg.memory).unwrap();
        let row = b.note("n-000000000003").unwrap();
        assert_eq!(
            (row.title.as_str(), row.origin.as_str()),
            ("New title", "ko")
        );
        let audit = std::fs::read_to_string(&log).unwrap();
        assert!(audit.contains("\tLEARN\tedit\t") && audit.contains("note=n-000000000003"));
        assert!(!audit.contains("Fixed body"), "never the note text");
        assert!(edit(&cfg, &rel, "", "x", None).is_err());
        assert!(edit(&cfg, &rel, "t", "ok... red", None).is_err());
        assert!(edit(&cfg, &rel, "OOC: stop", "b", None).is_err());
        assert!(edit(&cfg, "../escape.md", "t", "b", None).is_err());
    }

    #[test]
    fn forget_removes_the_file_and_the_index_row_only() {
        let (d, cfg) = setup("forget");
        let topic = cfg.learn.topics[0].clone();
        let keep = put(
            &cfg,
            &note("n-000000000004", &topic, "keep", "keep me", 1.0),
        );
        let go = put(
            &cfg,
            &note("n-000000000005", &topic, "go", "forget me", 2.0),
        );
        // Index both (an edit indexes).
        edit(&cfg, &keep, "keep", "keep me", None).unwrap();
        edit(&cfg, &go, "go", "forget me", None).unwrap();
        let log = d.0.join("audit.log");
        assert_eq!(forget(&cfg, &go, Some(&log)).unwrap(), "go");
        let left = list(&cfg).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, "n-000000000004");
        let b = lillith_memory::Brain::open(&cfg.memory).unwrap();
        assert!(b.note("n-000000000005").is_none());
        assert!(b.note("n-000000000004").is_some());
        assert!(std::fs::read_to_string(&log)
            .unwrap()
            .contains("\tLEARN\tforget\t"));
        assert!(forget(&cfg, &go, None).is_err(), "already gone");
    }

    #[test]
    fn activity_is_the_learning_tail_and_the_switch_toggles() {
        let (d, _cfg) = setup("activity");
        let log = d.0.join("audit.log");
        std::fs::write(
            &log,
            "2026-11-15T01:00:00Z\tLAUNCH\tllama-server\n\
             2026-11-15T01:01:00Z\tLEARN\twrite\trust\tnote=n-1\n\
             2026-11-15T01:02:00Z\tDREAM\tgoing\tx\tcard=c\n\
             2026-11-15T01:03:00Z\tCONFIRMED\tonline_research\tcard=c\n",
        )
        .unwrap();
        let a = activity(Some(&log), 2);
        assert_eq!(a.len(), 2);
        assert!(a[0].contains("DREAM") && a[1].contains("online_research"));
        assert_eq!(
            activity(Some(&log), 10).len(),
            3,
            "the launch line is not learning"
        );
        let m = d.0.join("run/learn-off");
        assert!(!switched_off_at(&m));
        set_switch_at(&m, false).unwrap();
        assert!(switched_off_at(&m));
        set_switch_at(&m, true).unwrap();
        set_switch_at(&m, true).unwrap();
        assert!(!switched_off_at(&m));
    }
}
