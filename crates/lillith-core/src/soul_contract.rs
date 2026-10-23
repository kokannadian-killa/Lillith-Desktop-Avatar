//! The Soul Contract: a Yes/No gate in front of every change Lillith wants
//! to make to disk (Phase 1 of the companion plan).
//!
//! Lillith never writes on her own. When she drafts something — for now, a
//! new skill file — she files a [`PendingAction`] as a *request*. The request
//! sits in the [`SoulContract`] until a human answers:
//!
//! - [`SoulContract::confirm`] performs the action (atomic write) and appends
//!   a line to the audit log.
//! - [`SoulContract::deny`] drops it and records the refusal.
//!
//! This type owns only the request/confirm bookkeeping, the atomic write and
//! the audit trail; it does no network and no reads of the wider filesystem.
//! The UI that shows the Yes/No prompt lives above it (the chat bubble / a
//! future overlay popup), and simply calls `confirm`/`deny`.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Something Lillith wants to do that needs a human Yes before it happens.
///
/// Only [`PendingAction::WriteSkill`] exists today; later phases (gated file
/// writes, task runs, building programs) add their own variants, each routed
/// through the same confirm/deny gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingAction {
    /// Write a skill markdown file. `path` is already resolved (the caller
    /// confines it to the skills folder); `body` is the file contents.
    WriteSkill { path: PathBuf, body: String },
}

impl PendingAction {
    /// Short machine tag used in the audit log.
    pub fn kind(&self) -> &'static str {
        match self {
            PendingAction::WriteSkill { .. } => "write_skill",
        }
    }

    /// The path this action touches, if any.
    pub fn target_path(&self) -> Option<&Path> {
        match self {
            PendingAction::WriteSkill { path, .. } => Some(path),
        }
    }

    /// A one-line human description for a confirmation prompt.
    pub fn summary(&self) -> String {
        match self {
            PendingAction::WriteSkill { path, body } => {
                format!("Write skill {} ({} bytes)", path.display(), body.len())
            }
        }
    }
}

/// Opaque id for a filed request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestId(pub u64);

/// What a confirmed action produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmed {
    /// The action that ran.
    pub action: PendingAction,
    /// The path written, if the action wrote one.
    pub path: Option<PathBuf>,
}

/// Soul Contract errors.
#[derive(Debug, thiserror::Error)]
pub enum ContractError {
    #[error("no pending request with id {0:?}")]
    Unknown(RequestId),
    #[error("soul contract io: {0}")]
    Io(#[from] std::io::Error),
}

type Clock = Box<dyn Fn() -> SystemTime + Send + Sync>;

/// The pending-request queue, the confirm/deny gate and the audit log.
pub struct SoulContract {
    audit_log: Option<PathBuf>,
    pending: Vec<(RequestId, PendingAction)>,
    next_id: u64,
    clock: Clock,
}

impl std::fmt::Debug for SoulContract {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SoulContract")
            .field("audit_log", &self.audit_log)
            .field("pending", &self.pending)
            .field("next_id", &self.next_id)
            .finish_non_exhaustive()
    }
}

impl SoulContract {
    /// A contract that records confirmed/denied actions to `audit_log`
    /// (`None` = don't write an audit trail, e.g. in tests that don't check it).
    pub fn new(audit_log: Option<PathBuf>) -> Self {
        Self {
            audit_log,
            pending: Vec::new(),
            next_id: 1,
            clock: Box::new(SystemTime::now),
        }
    }

    /// Overrides the clock (tests use a fixed time for deterministic audit lines).
    pub fn with_clock(mut self, clock: impl Fn() -> SystemTime + Send + Sync + 'static) -> Self {
        self.clock = Box::new(clock);
        self
    }

    /// Files `action` for confirmation and returns its id. Nothing happens to
    /// disk yet.
    pub fn request(&mut self, action: PendingAction) -> RequestId {
        let id = RequestId(self.next_id);
        self.next_id += 1;
        self.pending.push((id, action));
        id
    }

    /// The requests still waiting for a Yes/No.
    pub fn pending(&self) -> &[(RequestId, PendingAction)] {
        &self.pending
    }

    /// The oldest pending request, if any (what a prompt should show first).
    pub fn front(&self) -> Option<&(RequestId, PendingAction)> {
        self.pending.first()
    }

    fn take(&mut self, id: RequestId) -> Result<PendingAction, ContractError> {
        let pos = self
            .pending
            .iter()
            .position(|(i, _)| *i == id)
            .ok_or(ContractError::Unknown(id))?;
        Ok(self.pending.remove(pos).1)
    }

    /// Answers Yes: runs the action, records it, and returns what it did.
    pub fn confirm(&mut self, id: RequestId) -> Result<Confirmed, ContractError> {
        let action = self.take(id)?;
        let path = match &action {
            PendingAction::WriteSkill { path, body } => {
                atomic_write(path, body.as_bytes())?;
                Some(path.clone())
            }
        };
        self.audit("CONFIRM", &action);
        Ok(Confirmed { action, path })
    }

    /// Answers No: drops the request and records the refusal.
    pub fn deny(&mut self, id: RequestId) -> Result<PendingAction, ContractError> {
        let action = self.take(id)?;
        self.audit("DENY", &action);
        Ok(action)
    }

    /// Appends `<rfc3339>\t<decision>\t<kind>\t<path>` to the audit log.
    /// A logging failure is reported but never blocks the decision.
    fn audit(&self, decision: &str, action: &PendingAction) {
        let Some(log) = &self.audit_log else { return };
        let ts = format_utc((self.clock)());
        let path = action
            .target_path()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let line = format!("{ts}\t{decision}\t{}\t{path}\n", action.kind());
        if let Err(e) = append(log, &line) {
            tracing::warn!(
                "soul contract: could not write audit log {}: {e}",
                log.display()
            );
        }
    }
}

/// Atomic file write: `path.tmp` then rename, creating the parent directory.
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = tmp_sibling(path);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// A `<name>.<pid>.tmp` sibling of `path`, so concurrent writers don't clash.
fn tmp_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "skill".into());
    let tmp_name = format!(".{name}.{}.tmp", std::process::id());
    match path.parent() {
        Some(dir) => dir.join(tmp_name),
        None => PathBuf::from(tmp_name),
    }
}

/// Appends `line` to `path`, creating it and its parent directory.
fn append(path: &Path, line: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(line.as_bytes())
}

/// Formats a [`SystemTime`] as RFC 3339 UTC (`YYYY-MM-DDTHH:MM:SSZ`), with no
/// external time dependency. Times before the Unix epoch format as the epoch.
fn format_utc(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Days since the Unix epoch → (year, month, day), Howard Hinnant's
/// `civil_from_days` (proleptic Gregorian).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "lillith-soul-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn fixed(secs: u64) -> impl Fn() -> SystemTime + Send + Sync {
        move || UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn utc_formatting_is_correct() {
        // 1_700_000_000 == 2023-11-14T22:13:20Z.
        assert_eq!(
            format_utc(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            "2023-11-14T22:13:20Z"
        );
        assert_eq!(format_utc(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        // A leap day.
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }

    #[test]
    fn confirm_writes_file_and_audits() {
        let dir = tmpdir("confirm");
        let skills = dir.join("skills");
        let log = dir.join("audit.log");
        let mut sc = SoulContract::new(Some(log.clone())).with_clock(fixed(1_700_000_000));
        let path = skills.join("greet.md");
        let id = sc.request(PendingAction::WriteSkill {
            path: path.clone(),
            body: "# Greet\n\nSay hi.\n".into(),
        });
        // Nothing on disk until confirmed.
        assert!(!path.exists());
        assert_eq!(sc.pending().len(), 1);
        let done = sc.confirm(id).unwrap();
        assert_eq!(done.path.as_deref(), Some(path.as_path()));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# Greet\n\nSay hi.\n"
        );
        assert!(sc.pending().is_empty());
        let audit = std::fs::read_to_string(&log).unwrap();
        assert!(audit.contains("CONFIRM"), "{audit}");
        assert!(audit.contains("write_skill"), "{audit}");
        assert!(audit.contains(&path.display().to_string()), "{audit}");
        assert!(audit.starts_with("2023-11-14T22:13:20Z\t"), "{audit}");
        // No stray temp files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(&skills)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp file left: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deny_discards_and_audits_without_writing() {
        let dir = tmpdir("deny");
        let log = dir.join("audit.log");
        let path = dir.join("skills").join("nope.md");
        let mut sc = SoulContract::new(Some(log.clone())).with_clock(fixed(0));
        let id = sc.request(PendingAction::WriteSkill {
            path: path.clone(),
            body: "secret".into(),
        });
        let denied = sc.deny(id).unwrap();
        assert!(matches!(denied, PendingAction::WriteSkill { .. }));
        assert!(!path.exists(), "deny must not write");
        assert!(sc.pending().is_empty());
        let audit = std::fs::read_to_string(&log).unwrap();
        assert!(
            audit.contains("DENY") && audit.contains("write_skill"),
            "{audit}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_id_is_an_error_and_ids_are_unique() {
        let mut sc = SoulContract::new(None);
        let a = sc.request(PendingAction::WriteSkill {
            path: PathBuf::from("/tmp/a.md"),
            body: "a".into(),
        });
        let b = sc.request(PendingAction::WriteSkill {
            path: PathBuf::from("/tmp/b.md"),
            body: "b".into(),
        });
        assert_ne!(a, b);
        assert_eq!(sc.front().map(|(i, _)| *i), Some(a));
        assert!(matches!(
            sc.confirm(RequestId(9999)),
            Err(ContractError::Unknown(_))
        ));
        // Denying `a` leaves `b` pending.
        sc.deny(a).unwrap();
        assert_eq!(sc.pending().len(), 1);
        assert_eq!(sc.front().map(|(i, _)| *i), Some(b));
    }

    #[test]
    fn audit_is_optional() {
        // No audit log configured: confirm still works, writes the file.
        let dir = tmpdir("noaudit");
        let path = dir.join("s.md");
        let mut sc = SoulContract::new(None);
        let id = sc.request(PendingAction::WriteSkill {
            path: path.clone(),
            body: "hi".into(),
        });
        sc.confirm(id).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hi");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
