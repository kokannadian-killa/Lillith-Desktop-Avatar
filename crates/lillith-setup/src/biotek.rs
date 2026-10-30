//! Biotek Engine data home (`~/.biotek`), with a read fallback to and a
//! one-time, copy-only migration from the legacy `~/.aether` home.
//!
//! See `docs/BIOTEK_TERMINOLOGY_MAP.md`. Std only. Nothing here deletes,
//! moves or overwrites anything under the legacy home: migration copies
//! files that are missing in `~/.biotek` and leaves `~/.aether` intact.
//!
//! Resolution order for the data home ([`biotek_home`]):
//! 1. `$BIOTEK_HOME` (absolute, non-empty)
//! 2. `$AETHER_HOME` (deprecated alias, absolute, non-empty)
//! 3. `~/.biotek`
//!
//! The layout is the Phase 0 spec in `docs/PRESERVATION_MANIFEST.md` §12.
//! It never overrides `~/.config/lillith/config.toml` or the
//! `~/.local/share/lillith` memory store.

use std::io;
use std::path::{Path, PathBuf};

/// Primary env var for the data home.
pub const BIOTEK_HOME_ENV: &str = "BIOTEK_HOME";
/// Deprecated alias of [`BIOTEK_HOME_ENV`]; still honoured.
pub const LEGACY_AETHER_HOME_ENV: &str = "AETHER_HOME";
/// Primary data-home directory name under `$HOME`.
pub const BIOTEK_DIR_NAME: &str = ".biotek";
/// Legacy data-home directory name under `$HOME` (read fallback only).
pub const LEGACY_AETHER_DIR_NAME: &str = ".aether";
/// Marker written into the new home after a migration pass.
pub const MIGRATION_MARKER: &str = ".migrated-from-aether";

/// Sub-directories of the data home (relative paths).
pub const LAYOUT: &[&str] = &[
    "config",
    "logs",
    "memory",
    "reports",
    "bots/registry",
    "bots/swarm",
    "consent",
    "quantumsec",
    "teams/red",
    "teams/blue",
    "teams/purple",
];

fn env_abs(k: &str) -> Option<PathBuf> {
    std::env::var_os(k)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// The data-home rule from explicit values (testable without touching env).
pub fn biotek_home_from(
    biotek_env: Option<PathBuf>,
    aether_env: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    biotek_env
        .or(aether_env)
        .or_else(|| home.map(|h| h.join(BIOTEK_DIR_NAME)))
}

/// `$BIOTEK_HOME`, else `$AETHER_HOME` (deprecated), else `~/.biotek`.
pub fn biotek_home() -> Option<PathBuf> {
    biotek_home_from(
        env_abs(BIOTEK_HOME_ENV),
        env_abs(LEGACY_AETHER_HOME_ENV),
        crate::paths::home_dir(),
    )
}

/// `~/.aether` (legacy home; read fallback and migration source only).
pub fn legacy_aether_home() -> Option<PathBuf> {
    crate::paths::home_dir().map(|h| h.join(LEGACY_AETHER_DIR_NAME))
}

/// Deprecated alias of [`biotek_home`] for any caller written against the
/// original `~/.aether` mandate. Returns the Biotek home, not `~/.aether`.
#[deprecated(note = "use biotek::biotek_home (data home renamed to ~/.biotek)")]
pub fn aether_home() -> Option<PathBuf> {
    biotek_home()
}

/// Path to read `rel` from: `<new>/rel` if it exists, else `<legacy>/rel`
/// if that exists, else `<new>/rel` (where new files are written).
pub fn resolve_read_in(new: &Path, legacy: Option<&Path>, rel: &str) -> PathBuf {
    let primary = new.join(rel);
    if primary.exists() {
        return primary;
    }
    match legacy.map(|l| l.join(rel)) {
        Some(old) if old.exists() => old,
        _ => primary,
    }
}

/// [`resolve_read_in`] for the real homes.
pub fn resolve_read(rel: &str) -> Option<PathBuf> {
    let new = biotek_home()?;
    Some(resolve_read_in(&new, legacy_aether_home().as_deref(), rel))
}

/// Create the [`LAYOUT`] directories under `home` (idempotent).
pub fn ensure_layout_in(home: &Path) -> io::Result<()> {
    for d in LAYOUT {
        std::fs::create_dir_all(home.join(d))?;
    }
    Ok(())
}

/// What a migration pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    /// Files copied from the legacy home (relative paths).
    pub copied: Vec<PathBuf>,
    /// Files skipped because they already existed in the new home.
    pub skipped: Vec<PathBuf>,
}

/// One-time, non-destructive migration: copy every file under `legacy` that
/// does not yet exist under `new`. Never overwrites, never deletes, never
/// follows symlinks out of the tree. Writes [`MIGRATION_MARKER`] into `new`
/// and is a no-op once that marker exists or `legacy` is absent.
pub fn migrate_in(legacy: &Path, new: &Path) -> io::Result<MigrationReport> {
    let mut report = MigrationReport::default();
    if !legacy.is_dir() || new.join(MIGRATION_MARKER).exists() {
        return Ok(report);
    }
    std::fs::create_dir_all(new)?;
    copy_missing(legacy, new, Path::new(""), &mut report)?;
    std::fs::write(
        new.join(MIGRATION_MARKER),
        format!(
            "copied from {} ({} copied, {} skipped); legacy home left in place\n",
            legacy.display(),
            report.copied.len(),
            report.skipped.len()
        ),
    )?;
    Ok(report)
}

fn copy_missing(
    src_root: &Path,
    dst_root: &Path,
    rel: &Path,
    report: &mut MigrationReport,
) -> io::Result<()> {
    for entry in std::fs::read_dir(src_root.join(rel))? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let rel_child = rel.join(entry.file_name());
        let dst = dst_root.join(&rel_child);
        if ft.is_dir() {
            std::fs::create_dir_all(&dst)?;
            copy_missing(src_root, dst_root, &rel_child, report)?;
        } else if ft.is_file() {
            if dst.exists() {
                report.skipped.push(rel_child);
            } else {
                std::fs::copy(entry.path(), &dst)?;
                report.copied.push(rel_child);
            }
        }
        // symlinks and other types are left alone
    }
    Ok(())
}

/// [`migrate_in`] for the real homes, then [`ensure_layout_in`].
pub fn migrate_from_aether() -> io::Result<MigrationReport> {
    let new = biotek_home()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no home directory"))?;
    let report = match legacy_aether_home() {
        Some(old) if old != new => migrate_in(&old, &new)?,
        _ => MigrationReport::default(),
    };
    ensure_layout_in(&new)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "lillith-biotek-test-{}-{}",
            name,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn home_order() {
        let h = Some(PathBuf::from("/home/k"));
        assert_eq!(
            biotek_home_from(None, None, h.clone()),
            Some(PathBuf::from("/home/k/.biotek"))
        );
        assert_eq!(
            biotek_home_from(None, Some("/a".into()), h.clone()),
            Some(PathBuf::from("/a"))
        );
        assert_eq!(
            biotek_home_from(Some("/b".into()), Some("/a".into()), h),
            Some(PathBuf::from("/b"))
        );
        assert_eq!(biotek_home_from(None, None, None), None);
    }

    #[test]
    fn read_fallback() {
        let root = tmp("read");
        let (new, old) = (root.join("new"), root.join("old"));
        std::fs::create_dir_all(old.join("config")).unwrap();
        std::fs::write(old.join("config/a.toml"), "x").unwrap();
        assert_eq!(
            resolve_read_in(&new, Some(&old), "config/a.toml"),
            old.join("config/a.toml")
        );
        std::fs::create_dir_all(new.join("config")).unwrap();
        std::fs::write(new.join("config/a.toml"), "y").unwrap();
        assert_eq!(
            resolve_read_in(&new, Some(&old), "config/a.toml"),
            new.join("config/a.toml")
        );
        assert_eq!(resolve_read_in(&new, Some(&old), "nope"), new.join("nope"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn migrate_copies_never_overwrites_or_deletes() {
        let root = tmp("mig");
        let (new, old) = (root.join("new"), root.join("old"));
        std::fs::create_dir_all(old.join("consent")).unwrap();
        std::fs::write(old.join("consent/log.jsonl"), "old").unwrap();
        std::fs::write(old.join("README.md"), "old readme").unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("README.md"), "new readme").unwrap();

        let r = migrate_in(&old, &new).unwrap();
        assert_eq!(r.copied, vec![PathBuf::from("consent/log.jsonl")]);
        assert_eq!(r.skipped, vec![PathBuf::from("README.md")]);
        assert_eq!(
            std::fs::read_to_string(new.join("README.md")).unwrap(),
            "new readme"
        );
        assert_eq!(
            std::fs::read_to_string(new.join("consent/log.jsonl")).unwrap(),
            "old"
        );
        assert!(old.join("consent/log.jsonl").exists(), "legacy file kept");
        assert!(new.join(MIGRATION_MARKER).exists());

        // second pass is a no-op
        std::fs::write(old.join("late.txt"), "z").unwrap();
        assert_eq!(migrate_in(&old, &new).unwrap(), MigrationReport::default());
        assert!(!new.join("late.txt").exists());

        ensure_layout_in(&new).unwrap();
        for d in LAYOUT {
            assert!(new.join(d).is_dir());
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
