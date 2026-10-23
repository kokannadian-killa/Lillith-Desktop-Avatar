//! The Soul Contract: a Yes/No gate in front of everything Lillith wants to
//! do to the machine (Phases 1-2 of the companion plan).
//!
//! Lillith never mutates anything on her own. When she wants to act — write a
//! skill, write a file, read a file, list a folder, search, or run a shell
//! command — she files a [`PendingAction`]. Actions split into two groups:
//!
//! - **Reads** ([`PendingAction::ListDir`], [`ReadFile`], [`SearchFiles`]) do
//!   not change anything. They may run straight away through
//!   [`SoulContract::run_now`], which executes them and audits an `AUTO` line.
//! - **Mutations** ([`PendingAction::WriteSkill`], [`WriteFile`],
//!   [`RunShell`]) always need a human Yes. They sit in the queue until
//!   [`SoulContract::confirm`] runs them (and audits `CONFIRM`) or
//!   [`SoulContract::deny`] drops them (and audits `DENY`).
//!
//! [`ReadFile`]: PendingAction::ReadFile
//! [`SearchFiles`]: PendingAction::SearchFiles
//! [`WriteFile`]: PendingAction::WriteFile
//! [`RunShell`]: PendingAction::RunShell
//!
//! This type owns the request/confirm bookkeeping, the actual IO and the
//! audit trail. It does **no network** and trusts that paths/commands were
//! already vetted against the allowlist by the caller (see
//! `lillith_mind::tools`). The UI that shows the Yes/No prompt lives above it
//! (the chat bubble / a future overlay popup) and simply calls
//! `confirm`/`deny`/`run_now`.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Something Lillith wants to do, routed through the Soul Contract.
///
/// Reads ([`ListDir`], [`ReadFile`], [`SearchFiles`]) change nothing and may
/// run via [`SoulContract::run_now`]. Mutations ([`WriteSkill`], [`WriteFile`],
/// [`RunShell`]) always need a human Yes (see [`PendingAction::needs_confirmation`]).
/// The caller is responsible for confining every `path`/`command` to the
/// allowlist before filing it here.
///
/// [`ListDir`]: PendingAction::ListDir
/// [`ReadFile`]: PendingAction::ReadFile
/// [`SearchFiles`]: PendingAction::SearchFiles
/// [`WriteSkill`]: PendingAction::WriteSkill
/// [`WriteFile`]: PendingAction::WriteFile
/// [`RunShell`]: PendingAction::RunShell
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingAction {
    /// Write a skill markdown file. `path` is already resolved (the caller
    /// confines it to the skills folder); `body` is the file contents.
    WriteSkill { path: PathBuf, body: String },
    /// List the entries directly inside `path` (read-only).
    ListDir { path: PathBuf },
    /// Read up to `max_bytes` from `path` (read-only).
    ReadFile { path: PathBuf, max_bytes: usize },
    /// Search the text files under `path` for `query`, returning up to
    /// `max_bytes` of matching lines (read-only).
    SearchFiles {
        path: PathBuf,
        query: String,
        max_bytes: usize,
    },
    /// Write `body` to `path`, atomically (mutating; needs a Yes).
    WriteFile { path: PathBuf, body: String },
    /// Run `command` through `sh -c` and capture its output (mutating; needs a
    /// Yes). Only ever filed when `[mind] allow_shell` is set.
    RunShell { command: String },
}

impl PendingAction {
    /// Short machine tag used in the audit log.
    pub fn kind(&self) -> &'static str {
        match self {
            PendingAction::WriteSkill { .. } => "write_skill",
            PendingAction::ListDir { .. } => "list_dir",
            PendingAction::ReadFile { .. } => "read_file",
            PendingAction::SearchFiles { .. } => "search_files",
            PendingAction::WriteFile { .. } => "write_file",
            PendingAction::RunShell { .. } => "run_shell",
        }
    }

    /// The path this action touches, if any (shell commands touch none).
    pub fn target_path(&self) -> Option<&Path> {
        match self {
            PendingAction::WriteSkill { path, .. }
            | PendingAction::ListDir { path }
            | PendingAction::ReadFile { path, .. }
            | PendingAction::SearchFiles { path, .. }
            | PendingAction::WriteFile { path, .. } => Some(path),
            PendingAction::RunShell { .. } => None,
        }
    }

    /// Whether this action mutates the machine and therefore needs an explicit
    /// human Yes. Reads return `false` and may go through [`SoulContract::run_now`].
    pub fn needs_confirmation(&self) -> bool {
        matches!(
            self,
            PendingAction::WriteSkill { .. }
                | PendingAction::WriteFile { .. }
                | PendingAction::RunShell { .. }
        )
    }

    /// A one-line human description for a confirmation prompt.
    pub fn summary(&self) -> String {
        match self {
            PendingAction::WriteSkill { path, body } => {
                format!("Write skill {} ({} bytes)", path.display(), body.len())
            }
            PendingAction::ListDir { path } => format!("List folder {}", path.display()),
            PendingAction::ReadFile { path, .. } => format!("Read file {}", path.display()),
            PendingAction::SearchFiles { path, query, .. } => {
                format!("Search {} for {:?}", path.display(), query)
            }
            PendingAction::WriteFile { path, body } => {
                format!("Write file {} ({} bytes)", path.display(), body.len())
            }
            PendingAction::RunShell { command } => format!("Run shell: {command}"),
        }
    }

    /// Performs the action's IO and returns the written path (for writes) and
    /// any textual output (for reads and shell). The caller must have already
    /// vetted the path/command against the allowlist.
    fn execute(&self) -> std::io::Result<(Option<PathBuf>, Option<String>)> {
        match self {
            PendingAction::WriteSkill { path, body } | PendingAction::WriteFile { path, body } => {
                atomic_write(path, body.as_bytes())?;
                Ok((Some(path.clone()), None))
            }
            PendingAction::ListDir { path } => Ok((None, Some(list_dir(path)?))),
            PendingAction::ReadFile { path, max_bytes } => {
                Ok((None, Some(read_file_capped(path, *max_bytes)?)))
            }
            PendingAction::SearchFiles {
                path,
                query,
                max_bytes,
            } => Ok((None, Some(search_files(path, query, *max_bytes)?))),
            PendingAction::RunShell { command } => Ok((None, Some(run_shell(command)?))),
        }
    }
}

/// Opaque id for a filed request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestId(pub u64);

/// What a run action produced (via [`SoulContract::confirm`] or
/// [`SoulContract::run_now`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmed {
    /// The action that ran.
    pub action: PendingAction,
    /// The path written, if the action wrote one.
    pub path: Option<PathBuf>,
    /// Captured output, for reads and shell runs.
    pub output: Option<String>,
}

/// Soul Contract errors.
#[derive(Debug, thiserror::Error)]
pub enum ContractError {
    #[error("no pending request with id {0:?}")]
    Unknown(RequestId),
    #[error("soul contract io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{} needs an explicit confirmation, not run_now", .0.kind())]
    NeedsConfirmation(PendingAction),
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
        let (path, output) = action.execute()?;
        self.audit("CONFIRM", &action);
        Ok(Confirmed {
            action,
            path,
            output,
        })
    }

    /// Runs a non-mutating (read) action straight away, without queuing it for
    /// a Yes/No, and audits an `AUTO` line. A mutating action is refused with
    /// [`ContractError::NeedsConfirmation`] so writes/shell can never slip
    /// through this path.
    pub fn run_now(&mut self, action: PendingAction) -> Result<Confirmed, ContractError> {
        if action.needs_confirmation() {
            return Err(ContractError::NeedsConfirmation(action));
        }
        let (path, output) = action.execute()?;
        self.audit("AUTO", &action);
        Ok(Confirmed {
            action,
            path,
            output,
        })
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

/// Text file extensions [`search_files`] will open.
const SEARCHABLE_EXTS: &[&str] = &[
    "md", "markdown", "txt", "rs", "toml", "json", "yaml", "yml", "cfg", "ini", "conf", "sh",
    "fish", "py", "log", "csv",
];

fn has_searchable_ext(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| SEARCHABLE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Lists the entries directly inside `dir` (one name per line, directories
/// marked with a trailing `/`), sorted. Not recursive.
fn list_dir(dir: &Path) -> std::io::Result<String> {
    let mut rows: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        rows.push(if is_dir { format!("{name}/") } else { name });
    }
    rows.sort();
    Ok(rows.join("\n"))
}

/// Reads at most `max_bytes` from `path`, as lossy UTF-8. A truncated read is
/// marked with a trailing notice.
fn read_file_capped(path: &Path, max_bytes: usize) -> std::io::Result<String> {
    use std::io::Read;
    let cap = max_bytes.max(1);
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; cap];
    let n = f.read(&mut buf)?;
    buf.truncate(n);
    let mut out = String::from_utf8_lossy(&buf).into_owned();
    // If there is more to read beyond the cap, say so.
    let mut probe = [0u8; 1];
    if f.read(&mut probe)? > 0 {
        out.push_str(&format!("\n…[truncated at {cap} bytes]"));
    }
    Ok(out)
}

/// Searches the text files at or under `path` for `query` (case-insensitive
/// substring), returning `path:line: text` rows up to `max_bytes` total.
/// `path` may be a single file or a directory (walked recursively).
fn search_files(path: &Path, query: &str, max_bytes: usize) -> std::io::Result<String> {
    let needle = query.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return Ok(String::new());
    }
    let cap = max_bytes.max(1);
    let mut out = String::new();
    let mut stack = vec![path.to_path_buf()];
    // Bound the walk so a huge tree can't hang the UI.
    let mut budget_files = 5_000usize;
    while let Some(p) = stack.pop() {
        if out.len() >= cap {
            out.push_str("\n…[more matches omitted]");
            break;
        }
        let meta = match std::fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.file_type().is_symlink() {
            continue; // don't follow symlinks out of the tree
        }
        if meta.is_dir() {
            let mut kids: Vec<PathBuf> = match std::fs::read_dir(&p) {
                Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
                Err(_) => continue,
            };
            kids.sort();
            // Push reversed so the sorted order is visited front-to-back.
            for k in kids.into_iter().rev() {
                stack.push(k);
            }
            continue;
        }
        if !meta.is_file() || !has_searchable_ext(&p) {
            continue;
        }
        if budget_files == 0 {
            break;
        }
        budget_files -= 1;
        let Ok(contents) = std::fs::read_to_string(&p) else {
            continue;
        };
        for (i, line) in contents.lines().enumerate() {
            if line.to_ascii_lowercase().contains(&needle) {
                let row = format!("{}:{}: {}\n", p.display(), i + 1, line.trim());
                if out.len() + row.len() > cap {
                    out.push_str("…[more matches omitted]");
                    return Ok(out);
                }
                out.push_str(&row);
            }
        }
    }
    Ok(out)
}

/// Runs `command` through `sh -c` and returns its combined stdout+stderr with
/// an exit-status line. Only ever reached after an explicit confirmation.
fn run_shell(command: &str) -> std::io::Result<String> {
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .output()?;
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&out.stdout));
    if !out.stderr.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&out.stderr));
    }
    let code = out
        .status
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "signal".into());
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!("[exit {code}]"));
    Ok(text)
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
    fn write_file_needs_confirm_and_runs_on_yes() {
        let dir = tmpdir("writefile");
        let log = dir.join("audit.log");
        let target = dir.join("notes/out.txt");
        let action = PendingAction::WriteFile {
            path: target.clone(),
            body: "hello phase 2\n".into(),
        };
        assert!(action.needs_confirmation());
        let mut sc = SoulContract::new(Some(log.clone())).with_clock(fixed(1_700_000_000));
        // run_now refuses a mutation.
        assert!(matches!(
            sc.run_now(action.clone()),
            Err(ContractError::NeedsConfirmation(_))
        ));
        assert!(!target.exists(), "run_now must not write a mutation");
        // Gated path: nothing until confirm.
        let id = sc.request(action);
        assert!(!target.exists());
        let done = sc.confirm(id).unwrap();
        assert_eq!(done.path.as_deref(), Some(target.as_path()));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello phase 2\n");
        let audit = std::fs::read_to_string(&log).unwrap();
        assert!(audit.contains("CONFIRM\twrite_file\t"), "{audit}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_run_now_and_audit_auto() {
        let dir = tmpdir("reads");
        let log = dir.join("audit.log");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), "alpha TODO here\nsecond line\n").unwrap();
        std::fs::write(dir.join("sub/b.md"), "# B\nnothing todo\n").unwrap();
        let mut sc = SoulContract::new(Some(log.clone()));

        // ListDir
        let ls = sc
            .run_now(PendingAction::ListDir { path: dir.clone() })
            .unwrap();
        let out = ls.output.unwrap();
        assert!(out.contains("a.txt") && out.contains("sub/"), "{out}");
        assert!(ls.path.is_none());

        // ReadFile with a tiny cap -> truncation notice.
        let rf = sc
            .run_now(PendingAction::ReadFile {
                path: dir.join("a.txt"),
                max_bytes: 5,
            })
            .unwrap();
        let body = rf.output.unwrap();
        assert!(body.starts_with("alpha"), "{body}");
        assert!(body.contains("truncated"), "{body}");

        // SearchFiles (recursive, case-insensitive).
        let sf = sc
            .run_now(PendingAction::SearchFiles {
                path: dir.clone(),
                query: "todo".into(),
                max_bytes: 10_000,
            })
            .unwrap();
        let hits = sf.output.unwrap();
        assert!(hits.contains("a.txt:1:"), "{hits}");
        assert!(hits.contains("b.md:2:"), "{hits}");

        // All three audited as AUTO, none as CONFIRM.
        let audit = std::fs::read_to_string(&log).unwrap();
        assert_eq!(audit.matches("AUTO").count(), 3, "{audit}");
        assert!(
            audit.contains("list_dir")
                && audit.contains("read_file")
                && audit.contains("search_files")
        );
        assert!(!audit.contains("CONFIRM"), "{audit}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_shell_needs_confirm_and_captures_output() {
        let dir = tmpdir("shell");
        let log = dir.join("audit.log");
        let action = PendingAction::RunShell {
            command: "echo phase2 && exit 3".into(),
        };
        assert!(action.needs_confirmation());
        let mut sc = SoulContract::new(Some(log.clone()));
        assert!(matches!(
            sc.run_now(action.clone()),
            Err(ContractError::NeedsConfirmation(_))
        ));
        let id = sc.request(action);
        let done = sc.confirm(id).unwrap();
        let out = done.output.unwrap();
        assert!(out.contains("phase2"), "{out}");
        assert!(out.contains("[exit 3]"), "{out}");
        assert!(done.path.is_none());
        let audit = std::fs::read_to_string(&log).unwrap();
        assert!(audit.contains("CONFIRM\trun_shell\t"), "{audit}");
        let _ = std::fs::remove_dir_all(&dir);
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
