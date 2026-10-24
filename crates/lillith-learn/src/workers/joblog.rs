//! The D8 job log (docs §8, §11.5, §12): one append-only JSONL file per job,
//! `~/.local/share/lillith/bots/<job-id>.jsonl`, fsync per line. Each line:
//! `id`, `bot`, `task`, `status`, `ts`, `result_path` (D8), plus `backend`,
//! `session` and `attempt`. Not a learning source.
//!
//! Crash-safe and resumable: after a crash, [`JobLog::recover`] repairs a
//! torn last line and, for every job whose last status is not final,
//! either hands it back for resuming (same live session: its consent still
//! holds) or closes it as `CANCELLED`.

use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

/// A job's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    Running,
    Restarting,
    Done,
    /// Ended early by a budget (TTL); validated results kept.
    Partial,
    /// Out of restarts.
    Failed,
    /// Killed on an anomaly; results quarantined.
    Killed,
    /// End early, a stop, the kill switch, the deadline, parent exit,
    /// battery, thermal (never the safeword: it only hushes her voice).
    Cancelled,
    /// Tor down: nothing spawned (offline-only learning continues).
    Offline,
    /// A gate said no before anything ran.
    Refused,
}

impl JobStatus {
    pub fn word(self) -> &'static str {
        match self {
            JobStatus::Queued => "QUEUED",
            JobStatus::Running => "RUNNING",
            JobStatus::Restarting => "RESTARTING",
            JobStatus::Done => "DONE",
            JobStatus::Partial => "PARTIAL",
            JobStatus::Failed => "FAILED",
            JobStatus::Killed => "KILLED",
            JobStatus::Cancelled => "CANCELLED",
            JobStatus::Offline => "OFFLINE",
            JobStatus::Refused => "REFUSED",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [
            JobStatus::Queued,
            JobStatus::Running,
            JobStatus::Restarting,
            JobStatus::Done,
            JobStatus::Partial,
            JobStatus::Failed,
            JobStatus::Killed,
            JobStatus::Cancelled,
            JobStatus::Offline,
            JobStatus::Refused,
        ]
        .into_iter()
        .find(|j| j.word() == s)
    }

    pub fn is_final(self) -> bool {
        !matches!(
            self,
            JobStatus::Queued | JobStatus::Running | JobStatus::Restarting
        )
    }
}

/// `w-` then 1–48 of `[a-z0-9-]` (never a path).
pub fn job_id_ok(id: &str) -> bool {
    id.strip_prefix("w-").is_some_and(|r| {
        (1..=48).contains(&r.len())
            && r.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

/// A fresh job id.
pub fn new_job_id() -> String {
    format!("w-{:016x}", super::random_u64())
}

/// One line.
#[derive(Debug, Clone, PartialEq)]
pub struct JobEntry {
    pub id: String,
    pub bot: String,
    pub task: Value,
    pub status: JobStatus,
    pub ts: String,
    pub result_path: Option<String>,
    pub backend: String,
    pub session: String,
    pub attempt: u32,
}

impl JobEntry {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "bot": self.bot,
            "task": self.task,
            "status": self.status.word(),
            "ts": self.ts,
            "result_path": self.result_path,
            "backend": self.backend,
            "session": self.session,
            "attempt": self.attempt,
        })
    }

    pub fn from_json(v: &Value) -> Option<Self> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        Some(Self {
            id: s("id")?,
            bot: s("bot")?,
            task: v.get("task")?.clone(),
            status: JobStatus::parse(&s("status")?)?,
            ts: s("ts")?,
            result_path: s("result_path"),
            backend: s("backend").unwrap_or_else(|| "local".into()),
            session: s("session").unwrap_or_default(),
            attempt: v.get("attempt").and_then(Value::as_u64).unwrap_or(0) as u32,
        })
    }
}

/// The job log folder.
#[derive(Debug, Clone)]
pub struct JobLog {
    dir: Option<PathBuf>,
}

impl JobLog {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self { dir }
    }

    /// `<data>/lillith/bots` (`~/.local/share/lillith/bots`).
    pub fn default_dir() -> Option<PathBuf> {
        lillith_core::config::data_dir().map(|d| d.join("bots"))
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    pub fn path(&self, id: &str) -> std::io::Result<PathBuf> {
        let Some(d) = &self.dir else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no job log folder",
            ));
        };
        if !job_id_ok(id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "bad job id",
            ));
        }
        Ok(d.join(format!("{id}.jsonl")))
    }

    /// Appends one line, synced to disk (dir 0700, file 0600). A torn last
    /// line from a crash is closed first so the new line stays whole.
    pub fn append(&self, e: &JobEntry) -> std::io::Result<()> {
        let p = self.path(&e.id)?;
        if let Some(d) = p.parent() {
            let mut b = std::fs::DirBuilder::new();
            b.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                b.mode(0o700);
            }
            b.create(d)?;
        }
        let torn = std::fs::read(&p).is_ok_and(|b| !b.is_empty() && b.last() != Some(&b'\n'));
        let mut o = std::fs::OpenOptions::new();
        o.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let mut f = o.open(&p)?;
        let mut l = String::new();
        if torn {
            l.push('\n');
        }
        l.push_str(&e.to_json().to_string());
        l.push('\n');
        f.write_all(l.as_bytes())?;
        f.sync_data()
    }

    /// Every whole, valid line of one job.
    pub fn read(&self, id: &str) -> Vec<JobEntry> {
        let Ok(p) = self.path(id) else {
            return Vec::new();
        };
        std::fs::read_to_string(p)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter_map(|v| JobEntry::from_json(&v))
            .collect()
    }

    /// After a restart: unfinished jobs of `live_session` are returned for
    /// resuming; every other unfinished job is closed as `CANCELLED`.
    pub fn recover(&self, live_session: Option<&str>) -> std::io::Result<Vec<JobEntry>> {
        let Some(d) = &self.dir else {
            return Ok(Vec::new());
        };
        let rd = match std::fs::read_dir(d) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut resume = Vec::new();
        for ent in rd.flatten() {
            let name = ent.file_name().to_string_lossy().to_string();
            let Some(id) = name.strip_suffix(".jsonl") else {
                continue;
            };
            if !job_id_ok(id) {
                continue;
            }
            let Some(last) = self.read(id).pop() else {
                continue;
            };
            if last.status.is_final() {
                continue;
            }
            if live_session.is_some_and(|s| !s.is_empty() && s == last.session) {
                resume.push(last);
            } else {
                self.append(&JobEntry {
                    status: JobStatus::Cancelled,
                    ts: crate::audit::utc(crate::now_s()),
                    result_path: None,
                    ..last
                })?;
            }
        }
        resume.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(resume)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, status: JobStatus, session: &str) -> JobEntry {
        JobEntry {
            id: id.into(),
            bot: "local:research-worker".into(),
            task: json!({"kind": "research", "topic": "rust traits", "urls": ["https://docs.example.org/a"]}),
            status,
            ts: "2026-10-24T17:00:00Z".into(),
            result_path: None,
            backend: "local".into(),
            session: session.into(),
            attempt: 0,
        }
    }

    fn dir() -> PathBuf {
        std::env::temp_dir().join(format!("lillith-bots-{}", super::super::random_token()))
    }

    #[test]
    fn d8_line_shape_append_only_and_private() {
        let d = dir();
        let log = JobLog::new(Some(d.clone()));
        log.append(&entry("w-1", JobStatus::Queued, "s")).unwrap();
        log.append(&entry("w-1", JobStatus::Done, "s")).unwrap();
        let text = std::fs::read_to_string(d.join("w-1.jsonl")).unwrap();
        assert_eq!(text.lines().count(), 2);
        let v: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        for k in ["id", "bot", "task", "status", "ts", "result_path"] {
            assert!(v.get(k).is_some(), "D8 field {k}");
        }
        assert_eq!(v["status"], "QUEUED");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(d.join("w-1.jsonl"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert!(log
            .append(&entry("../../etc/x", JobStatus::Queued, "s"))
            .is_err());
        assert_eq!(
            JobLog::default_dir().map(|p| p.ends_with("lillith/bots")),
            lillith_core::config::data_dir().map(|_| true)
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn recover_resumes_same_session_and_cancels_the_rest_after_a_torn_line() {
        let d = dir();
        let log = JobLog::new(Some(d.clone()));
        log.append(&entry("w-a", JobStatus::Running, "s1")).unwrap();
        log.append(&entry("w-b", JobStatus::Running, "s0")).unwrap();
        log.append(&entry("w-c", JobStatus::Done, "s1")).unwrap();
        // A crash mid-write: half a line, no newline.
        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(d.join("w-b.jsonl"))
                .unwrap();
            f.write_all(br#"{"id":"w-b","sta"#).unwrap();
        }
        let r = log.recover(Some("s1")).unwrap();
        assert_eq!(r.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["w-a"]);
        let b = log.read("w-b");
        assert_eq!(
            b.last().unwrap().status,
            JobStatus::Cancelled,
            "other session closed"
        );
        assert_eq!(b.len(), 2, "torn line skipped, new line whole");
        assert_eq!(log.read("w-c").len(), 1, "finished jobs untouched");
        // No live session (app restart): everything unfinished closes.
        let r = log.recover(None).unwrap();
        assert!(r.is_empty());
        assert_eq!(log.read("w-a").last().unwrap().status, JobStatus::Cancelled);
        let _ = std::fs::remove_dir_all(&d);
    }
}
