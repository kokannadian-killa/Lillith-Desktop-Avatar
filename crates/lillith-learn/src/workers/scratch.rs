//! A worker's scratch dir: tmpfs (`/dev/shm`) where there is one, mode
//! 0700, unique per worker, and wiped when the guard drops — on success,
//! kill or crash (the supervisor holds the guard, never the worker). The
//! name carries the owning process id, so a dir left behind by a parent
//! that died (SIGKILL, power loss with a persistent `/dev/shm`) is swept at
//! the next start ([`sweep_stale`]).

use std::path::{Path, PathBuf};

/// Every scratch dir name starts with this.
const PREFIX: &str = "lillith-worker-";

/// A scratch dir named the old way (no pid) older than this is stale.
const LEGACY_STALE: std::time::Duration = std::time::Duration::from_secs(3600);

/// Removes stale scratch dirs: ours (same uid, real dirs, never symlinks)
/// whose owning process is gone, plus old-style (pre-pid) ones over an hour
/// old. Never touches a live process's dirs. Returns how many went.
pub fn sweep_stale() -> usize {
    let root = tmpfs_root().unwrap_or_else(std::env::temp_dir);
    sweep_stale_in(&root)
}

pub(crate) fn sweep_stale_in(root: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(root) else {
        return 0;
    };
    let me = std::process::id();
    let mut n = 0;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix(PREFIX) else {
            continue;
        };
        let Ok(md) = std::fs::symlink_metadata(e.path()) else {
            continue;
        };
        if !md.is_dir() || !owned_by_me(&md) {
            continue;
        }
        let stale = match rest
            .strip_prefix('p')
            .and_then(|r| r.split('-').next())
            .and_then(|p| p.parse::<u32>().ok())
        {
            Some(pid) => pid != me && !process_alive(pid),
            None => md
                .modified()
                .ok()
                .and_then(|m| m.elapsed().ok())
                .is_some_and(|age| age >= LEGACY_STALE),
        };
        if stale && std::fs::remove_dir_all(e.path()).is_ok() {
            n += 1;
        }
    }
    n
}

#[cfg(unix)]
fn owned_by_me(md: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: plain syscall, no arguments.
    md.uid() == unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
fn owned_by_me(_md: &std::fs::Metadata) -> bool {
    true
}

#[cfg(target_os = "linux")]
fn process_alive(pid: u32) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

#[cfg(not(target_os = "linux"))]
fn process_alive(_pid: u32) -> bool {
    // No way to tell: keep it.
    true
}

/// The tmpfs root, if this machine has one we can write.
pub(crate) fn tmpfs_root() -> Option<PathBuf> {
    let p = Path::new("/dev/shm");
    (cfg!(target_os = "linux") && p.is_dir()).then(|| p.to_path_buf())
}

/// A scratch dir; removed with everything in it on drop.
#[derive(Debug)]
pub struct Scratch {
    path: PathBuf,
    tmpfs: bool,
}

impl Scratch {
    /// A fresh dir for `job`.
    pub fn new(job: &str) -> std::io::Result<Self> {
        let (root, tmpfs) = match tmpfs_root() {
            Some(r) => (r, true),
            None => (std::env::temp_dir(), false),
        };
        Self::new_in(&root, job, tmpfs)
    }

    pub(crate) fn new_in(root: &Path, job: &str, tmpfs: bool) -> std::io::Result<Self> {
        let safe: String = job
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .take(48)
            .collect();
        let path = root.join(format!(
            "{PREFIX}p{}-{safe}-{:016x}",
            std::process::id(),
            super::random_u64()
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(0o700).create(&path)?;
        }
        #[cfg(not(unix))]
        std::fs::DirBuilder::new().create(&path)?;
        Ok(Self { path, tmpfs })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether it lives in RAM (tmpfs).
    pub fn is_tmpfs(&self) -> bool {
        self.tmpfs
    }

    /// Removes the dir now (also done on drop).
    pub fn wipe(&self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        self.wipe();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_is_tmpfs_private_and_wiped_on_drop() {
        let s = Scratch::new("job-1").unwrap();
        let p = s.path().to_path_buf();
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt;
            assert!(s.is_tmpfs() && p.starts_with("/dev/shm"), "{p:?}");
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        std::fs::create_dir_all(p.join("a/b")).unwrap();
        std::fs::write(p.join("a/b/raw.html"), "<html>").unwrap();
        let other = Scratch::new("job-1").unwrap();
        assert_ne!(other.path(), s.path(), "fresh dir per worker");
        drop(s);
        assert!(!p.exists(), "wiped");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn stale_scratch_of_a_dead_parent_is_swept_at_start_live_ones_kept() {
        let root = std::env::temp_dir().join(format!(
            "lillith-scratch-sweep-{}",
            super::super::random_token()
        ));
        std::fs::create_dir_all(&root).unwrap();
        // A pid that can't exist (over pid_max), ours, and init (alive).
        let dead = root.join(format!("{PREFIX}p4294967294-w-1-00"));
        let mine = Scratch::new_in(&root, "w-2", false).unwrap();
        let init = root.join(format!("{PREFIX}p1-w-3-00"));
        let legacy_new = root.join(format!("{PREFIX}w-4-00"));
        let other = root.join("not-a-worker");
        for d in [&dead, &init, &legacy_new, &other] {
            std::fs::create_dir_all(d.join("sub")).unwrap();
        }
        std::fs::write(dead.join("sub/raw.html"), "<html>").unwrap();
        // A symlink named like a scratch dir is never followed or removed.
        let link = root.join(format!("{PREFIX}p4294967293-w-5-00"));
        std::os::unix::fs::symlink(&other, &link).unwrap();
        assert!(mine
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(&format!("{PREFIX}p{}-", std::process::id())));
        assert_eq!(sweep_stale_in(&root), 1);
        assert!(!dead.exists(), "dead parent's scratch swept");
        assert!(mine.path().exists() && init.exists(), "live ones kept");
        assert!(legacy_new.exists(), "a fresh old-style dir is kept");
        assert!(other.join("sub").exists() && link.exists());
        drop(mine);
        let _ = std::fs::remove_dir_all(&root);
    }
}
