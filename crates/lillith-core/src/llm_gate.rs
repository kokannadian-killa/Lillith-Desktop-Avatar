//! One model request at a time, and chat always wins
//! (docs/SELF_LEARNING.md §1, "LlmGate").
//!
//! `llama-server` runs with one slot, so a background request would stall
//! Ko's chat. Every caller shares an advisory lock on
//! `<runtime>/lillith/llm.lock`:
//!
//! - **Chat** first bumps `<runtime>/lillith/llm.want` ([`LlmGate::signal_want`]),
//!   then takes the lock (waiting briefly; it never blocks for long and goes
//!   ahead without the lock if something is wrong, chat must not stall).
//! - **The learner** only ever uses [`LlmGate::try_lock`], remembers
//!   [`LlmGate::want_stamp`] when it got the lock, re-checks it while its
//!   reply streams, and drops the request as soon as the stamp changed
//!   ([`GateGuard::preempted`]).
//!
//! The lock is `flock(2)` on unix and `LockFileEx` on Windows; closing the
//! file (dropping the [`GateGuard`]) releases it, also when the process
//! dies. Nothing here talks to the network or starts a program.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// The shared lock + want files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmGate {
    lock: PathBuf,
    want: PathBuf,
}

/// Holds the model lock until dropped.
#[derive(Debug)]
pub struct GateGuard {
    _file: File,
    want: PathBuf,
    stamp: u128,
}

impl GateGuard {
    /// `true` once chat asked for the model after this guard was taken.
    pub fn preempted(&self) -> bool {
        read_stamp(&self.want) != self.stamp
    }
}

/// `$XDG_RUNTIME_DIR/lillith`, else `<data>/lillith/run` (Windows:
/// `%LOCALAPPDATA%\lillith\run`). Not created here.
pub fn runtime_dir() -> Option<PathBuf> {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(d) if !cfg!(windows) => Some(PathBuf::from(d).join("lillith")),
        _ => crate::config::data_dir().map(|d| d.join("run")),
    }
}

fn private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

fn open_private(path: &Path, truncate: bool) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.read(true).write(true).create(true).truncate(truncate);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

fn read_stamp(path: &Path) -> u128 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

impl LlmGate {
    /// The gate files inside `dir` (tests use a temp dir).
    pub fn at(dir: &Path) -> Self {
        Self {
            lock: dir.join("llm.lock"),
            want: dir.join("llm.want"),
        }
    }

    /// The gate every Lillith process shares ([`runtime_dir`]).
    pub fn shared() -> Option<Self> {
        runtime_dir().map(|d| Self::at(&d))
    }

    pub fn lock_path(&self) -> &Path {
        &self.lock
    }

    /// `true` when chat asked for the model less than `within` ago (a
    /// learner then doesn't even start).
    pub fn wanted_within(&self, within: std::time::Duration) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let w = self.want_stamp();
        w != 0 && now.saturating_sub(w) < within.as_nanos()
    }

    /// The current want stamp (0 = never asked).
    pub fn want_stamp(&self) -> u128 {
        read_stamp(&self.want)
    }

    /// Chat side: "I want the model now". Writes a new, larger stamp
    /// (atomic tmp + rename).
    pub fn signal_want(&self) -> io::Result<()> {
        if let Some(d) = self.want.parent() {
            private_dir(d)?;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let next = now.max(self.want_stamp() + 1);
        let tmp = self.want.with_extension("want.tmp");
        {
            use std::io::Write;
            let mut f = open_private(&tmp, true)?;
            f.write_all(next.to_string().as_bytes())?;
        }
        std::fs::rename(&tmp, &self.want)
    }

    /// Takes the lock if nobody holds it (never waits).
    pub fn try_lock(&self) -> io::Result<Option<GateGuard>> {
        if let Some(d) = self.lock.parent() {
            private_dir(d)?;
        }
        let f = open_private(&self.lock, false)?;
        if !sys::try_lock(&f)? {
            return Ok(None);
        }
        Ok(Some(GateGuard {
            _file: f,
            want: self.want.clone(),
            stamp: self.want_stamp(),
        }))
    }

    /// Chat side, blocking: [`signal_want`](Self::signal_want), then poll
    /// [`try_lock`](Self::try_lock) for at most `max_wait`. `None` = go
    /// ahead without the lock (chat never stalls on the gate).
    pub fn chat_acquire(&self, max_wait: std::time::Duration) -> Option<GateGuard> {
        if let Err(e) = self.signal_want() {
            tracing::debug!("llm gate: want: {e}");
        }
        let start = std::time::Instant::now();
        loop {
            match self.try_lock() {
                Ok(Some(g)) => return Some(g),
                Ok(None) if start.elapsed() < max_wait => {
                    std::thread::sleep(std::time::Duration::from_millis(10))
                }
                Ok(None) => return None,
                Err(e) => {
                    tracing::debug!("llm gate: lock: {e}");
                    return None;
                }
            }
        }
    }
}

/// An exclusive advisory lock on `path` (created 0600, its folder 0700)
/// if nobody holds it; released when the returned file is dropped. Used
/// for single-writer folders (the learner's `knowledge/.lock`).
pub fn try_lock_file(path: &Path) -> io::Result<Option<File>> {
    if let Some(d) = path.parent() {
        private_dir(d)?;
    }
    let f = open_private(path, false)?;
    Ok(sys::try_lock(&f)?.then_some(f))
}

#[cfg(unix)]
mod sys {
    use std::fs::File;
    use std::os::unix::io::AsRawFd;

    pub fn try_lock(f: &File) -> std::io::Result<bool> {
        // SAFETY: flock on a valid, owned fd; no memory is passed.
        let r = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if r == 0 {
            return Ok(true);
        }
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
            Ok(false)
        } else {
            Err(e)
        }
    }
}

#[cfg(windows)]
mod sys {
    use std::fs::File;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{ERROR_LOCK_VIOLATION, HANDLE};
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;

    pub fn try_lock(f: &File) -> std::io::Result<bool> {
        // SAFETY: a zeroed OVERLAPPED (offset 0) and a valid file handle.
        let ok = unsafe {
            let mut ov: OVERLAPPED = std::mem::zeroed();
            LockFileEx(
                f.as_raw_handle() as HANDLE,
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                0,
                1,
                0,
                &mut ov,
            )
        };
        if ok != 0 {
            return Ok(true);
        }
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
            Ok(false)
        } else {
            Err(e)
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod sys {
    pub fn try_lock(_f: &std::fs::File) -> std::io::Result<bool> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lillith-gate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn one_holder_at_a_time_and_release_on_drop() {
        let d = tmp("one");
        let g = LlmGate::at(&d);
        let a = g.try_lock().unwrap().expect("free");
        assert!(g.try_lock().unwrap().is_none(), "held");
        drop(a);
        assert!(g.try_lock().unwrap().is_some(), "released on drop");
        let f = try_lock_file(&d.join("k/.lock")).unwrap().expect("free");
        assert!(try_lock_file(&d.join("k/.lock")).unwrap().is_none());
        drop(f);
        assert!(try_lock_file(&d.join("k/.lock")).unwrap().is_some());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn want_preempts_holder_and_chat_gets_the_lock() {
        let d = tmp("want");
        let g = LlmGate::at(&d);
        let held = g.try_lock().unwrap().unwrap();
        assert!(!held.preempted());
        assert!(!g.wanted_within(Duration::from_secs(2)));
        g.signal_want().unwrap();
        assert!(held.preempted());
        assert!(g.wanted_within(Duration::from_secs(2)));
        // Chat can't get it while held, so it gives up after max_wait...
        assert!(g.chat_acquire(Duration::from_millis(30)).is_none());
        drop(held);
        // ...and gets it once the learner let go.
        assert!(g.chat_acquire(Duration::from_millis(30)).is_some());
        let _ = std::fs::remove_dir_all(d);
    }

    #[cfg(unix)]
    #[test]
    fn gate_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("perm");
        let g = LlmGate::at(&d);
        drop(g.try_lock().unwrap());
        g.signal_want().unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&d), 0o700);
        assert_eq!(mode(&d.join("llm.lock")), 0o600);
        assert_eq!(mode(&d.join("llm.want")), 0o600);
        let _ = std::fs::remove_dir_all(d);
    }
}
