//! Lillith's scoped local file/tool API (Phase 2 of the companion plan).
//!
//! This is the bridge between a tool request the GGUF mind writes into a
//! reply and a [`PendingAction`] the Soul Contract can run. It owns two
//! things:
//!
//! - [`Allowlist`] — which folders she may touch. By default: the config dir,
//!   the knowledge and skills folders, plus every `[mind] allow_paths` root
//!   (e.g. `~/Projects`). A built-in deny list keeps `/etc` and the usual
//!   secrets dirs (`~/.ssh`, `~/.gnupg`, `~/.aws`, …) out of reach unless a
//!   matching `[mind] unlock_paths` root re-opens them. There is no network
//!   tool at all.
//! - [`MindTools::plan`] — takes a parsed [`ToolRequest`], confines its path
//!   to the allowlist, and returns a [`Planned`]: a read she may run straight
//!   away (audited, no popup) or a mutation that needs a Soul Contract Yes.
//!
//! The model asks for a tool with a fenced ` ```tool ` block; see
//! [`parse_tool_request`] for the exact format.

use lillith_core::access::{self, commands::CommandClass, organize, programs};
use lillith_core::config::{expand_tilde, AccessConfig, MindConfig};
use lillith_core::soul_contract::PendingAction;
use std::path::{Component, Path, PathBuf};

/// A tool the model asked for, parsed out of a reply. Paths here are raw
/// (as the model wrote them); [`MindTools::plan`] normalises and vets them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolRequest {
    /// List a folder's entries.
    List { path: String },
    /// Read a file.
    Read { path: String },
    /// Search a file or folder for a substring.
    Search { path: String, query: String },
    /// Write a file (needs a Yes).
    Write { path: String, body: String },
    /// Run a shell command (needs a Yes, and `allow_shell`).
    Shell { command: String },
    /// Find files/folders by name (`*` wildcards) under a folder.
    Locate { path: String, pattern: String },
    /// Move or rename into a folder / to a new path (never overwrites).
    Move { from: String, to: String },
    /// Copy a file or folder (never overwrites).
    Copy { from: String, to: String },
    /// Make a folder.
    Mkdir { path: String },
    /// Rename in place to `name`.
    Rename { path: String, name: String },
    /// Sort the files in a folder into sub-folders by `type` or `date`.
    Sort { path: String, by: String },
    /// Undo the last organize batch.
    Undo,
    /// Delete paths: lists everything, then waits for Ko's signature.
    Delete { paths: Vec<String> },
    /// Open an installed app (optionally with files) or a program by name.
    Open { app: String, files: Vec<String> },
    /// Is a program installed?
    Installed { name: String },
    /// List installed apps (optionally filtered).
    Programs { filter: String },
    /// Run a CLI command (read-only ones run now; others need a Yes).
    Run { command: String },
    /// List the skills folder.
    ListSkills,
    /// Read one skill by name or file.
    ReadSkill { name: String },
    /// Import a skills folder (default: the one on the desktop).
    ImportSkills { from: Option<String> },
}

impl ToolRequest {
    /// The tool's name as she writes it (for notes and logs).
    pub fn op(&self) -> &'static str {
        match self {
            ToolRequest::List { .. } => "list",
            ToolRequest::Read { .. } => "read",
            ToolRequest::Search { .. } => "search",
            ToolRequest::Write { .. } => "write",
            ToolRequest::Shell { .. } => "shell",
            ToolRequest::Locate { .. } => "locate",
            ToolRequest::Move { .. } => "move",
            ToolRequest::Copy { .. } => "copy",
            ToolRequest::Mkdir { .. } => "mkdir",
            ToolRequest::Rename { .. } => "rename",
            ToolRequest::Sort { .. } => "sort",
            ToolRequest::Undo => "undo",
            ToolRequest::Delete { .. } => "delete",
            ToolRequest::Open { .. } => "open",
            ToolRequest::Installed { .. } => "installed",
            ToolRequest::Programs { .. } => "programs",
            ToolRequest::Run { .. } => "run",
            ToolRequest::ListSkills => "list_skills",
            ToolRequest::ReadSkill { .. } => "read_skill",
            ToolRequest::ImportSkills { .. } => "import_skills",
        }
    }
}

/// The outcome of planning a [`ToolRequest`] against the allowlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Planned {
    /// A read she may run now (via `SoulContract::run_now`): no popup, audited.
    Read(PendingAction),
    /// A mutation that must wait for a Soul Contract Yes (a
    /// [`PendingAction::DeletePaths`] waits for a signature instead).
    Confirm(PendingAction),
    /// Answered on the spot (installed apps, skill list, an import report…).
    Info(String),
}

/// Why a tool request was refused before it ever reached the gate.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolError {
    #[error("{} is outside Lillith's allowed folders", .0.display())]
    OutsideAllowlist(PathBuf),
    #[error("shell is turned off (set [mind] allow_shell = true to enable it)")]
    ShellDisabled,
    #[error("the tool request was empty")]
    Empty,
    #[error("{0} is switched off in [mind.access]")]
    Disabled(&'static str),
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    NotFound(String),
}

/// The set of folders Lillith's tool API may touch, plus the shell switch and
/// read cap.
#[derive(Debug, Clone)]
pub struct Allowlist {
    allow: Vec<PathBuf>,
    deny: Vec<PathBuf>,
    unlock: Vec<PathBuf>,
    allow_shell: bool,
    max_read_bytes: usize,
}

impl Allowlist {
    /// Builds the allowlist from `[mind]`: allowed roots from
    /// [`MindConfig::resolved_allow_roots`], the built-in deny roots, and any
    /// `unlock_paths` that re-open them.
    pub fn from_mind(cfg: &MindConfig) -> Self {
        Self {
            allow: cfg.resolved_allow_roots(),
            deny: default_deny_roots(),
            unlock: cfg.resolved_unlock_roots(),
            allow_shell: cfg.allow_shell,
            max_read_bytes: cfg.read_cap(),
        }
    }

    /// Builds an allowlist directly (tests).
    pub fn new(
        allow: Vec<PathBuf>,
        deny: Vec<PathBuf>,
        unlock: Vec<PathBuf>,
        allow_shell: bool,
        max_read_bytes: usize,
    ) -> Self {
        Self {
            allow,
            deny,
            unlock,
            allow_shell,
            max_read_bytes: max_read_bytes.max(1),
        }
    }

    pub fn allow_shell(&self) -> bool {
        self.allow_shell
    }

    pub fn max_read_bytes(&self) -> usize {
        self.max_read_bytes
    }

    pub fn allow_roots(&self) -> &[PathBuf] {
        &self.allow
    }

    /// Whether `path` may be touched: inside an allowed root, and not inside a
    /// deny root (unless an unlock root re-opens it). Operates on the
    /// lexically-normalised path, so `..` cannot climb out of an allowed root.
    pub fn is_allowed(&self, path: &Path) -> bool {
        let p = normalize(path);
        let under_allow = self.allow.iter().any(|r| p.starts_with(r));
        if !under_allow {
            return false;
        }
        let under_deny = self.deny.iter().any(|r| p.starts_with(r));
        let under_unlock = self.unlock.iter().any(|r| p.starts_with(r));
        !under_deny || under_unlock
    }
}

/// The scoped tool API ChatSession drives: an [`Allowlist`] plus the audit
/// log path mutations are recorded against.
#[derive(Debug, Clone)]
pub struct MindTools {
    allow: Allowlist,
    audit_log: Option<PathBuf>,
    access: AccessConfig,
    skills_dir: Option<PathBuf>,
}

impl MindTools {
    /// Builds from `[mind]` (allowlist + audit log path).
    pub fn from_mind(cfg: &MindConfig) -> Self {
        Self {
            allow: Allowlist::from_mind(cfg),
            audit_log: cfg.audit_log_path(),
            access: cfg.access.clone(),
            skills_dir: cfg.resolved_skills_dir(),
        }
    }

    /// Builds directly (tests). Access switches default to on; see
    /// [`MindTools::with_access`].
    pub fn new(allow: Allowlist, audit_log: Option<PathBuf>) -> Self {
        Self {
            allow,
            audit_log,
            access: AccessConfig::default(),
            skills_dir: None,
        }
    }

    /// Replaces the `[mind.access]` switches (tests).
    pub fn with_access(mut self, access: AccessConfig) -> Self {
        self.access = access;
        self
    }

    /// Sets the skills folder (tests).
    pub fn with_skills_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.skills_dir = dir;
        self
    }

    pub fn access(&self) -> &AccessConfig {
        &self.access
    }

    pub fn skills_dir(&self) -> Option<&Path> {
        self.skills_dir.as_deref()
    }

    /// Lillith's data folder (the audit log's folder).
    pub fn data_dir(&self) -> Option<&Path> {
        self.audit_log.as_deref().and_then(Path::parent)
    }

    /// The organize undo journal (`<data>/lillith/organize-journal`).
    pub fn journal_dir(&self) -> Option<PathBuf> {
        self.data_dir().map(|d| d.join("organize-journal"))
    }

    pub fn allowlist(&self) -> &Allowlist {
        &self.allow
    }

    pub fn audit_log(&self) -> Option<&Path> {
        self.audit_log.as_deref()
    }

    /// Confines a request to the allowlist and turns it into a [`Planned`]
    /// action. Reads come back as [`Planned::Read`] (run now, no popup);
    /// writes and shell runs as [`Planned::Confirm`] (need a Yes).
    pub fn plan(&self, req: ToolRequest) -> Result<Planned, ToolError> {
        match req {
            ToolRequest::List { path } => {
                let p = self.vet(&path)?;
                Ok(Planned::Read(PendingAction::ListDir { path: p }))
            }
            ToolRequest::Read { path } => {
                let p = self.vet(&path)?;
                Ok(Planned::Read(PendingAction::ReadFile {
                    path: p,
                    max_bytes: self.allow.max_read_bytes,
                }))
            }
            ToolRequest::Search { path, query } => {
                let p = self.vet(&path)?;
                Ok(Planned::Read(PendingAction::SearchFiles {
                    path: p,
                    query,
                    max_bytes: self.allow.max_read_bytes,
                }))
            }
            ToolRequest::Write { path, body } => {
                let p = self.vet(&path)?;
                Ok(Planned::Confirm(PendingAction::WriteFile { path: p, body }))
            }
            ToolRequest::Shell { command } => {
                if !self.allow.allow_shell {
                    return Err(ToolError::ShellDisabled);
                }
                if command.trim().is_empty() {
                    return Err(ToolError::Empty);
                }
                if let CommandClass::Removal(r) = access::commands::classify(&command, &[]) {
                    return Err(ToolError::Refused(r));
                }
                Ok(Planned::Confirm(PendingAction::RunShell { command }))
            }
            ToolRequest::Locate { path, pattern } => {
                let p = self.vet(&path)?;
                if pattern.trim().is_empty() {
                    return Err(ToolError::Empty);
                }
                // Hits inside denied folders are dropped.
                let report = access::find::find_report(&p, pattern.trim());
                let kept: Vec<&str> = report
                    .lines()
                    .filter(|l| {
                        l.starts_with('…')
                            || l.starts_with("nothing")
                            || self.allow.is_allowed(Path::new(l.trim_end_matches('/')))
                    })
                    .collect();
                Ok(Planned::Info(kept.join("\n")))
            }
            ToolRequest::Move { from, to } => {
                let op = organize::OrganizeOp::Move {
                    from: self.vet_existing(&from)?,
                    to: self.vet(&to)?,
                };
                self.organize(vec![op])
            }
            ToolRequest::Copy { from, to } => {
                let op = organize::OrganizeOp::Copy {
                    from: self.vet_existing(&from)?,
                    to: self.vet(&to)?,
                };
                self.organize(vec![op])
            }
            ToolRequest::Mkdir { path } => {
                let op = organize::OrganizeOp::Mkdir {
                    path: self.vet(&path)?,
                };
                self.organize(vec![op])
            }
            ToolRequest::Rename { path, name } => {
                let from = self.vet_existing(&path)?;
                let name = name.trim();
                if name.is_empty() || name.contains('/') || name == "." || name == ".." {
                    return Err(ToolError::Refused(format!(
                        "{name:?} is not a plain new name (use move to change folders)"
                    )));
                }
                let to = from.with_file_name(name);
                self.organize(vec![organize::OrganizeOp::Move { from, to }])
            }
            ToolRequest::Sort { path, by } => {
                self.need(self.access.organize, "organize")?;
                let dir = self.vet_existing(&path)?;
                let by = if by.trim().to_ascii_lowercase().starts_with("date")
                    || by.trim().eq_ignore_ascii_case("month")
                {
                    organize::SortBy::Date
                } else {
                    organize::SortBy::Type
                };
                let plan = organize::suggest_sort(&dir, by)
                    .map_err(|e| ToolError::NotFound(format!("{}: {e}", dir.display())))?;
                if plan.is_empty() {
                    return Ok(Planned::Info(format!(
                        "{} has no loose files to sort",
                        dir.display()
                    )));
                }
                self.organize(plan.ops)
            }
            ToolRequest::Undo => {
                self.need(self.access.organize, "organize")?;
                let j = self
                    .journal_dir()
                    .ok_or_else(|| ToolError::NotFound("no data folder for the journal".into()))?;
                let rep = organize::undo_last(&j)
                    .map_err(|e| ToolError::Refused(format!("undo failed: {e}")))?;
                Ok(Planned::Info(rep.text()))
            }
            ToolRequest::Delete { paths } => {
                if paths.is_empty() {
                    return Err(ToolError::Empty);
                }
                let mut vetted = Vec::new();
                for p in &paths {
                    vetted.push(self.vet_existing(p)?);
                }
                let manifest = access::deletion::DeletionManifest::build(&vetted, self.data_dir())
                    .map_err(|e| ToolError::Refused(e.to_string()))?;
                Ok(Planned::Confirm(PendingAction::DeletePaths { manifest }))
            }
            ToolRequest::Open { app, files } => {
                self.need(self.access.programs, "programs")?;
                let mut fs = Vec::new();
                for f in &files {
                    fs.push(self.vet_existing(f)?);
                }
                let app = app.trim();
                if app.is_empty() {
                    return Err(ToolError::Empty);
                }
                let apps = programs::desktop_apps();
                if let Some(a) = programs::find_app(&apps, app) {
                    return Ok(Planned::Read(PendingAction::Launch {
                        argv: programs::launch_argv(a, &fs),
                        app: Some(a.id.clone()),
                    }));
                }
                if programs::which(app).is_some() {
                    let mut argv = vec![app.to_string()];
                    argv.extend(fs.iter().map(|f| f.display().to_string()));
                    return Ok(Planned::Confirm(PendingAction::Launch { argv, app: None }));
                }
                Err(ToolError::NotFound(format!(
                    "{app} isn't installed (no app entry, not on PATH)"
                )))
            }
            ToolRequest::Installed { name } => {
                self.need(self.access.programs, "programs")?;
                Ok(Planned::Info(programs::installed_report(name.trim())))
            }
            ToolRequest::Programs { filter } => {
                self.need(self.access.programs, "programs")?;
                Ok(Planned::Info(programs::apps_report(&filter)))
            }
            ToolRequest::Run { command } => {
                self.need(self.access.commands, "commands")?;
                let command = command.trim().to_string();
                if command.is_empty() {
                    return Err(ToolError::Empty);
                }
                let class = access::commands::classify(&command, &self.allow.deny);
                let action = PendingAction::RunCommand {
                    command,
                    cwd: None,
                    timeout_s: self.access.command_timeout_s.max(1),
                };
                match class {
                    CommandClass::Removal(r) => Err(ToolError::Refused(r)),
                    CommandClass::Safe => Ok(Planned::Read(action)),
                    CommandClass::NeedsApproval(_) => Ok(Planned::Confirm(action)),
                }
            }
            ToolRequest::ListSkills => {
                let dir = self.skills()?;
                Ok(Planned::Info(skills_listing(&dir)))
            }
            ToolRequest::ReadSkill { name } => {
                let dir = self.skills()?;
                let path = find_skill(&dir, &name)
                    .ok_or_else(|| ToolError::NotFound(format!("no skill called {name:?}")))?;
                Ok(Planned::Read(PendingAction::ReadFile {
                    path,
                    max_bytes: self.allow.max_read_bytes,
                }))
            }
            ToolRequest::ImportSkills { from } => {
                let dir = self.skills()?;
                let log = self.data_dir().map(|d| d.join("skills-import.log"));
                let sources = match from {
                    Some(f) => vec![normalize(Path::new(&f))],
                    None => access::skill_import::find_desktop_skills(),
                };
                if sources.is_empty() {
                    return Ok(Planned::Info(
                        "no skills folder on the desktop (looked for skills / lillith-skills)"
                            .into(),
                    ));
                }
                let mut out = Vec::new();
                for src in sources {
                    match access::skill_import::import(&src, &dir, log.as_deref()) {
                        Ok(r) => out.push(r.text()),
                        Err(e) => out.push(format!("{}: {e}", src.display())),
                    }
                }
                Ok(Planned::Info(out.join("\n")))
            }
        }
    }

    fn need(&self, on: bool, what: &'static str) -> Result<(), ToolError> {
        if on {
            Ok(())
        } else {
            Err(ToolError::Disabled(what))
        }
    }

    fn skills(&self) -> Result<PathBuf, ToolError> {
        self.need(self.access.skills, "skills")?;
        self.skills_dir
            .clone()
            .ok_or_else(|| ToolError::NotFound("no skills folder configured".into()))
    }

    /// Vets an organize batch and files it: small batches run now, bigger ones
    /// show the plan and wait for a Yes.
    fn organize(&self, ops: Vec<organize::OrganizeOp>) -> Result<Planned, ToolError> {
        self.need(self.access.organize, "organize")?;
        let plan = organize::OrganizePlan { ops };
        if let Some(r) = plan.refusal(self.data_dir()) {
            return Err(ToolError::Refused(r));
        }
        let journal = self
            .journal_dir()
            .ok_or_else(|| ToolError::NotFound("no data folder for the undo journal".into()))?;
        let action = PendingAction::Organize {
            plan,
            journal,
            preview_over: self.access.preview_over,
        };
        Ok(if action.needs_confirmation() {
            Planned::Confirm(action)
        } else {
            Planned::Read(action)
        })
    }

    /// [`vet`](Self::vet) plus "it must exist".
    fn vet_existing(&self, raw: &str) -> Result<PathBuf, ToolError> {
        let p = self.vet(raw)?;
        if std::fs::symlink_metadata(&p).is_err() {
            return Err(ToolError::NotFound(format!(
                "{} doesn't exist",
                p.display()
            )));
        }
        Ok(p)
    }

    /// Normalises a path and checks it against the allowlist.
    fn vet(&self, raw: &str) -> Result<PathBuf, ToolError> {
        let p = normalize(Path::new(raw));
        if self.allow.is_allowed(&p) {
            Ok(p)
        } else {
            Err(ToolError::OutsideAllowlist(p))
        }
    }
}

/// Text and script files in the skills folder, one per line with its title /
/// first line (scripts are marked so she knows to `run` them).
pub fn skills_listing(dir: &Path) -> String {
    let mut rows = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        let mut kids: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        kids.sort();
        for k in kids {
            let name = k
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if name.starts_with('.') {
                continue;
            }
            if k.is_dir() {
                stack.push(k);
                continue;
            }
            let rel = k.strip_prefix(dir).unwrap_or(&k).display().to_string();
            let ext = k
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if matches!(ext.as_str(), "md" | "markdown" | "txt") {
                let doc = std::fs::read_to_string(&k)
                    .map(|c| crate::knowledge::parse_doc(&k, &c, crate::knowledge::DocKind::Skill))
                    .ok();
                match doc {
                    Some(d) if !d.summary.is_empty() => {
                        rows.push(format!("{rel} — {}: {}", d.title, d.summary))
                    }
                    Some(d) => rows.push(format!("{rel} — {}", d.title)),
                    None => rows.push(rel),
                }
            } else {
                rows.push(format!(
                    "{rel} (script/file — run it with: run {})",
                    k.display()
                ));
            }
        }
    }
    if rows.is_empty() {
        format!("the skills folder {} is empty", dir.display())
    } else {
        format!("skills in {}:\n{}", dir.display(), rows.join("\n"))
    }
}

/// Resolves a skill name to a file in `dir`: an exact relative path, a file
/// stem (any case), or a `# Title` match. Never leaves `dir`.
pub fn find_skill(dir: &Path, name: &str) -> Option<PathBuf> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let direct = normalize(&dir.join(name));
    if direct.starts_with(dir) && direct.is_file() {
        return Some(direct);
    }
    let want = name.to_lowercase();
    let want_stem = Path::new(&want)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| want.clone());
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                files.push(p);
            }
        }
    }
    files.sort();
    let stem = |p: &Path| {
        p.file_stem()
            .map(|s| s.to_string_lossy().to_lowercase().replace(['_', '-'], " "))
            .unwrap_or_default()
    };
    let ws = want_stem.replace(['_', '-'], " ");
    files
        .iter()
        .find(|p| stem(p) == ws)
        .or_else(|| {
            files.iter().find(|p| {
                std::fs::read_to_string(p)
                    .map(|c| {
                        crate::knowledge::parse_doc(p, &c, crate::knowledge::DocKind::Skill)
                            .title
                            .to_lowercase()
                            == want
                    })
                    .unwrap_or(false)
            })
        })
        .or_else(|| files.iter().find(|p| stem(p).contains(&ws)))
        .cloned()
}

/// The built-in deny roots: `/etc` and the usual per-user secrets folders.
/// `unlock_paths` can re-open any of these.
fn default_deny_roots() -> Vec<PathBuf> {
    let mut v = vec![
        PathBuf::from("/etc"),
        PathBuf::from("/proc"),
        PathBuf::from("/sys"),
        PathBuf::from("/dev"),
    ];
    // Shared with the delete/organize protection list (lillith_core::access).
    v.extend(access::secret_roots());
    v
}

/// Lexically normalises a path: expands a leading `~`, makes it absolute
/// (relative paths are taken against `$HOME`), and resolves `.`/`..` without
/// touching the filesystem (so it works for files that don't exist yet and
/// cannot be fooled by a path that doesn't exist on disk).
pub fn normalize(path: &Path) -> PathBuf {
    // Expand a leading tilde against HOME.
    let expanded: PathBuf = match path.to_str() {
        Some(s) if s == "~" || s.starts_with("~/") => {
            expand_tilde(s).unwrap_or_else(|| path.to_path_buf())
        }
        _ => path.to_path_buf(),
    };
    // Anchor relative paths at HOME so there is always a root to confine.
    let anchored = if expanded.is_absolute() {
        expanded
    } else if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        home.join(expanded)
    } else {
        expanded
    };
    let mut out = PathBuf::new();
    for comp in anchored.components() {
        match comp {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(c) => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Parsing tool requests out of a model reply.
// ---------------------------------------------------------------------------

/// Parses the first tool request out of `reply`.
///
/// She asks for a tool with a fenced block whose info string is `tool`:
///
/// ````text
/// ```tool
/// list ~/Projects
/// ```
/// ```tool
/// read ~/Projects/notes.md
/// ```
/// ```tool
/// search ~/Projects TODO
/// ```
/// ```tool
/// write ~/Projects/hello.txt
/// line one
/// line two
/// ```
/// ```tool
/// shell
/// cargo build --locked
/// ```
/// ````
///
/// The first line inside the block is the command (`list`/`read`/`search`/
/// `write`/`shell` plus its path/args); for `write` and `shell` the remaining
/// lines are the file body / the command. The command may also be written on
/// the fence line itself (`` ```tool read ~/x `` ). A JSON body such as
/// `{"op":"read","path":"~/x"}` is accepted too. Returns `None` if there is no
/// well-formed tool block.
pub fn parse_tool_request(reply: &str) -> Option<ToolRequest> {
    for (info, body) in fenced_blocks(reply) {
        let rest = info.strip_prefix("tool")?;
        // The info string's first token must be exactly "tool".
        if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
            continue;
        }
        if let Some(req) = parse_tool_body(rest.trim(), &body) {
            return Some(req);
        }
    }
    None
}

/// Parses one tool block. `inline` is anything after `tool` on the fence line;
/// `body` is the fenced content.
fn parse_tool_body(inline: &str, body: &str) -> Option<ToolRequest> {
    // JSON form: the body is a single `{...}` object.
    let trimmed = body.trim();
    if inline.is_empty() && trimmed.starts_with('{') {
        if let Some(req) = parse_tool_json(trimmed) {
            return Some(req);
        }
    }
    // Line form: command from the inline text, else the first body line.
    let (command_line, content): (String, String) = if inline.is_empty() {
        let mut lines = body.lines();
        let first = lines.next()?.trim().to_string();
        let rest: Vec<&str> = lines.collect();
        (first, rest.join("\n"))
    } else {
        (inline.to_string(), body.to_string())
    };
    let command_line = command_line.trim();
    if command_line.is_empty() {
        return None;
    }
    let mut parts = command_line.splitn(2, char::is_whitespace);
    let op = parts.next()?.to_ascii_lowercase();
    let arg = parts
        .next()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    match op.as_str() {
        "list" | "ls" | "dir" => (!arg.is_empty()).then_some(ToolRequest::List { path: arg }),
        "read" | "cat" => (!arg.is_empty()).then_some(ToolRequest::Read { path: arg }),
        "search" | "grep" | "find" => {
            let mut a = arg.splitn(2, char::is_whitespace);
            let path = a.next().unwrap_or_default().trim().to_string();
            let query = a.next().unwrap_or_default().trim().to_string();
            (!path.is_empty() && !query.is_empty()).then_some(ToolRequest::Search { path, query })
        }
        "write" => {
            let content = content.trim_end_matches('\n').to_string();
            (!arg.is_empty()).then_some(ToolRequest::Write {
                path: arg,
                body: content,
            })
        }
        "shell" | "sh" => {
            // Shell command: inline tail if present, else the body.
            let command = if arg.is_empty() {
                content.trim().to_string()
            } else {
                arg
            };
            (!command.is_empty()).then_some(ToolRequest::Shell { command })
        }
        "run" | "exec" | "command" => {
            let command = if arg.is_empty() {
                content.trim().to_string()
            } else {
                arg
            };
            (!command.is_empty()).then_some(ToolRequest::Run { command })
        }
        "locate" | "find_name" | "findname" => {
            let (path, pattern) = split_two(&arg)?;
            Some(ToolRequest::Locate { path, pattern })
        }
        "move" | "mv" => {
            let (from, to) = split_two(&arg)?;
            Some(ToolRequest::Move { from, to })
        }
        "copy" | "cp" => {
            let (from, to) = split_two(&arg)?;
            Some(ToolRequest::Copy { from, to })
        }
        "mkdir" => (!arg.is_empty()).then_some(ToolRequest::Mkdir { path: arg }),
        "rename" => {
            let (path, name) = split_two(&arg)?;
            Some(ToolRequest::Rename { path, name })
        }
        "sort" | "tidy" => {
            if arg.is_empty() {
                return None;
            }
            // `sort ~/Downloads by type` / `sort ~/Downloads date`.
            let lower = arg.to_ascii_lowercase();
            let (path, by) = ["type", "date", "month"]
                .iter()
                .find_map(|k| {
                    lower.ends_with(&format!(" {k}")).then(|| {
                        let p = arg[..arg.len() - k.len() - 1].trim();
                        let p = p.strip_suffix(" by").unwrap_or(p).trim();
                        (p.to_string(), k.to_string())
                    })
                })
                .unwrap_or((arg.clone(), "type".into()));
            Some(ToolRequest::Sort { path, by })
        }
        "undo" | "undo_organize" | "undo-organize" => Some(ToolRequest::Undo),
        "delete" | "trash" | "rm" | "remove" => {
            let mut paths: Vec<String> = Vec::new();
            if !arg.is_empty() {
                paths.push(arg);
            }
            paths.extend(
                content
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string),
            );
            (!paths.is_empty()).then_some(ToolRequest::Delete { paths })
        }
        "open" | "launch" | "start" => {
            let (app, mut files) = match split_arrow(&arg) {
                Some((a, f)) => (a, vec![f]),
                None => (arg.clone(), Vec::new()),
            };
            files.extend(
                content
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string),
            );
            (!app.is_empty()).then_some(ToolRequest::Open { app, files })
        }
        "installed" | "which" | "has" => {
            (!arg.is_empty()).then_some(ToolRequest::Installed { name: arg })
        }
        "programs" | "apps" => Some(ToolRequest::Programs { filter: arg }),
        "list_skills" | "skills" | "list-skills" => Some(ToolRequest::ListSkills),
        "read_skill" | "skill" | "read-skill" => {
            (!arg.is_empty()).then_some(ToolRequest::ReadSkill { name: arg })
        }
        "import_skills" | "import-skills" => Some(ToolRequest::ImportSkills {
            from: (!arg.is_empty()).then_some(arg),
        }),
        _ => None,
    }
}

/// `a -> b` / `a → b`.
fn split_arrow(arg: &str) -> Option<(String, String)> {
    for sep in [" -> ", " → ", " => "] {
        if let Some((a, b)) = arg.split_once(sep) {
            let (a, b) = (a.trim(), b.trim());
            if !a.is_empty() && !b.is_empty() {
                return Some((a.to_string(), b.to_string()));
            }
        }
    }
    None
}

/// Two arguments: `a -> b`, else exactly two words (paths with spaces need the arrow).
fn split_two(arg: &str) -> Option<(String, String)> {
    if let Some(p) = split_arrow(arg) {
        return Some(p);
    }
    let mut it = arg.splitn(2, char::is_whitespace);
    let a = it.next()?.trim().to_string();
    let b = it.next()?.trim().to_string();
    (!a.is_empty() && !b.is_empty()).then_some((a, b))
}

/// Parses a `{"op":...}` JSON tool call (no serde struct needed; small and
/// forgiving).
fn parse_tool_json(s: &str) -> Option<ToolRequest> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    let op = v
        .get("op")
        .or_else(|| v.get("tool"))?
        .as_str()?
        .to_ascii_lowercase();
    let path = v
        .get("path")
        .and_then(|p| p.as_str())
        .map(|s| s.to_string());
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let list = |k: &str| -> Vec<String> {
        match v.get(k) {
            Some(serde_json::Value::Array(a)) => a
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect(),
            Some(serde_json::Value::String(s)) => vec![s.clone()],
            _ => Vec::new(),
        }
    };
    match op.as_str() {
        "locate" | "find_name" => Some(ToolRequest::Locate {
            path: path?,
            pattern: get("pattern").or_else(|| get("name"))?,
        }),
        "move" | "copy" => {
            let from = get("from").or(path)?;
            let to = get("to")?;
            Some(if op == "move" {
                ToolRequest::Move { from, to }
            } else {
                ToolRequest::Copy { from, to }
            })
        }
        "mkdir" => path.map(|path| ToolRequest::Mkdir { path }),
        "rename" => Some(ToolRequest::Rename {
            path: path?,
            name: get("name").or_else(|| get("to"))?,
        }),
        "sort" => Some(ToolRequest::Sort {
            path: path?,
            by: get("by").unwrap_or_else(|| "type".into()),
        }),
        "undo" | "undo_organize" => Some(ToolRequest::Undo),
        "delete" | "trash" => {
            let mut paths = list("paths");
            if let Some(p) = path {
                paths.insert(0, p);
            }
            (!paths.is_empty()).then_some(ToolRequest::Delete { paths })
        }
        "open" | "launch" => Some(ToolRequest::Open {
            app: get("app").or_else(|| get("name"))?,
            files: [list("files"), list("file")].concat(),
        }),
        "installed" => Some(ToolRequest::Installed {
            name: get("name").or_else(|| get("app"))?,
        }),
        "programs" | "apps" => Some(ToolRequest::Programs {
            filter: get("filter").unwrap_or_default(),
        }),
        "run" => Some(ToolRequest::Run {
            command: get("command").or_else(|| get("cmd"))?,
        }),
        "list_skills" => Some(ToolRequest::ListSkills),
        "read_skill" => Some(ToolRequest::ReadSkill {
            name: get("name").or(path)?,
        }),
        "import_skills" => Some(ToolRequest::ImportSkills {
            from: get("from").or(path),
        }),
        "list" => path.map(|path| ToolRequest::List { path }),
        "read" => path.map(|path| ToolRequest::Read { path }),
        "search" => {
            let query = v.get("query").and_then(|q| q.as_str())?.to_string();
            path.map(|path| ToolRequest::Search { path, query })
        }
        "write" => {
            let body = v
                .get("body")
                .or_else(|| v.get("content"))
                .and_then(|b| b.as_str())
                .unwrap_or("")
                .to_string();
            path.map(|path| ToolRequest::Write { path, body })
        }
        "shell" => {
            let command = v
                .get("command")
                .or_else(|| v.get("cmd"))
                .and_then(|c| c.as_str())?
                .to_string();
            Some(ToolRequest::Shell { command })
        }
        _ => None,
    }
}

/// Yields `(info, body)` for each fenced code block in `text` (``` or ~~~).
fn fenced_blocks(text: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_start();
        let info = line
            .strip_prefix("```")
            .or_else(|| line.strip_prefix("~~~"));
        if let Some(info) = info {
            let mut body = String::new();
            let mut j = i + 1;
            let mut closed = false;
            while j < lines.len() {
                let l = lines[j].trim_start();
                if l.starts_with("```") || l.starts_with("~~~") {
                    closed = true;
                    break;
                }
                body.push_str(lines[j]);
                body.push('\n');
                j += 1;
            }
            if closed {
                blocks.push((info.trim().to_string(), body));
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowlist(allow: &[&str], deny: &[&str], unlock: &[&str], shell: bool) -> Allowlist {
        Allowlist::new(
            allow.iter().map(PathBuf::from).collect(),
            deny.iter().map(PathBuf::from).collect(),
            unlock.iter().map(PathBuf::from).collect(),
            shell,
            64 * 1024,
        )
    }

    #[test]
    fn normalize_resolves_dotdot_and_tilde() {
        assert_eq!(
            normalize(Path::new("/home/ko/Projects/../Projects/a.txt")),
            PathBuf::from("/home/ko/Projects/a.txt")
        );
        assert_eq!(
            normalize(Path::new("/home/ko/./sub/./x")),
            PathBuf::from("/home/ko/sub/x")
        );
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(
                normalize(Path::new("~/Projects/a")),
                PathBuf::from(&home).join("Projects/a")
            );
        }
    }

    #[test]
    fn allowlist_confines_reads_and_blocks_traversal() {
        let a = allowlist(
            &["/home/ko/Projects", "/home/ko/.config/lillith"],
            &["/etc", "/home/ko/.ssh"],
            &[],
            false,
        );
        assert!(a.is_allowed(Path::new("/home/ko/Projects/src/main.rs")));
        assert!(a.is_allowed(Path::new("/home/ko/.config/lillith/config.toml")));
        // Outside every allow root.
        assert!(!a.is_allowed(Path::new("/home/ko/Secrets/pw")));
        assert!(!a.is_allowed(Path::new("/etc/passwd")));
        // `..` that climbs out of an allowed root is rejected after normalising.
        assert!(!a.is_allowed(Path::new("/home/ko/Projects/../../.ssh/id_rsa")));
    }

    #[test]
    fn deny_wins_unless_unlocked() {
        // A secrets dir nested inside an allowed root is still denied…
        let a = allowlist(&["/home/ko"], &["/home/ko/.ssh"], &[], false);
        assert!(a.is_allowed(Path::new("/home/ko/notes.md")));
        assert!(!a.is_allowed(Path::new("/home/ko/.ssh/id_rsa")));
        // …unless an unlock root re-opens it.
        let b = allowlist(
            &["/home/ko"],
            &["/home/ko/.ssh"],
            &["/home/ko/.ssh/pub"],
            false,
        );
        assert!(!b.is_allowed(Path::new("/home/ko/.ssh/id_rsa")));
        assert!(b.is_allowed(Path::new("/home/ko/.ssh/pub/key.pub")));
    }

    #[test]
    fn plan_gates_writes_and_shell_but_not_reads() {
        let tools = MindTools::new(allowlist(&["/home/ko/Projects"], &[], &[], true), None);
        // Reads -> run-now.
        assert!(matches!(
            tools.plan(ToolRequest::Read {
                path: "/home/ko/Projects/a.md".into()
            }),
            Ok(Planned::Read(PendingAction::ReadFile { .. }))
        ));
        assert!(matches!(
            tools.plan(ToolRequest::List {
                path: "/home/ko/Projects".into()
            }),
            Ok(Planned::Read(PendingAction::ListDir { .. }))
        ));
        // Write -> needs confirm, and the stored path is normalised.
        match tools.plan(ToolRequest::Write {
            path: "/home/ko/Projects/sub/../out.txt".into(),
            body: "hi".into(),
        }) {
            Ok(Planned::Confirm(PendingAction::WriteFile { path, body })) => {
                assert_eq!(path, PathBuf::from("/home/ko/Projects/out.txt"));
                assert_eq!(body, "hi");
            }
            other => panic!("expected gated write, got {other:?}"),
        }
        // Shell -> needs confirm when enabled.
        assert!(matches!(
            tools.plan(ToolRequest::Shell {
                command: "ls".into()
            }),
            Ok(Planned::Confirm(PendingAction::RunShell { .. }))
        ));
    }

    #[test]
    fn plan_refuses_outside_allowlist_and_disabled_shell() {
        let tools = MindTools::new(allowlist(&["/home/ko/Projects"], &[], &[], false), None);
        assert!(matches!(
            tools.plan(ToolRequest::Read {
                path: "/etc/passwd".into()
            }),
            Err(ToolError::OutsideAllowlist(_))
        ));
        assert!(matches!(
            tools.plan(ToolRequest::Write {
                path: "/home/ko/other/x".into(),
                body: "x".into()
            }),
            Err(ToolError::OutsideAllowlist(_))
        ));
        // Shell off by default.
        assert!(matches!(
            tools.plan(ToolRequest::Shell {
                command: "rm -rf /".into()
            }),
            Err(ToolError::ShellDisabled)
        ));
    }

    #[test]
    fn parse_line_form_commands() {
        assert_eq!(
            parse_tool_request("sure~\n```tool\nlist ~/Projects\n```"),
            Some(ToolRequest::List {
                path: "~/Projects".into()
            })
        );
        assert_eq!(
            parse_tool_request("```tool\nread ~/Projects/notes.md\n```"),
            Some(ToolRequest::Read {
                path: "~/Projects/notes.md".into()
            })
        );
        assert_eq!(
            parse_tool_request("```tool\nsearch ~/Projects TODO item\n```"),
            Some(ToolRequest::Search {
                path: "~/Projects".into(),
                query: "TODO item".into()
            })
        );
        assert_eq!(
            parse_tool_request("```tool\nwrite ~/Projects/a.txt\nline one\nline two\n```"),
            Some(ToolRequest::Write {
                path: "~/Projects/a.txt".into(),
                body: "line one\nline two".into()
            })
        );
        assert_eq!(
            parse_tool_request("```tool\nshell\ncargo build --locked\n```"),
            Some(ToolRequest::Shell {
                command: "cargo build --locked".into()
            })
        );
    }

    #[test]
    fn parse_inline_and_json_and_rejects_non_tool() {
        // Command written on the fence line.
        assert_eq!(
            parse_tool_request("```tool read ~/x.md\n```"),
            Some(ToolRequest::Read {
                path: "~/x.md".into()
            })
        );
        // JSON body.
        assert_eq!(
            parse_tool_request("```tool\n{\"op\":\"read\",\"path\":\"~/x.md\"}\n```"),
            Some(ToolRequest::Read {
                path: "~/x.md".into()
            })
        );
        assert_eq!(
            parse_tool_request("```tool\n{\"op\":\"shell\",\"command\":\"ls -la\"}\n```"),
            Some(ToolRequest::Shell {
                command: "ls -la".into()
            })
        );
        // A plain code fence is not a tool, and `tooling` must not match.
        assert!(parse_tool_request("```rust\nfn main(){}\n```").is_none());
        assert!(parse_tool_request("```tooling\nread ~/x\n```").is_none());
        // Unknown op / missing arg.
        assert!(parse_tool_request("```tool\nread\n```").is_none());
        assert!(parse_tool_request("```tool\nfrobnicate ~/x\n```").is_none());
        // Unclosed fence.
        assert!(parse_tool_request("```tool\nread ~/x").is_none());
    }

    // ---- default-on access tools ----

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lillith-fstools-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn access_tools(root: &Path) -> MindTools {
        MindTools::new(
            Allowlist::new(
                vec![root.to_path_buf()],
                default_deny_roots(),
                vec![],
                false,
                4096,
            ),
            Some(root.join("data/audit.log")),
        )
        .with_skills_dir(Some(root.join("skills")))
    }

    #[test]
    fn parses_the_new_tools() {
        let p = |s: &str| parse_tool_request(&format!("```tool\n{s}\n```"));
        assert_eq!(
            p("move ~/Downloads/a b.pdf -> ~/Documents"),
            Some(ToolRequest::Move {
                from: "~/Downloads/a b.pdf".into(),
                to: "~/Documents".into()
            })
        );
        assert_eq!(
            p("rename ~/x.txt -> y.txt"),
            Some(ToolRequest::Rename {
                path: "~/x.txt".into(),
                name: "y.txt".into()
            })
        );
        assert_eq!(
            p("sort ~/Downloads by date"),
            Some(ToolRequest::Sort {
                path: "~/Downloads".into(),
                by: "date".into()
            })
        );
        assert_eq!(
            p("delete ~/old.iso\n~/older.iso"),
            Some(ToolRequest::Delete {
                paths: vec!["~/old.iso".into(), "~/older.iso".into()]
            })
        );
        assert_eq!(
            p("open kate -> ~/notes.md"),
            Some(ToolRequest::Open {
                app: "kate".into(),
                files: vec!["~/notes.md".into()]
            })
        );
        assert_eq!(
            p("run du -sh ~"),
            Some(ToolRequest::Run {
                command: "du -sh ~".into()
            })
        );
        assert_eq!(
            p("installed gimp"),
            Some(ToolRequest::Installed {
                name: "gimp".into()
            })
        );
        assert_eq!(p("list_skills"), Some(ToolRequest::ListSkills));
        assert_eq!(
            p("read_skill tidy"),
            Some(ToolRequest::ReadSkill {
                name: "tidy".into()
            })
        );
        assert_eq!(p("undo"), Some(ToolRequest::Undo));
        assert_eq!(
            p("import_skills"),
            Some(ToolRequest::ImportSkills { from: None })
        );
        assert_eq!(
            p("locate ~ *.pdf"),
            Some(ToolRequest::Locate {
                path: "~".into(),
                pattern: "*.pdf".into()
            })
        );
        assert_eq!(
            parse_tool_request("```tool\n{\"op\":\"delete\",\"paths\":[\"/a\",\"/b\"]}\n```"),
            Some(ToolRequest::Delete {
                paths: vec!["/a".into(), "/b".into()]
            })
        );
    }

    #[test]
    fn delete_is_always_held_for_a_signature() {
        let d = tmp("plan-del");
        std::fs::write(d.join("junk.log"), "x").unwrap();
        let t = access_tools(&d);
        match t.plan(ToolRequest::Delete {
            paths: vec![d.join("junk.log").display().to_string()],
        }) {
            Ok(Planned::Confirm(a @ PendingAction::DeletePaths { .. })) => {
                assert!(a.needs_confirmation());
                // Even a plain confirm cannot run it.
                let mut sc = lillith_core::soul_contract::SoulContract::new(None);
                let id = sc.request(a);
                assert!(sc.confirm(id).is_err());
            }
            other => panic!("expected a held deletion, got {other:?}"),
        }
        assert!(d.join("junk.log").exists());
        // Outside the allowlist / system paths are refused before listing.
        assert!(t
            .plan(ToolRequest::Delete {
                paths: vec!["/usr".into()]
            })
            .is_err());
        let wide = MindTools::new(
            Allowlist::new(vec![PathBuf::from("/")], vec![], vec![], false, 1),
            None,
        );
        assert!(matches!(
            wide.plan(ToolRequest::Delete {
                paths: vec!["/usr".into()]
            }),
            Err(ToolError::Refused(_))
        ));
    }

    #[test]
    fn run_gates_by_command_class() {
        let d = tmp("plan-run");
        let t = access_tools(&d);
        assert!(matches!(
            t.plan(ToolRequest::Run {
                command: "df -h".into()
            }),
            Ok(Planned::Read(PendingAction::RunCommand { .. }))
        ));
        for c in [
            "sudo pacman -Syu",
            "kill 4242",
            "pkexec true",
            "echo x > ~/f",
            "chmod 600 ~/x",
        ] {
            assert!(
                matches!(
                    t.plan(ToolRequest::Run { command: c.into() }),
                    Ok(Planned::Confirm(_))
                ),
                "{c}"
            );
        }
        for c in ["rm -rf ~/x", "find ~ -name '*.tmp' -delete", "sudo rm /x"] {
            assert!(
                matches!(
                    t.plan(ToolRequest::Run { command: c.into() }),
                    Err(ToolError::Refused(_))
                ),
                "{c}"
            );
        }
        // The raw shell tool refuses removal too.
        let sh = MindTools::new(
            Allowlist::new(vec![d.clone()], vec![], vec![], true, 1),
            None,
        );
        assert!(matches!(
            sh.plan(ToolRequest::Shell {
                command: "rm -rf /".into()
            }),
            Err(ToolError::Refused(_))
        ));
        // Switched off.
        let off = access_tools(&d).with_access(lillith_core::config::AccessConfig {
            commands: false,
            programs: false,
            organize: false,
            skills: false,
            ..Default::default()
        });
        assert!(matches!(
            off.plan(ToolRequest::Run {
                command: "ls".into()
            }),
            Err(ToolError::Disabled(_))
        ));
        assert!(matches!(
            off.plan(ToolRequest::Installed { name: "sh".into() }),
            Err(ToolError::Disabled(_))
        ));
        assert!(matches!(
            off.plan(ToolRequest::Undo),
            Err(ToolError::Disabled(_))
        ));
        assert!(matches!(
            off.plan(ToolRequest::ListSkills),
            Err(ToolError::Disabled(_))
        ));
    }

    #[test]
    fn organize_runs_small_batches_and_previews_big_ones() {
        let d = tmp("plan-org");
        for i in 0..12 {
            std::fs::write(d.join(format!("f{i}.png")), "x").unwrap();
        }
        let t = access_tools(&d);
        assert!(matches!(
            t.plan(ToolRequest::Mkdir {
                path: d.join("New").display().to_string()
            }),
            Ok(Planned::Read(PendingAction::Organize { .. }))
        ));
        // 12 files + 1 mkdir > preview_over (10): plan shown, needs a Yes.
        match t.plan(ToolRequest::Sort {
            path: d.display().to_string(),
            by: "type".into(),
        }) {
            Ok(Planned::Confirm(a @ PendingAction::Organize { .. })) => {
                assert!(a.prompt().detail.unwrap().contains("f11.png"));
            }
            other => panic!("{other:?}"),
        }
        assert!(t
            .plan(ToolRequest::Move {
                from: d.join("f0.png").display().to_string(),
                to: "/usr/share".into()
            })
            .is_err());
    }

    #[test]
    fn skills_can_be_listed_and_read_by_name() {
        let d = tmp("plan-skills");
        std::fs::create_dir_all(d.join("skills/scripts")).unwrap();
        std::fs::write(
            d.join("skills/tidy-downloads.md"),
            "# Tidy downloads\nSort them.\n",
        )
        .unwrap();
        std::fs::write(d.join("skills/scripts/backup.sh"), "#!/bin/sh\n").unwrap();
        let t = access_tools(&d);
        let Ok(Planned::Info(list)) = t.plan(ToolRequest::ListSkills) else {
            panic!()
        };
        assert!(
            list.contains("tidy-downloads.md — Tidy downloads: Sort them."),
            "{list}"
        );
        assert!(list.contains("scripts/backup.sh (script"), "{list}");
        for name in [
            "tidy downloads",
            "tidy-downloads",
            "Tidy downloads",
            "tidy-downloads.md",
        ] {
            match t.plan(ToolRequest::ReadSkill { name: name.into() }) {
                Ok(Planned::Read(PendingAction::ReadFile { path, .. })) => {
                    assert_eq!(path, d.join("skills/tidy-downloads.md"), "{name}")
                }
                other => panic!("{name}: {other:?}"),
            }
        }
        assert!(t
            .plan(ToolRequest::ReadSkill {
                name: "../../etc/passwd".into()
            })
            .is_err());
        // A skill script runs through `run`, which asks Ko.
        assert!(matches!(
            t.plan(ToolRequest::Run {
                command: d.join("skills/scripts/backup.sh").display().to_string()
            }),
            Ok(Planned::Confirm(_))
        ));
    }
}
