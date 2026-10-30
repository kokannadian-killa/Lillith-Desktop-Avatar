//! Dream-learning orchestration (docs/SELF_LEARNING.md §11, P7): runs one
//! approved [`Session`] until it stops.
//!
//! - **Start.** Only [`Session::from_card`], i.e. a mouse click on Yes on
//!   the dream card ([`super::dream::propose_session`]). The entry points
//!   (`/dream`, "Dream learning…", `lillith dream`) only open that card.
//! - **Work.** A [`DreamPlanner`] hands out research [`Task`]s; the
//!   [`Supervisor`] runs each one in a sandboxed, network-less helper
//!   worker (the parent does every GET over Tor, paced, budgeted). Only
//!   validated candidates come back; quarantined ones stay quarantined.
//! - **Memory.** Accepted facts are saved by a [`DreamSink`]: the
//!   production [`NoteSink`] writes untrusted `web` notes (`kind: dream`,
//!   the session id as `trip_id`) and indexes them in the memory graph at
//!   `[learn.dream] dream_strength` (weak; they only grow if used).
//!   [`DreamSink::after_session`] is the self-compression hook (§13, P8:
//!   a no-op today).
//! - **Announce.** Every step is a [`DreamEvent`] (where she's going,
//!   where she's been, what she saved, how it ended) and a `DREAM` audit
//!   line (fail-closed: no line, the session stops as `audit_failed`).
//! - **Stops.** The deadline fixed at card time, End early, the plain stop,
//!   the kill switch (Ctrl-C / SIGTERM) and battery / thermal end it; the
//!   supervisor kills the worker's process group within one poll. The
//!   safeword is **not** a stop (voice only; Ko: "Just the voice").
//! - **Status.** [`DreamControl`] holds the live [`DreamStatus`] for the
//!   menu and the Bots tab ([`dream_status`], [`end_dream_early`],
//!   [`stop_dream`]); the app's copy is also mirrored to
//!   `$XDG_RUNTIME_DIR/lillith/dream-status.json` ([`read_status_file`]).

use super::supervisor::{Session, Supervisor, Task};
use super::validate::Candidate;
use super::{JobStatus, StopReason};
use crate::store::{Note, NoteStore, WebSource};
use lillith_core::config::Config;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The status file name under the runtime dir.
pub const STATUS_FILE: &str = "dream-status.json";
/// Failed / refused jobs in a row before the session gives up.
pub const MAX_FAILED_IN_A_ROW: u32 = 3;
/// Most visited addresses kept in [`DreamStatus::visited`].
pub const MAX_VISITED_SHOWN: usize = 200;

/// Most saved items the live log keeps (oldest drop first).
pub const MAX_ITEMS_SHOWN: usize = 500;

/// One note the session saved, for the live log (Bots tab: title, where
/// from, and a per-item Forget via [`forget_dream_item`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DreamItem {
    /// The note id ([`forget_dream_item`] takes it).
    pub id: String,
    pub title: String,
    /// The source host (`doc.rust-lang.org`).
    pub host: String,
    /// The note file (local, under the knowledge folder; empty when the
    /// sink writes no file). An item whose file is gone was forgotten and
    /// is dropped from the status ([`DreamStatus::without_forgotten`]).
    pub path: String,
}

impl DreamItem {
    fn to_json(&self) -> Value {
        json!({ "id": self.id, "title": self.title, "host": self.host, "path": self.path })
    }

    fn from_json(v: &Value) -> Option<Self> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(String::from);
        Some(Self {
            id: s("id")?,
            title: s("title").unwrap_or_default(),
            host: s("host").unwrap_or_default(),
            path: s("path").unwrap_or_default(),
        })
    }
}

/// Where a dream is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DreamState {
    /// Nothing running, no card up.
    #[default]
    Idle,
    /// The card is up (waiting for a click).
    Proposed,
    /// A session is running.
    Running,
    /// Stopping (workers being killed, the summary being written).
    Ending,
    /// The last session ended ([`DreamStatus::stop_reason`] says why).
    Ended,
}

impl DreamState {
    pub fn word(self) -> &'static str {
        match self {
            DreamState::Idle => "idle",
            DreamState::Proposed => "proposed",
            DreamState::Running => "running",
            DreamState::Ending => "ending",
            DreamState::Ended => "ended",
        }
    }

    fn from_word(w: &str) -> Option<Self> {
        [
            DreamState::Idle,
            DreamState::Proposed,
            DreamState::Running,
            DreamState::Ending,
            DreamState::Ended,
        ]
        .into_iter()
        .find(|s| s.word() == w)
    }
}

/// The live session status (the lead's Bots tab, `/dream status`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DreamStatus {
    pub state: DreamState,
    /// The session id (`s-…`).
    pub session: Option<String>,
    /// UTC start (`YYYY-MM-DDTHH:MM:SSZ`).
    pub started: Option<String>,
    /// UTC hard deadline (the card's end time).
    pub deadline: Option<String>,
    /// The same deadline as she says it, in the system's local time zone
    /// (`7:30 PM MDT`, [`lillith_core::localtime::clock_label`]).
    pub deadline_local: Option<String>,
    /// What she is looking up right now (`topic — host`).
    pub where_now: Option<String>,
    /// Addresses fetched so far (newest last, at most [`MAX_VISITED_SHOWN`]).
    pub visited: Vec<String>,
    pub saved_count: usize,
    /// What was saved (newest last, at most [`MAX_ITEMS_SHOWN`]).
    pub items: Vec<DreamItem>,
    pub quarantined_count: usize,
    pub stop_reason: Option<StopReason>,
    /// The end-of-session summary.
    pub summary: Option<String>,
    /// The optional local-model digest of the saved notes
    /// ([`super::digest::summarize_session`]); `None` when off, the model
    /// is down or chat needed it.
    pub digest: Option<String>,
}

impl DreamStatus {
    /// One line for chat / a tooltip.
    pub fn line(&self) -> String {
        match self.state {
            DreamState::Idle => "Not dreaming.".into(),
            DreamState::Proposed => "The dream card is up, waiting for a click.".into(),
            DreamState::Running | DreamState::Ending => format!(
                "Dreaming{}: {} page{} visited, {} saved{}{}.",
                self.deadline_local
                    .as_deref()
                    .or(self.deadline.as_deref())
                    .map(|d| format!(" until {d}"))
                    .unwrap_or_default(),
                self.visited.len(),
                if self.visited.len() == 1 { "" } else { "s" },
                self.saved_count,
                if self.quarantined_count > 0 {
                    format!(", {} quarantined", self.quarantined_count)
                } else {
                    String::new()
                },
                self.where_now
                    .as_deref()
                    .map(|w| format!(", now: {w}"))
                    .unwrap_or_default(),
            ),
            DreamState::Ended => self
                .summary
                .clone()
                .unwrap_or_else(|| "The last dream ended.".into()),
        }
    }

    /// This status minus items whose note file is gone (forgotten, from
    /// this or another process).
    pub fn without_forgotten(mut self) -> Self {
        self.items
            .retain(|i| i.path.is_empty() || Path::new(&i.path).exists());
        self
    }

    pub fn to_json(&self) -> Value {
        json!({
            "state": self.state.word(),
            "session": self.session,
            "started": self.started,
            "deadline": self.deadline,
            "deadline_local": self.deadline_local,
            "where_now": self.where_now,
            "visited": self.visited,
            "saved_count": self.saved_count,
            "items": self.items.iter().map(DreamItem::to_json).collect::<Vec<_>>(),
            "quarantined_count": self.quarantined_count,
            "stop_reason": self.stop_reason.map(StopReason::word),
            "summary": self.summary,
            "digest": self.digest,
        })
    }

    pub fn from_json(v: &Value) -> Option<Self> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(String::from);
        let n = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0) as usize;
        Some(Self {
            state: DreamState::from_word(v.get("state")?.as_str()?)?,
            session: s("session"),
            started: s("started"),
            deadline: s("deadline"),
            deadline_local: s("deadline_local"),
            where_now: s("where_now"),
            visited: v
                .get("visited")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|u| u.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            saved_count: n("saved_count"),
            items: v
                .get("items")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(DreamItem::from_json).collect())
                .unwrap_or_default(),
            quarantined_count: n("quarantined_count"),
            stop_reason: s("stop_reason")
                .and_then(|w| StopReason::ALL.into_iter().find(|r| r.word() == w)),
            summary: s("summary"),
            digest: s("digest"),
        })
    }
}

/// The shared handle on the (at most one) dream of a process: the run
/// thread updates it, the menu / chat / Bots tab read it and stop it.
#[derive(Clone, Default)]
pub struct DreamControl {
    inner: Arc<Mutex<Ctl>>,
}

#[derive(Default)]
struct Ctl {
    status: DreamStatus,
    session: Option<Session>,
    file: Option<PathBuf>,
}

impl DreamControl {
    /// A control that also mirrors its status to `file` (0600).
    pub fn with_status_file(file: Option<PathBuf>) -> Self {
        let c = Self::default();
        c.lock().file = file;
        c
    }

    /// Records the after-session digest (the status file follows).
    pub fn set_digest(&self, text: String) {
        self.update(|s| s.digest = Some(text));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Ctl> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn update(&self, f: impl FnOnce(&mut DreamStatus)) {
        let mut g = self.lock();
        f(&mut g.status);
        if let Some(p) = g.file.clone() {
            let _ = write_private(&p, g.status.to_json().to_string().as_bytes());
        }
    }

    /// The current status (cheap clone).
    pub fn status(&self) -> DreamStatus {
        self.lock().status.clone()
    }

    /// Whether a session is running (or ending).
    pub fn is_running(&self) -> bool {
        matches!(
            self.lock().status.state,
            DreamState::Running | DreamState::Ending
        )
    }

    /// Claims the single dream slot for a card: `false` when a card is up
    /// or a session runs.
    pub fn try_propose(&self) -> bool {
        let mut g = self.lock();
        if matches!(
            g.status.state,
            DreamState::Proposed | DreamState::Running | DreamState::Ending
        ) {
            return false;
        }
        g.status = DreamStatus {
            state: DreamState::Proposed,
            ..Default::default()
        };
        if let Some(p) = g.file.clone() {
            let _ = write_private(&p, g.status.to_json().to_string().as_bytes());
        }
        true
    }

    /// The card was refused / closed: back to idle with `why` as summary.
    pub fn proposal_refused(&self, why: &str) {
        self.update(|s| {
            if s.state == DreamState::Proposed {
                *s = DreamStatus {
                    state: DreamState::Idle,
                    summary: Some(why.to_string()),
                    ..Default::default()
                };
            }
        });
    }

    pub(super) fn attach(&self, session: &Session) {
        let now = crate::now_s();
        let end = now + session.remaining().as_secs_f64();
        let deadline = crate::audit::utc(end);
        let deadline_local = lillith_core::localtime::clock_label(end.round() as i64);
        self.lock().session = Some(session.clone());
        self.update(|s| {
            *s = DreamStatus {
                state: DreamState::Running,
                session: Some(session.id().to_string()),
                started: Some(crate::audit::utc(now)),
                deadline: Some(deadline),
                deadline_local: Some(deadline_local),
                ..Default::default()
            }
        });
    }

    fn detach(&self, reason: StopReason, summary: String) {
        self.lock().session = None;
        self.update(|s| {
            s.state = DreamState::Ended;
            s.where_now = None;
            s.stop_reason = Some(reason);
            s.summary = Some(summary);
        });
    }

    /// End early (menu "End dream early", `/dream end`). `true` when a
    /// session was running.
    pub fn end_early(&self) -> bool {
        let s = self.lock().session.clone();
        match s {
            Some(s) => {
                s.end_early();
                true
            }
            None => false,
        }
    }

    /// The plain stop (`/dream stop`, the CLI). `true` when a session was
    /// running.
    pub fn stop(&self) -> bool {
        let s = self.lock().session.clone();
        match s {
            Some(s) => {
                s.stop();
                true
            }
            None => false,
        }
    }
}

/// The process-wide control the app's hooks use. Mirrors its status to
/// `<runtime dir>/dream-status.json`.
pub fn global() -> &'static DreamControl {
    static G: OnceLock<DreamControl> = OnceLock::new();
    G.get_or_init(|| DreamControl::with_status_file(status_file_path()))
}

/// The live status of this process's dream (the Bots tab API).
pub fn dream_status() -> DreamStatus {
    global().status().without_forgotten()
}

/// End the running dream early (`true` if one was running).
pub fn end_dream_early() -> bool {
    global().end_early()
}

/// Hard-stop the running dream (`true` if one was running).
pub fn stop_dream() -> bool {
    global().stop()
}

/// `<runtime dir>/dream-status.json` (`None` without a runtime dir).
pub fn status_file_path() -> Option<PathBuf> {
    lillith_core::llm_gate::runtime_dir().map(|d| d.join(STATUS_FILE))
}

/// The status another process (the app running the dream) last wrote;
/// for the settings window, which runs in its own process.
pub fn read_status_file() -> Option<DreamStatus> {
    let p = status_file_path()?;
    let t = std::fs::read_to_string(p).ok()?;
    DreamStatus::from_json(&serde_json::from_str(&t).ok()?).map(DreamStatus::without_forgotten)
}

/// Forgets one dream-saved note (the live log's per-item Forget): deletes
/// its file under the knowledge folder and its memory index row + node.
/// Only `kind: dream` notes; anything else is refused. Writes a
/// `dream_forget` audit line (the id only). Returns the note's title.
/// Works from any process (the settings window too): the item drops out
/// of every status read once its file is gone.
pub fn forget_dream_item(cfg: &Config, id: &str) -> Result<String, String> {
    forget_dream_item_in(cfg, id, cfg.mind.audit_log_path().as_deref(), global())
}

/// [`forget_dream_item`] with an explicit audit log and control (tests).
pub(super) fn forget_dream_item_in(
    cfg: &Config,
    id: &str,
    audit_log: Option<&Path>,
    control: &DreamControl,
) -> Result<String, String> {
    if !crate::store::id_ok(id) {
        return Err("not a note id".into());
    }
    let topic = crate::online::WEB_TOPIC;
    let dir = cfg
        .mind
        .resolved_knowledge_dir()
        .ok_or("no knowledge folder")?;
    let topics = [topic.to_string()];
    let store = NoteStore::open(&dir, &topics, false).map_err(|e| format!("knowledge: {e}"))?;
    let path = store.path_for(topic, id).map_err(|e| e.to_string())?;
    let file_note = store.read(&store.rel(&path));
    let brain = if cfg.memory.enabled {
        lillith_memory::Brain::open(&cfg.memory).ok()
    } else {
        None
    };
    let row = brain.as_ref().and_then(|b| b.note(id));
    let title = match (&file_note, &row) {
        (None, None) => return Err("no such dream note".into()),
        (Some(n), _) if n.kind != "dream" => return Err("not a dream note".into()),
        (_, Some(r)) if r.kind != "dream" => return Err("not a dream note".into()),
        (Some(n), _) => n.title.clone(),
        (None, Some(r)) => r.title.clone(),
    };
    if file_note.is_some() {
        let lock = match store.lock() {
            Ok(Some(l)) => l,
            Ok(None) => return Err("another writer holds knowledge/.lock".into()),
            Err(e) => return Err(format!("lock: {e}")),
        };
        store
            .remove(&lock, &path)
            .map_err(|e| format!("remove: {e}"))?;
    }
    if let Some(b) = &brain {
        b.forget_note(id).map_err(|e| format!("memory: {e}"))?;
    }
    crate::audit::record(audit_log, "dream_forget", topic, id);
    // Only touch this process's status when it holds the item (the
    // settings window must not overwrite the app's status file).
    if control.status().items.iter().any(|i| i.id == id) {
        control.update(|s| s.items.retain(|i| i.id != id));
    }
    Ok(title)
}

fn write_private(p: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(d) = p.parent() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(d)?;
        }
        #[cfg(not(unix))]
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("json.tmp");
    let mut o = std::fs::OpenOptions::new();
    o.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(&tmp)?;
    f.write_all(bytes)?;
    drop(f);
    std::fs::rename(tmp, p)
}

/// What she announces (chat line + overlay event); each also has a
/// `DREAM` audit line.
#[derive(Debug, Clone, PartialEq)]
pub enum DreamEvent {
    Started {
        session: String,
        /// UTC (for the audit).
        deadline: String,
        /// Local wall clock (what she says), e.g. `7:30 PM MDT`.
        deadline_local: String,
    },
    /// Where she's going.
    Going {
        topic: String,
        urls: Vec<String>,
    },
    /// Where she's been.
    Visited {
        url: String,
    },
    /// What she saved.
    Saved {
        id: String,
        title: String,
        url: String,
    },
    /// Results kept out of memory (read-only quarantine).
    Quarantined {
        count: usize,
        why: String,
    },
    Ended {
        reason: StopReason,
        summary: String,
    },
}

impl DreamEvent {
    /// The overlay event name (`dream.*`).
    pub fn name(&self) -> &'static str {
        match self {
            DreamEvent::Started { .. } => "dream.started",
            DreamEvent::Going { .. } => "dream.going",
            DreamEvent::Visited { .. } => "dream.visited",
            DreamEvent::Saved { .. } => "dream.saved",
            DreamEvent::Quarantined { .. } => "dream.quarantined",
            DreamEvent::Ended { .. } => "dream.ended",
        }
    }

    /// Her line.
    pub fn line(&self) -> String {
        match self {
            DreamEvent::Started {
                deadline,
                deadline_local,
                ..
            } => {
                let at = if deadline_local.is_empty() {
                    deadline
                } else {
                    deadline_local
                };
                format!("Going to dream now — I'll be back by {at}.")
            }
            DreamEvent::Going { topic, urls } => format!(
                "Dreaming about {topic}: going to {}.",
                hosts_of(urls).join(", ")
            ),
            DreamEvent::Visited { url } => format!("Been to {url}."),
            DreamEvent::Saved { title, url, .. } => {
                format!("Saved \"{title}\" (from {url}).")
            }
            DreamEvent::Quarantined { count, why } => format!(
                "Kept {count} thing{} out of my memory ({why}).",
                if *count == 1 { "" } else { "s" }
            ),
            DreamEvent::Ended { summary, .. } => summary.clone(),
        }
    }
}

fn hosts_of(urls: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for u in urls {
        let h = crate::online::Url::parse(u)
            .map(|u| u.host().to_string())
            .unwrap_or_else(|_| u.clone());
        if !out.contains(&h) {
            out.push(h);
        }
    }
    out
}

/// Where announcements go (chat feed, overlay events, the CLI).
pub type Announce = Arc<dyn Fn(&DreamEvent) + Send + Sync>;

/// The curiosity queue: what to look up next.
pub trait DreamPlanner {
    /// The next task, or `None` when there is nothing left.
    fn next(&mut self) -> Option<Task>;
}

/// Fixed public reference pages per knowledge topic: the same list for
/// everyone (zero fingerprinting: nothing from Ko's files, history or
/// memory goes into an address). Personal topics (`ko-preferences`,
/// `self`) are never looked up online.
pub const SEEDS: &[(&str, &[&str])] = &[
    (
        "rust",
        &[
            "https://doc.rust-lang.org/book/ch10-02-traits.html",
            "https://doc.rust-lang.org/book/ch16-00-concurrency.html",
            "https://doc.rust-lang.org/book/ch09-00-error-handling.html",
        ],
    ),
    (
        "linux-garuda",
        &[
            "https://wiki.archlinux.org/title/Pacman",
            "https://wiki.archlinux.org/title/Systemd",
            "https://wiki.archlinux.org/title/Btrfs",
        ],
    ),
    (
        "python",
        &[
            "https://docs.python.org/3/tutorial/classes.html",
            "https://docs.python.org/3/library/pathlib.html",
        ],
    ),
    (
        "coding",
        &[
            "https://en.wikipedia.org/wiki/Big_O_notation",
            "https://en.wikipedia.org/wiki/Regular_expression",
        ],
    ),
    (
        "tools",
        &[
            "https://wiki.archlinux.org/title/Git",
            "https://wiki.archlinux.org/title/Bash",
        ],
    ),
    (
        "fixes",
        &[
            "https://wiki.archlinux.org/title/General_troubleshooting",
            "https://wiki.archlinux.org/title/System_maintenance",
        ],
    ),
];

/// A fixed queue of tasks, one address each, topics interleaved.
#[derive(Debug, Clone, Default)]
pub struct SeedPlanner {
    queue: VecDeque<Task>,
}

impl SeedPlanner {
    pub fn new(tasks: impl IntoIterator<Item = Task>) -> Self {
        Self {
            queue: tasks.into_iter().collect(),
        }
    }

    /// [`SEEDS`] for the topics `[learn] topics` has, round-robin.
    pub fn from_config(cfg: &Config) -> Self {
        let seeds: Vec<(&str, &[&str])> = SEEDS
            .iter()
            .filter(|(t, _)| cfg.learn.topics.iter().any(|x| x.trim() == *t))
            .copied()
            .collect();
        let most = seeds.iter().map(|(_, u)| u.len()).max().unwrap_or(0);
        let mut tasks = Vec::new();
        for i in 0..most {
            for (topic, urls) in &seeds {
                if let Some(u) = urls.get(i) {
                    tasks.push(Task::research(topic, &[u]));
                }
            }
        }
        Self::new(tasks)
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

impl DreamPlanner for SeedPlanner {
    fn next(&mut self) -> Option<Task> {
        self.queue.pop_front()
    }
}

/// One saved fact.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedFact {
    pub id: String,
    pub title: String,
    pub url: String,
    /// The note file (empty when the sink writes none).
    pub path: PathBuf,
    /// Bytes it took on disk.
    pub bytes: u64,
}

/// Where accepted candidates go. The supervisor never writes memory; the
/// orchestrator hands each accepted [`Candidate`] here.
pub trait DreamSink {
    fn save(&mut self, topic: &str, c: &Candidate, session: &str) -> Result<SavedFact, String>;
    /// Memory self-compression after a session (§13 compaction, P8). No-op
    /// until compaction exists.
    fn after_session(&mut self) {}
}

/// The production sink: untrusted `web` notes (`kind: dream`) plus a weak
/// graph node at `dream_strength`.
pub struct NoteSink {
    store: Option<NoteStore>,
    brain: Option<lillith_memory::Brain>,
    strength: f64,
    audit_log: Option<PathBuf>,
}

impl NoteSink {
    /// Notes under `knowledge_dir` (`None` = nothing can be saved);
    /// `brain` indexes them (`None` = files only).
    pub fn new(
        cfg: &Config,
        knowledge_dir: Option<&Path>,
        brain: Option<lillith_memory::Brain>,
        audit_log: Option<PathBuf>,
    ) -> Self {
        let mut topics = cfg.learn.topics.clone();
        if !topics.iter().any(|t| t == crate::online::WEB_TOPIC) {
            topics.push(crate::online::WEB_TOPIC.into());
        }
        let store = knowledge_dir.and_then(|d| match NoteStore::open(d, &topics, false) {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!("dream: knowledge folder {}: {e}", d.display());
                None
            }
        });
        Self {
            store,
            brain,
            strength: f64::from(cfg.learn.dream.dream_strength),
            audit_log,
        }
    }
}

impl NoteSink {
    #[cfg(test)]
    pub(crate) fn brain_for_tests(&self) -> Option<&lillith_memory::Brain> {
        self.brain.as_ref()
    }
}

impl DreamSink for NoteSink {
    fn save(&mut self, topic: &str, c: &Candidate, session: &str) -> Result<SavedFact, String> {
        let store = self.store.as_ref().ok_or("no knowledge folder")?;
        let title: String = c.title.trim().chars().take(120).collect();
        let body = crate::paths::drop_secret_lines(c.text.trim());
        let body = body.trim();
        if title.is_empty() || body.is_empty() {
            return Err("empty".into());
        }
        // §7 ingest filter again (the validator ran it; belt and braces).
        let item = crate::extract::Item {
            kind: "web".into(),
            title: title.clone(),
            body: body.to_string(),
            links: Vec::new(),
        };
        if !crate::extract::allowed(&item) || lillith_memory::is_safeword(body) {
            return Err("dropped by the ingest filter".into());
        }
        let lock = match store.lock() {
            Ok(Some(l)) => l,
            Ok(None) => return Err("another writer holds knowledge/.lock".into()),
            Err(e) => return Err(format!("lock: {e}")),
        };
        let now = crate::now_s();
        let note = Note {
            id: crate::learner::new_id(&c.url, &title, now),
            title: title.clone(),
            topic: crate::online::WEB_TOPIC.into(),
            kind: "dream".into(),
            source_type: "web".into(),
            source_ref: c.url.clone(),
            source_hash: crate::fnv1a(body.as_bytes()),
            created: now,
            updated: now,
            confidence: crate::online::WEB_CONFIDENCE,
            quiz_asked: 0,
            quiz_passed: 0,
            links: Vec::new(),
            origin: "learned".into(),
            body: format!("{body}\n\n(dreamed about: {topic})"),
            web: Some(WebSource {
                url: c.url.clone(),
                trip_id: session.to_string(),
            }),
        };
        let path = store
            .write(&lock, &note)
            .map_err(|e| format!("write: {e}"))?;
        let rel = store.rel(&path);
        crate::audit::record(
            self.audit_log.as_deref(),
            "dream",
            crate::online::WEB_TOPIC,
            &note.id,
        );
        if let Some(b) = &self.brain {
            let row = lillith_memory::NoteRow {
                id: note.id.clone(),
                path: rel,
                topic: crate::online::WEB_TOPIC.into(),
                kind: note.kind.clone(),
                title: note.title.clone(),
                source_type: note.source_type.clone(),
                source_ref: note.source_ref.clone(),
                source_hash: note.source_hash.clone(),
                content_hash: note.content_hash(),
                confidence: note.confidence,
                origin: note.origin.clone(),
                created_at: now,
                updated_at: now,
            };
            if let Err(e) = b.upsert_note_with_strength(
                &row,
                &note.body,
                &lillith_memory::Ctx::at(now),
                self.strength,
            ) {
                tracing::warn!("dream: memory index of {}: {e}", note.id);
            }
        }
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        Ok(SavedFact {
            id: note.id,
            title,
            url: c.url.clone(),
            path,
            bytes,
        })
    }
}

/// Session-wide caps from `[learn.dream]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DreamLimits {
    /// `max_pages_per_hour` (fetched pages in any rolling hour).
    pub pages_per_hour: u32,
    /// `max_bytes_per_session` (fetched bytes).
    pub bytes_per_session: u64,
    /// `max_disk_mb` (bytes of notes written).
    pub disk_bytes: u64,
}

impl DreamLimits {
    pub fn from_config(cfg: &Config) -> Self {
        let d = &cfg.learn.dream;
        Self {
            pages_per_hour: d.max_pages_per_hour.max(1),
            bytes_per_session: d.max_bytes_per_session.max(1),
            disk_bytes: u64::from(d.max_disk_mb.max(1)) * 1024 * 1024,
        }
    }
}

/// Battery / thermal check (`[learn.dream] stop_battery_pct`,
/// `stop_cpu_temp_c`) from Linux sysfs: on battery at or below the
/// percentage → [`StopReason::LowBattery`]; any thermal zone at or above
/// the temperature → [`StopReason::Thermal`].
pub fn power_probe(cfg: &Config) -> impl Fn() -> Option<StopReason> + Send {
    let pct = cfg.learn.dream.stop_battery_pct;
    let temp = cfg.learn.dream.stop_cpu_temp_c;
    move || power_check(Path::new("/sys/class"), pct, temp)
}

/// [`power_probe`] against a sysfs root (tests use a fake tree).
pub fn power_check(sys: &Path, battery_pct: u32, cpu_temp_c: u32) -> Option<StopReason> {
    let read = |p: PathBuf| {
        std::fs::read_to_string(p)
            .ok()
            .map(|s| s.trim().to_string())
    };
    if let Ok(rd) = std::fs::read_dir(sys.join("power_supply")) {
        for e in rd.flatten() {
            let d = e.path();
            if read(d.join("type")).as_deref() != Some("Battery") {
                continue;
            }
            let discharging = read(d.join("status")).as_deref() == Some("Discharging");
            let cap = read(d.join("capacity")).and_then(|c| c.parse::<u32>().ok());
            if discharging && cap.is_some_and(|c| c <= battery_pct) {
                return Some(StopReason::LowBattery);
            }
        }
    }
    if cpu_temp_c > 0 {
        if let Ok(rd) = std::fs::read_dir(sys.join("thermal")) {
            for e in rd.flatten() {
                if !e.file_name().to_string_lossy().starts_with("thermal_zone") {
                    continue;
                }
                let t = read(e.path().join("temp")).and_then(|t| t.parse::<i64>().ok());
                if t.is_some_and(|m| m >= i64::from(cpu_temp_c) * 1000) {
                    return Some(StopReason::Thermal);
                }
            }
        }
    }
    None
}

/// What a finished session did.
#[derive(Debug, Clone, PartialEq)]
pub struct DreamReport {
    pub session: String,
    pub reason: StopReason,
    /// Why it stopped on its own, when it did (byte / disk budget, nothing
    /// left, Tor down, failures).
    pub note: Option<String>,
    pub visited: Vec<String>,
    pub saved: Vec<SavedFact>,
    pub quarantined: usize,
    pub jobs: usize,
    /// Every worker pid spawned (their process groups are gone by the end).
    pub pids: Vec<u32>,
    pub summary: String,
}

/// One session run.
pub struct DreamRun<'a, 'f> {
    sup: &'a mut Supervisor<'f>,
    planner: &'a mut dyn DreamPlanner,
    sink: &'a mut dyn DreamSink,
    control: DreamControl,
    announce: Announce,
    audit_log: Option<PathBuf>,
    limits: DreamLimits,
    conditions: Box<dyn Fn() -> Option<StopReason> + 'a>,
    tick: Duration,
}

impl<'a, 'f> DreamRun<'a, 'f> {
    pub fn new(
        cfg: &Config,
        sup: &'a mut Supervisor<'f>,
        planner: &'a mut dyn DreamPlanner,
        sink: &'a mut dyn DreamSink,
        control: DreamControl,
        announce: Announce,
    ) -> Self {
        Self {
            sup,
            planner,
            sink,
            control,
            announce,
            audit_log: lillith_core::config::audit_log_path(),
            limits: DreamLimits::from_config(cfg),
            conditions: Box::new(|| None),
            tick: Duration::from_millis(100),
        }
    }

    /// Where the `DREAM` lines go (the session's own log).
    pub fn with_audit_log(mut self, log: Option<PathBuf>) -> Self {
        self.audit_log = log;
        self
    }

    pub fn with_limits(mut self, l: DreamLimits) -> Self {
        self.limits = l;
        self
    }

    /// Battery / thermal (or any other) stop probe, e.g. [`power_probe`].
    pub fn with_conditions(mut self, f: impl Fn() -> Option<StopReason> + 'a) -> Self {
        self.conditions = Box::new(f);
        self
    }

    fn audit(&self, session: &Session, op: &str, reason: &str) -> bool {
        match crate::audit::record_dream(self.audit_log.as_deref(), op, reason, session.card()) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("dream audit ({op}): {e}; stopping the session");
                session.request_stop(StopReason::AuditFailed);
                false
            }
        }
    }

    fn say(&self, e: DreamEvent) {
        (self.announce)(&e);
    }

    /// Runs `session` until it stops; always ends it (`DREAM\tend`).
    pub fn run(self, session: Session) -> DreamReport {
        self.control.attach(&session);
        let st = self.control.status();
        self.say(DreamEvent::Started {
            session: session.id().to_string(),
            deadline: st.deadline.clone().unwrap_or_default(),
            deadline_local: st.deadline_local.clone().unwrap_or_default(),
        });
        let mut visited: Vec<String> = Vec::new();
        let mut saved: Vec<SavedFact> = Vec::new();
        let mut quarantined = 0usize;
        let mut jobs = 0usize;
        let mut pids = Vec::new();
        let mut pages: VecDeque<Instant> = VecDeque::new();
        let mut bytes = 0u64;
        let mut disk = 0u64;
        let mut failed = 0u32;
        let mut note: Option<String> = None;
        let stop_with = |session: &Session, why: &str, note: &mut Option<String>| {
            if note.is_none() {
                *note = Some(why.to_string());
            }
            session.stop();
        };
        let reason = loop {
            if let Some(r) = session.stop_reason() {
                break r;
            }
            if let Some(r) = (self.conditions)() {
                session.request_stop(r);
                continue;
            }
            let hour = Duration::from_secs(3600);
            while pages.front().is_some_and(|t| t.elapsed() >= hour) {
                pages.pop_front();
            }
            if pages.len() as u64 >= u64::from(self.limits.pages_per_hour) {
                // Hourly page cap: wait (stops still work at once).
                std::thread::sleep(self.tick);
                continue;
            }
            if bytes >= self.limits.bytes_per_session {
                stop_with(&session, "the session's byte budget is used up", &mut note);
                continue;
            }
            let Some(task) = self.planner.next() else {
                stop_with(&session, "nothing left to look up", &mut note);
                continue;
            };
            let topic = task.topic.clone();
            if !self.audit(
                &session,
                "going",
                &format!("topic={topic} urls={}", task.urls.join(",")),
            ) {
                continue;
            }
            self.control.update(|s| {
                s.where_now = Some(format!("{topic} — {}", hosts_of(&task.urls).join(", ")))
            });
            self.say(DreamEvent::Going {
                topic: topic.clone(),
                urls: task.urls.clone(),
            });
            let rep = self.sup.run(&task, &session);
            jobs += 1;
            pids.extend(rep.pids.iter().copied());
            for _ in 0..rep.pages {
                pages.push_back(Instant::now());
            }
            bytes = bytes.saturating_add(rep.bytes);
            for u in &rep.fetched {
                if !self.audit(&session, "visited", &format!("url={u}")) {
                    break;
                }
                visited.push(u.clone());
                self.control.update(|s| {
                    s.visited.push(u.clone());
                    if s.visited.len() > MAX_VISITED_SHOWN {
                        s.visited.remove(0);
                    }
                });
                self.say(DreamEvent::Visited { url: u.clone() });
            }
            if !rep.quarantined.is_empty() {
                quarantined += rep.quarantined.len();
                let why = rep
                    .kill
                    .clone()
                    .unwrap_or_else(|| "failed validation".into());
                self.audit(
                    &session,
                    "quarantined",
                    &format!("count={} why={why}", rep.quarantined.len()),
                );
                self.control
                    .update(|s| s.quarantined_count += rep.quarantined.len());
                self.say(DreamEvent::Quarantined {
                    count: rep.quarantined.len(),
                    why,
                });
            }
            for c in &rep.accepted {
                if session.stop_reason().is_some() {
                    // A stop discards in-flight work.
                    break;
                }
                if disk >= self.limits.disk_bytes {
                    stop_with(&session, "the session's disk budget is used up", &mut note);
                    break;
                }
                match self.sink.save(&topic, c, session.id()) {
                    Ok(f) => {
                        disk = disk.saturating_add(f.bytes);
                        if !self.audit(&session, "saved", &format!("note={} url={}", f.id, f.url)) {
                            break;
                        }
                        let item = DreamItem {
                            id: f.id.clone(),
                            title: f.title.clone(),
                            host: hosts_of(std::slice::from_ref(&f.url))
                                .pop()
                                .unwrap_or_default(),
                            path: f.path.to_string_lossy().into_owned(),
                        };
                        self.control.update(|s| {
                            s.saved_count += 1;
                            s.items.push(item);
                            if s.items.len() > MAX_ITEMS_SHOWN {
                                s.items.remove(0);
                            }
                        });
                        self.say(DreamEvent::Saved {
                            id: f.id.clone(),
                            title: f.title.clone(),
                            url: f.url.clone(),
                        });
                        saved.push(f);
                    }
                    Err(e) => {
                        tracing::info!("dream: not saved ({}): {e}", c.url);
                    }
                }
            }
            match rep.status {
                JobStatus::Offline => {
                    stop_with(&session, "Tor isn't answering", &mut note);
                }
                JobStatus::Failed | JobStatus::Refused | JobStatus::Killed => {
                    failed += 1;
                    if failed >= MAX_FAILED_IN_A_ROW {
                        stop_with(&session, "too many failed trips in a row", &mut note);
                    }
                }
                _ => failed = 0,
            }
        };
        self.control.update(|s| {
            s.state = DreamState::Ending;
            s.where_now = None;
        });
        self.sink.after_session();
        let reason = session.stop_reason().unwrap_or(reason);
        if let Err(e) = session.finish() {
            tracing::warn!("dream: end line: {e}");
        }
        let summary = summary_line(reason, note.as_deref(), &visited, &saved, quarantined);
        self.say(DreamEvent::Ended {
            reason,
            summary: summary.clone(),
        });
        self.control.detach(reason, summary.clone());
        DreamReport {
            session: session.id().to_string(),
            reason,
            note,
            visited,
            saved,
            quarantined,
            jobs,
            pids,
            summary,
        }
    }
}

/// The end-of-session summary: where she's been and what she saved.
pub fn summary_line(
    reason: StopReason,
    note: Option<&str>,
    visited: &[String],
    saved: &[SavedFact],
    quarantined: usize,
) -> String {
    let why = match reason {
        StopReason::EndEarly => "you woke me early".to_string(),
        StopReason::SessionDeadline => "time was up".to_string(),
        StopReason::KillSwitch => "the stop switch was pulled".to_string(),
        StopReason::LowBattery => "the battery got low".to_string(),
        StopReason::Thermal => "the CPU got too hot".to_string(),
        StopReason::AuditFailed => "I couldn't write the audit log".to_string(),
        StopReason::ParentExit => "the app was closing".to_string(),
        StopReason::Stop => note.unwrap_or("it was stopped").to_string(),
    };
    let mut s = format!(
        "Back from dreaming ({why}). Where I went: {}. What I saved: {}.",
        if visited.is_empty() {
            "nowhere".to_string()
        } else {
            hosts_of(visited).join(", ")
        },
        if saved.is_empty() {
            "nothing".to_string()
        } else {
            format!(
                "{} note{} ({})",
                saved.len(),
                if saved.len() == 1 { "" } else { "s" },
                saved
                    .iter()
                    .take(5)
                    .map(|f| format!("\"{}\"", f.title))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    );
    if quarantined > 0 {
        s.push_str(&format!(
            " {quarantined} result{} quarantined.",
            if quarantined == 1 { "" } else { "s" }
        ));
    }
    s
}
