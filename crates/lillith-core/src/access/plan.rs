//! The Soul Contract **plan review**: every change Lillith wants to make to
//! Ko's machine is laid out as one numbered plan he checks over in detail
//! and then approves in full, approves in part (a checkbox per item), or
//! declines. Nothing touches disk and nothing launches until he does.
//!
//! - Items are listed in execution order. Each shows the operation, the full
//!   source and destination paths, sizes, and the exact auto-renamed name
//!   when a destination is taken (nothing is ever overwritten). Edits carry
//!   a diff, deletes their full recursive listing, program runs the exact
//!   program, its full path, arguments, files, working folder and whether
//!   it is risky.
//! - [`ChangePlan::fingerprint`] covers the items *and* the disk state they
//!   were planned against. A [`PlanApproval`] is bound to that fingerprint
//!   and to the chosen item numbers; it is consumed when used. If the disk
//!   changed since the plan was made, the plan is rebuilt and Ko is asked
//!   again (see `SoulContract::approve_plan`).
//! - Deletes additionally need the typed phrase for exactly the selected
//!   delete items ([`ChangePlan::deletion_manifest`]); Trash is the default.
//! - Reads, listing, searching and "is it installed?" never come here.

use std::collections::{BTreeSet, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use super::deletion::{self, DeleteMode, DeletionManifest};
use super::organize::{self, JournalEntry, OrganizeOp, OrganizePlan};
use super::{human_size, protected_reason};

/// A vetted request for one step (paths already allowlisted by the caller).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PlanRequest {
    /// Make a folder (and missing parents).
    Mkdir { path: PathBuf },
    /// Move/rename `from` to `to` (into `to` when it is, or will be, a folder).
    Move { from: PathBuf, to: PathBuf },
    /// Copy `from` to `to` (same rule).
    Copy { from: PathBuf, to: PathBuf },
    /// Create or replace a file's contents. `skill` = a skill file.
    Write {
        path: PathBuf,
        body: String,
        skill: bool,
    },
    /// Delete a file or folder (Trash by default; needs the typed phrase).
    Delete { path: PathBuf },
    /// Copy one file into the skills folder at exactly `to`.
    SkillCopy {
        from: PathBuf,
        to: PathBuf,
        conflict_with: Option<PathBuf>,
    },
    /// Undo one organize batch (journal file).
    Undo { batch: PathBuf },
    /// Start a program (detached). `app` = desktop-entry id when it is one.
    Launch {
        argv: Vec<String>,
        app: Option<String>,
    },
    /// Run a command line with captured output. `shell` = the raw shell tool.
    Command {
        command: String,
        cwd: Option<PathBuf>,
        timeout_s: u64,
        shell: bool,
    },
    /// Run a Windows `cmd` / PowerShell command (captured). Always a Soul
    /// Contract plan item. When `elevated`, the card shows an
    /// `ELEVATED (admin)` badge and the admin helper runs it after UAC.
    WinCommand {
        shell: super::winshell::WinShell,
        command: String,
        cwd: Option<PathBuf>,
        timeout_s: u64,
        elevated: bool,
        /// From `[mind.access.windows] admin_prompt = "per-command"`.
        admin_per_command: bool,
        /// From `[mind.access.windows] admin_idle_timeout_min`.
        admin_idle_min: u64,
    },
}

impl PlanRequest {
    /// Short operation name shown in the plan.
    pub fn op(&self) -> &'static str {
        match self {
            PlanRequest::Mkdir { .. } => "mkdir",
            PlanRequest::Move { .. } => "move",
            PlanRequest::Copy { .. } => "copy",
            PlanRequest::Write { skill: true, .. } => "write skill",
            PlanRequest::Write { .. } => "write",
            PlanRequest::Delete { .. } => "delete",
            PlanRequest::SkillCopy { .. } => "import skill",
            PlanRequest::Undo { .. } => "undo organize",
            PlanRequest::Launch { .. } => "launch",
            PlanRequest::Command { shell: true, .. } => "shell",
            PlanRequest::Command { .. } => "run",
            PlanRequest::WinCommand {
                shell: super::winshell::WinShell::Cmd,
                elevated: false,
                ..
            } => "run_cmd",
            PlanRequest::WinCommand {
                shell: super::winshell::WinShell::Cmd,
                elevated: true,
                ..
            } => "run_cmd (admin)",
            PlanRequest::WinCommand {
                shell: super::winshell::WinShell::PowerShell,
                elevated: false,
                ..
            } => "run_powershell",
            PlanRequest::WinCommand {
                shell: super::winshell::WinShell::PowerShell,
                elevated: true,
                ..
            } => "run_powershell (admin)",
        }
    }

    /// Program launches and command runs (vs. file changes).
    pub fn is_program(&self) -> bool {
        matches!(
            self,
            PlanRequest::Launch { .. }
                | PlanRequest::Command { .. }
                | PlanRequest::WinCommand { .. }
        )
    }

    /// Whether this step runs elevated (admin helper). Always shown on the card.
    pub fn elevated(&self) -> bool {
        matches!(self, PlanRequest::WinCommand { elevated: true, .. })
    }
}

/// One numbered row of the plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanItem {
    /// 1-based, in execution order.
    pub n: usize,
    pub req: PlanRequest,
    /// Folder this item is grouped under in the review (its destination or
    /// target folder).
    pub group: String,
    /// One-line summary (`move  /a/x.png → /a/Images/x.png`).
    pub line: String,
    /// Labelled facts: from, to, auto-renamed, size, program, path, args,
    /// files, cwd, risky…
    pub fields: Vec<(String, String)>,
    /// Diff of an edit, a delete's full recursive listing, an undo's steps.
    pub detail: Vec<String>,
    /// Final destination (moves, copies, skill copies).
    pub dest: Option<PathBuf>,
    /// Why it is risky (commands/launches), if it is.
    pub risky: Option<String>,
    /// The `ELEVATED (admin)` badge: this step runs through the admin helper.
    pub elevated: bool,
    /// Bytes involved (files moved/copied/written/deleted).
    pub bytes: u64,
    /// For deletes: everything under the path.
    pub manifest: Option<DeletionManifest>,
}

/// A full plan for review. Build it with [`ChangePlan::build`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangePlan {
    pub items: Vec<PlanItem>,
    /// Hash of the items and the disk state they were planned against.
    pub fingerprint: u64,
    /// Undo journal folder (organize steps are journaled as one batch).
    pub journal: Option<PathBuf>,
    /// Lillith's data folder (protected).
    pub data_dir: Option<PathBuf>,
    requests: Vec<PlanRequest>,
}

fn stat_hash(p: &Path, h: &mut impl Hasher) {
    match std::fs::symlink_metadata(p) {
        Ok(m) => {
            true.hash(h);
            m.len().hash(h);
            m.is_dir().hash(h);
            m.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .hash(h);
        }
        Err(_) => false.hash(h),
    }
}

/// Size of a file, or of everything in a folder (capped walk).
fn tree_size(p: &Path) -> u64 {
    let Ok(m) = std::fs::symlink_metadata(p) else {
        return 0;
    };
    if !m.is_dir() {
        return m.len();
    }
    let mut total = 0;
    let mut stack = vec![p.to_path_buf()];
    let mut seen = 0usize;
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            seen += 1;
            if seen > 20_000 {
                return total;
            }
            if let Ok(m) = e.metadata() {
                if m.is_dir() {
                    stack.push(e.path());
                } else {
                    total += m.len();
                }
            }
        }
    }
    total
}

/// A short line diff (`-old` / `+new`), with up to 3 lines of context.
pub fn line_diff(old: &str, new: &str, max_lines: usize) -> Vec<String> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    if a.len() * b.len() > 4_000_000 {
        return vec![format!(
            "(too big to diff: {} → {} lines)",
            a.len(),
            b.len()
        )];
    }
    // LCS table.
    let mut t = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            t[i][j] = if a[i] == b[j] {
                t[i + 1][j + 1] + 1
            } else {
                t[i + 1][j].max(t[i][j + 1])
            };
        }
    }
    let mut ops: Vec<(char, &str)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            ops.push((' ', a[i]));
            i += 1;
            j += 1;
        } else if j < b.len() && (i == a.len() || t[i][j + 1] > t[i + 1][j]) {
            ops.push(('+', b[j]));
            j += 1;
        } else {
            ops.push(('-', a[i]));
            i += 1;
        }
    }
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, o)| o.0 != ' ')
        .map(|(k, _)| k)
        .collect();
    if changed.is_empty() {
        return vec!["(no change)".into()];
    }
    let keep: BTreeSet<usize> = changed
        .iter()
        .flat_map(|&k| k.saturating_sub(3)..=(k + 3).min(ops.len() - 1))
        .collect();
    let mut out = Vec::new();
    let mut last = None;
    for k in keep {
        if last.is_some_and(|l: usize| k > l + 1) {
            out.push("…".into());
        }
        out.push(format!("{}{}", ops[k].0, ops[k].1));
        last = Some(k);
        if out.len() >= max_lines {
            out.push(format!("…[diff cut at {max_lines} lines]"));
            break;
        }
    }
    out
}

const DIFF_MAX: usize = 400;

impl ChangePlan {
    /// Plans `requests` in order against the disk as it is now. Refuses the
    /// whole plan when any step touches a protected/system path.
    pub fn build(
        requests: Vec<PlanRequest>,
        data_dir: Option<&Path>,
        journal: Option<&Path>,
    ) -> Result<Self, String> {
        if requests.is_empty() {
            return Err("the plan is empty".into());
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let mut items = Vec::new();
        let mut claimed: HashSet<PathBuf> = HashSet::new();
        let mut planned_dirs: HashSet<PathBuf> = HashSet::new();
        for (k, req) in requests.iter().enumerate() {
            req.hash(&mut h);
            let mut it = PlanItem {
                n: k + 1,
                req: req.clone(),
                group: String::new(),
                line: String::new(),
                fields: Vec::new(),
                detail: Vec::new(),
                dest: None,
                risky: None,
                elevated: req.elevated(),
                bytes: 0,
                manifest: None,
            };

            match req {
                PlanRequest::Mkdir { path } => {
                    refuse_organize(OrganizeOp::Mkdir { path: path.clone() }, data_dir)?;
                    stat_hash(path, &mut h);
                    it.group = parent_of(path);
                    it.line = format!("mkdir {}", path.display());
                    f(&mut it, "folder", path.display().to_string());
                    if path.is_dir() {
                        f(&mut it, "note", "already exists (nothing to do)".into());
                    }
                    planned_dirs.insert(path.clone());
                }
                PlanRequest::Move { from, to } | PlanRequest::Copy { from, to } => {
                    let is_move = matches!(req, PlanRequest::Move { .. });
                    let op = if is_move {
                        OrganizeOp::Move {
                            from: from.clone(),
                            to: to.clone(),
                        }
                    } else {
                        OrganizeOp::Copy {
                            from: from.clone(),
                            to: to.clone(),
                        }
                    };
                    refuse_organize(op, data_dir)?;
                    if std::fs::symlink_metadata(from).is_err() {
                        return Err(format!("{} doesn't exist", from.display()));
                    }
                    stat_hash(from, &mut h);
                    stat_hash(to, &mut h);
                    let into = (to.is_dir() || planned_dirs.contains(to))
                        && !organize::same_file(from, to);
                    let target = if into {
                        match from.file_name() {
                            Some(n) => to.join(n),
                            None => to.clone(),
                        }
                    } else {
                        to.clone()
                    };
                    let dest = if organize::same_file(from, &target) {
                        target.clone()
                    } else {
                        organize::unique_dest_with(&target, |p| claimed.contains(p))
                    };
                    stat_hash(&dest, &mut h);
                    claimed.insert(dest.clone());
                    it.bytes = tree_size(from);
                    it.group = parent_of(&dest);
                    it.line = format!(
                        "{} {} → {}",
                        if is_move { "move" } else { "copy" },
                        from.display(),
                        dest.display()
                    );
                    f(&mut it, "from", from.display().to_string());
                    f(&mut it, "to", dest.display().to_string());
                    if dest != target {
                        f(
                            &mut it,
                            "auto-renamed",
                            format!("{} is taken → {}", target.display(), dest.display()),
                        );
                    }
                    let v = human_size(it.bytes);
                    f(&mut it, "size", v);
                    it.dest = Some(dest);
                }
                PlanRequest::Write { path, body, skill } => {
                    if !skill {
                        if let Some(r) = protected_reason(path, data_dir) {
                            return Err(r);
                        }
                    } else if let Some(r) = protected_reason(path, None) {
                        return Err(r);
                    }
                    let old = std::fs::read(path).ok();
                    old.hash(&mut h);
                    it.group = parent_of(path);
                    it.bytes = body.len() as u64;
                    f(&mut it, "file", path.display().to_string());
                    match &old {
                        Some(o) => {
                            it.line = format!("edit {} ({} bytes)", path.display(), body.len());
                            let v = format!(
                                "{} → {}",
                                human_size(o.len() as u64),
                                human_size(it.bytes)
                            );
                            f(&mut it, "change", v);
                            it.detail = line_diff(&String::from_utf8_lossy(o), body, DIFF_MAX);
                        }
                        None => {
                            it.line = format!("new file {} ({} bytes)", path.display(), body.len());
                            let v = human_size(it.bytes);
                            f(&mut it, "size", v);
                            it.detail = body
                                .lines()
                                .take(DIFF_MAX)
                                .map(|l| format!("+{l}"))
                                .collect();
                        }
                    }
                }
                PlanRequest::Delete { path } => {
                    let m = DeletionManifest::build(std::slice::from_ref(path), data_dir)
                        .map_err(|e| e.to_string())?;
                    m.fingerprint().hash(&mut h);
                    it.group = parent_of(path);
                    it.bytes = m.bytes();
                    it.line = format!("delete {} ({})", path.display(), m.totals());
                    f(&mut it, "path", path.display().to_string());
                    f(&mut it, "contents", m.totals());
                    it.detail = m.entries.iter().map(deletion::entry_line).collect();
                    it.manifest = Some(m);
                }
                PlanRequest::SkillCopy {
                    from,
                    to,
                    conflict_with,
                } => {
                    if std::fs::symlink_metadata(from).is_err() {
                        return Err(format!("{} doesn't exist", from.display()));
                    }
                    stat_hash(from, &mut h);
                    stat_hash(to, &mut h);
                    if std::fs::symlink_metadata(to).is_ok() || claimed.contains(to) {
                        return Err(format!("{} already exists", to.display()));
                    }
                    claimed.insert(to.clone());
                    it.bytes = tree_size(from);
                    it.group = parent_of(to);
                    it.line = format!("copy skill {} → {}", from.display(), to.display());
                    f(&mut it, "from", from.display().to_string());
                    f(&mut it, "to", to.display().to_string());
                    if let Some(c) = conflict_with {
                        f(
                            &mut it,
                            "auto-renamed",
                            format!(
                                "{} differs from mine → kept yours as {}",
                                c.display(),
                                to.display()
                            ),
                        );
                    }
                    let v = human_size(it.bytes);
                    f(&mut it, "size", v);
                    it.dest = Some(to.clone());
                }
                PlanRequest::Undo { batch } => {
                    let steps = organize::undo_preview(batch).map_err(|e| e.to_string())?;
                    std::fs::read(batch).ok().hash(&mut h);
                    it.group = parent_of(batch);
                    it.line = format!("undo the last organize batch ({} step(s))", steps.len());
                    f(&mut it, "journal", batch.display().to_string());
                    it.detail = steps;
                }
                PlanRequest::Launch { argv, app } => {
                    let prog = argv.first().cloned().unwrap_or_default();
                    let full = super::programs::which(&prog);
                    full.hash(&mut h);
                    it.group = "programs".into();
                    it.line = format!("launch {}", argv.join(" "));
                    if let Some(a) = app {
                        f(&mut it, "app", a.clone());
                    }
                    f(&mut it, "program", prog.clone());
                    f(
                        &mut it,
                        "full path",
                        full.as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "(not found on PATH)".into()),
                    );
                    f(&mut it, "arguments", quote_args(&argv[1.min(argv.len())..]));
                    let files = files_in(&argv[1.min(argv.len())..]);
                    f(&mut it, "files", files_text(&files));
                    f(
                        &mut it,
                        "working folder",
                        super::home()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default(),
                    );
                    it.risky = if app.is_some() {
                        None
                    } else {
                        Some("not an installed app: a bare program".into())
                    };
                    let v = risky_text(&it.risky);
                    f(&mut it, "risky", v);
                }
                PlanRequest::Command {
                    command,
                    cwd,
                    timeout_s,
                    shell,
                } => {
                    use super::commands::{classify, CommandClass};
                    let class = classify(command, &super::secret_roots());
                    if let CommandClass::Removal(r) = &class {
                        return Err(r.clone());
                    }
                    let words = super::commands::split_words(command);
                    let prog = words.first().cloned().unwrap_or_default();
                    let full = super::programs::which(&prog);
                    full.hash(&mut h);
                    it.group = "programs".into();
                    it.line = format!("{} $ {command}", if *shell { "shell" } else { "run" });
                    f(&mut it, "command", command.clone());
                    f(&mut it, "program", prog.clone());
                    f(
                        &mut it,
                        "full path",
                        full.as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "(shell builtin / not on PATH)".into()),
                    );
                    f(
                        &mut it,
                        "arguments",
                        quote_args(words.get(1..).unwrap_or(&[])),
                    );
                    let files = files_in(words.get(1..).unwrap_or(&[]));
                    f(&mut it, "files", files_text(&files));
                    f(
                        &mut it,
                        "working folder",
                        cwd.clone()
                            .or_else(super::home)
                            .map(|p| p.display().to_string())
                            .unwrap_or_default(),
                    );
                    f(&mut it, "timeout", format!("{}s", timeout_s.max(&1)));
                    it.risky = match class {
                        CommandClass::NeedsApproval(r) => Some(r),
                        _ if *shell => Some("raw shell".into()),
                        _ => None,
                    };
                    let v = risky_text(&it.risky);
                    f(&mut it, "risky", v);
                }
                PlanRequest::WinCommand {
                    shell,
                    command,
                    cwd,
                    timeout_s,
                    elevated,
                    admin_per_command: _,
                    admin_idle_min: _,
                } => {
                    let risk = super::winshell::assess(*shell, command)?;
                    shell.hash(&mut h);
                    elevated.hash(&mut h);
                    it.group = "programs".into();
                    it.line = if *elevated {
                        format!("ELEVATED (admin) · {} $ {command}", shell.as_str())
                    } else {
                        format!("{} $ {command}", shell.as_str())
                    };
                    f(&mut it, "command", command.clone());
                    f(&mut it, "shell", shell.program().into());
                    f(
                        &mut it,
                        "invocation",
                        super::winshell::describe(*shell).into(),
                    );
                    f(
                        &mut it,
                        "working folder",
                        cwd.clone()
                            .or_else(super::home)
                            .map(|p| p.display().to_string())
                            .unwrap_or_default(),
                    );
                    f(&mut it, "timeout", format!("{}s", timeout_s.max(&1)));
                    f(
                        &mut it,
                        "admin",
                        if *elevated {
                            "ELEVATED (admin) — UAC helper".into()
                        } else {
                            "no (runs as you)".into()
                        },
                    );
                    it.risky = if *elevated {
                        Some(match risk {
                            Some(r) => format!("ELEVATED (admin); {r}"),
                            None => "ELEVATED (admin)".into(),
                        })
                    } else {
                        risk
                    };
                    let v = risky_text(&it.risky);
                    f(&mut it, "risky", v);
                }
            }
            items.push(it);
        }
        Ok(Self {
            items,
            fingerprint: h.finish(),
            journal: journal.map(Path::to_path_buf),
            data_dir: data_dir.map(Path::to_path_buf),
            requests,
        })
    }

    /// The same requests planned again against the disk as it is now.
    pub fn rebuild(&self) -> Result<Self, String> {
        Self::build(
            self.requests.clone(),
            self.data_dir.as_deref(),
            self.journal.as_deref(),
        )
    }

    /// The requests this plan was built from (in order).
    pub fn requests(&self) -> &[PlanRequest] {
        &self.requests
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// All item numbers.
    pub fn all(&self) -> BTreeSet<usize> {
        self.items.iter().map(|i| i.n).collect()
    }

    /// Item numbers that are deletes.
    pub fn delete_items(&self) -> BTreeSet<usize> {
        self.items
            .iter()
            .filter(|i| i.manifest.is_some())
            .map(|i| i.n)
            .collect()
    }

    /// The combined listing of the selected delete items (what the typed
    /// phrase signs), or `None` when none is selected.
    pub fn deletion_manifest(&self, selected: &BTreeSet<usize>) -> Option<DeletionManifest> {
        let roots: Vec<PathBuf> = self
            .items
            .iter()
            .filter(|i| selected.contains(&i.n))
            .filter_map(|i| match &i.req {
                PlanRequest::Delete { path } => Some(path.clone()),
                _ => None,
            })
            .collect();
        if roots.is_empty() {
            return None;
        }
        DeletionManifest::build(&roots, self.data_dir.as_deref()).ok()
    }

    /// Totals line: counts by kind and bytes.
    pub fn totals(&self, selected: Option<&BTreeSet<usize>>) -> String {
        let pick: Vec<&PlanItem> = self
            .items
            .iter()
            .filter(|i| selected.is_none_or(|s| s.contains(&i.n)))
            .collect();
        let files = pick.iter().filter(|i| !i.req.is_program()).count();
        let progs = pick.len() - files;
        let dels = pick.iter().filter(|i| i.manifest.is_some()).count();
        let bytes: u64 = pick.iter().map(|i| i.bytes).sum();
        format!(
            "{} item(s): {files} file change(s){}, {progs} program run(s), {} involved",
            pick.len(),
            if dels > 0 {
                format!(" incl. {dels} delete(s)")
            } else {
                String::new()
            },
            human_size(bytes)
        )
    }

    /// The whole plan as numbered text (console, logs, the model's note).
    pub fn listing(&self) -> String {
        let mut s = String::new();
        let mut group = None;
        for it in &self.items {
            if group.as_deref() != Some(it.group.as_str()) {
                s.push_str(&format!("[{}]\n", it.group));
                group = Some(it.group.clone());
            }
            s.push_str(&format!("{:>3}. {}\n", it.n, it.line));
            for (k, v) in &it.fields {
                s.push_str(&format!("       {k}: {v}\n"));
            }
            for d in &it.detail {
                s.push_str(&format!("         {d}\n"));
            }
        }
        s.push_str(&format!("— {}", self.totals(None)));
        s
    }

    /// Runs exactly the `selected` items, in order. Organize steps (mkdir,
    /// move, copy, skill copies) go into one undo batch. `deletes` is the
    /// mode the human signed for (required when a delete is selected).
    /// Returns one `(item, result)` per selected item.
    pub fn execute(
        &self,
        selected: &BTreeSet<usize>,
        deletes: Option<DeleteMode>,
        trash: &Path,
        command_output_max: usize,
    ) -> Vec<(usize, Result<String, String>)> {
        let mut out = Vec::new();
        let mut journal: Vec<JournalEntry> = Vec::new();
        for it in self.items.iter().filter(|i| selected.contains(&i.n)) {
            let r: Result<String, String> = match &it.req {
                PlanRequest::Mkdir { path } => {
                    organize::run_op(&OrganizeOp::Mkdir { path: path.clone() })
                        .map(|e| match e {
                            Some(e) => {
                                let s = format!("made folder {}", e.to.display());
                                journal.push(e);
                                s
                            }
                            None => format!("{} already there", path.display()),
                        })
                        .map_err(|e| e.to_string())
                }
                PlanRequest::Move { from, .. } | PlanRequest::Copy { from, .. } => {
                    let dest = it.dest.clone().unwrap_or_default();
                    let is_move = matches!(it.req, PlanRequest::Move { .. });
                    exact_move_copy(from, &dest, is_move).map(|done| {
                        if done {
                            journal.push(JournalEntry {
                                op: if is_move { "move" } else { "copy" }.into(),
                                from: from.clone(),
                                to: dest.clone(),
                            });
                        }
                        format!(
                            "{} {} → {}",
                            if is_move { "moved" } else { "copied" },
                            from.display(),
                            dest.display()
                        )
                    })
                }
                PlanRequest::SkillCopy { from, to, .. } => (|| {
                    if let Some(p) = to.parent() {
                        std::fs::create_dir_all(p)?;
                    }
                    let meta = std::fs::metadata(from)?;
                    super::skill_import::copy_new(from, to, &meta)?;
                    Ok::<_, std::io::Error>(())
                })()
                .map(|()| {
                    journal.push(JournalEntry {
                        op: "copy".into(),
                        from: from.clone(),
                        to: to.clone(),
                    });
                    if let Some(d) = &self.data_dir {
                        let ts = crate::soul_contract::format_utc(std::time::SystemTime::now());
                        let _ = super::append_line(
                            &d.join("skills-import.log"),
                            &format!("{ts}\t+\t{}\t{}", from.display(), to.display()),
                        );
                    }
                    format!("copied {} → {}", from.display(), to.display())
                })
                .map_err(|e| e.to_string()),
                PlanRequest::Write { path, body, .. } => atomic_write(path, body.as_bytes())
                    .map(|()| format!("wrote {} ({} bytes)", path.display(), body.len()))
                    .map_err(|e| e.to_string()),
                PlanRequest::Delete { .. } => match (deletes, &it.manifest) {
                    (Some(mode), Some(m)) => {
                        let (done, err) = deletion::execute_in(m, mode, trash);
                        if let Some(d) = &self.data_dir {
                            let ts = crate::soul_contract::format_utc(std::time::SystemTime::now());
                            for l in &done {
                                let _ = super::append_line(
                                    &d.join("deletions.log"),
                                    &format!("{ts}\t{}\t{l}", mode.as_str()),
                                );
                            }
                            if let Some(e) = &err {
                                let _ = super::append_line(
                                    &d.join("deletions.log"),
                                    &format!("{ts}\tERROR\t{e}"),
                                );
                            }
                        }
                        match err {
                            None => Ok(done.join("\n")),
                            Some(e) => Err(format!("stopped after {}: {e}", done.len())),
                        }
                    }
                    _ => Err("not signed: nothing deleted".into()),
                },
                PlanRequest::Undo { batch } => organize::undo_batch_in(batch, trash)
                    .map(|r| r.text())
                    .map_err(|e| e.to_string()),
                PlanRequest::Launch { argv, .. } => super::programs::spawn_detached(argv)
                    .map(|pid| format!("started {} (pid {pid})", argv.join(" ")))
                    .map_err(|e| e.to_string()),
                PlanRequest::Command {
                    command,
                    cwd,
                    timeout_s,
                    ..
                } => super::commands::run_captured(
                    command,
                    cwd.as_deref(),
                    std::time::Duration::from_secs((*timeout_s).max(1)),
                    command_output_max,
                )
                .map_err(|e| e.to_string()),
                PlanRequest::WinCommand {
                    shell,
                    command,
                    cwd,
                    timeout_s,
                    elevated,
                    admin_per_command,
                    admin_idle_min,
                } => {
                    let t = std::time::Duration::from_secs((*timeout_s).max(1));
                    if *elevated {
                        match super::admin_broker::default_helper_path() {
                            None => Err("cannot find lillith-admin.exe".to_string()),
                            Some(helper) => super::admin_broker::run_elevated(
                                super::admin_broker::ElevatedRun {
                                    shell: *shell,
                                    command,
                                    cwd: cwd.as_deref(),
                                    timeout: t,
                                    cap: command_output_max,
                                    per_command: *admin_per_command,
                                    idle_min: (*admin_idle_min).max(1),
                                    helper: &helper,
                                },
                            )
                            .map_err(|e| e.to_string()),
                        }
                    } else {
                        super::winshell::run(*shell, command, cwd.as_deref(), t, command_output_max)
                            .map_err(|e| e.to_string())
                    }
                }
            };
            out.push((it.n, r));
        }
        if !journal.is_empty() {
            if let Some(j) = &self.journal {
                let _ = organize::write_batch(j, &journal);
            }
        }
        out
    }
}

fn f(it: &mut PlanItem, k: &str, v: String) {
    it.fields.push((k.into(), v));
}

fn refuse_organize(op: OrganizeOp, data_dir: Option<&Path>) -> Result<(), String> {
    match (OrganizePlan { ops: vec![op] }).refusal(data_dir) {
        Some(r) => Err(r),
        None => Ok(()),
    }
}

/// Move/copy to exactly `dest` (which must not exist). `Ok(false)` = no-op.
fn exact_move_copy(from: &Path, dest: &Path, is_move: bool) -> Result<bool, String> {
    if organize::same_file(from, dest) {
        return Ok(false);
    }
    if std::fs::symlink_metadata(dest).is_ok() {
        return Err(format!(
            "{} appeared meanwhile; nothing overwritten",
            dest.display()
        ));
    }
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let r = if is_move {
        organize::move_no_clobber(from, dest)
    } else {
        organize::copy_tree(from, dest)
    };
    r.map(|()| true).map_err(|e| e.to_string())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!(
        "{}lillith-tmp",
        path.extension()
            .map(|e| format!("{}.", e.to_string_lossy()))
            .unwrap_or_default()
    ));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

fn parent_of(p: &Path) -> String {
    p.parent()
        .map(|d| d.display().to_string())
        .unwrap_or_else(|| "/".into())
}

fn quote_args(a: &[String]) -> String {
    if a.is_empty() {
        return "(none)".into();
    }
    a.iter()
        .map(|s| {
            if s.contains(char::is_whitespace) {
                format!("{s:?}")
            } else {
                s.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Arguments that name existing files/folders (absolute or `~`).
fn files_in(args: &[String]) -> Vec<PathBuf> {
    args.iter()
        .filter(|a| !a.starts_with('-'))
        .filter_map(|a| {
            let p = if let Some(rest) = a.strip_prefix("~/") {
                super::home()?.join(rest)
            } else {
                PathBuf::from(a)
            };
            (p.is_absolute() && std::fs::symlink_metadata(&p).is_ok()).then_some(p)
        })
        .collect()
}

fn files_text(f: &[PathBuf]) -> String {
    if f.is_empty() {
        "(none named)".into()
    } else {
        f.iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn risky_text(r: &Option<String>) -> String {
    match r {
        Some(r) => format!("YES: {r}"),
        None => "no (read-only / installed app)".into(),
    }
}

/// Ko's go-ahead for exactly some items of exactly one plan. Single use
/// (consumed by `SoulContract::approve_plan`).
#[derive(Debug, PartialEq, Eq)]
pub struct PlanApproval {
    fingerprint: u64,
    selected: BTreeSet<usize>,
}

impl PlanApproval {
    /// Approve every item.
    pub fn all(plan: &ChangePlan) -> Self {
        Self {
            fingerprint: plan.fingerprint,
            selected: plan.all(),
        }
    }

    /// Approve only `items` (numbers not in the plan are ignored).
    pub fn subset(plan: &ChangePlan, items: impl IntoIterator<Item = usize>) -> Self {
        let all = plan.all();
        Self {
            fingerprint: plan.fingerprint,
            selected: items.into_iter().filter(|n| all.contains(n)).collect(),
        }
    }

    pub fn selected(&self) -> &BTreeSet<usize> {
        &self.selected
    }

    pub(crate) fn fingerprint(&self) -> u64 {
        self.fingerprint
    }
}

/// Parses a console answer: `all`, `none`, or a list like `1,3-5`.
/// `Ok(None)` = decline.
pub fn parse_selection(input: &str, max: usize) -> Result<Option<BTreeSet<usize>>, String> {
    let t = input.trim().to_ascii_lowercase();
    match t.as_str() {
        "all" | "a" | "yes" | "y" => return Ok(Some((1..=max).collect())),
        "none" | "n" | "no" | "" | "decline" => return Ok(None),
        _ => {}
    }
    let mut out = BTreeSet::new();
    for part in t.split([',', ' ']).filter(|p| !p.is_empty()) {
        let (a, b) = match part.split_once('-') {
            Some((a, b)) => (a.trim(), b.trim()),
            None => (part, part),
        };
        let a: usize = a.parse().map_err(|_| format!("{part:?} is not a number"))?;
        let b: usize = b.parse().map_err(|_| format!("{part:?} is not a number"))?;
        if a == 0 || b < a || b > max {
            return Err(format!("{part:?} is not in 1-{max}"));
        }
        out.extend(a..=b);
    }
    if out.is_empty() {
        return Ok(None);
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::super::testutil::tmp;
    use super::*;

    #[test]
    fn plan_shows_exact_destinations_and_auto_renames_without_touching_disk() {
        let d = tmp("plan-show");
        std::fs::write(d.join("a.png"), b"1234").unwrap();
        std::fs::write(d.join("b.png"), b"5").unwrap();
        std::fs::create_dir_all(d.join("Pics")).unwrap();
        std::fs::write(d.join("Pics/a.png"), b"old").unwrap();
        let reqs = vec![
            PlanRequest::Mkdir {
                path: d.join("New"),
            },
            PlanRequest::Move {
                from: d.join("a.png"),
                to: d.join("Pics"),
            },
            PlanRequest::Copy {
                from: d.join("b.png"),
                to: d.join("New"),
            },
        ];
        let plan = ChangePlan::build(reqs, None, Some(&d.join("j"))).unwrap();
        assert_eq!(
            plan.items[1].dest.as_deref(),
            Some(d.join("Pics/a (2).png").as_path())
        );
        assert!(plan.items[1]
            .fields
            .iter()
            .any(|(k, v)| k == "auto-renamed" && v.contains("a (2).png")));
        // Into the folder the plan makes first.
        assert_eq!(
            plan.items[2].dest.as_deref(),
            Some(d.join("New/b.png").as_path())
        );
        let l = plan.listing();
        assert!(l.contains("  1. mkdir") && l.contains("— 3 item(s)"), "{l}");
        // Nothing happened.
        assert!(!d.join("New").exists() && d.join("a.png").exists());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn subset_runs_only_chosen_items_and_journals_them() {
        let d = tmp("plan-subset");
        for f in ["1.txt", "2.txt", "3.txt"] {
            std::fs::write(d.join(f), f).unwrap();
        }
        std::fs::create_dir_all(d.join("Docs")).unwrap();
        let reqs = ["1.txt", "2.txt", "3.txt"]
            .iter()
            .map(|f| PlanRequest::Move {
                from: d.join(f),
                to: d.join("Docs"),
            })
            .collect();
        let plan = ChangePlan::build(reqs, None, Some(&d.join("j"))).unwrap();
        let sel: BTreeSet<usize> = [1, 3].into();
        let res = plan.execute(&sel, None, &d.join("trash"), 1000);
        assert_eq!(res.len(), 2);
        assert!(d.join("Docs/1.txt").exists() && d.join("Docs/3.txt").exists());
        assert!(d.join("2.txt").exists(), "unchosen item untouched");
        let b = organize::batches(&d.join("j"));
        assert_eq!(organize::read_batch(&b[0]).unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn edits_show_a_diff_and_the_fingerprint_tracks_the_disk() {
        let d = tmp("plan-diff");
        let f = d.join("notes.txt");
        std::fs::write(&f, "a\nb\nc\n").unwrap();
        let plan = ChangePlan::build(
            vec![PlanRequest::Write {
                path: f.clone(),
                body: "a\nB\nc\n".into(),
                skill: false,
            }],
            None,
            None,
        )
        .unwrap();
        assert_eq!(plan.items[0].detail, [" a", "-b", "+B", " c"]);
        assert_eq!(plan.rebuild().unwrap().fingerprint, plan.fingerprint);
        std::fs::write(&f, "changed\n").unwrap();
        assert_ne!(plan.rebuild().unwrap().fingerprint, plan.fingerprint);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn selection_parsing() {
        assert_eq!(parse_selection("all", 4).unwrap().unwrap().len(), 4);
        assert_eq!(parse_selection("none", 4).unwrap(), None);
        assert_eq!(parse_selection("", 4).unwrap(), None);
        assert_eq!(
            parse_selection("1,3-4", 4).unwrap().unwrap(),
            [1, 3, 4].into()
        );
        assert!(parse_selection("5", 4).is_err());
        assert!(parse_selection("x", 4).is_err());
    }

    #[test]
    fn programs_show_path_args_files_and_risk() {
        let d = tmp("plan-prog");
        std::fs::write(d.join("f.txt"), "x").unwrap();
        let cmd = format!("cat {}", d.join("f.txt").display());
        let plan = ChangePlan::build(
            vec![
                PlanRequest::Command {
                    command: cmd,
                    cwd: Some(d.clone()),
                    timeout_s: 5,
                    shell: false,
                },
                PlanRequest::Command {
                    command: "chmod 600 /tmp/x".into(),
                    cwd: None,
                    timeout_s: 5,
                    shell: false,
                },
            ],
            None,
            None,
        )
        .unwrap();
        let get = |i: usize, k: &str| {
            plan.items[i]
                .fields
                .iter()
                .find(|f| f.0 == k)
                .map(|f| f.1.clone())
                .unwrap()
        };
        assert_eq!(get(0, "program"), "cat");
        assert!(
            get(0, "full path").ends_with("/cat"),
            "{}",
            get(0, "full path")
        );
        assert!(get(0, "files").contains("f.txt"));
        assert_eq!(get(0, "working folder"), d.display().to_string());
        assert!(get(0, "risky").starts_with("no"));
        assert!(get(1, "risky").starts_with("YES"), "{}", get(1, "risky"));
        // Removal commands never make it into a plan.
        assert!(ChangePlan::build(
            vec![PlanRequest::Command {
                command: "rm -rf /tmp/x".into(),
                cwd: None,
                timeout_s: 5,
                shell: false
            }],
            None,
            None
        )
        .is_err());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn win_command_shows_admin_badge_and_never_runs_without_confirm() {
        let plan = ChangePlan::build(
            vec![
                PlanRequest::WinCommand {
                    shell: super::super::winshell::WinShell::Cmd,
                    command: "ver".into(),
                    cwd: None,
                    timeout_s: 5,
                    elevated: false,
                    admin_per_command: false,
                    admin_idle_min: 15,
                },
                PlanRequest::WinCommand {
                    shell: super::super::winshell::WinShell::PowerShell,
                    command: "Get-Process".into(),
                    cwd: None,
                    timeout_s: 5,
                    elevated: true,
                    admin_per_command: false,
                    admin_idle_min: 15,
                },
            ],
            None,
            None,
        )
        .unwrap();
        assert!(!plan.items[0].elevated);
        assert!(plan.items[1].elevated);
        assert!(plan.items[1].line.contains("ELEVATED (admin)"));
        assert!(plan.items[1]
            .fields
            .iter()
            .any(|(k, v)| k == "admin" && v.contains("ELEVATED")));
        // Removals never become a plan item.
        assert!(ChangePlan::build(
            vec![PlanRequest::WinCommand {
                shell: super::super::winshell::WinShell::Cmd,
                command: "del /q C:\\x".into(),
                cwd: None,
                timeout_s: 5,
                elevated: true,
                admin_per_command: false,
                admin_idle_min: 15,
            }],
            None,
            None
        )
        .is_err());
        // Execute is Windows-only; on Linux it returns an error without
        // spawning anything (and never runs without going through approve).
        #[cfg(not(windows))]
        {
            let sel: BTreeSet<usize> = [1].into();
            let res = plan.execute(&sel, None, &std::env::temp_dir(), 1000);
            assert!(res[0].1.is_err());
        }
    }
}
