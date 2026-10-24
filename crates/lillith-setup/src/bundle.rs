//! The all-in-one download's bundled llama.cpp and model folder.
//!
//! `tools/bundle/make-bundle.sh` (build machine only) puts a pinned, prebuilt
//! `llama-server` in `bundle/llama/<platform>/` next to Lillith and an empty
//! `bundle/models/` for the user's own GGUF. This module only computes where
//! those live; nothing here downloads anything.
//!
//! Layouts covered (each `<root>` is tried in order):
//! * the exe's own folder (a `llama-server` dropped right next to Lillith),
//! * `<exe dir>/bundle/...` (Windows install / portable folder),
//! * `<exe dir>/../bundle/...` (Linux portable zip: `bin/lillith`),
//! * `$XDG_DATA_HOME/lillith/llama/<platform>` (where `linux/install.sh`
//!   copies the bundled server), and on Windows
//!   `%LOCALAPPDATA%\Programs\Lillith\bundle\...` (the installer's folder).

use crate::paths;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// `bundle/llama/<platform>` for this build, or `None` where no prebuilt
/// server is bundled (the regular detection still runs).
pub const LLAMA_PLATFORM: Option<&str> = if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
    Some("linux-x64")
} else if cfg!(all(windows, target_arch = "x86_64")) {
    Some("windows-x64")
} else {
    None
};

/// Folders that may hold a `bundle/` folder: the exe's folder, its parent,
/// then the installers' locations.
pub fn install_roots() -> Vec<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf));
    // Windows: the installer's default folder (also covers lillith-settings
    // started from somewhere else).
    let extra: Vec<PathBuf> = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
            .filter(|v| !v.is_empty())
            .map(|l| PathBuf::from(l).join("Programs").join("Lillith"))
            .into_iter()
            .collect()
    } else {
        Vec::new()
    };
    install_roots_from(exe_dir.as_deref(), paths::data_home().as_deref(), &extra)
}

/// [`install_roots`] from explicit values (testable).
pub fn install_roots_from(
    exe_dir: Option<&Path>,
    data_home: Option<&Path>,
    extra: &[PathBuf],
) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(d) = exe_dir {
        v.push(d.to_path_buf());
        if let Some(p) = d.parent().filter(|p| !p.as_os_str().is_empty()) {
            v.push(p.to_path_buf());
        }
    }
    if let Some(d) = data_home {
        v.push(d.join("lillith"));
    }
    v.extend(extra.iter().cloned());
    dedup(v)
}

/// Folders a bundled `llama-server` may be in, best first: next to the exe
/// (`roots[0]`), then `<root>/bundle/llama/<platform>` and
/// `<root>/llama/<platform>` for every root. CPU build only; the optional
/// `linux-x64-vulkan` build is picked by hand (Settings > Brain).
pub fn llama_dirs_from(roots: &[PathBuf], platform: Option<&str>) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(first) = roots.first() {
        v.push(first.clone());
    }
    if let Some(p) = platform {
        for r in roots {
            v.push(r.join("bundle").join("llama").join(p));
            v.push(r.join("llama").join(p));
        }
    }
    dedup(v)
}

/// [`llama_dirs_from`] for this install.
pub fn llama_dirs() -> Vec<PathBuf> {
    llama_dirs_from(&install_roots(), LLAMA_PLATFORM)
}

/// The bundled `llama-server`, if this install has one.
pub fn find_llama_server() -> Option<PathBuf> {
    crate::detect::find_binary("llama-server", &llama_dirs())
}

/// Existing `<root>/bundle/models` folders (the user's own GGUFs; the
/// bundle ships none).
pub fn model_dirs_from(roots: &[PathBuf]) -> Vec<PathBuf> {
    dedup(
        roots
            .iter()
            .map(|r| r.join("bundle").join("models"))
            .filter(|d| d.is_dir())
            .collect(),
    )
}

/// [`model_dirs_from`] for this install.
pub fn model_dirs() -> Vec<PathBuf> {
    model_dirs_from(&install_roots())
}

fn dedup(v: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    v.into_iter().filter(|p| seen.insert(p.clone())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lillith-bundle-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn roots_cover_exe_dir_parent_and_installed_copy() {
        let r = install_roots_from(
            Some(Path::new("/pkg/bin")),
            Some(Path::new("/home/k/.local/share")),
            &[PathBuf::from("/pkg")],
        );
        assert_eq!(
            r,
            vec![
                PathBuf::from("/pkg/bin"),
                PathBuf::from("/pkg"),
                PathBuf::from("/home/k/.local/share/lillith"),
            ]
        );
    }

    #[test]
    fn llama_dirs_order() {
        let roots = vec![PathBuf::from("/pkg/bin"), PathBuf::from("/pkg")];
        let d = llama_dirs_from(&roots, Some("linux-x64"));
        assert_eq!(
            d,
            vec![
                PathBuf::from("/pkg/bin"),
                PathBuf::from("/pkg/bin/bundle/llama/linux-x64"),
                PathBuf::from("/pkg/bin/llama/linux-x64"),
                PathBuf::from("/pkg/bundle/llama/linux-x64"),
                PathBuf::from("/pkg/llama/linux-x64"),
            ]
        );
        // No prebuilt for this platform: only "next to the exe".
        assert_eq!(
            llama_dirs_from(&roots, None),
            vec![PathBuf::from("/pkg/bin")]
        );
        assert!(!d.iter().any(|p| p.to_string_lossy().contains("vulkan")));
    }

    #[test]
    fn this_platform_is_bundled_on_x86_64_linux_and_windows() {
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            assert_eq!(LLAMA_PLATFORM, Some("linux-x64"));
        }
        if cfg!(all(windows, target_arch = "x86_64")) {
            assert_eq!(LLAMA_PLATFORM, Some("windows-x64"));
        }
    }

    #[test]
    fn model_dirs_only_existing_bundle_models() {
        let root = tmp("models");
        let pkg = root.join("Lillith");
        std::fs::create_dir_all(pkg.join("bin")).unwrap();
        let roots = install_roots_from(Some(&pkg.join("bin")), None, &[]);
        assert!(model_dirs_from(&roots).is_empty());
        std::fs::create_dir_all(pkg.join("bundle/models")).unwrap();
        assert_eq!(model_dirs_from(&roots), vec![pkg.join("bundle/models")]);
        std::fs::remove_dir_all(&root).ok();
    }
}
