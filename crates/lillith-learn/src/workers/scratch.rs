//! A worker's scratch dir: tmpfs (`/dev/shm`) where there is one, mode
//! 0700, unique per worker, and wiped when the guard drops — on success,
//! kill or crash (the supervisor holds the guard, never the worker).

use std::path::{Path, PathBuf};

/// The tmpfs root, if this machine has one we can write.
fn tmpfs_root() -> Option<PathBuf> {
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
            "lillith-worker-{safe}-{:016x}",
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
}
