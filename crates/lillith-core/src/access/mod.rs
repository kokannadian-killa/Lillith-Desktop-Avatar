//! Lillith's default-on access to the user's files and programs
//! (`[mind.access]`): the IO side behind the tool API.
//!
//! - [`organize`]: non-destructive move / rename / copy / mkdir with an undo
//!   journal (`<data>/lillith/organize-journal/`) and sort suggestions.
//! - [`deletion`]: the only way anything gets deleted. It lists every file and
//!   folder, then needs a signed Soul Contract
//!   ([`crate::soul_contract::SoulContract::sign_deletion`]) for exactly that
//!   set. Trash by default, permanent only when the signer picks it. System
//!   paths are refused even when signed.
//! - [`programs`]: installed apps (`.desktop` entries) and `PATH` binaries,
//!   install checks, and launching an app with files.
//! - [`commands`]: classifies a CLI command (read-only, needs a Yes, or file
//!   removal, which is never a command) and runs it with captured output, a
//!   timeout and a command log.
//!
//! Everything here is local. Nothing opens a network connection.

pub mod commands;
pub mod deletion;
pub mod find;
pub mod organize;
pub mod programs;
pub mod skill_import;

use std::path::{Component, Path, PathBuf};

/// Folders that are never touched by organize or delete, nor anything inside
/// them (even with a signed contract).
pub const SYSTEM_TREES: &[&str] = &[
    "/bin",
    "/boot",
    "/dev",
    "/efi",
    "/etc",
    "/lib",
    "/lib32",
    "/lib64",
    "/libx32",
    "/opt",
    "/proc",
    "/root",
    "/sbin",
    "/srv",
    "/sys",
    "/usr",
    "/var",
    "/snap",
    "/lost+found",
];

/// Folders that may hold the user's things but are never removed or moved
/// themselves: the filesystem root and the mount-point parents.
pub const KEEP_ITSELF: &[&str] = &["/", "/home", "/mnt", "/media", "/run", "/run/media", "/tmp"];

/// Per-user folders (relative to `$HOME`) that are never deleted or moved
/// themselves; the secrets ones are protected with everything inside.
const HOME_KEEP_ITSELF: &[&str] = &[
    ".config",
    ".local",
    ".local/share",
    ".local/bin",
    ".cache",
    "Desktop",
    "Documents",
    "Downloads",
    "Music",
    "Pictures",
    "Videos",
    "Templates",
    "Public",
];
/// Secrets folders under `$HOME`: never deleted or moved, with their contents.
pub const HOME_SECRET_TREES: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".config/gcloud",
    ".password-store",
    ".docker",
    ".kube",
    ".local/share/keyrings",
    ".mozilla",
    ".thunderbird",
];

/// The secrets folders under `$HOME` as absolute paths (commands touching
/// them need approval; the tool API denies them).
pub fn secret_roots() -> Vec<PathBuf> {
    match home() {
        Some(h) => HOME_SECRET_TREES.iter().map(|s| h.join(s)).collect(),
        None => Vec::new(),
    }
}

/// The user's home (`$HOME`).
pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// Lexically normalises `path`: `~` expanded, relative paths anchored at
/// `$HOME`, `.`/`..` resolved without touching the disk.
pub fn normalize(path: &Path) -> PathBuf {
    let expanded = match path.to_str() {
        Some(s) if s == "~" || s.starts_with("~/") => {
            crate::config::expand_tilde(s).unwrap_or_else(|| path.to_path_buf())
        }
        _ => path.to_path_buf(),
    };
    let anchored = if expanded.is_absolute() {
        expanded
    } else if let Some(h) = home() {
        h.join(expanded)
    } else {
        expanded
    };
    let mut out = PathBuf::new();
    for c in anchored.components() {
        match c {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(n) => out.push(n),
        }
    }
    out
}

/// `path` with its parent resolved through symlinks (the last component is
/// kept as is, so a symlink is judged as the link, not its target). This is
/// what protection checks run on: `~/link-to-usr/bin` is really `/usr/bin`.
pub fn real_path(path: &Path) -> PathBuf {
    let p = normalize(path);
    match (p.parent(), p.file_name()) {
        (Some(parent), Some(name)) => match std::fs::canonicalize(parent) {
            Ok(rp) => rp.join(name),
            Err(_) => p,
        },
        _ => p,
    }
}

/// Why a path may not be organized or deleted, or `None` if it may.
/// `data_dir` is Lillith's own data folder (audit / journals live there).
pub fn protected_reason(path: &Path, data_dir: Option<&Path>) -> Option<String> {
    let p = real_path(path);
    let lex = normalize(path);
    for cand in [&p, &lex] {
        for t in SYSTEM_TREES {
            if cand.starts_with(t) {
                return Some(format!("{} is a system folder ({t})", cand.display()));
            }
        }
        for k in KEEP_ITSELF {
            if cand == Path::new(k) {
                return Some(format!("{} is a top-level folder", cand.display()));
            }
        }
        // /run is system except the removable-media tree.
        if cand.starts_with("/run") && !cand.starts_with("/run/media") {
            return Some(format!("{} is a runtime system folder", cand.display()));
        }
        // /run/media/<user> itself.
        if cand.starts_with("/run/media") && cand.components().count() <= 4 {
            return Some(format!("{} is a mount folder", cand.display()));
        }
        // A mount point itself (/mnt/x, /media/x).
        for m in ["/mnt", "/media"] {
            if cand.parent() == Some(Path::new(m)) {
                return Some(format!("{} is a mount point", cand.display()));
            }
        }
        if let Some(h) = home() {
            if *cand == h {
                return Some("your home folder itself is never removed".into());
            }
            if cand.parent() == h.parent() && cand.starts_with("/home") {
                return Some(format!("{} is a home folder", cand.display()));
            }
            for k in HOME_KEEP_ITSELF {
                if *cand == h.join(k) {
                    return Some(format!(
                        "{} is a standard folder of your home",
                        cand.display()
                    ));
                }
            }
            for s in HOME_SECRET_TREES {
                if cand.starts_with(h.join(s)) {
                    return Some(format!("{} holds secrets/keys", cand.display()));
                }
            }
            // Other users' homes.
            if cand.starts_with("/home") && !cand.starts_with(&h) {
                return Some(format!("{} belongs to another user", cand.display()));
            }
        }
        if let Some(d) = data_dir {
            if cand.starts_with(d) || d.starts_with(cand.as_path()) {
                return Some(format!(
                    "{} holds Lillith's own logs and journals",
                    cand.display()
                ));
            }
        }
    }
    None
}

/// Whether `path` is inside one of `roots` (lexically and after resolving the
/// parent's symlinks).
pub fn within_roots(path: &Path, roots: &[PathBuf]) -> bool {
    let lex = normalize(path);
    let real = real_path(path);
    roots.iter().any(|r| lex.starts_with(r))
        && roots.iter().any(|r| {
            let rr = std::fs::canonicalize(r).unwrap_or_else(|_| r.clone());
            real.starts_with(&rr) || real.starts_with(r)
        })
}

/// Human-readable byte size (`1.5 MiB`).
pub fn human_size(bytes: u64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

/// Appends one line to `path`, creating its folder.
pub(crate) fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(line.as_bytes())?;
    if !line.ends_with('\n') {
        f.write_all(b"\n")?;
    }
    Ok(())
}

/// Seconds since the Unix epoch, now.
pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::PathBuf;

    pub fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "lillith-access-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_paths_are_protected() {
        for p in [
            "/",
            "/usr",
            "/usr/bin/ls",
            "/etc",
            "/etc/fstab",
            "/boot",
            "/bin",
            "/home",
            "/var/log",
            "/mnt",
            "/media",
            "/proc/1",
            "/sys",
            "/run/user/1000",
            "/tmp",
        ] {
            assert!(protected_reason(Path::new(p), None).is_some(), "{p}");
        }
        let h = home().unwrap();
        assert!(protected_reason(&h, None).is_some(), "home itself");
        assert!(protected_reason(&h.join(".ssh/id_ed25519"), None).is_some());
        assert!(protected_reason(&h.join("Downloads"), None).is_some());
        assert!(protected_reason(&h.join("Downloads/old.iso"), None).is_none());
        assert!(
            protected_reason(Path::new("/mnt/usb"), None).is_some(),
            "mount point"
        );
        assert!(protected_reason(Path::new("/mnt/usb/old"), None).is_none());
        assert!(protected_reason(Path::new("/home/someoneelse/x"), None).is_some());
        let data = h.join(".local/share/lillith");
        assert!(protected_reason(&data.join("audit.log"), Some(&data)).is_some());
    }

    #[test]
    fn dotdot_cannot_escape_into_system() {
        let h = home().unwrap();
        let sneaky = h.join("../../usr/lib");
        assert!(protected_reason(&sneaky, None).is_some());
    }

    #[test]
    fn sizes_read_well() {
        assert_eq!(human_size(12), "12 B");
        assert_eq!(human_size(1536), "1.5 KiB");
    }
}
