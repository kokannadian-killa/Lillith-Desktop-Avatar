//! Non-destructive organizing: move, rename, copy and make folders, with an
//! undo journal.
//!
//! - Nothing is ever overwritten: a clash gets an auto-renamed destination
//!   (`report.pdf` → `report (2).pdf`).
//! - Every batch is journaled as one file under the journal folder
//!   (`<data>/lillith/organize-journal/<secs>-<pid>-<n>.tsv`), and
//!   [`undo_last`] reverses the newest batch (moves go back, copies go to the
//!   Trash, empty created folders are removed). `lillith undo-organize` calls it.
//! - [`suggest_sort`] proposes a plan (by type or by month); it does nothing
//!   until the plan is run.

use std::path::{Path, PathBuf};

use super::{now_secs, protected_reason};

/// One organize step. `to` may be an existing folder (the item goes inside it)
/// or a new full path (a rename).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrganizeOp {
    /// Move or rename `from` to `to`.
    Move { from: PathBuf, to: PathBuf },
    /// Copy `from` (file or folder) to `to`.
    Copy { from: PathBuf, to: PathBuf },
    /// Create the folder `path` (and missing parents).
    Mkdir { path: PathBuf },
}

impl OrganizeOp {
    /// One line for a plan preview.
    pub fn describe(&self) -> String {
        match self {
            OrganizeOp::Move { from, to } => format!("move  {} → {}", from.display(), to.display()),
            OrganizeOp::Copy { from, to } => format!("copy  {} → {}", from.display(), to.display()),
            OrganizeOp::Mkdir { path } => format!("mkdir {}", path.display()),
        }
    }

    /// Every path the op touches (for allowlist / protection checks).
    pub fn paths(&self) -> Vec<&Path> {
        match self {
            OrganizeOp::Move { from, to } | OrganizeOp::Copy { from, to } => vec![from, to],
            OrganizeOp::Mkdir { path } => vec![path],
        }
    }
}

/// A batch of organize steps run (and undone) together.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OrganizePlan {
    pub ops: Vec<OrganizeOp>,
}

impl OrganizePlan {
    pub fn len(&self) -> usize {
        self.ops.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
    /// The plan, one step per line.
    pub fn preview(&self) -> String {
        self.ops
            .iter()
            .map(|o| o.describe())
            .collect::<Vec<_>>()
            .join("\n")
    }
    /// Why the plan may not run (a protected path), if any. Sources may not be
    /// system or secret paths; destinations may not be inside them.
    pub fn refusal(&self, data_dir: Option<&Path>) -> Option<String> {
        for op in &self.ops {
            for p in op.paths() {
                let check = match op {
                    // Copying *from* a standard folder is fine; moving it is not.
                    OrganizeOp::Copy { from, .. } if p == from.as_path() => {
                        if super::SYSTEM_TREES.iter().any(|t| p.starts_with(t)) {
                            None
                        } else {
                            secret_reason(p)
                        }
                    }
                    OrganizeOp::Mkdir { .. } => tree_reason(p, data_dir),
                    OrganizeOp::Move { to, .. } | OrganizeOp::Copy { to, .. }
                        if p == to.as_path() =>
                    {
                        tree_reason(p, data_dir)
                    }
                    _ => protected_reason(p, data_dir),
                };
                if let Some(r) = check {
                    return Some(r);
                }
            }
        }
        None
    }
}

fn secret_reason(p: &Path) -> Option<String> {
    let h = super::home()?;
    let real = super::real_path(p);
    super::HOME_SECRET_TREES
        .iter()
        .any(|s| real.starts_with(h.join(s)))
        .then(|| format!("{} holds secrets/keys", p.display()))
}

/// Destinations: anything not inside a system tree / secrets / Lillith data.
fn tree_reason(p: &Path, data_dir: Option<&Path>) -> Option<String> {
    let real = super::real_path(p);
    for t in super::SYSTEM_TREES {
        if real.starts_with(t) || super::normalize(p).starts_with(t) {
            return Some(format!("{} is inside a system folder", p.display()));
        }
    }
    if real.starts_with("/run") && !real.starts_with("/run/media") {
        return Some(format!("{} is a runtime system folder", p.display()));
    }
    if let Some(d) = data_dir {
        if real.starts_with(d) {
            return Some(format!("{} is Lillith's own data", p.display()));
        }
    }
    secret_reason(p)
}

/// `path` if free, else `name (2).ext`, `name (3).ext`, … (never overwrites).
pub fn unique_dest(path: &Path) -> PathBuf {
    unique_dest_with(path, |_| false)
}

/// [`unique_dest`] that also avoids every path `taken` says is spoken for
/// (earlier steps of the same plan).
pub fn unique_dest_with(path: &Path, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let free = |p: &Path| std::fs::symlink_metadata(p).is_err() && !taken(p);
    if free(path) {
        return path.to_path_buf();
    }
    let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    // Keep a multi-part extension like `.tar.gz` together.
    let (stem, ext) = match name.find('.') {
        Some(0) | None => (name.clone(), String::new()),
        Some(i) => (name[..i].to_string(), name[i..].to_string()),
    };
    for n in 2.. {
        let cand = parent.join(format!("{stem} ({n}){ext}"));
        if free(&cand) {
            return cand;
        }
    }
    unreachable!()
}

/// Where `from` actually lands for `to`: inside `to` when it is an existing
/// folder, else `to` itself; then made unique.
fn resolve_dest(from: &Path, to: &Path) -> PathBuf {
    let target = if to.is_dir() && !same_file(from, to) {
        match from.file_name() {
            Some(n) => to.join(n),
            None => to.to_path_buf(),
        }
    } else {
        to.to_path_buf()
    };
    if same_file(from, &target) {
        return target;
    }
    unique_dest(&target)
}

pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// One journal row: what ran and where it actually ended up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    /// `move`, `copy` or `mkdir`.
    pub op: String,
    pub from: PathBuf,
    pub to: PathBuf,
}

/// What [`execute`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OrganizeReport {
    pub done: Vec<JournalEntry>,
    pub failed: Vec<String>,
    /// The journal file for this batch (`None` if nothing ran).
    pub journal: Option<PathBuf>,
}

impl OrganizeReport {
    pub fn text(&self) -> String {
        let mut s = String::new();
        for e in &self.done {
            if e.op == "mkdir" {
                s.push_str(&format!("made folder {}\n", e.to.display()));
            } else {
                s.push_str(&format!(
                    "{} {} → {}\n",
                    e.op,
                    e.from.display(),
                    e.to.display()
                ));
            }
        }
        for f in &self.failed {
            s.push_str(&format!("failed: {f}\n"));
        }
        if !self.done.is_empty() {
            s.push_str("(undo with `lillith undo-organize` or ask me to undo)");
        }
        s.trim_end().to_string()
    }
}

/// Runs `plan`, never overwriting, and journals the batch into `journal_dir`.
/// Steps that fail are reported and skipped; the rest still run.
pub fn execute(
    plan: &OrganizePlan,
    journal_dir: &Path,
    data_dir: Option<&Path>,
) -> std::io::Result<OrganizeReport> {
    if let Some(r) = plan.refusal(data_dir) {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, r));
    }
    let mut report = OrganizeReport::default();
    for op in &plan.ops {
        match run_op(op) {
            Ok(Some(e)) => report.done.push(e),
            Ok(None) => {}
            Err(e) => report.failed.push(format!("{}: {e}", op.describe())),
        }
    }
    if !report.done.is_empty() {
        report.journal = Some(write_batch(journal_dir, &report.done)?);
    }
    Ok(report)
}

/// Writes one undo batch (`op<TAB>from<TAB>to` per line) and returns its file.
pub fn write_batch(journal_dir: &Path, entries: &[JournalEntry]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(journal_dir)?;
    let file = new_batch_file(journal_dir);
    let mut body = String::new();
    for e in entries {
        body.push_str(&format!(
            "{}\t{}\t{}\n",
            e.op,
            esc(&e.from.to_string_lossy()),
            esc(&e.to.to_string_lossy())
        ));
    }
    std::fs::write(&file, body)?;
    Ok(file)
}

fn new_batch_file(dir: &Path) -> PathBuf {
    let base = format!("{:012}-{}", now_secs(), std::process::id());
    for n in 0.. {
        let p = dir.join(format!("{base}-{n:04}.tsv"));
        if !p.exists() {
            return p;
        }
    }
    unreachable!()
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}
fn unesc(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub(crate) fn run_op(op: &OrganizeOp) -> std::io::Result<Option<JournalEntry>> {
    match op {
        OrganizeOp::Mkdir { path } => {
            if path.is_dir() {
                return Ok(None); // already there: nothing to undo
            }
            if path.exists() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "a file with that name exists",
                ));
            }
            // Journal only the topmost folder actually created.
            let mut top = path.clone();
            while let Some(p) = top.parent() {
                if p.exists() {
                    break;
                }
                top = p.to_path_buf();
            }
            std::fs::create_dir_all(path)?;
            Ok(Some(JournalEntry {
                op: "mkdir".into(),
                from: top,
                to: path.clone(),
            }))
        }
        OrganizeOp::Move { from, to } => {
            std::fs::symlink_metadata(from)?;
            let dest = resolve_dest(from, to);
            if same_file(from, &dest) {
                return Ok(None);
            }
            if let Some(p) = dest.parent() {
                std::fs::create_dir_all(p)?;
            }
            move_no_clobber(from, &dest)?;
            Ok(Some(JournalEntry {
                op: "move".into(),
                from: from.clone(),
                to: dest,
            }))
        }
        OrganizeOp::Copy { from, to } => {
            std::fs::symlink_metadata(from)?;
            let mut dest = resolve_dest(from, to);
            if std::fs::symlink_metadata(&dest).is_ok() {
                dest = unique_dest(&dest);
            }
            if let Some(p) = dest.parent() {
                std::fs::create_dir_all(p)?;
            }
            copy_tree(from, &dest)?;
            Ok(Some(JournalEntry {
                op: "copy".into(),
                from: from.clone(),
                to: dest,
            }))
        }
    }
}

/// Renames `from` → `dest` (which must not exist). Across drives a file is
/// copied then the original removed; a folder across drives is refused (copy
/// it instead), so a move can never half-delete a tree.
pub(crate) fn move_no_clobber(from: &Path, dest: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(dest).is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} already exists", dest.display()),
        ));
    }
    match std::fs::rename(from, dest) {
        Ok(()) => Ok(()),
        Err(e) if is_cross_device(&e) => {
            let meta = std::fs::symlink_metadata(from)?;
            if meta.is_dir() {
                return Err(std::io::Error::other(
                    "folder is on another drive: copy it instead",
                ));
            }
            copy_tree(from, dest)?;
            std::fs::remove_file(from)
        }
        Err(e) => Err(e),
    }
}

pub(crate) fn is_cross_device(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(18) // EXDEV
}

/// Copies a file / symlink / folder to `dest` (which must not exist).
pub(crate) fn copy_tree(from: &Path, dest: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(dest).is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} already exists", dest.display()),
        ));
    }
    let meta = std::fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        #[cfg(unix)]
        {
            let t = std::fs::read_link(from)?;
            return std::os::unix::fs::symlink(t, dest);
        }
        #[cfg(not(unix))]
        return Ok(());
    }
    if meta.is_dir() {
        if dest.starts_with(from) {
            return Err(std::io::Error::other("can't copy a folder into itself"));
        }
        std::fs::create_dir_all(dest)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            copy_tree(&e.path(), &dest.join(e.file_name()))?;
        }
        return Ok(());
    }
    // create_new: never clobber even if something appeared meanwhile.
    let mut src = std::fs::File::open(from)?;
    let mut dst = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)?;
    std::io::copy(&mut src, &mut dst)?;
    let _ = dst.set_permissions(meta.permissions());
    Ok(())
}

/// The journal batches, newest first.
pub fn batches(journal_dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(journal_dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "tsv"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v.reverse();
    v
}

/// Reads one journal batch.
pub fn read_batch(file: &Path) -> std::io::Result<Vec<JournalEntry>> {
    let text = std::fs::read_to_string(file)?;
    Ok(text
        .lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, '\t');
            Some(JournalEntry {
                op: it.next()?.to_string(),
                from: PathBuf::from(unesc(it.next()?)),
                to: PathBuf::from(unesc(it.next()?)),
            })
        })
        .collect())
}

/// What [`undo_last`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UndoReport {
    pub batch: Option<PathBuf>,
    pub lines: Vec<String>,
}

impl UndoReport {
    pub fn text(&self) -> String {
        if self.batch.is_none() {
            return "nothing to undo".into();
        }
        self.lines.join("\n")
    }
}

/// Undoes the newest batch in `journal_dir` (in reverse order) and marks it
/// done (`.tsv` → `.undone`). Moves go back (to a fresh name if the original
/// spot was taken meanwhile), copies go to the Trash, created folders are
/// removed only if empty. Nothing is permanently deleted.
pub fn undo_last(journal_dir: &Path) -> std::io::Result<UndoReport> {
    undo_last_in(journal_dir, &super::deletion::home_trash_dir())
}

/// [`undo_last`] with an explicit Trash folder (tests).
pub fn undo_last_in(journal_dir: &Path, trash: &Path) -> std::io::Result<UndoReport> {
    let Some(file) = batches(journal_dir).into_iter().next() else {
        return Ok(UndoReport::default());
    };
    undo_batch_in(&file, trash)
}

/// What undoing `file` would do, one line per step (newest first).
pub fn undo_preview(file: &Path) -> std::io::Result<Vec<String>> {
    Ok(read_batch(file)?
        .iter()
        .rev()
        .map(|e| match e.op.as_str() {
            "move" => format!("move back {} → {}", e.to.display(), e.from.display()),
            "copy" => format!("send copy {} to the Trash", e.to.display()),
            "mkdir" => format!("remove folder {} if empty", e.to.display()),
            o => format!("{o} {}", e.to.display()),
        })
        .collect())
}

/// Undoes one batch file (see [`undo_last_in`]).
pub fn undo_batch_in(file: &Path, trash: &Path) -> std::io::Result<UndoReport> {
    let file = file.to_path_buf();
    let entries = read_batch(&file)?;
    let mut rep = UndoReport {
        batch: Some(file.clone()),
        lines: Vec::new(),
    };
    for e in entries.iter().rev() {
        let line = match e.op.as_str() {
            "move" => {
                if std::fs::symlink_metadata(&e.to).is_err() {
                    format!("skipped: {} is gone", e.to.display())
                } else {
                    let back = unique_dest(&e.from);
                    if let Some(p) = back.parent() {
                        let _ = std::fs::create_dir_all(p);
                    }
                    match move_no_clobber(&e.to, &back) {
                        Ok(()) => format!("moved back {} → {}", e.to.display(), back.display()),
                        Err(err) => format!("failed to move back {}: {err}", e.to.display()),
                    }
                }
            }
            "copy" => {
                if std::fs::symlink_metadata(&e.to).is_err() {
                    format!("skipped: copy {} is gone", e.to.display())
                } else {
                    match super::deletion::trash_put_in(&e.to, trash) {
                        Ok(_) => format!("copy {} sent to Trash", e.to.display()),
                        Err(err) => format!("left copy {} ({err})", e.to.display()),
                    }
                }
            }
            "mkdir" => {
                // Remove created folders bottom-up while empty.
                let mut cur = e.to.clone();
                let mut removed = Vec::new();
                loop {
                    if std::fs::remove_dir(&cur).is_err() {
                        break;
                    }
                    removed.push(cur.display().to_string());
                    if cur == e.from {
                        break;
                    }
                    match cur.parent() {
                        Some(p) if p.starts_with(&e.from) => cur = p.to_path_buf(),
                        _ => break,
                    }
                }
                if removed.is_empty() {
                    format!("kept folder {} (not empty)", e.to.display())
                } else {
                    format!("removed empty folder {}", removed.join(", "))
                }
            }
            other => format!("unknown journal op {other}"),
        };
        rep.lines.push(line);
    }
    let _ = std::fs::rename(&file, file.with_extension("undone"));
    Ok(rep)
}

/// How [`suggest_sort`] groups files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    /// `Images/`, `Documents/`, `Audio/`, …
    Type,
    /// `2026-10/`, by modification month.
    Date,
}

/// Folder name for a file's kind.
pub fn type_folder(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "heic" | "tif" | "tiff"
        | "avif" | "raw" | "cr2" | "nef" => "Images",
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "flv" | "wmv" => "Videos",
        "mp3" | "flac" | "ogg" | "opus" | "wav" | "m4a" | "aac" => "Audio",
        "pdf" | "doc" | "docx" | "odt" | "rtf" | "txt" | "md" | "epub" | "xls" | "xlsx" | "ods"
        | "ppt" | "pptx" | "odp" | "csv" => "Documents",
        "zip" | "tar" | "gz" | "xz" | "zst" | "bz2" | "7z" | "rar" | "iso" => "Archives",
        "rs" | "py" | "js" | "ts" | "c" | "h" | "cpp" | "java" | "go" | "sh" | "fish" | "toml"
        | "json" | "yaml" | "yml" | "html" | "css" => "Code",
        "appimage" | "deb" | "rpm" | "flatpakref" | "exe" | "msi" => "Installers",
        "ttf" | "otf" | "woff" | "woff2" => "Fonts",
        _ => "Other",
    }
}

/// A plan that sorts the files directly inside `dir` into sub-folders (by
/// type or by modification month). Folders and hidden files are left alone.
/// Nothing runs until the plan is executed.
pub fn suggest_sort(dir: &Path, by: SortBy) -> std::io::Result<OrganizePlan> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.path())
        .filter(|p| {
            !p.file_name()
                .map(|n| n.to_string_lossy().starts_with('.'))
                .unwrap_or(true)
        })
        .collect();
    files.sort();
    let mut plan = OrganizePlan::default();
    let mut made: Vec<PathBuf> = Vec::new();
    for f in files {
        let folder = match by {
            SortBy::Type => type_folder(&f).to_string(),
            SortBy::Date => month_of(&f),
        };
        let target = dir.join(&folder);
        if !target.is_dir() && !made.contains(&target) {
            plan.ops.push(OrganizeOp::Mkdir {
                path: target.clone(),
            });
            made.push(target.clone());
        }
        plan.ops.push(OrganizeOp::Move {
            from: f,
            to: target,
        });
    }
    Ok(plan)
}

fn month_of(p: &Path) -> String {
    let secs = std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, m, _) = crate::soul_contract::civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}")
}

#[cfg(test)]
mod tests {
    use super::super::testutil::tmp;
    use super::*;

    #[test]
    fn organize_never_overwrites() {
        let d = tmp("org-noclobber");
        std::fs::write(d.join("a.txt"), "A").unwrap();
        std::fs::create_dir(d.join("box")).unwrap();
        std::fs::write(d.join("box/a.txt"), "OLD").unwrap();
        std::fs::write(d.join("b.txt"), "B").unwrap();
        let plan = OrganizePlan {
            ops: vec![
                OrganizeOp::Move {
                    from: d.join("a.txt"),
                    to: d.join("box"),
                },
                // rename onto an existing name
                OrganizeOp::Move {
                    from: d.join("b.txt"),
                    to: d.join("box/a.txt"),
                },
                OrganizeOp::Copy {
                    from: d.join("box/a.txt"),
                    to: d.join("box/a.txt"),
                },
            ],
        };
        let rep = execute(&plan, &d.join("journal"), None).unwrap();
        assert!(rep.failed.is_empty(), "{:?}", rep.failed);
        assert_eq!(std::fs::read_to_string(d.join("box/a.txt")).unwrap(), "OLD");
        assert_eq!(
            std::fs::read_to_string(d.join("box/a (2).txt")).unwrap(),
            "A"
        );
        assert_eq!(
            std::fs::read_to_string(d.join("box/a (3).txt")).unwrap(),
            "B"
        );
        assert_eq!(
            std::fs::read_to_string(d.join("box/a (4).txt")).unwrap(),
            "OLD"
        );
        assert!(!d.join("a.txt").exists() && !d.join("b.txt").exists());
    }

    #[test]
    fn unique_dest_keeps_double_extensions() {
        let d = tmp("org-unique");
        std::fs::write(d.join("x.tar.gz"), "").unwrap();
        assert_eq!(unique_dest(&d.join("x.tar.gz")), d.join("x (2).tar.gz"));
        assert_eq!(unique_dest(&d.join("free")), d.join("free"));
    }

    #[test]
    fn undo_reverses_the_last_batch() {
        let d = tmp("org-undo");
        std::fs::write(d.join("pic.png"), "P").unwrap();
        std::fs::write(d.join("doc.pdf"), "D").unwrap();
        std::fs::create_dir(d.join("sub")).unwrap();
        std::fs::write(d.join("sub/keep.txt"), "K").unwrap();
        let mut plan = suggest_sort(&d, SortBy::Type).unwrap();
        plan.ops.push(OrganizeOp::Copy {
            from: d.join("sub/keep.txt"),
            to: d.join("sub/keep-copy.txt"),
        });
        let j = d.join("journal");
        let rep = execute(&plan, &j, None).unwrap();
        assert!(d.join("Images/pic.png").exists());
        assert!(d.join("Documents/doc.pdf").exists());
        assert!(d.join("sub/keep-copy.txt").exists());
        assert!(rep.journal.is_some());

        let trash = d.join("xdg/Trash");
        let undo = undo_last_in(&j, &trash).unwrap();
        assert!(undo.batch.is_some(), "{}", undo.text());
        assert_eq!(std::fs::read_to_string(d.join("pic.png")).unwrap(), "P");
        assert_eq!(std::fs::read_to_string(d.join("doc.pdf")).unwrap(), "D");
        assert!(!d.join("Images").exists() && !d.join("Documents").exists());
        assert!(
            !d.join("sub/keep-copy.txt").exists(),
            "copy went to the trash"
        );
        assert!(d.join("xdg/Trash/files/keep-copy.txt").exists());
        assert_eq!(
            std::fs::read_to_string(d.join("sub/keep.txt")).unwrap(),
            "K"
        );
        // Second undo: nothing left.
        assert!(undo_last_in(&j, &trash).unwrap().batch.is_none());
    }

    #[test]
    fn undo_never_clobbers_a_new_file_at_the_old_spot() {
        let d = tmp("org-undo-clash");
        std::fs::write(d.join("a.txt"), "A").unwrap();
        let plan = OrganizePlan {
            ops: vec![OrganizeOp::Move {
                from: d.join("a.txt"),
                to: d.join("b.txt"),
            }],
        };
        let j = d.join("j");
        execute(&plan, &j, None).unwrap();
        std::fs::write(d.join("a.txt"), "NEW").unwrap();
        undo_last(&j).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("a.txt")).unwrap(), "NEW");
        assert_eq!(std::fs::read_to_string(d.join("a (2).txt")).unwrap(), "A");
    }

    #[test]
    fn organize_refuses_system_paths() {
        let d = tmp("org-sys");
        let plan = OrganizePlan {
            ops: vec![OrganizeOp::Move {
                from: PathBuf::from("/etc/hosts"),
                to: d.clone(),
            }],
        };
        assert!(execute(&plan, &d.join("j"), None).is_err());
        let plan = OrganizePlan {
            ops: vec![OrganizeOp::Mkdir {
                path: PathBuf::from("/usr/lillith-test"),
            }],
        };
        assert!(execute(&plan, &d.join("j"), None).is_err());
    }
}
