//! Importing a skills folder Ko drops on the desktop into Lillith's own
//! skills folder (`[mind] skills_dir`, default `<data>/lillith/skills`).
//!
//! - [`find_desktop_skills`] looks on the desktop (`xdg-user-dir DESKTOP`,
//!   `user-dirs.dirs`, `~/Desktop`) for a folder named `skills`, `Skills`,
//!   `lillith-skills` (any case, `-`/`_`/space).
//! - [`import`] **copies** new files in. The desktop original is never
//!   touched. A file that already exists with the same contents is skipped; one
//!   that differs is never overwritten: the incoming copy is kept next to it
//!   as `name (from desktop).md` (once per distinct content) and reported as a
//!   conflict so Lillith can tell Ko.
//! - Every import is appended to `<data>/lillith/skills-import.log`.

use std::path::{Path, PathBuf};

/// Folder names recognised as a skills folder (compared case-insensitively,
/// with `-`, `_` and spaces ignored).
pub const SKILL_FOLDER_NAMES: &[&str] = &["skills", "lillithskills", "lilyskills", "lillithskill"];

/// Biggest single file imported.
pub const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;

/// The desktop folder(s), most specific first.
pub fn desktop_dirs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !p.as_os_str().is_empty() && !v.contains(&p) {
            v.push(p);
        }
    };
    let mut c = std::process::Command::new("xdg-user-dir");
    c.arg("DESKTOP");
    crate::launch_audit::record_command_once("skill_import_desktop", &c);
    if let Ok(out) = c
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
    {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if out.status.success() && s.starts_with('/') {
            push(PathBuf::from(s));
        }
    }
    if let Some(h) = super::home() {
        let cfg = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|x| !x.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| h.join(".config"));
        if let Ok(t) = std::fs::read_to_string(cfg.join("user-dirs.dirs")) {
            for l in t.lines() {
                if let Some(v) = l.trim().strip_prefix("XDG_DESKTOP_DIR=") {
                    let v = v
                        .trim()
                        .trim_matches('"')
                        .replace("$HOME", &h.to_string_lossy());
                    push(PathBuf::from(v));
                }
            }
        }
        push(h.join("Desktop"));
    }
    v
}

fn squash(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '-' | '_' | ' '))
        .collect::<String>()
        .to_lowercase()
}

/// Skills folders found in `dirs` (directories only).
pub fn find_skill_folders(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d) else {
            continue;
        };
        let mut found: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| {
                SKILL_FOLDER_NAMES.contains(&squash(&e.file_name().to_string_lossy()).as_str())
            })
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        found.sort();
        for f in found {
            if !out.contains(&f) {
                out.push(f);
            }
        }
    }
    out
}

/// Skills folders on the desktop.
pub fn find_desktop_skills() -> Vec<PathBuf> {
    find_skill_folders(&desktop_dirs())
}

/// What an import did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub from: PathBuf,
    pub to: PathBuf,
    /// Newly copied (relative paths).
    pub imported: Vec<PathBuf>,
    /// Already there with the same contents.
    pub unchanged: Vec<PathBuf>,
    /// Existed with different contents: `(existing, incoming copy kept as)`.
    pub conflicts: Vec<(PathBuf, PathBuf)>,
    pub skipped: Vec<String>,
}

impl ImportReport {
    /// Anything new happened (worth telling Ko).
    pub fn changed(&self) -> bool {
        !self.imported.is_empty() || !self.conflicts.is_empty()
    }

    /// A short summary for chat.
    pub fn text(&self) -> String {
        let mut s = format!(
            "skills import from {} → {}: {} new, {} unchanged, {} conflict(s)",
            self.from.display(),
            self.to.display(),
            self.imported.len(),
            self.unchanged.len(),
            self.conflicts.len()
        );
        for p in &self.imported {
            s.push_str(&format!("\n  + {}", p.display()));
        }
        for (a, b) in &self.conflicts {
            s.push_str(&format!(
                "\n  ! {} differs from mine — kept yours as {}",
                a.display(),
                b.display()
            ));
        }
        for k in &self.skipped {
            s.push_str(&format!("\n  - skipped {k}"));
        }
        s
    }
}

fn same_contents(a: &Path, b: &Path) -> bool {
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) if x.len() == y.len() => {
            matches!((std::fs::read(a), std::fs::read(b)), (Ok(p), Ok(q)) if p == q)
        }
        _ => false,
    }
}

/// `name (from desktop).ext` siblings of `dest` (any number suffix).
fn incoming_name(dest: &Path) -> PathBuf {
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let (stem, ext) = match name.find('.') {
        Some(0) | None => (name.clone(), String::new()),
        Some(i) => (name[..i].to_string(), name[i..].to_string()),
    };
    dest.with_file_name(format!("{stem} (from desktop){ext}"))
}

/// One file an import would copy (nothing is copied while planning).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedCopy {
    pub src: PathBuf,
    /// Exact destination (a `(from desktop)` name when hers differs).
    pub dest: PathBuf,
    /// Path relative to the skills folder that already exists and differs.
    pub conflict_with: Option<PathBuf>,
    pub size: u64,
}

/// What [`plan_import`] found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportPlan {
    pub copies: Vec<PlannedCopy>,
    pub unchanged: Vec<PathBuf>,
    pub skipped: Vec<String>,
}

/// Works out exactly what importing `from` into `to` would copy, without
/// touching either folder.
pub fn plan_import(from: &Path, to: &Path) -> std::io::Result<ImportPlan> {
    let mut out = ImportPlan::default();
    if !from.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} is not a folder", from.display()),
        ));
    }
    if to.starts_with(from) || from.starts_with(to) {
        return Err(std::io::Error::other("source and skills folder overlap"));
    }
    let mut stack = vec![PathBuf::new()];
    let mut claimed: Vec<PathBuf> = Vec::new();
    while let Some(rel) = stack.pop() {
        let dir = from.join(&rel);
        let mut kids: Vec<_> = std::fs::read_dir(&dir)?.flatten().collect();
        kids.sort_by_key(|e| e.file_name());
        for e in kids {
            let name = e.file_name();
            let r = rel.join(&name);
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let meta = std::fs::symlink_metadata(e.path())?;
            if meta.file_type().is_symlink() {
                out.skipped.push(format!("{} (symlink)", r.display()));
                continue;
            }
            if meta.is_dir() {
                stack.push(r);
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            if meta.len() > MAX_FILE_BYTES {
                out.skipped.push(format!("{} (too big)", r.display()));
                continue;
            }
            let src = e.path();
            let dest = to.join(&r);
            if std::fs::symlink_metadata(&dest).is_err() && !claimed.contains(&dest) {
                claimed.push(dest.clone());
                out.copies.push(PlannedCopy {
                    src,
                    dest,
                    conflict_with: None,
                    size: meta.len(),
                });
            } else if same_contents(&src, &dest) {
                out.unchanged.push(r);
            } else {
                let base = incoming_name(&dest);
                let mut cand = base.clone();
                let mut n = 2;
                let mut already = false;
                while std::fs::symlink_metadata(&cand).is_ok() || claimed.contains(&cand) {
                    if same_contents(&src, &cand) {
                        already = true;
                        break;
                    }
                    let s = base.to_string_lossy().to_string();
                    let (a, b) = match s.rfind(" (from desktop)") {
                        Some(i) => (s[..i].to_string(), s[i + 15..].to_string()),
                        None => (s.clone(), String::new()),
                    };
                    cand = PathBuf::from(format!("{a} (from desktop {n}){b}"));
                    n += 1;
                }
                if already {
                    out.unchanged
                        .push(cand.strip_prefix(to).unwrap_or(&cand).to_path_buf());
                } else {
                    claimed.push(cand.clone());
                    out.copies.push(PlannedCopy {
                        src,
                        dest: cand,
                        conflict_with: Some(r),
                        size: meta.len(),
                    });
                }
            }
        }
    }
    Ok(out)
}

/// Copies the files under `from` into `to` (see the module docs). Hidden files
/// and folders (`.git`) and symlinks are skipped. `log` gets one line per
/// import run.
pub fn import(from: &Path, to: &Path, log: Option<&Path>) -> std::io::Result<ImportReport> {
    let mut rep = ImportReport {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
        ..Default::default()
    };
    if !from.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} is not a folder", from.display()),
        ));
    }
    if to.starts_with(from) || from.starts_with(to) {
        return Err(std::io::Error::other("source and skills folder overlap"));
    }
    std::fs::create_dir_all(to)?;
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let dir = from.join(&rel);
        let mut kids: Vec<_> = std::fs::read_dir(&dir)?.flatten().collect();
        kids.sort_by_key(|e| e.file_name());
        for e in kids {
            let name = e.file_name();
            let r = rel.join(&name);
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let meta = std::fs::symlink_metadata(e.path())?;
            if meta.file_type().is_symlink() {
                rep.skipped.push(format!("{} (symlink)", r.display()));
                continue;
            }
            if meta.is_dir() {
                stack.push(r);
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            if meta.len() > MAX_FILE_BYTES {
                rep.skipped.push(format!("{} (too big)", r.display()));
                continue;
            }
            let src = e.path();
            let dest = to.join(&r);
            if let Some(p) = dest.parent() {
                std::fs::create_dir_all(p)?;
            }
            if std::fs::symlink_metadata(&dest).is_err() {
                copy_new(&src, &dest, &meta)?;
                rep.imported.push(r);
            } else if same_contents(&src, &dest) {
                rep.unchanged.push(r);
            } else {
                // Keep both. Reuse an earlier incoming copy with the same bytes.
                let base = incoming_name(&dest);
                let mut cand = base.clone();
                let mut n = 2;
                let mut already = false;
                while std::fs::symlink_metadata(&cand).is_ok() {
                    if same_contents(&src, &cand) {
                        already = true;
                        break;
                    }
                    let s = base.to_string_lossy().to_string();
                    let (a, b) = match s.rfind(" (from desktop)") {
                        Some(i) => (s[..i].to_string(), s[i + 15..].to_string()),
                        None => (s.clone(), String::new()),
                    };
                    cand = PathBuf::from(format!("{a} (from desktop {n}){b}"));
                    n += 1;
                }
                let kept = cand.strip_prefix(to).unwrap_or(&cand).to_path_buf();
                if already {
                    rep.unchanged.push(kept);
                } else {
                    copy_new(&src, &cand, &meta)?;
                    rep.conflicts.push((r, kept));
                }
            }
        }
    }
    if let Some(l) = log {
        let ts = crate::soul_contract::format_utc(std::time::SystemTime::now());
        let mut line = format!(
            "{ts}\t{}\t{}\tnew={} unchanged={} conflicts={}",
            from.display(),
            to.display(),
            rep.imported.len(),
            rep.unchanged.len(),
            rep.conflicts.len()
        );
        for p in &rep.imported {
            line.push_str(&format!("\n{ts}\t+\t{}", p.display()));
        }
        for (a, b) in &rep.conflicts {
            line.push_str(&format!(
                "\n{ts}\t!\t{}\tkept as {}",
                a.display(),
                b.display()
            ));
        }
        let _ = super::append_line(l, &line);
    }
    Ok(rep)
}

pub(crate) fn copy_new(src: &Path, dest: &Path, meta: &std::fs::Metadata) -> std::io::Result<()> {
    let mut s = std::fs::File::open(src)?;
    let mut d = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)?;
    std::io::copy(&mut s, &mut d)?;
    let _ = d.set_permissions(meta.permissions());
    Ok(())
}

/// Imports every desktop skills folder into `to`. Returns one report per
/// folder found (empty when there is none).
pub fn import_from_desktop(to: &Path, log: Option<&Path>) -> Vec<std::io::Result<ImportReport>> {
    find_desktop_skills()
        .into_iter()
        .map(|f| import(&f, to, log))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::testutil::tmp;
    use super::*;

    #[test]
    fn finds_skills_folders_by_name() {
        let d = tmp("skill-find");
        std::fs::create_dir_all(d.join("Skills")).unwrap();
        std::fs::create_dir_all(d.join("lillith-skills")).unwrap();
        std::fs::create_dir_all(d.join("other")).unwrap();
        std::fs::write(d.join("skills.txt"), "").unwrap();
        let f = find_skill_folders(&[d.clone()]);
        assert_eq!(f, vec![d.join("Skills"), d.join("lillith-skills")]);
    }

    #[test]
    fn import_copies_new_keeps_both_on_conflict_and_never_touches_the_source() {
        let d = tmp("skill-import");
        let src = d.join("Desktop/skills");
        let dst = d.join("data/skills");
        std::fs::create_dir_all(src.join("scripts")).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("new.md"), "# new").unwrap();
        std::fs::write(src.join("same.md"), "same").unwrap();
        std::fs::write(src.join("mine.md"), "desktop version").unwrap();
        std::fs::write(src.join("scripts/tidy.sh"), "#!/bin/sh\necho tidy").unwrap();
        std::fs::write(src.join(".hidden"), "x").unwrap();
        std::fs::write(dst.join("same.md"), "same").unwrap();
        std::fs::write(dst.join("mine.md"), "her own edits").unwrap();
        let log = d.join("data/skills-import.log");

        let r = import(&src, &dst, Some(&log)).unwrap();
        assert_eq!(
            r.imported,
            vec![PathBuf::from("new.md"), PathBuf::from("scripts/tidy.sh")]
        );
        assert_eq!(r.unchanged, vec![PathBuf::from("same.md")]);
        assert_eq!(
            r.conflicts,
            vec![(
                PathBuf::from("mine.md"),
                PathBuf::from("mine (from desktop).md")
            )]
        );
        assert_eq!(
            std::fs::read_to_string(dst.join("mine.md")).unwrap(),
            "her own edits"
        );
        assert_eq!(
            std::fs::read_to_string(dst.join("mine (from desktop).md")).unwrap(),
            "desktop version"
        );
        assert!(!dst.join(".hidden").exists());
        // Source untouched.
        assert_eq!(
            std::fs::read_to_string(src.join("mine.md")).unwrap(),
            "desktop version"
        );
        assert!(src.join("new.md").exists());
        assert!(std::fs::read_to_string(&log).unwrap().contains("new=2"));

        // Running again changes nothing (no duplicate conflict copies).
        let r2 = import(&src, &dst, Some(&log)).unwrap();
        assert!(!r2.changed(), "{}", r2.text());

        // A newer desktop edit is kept as a second copy, nothing overwritten.
        std::fs::write(src.join("mine.md"), "desktop v2").unwrap();
        let r3 = import(&src, &dst, None).unwrap();
        assert_eq!(
            r3.conflicts,
            vec![(
                PathBuf::from("mine.md"),
                PathBuf::from("mine (from desktop 2).md")
            )]
        );
        assert_eq!(
            std::fs::read_to_string(dst.join("mine.md")).unwrap(),
            "her own edits"
        );
        assert_eq!(
            std::fs::read_to_string(dst.join("mine (from desktop).md")).unwrap(),
            "desktop version"
        );
    }

    #[cfg(unix)]
    #[test]
    fn import_keeps_script_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("skill-perm");
        std::fs::create_dir_all(d.join("s")).unwrap();
        std::fs::write(d.join("s/run.sh"), "#!/bin/sh").unwrap();
        std::fs::set_permissions(d.join("s/run.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        import(&d.join("s"), &d.join("t"), None).unwrap();
        let m = std::fs::metadata(d.join("t/run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(m & 0o111, 0o111);
    }

    #[test]
    fn overlapping_folders_are_refused() {
        let d = tmp("skill-overlap");
        std::fs::create_dir_all(d.join("a/b")).unwrap();
        assert!(import(&d.join("a"), &d.join("a/b"), None).is_err());
    }
}
