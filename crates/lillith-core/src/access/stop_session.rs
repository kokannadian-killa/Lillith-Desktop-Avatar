//! The cross-process stop barrier: how a Stop / Ctrl-C / kill switch fired in
//! one Lillith process (say the detached `lillith-settings` window) reaches
//! every other Lillith process of the same user (the overlay, `lillith chat`,
//! a dream session) and how the stopper learns whether they really went quiet.
//!
//! It is files in one private, per-user directory (`<runtime>/stop/`, mode
//! 0700, files 0600; [`crate::llm_gate::runtime_dir`]). No socket, no TCP,
//! nothing leaves the machine. Only processes of the same user can write
//! there; a directory owned by someone else or open to group/others is
//! refused.
//!
//! - `barrier.log`: append-only. [`super::kill_switch::stop_all`] appends one
//!   line and fsyncs it **before** it reports anything. Its length is the
//!   barrier generation; every joined process folds it into
//!   [`super::kill_switch::epoch`], so every command runner that polls the
//!   epoch (and every worker that checks `stopped_since`) sees a stop from
//!   any process, even one that crashed right after writing it. A process
//!   only compares against the generation it saw when its work started, so
//!   an old barrier never stops new work after a restart, and nothing is
//!   re-run or re-approved because of it (there is no queue to replay).
//! - `proc-<pid>`: "I am a joined Lillith process" (pid + Linux start time,
//!   so a reused pid is not mistaken for us). Stale entries are pruned.
//! - `ack-<pid>`: each joined process's watcher thread writes, for the
//!   newest generation it saw, whether its stopped command trees are really
//!   gone ([`Quiet::Quiet`]), still alive ([`Quiet::Busy`]) or could not be
//!   checked on this OS ([`Quiet::Unverified`]).
//!
//! [`confirm`] turns those into a [`StopOutcome`]: `Quiescent` only when every
//! live joined process acknowledged this generation with verified-empty
//! trees; otherwise `Incomplete` (an unanswered process is never counted as
//! stopped).
//!
//! The safeword ("red" / "OOC: stop") never touches any of this: it only
//! hushes speech in its own process.
//!
//! Limits (honest): process-group tracking cannot see a descendant that left
//! its group (`setsid`, double fork); such an escapee is not found and not
//! killed. Tree verification needs `/proc` (Linux); elsewhere acks say
//! `Unverified` and the outcome is `Incomplete`.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The barrier file inside the session directory.
pub const BARRIER_FILE: &str = "barrier.log";

/// How long a process's watcher waits for its stopped trees to vanish before
/// it acknowledges `Busy`.
pub const LOCAL_QUIET_DEADLINE: Duration = Duration::from_secs(3);

/// Default time [`super::kill_switch::stop_all_confirmed`] waits for acks.
pub const CONFIRM_DEADLINE: Duration = Duration::from_secs(5);

const WATCH_EVERY: Duration = Duration::from_millis(50);

/// The joined session (one per process).
struct Session {
    dir: PathBuf,
    barrier: PathBuf,
    /// Last generation read successfully (returned on a transient error).
    last: AtomicU64,
    /// The generation this process's own last stop produced (its watcher
    /// does not re-run the stop hooks for it).
    own: AtomicU64,
}

static SESSION: OnceLock<Session> = OnceLock::new();

/// Whether this process joined a stop session.
pub fn joined() -> bool {
    SESSION.get().is_some()
}

/// The joined session's directory.
pub fn dir() -> Option<PathBuf> {
    SESSION.get().map(|s| s.dir.clone())
}

/// The default session directory: `<runtime>/stop`.
pub fn default_dir() -> Option<PathBuf> {
    crate::llm_gate::runtime_dir().map(|d| d.join("stop"))
}

/// Joins the per-user stop session at [`default_dir`]. Apps call this once
/// at start-up (the overlay / `lillith` CLI and `lillith-settings`). Library
/// users and unit tests that never join keep the old, process-local stop.
pub fn join() -> io::Result<PathBuf> {
    let d = default_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no runtime directory"))?;
    join_at(&d)
}

/// [`join`] at an explicit directory (tests use a temp dir). Joining twice
/// is a no-op that returns the first directory.
pub fn join_at(dir: &Path) -> io::Result<PathBuf> {
    if let Some(s) = SESSION.get() {
        return Ok(s.dir.clone());
    }
    private_dir(dir)?;
    check_private(dir)?;
    let barrier = dir.join(BARRIER_FILE);
    // Make sure the file exists so its length is a real reading.
    open_append(&barrier)?;
    let g = std::fs::metadata(&barrier)?.len();
    let s = Session {
        dir: dir.to_path_buf(),
        barrier,
        last: AtomicU64::new(g),
        own: AtomicU64::new(0),
    };
    if SESSION.set(s).is_err() {
        return Ok(SESSION.get().map(|s| s.dir.clone()).unwrap_or_default());
    }
    register_self(dir)?;
    let start = g;
    std::thread::Builder::new()
        .name("lillith-stop-watch".into())
        .spawn(move || watch(start))?;
    tracing::info!(dir = %dir.display(), "joined the cross-process stop session");
    Ok(dir.to_path_buf())
}

/// The barrier generation (0 when not joined). Cheap: one `stat`.
pub fn generation() -> u64 {
    let Some(s) = SESSION.get() else {
        return 0;
    };
    match std::fs::metadata(&s.barrier) {
        Ok(m) => {
            s.last.store(m.len(), Ordering::SeqCst);
            m.len()
        }
        // Deleted under us: the reading changes, which fails closed (every
        // runner treats it as a stop).
        Err(e) if e.kind() == io::ErrorKind::NotFound => 0,
        Err(_) => s.last.load(Ordering::SeqCst),
    }
}

/// Appends one stop record to the barrier and fsyncs it. Returns the new
/// generation. `Err` when not joined or the write failed (the stop then
/// only reached this process).
pub fn persist(reason: &str) -> io::Result<u64> {
    let s = SESSION
        .get()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "no stop session joined"))?;
    let mut f = open_append(&s.barrier)?;
    let line = format!(
        "{}\tpid={}\t{}\n",
        super::now_secs(),
        std::process::id(),
        reason.replace(['\n', '\t'], " ")
    );
    f.write_all(line.as_bytes())?;
    f.sync_all()?;
    let g = f.metadata()?.len();
    s.own.store(g, Ordering::SeqCst);
    s.last.store(g, Ordering::SeqCst);
    Ok(g)
}

// ------------------------------------------------------------ local trees

/// One command tree (process group) a runner started, tracked so a stop can
/// check it is really gone. Dropping it (the runner returned) moves a
/// stopped group to the "must vanish" list.
#[must_use = "the group is untracked when this is dropped"]
pub struct GroupGuard {
    pgid: u32,
    start: u64,
}

struct Tracked {
    active: Vec<(u32, u64)>,
    stopped: Vec<u32>,
}

static TRACKED: Mutex<Tracked> = Mutex::new(Tracked {
    active: Vec::new(),
    stopped: Vec::new(),
});

/// Tracks the process group `pgid` a runner started at stop epoch `start`
/// (a [`super::kill_switch::epoch`] value).
pub fn track_group(pgid: u32, start: u64) -> GroupGuard {
    if let Ok(mut t) = TRACKED.lock() {
        t.active.push((pgid, start));
    }
    GroupGuard { pgid, start }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        let stopped = super::kill_switch::stopped_since(self.start);
        if let Ok(mut t) = TRACKED.lock() {
            if let Some(i) = t
                .active
                .iter()
                .position(|&(p, s)| p == self.pgid && s == self.start)
            {
                t.active.swap_remove(i);
            }
            if stopped && !t.stopped.contains(&self.pgid) {
                t.stopped.push(self.pgid);
            }
        }
    }
}

/// Whether this process's stopped work is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quiet {
    /// Every runner that was going when the stop came returned and no
    /// process of their groups is left (checked in `/proc`).
    Quiet,
    /// Runners returned but the trees could not be checked (no `/proc`).
    Unverified,
    /// This many runners / group members are still alive.
    Busy(usize),
}

impl Quiet {
    fn word(self) -> String {
        match self {
            Quiet::Quiet => "quiet".into(),
            Quiet::Unverified => "unverified".into(),
            Quiet::Busy(n) => format!("busy {n}"),
        }
    }

    fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        match s {
            "quiet" => Some(Quiet::Quiet),
            "unverified" => Some(Quiet::Unverified),
            _ => s
                .strip_prefix("busy ")?
                .trim()
                .parse()
                .ok()
                .map(Quiet::Busy),
        }
    }
}

/// Waits up to `deadline` for every runner that started before the latest
/// stop to return and for every stopped group to be empty.
pub fn local_quiescence(deadline: Duration) -> Quiet {
    let t0 = Instant::now();
    loop {
        let now = super::kill_switch::epoch();
        let (old, groups) = match TRACKED.lock() {
            Ok(mut t) => {
                let old = t.active.iter().filter(|&&(_, s)| s != now).count();
                t.stopped.retain(|&g| group_alive(g).unwrap_or(false));
                (old, t.stopped.clone())
            }
            Err(_) => (0, Vec::new()),
        };
        let left: usize = groups
            .iter()
            .filter(|&&g| group_alive(g) == Some(true))
            .count();
        if old == 0 && left == 0 {
            return if cfg!(target_os = "linux") {
                Quiet::Quiet
            } else {
                Quiet::Unverified
            };
        }
        if t0.elapsed() >= deadline {
            return Quiet::Busy(old + left);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Whether any live (non-zombie) process is in group `pgid`. `None` when it
/// can't be checked (no `/proc`).
pub fn group_alive(pgid: u32) -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        let rd = std::fs::read_dir("/proc").ok()?;
        for e in rd.flatten() {
            let name = e.file_name();
            let Some(n) = name.to_str() else { continue };
            if !n.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            if let Some((state, _ppid, pgrp)) = proc_stat(n.parse().unwrap_or(0)) {
                if pgrp == pgid && state != 'Z' && state != 'X' {
                    return Some(true);
                }
            }
        }
        Some(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pgid;
        None
    }
}

/// `(state, ppid, pgrp)` from `/proc/<pid>/stat`.
#[cfg(target_os = "linux")]
fn proc_stat(pid: u32) -> Option<(char, u32, u32)> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &s[s.rfind(')')? + 2..];
    let mut it = rest.split_whitespace();
    let state = it.next()?.chars().next()?;
    let ppid = it.next()?.parse().ok()?;
    let pgrp = it.next()?.parse().ok()?;
    Some((state, ppid, pgrp))
}

/// Linux start time (clock ticks after boot) of `pid`, field 22.
#[cfg(target_os = "linux")]
fn start_time(pid: u32) -> Option<u64> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &s[s.rfind(')')? + 2..];
    rest.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(not(target_os = "linux"))]
fn start_time(_pid: u32) -> Option<u64> {
    None
}

/// Whether the registered process `pid` (with start time `start`, 0 =
/// unknown) is still that same, live process. `None`: can't tell.
fn registered_alive(pid: u32, start: u64) -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        match proc_stat(pid) {
            None => Some(false),
            Some((st, _, _)) if st == 'Z' || st == 'X' => Some(false),
            Some(_) => Some(start == 0 || start_time(pid) == Some(start)),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (pid, start);
        None
    }
}

// ------------------------------------------------------------ watcher/acks

fn register_self(dir: &Path) -> io::Result<()> {
    let pid = std::process::id();
    let st = start_time(pid).unwrap_or(0);
    write_atomic(&dir.join(format!("proc-{pid}")), &format!("{pid} {st}\n"))
}

/// The watcher: on every new generation (any process's stop), run this
/// process's stop hooks (unless the stop was our own: [`super::kill_switch::stop_all`]
/// already ran them), wait for the stopped trees to vanish, then write the
/// ack.
fn watch(mut seen: u64) {
    let Some(s) = SESSION.get() else { return };
    loop {
        std::thread::sleep(WATCH_EVERY);
        let g = generation();
        if g == seen {
            continue;
        }
        seen = g;
        if g != s.own.load(Ordering::SeqCst) {
            tracing::warn!("stop barrier moved (generation {g}): another Lillith process pulled the kill switch");
            super::kill_switch::run_foreign_stop_hooks();
        }
        let q = local_quiescence(LOCAL_QUIET_DEADLINE);
        let pid = std::process::id();
        let body = format!("{g} {}\n", q.word());
        if let Err(e) = write_atomic(&s.dir.join(format!("ack-{pid}")), &body) {
            tracing::warn!("could not write stop ack: {e}");
        }
    }
}

/// What a cross-process stop achieved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopOutcome {
    /// The barrier was not saved (no session joined, or the write failed):
    /// only this process was stopped. Never shown as "stopped everywhere".
    LocalOnly { reason: String, local: Quiet },
    /// Every live joined process acknowledged generation `generation` with
    /// verified-empty trees.
    Quiescent { generation: u64, processes: usize },
    /// The barrier is saved (`generation`), but not every process confirmed.
    Incomplete {
        generation: u64,
        /// Live processes that didn't acknowledge in time.
        unconfirmed: Vec<u32>,
        /// Processes that acknowledged with trees still alive (pid, count).
        busy: Vec<(u32, usize)>,
        /// Processes that acknowledged without being able to check.
        unverified: Vec<u32>,
    },
}

impl StopOutcome {
    /// Only `Quiescent` is a confirmed stop.
    pub fn confirmed(&self) -> bool {
        matches!(self, StopOutcome::Quiescent { .. })
    }

    /// One truthful line for the UI.
    pub fn line(&self) -> String {
        match self {
            StopOutcome::Quiescent { processes, .. } => format!(
                "Stopped: {processes} Lillith process(es) confirmed their task trees are gone."
            ),
            StopOutcome::Incomplete {
                unconfirmed,
                busy,
                unverified,
                ..
            } => {
                let mut parts = Vec::new();
                if !unconfirmed.is_empty() {
                    parts.push(format!("{} process(es) did not answer", unconfirmed.len()));
                }
                if !busy.is_empty() {
                    let n: usize = busy.iter().map(|b| b.1).sum();
                    parts.push(format!("{n} task process(es) still alive"));
                }
                if !unverified.is_empty() {
                    parts.push(format!("{} could not be checked", unverified.len()));
                }
                format!(
                    "Stop saved but NOT confirmed ({}): treat that work as possibly still running.",
                    parts.join(", ")
                )
            }
            StopOutcome::LocalOnly { reason, local } => format!(
                "Stopped in this window only ({}); other Lillith windows were NOT told: {reason}.",
                match local {
                    Quiet::Quiet => "its task trees are gone".to_string(),
                    Quiet::Unverified => "not verifiable here".to_string(),
                    Quiet::Busy(n) => format!("{n} task process(es) still alive"),
                }
            ),
        }
    }
}

/// Waits up to `deadline` until every live joined process acknowledged
/// generation `generation` (or newer).
pub fn confirm(generation: u64, deadline: Duration) -> StopOutcome {
    let Some(dir) = dir() else {
        return StopOutcome::LocalOnly {
            reason: "no stop session".into(),
            local: local_quiescence(deadline),
        };
    };
    confirm_in(&dir, generation, deadline)
}

/// [`confirm`] against an explicit session directory.
pub fn confirm_in(dir: &Path, generation: u64, deadline: Duration) -> StopOutcome {
    let t0 = Instant::now();
    loop {
        let mut unconfirmed = Vec::new();
        let mut busy = Vec::new();
        let mut unverified = Vec::new();
        let mut ok = 0usize;
        for (pid, start) in registered(dir) {
            match registered_alive(pid, start) {
                Some(false) => {
                    // Dead or a reused pid: prune it, it runs nothing of ours.
                    let _ = std::fs::remove_file(dir.join(format!("proc-{pid}")));
                    let _ = std::fs::remove_file(dir.join(format!("ack-{pid}")));
                    continue;
                }
                Some(true) | None => {}
            }
            match read_ack(dir, pid) {
                Some((g, q)) if g >= generation => match q {
                    Quiet::Quiet => ok += 1,
                    Quiet::Unverified => unverified.push(pid),
                    Quiet::Busy(n) => busy.push((pid, n)),
                },
                _ => unconfirmed.push(pid),
            }
        }
        if unconfirmed.is_empty() && busy.is_empty() && unverified.is_empty() {
            return StopOutcome::Quiescent {
                generation,
                processes: ok,
            };
        }
        if t0.elapsed() >= deadline {
            unconfirmed.sort_unstable();
            return StopOutcome::Incomplete {
                generation,
                unconfirmed,
                busy,
                unverified,
            };
        }
        std::thread::sleep(WATCH_EVERY);
    }
}

fn registered(dir: &Path) -> Vec<(u32, u64)> {
    let mut v = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return v;
    };
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix("proc-"))
            .and_then(|p| p.parse::<u32>().ok())
        else {
            continue;
        };
        let start = std::fs::read_to_string(e.path())
            .ok()
            .and_then(|s| s.split_whitespace().nth(1)?.parse().ok())
            .unwrap_or(0);
        v.push((pid, start));
    }
    v.sort_unstable();
    v
}

fn read_ack(dir: &Path, pid: u32) -> Option<(u64, Quiet)> {
    let s = std::fs::read_to_string(dir.join(format!("ack-{pid}"))).ok()?;
    let s = s.trim();
    let (g, q) = s.split_once(' ')?;
    Some((g.parse().ok()?, Quiet::parse(q)?))
}

// ------------------------------------------------------------ file helpers

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

/// Refuses a session directory another user owns or others can write.
fn check_private(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::symlink_metadata(dir)?;
        if !m.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "stop session path is not a directory",
            ));
        }
        // SAFETY: geteuid has no preconditions.
        let me = unsafe { libc::geteuid() };
        if m.uid() != me {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "stop session directory belongs to another user",
            ));
        }
        if m.mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "stop session directory is open to other users (needs 0700)",
            ));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
    Ok(())
}

fn open_append(path: &Path) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    o.open(path)
}

fn write_atomic(path: &Path, body: &str) -> io::Result<()> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    {
        let mut o = OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let mut f = o.open(&tmp)?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_words_round_trip() {
        for q in [Quiet::Quiet, Quiet::Unverified, Quiet::Busy(3)] {
            assert_eq!(Quiet::parse(&q.word()), Some(q));
        }
        assert_eq!(Quiet::parse("nonsense"), None);
    }

    #[test]
    fn only_quiescent_is_confirmed_and_lines_never_overclaim() {
        let q = StopOutcome::Quiescent {
            generation: 9,
            processes: 2,
        };
        assert!(q.confirmed());
        assert!(q.line().starts_with("Stopped"));
        let i = StopOutcome::Incomplete {
            generation: 9,
            unconfirmed: vec![42],
            busy: vec![],
            unverified: vec![],
        };
        assert!(!i.confirmed());
        assert!(i.line().contains("NOT confirmed"));
        assert!(!i.line().starts_with("Stopped"));
        let l = StopOutcome::LocalOnly {
            reason: "x".into(),
            local: Quiet::Quiet,
        };
        assert!(!l.confirmed());
        assert!(l.line().contains("NOT told"));
    }

    #[test]
    fn a_live_process_that_never_acks_is_unconfirmed_and_dead_ones_are_pruned() {
        let d = std::env::temp_dir().join(format!("lillith-stopsess-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        private_dir(&d).unwrap();
        // Ourselves, registered but never acking (no watcher here).
        let me = std::process::id();
        let st = start_time(me).unwrap_or(0);
        std::fs::write(d.join(format!("proc-{me}")), format!("{me} {st}\n")).unwrap();
        // A pid that cannot exist on Linux (> pid_max).
        std::fs::write(d.join("proc-4194399"), "4194399 1\n").unwrap();
        let out = confirm_in(&d, 1, Duration::from_millis(200));
        match out {
            StopOutcome::Incomplete { unconfirmed, .. } if cfg!(target_os = "linux") => {
                assert_eq!(unconfirmed, vec![me]);
                assert!(!d.join("proc-4194399").exists(), "dead entry pruned");
            }
            StopOutcome::Incomplete { .. } => {}
            o => panic!("never confirmed without an ack: {o:?}"),
        }
        // A matching ack confirms.
        std::fs::write(d.join(format!("ack-{me}")), "5 quiet\n").unwrap();
        assert!(
            confirm_in(&d, 5, Duration::from_millis(200)).confirmed() || !cfg!(target_os = "linux")
        );
        // An older ack does not.
        assert!(!confirm_in(&d, 6, Duration::from_millis(100)).confirmed());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn a_session_dir_open_to_others_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("lillith-stopsess-perm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(check_private(&d).is_err());
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(check_private(&d).is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }
}
