//! The note files (docs/SELF_LEARNING.md §2): markdown with a small YAML
//! front matter under `knowledge/<topic>/<id>.md`.
//!
//! - folders are created 0700 (`DirBuilder::mode`), files 0600
//!   (`OpenOptions::mode`, set before any byte is written);
//! - every write is atomic (`*.tmp`, then rename) and happens under the
//!   single-writer lock `knowledge/.lock`;
//! - a note is at most [`MAX_NOTE_BYTES`](crate::MAX_NOTE_BYTES);
//! - topics come from `[learn] topics`; new ones only with
//!   `allow_create_topics` and a slug `[a-z0-9-]{1,32}`; the rest go to
//!   `inbox/`;
//! - nothing is ever written outside the knowledge folder, and never a
//!   config / persona / modelfile / audit file.

use crate::audit::utc;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Where unsorted notes go.
pub const INBOX: &str = "inbox";

/// Note kinds the learner writes.
pub const KINDS: &[&str] = &[
    "fact",
    "howto",
    "snippet",
    "preference",
    "fix",
    "skill",
    "web",
];

/// One note.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub id: String,
    pub title: String,
    pub topic: String,
    pub kind: String,
    pub source_type: String,
    pub source_ref: String,
    pub source_hash: String,
    pub created: f64,
    pub updated: f64,
    pub confidence: f64,
    pub quiz_asked: u32,
    pub quiz_passed: u32,
    pub links: Vec<String>,
    /// `learned` | `ko`.
    pub origin: String,
    pub body: String,
    /// Where a `web` note came from (§2: `url`, `fetched_via: tor`,
    /// `trip_id`); `None` for every other note.
    pub web: Option<WebSource>,
}

/// The origin of a note fetched on a gated online trip (§5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSource {
    /// The exact URL the text was fetched from (tracking params stripped).
    pub url: String,
    /// The trip (and its Tor circuit) that fetched it.
    pub trip_id: String,
}

fn q(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

fn unq(s: &str) -> String {
    let s = s.trim();
    if s.starts_with('"') {
        serde_json::from_str::<String>(s).unwrap_or_default()
    } else {
        s.to_string()
    }
}

/// Unix seconds of `YYYY-MM-DDTHH:MM:SSZ` (0 when unreadable).
pub fn parse_utc(s: &str) -> f64 {
    let s = s.trim().trim_matches('"');
    let num = |a: usize, b: usize| s.get(a..b).and_then(|x| x.parse::<i64>().ok());
    let (Some(y), Some(m), Some(d)) = (num(0, 4), num(5, 7), num(8, 10)) else {
        return 0.0;
    };
    let (hh, mm, ss) = (
        num(11, 13).unwrap_or(0),
        num(14, 16).unwrap_or(0),
        num(17, 19).unwrap_or(0),
    );
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    (days * 86_400 + hh * 3600 + mm * 60 + ss) as f64
}

/// Splits `{ a: 1, b: "x" }` into key/value pairs.
fn inline_map(v: &str) -> Vec<(String, String)> {
    let inner = v.trim().trim_start_matches('{').trim_end_matches('}');
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut esc = false;
    let mut parts = Vec::new();
    for c in inner.chars() {
        if in_str {
            cur.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                cur.push(c);
            }
            ',' => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    parts.push(cur);
    for p in parts {
        if let Some((k, v)) = p.split_once(':') {
            out.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    out
}

impl Note {
    /// The file text: front matter + body.
    pub fn render(&self) -> String {
        let links = self
            .links
            .iter()
            .map(|l| q(l))
            .collect::<Vec<_>>()
            .join(", ");
        let web = match &self.web {
            Some(w) => format!(
                "url: {}\nfetched_via: tor\ntrip_id: {}\n",
                q(&w.url),
                q(&w.trip_id)
            ),
            None => String::new(),
        };
        format!(
            "---\nid: {}\ntitle: {}\ntopic: {}\nkind: {}\nsource: {{ type: {}, ref: {}, hash: {} }}\n\
             {}created: {}\nupdated: {}\nconfidence: {:.2}\nquiz: {{ asked: {}, passed: {}, last: null }}\n\
             links: [{}]\norigin: {}\n---\n{}\n",
            self.id,
            q(&self.title),
            self.topic,
            self.kind,
            self.source_type,
            q(&self.source_ref),
            q(&self.source_hash),
            web,
            utc(self.created),
            utc(self.updated),
            self.confidence,
            self.quiz_asked,
            self.quiz_passed,
            links,
            self.origin,
            self.body.trim_end()
        )
    }

    /// Parses a note file; `None` without a front matter `id`.
    pub fn parse(text: &str) -> Option<Note> {
        let rest = text.strip_prefix("---\n")?;
        let end = rest.find("\n---\n")?;
        let (head, body) = (&rest[..end], &rest[end + 5..]);
        let mut n = Note {
            id: String::new(),
            title: String::new(),
            topic: INBOX.into(),
            kind: "fact".into(),
            source_type: String::new(),
            source_ref: String::new(),
            source_hash: String::new(),
            created: 0.0,
            updated: 0.0,
            confidence: 0.5,
            quiz_asked: 0,
            quiz_passed: 0,
            links: Vec::new(),
            origin: "learned".into(),
            body: body.trim_end().to_string(),
            web: None,
        };
        let (mut url, mut trip) = (None, None);
        for line in head.lines() {
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            let v = v.trim();
            match k.trim() {
                "id" => n.id = unq(v),
                "title" => n.title = unq(v),
                "topic" => n.topic = unq(v),
                "kind" => n.kind = unq(v),
                "created" => n.created = parse_utc(v),
                "updated" => n.updated = parse_utc(v),
                "confidence" => n.confidence = v.parse().unwrap_or(0.5),
                "origin" => n.origin = unq(v),
                "url" => url = Some(unq(v)),
                "trip_id" => trip = Some(unq(v)),
                "source" => {
                    for (k, v) in inline_map(v) {
                        match k.as_str() {
                            "type" => n.source_type = unq(&v),
                            "ref" => n.source_ref = unq(&v),
                            "hash" => n.source_hash = unq(&v),
                            _ => {}
                        }
                    }
                }
                "quiz" => {
                    for (k, v) in inline_map(v) {
                        match k.as_str() {
                            "asked" => n.quiz_asked = v.parse().unwrap_or(0),
                            "passed" => n.quiz_passed = v.parse().unwrap_or(0),
                            _ => {}
                        }
                    }
                }
                "links" => {
                    n.links = serde_json::from_str::<Vec<String>>(v).unwrap_or_default();
                }
                _ => {}
            }
        }
        if let Some(url) = url {
            n.web = Some(WebSource {
                url,
                trip_id: trip.unwrap_or_default(),
            });
        }
        (!n.id.is_empty()).then_some(n)
    }

    /// The content hash written to the index.
    pub fn content_hash(&self) -> String {
        crate::fnv1a(format!("{}\n{}", self.title, self.body).as_bytes())
    }
}

/// `true` for a topic slug `[a-z0-9-]{1,32}`.
pub fn slug_ok(s: &str) -> bool {
    (1..=32).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `true` for a note id `n-[a-z0-9-]{1,40}`.
pub fn id_ok(s: &str) -> bool {
    s.strip_prefix("n-").is_some_and(|r| {
        (1..=40).contains(&r.len())
            && r.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

/// File names never written, even inside the knowledge folder.
fn forbidden_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "config.toml"
        || n.starts_with("persona")
        || n.starts_with("modelfile")
        || n == "audit.log"
        || n.ends_with(".toml")
        || n.ends_with(".db")
        || n == ".lock"
}

fn private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

/// The note folder.
#[derive(Debug, Clone)]
pub struct NoteStore {
    root: PathBuf,
    topics: Vec<String>,
    allow_create: bool,
}

/// The single-writer lock (released on drop).
#[derive(Debug)]
pub struct WriteLock {
    _f: File,
}

impl NoteStore {
    /// The store at `root` (created 0700 with the seed topic folders and
    /// `inbox/`).
    pub fn open(root: &Path, topics: &[String], allow_create: bool) -> io::Result<Self> {
        private_dir(root)?;
        let root = std::fs::canonicalize(root)?;
        let topics: Vec<String> = topics.iter().filter(|t| slug_ok(t)).cloned().collect();
        for t in topics.iter().map(String::as_str).chain([INBOX]) {
            private_dir(&root.join(t))?;
        }
        Ok(Self {
            root,
            topics,
            allow_create,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Takes `knowledge/.lock`; `None` while another writer holds it.
    pub fn lock(&self) -> io::Result<Option<WriteLock>> {
        Ok(
            lillith_core::llm_gate::try_lock_file(&self.root.join(".lock"))?
                .map(|f| WriteLock { _f: f }),
        )
    }

    /// The folder a note of `topic` goes to: a configured topic, a new
    /// valid slug (with `allow_create_topics`), else `inbox`.
    pub fn resolve_topic(&self, topic: Option<&str>) -> String {
        match topic {
            Some(t) if t == INBOX || self.topics.iter().any(|x| x == t) => t.to_string(),
            Some(t) if self.allow_create && slug_ok(t) => t.to_string(),
            _ => INBOX.to_string(),
        }
    }

    /// `<root>/<topic>/<id>.md`, refused when it would leave the root or
    /// hit a forbidden name.
    pub fn path_for(&self, topic: &str, id: &str) -> io::Result<PathBuf> {
        let refuse = |why: &str| io::Error::new(io::ErrorKind::PermissionDenied, why.to_string());
        if !slug_ok(topic) || !id_ok(id) {
            return Err(refuse("bad topic or note id"));
        }
        let name = format!("{id}.md");
        if forbidden_name(&name) {
            return Err(refuse("forbidden file name"));
        }
        let p = self.root.join(topic).join(name);
        self.check_inside(&p)?;
        Ok(p)
    }

    /// Refuses any path not inside the knowledge folder (or a protected
    /// file name).
    pub fn check_inside(&self, p: &Path) -> io::Result<()> {
        let refuse = |why: &str| io::Error::new(io::ErrorKind::PermissionDenied, why.to_string());
        if p.components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(refuse("path leaves the knowledge folder"));
        }
        if !p.starts_with(&self.root) {
            return Err(refuse("path outside the knowledge folder"));
        }
        if let Some(parent) = p.parent() {
            if let Ok(c) = std::fs::canonicalize(parent) {
                if !c.starts_with(&self.root) {
                    return Err(refuse("folder resolves outside the knowledge folder"));
                }
            }
        }
        if p.file_name()
            .map(|n| forbidden_name(&n.to_string_lossy()))
            .unwrap_or(true)
        {
            return Err(refuse("protected file name"));
        }
        Ok(())
    }

    /// Writes `note` atomically (0600 tmp file, then rename) and returns
    /// its path. The caller holds [`lock`](Self::lock). The body is cut so
    /// the file stays within [`MAX_NOTE_BYTES`](crate::MAX_NOTE_BYTES).
    pub fn write(&self, _lock: &WriteLock, note: &Note) -> io::Result<PathBuf> {
        let topic = self.resolve_topic(Some(&note.topic));
        let path = self.path_for(&topic, &note.id)?;
        private_dir(path.parent().unwrap_or(&self.root))?;
        let mut n = note.clone();
        n.topic = topic;
        let mut text = n.render();
        while text.len() > crate::MAX_NOTE_BYTES && !n.body.is_empty() {
            let over = text.len() - crate::MAX_NOTE_BYTES;
            let mut keep = n.body.len().saturating_sub(over + 4);
            while !n.body.is_char_boundary(keep) {
                keep -= 1;
            }
            n.body.truncate(keep);
            text = n.render();
        }
        let tmp = path.with_extension("md.tmp");
        let _ = std::fs::remove_file(&tmp);
        {
            let mut o = std::fs::OpenOptions::new();
            o.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                o.mode(0o600);
            }
            let mut f = o.open(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &path)?;
        Ok(path)
    }

    /// Reads the note at `rel` (relative to the root).
    pub fn read(&self, rel: &str) -> Option<Note> {
        let p = self.root.join(rel);
        self.check_inside(&p).ok()?;
        let md = std::fs::symlink_metadata(&p).ok()?;
        if !md.is_file() || md.len() > (crate::MAX_NOTE_BYTES as u64) * 2 {
            return None;
        }
        Note::parse(&std::fs::read_to_string(p).ok()?)
    }

    /// Every note on disk as `(relative path, note)` (topic folders one
    /// level deep; symlinks are skipped).
    pub fn read_all(&self) -> Vec<(String, Note)> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.root) else {
            return out;
        };
        let mut dirs: Vec<_> = rd
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter(|e| slug_ok(&e.file_name().to_string_lossy()))
            .collect();
        dirs.sort_by_key(|e| e.file_name());
        for d in dirs {
            let topic = d.file_name().to_string_lossy().into_owned();
            let Ok(files) = std::fs::read_dir(d.path()) else {
                continue;
            };
            let mut files: Vec<_> = files.flatten().collect();
            files.sort_by_key(|e| e.file_name());
            for f in files {
                let name = f.file_name().to_string_lossy().into_owned();
                if !name.ends_with(".md") || !f.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    continue;
                }
                let rel = format!("{topic}/{name}");
                if let Some(mut n) = self.read(&rel) {
                    n.topic = topic.clone();
                    out.push((rel, n));
                }
            }
        }
        out
    }

    /// The relative path of a written note.
    pub fn rel(&self, p: &Path) -> String {
        p.strip_prefix(&self.root)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/")
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn note(id: &str, topic: &str, title: &str) -> Note {
        Note {
            id: id.into(),
            title: title.into(),
            topic: topic.into(),
            kind: "howto".into(),
            source_type: "man".into(),
            source_ref: "man:pacman(8)".into(),
            source_hash: "fnv1a:1".into(),
            created: 1_792_800_000.0,
            updated: 1_792_800_060.0,
            confidence: 0.6,
            quiz_asked: 0,
            quiz_passed: 0,
            links: vec!["entity:pacman".into()],
            origin: "learned".into(),
            body: "Use `pacman -Rns $(pacman -Qtdq)`.\n\n\"quoted\" body".into(),
            web: None,
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lillith-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn render_parse_round_trip() {
        let n = note(
            "n-abc",
            "linux-garuda",
            "Remove orphan packages: \"pacman\"",
        );
        let back = Note::parse(&n.render()).unwrap();
        assert_eq!(back, n);
        assert_eq!(parse_utc("2026-10-24T00:00:00Z"), 1_792_800_000.0);
        assert!(Note::parse("no front matter").is_none());
    }

    #[test]
    fn slugs_and_topics() {
        assert!(slug_ok("linux-garuda") && slug_ok("a") && !slug_ok("") && !slug_ok("Bad"));
        assert!(!slug_ok("../x") && !slug_ok(&"a".repeat(33)) && !slug_ok("a b"));
        let d = tmp("topics");
        let s = NoteStore::open(&d.join("knowledge"), &["rust".into()], true).unwrap();
        assert_eq!(s.resolve_topic(Some("rust")), "rust");
        assert_eq!(s.resolve_topic(Some("git")), "git");
        assert_eq!(s.resolve_topic(Some("Not A Slug")), INBOX);
        assert_eq!(s.resolve_topic(None), INBOX);
        let s = NoteStore::open(&d.join("knowledge"), &["rust".into()], false).unwrap();
        assert_eq!(s.resolve_topic(Some("git")), INBOX);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn writes_stay_inside_and_never_hit_protected_files() {
        let d = tmp("inside");
        let s = NoteStore::open(&d.join("knowledge"), &["rust".into()], true).unwrap();
        assert!(s.path_for("rust", "n-1").is_ok());
        assert!(s.path_for("..", "n-1").is_err());
        assert!(s.path_for("rust", "../../config").is_err());
        assert!(s.path_for("rust", "config.toml").is_err());
        for p in [
            "config.toml",
            "persona.md",
            "modelfile",
            "audit.log",
            "rust/x.toml",
        ] {
            assert!(s.check_inside(&s.root().join(p)).is_err(), "{p}");
        }
        assert!(s.check_inside(&d.join("elsewhere.md")).is_err());
        assert!(s.check_inside(&s.root().join("rust/../../x.md")).is_err());
        #[cfg(unix)]
        {
            // A topic folder that is a symlink out of the root is refused.
            std::fs::create_dir_all(d.join("outside")).unwrap();
            std::os::unix::fs::symlink(d.join("outside"), s.root().join("evil")).unwrap();
            assert!(s.path_for("evil", "n-1").is_err());
            let lock = s.lock().unwrap().unwrap();
            let mut n = note("n-2", "evil", "t");
            n.topic = "evil".into();
            assert!(s.write(&lock, &n).is_err());
            assert_eq!(std::fs::read_dir(d.join("outside")).unwrap().count(), 0);
        }
        let _ = std::fs::remove_dir_all(d);
    }

    #[cfg(unix)]
    #[test]
    fn permissions_are_0700_and_0600() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("perm");
        let s = NoteStore::open(&d.join("knowledge"), &["rust".into()], true).unwrap();
        let lock = s.lock().unwrap().unwrap();
        assert!(s.lock().unwrap().is_none(), "single writer");
        let p = s.write(&lock, &note("n-p1", "git", "t")).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(s.root()), 0o700);
        assert_eq!(mode(&s.root().join("rust")), 0o700);
        assert_eq!(mode(&s.root().join(INBOX)), 0o700);
        assert_eq!(mode(&s.root().join("git")), 0o700, "created topic");
        assert_eq!(mode(&p), 0o600);
        assert_eq!(mode(&s.root().join(".lock")), 0o600);
        assert!(!p.with_extension("md.tmp").exists(), "atomic rename");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn notes_are_capped_and_read_back() {
        let d = tmp("cap");
        let s = NoteStore::open(&d.join("knowledge"), &["rust".into()], true).unwrap();
        let lock = s.lock().unwrap().unwrap();
        let mut n = note("n-big", "rust", "big");
        n.body = "word ".repeat(4000);
        let p = s.write(&lock, &n).unwrap();
        assert!(std::fs::metadata(&p).unwrap().len() as usize <= crate::MAX_NOTE_BYTES);
        s.write(&lock, &note("n-small", "nope topic", "x")).unwrap();
        let all = s.read_all();
        assert_eq!(all.len(), 2);
        assert!(all
            .iter()
            .any(|(r, n)| r == "inbox/n-small.md" && n.topic == INBOX));
        let _ = std::fs::remove_dir_all(d);
    }
}
