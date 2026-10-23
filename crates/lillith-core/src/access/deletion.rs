//! Deleting, the only way: a full listing, a signed Soul Contract, then the
//! Trash (or, only if the signer picks it, permanent removal).
//!
//! 1. [`DeletionManifest::build`] walks every requested path (recursively, no
//!    symlink following) and records every file and folder with its size and
//!    modification time, plus totals and a fingerprint. System paths are
//!    refused here (see [`super::protected_reason`]).
//! 2. The manifest is filed as
//!    [`PendingAction::DeletePaths`](crate::soul_contract::PendingAction::DeletePaths).
//!    The ordinary Yes ([`SoulContract::confirm`](crate::soul_contract::SoulContract::confirm))
//!    refuses it. Only [`SoulContract::sign_deletion`](crate::soul_contract::SoulContract::sign_deletion)
//!    with a [`DeletionSignature`] runs it, and a signature can only be made by
//!    typing the manifest's phrase (`DELETE 12 ITEMS`), which only the GUI /
//!    console does for the human; the model has no way to produce one.
//! 3. Before anything is removed the paths are listed again; if anything
//!    changed the request is re-filed and the human is asked again.
//! 4. Every removed path is written to `deletions.log`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use super::{human_size, protected_reason};

/// Most entries one contract may list (a bigger set must be split).
pub const MAX_ENTRIES: usize = 20_000;

/// What a listed entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    Other,
}

/// One file or folder that would be deleted.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeleteEntry {
    /// Absolute path.
    pub path: PathBuf,
    pub kind: EntryKind,
    /// Bytes (0 for folders).
    pub size: u64,
    /// Modification time, ns since the epoch.
    pub mtime_ns: i128,
    /// Nesting below the requested root (0 = the root itself).
    pub depth: usize,
}

/// Everything a deletion would remove, listed up front.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionManifest {
    /// The paths that were asked for (absolute).
    pub roots: Vec<PathBuf>,
    /// Every file and folder, each root followed by its contents.
    pub entries: Vec<DeleteEntry>,
}

/// Why a manifest could not be built.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("refused: {0}")]
    Protected(String),
    #[error("{0} does not exist")]
    Missing(PathBuf),
    #[error("too many items ({0}+); delete a smaller set at a time")]
    TooMany(usize),
    #[error("nothing to delete")]
    Empty,
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

impl DeletionManifest {
    /// Lists `paths` (absolute or `~`) and everything inside them.
    /// `data_dir` (Lillith's own logs) is protected along with system paths.
    pub fn build(paths: &[PathBuf], data_dir: Option<&Path>) -> Result<Self, ManifestError> {
        let mut roots: Vec<PathBuf> = Vec::new();
        for p in paths {
            let abs = super::real_path(p);
            if let Some(r) = protected_reason(p, data_dir) {
                return Err(ManifestError::Protected(r));
            }
            if std::fs::symlink_metadata(&abs).is_err() {
                return Err(ManifestError::Missing(abs));
            }
            // Skip a root already covered by another one.
            if roots.iter().any(|r| abs.starts_with(r)) {
                continue;
            }
            roots.retain(|r| !r.starts_with(&abs));
            roots.push(abs);
        }
        if roots.is_empty() {
            return Err(ManifestError::Empty);
        }
        roots.sort();
        let mut entries = Vec::new();
        for r in &roots {
            walk(r, 0, &mut entries)?;
        }
        Ok(Self { roots, entries })
    }

    /// Lists the same roots again (to compare before deleting).
    pub fn relist(&self, data_dir: Option<&Path>) -> Result<Self, ManifestError> {
        Self::build(&self.roots, data_dir)
    }

    pub fn files(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.kind != EntryKind::Dir)
            .count()
    }
    pub fn dirs(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.kind == EntryKind::Dir)
            .count()
    }
    pub fn bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.size).sum()
    }

    /// A hash over every path, kind, size and mtime: any change on disk
    /// changes it.
    pub fn fingerprint(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.roots.hash(&mut h);
        self.entries.hash(&mut h);
        h.finish()
    }

    /// What the human must type to sign: `DELETE <n> ITEMS`.
    pub fn phrase(&self) -> String {
        let n = self.entries.len();
        if n == 1 {
            "DELETE 1 ITEM".into()
        } else {
            format!("DELETE {n} ITEMS")
        }
    }

    /// `N files, M folders, X total`.
    pub fn totals(&self) -> String {
        format!(
            "{} file{}, {} folder{}, {} total",
            self.files(),
            if self.files() == 1 { "" } else { "s" },
            self.dirs(),
            if self.dirs() == 1 { "" } else { "s" },
            human_size(self.bytes())
        )
    }

    /// The full listing: one line per entry (indented by depth), every path
    /// absolute, then the totals. Nothing is elided.
    pub fn listing(&self) -> String {
        let mut s = String::new();
        for e in &self.entries {
            s.push_str(&entry_line(e));
            s.push('\n');
        }
        s.push_str(&format!("— {}", self.totals()));
        s
    }
}

/// One listing row: `  [dir]  /abs/path/` / `  1.2 MiB  /abs/file`.
pub fn entry_line(e: &DeleteEntry) -> String {
    let indent = "  ".repeat(e.depth.min(12));
    match e.kind {
        EntryKind::Dir => format!("{indent}[dir]     {}/", e.path.display()),
        EntryKind::Symlink => {
            let t = std::fs::read_link(&e.path)
                .map(|t| t.display().to_string())
                .unwrap_or_default();
            format!("{indent}[link]    {} -> {t}", e.path.display())
        }
        _ => format!("{indent}{:>9} {}", human_size(e.size), e.path.display()),
    }
}

fn walk(p: &Path, depth: usize, out: &mut Vec<DeleteEntry>) -> Result<(), ManifestError> {
    if out.len() >= MAX_ENTRIES {
        return Err(ManifestError::TooMany(MAX_ENTRIES));
    }
    let meta = std::fs::symlink_metadata(p)?;
    let ft = meta.file_type();
    let kind = if ft.is_symlink() {
        EntryKind::Symlink
    } else if ft.is_dir() {
        EntryKind::Dir
    } else if ft.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    };
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i128)
        .unwrap_or(0);
    out.push(DeleteEntry {
        path: p.to_path_buf(),
        kind,
        size: if kind == EntryKind::Dir {
            0
        } else {
            meta.len()
        },
        mtime_ns,
        depth,
    });
    if kind == EntryKind::Dir {
        let mut kids: Vec<PathBuf> = std::fs::read_dir(p)?
            .map(|e| e.map(|e| e.path()))
            .collect::<Result<_, _>>()?;
        kids.sort();
        for k in kids {
            walk(&k, depth + 1, out)?;
        }
    }
    Ok(())
}

/// Trash (default, recoverable) or permanent removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeleteMode {
    #[default]
    Trash,
    Permanent,
}

impl DeleteMode {
    pub fn as_str(self) -> &'static str {
        match self {
            DeleteMode::Trash => "trash",
            DeleteMode::Permanent => "permanent",
        }
    }
}

/// The human's signature on one exact manifest. It can only be made by
/// [`DeletionSignature::sign`] with the typed phrase, carries the manifest's
/// fingerprint, and is consumed by
/// [`SoulContract::sign_deletion`](crate::soul_contract::SoulContract::sign_deletion).
#[derive(Debug, PartialEq, Eq)]
pub struct DeletionSignature {
    fingerprint: u64,
    mode: DeleteMode,
}

/// Why a signature was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignError {
    #[error("type exactly \"{0}\" to sign")]
    WrongPhrase(String),
}

impl DeletionSignature {
    /// Signs `manifest` if `typed` is its [`DeletionManifest::phrase`]
    /// (surrounding spaces and letter case ignored).
    pub fn sign(
        manifest: &DeletionManifest,
        typed: &str,
        mode: DeleteMode,
    ) -> Result<Self, SignError> {
        let want = manifest.phrase();
        let norm = |s: &str| {
            s.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_uppercase()
        };
        if norm(typed) != want {
            return Err(SignError::WrongPhrase(want));
        }
        Ok(Self {
            fingerprint: manifest.fingerprint(),
            mode,
        })
    }
    pub fn mode(&self) -> DeleteMode {
        self.mode
    }
    pub(crate) fn fingerprint(&self) -> u64 {
        self.fingerprint
    }
}

/// Removes exactly the listed set. Trash moves each root into the freedesktop
/// Trash; Permanent removes the listed entries deepest-first with
/// non-recursive calls, so anything that appeared inside a folder after the
/// listing makes that folder's removal fail instead of being deleted unseen.
/// Returns one line per removed path and the first error, if any (it stops
/// there).
pub fn execute(
    manifest: &DeletionManifest,
    mode: DeleteMode,
) -> (Vec<String>, Option<std::io::Error>) {
    execute_in(manifest, mode, &home_trash_dir())
}

/// [`execute`] with an explicit Trash folder.
pub fn execute_in(
    manifest: &DeletionManifest,
    mode: DeleteMode,
    trash: &Path,
) -> (Vec<String>, Option<std::io::Error>) {
    let mut done = Vec::new();
    match mode {
        DeleteMode::Trash => {
            for r in &manifest.roots {
                match trash_put_in(r, trash) {
                    Ok((where_, _)) => {
                        done.push(format!("{} → Trash ({})", r.display(), where_.display()))
                    }
                    Err(e) => return (done, Some(e)),
                }
            }
        }
        DeleteMode::Permanent => {
            let mut es: Vec<&DeleteEntry> = manifest.entries.iter().collect();
            es.sort_by(|a, b| {
                b.path
                    .components()
                    .count()
                    .cmp(&a.path.components().count())
            });
            for e in es {
                let r = if e.kind == EntryKind::Dir {
                    std::fs::remove_dir(&e.path)
                } else {
                    std::fs::remove_file(&e.path)
                };
                if let Err(err) = r {
                    return (
                        done,
                        Some(std::io::Error::new(
                            err.kind(),
                            format!("{}: {err}", e.path.display()),
                        )),
                    );
                }
                done.push(format!("{} (permanently deleted)", e.path.display()));
            }
        }
    }
    (done, None)
}

/// `$XDG_DATA_HOME/Trash` (default `~/.local/share/Trash`).
pub fn home_trash_dir() -> PathBuf {
    if let Some(x) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(x).join("Trash");
    }
    super::home()
        .map(|h| h.join(".local/share/Trash"))
        .unwrap_or_else(|| PathBuf::from(".Trash"))
}

/// Moves `path` into the user's Trash.
pub fn trash_put(path: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
    trash_put_in(path, &home_trash_dir())
}

/// Moves `path` into the freedesktop Trash at `trash` (`files/` + a
/// `.trashinfo` in `info/`). If it lives on another drive, the drive's own
/// `$topdir/.Trash-$uid` is used. Returns `(trashed file, info file)`.
pub fn trash_put_in(path: &Path, trash: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
    let abs = super::normalize(path);
    match put_into(&abs, trash, &abs) {
        Err(e) if super::organize::is_cross_device(&e) => {
            let top = mount_top(&abs)?;
            let uid = current_uid();
            let t = top.join(format!(".Trash-{uid}"));
            let rel = abs.strip_prefix(&top).unwrap_or(&abs).to_path_buf();
            put_into(&abs, &t, &rel).map_err(|e| {
                std::io::Error::new(
                    e.kind(),
                    format!(
                        "can't move {} to a Trash on its drive ({e}); choose permanent delete instead",
                        abs.display()
                    ),
                )
            })
        }
        r => r,
    }
}

fn put_into(abs: &Path, trash: &Path, recorded: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
    let files = trash.join("files");
    let info = trash.join("info");
    std::fs::create_dir_all(&files)?;
    std::fs::create_dir_all(&info)?;
    let base = abs
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "item".into());
    for n in 1u32.. {
        let name = if n == 1 {
            base.clone()
        } else {
            format!("{base}.{n}")
        };
        let info_path = info.join(format!("{name}.trashinfo"));
        // Reserve the name atomically with the info file.
        let f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&info_path);
        let mut f = match f {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        let dest = files.join(&name);
        if std::fs::symlink_metadata(&dest).is_ok() {
            let _ = std::fs::remove_file(&info_path);
            continue;
        }
        use std::io::Write;
        let date = crate::soul_contract::format_utc(std::time::SystemTime::now());
        let date = date.trim_end_matches('Z');
        write!(
            f,
            "[Trash Info]\nPath={}\nDeletionDate={date}\n",
            url_encode(&recorded.to_string_lossy())
        )?;
        drop(f);
        if let Err(e) = std::fs::rename(abs, &dest) {
            let _ = std::fs::remove_file(&info_path);
            return Err(e);
        }
        return Ok((dest, info_path));
    }
    unreachable!()
}

fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(unix)]
fn current_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self")
        .map(|m| m.uid())
        .unwrap_or(1000)
}
#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

#[cfg(unix)]
fn mount_top(p: &Path) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let dev = std::fs::symlink_metadata(p)?.dev();
    let mut top = p.to_path_buf();
    while let Some(parent) = top.parent() {
        match std::fs::metadata(parent) {
            Ok(m) if m.dev() == dev => top = parent.to_path_buf(),
            _ => break,
        }
    }
    Ok(top)
}
#[cfg(not(unix))]
fn mount_top(p: &Path) -> std::io::Result<PathBuf> {
    Ok(p.parent().unwrap_or(p).to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::super::testutil::tmp;
    use super::*;

    #[test]
    fn manifest_lists_every_entry_recursively() {
        let d = tmp("del-manifest");
        std::fs::create_dir_all(d.join("old/deep")).unwrap();
        std::fs::write(d.join("old/a.txt"), "aaaa").unwrap();
        std::fs::write(d.join("old/deep/b.bin"), "bb").unwrap();
        std::fs::write(d.join("lone.log"), "x").unwrap();
        let m = DeletionManifest::build(&[d.join("old"), d.join("lone.log")], None).unwrap();
        let paths: Vec<_> = m.entries.iter().map(|e| e.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                d.join("lone.log"),
                d.join("old"),
                d.join("old/a.txt"),
                d.join("old/deep"),
                d.join("old/deep/b.bin"),
            ]
        );
        assert_eq!((m.files(), m.dirs(), m.bytes()), (3, 2, 7));
        let l = m.listing();
        for p in &paths {
            assert!(l.contains(&p.display().to_string()), "{l}");
        }
        assert!(l.contains("3 files, 2 folders"));
        assert_eq!(m.phrase(), "DELETE 5 ITEMS");
    }

    #[test]
    fn signing_needs_the_exact_phrase() {
        let d = tmp("del-sign");
        std::fs::write(d.join("x"), "1").unwrap();
        let m = DeletionManifest::build(&[d.join("x")], None).unwrap();
        assert!(DeletionSignature::sign(&m, "yes", DeleteMode::Trash).is_err());
        assert!(DeletionSignature::sign(&m, "DELETE 2 ITEMS", DeleteMode::Trash).is_err());
        assert!(DeletionSignature::sign(&m, " delete 1 item ", DeleteMode::Trash).is_ok());
    }

    #[test]
    fn system_paths_are_refused() {
        for p in ["/", "/usr", "/etc/passwd", "/boot", "/bin"] {
            let r = DeletionManifest::build(&[PathBuf::from(p)], None);
            assert!(matches!(r, Err(ManifestError::Protected(_))), "{p}");
        }
        let h = super::super::home().unwrap();
        assert!(matches!(
            DeletionManifest::build(&[h.clone()], None),
            Err(ManifestError::Protected(_))
        ));
        assert!(matches!(
            DeletionManifest::build(&[h.join("..")], None),
            Err(ManifestError::Protected(_))
        ));
    }

    #[test]
    fn trash_writes_a_trashinfo_and_never_clobbers() {
        let d = tmp("del-trash");
        let trash = d.join("Trash");
        std::fs::write(d.join("f.txt"), "1").unwrap();
        let (a, info) = trash_put_in(&d.join("f.txt"), &trash).unwrap();
        assert_eq!(a, trash.join("files/f.txt"));
        let i = std::fs::read_to_string(info).unwrap();
        assert!(i.starts_with("[Trash Info]\nPath=/"), "{i}");
        assert!(i.contains("DeletionDate="));
        std::fs::write(d.join("f.txt"), "2").unwrap();
        let (b, _) = trash_put_in(&d.join("f.txt"), &trash).unwrap();
        assert_eq!(b, trash.join("files/f.txt.2"));
        assert_eq!(std::fs::read_to_string(a).unwrap(), "1");
    }

    #[test]
    fn permanent_never_removes_what_was_not_listed() {
        let d = tmp("del-perm");
        std::fs::create_dir(d.join("dir")).unwrap();
        std::fs::write(d.join("dir/a"), "1").unwrap();
        let m = DeletionManifest::build(&[d.join("dir")], None).unwrap();
        std::fs::write(d.join("dir/new"), "late").unwrap();
        assert!(execute(&m, DeleteMode::Permanent).1.is_some());
        assert!(d.join("dir/new").exists(), "unlisted file survived");
        assert!(!d.join("dir/a").exists());
    }
}
