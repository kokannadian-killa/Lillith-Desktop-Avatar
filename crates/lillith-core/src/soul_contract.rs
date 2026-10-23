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
//! `lillith_mind::tools`). The UI that shows the Yes/No prompt lives above it:
//! every egui surface raises the shared Soul Contract popup
//! (`lillith_theme::egui_contract`) from a [`ContractPrompt`]
//! ([`PendingAction::prompt`]) and answers through [`SoulContract::answer`]
//! (or [`decide`]): **Yes** runs the action and audits `CONFIRM`, **No**
//! discards it and audits `DENY`.

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
    /// Scaffold a new local project into `dir` (mutating; needs a Yes). Uses
    /// offline, already-installed tools only; see [`crate::build`]. The caller
    /// confines `dir` to the projects root (see [`crate::build::resolve_project_dir`]).
    ScaffoldProject {
        dir: PathBuf,
        language: crate::build::Language,
        template: String,
    },
    /// Build (compile / syntax-check) the local project in `dir`, offline
    /// (mutating — it can touch `target/` / `__pycache__`; needs a Yes).
    BuildProject {
        dir: PathBuf,
        language: crate::build::Language,
    },
    /// Run the local project in `dir`, offline (mutating; needs a Yes).
    RunProject {
        dir: PathBuf,
        language: crate::build::Language,
    },
    /// Move / rename / copy / make folders (non-destructive, journaled for
    /// undo). Needs a Yes only when the batch has more than `preview_over`
    /// steps (the human sees the plan first). `journal` is the undo journal
    /// folder; Lillith's data folder (its parent) is protected.
    Organize {
        plan: crate::access::organize::OrganizePlan,
        journal: PathBuf,
        preview_over: usize,
    },
    /// Delete exactly the listed files and folders. The plain Yes
    /// ([`SoulContract::confirm`]) refuses it: only
    /// [`SoulContract::sign_deletion`] with the human's typed signature runs
    /// it.
    DeletePaths {
        manifest: crate::access::deletion::DeletionManifest,
    },
    /// Run a CLI command with captured output. Read-only commands (see
    /// [`crate::access::commands::classify`]) run without asking; anything else
    /// needs a Yes on a card showing the exact command; file removal is
    /// refused (use [`PendingAction::DeletePaths`]).
    RunCommand {
        command: String,
        cwd: Option<PathBuf>,
        timeout_s: u64,
    },
    /// Open a program. `app` is the desktop-entry id when it is an installed
    /// app (runs without asking); a bare command line needs a Yes.
    Launch {
        argv: Vec<String>,
        app: Option<String>,
    },
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
            PendingAction::ScaffoldProject { .. } => "scaffold_project",
            PendingAction::BuildProject { .. } => "build_project",
            PendingAction::RunProject { .. } => "run_project",
            PendingAction::Organize { .. } => "organize",
            PendingAction::DeletePaths { .. } => "delete_paths",
            PendingAction::RunCommand { .. } => "run_command",
            PendingAction::Launch { .. } => "launch",
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
            PendingAction::ScaffoldProject { dir, .. }
            | PendingAction::BuildProject { dir, .. }
            | PendingAction::RunProject { dir, .. } => Some(dir),
            PendingAction::RunCommand { cwd, .. } => cwd.as_deref(),
            PendingAction::DeletePaths { manifest } => manifest.roots.first().map(PathBuf::as_path),
            PendingAction::RunShell { .. }
            | PendingAction::Organize { .. }
            | PendingAction::Launch { .. } => None,
        }
    }

    /// Whether this action mutates the machine and therefore needs an explicit
    /// human Yes. Reads return `false` and may go through [`SoulContract::run_now`].
    pub fn needs_confirmation(&self) -> bool {
        match self {
            PendingAction::WriteSkill { .. }
            | PendingAction::WriteFile { .. }
            | PendingAction::RunShell { .. }
            | PendingAction::ScaffoldProject { .. }
            | PendingAction::BuildProject { .. }
            | PendingAction::RunProject { .. }
            | PendingAction::DeletePaths { .. } => true,
            PendingAction::Organize {
                plan, preview_over, ..
            } => plan.len() > *preview_over,
            PendingAction::RunCommand { command, .. } => {
                !crate::access::commands::classify(command, &crate::access::secret_roots())
                    .is_safe()
            }
            PendingAction::Launch { app, .. } => app.is_none(),
            PendingAction::ListDir { .. }
            | PendingAction::ReadFile { .. }
            | PendingAction::SearchFiles { .. } => false,
        }
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
            PendingAction::ScaffoldProject {
                dir,
                language,
                template,
            } => crate::build::scaffold_summary(dir, *language, template),
            PendingAction::BuildProject { dir, language } => {
                crate::build::build_summary(dir, *language)
            }
            PendingAction::RunProject { dir, language } => {
                crate::build::run_summary(dir, *language)
            }
            PendingAction::Organize { plan, .. } => format!(
                "Organize: {} step{} (nothing is overwritten; undo anytime)",
                plan.len(),
                if plan.len() == 1 { "" } else { "s" }
            ),
            PendingAction::DeletePaths { manifest } => {
                format!("Delete {}", manifest.totals())
            }
            PendingAction::RunCommand { command, .. } => format!("Run command: {command}"),
            PendingAction::Launch { argv, app } => match app {
                Some(a) => format!("Open {a}: {}", argv.join(" ")),
                None => format!("Start program: {}", argv.join(" ")),
            },
        }
    }

    /// Short upper-case heading for the confirmation popup.
    pub fn title(&self) -> &'static str {
        match self {
            PendingAction::WriteSkill { .. } => "WRITE SKILL",
            PendingAction::ListDir { .. } => "LIST FOLDER",
            PendingAction::ReadFile { .. } => "READ FILE",
            PendingAction::SearchFiles { .. } => "SEARCH FILES",
            PendingAction::WriteFile { .. } => "WRITE FILE",
            PendingAction::RunShell { .. } => "RUN SHELL",
            PendingAction::ScaffoldProject { .. } => "SCAFFOLD PROJECT",
            PendingAction::BuildProject { .. } => "BUILD PROJECT",
            PendingAction::RunProject { .. } => "RUN PROJECT",
            PendingAction::Organize { .. } => "ORGANIZE FILES",
            PendingAction::DeletePaths { .. } => "DELETE — SIGN THE LIST",
            PendingAction::RunCommand { .. } => "RUN COMMAND",
            PendingAction::Launch { .. } => "OPEN PROGRAM",
        }
    }

    /// The toolkit-free text a Yes/No popup shows for this action: heading,
    /// one-line summary, the touched path and a capped preview of what would
    /// be written / run. Callers may swap in a richer preview with
    /// [`ContractPrompt::with_detail`] (the Build tab shows the exact
    /// commands).
    pub fn prompt(&self) -> ContractPrompt {
        let detail = match self {
            PendingAction::WriteSkill { body, .. } | PendingAction::WriteFile { body, .. } => {
                Some(cap_preview(body, PROMPT_DETAIL_MAX))
            }
            PendingAction::RunShell { command } => {
                Some(format!("$ {}", cap_preview(command, PROMPT_DETAIL_MAX)))
            }
            PendingAction::RunCommand { command, .. } => {
                let why = match crate::access::commands::classify(
                    command,
                    &crate::access::secret_roots(),
                ) {
                    crate::access::commands::CommandClass::NeedsApproval(r) => {
                        format!("\n\nwhy I'm asking: {r}")
                    }
                    _ => String::new(),
                };
                Some(format!("$ {command}{why}"))
            }
            PendingAction::Launch { argv, .. } => Some(format!("$ {}", argv.join(" "))),
            PendingAction::Organize { plan, .. } => Some(plan.preview()),
            // The full listing: every file and folder, never elided.
            PendingAction::DeletePaths { manifest } => Some(manifest.listing()),
            _ => None,
        };
        ContractPrompt {
            kind: self.kind(),
            title: self.title(),
            summary: self.summary(),
            target: self.target_path().map(|p| p.display().to_string()),
            detail,
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
            PendingAction::RunShell { command } => {
                refuse_removal(command)?;
                Ok((None, Some(run_shell(command)?)))
            }
            PendingAction::RunCommand {
                command,
                cwd,
                timeout_s,
            } => {
                refuse_removal(command)?;
                Ok((
                    None,
                    Some(crate::access::commands::run_captured(
                        command,
                        cwd.as_deref(),
                        std::time::Duration::from_secs((*timeout_s).max(1)),
                        COMMAND_OUTPUT_MAX,
                    )?),
                ))
            }
            PendingAction::Launch { argv, .. } => {
                let pid = crate::access::programs::spawn_detached(argv)?;
                Ok((
                    None,
                    Some(format!("started {} (pid {pid})", argv.join(" "))),
                ))
            }
            PendingAction::Organize { plan, journal, .. } => {
                let rep = crate::access::organize::execute(plan, journal, journal.parent())?;
                Ok((None, Some(rep.text())))
            }
            PendingAction::DeletePaths { .. } => Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "deleting needs a signed Soul Contract",
            )),
            PendingAction::ScaffoldProject {
                dir,
                language,
                template,
            } => Ok((
                Some(dir.clone()),
                Some(crate::build::scaffold(dir, *language, template)?),
            )),
            PendingAction::BuildProject { dir, language } => {
                Ok((None, Some(crate::build::build(dir, *language)?)))
            }
            PendingAction::RunProject { dir, language } => {
                Ok((None, Some(crate::build::run(dir, *language)?)))
            }
        }
    }
}

/// Most bytes of command output kept.
pub const COMMAND_OUTPUT_MAX: usize = 16 * 1024;

/// Refuses a command that deletes files (that goes through the delete contract).
fn refuse_removal(command: &str) -> std::io::Result<()> {
    if let crate::access::commands::CommandClass::Removal(r) =
        crate::access::commands::classify(command, &[])
    {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, r));
    }
    Ok(())
}

/// Longest preview (bytes) a [`ContractPrompt`] carries for a body / command.
pub const PROMPT_DETAIL_MAX: usize = 1200;

/// `text` cut to at most `max` bytes on a char boundary, with a note of how
/// much was left out.
fn cap_preview(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…[{} more bytes]", &text[..end], text.len() - end)
}

/// What the Yes/No popup shows for a [`PendingAction`] (see
/// [`PendingAction::prompt`]). Plain text only, so any toolkit can draw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractPrompt {
    /// The audit tag ([`PendingAction::kind`]).
    pub kind: &'static str,
    /// Upper-case heading ([`PendingAction::title`]).
    pub title: &'static str,
    /// One-line description ([`PendingAction::summary`]).
    pub summary: String,
    /// The path the action touches, if any.
    pub target: Option<String>,
    /// What would be written / run, capped at [`PROMPT_DETAIL_MAX`].
    pub detail: Option<String>,
}

impl ContractPrompt {
    /// Replaces the preview (e.g. the Build tab's exact command list).
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        let d = detail.into();
        self.detail = (!d.trim().is_empty()).then_some(d);
        self
    }
}

/// The human's answer to a Soul Contract prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// Run it (audits `CONFIRM`).
    Yes,
    /// Discard it (audits `DENY`).
    No,
}

impl Answer {
    /// `true` → [`Answer::Yes`].
    pub fn from_yes(yes: bool) -> Self {
        if yes {
            Answer::Yes
        } else {
            Answer::No
        }
    }
}

/// How an answered request ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Yes: the action ran (and was audited `CONFIRM`).
    Confirmed(Confirmed),
    /// No: the action was discarded (and audited `DENY`).
    Denied(PendingAction),
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
    /// A deletion was answered with a plain Yes: it needs the signed list.
    #[error("deleting needs your signature on the full list (request {0:?})")]
    NeedsSignature(RequestId),
    /// [`SoulContract::sign_deletion`] on something that is not a deletion.
    #[error("request {0:?} is not a deletion")]
    NotADeletion(RequestId),
    /// The signature was made for a different list. Nothing was deleted and
    /// the request is still pending.
    #[error("that signature is for a different list; nothing was deleted")]
    WrongSet,
    /// The files changed since they were listed. Nothing was deleted; the new
    /// listing was filed as `new_id` and must be signed again.
    #[error("the files changed since you saw the list; nothing was deleted — please review the new list")]
    Changed { new_id: RequestId },
    /// The paths can no longer be listed (gone / now protected).
    #[error("refused: {0}")]
    Refused(String),
}

type Clock = Box<dyn Fn() -> SystemTime + Send + Sync>;

/// The pending-request queue, the confirm/deny gate and the audit log.
pub struct SoulContract {
    audit_log: Option<PathBuf>,
    trash_dir: Option<PathBuf>,
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
            trash_dir: None,
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

    /// Uses `dir` as the Trash instead of `$XDG_DATA_HOME/Trash` (tests).
    pub fn with_trash_dir(mut self, dir: PathBuf) -> Self {
        self.trash_dir = Some(dir);
        self
    }

    /// Lillith's data folder (the audit log's folder); protected from
    /// organize/delete. Also holds `commands.log` and `deletions.log`.
    pub fn data_dir(&self) -> Option<&Path> {
        self.audit_log.as_deref().and_then(Path::parent)
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
    /// A deletion is refused with [`ContractError::NeedsSignature`] and stays
    /// pending: it only runs through [`sign_deletion`](Self::sign_deletion).
    pub fn confirm(&mut self, id: RequestId) -> Result<Confirmed, ContractError> {
        if let Some((_, PendingAction::DeletePaths { .. })) =
            self.pending.iter().find(|(i, _)| *i == id)
        {
            return Err(ContractError::NeedsSignature(id));
        }
        let action = self.take(id)?;
        let res = action.execute();
        self.log_command("CONFIRM", &action, &res);
        let (path, output) = res?;
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
        let res = action.execute();
        self.log_command("AUTO", &action, &res);
        let (path, output) = res?;
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
        self.log_command("DENY", &action, &Ok((None, None)));
        Ok(action)
    }

    /// Runs a deletion the human signed. The signature must be for exactly
    /// this request's list (else [`ContractError::WrongSet`], still pending).
    /// The paths are listed again first: if anything changed, nothing is
    /// deleted, the new list is filed and [`ContractError::Changed`] returned
    /// so the human is asked again. The signature is consumed either way.
    /// Every removed path goes to `deletions.log`.
    pub fn sign_deletion(
        &mut self,
        id: RequestId,
        signature: crate::access::deletion::DeletionSignature,
    ) -> Result<Confirmed, ContractError> {
        use crate::access::deletion;
        let Some((_, action)) = self.pending.iter().find(|(i, _)| *i == id) else {
            return Err(ContractError::Unknown(id));
        };
        let PendingAction::DeletePaths { manifest } = action else {
            return Err(ContractError::NotADeletion(id));
        };
        if signature.fingerprint() != manifest.fingerprint() {
            return Err(ContractError::WrongSet);
        }
        let action = self.take(id)?;
        let PendingAction::DeletePaths { manifest } = &action else {
            unreachable!()
        };
        let data = self.data_dir().map(Path::to_path_buf);
        match manifest.relist(data.as_deref()) {
            Ok(now) if now.fingerprint() == manifest.fingerprint() => {}
            Ok(now) => {
                self.audit("CHANGED", &action);
                let new_id = self.request(PendingAction::DeletePaths { manifest: now });
                return Err(ContractError::Changed { new_id });
            }
            Err(e) => {
                self.audit("REFUSED", &action);
                return Err(ContractError::Refused(e.to_string()));
            }
        }
        let mode = signature.mode();
        let trash = self
            .trash_dir
            .clone()
            .unwrap_or_else(deletion::home_trash_dir);
        let (done, err) = deletion::execute_in(manifest, mode, &trash);
        self.audit(
            &format!("CONFIRM-SIGNED-{}", mode.as_str().to_uppercase()),
            &action,
        );
        if let Some(d) = &data {
            let log = d.join("deletions.log");
            let ts = format_utc((self.clock)());
            for line in &done {
                let _ =
                    crate::access::append_line(&log, &format!("{ts}\t{}\t{line}", mode.as_str()));
            }
            if let Some(e) = &err {
                let _ = crate::access::append_line(&log, &format!("{ts}\tERROR\t{e}"));
            }
        }
        if let Some(e) = err {
            return Err(ContractError::Io(std::io::Error::new(
                e.kind(),
                format!("stopped after {} item(s): {e}", done.len()),
            )));
        }
        let mut out = done.join("\n");
        out.push_str(&format!(
            "\n— deleted {} ({})",
            manifest.totals(),
            mode.as_str()
        ));
        Ok(Confirmed {
            action,
            path: None,
            output: Some(out),
        })
    }

    /// Records commands / launches in `commands.log` next to the audit log.
    fn log_command(
        &self,
        decision: &str,
        action: &PendingAction,
        res: &std::io::Result<(Option<PathBuf>, Option<String>)>,
    ) {
        let cmd = match action {
            PendingAction::RunCommand { command, .. } | PendingAction::RunShell { command } => {
                command.clone()
            }
            PendingAction::Launch { argv, .. } => format!("[launch] {}", argv.join(" ")),
            _ => return,
        };
        let Some(d) = self.data_dir() else { return };
        let result = match res {
            Ok((_, Some(o))) => o.lines().last().unwrap_or("").to_string(),
            Ok(_) => String::new(),
            Err(e) => format!("error: {e}"),
        };
        crate::access::commands::log_command(&d.join("commands.log"), decision, &cmd, &result);
    }

    /// Answers the popup: [`Answer::Yes`] is [`confirm`](Self::confirm),
    /// [`Answer::No`] is [`deny`](Self::deny). There is no third path.
    pub fn answer(&mut self, id: RequestId, answer: Answer) -> Result<Verdict, ContractError> {
        match answer {
            Answer::Yes => self.confirm(id).map(Verdict::Confirmed),
            Answer::No => self.deny(id).map(Verdict::Denied),
        }
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

/// One-shot gate for a UI that held `action` behind the popup: files it with
/// a contract bound to `audit_log` and answers it straight away. Yes runs it
/// and audits `CONFIRM`; No discards it and audits `DENY`.
pub fn decide(
    audit_log: Option<PathBuf>,
    action: PendingAction,
    answer: Answer,
) -> Result<Verdict, ContractError> {
    let mut sc = SoulContract::new(audit_log);
    let id = sc.request(action);
    sc.answer(id, answer)
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
pub(crate) fn format_utc(t: SystemTime) -> String {
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
pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
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
    fn scaffold_project_needs_confirm_and_runs_on_yes() {
        use crate::build::Language;
        let dir = tmpdir("scaffold");
        let log = dir.join("audit.log");
        let project = dir.join("builds").join("demo");
        let action = PendingAction::ScaffoldProject {
            dir: project.clone(),
            language: Language::Python,
            template: "script".into(),
        };
        assert!(action.needs_confirmation());
        assert_eq!(action.kind(), "scaffold_project");
        assert_eq!(action.target_path(), Some(project.as_path()));
        let mut sc = SoulContract::new(Some(log.clone()));
        // run_now refuses a mutation.
        assert!(matches!(
            sc.run_now(action.clone()),
            Err(ContractError::NeedsConfirmation(_))
        ));
        assert!(!project.exists(), "run_now must not scaffold");
        // Gated path: nothing until confirm.
        let id = sc.request(action);
        assert!(!project.exists());
        let done = sc.confirm(id).unwrap();
        assert_eq!(done.path.as_deref(), Some(project.as_path()));
        assert!(project.join("main.py").exists(), "scaffold wrote main.py");
        let audit = std::fs::read_to_string(&log).unwrap();
        assert!(audit.contains("CONFIRM\tscaffold_project\t"), "{audit}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn answer_routes_yes_to_confirm_and_no_to_deny() {
        let dir = tmpdir("answer");
        let log = dir.join("audit.log");
        let yes_path = dir.join("yes.txt");
        let no_path = dir.join("no.txt");
        let mut sc = SoulContract::new(Some(log.clone())).with_clock(fixed(0));
        let y = sc.request(PendingAction::WriteFile {
            path: yes_path.clone(),
            body: "y".into(),
        });
        let n = sc.request(PendingAction::WriteFile {
            path: no_path.clone(),
            body: "n".into(),
        });
        assert!(!yes_path.exists() && !no_path.exists());
        match sc.answer(n, Answer::No).unwrap() {
            Verdict::Denied(a) => assert_eq!(a.kind(), "write_file"),
            v => panic!("{v:?}"),
        }
        assert!(!no_path.exists(), "No must not write");
        match sc.answer(y, Answer::Yes).unwrap() {
            Verdict::Confirmed(c) => assert_eq!(c.path.as_deref(), Some(yes_path.as_path())),
            v => panic!("{v:?}"),
        }
        assert_eq!(std::fs::read_to_string(&yes_path).unwrap(), "y");
        let audit = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = audit.lines().collect();
        assert_eq!(lines.len(), 2, "{audit}");
        assert!(lines[0].contains("\tDENY\twrite_file\t"), "{audit}");
        assert!(lines[1].contains("\tCONFIRM\twrite_file\t"), "{audit}");
        // Answered ids are gone: a second answer is refused.
        assert!(matches!(
            sc.answer(y, Answer::Yes),
            Err(ContractError::Unknown(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn decide_is_a_one_shot_gate() {
        let dir = tmpdir("decide");
        let log = dir.join("audit.log");
        let action = PendingAction::RunShell {
            command: "echo popup".into(),
        };
        let v = decide(Some(log.clone()), action.clone(), Answer::No).unwrap();
        assert_eq!(v, Verdict::Denied(action.clone()));
        let v = decide(Some(log.clone()), action, Answer::Yes).unwrap();
        match v {
            Verdict::Confirmed(c) => assert!(c.output.unwrap().contains("popup")),
            v => panic!("{v:?}"),
        }
        let audit = std::fs::read_to_string(&log).unwrap();
        assert!(audit.contains("DENY\trun_shell"), "{audit}");
        assert!(audit.contains("CONFIRM\trun_shell"), "{audit}");
        assert_eq!(Answer::from_yes(true), Answer::Yes);
        assert_eq!(Answer::from_yes(false), Answer::No);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prompt_describes_every_gated_action() {
        use crate::build::Language;
        let w = PendingAction::WriteFile {
            path: PathBuf::from("/tmp/x/notes.txt"),
            body: "hello".into(),
        }
        .prompt();
        assert_eq!((w.kind, w.title), ("write_file", "WRITE FILE"));
        assert_eq!(w.target.as_deref(), Some("/tmp/x/notes.txt"));
        assert_eq!(w.detail.as_deref(), Some("hello"));
        let s = PendingAction::RunShell {
            command: "ls -la".into(),
        }
        .prompt();
        assert_eq!(s.title, "RUN SHELL");
        assert_eq!(s.target, None);
        assert_eq!(s.detail.as_deref(), Some("$ ls -la"));
        let k = PendingAction::WriteSkill {
            path: PathBuf::from("/tmp/skills/greet.md"),
            body: "# Greet".into(),
        }
        .prompt();
        assert_eq!(k.title, "WRITE SKILL");
        let b = PendingAction::BuildProject {
            dir: PathBuf::from("/tmp/builds/demo"),
            language: Language::Rust,
        }
        .prompt();
        assert_eq!(b.title, "BUILD PROJECT");
        assert!(b.detail.is_none());
        let b = b.with_detail("cargo build --offline");
        assert_eq!(b.detail.as_deref(), Some("cargo build --offline"));
        assert!(b.clone().with_detail("  ").detail.is_none());
        for a in [
            PendingAction::ScaffoldProject {
                dir: PathBuf::from("/tmp/builds/demo"),
                language: Language::Python,
                template: "script".into(),
            },
            PendingAction::RunProject {
                dir: PathBuf::from("/tmp/builds/demo"),
                language: Language::Fish,
            },
        ] {
            let p = a.prompt();
            assert!(a.needs_confirmation());
            assert!(p.title.ends_with("PROJECT"), "{}", p.title);
            assert_eq!(p.summary, a.summary());
        }
    }

    #[test]
    fn prompt_detail_is_capped_on_a_char_boundary() {
        let body = "é".repeat(PROMPT_DETAIL_MAX); // 2 bytes each
        let p = PendingAction::WriteFile {
            path: PathBuf::from("/tmp/big.txt"),
            body: body.clone(),
        }
        .prompt();
        let d = p.detail.unwrap();
        assert!(d.len() < body.len());
        assert!(d.contains("more bytes]"), "{}", &d[d.len() - 30..]);
        assert_eq!(cap_preview("short", 10), "short");
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

    // ---- deletion / organize / commands (default-on access) ----

    use crate::access::deletion::{DeleteMode, DeletionManifest, DeletionSignature};

    fn del_setup(tag: &str) -> (PathBuf, SoulContract) {
        let d = tmpdir(tag);
        std::fs::create_dir_all(&d).unwrap();
        let sc = SoulContract::new(Some(d.join("data/audit.log"))).with_trash_dir(d.join("Trash"));
        (d, sc)
    }

    #[test]
    fn delete_without_a_signature_never_runs() {
        let (d, mut sc) = del_setup("del-nosig");
        std::fs::write(d.join("victim.txt"), "x").unwrap();
        let m = DeletionManifest::build(&[d.join("victim.txt")], None).unwrap();
        let action = PendingAction::DeletePaths { manifest: m };
        assert!(action.needs_confirmation());
        assert!(matches!(
            sc.run_now(action.clone()),
            Err(ContractError::NeedsConfirmation(_))
        ));
        let id = sc.request(action.clone());
        assert!(matches!(
            sc.confirm(id),
            Err(ContractError::NeedsSignature(_))
        ));
        assert!(sc.answer(id, Answer::Yes).is_err());
        assert!(decide(None, action, Answer::Yes).is_err());
        assert!(d.join("victim.txt").exists());
        assert_eq!(sc.pending().len(), 1, "still waiting for a signature");
    }

    #[test]
    fn a_signature_for_set_a_cannot_delete_set_b() {
        let (d, mut sc) = del_setup("del-ab");
        std::fs::write(d.join("a.txt"), "a").unwrap();
        std::fs::write(d.join("b.txt"), "b").unwrap();
        let ma = DeletionManifest::build(&[d.join("a.txt")], None).unwrap();
        let mb = DeletionManifest::build(&[d.join("b.txt")], None).unwrap();
        let _ida = sc.request(PendingAction::DeletePaths {
            manifest: ma.clone(),
        });
        let idb = sc.request(PendingAction::DeletePaths { manifest: mb });
        let sig_a = DeletionSignature::sign(&ma, &ma.phrase(), DeleteMode::Trash).unwrap();
        assert!(matches!(
            sc.sign_deletion(idb, sig_a),
            Err(ContractError::WrongSet)
        ));
        assert!(d.join("a.txt").exists() && d.join("b.txt").exists());
        // Not a deletion at all.
        let w = sc.request(PendingAction::WriteFile {
            path: d.join("w"),
            body: "x".into(),
        });
        let sig_a = DeletionSignature::sign(&ma, &ma.phrase(), DeleteMode::Trash).unwrap();
        assert!(matches!(
            sc.sign_deletion(w, sig_a),
            Err(ContractError::NotADeletion(_))
        ));
    }

    #[test]
    fn signed_deletion_trashes_exactly_the_list_and_logs_it() {
        let (d, mut sc) = del_setup("del-ok");
        std::fs::create_dir_all(d.join("old/sub")).unwrap();
        std::fs::write(d.join("old/sub/f"), "1").unwrap();
        std::fs::write(d.join("keep"), "k").unwrap();
        let m = DeletionManifest::build(&[d.join("old")], None).unwrap();
        let id = sc.request(PendingAction::DeletePaths {
            manifest: m.clone(),
        });
        let sig = DeletionSignature::sign(&m, "DELETE 3 ITEMS", DeleteMode::Trash).unwrap();
        let done = sc.sign_deletion(id, sig).unwrap();
        assert!(done.output.unwrap().contains("deleted"));
        assert!(!d.join("old").exists());
        assert!(d.join("Trash/files/old/sub/f").exists(), "recoverable");
        assert!(d.join("keep").exists());
        let log = std::fs::read_to_string(d.join("data/deletions.log")).unwrap();
        assert!(log.contains(&d.join("old").display().to_string()));
        let audit = std::fs::read_to_string(d.join("data/audit.log")).unwrap();
        assert!(audit.contains("CONFIRM-SIGNED-TRASH\tdelete_paths"));
        // Single use: the request is gone.
        let sig = DeletionSignature::sign(&m, "DELETE 3 ITEMS", DeleteMode::Trash).unwrap();
        assert!(matches!(
            sc.sign_deletion(id, sig),
            Err(ContractError::Unknown(_))
        ));
    }

    #[test]
    fn a_changed_set_is_asked_again() {
        let (d, mut sc) = del_setup("del-changed");
        std::fs::create_dir(d.join("dir")).unwrap();
        std::fs::write(d.join("dir/a"), "1").unwrap();
        let m = DeletionManifest::build(&[d.join("dir")], None).unwrap();
        let id = sc.request(PendingAction::DeletePaths {
            manifest: m.clone(),
        });
        std::fs::write(d.join("dir/sneaked-in"), "new").unwrap();
        let sig = DeletionSignature::sign(&m, &m.phrase(), DeleteMode::Permanent).unwrap();
        let new_id = match sc.sign_deletion(id, sig) {
            Err(ContractError::Changed { new_id }) => new_id,
            other => panic!("expected Changed, got {other:?}"),
        };
        assert!(d.join("dir/a").exists() && d.join("dir/sneaked-in").exists());
        let PendingAction::DeletePaths { manifest: m2 } = &sc.front().unwrap().1 else {
            panic!()
        };
        assert_eq!(m2.entries.len(), 3, "re-listed with the new file");
        let m2 = m2.clone();
        // The old signature's phrase no longer fits; a new one works.
        let sig2 = DeletionSignature::sign(&m2, &m2.phrase(), DeleteMode::Permanent).unwrap();
        sc.sign_deletion(new_id, sig2).unwrap();
        assert!(!d.join("dir").exists());
    }

    #[test]
    fn system_paths_are_refused_even_when_signed() {
        assert!(DeletionManifest::build(&[PathBuf::from("/usr")], None).is_err());
        assert!(DeletionManifest::build(&[PathBuf::from("/")], None).is_err());
        assert!(DeletionManifest::build(&[PathBuf::from("/etc/hostname")], None).is_err());
    }

    #[test]
    fn commands_are_gated_and_logged() {
        let (d, mut sc) = del_setup("cmd-gate");
        let safe = PendingAction::RunCommand {
            command: "echo hi".into(),
            cwd: None,
            timeout_s: 5,
        };
        assert!(!safe.needs_confirmation());
        let out = sc.run_now(safe).unwrap().output.unwrap();
        assert!(out.contains("hi"));
        for risky in [
            "sudo pacman -Syu",
            "kill 1",
            "echo x > f",
            "systemctl stop sddm",
            "dd if=/dev/zero of=x",
        ] {
            let a = PendingAction::RunCommand {
                command: risky.into(),
                cwd: Some(d.clone()),
                timeout_s: 5,
            };
            assert!(a.needs_confirmation(), "{risky}");
            assert!(matches!(
                sc.run_now(a.clone()),
                Err(ContractError::NeedsConfirmation(_))
            ));
            let p = a.prompt();
            assert!(
                p.detail.unwrap().contains(risky),
                "card shows the exact command"
            );
        }
        // Removal never runs, not even with a Yes (and not via RunShell either).
        std::fs::write(d.join("f"), "x").unwrap();
        for a in [
            PendingAction::RunCommand {
                command: format!("rm {}", d.join("f").display()),
                cwd: None,
                timeout_s: 5,
            },
            PendingAction::RunShell {
                command: format!("cd {} && rm -f f", d.display()),
            },
        ] {
            let id = sc.request(a);
            assert!(sc.confirm(id).is_err());
        }
        assert!(d.join("f").exists());
        let log = std::fs::read_to_string(d.join("data/commands.log")).unwrap();
        assert!(log.contains("AUTO\techo hi"));
        assert!(log.contains("CONFIRM\trm "));
    }

    #[test]
    fn small_organize_runs_now_big_one_needs_a_yes() {
        let (d, mut sc) = del_setup("org-gate");
        std::fs::write(d.join("a.png"), "a").unwrap();
        let plan = crate::access::organize::suggest_sort(&d, crate::access::organize::SortBy::Type)
            .unwrap();
        let small = PendingAction::Organize {
            plan: plan.clone(),
            journal: d.join("data/organize-journal"),
            preview_over: 10,
        };
        assert!(!small.needs_confirmation());
        let big = PendingAction::Organize {
            plan,
            journal: d.join("data/organize-journal"),
            preview_over: 1,
        };
        assert!(big.needs_confirmation());
        assert!(big.prompt().detail.unwrap().contains("a.png"));
        sc.run_now(small).unwrap();
        assert!(d.join("Images/a.png").exists());
    }
}
