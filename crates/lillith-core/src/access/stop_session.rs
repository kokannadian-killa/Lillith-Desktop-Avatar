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
//!   so a reused pid is not mistaken for us), plus every command tree it
//!   currently runs (or stopped but has not seen vanish): each group id with
//!   its leader's start time and every descendant (pid + start time) its
//!   runner's ~200 ms `/proc` snapshots found. Rewritten (atomic rename,
//!   fsynced, 0600) whenever a runner starts or returns or a snapshot grows.
//!   An entry whose process is dead (SIGKILLed / crashed) is only pruned
//!   after [`confirm_in`] killed every recorded tree and verified it gone.
//! - `ack-<pid>`: each joined process's watcher thread writes, for the
//!   newest generation it saw, whether its stopped command trees are really
//!   gone ([`Quiet::Quiet`]), still alive ([`Quiet::Busy`]) or could not be
//!   checked on this OS ([`Quiet::Unverified`]).
//!
//! - `disarmed`: written (fsynced) with every global stop. While it exists,
//!   [`super::commands::run_child`] refuses to start anything, in every
//!   process, including ones started after the stop. Only an explicit
//!   [`rearm`] (Settings ▸ Bots ▸ Re-arm) removes it; re-arming never
//!   re-approves or re-runs anything.
//!
//! Joining: every process joins lazily on first use (auto-join) unless
//! `LILLITH_STOP_SESSION=off` (`=on` forces it). `off` only works in debug
//! builds; release builds ignore it (with a warning) so no environment
//! variable can take a shipped Lillith out of the kill switch's reach. Under `cargo test` /
//! `cargo run` (`CARGO_MANIFEST_DIR` set at run time, or a test binary
//! started directly from `target/…/deps/`) auto-join only happens
//! in a fully scratch XDG environment (all of `XDG_RUNTIME_DIR`,
//! `XDG_STATE_HOME`, `XDG_DATA_HOME`, `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`
//! set, and the runtime dir inside the same private tree as the state dir,
//! never `/run/user/…` or an inherited `/tmp/xdg-runtime-…`), so an ordinary test run never stops
//! or disarms a live Lillith; the binaries' explicit [`join`] follows the
//! same rule. lillith-core's own unit tests never auto-join.
//!
//! [`confirm`] turns those into a [`StopOutcome`]: `Quiescent` only when every
//! live joined process acknowledged this generation with verified-empty
//! tracked trees and every tree a dead process left behind was killed and
//! verified gone; otherwise `Incomplete` (an unanswered process, a dead
//! process's tree that survives the kill, or one that can't be checked is
//! never counted as stopped).
//!
//! Pid reuse: every pid is recorded with its Linux start time
//! (`/proc/<pid>/stat` field 22) and only signalled while that still
//! matches. A group is only killed while its leader pid is gone (Linux never
//! reuses a pid that is still a live group's id) or still has the recorded
//! start time.
//!
//! The safeword ("red" / "OOC: stop") never touches any of this: it only
//! hushes speech in its own process.
//!
//! Containment: on a stop a runner kills its process group **and** every
//! descendant it has seen (a `/proc` parent walk, refreshed while the
//! command runs and repeated at kill time), including ones that left the
//! group with `setsid`, plus their new groups. Limits (honest): a descendant
//! whose parent chain was already broken before the runner saw it (a
//! double fork that finished between two ~200 ms snapshots) is not found.
//! Tree verification needs `/proc` (Linux); elsewhere acks say `Unverified`
//! and the outcome is `Incomplete`.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The barrier file inside the session directory.
pub const BARRIER_FILE: &str = "barrier.log";

/// Present while dispatch is disarmed (after a global stop, until [`rearm`]).
pub const DISARMED_FILE: &str = "disarmed";

/// What a refused dispatch says.
pub const DISARMED_MSG: &str = "commands are off: the kill switch was pulled (Stop all / Ctrl-C). \
Nothing was started. Re-arm in Settings ▸ Bots to run commands again.";

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
    /// This process's Linux start time (0 = unknown), for its entry.
    self_start: u64,
}

static SESSION: OnceLock<Session> = OnceLock::new();

static AUTO: std::sync::Once = std::sync::Once::new();

/// Joins on first use (see the module docs for when it doesn't).
fn session() -> Option<&'static Session> {
    if let Some(s) = SESSION.get() {
        return Some(s);
    }
    AUTO.call_once(|| {
        if cfg!(test) {
            return;
        }
        match join_rule() {
            JoinRule::Join => {
                if let Err(e) = join() {
                    tracing::warn!(
                        "cross-process stop unavailable (stops stay in this process): {e}"
                    );
                }
            }
            JoinRule::OptedOut => note_opt_out(),
            JoinRule::No => {}
        }
    });
    SESSION.get()
}

/// The outcome of the join rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JoinRule {
    Join,
    /// `LILLITH_STOP_SESSION=off` (debug builds only) where this process
    /// would otherwise join: it stays out, but leaves an `optout-<pid>`
    /// marker so a stop never claims it was checked.
    OptedOut,
    /// A cargo test / run without fully scratch XDG dirs: nothing is
    /// written anywhere (never the live session).
    No,
}

/// The environment part of the join rule, shared by auto-join and the
/// binaries' explicit [`join`]: `LILLITH_STOP_SESSION`, and never the live
/// session from a cargo test / run without fully scratch XDG dirs.
fn join_rule() -> JoinRule {
    let off = match std::env::var("LILLITH_STOP_SESSION").as_deref() {
        // Release builds never let an environment variable take a process
        // out of the kill switch's reach.
        Ok("off") if cfg!(debug_assertions) => true,
        Ok("off") => {
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| {
                tracing::warn!(
                    "LILLITH_STOP_SESSION=off is ignored in release builds: this process joins the cross-process stop session"
                );
            });
            false
        }
        Ok("on") => return JoinRule::Join,
        _ => false,
    };
    match (cargo_rule_allows(), off) {
        (false, _) => JoinRule::No,
        (true, true) => JoinRule::OptedOut,
        (true, false) => JoinRule::Join,
    }
}

/// Writes (once) this process's `optout-<pid>` marker: "a Lillith process
/// that may run commands is outside the stop session". Best effort.
fn note_opt_out() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Some(d) = default_dir() else { return };
        if private_dir(&d).is_err() || check_private(&d).is_err() {
            return;
        }
        let pid = std::process::id();
        let st = start_time(pid).unwrap_or(0);
        match write_atomic(&d.join(format!("optout-{pid}")), &format!("{pid} {st}\n")) {
            Ok(()) => tracing::warn!(
                "LILLITH_STOP_SESSION=off: this process is outside the cross-process stop; stops elsewhere report it as not stopped"
            ),
            Err(e) => tracing::warn!("could not leave the stop-session opt-out marker: {e}"),
        }
    });
}

/// The cargo test / run part of the join rule.
fn cargo_rule_allows() -> bool {
    // A cargo test binary run directly lives in `target/<profile>/deps/`.
    let test_binary = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.ends_with("deps")))
        .unwrap_or(false);
    if test_binary || std::env::var_os("CARGO_MANIFEST_DIR").is_some() {
        // A cargo test / run: only into a fully scratch XDG environment
        // (every XDG base dir set, runtime dir not the session's
        // /run/user/<uid>), never the live session's stop barrier.
        let set = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
        let all = [
            "XDG_STATE_HOME",
            "XDG_DATA_HOME",
            "XDG_CONFIG_HOME",
            "XDG_CACHE_HOME",
        ]
        .iter()
        .all(|k| set(k).is_some());
        let (Some(rt), Some(st)) = (set("XDG_RUNTIME_DIR"), set("XDG_STATE_HOME")) else {
            return false;
        };
        return all && scratch_runtime(Path::new(&rt), Path::new(&st), &std::env::temp_dir());
    }
    true
}

/// Whether `runtime` belongs to the same scratch tree as `state` (a test that
/// set only some XDG dirs inherits the live session's runtime dir, e.g.
/// `/run/user/<uid>` or `/tmp/xdg-runtime-…`, which must never be joined):
/// the runtime dir's parent must hold the state dir and be deeper than the
/// temp dir / filesystem root.
fn scratch_runtime(runtime: &Path, state: &Path, tmp: &Path) -> bool {
    if runtime.starts_with("/run/user") {
        return false;
    }
    let Some(root) = runtime.parent() else {
        return false;
    };
    root != Path::new("/") && root != tmp && !tmp.starts_with(root) && state.starts_with(root)
}

/// Whether this process joined a stop session (joining now if allowed).
pub fn joined() -> bool {
    session().is_some()
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
    let rule = join_rule();
    if rule == JoinRule::OptedOut {
        note_opt_out();
    }
    if rule != JoinRule::Join {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "stop session off (LILLITH_STOP_SESSION=off, or a cargo test/run without scratch XDG dirs)",
        ));
    }
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
        self_start: start_time(std::process::id()).unwrap_or(0),
    };
    if SESSION.set(s).is_err() {
        return Ok(SESSION.get().map(|s| s.dir.clone()).unwrap_or_default());
    }
    register_self()?;
    let start = g;
    std::thread::Builder::new()
        .name("lillith-stop-watch".into())
        .spawn(move || watch(start))?;
    tracing::info!(dir = %dir.display(), "joined the cross-process stop session");
    Ok(dir.to_path_buf())
}

/// The barrier generation (0 when not joined). Cheap: one `stat`.
pub fn generation() -> u64 {
    let Some(s) = session() else {
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
    let s = session()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "no stop session joined"))?;
    // Disarm first: nothing may be dispatched once the stop is reported.
    write_atomic(
        &s.dir.join(DISARMED_FILE),
        &format!("pid={}\n", std::process::id()),
    )?;
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

/// Whether dispatch is disarmed (a global stop happened and nobody re-armed).
/// `false` when not joined (process-local behaviour, as before).
pub fn disarmed() -> bool {
    session().is_some_and(|s| s.dir.join(DISARMED_FILE).exists())
}

/// Re-arms dispatch after a global stop (an explicit user action). It does
/// not move any epoch, re-approve or re-run anything: old plans and queued
/// steps were dropped by the stop.
pub fn rearm() -> io::Result<()> {
    let s = session()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "no stop session joined"))?;
    match std::fs::remove_file(s.dir.join(DISARMED_FILE)) {
        Ok(()) => {
            tracing::warn!(
                "kill switch re-armed: commands may run again (each still needs its own Yes)"
            );
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

// ------------------------------------------------------------ local trees

/// One command tree (process group) a runner started, tracked so a stop can
/// check it is really gone. Dropping it (the runner returned) moves a
/// stopped group to the "must vanish" list.
///
/// Every change (a runner starts or returns, its descendant snapshot grows,
/// a stopped tree vanishes) is also written to this process's
/// `proc-<pid>` entry (fsynced, 0600), so if this process is SIGKILLed or
/// crashes mid-command the next stop can still find, kill and check the
/// trees it left behind (see [`confirm_in`]).
#[must_use = "the group is untracked when this is dropped"]
pub struct GroupGuard {
    id: u64,
    start: u64,
}

impl GroupGuard {
    /// Remembers descendants the runner saw (so a stop can check them, also
    /// from another process if this one dies). Persists the entry when
    /// anything new was added.
    pub fn note_pids(&mut self, pids: &[(u32, u64)]) {
        let mut changed = false;
        if let Ok(mut t) = TRACKED.lock() {
            if let Some(tree) = t.active.iter_mut().find(|x| x.id == self.id) {
                for p in pids {
                    if !tree.pids.contains(p) {
                        tree.pids.push(*p);
                        changed = true;
                    }
                }
            }
        }
        if changed {
            persist_trees();
        }
    }
}

/// A tracked command tree.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Tree {
    id: u64,
    /// The group (= the runner's direct child's pid).
    pgid: u32,
    /// Linux start time of the group leader (0 = unknown).
    leader_start: u64,
    /// The kill-switch epoch the runner started at.
    epoch: u64,
    /// Descendants (pid, Linux start time) the runner saw.
    pids: Vec<(u32, u64)>,
}

struct Tracked {
    next: u64,
    active: Vec<Tree>,
    stopped: Vec<Tree>,
}

static TRACKED: Mutex<Tracked> = Mutex::new(Tracked {
    next: 0,
    active: Vec::new(),
    stopped: Vec::new(),
});

/// Serializes `proc-<pid>` rewrites (snapshot + write under one lock, so an
/// older snapshot never overwrites a newer one).
static ENTRY_WRITE: Mutex<()> = Mutex::new(());

/// Tracks the process group `pgid` a runner started at stop epoch `start`
/// (a [`super::kill_switch::epoch`] value), and persists it.
pub fn track_group(pgid: u32, start: u64) -> GroupGuard {
    let leader_start = start_time(pgid).unwrap_or(0);
    let mut id = 0;
    if let Ok(mut t) = TRACKED.lock() {
        t.next += 1;
        id = t.next;
        t.active.push(Tree {
            id,
            pgid,
            leader_start,
            epoch: start,
            pids: Vec::new(),
        });
    }
    persist_trees();
    GroupGuard { id, start }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        let stopped = super::kill_switch::stopped_since(self.start);
        if let Ok(mut t) = TRACKED.lock() {
            let tree = t
                .active
                .iter()
                .position(|x| x.id == self.id)
                .map(|i| t.active.swap_remove(i));
            if stopped {
                // Drop stopped trees that are verifiably gone so the entry
                // does not grow without bound.
                t.stopped.retain(tree_maybe_alive);
                if let Some(tree) = tree {
                    t.stopped.push(tree);
                }
            }
        }
        persist_trees();
    }
}

/// Whether any process of `tree` may still be alive (unknown counts as alive).
fn tree_maybe_alive(tree: &Tree) -> bool {
    group_alive(tree.pgid).unwrap_or(true) || any_alive(&tree.pids)
}

/// Renders this process's entry: `<pid> <start>`, then `v2`, one
/// `g <pgid> <leader start>` per tracked group (running or stopped but not
/// yet verified gone), one `p <pid> <start>` per recorded descendant, then
/// `end` (a missing `end` means the entry can't be trusted).
fn render_entry(pid: u32, start: u64, trees: &[&Tree]) -> String {
    let mut s = format!("{pid} {start}\nv2\n");
    let mut seen: Vec<(u32, u64)> = Vec::new();
    for t in trees {
        s.push_str(&format!("g {} {}\n", t.pgid, t.leader_start));
        for p in &t.pids {
            if !seen.contains(p) {
                seen.push(*p);
                s.push_str(&format!("p {} {}\n", p.0, p.1));
            }
        }
    }
    s.push_str("end\n");
    s
}

/// What a `proc-<pid>` entry says a (possibly dead) process left running.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Entry {
    /// (pgid, leader start time).
    groups: Vec<(u32, u64)>,
    /// (pid, start time).
    pids: Vec<(u32, u64)>,
}

/// Parses a v2 entry. `None`: unreadable, an old format that did not record
/// trees, or truncated, i.e. its trees can't be checked.
fn parse_entry(body: &str) -> Option<Entry> {
    let mut lines = body.lines();
    let mut head = lines.next()?.split_whitespace();
    head.next()?.parse::<u32>().ok()?;
    head.next()?.parse::<u64>().ok()?;
    if lines.next()?.trim() != "v2" {
        return None;
    }
    let mut e = Entry::default();
    for l in lines {
        let mut it = l.split_whitespace();
        match it.next()? {
            "end" => return Some(e),
            "g" => e
                .groups
                .push((it.next()?.parse().ok()?, it.next()?.parse().ok()?)),
            "p" => e
                .pids
                .push((it.next()?.parse().ok()?, it.next()?.parse().ok()?)),
            _ => return None,
        }
    }
    None
}

/// Rewrites this process's entry from [`TRACKED`] (no-op when not joined).
fn persist_trees() {
    if SESSION.get().is_none() {
        return;
    }
    if let Err(e) = write_entry() {
        tracing::warn!("could not record running command trees for the stop session: {e}");
    }
}

/// Writes this process's `proc-<pid>` entry (atomic rename, fsynced, 0600).
fn write_entry() -> io::Result<()> {
    let s = SESSION
        .get()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "no stop session joined"))?;
    let _w = ENTRY_WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let pid = std::process::id();
    let body = {
        let t = TRACKED.lock().unwrap_or_else(|e| e.into_inner());
        let trees: Vec<&Tree> = t.active.iter().chain(t.stopped.iter()).collect();
        render_entry(pid, s.self_start, &trees)
    };
    write_atomic(&s.dir.join(format!("proc-{pid}")), &body)
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
    local_quiescence_counted(deadline).0
}

/// [`local_quiescence`] plus how many tracked command trees it saw stopped
/// and then gone (when `Quiet`).
fn local_quiescence_counted(deadline: Duration) -> (Quiet, usize) {
    let t0 = Instant::now();
    let mut ids: Vec<u64> = Vec::new();
    loop {
        let now = super::kill_switch::epoch();
        let (old, left, pruned) = match TRACKED.lock() {
            Ok(mut t) => {
                for x in t
                    .active
                    .iter()
                    .filter(|x| x.epoch != now)
                    .chain(t.stopped.iter())
                {
                    if !ids.contains(&x.id) {
                        ids.push(x.id);
                    }
                }
                let old = t.active.iter().filter(|x| x.epoch != now).count();
                let before = t.stopped.len();
                t.stopped
                    .retain(|x| group_alive(x.pgid).unwrap_or(false) || any_alive(&x.pids));
                (old, t.stopped.len(), t.stopped.len() != before)
            }
            Err(_) => (0, 0, false),
        };
        if pruned {
            persist_trees();
        }
        if old == 0 && left == 0 {
            return if cfg!(target_os = "linux") {
                (Quiet::Quiet, ids.len())
            } else {
                (Quiet::Unverified, ids.len())
            };
        }
        if t0.elapsed() >= deadline {
            return (Quiet::Busy(old + left), ids.len());
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

/// Whether any of `pids` (pid, start time) is still that same live process.
fn any_alive(pids: &[(u32, u64)]) -> bool {
    pids.iter()
        .any(|&(p, st)| registered_alive(p, st) == Some(true))
}

/// Every descendant of `root` (pid, start time), by a `/proc` parent walk.
/// Empty where there is no `/proc`.
pub fn descendants(root: u32) -> Vec<(u32, u64)> {
    #[cfg(target_os = "linux")]
    {
        let Ok(rd) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        let mut all: Vec<(u32, u32)> = Vec::new();
        for e in rd.flatten() {
            let name = e.file_name();
            let Some(pid) = name.to_str().and_then(|n| n.parse::<u32>().ok()) else {
                continue;
            };
            if let Some((_, ppid, _)) = proc_stat(pid) {
                all.push((pid, ppid));
            }
        }
        let mut out: Vec<u32> = Vec::new();
        let mut frontier = vec![root];
        while let Some(p) = frontier.pop() {
            for &(c, pp) in &all {
                if pp == p && c != root && !out.contains(&c) {
                    out.push(c);
                    frontier.push(c);
                }
            }
        }
        out.into_iter()
            .filter_map(|p| Some((p, start_time(p)?)))
            .collect()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = root;
        Vec::new()
    }
}

/// SIGKILLs `root`'s descendants (fresh walk plus `known`, each checked
/// against its start time so a reused pid is never hit) and the process
/// groups they moved to (never this process's own group). Repeats a few
/// times to catch forks in flight. Adds what it found to `known`.
pub fn kill_descendants(root: u32, known: &mut Vec<(u32, u64)>) {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: getpgrp has no preconditions.
        let own_pgrp = unsafe { libc::getpgrp() } as u32;
        for _ in 0..3 {
            for p in descendants(root) {
                if !known.contains(&p) {
                    known.push(p);
                }
            }
            let mut groups: Vec<u32> = Vec::new();
            for &(pid, st) in known.iter() {
                if start_time(pid) != Some(st) {
                    continue;
                }
                if let Some((_, _, g)) = proc_stat(pid) {
                    if g > 1 && g != own_pgrp && g != root && !groups.contains(&g) {
                        groups.push(g);
                    }
                }
                // SAFETY: kill(2) on a pid verified (start time) as a descendant.
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGKILL);
                }
            }
            for g in groups {
                // SAFETY: killpg(2) on a group a verified descendant moved to.
                unsafe {
                    libc::killpg(g as libc::pid_t, libc::SIGKILL);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (root, known);
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

/// Whether `/proc` can be read at all (else nothing can be checked).
#[cfg(target_os = "linux")]
fn proc_usable() -> bool {
    std::fs::metadata("/proc/self/stat").is_ok()
}

/// Whether the registered process `pid` (with start time `start`, 0 =
/// unknown) is still that same, live process. `None`: can't tell.
fn registered_alive(pid: u32, start: u64) -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        if !proc_usable() {
            return None;
        }
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

/// Writes this process's first entry (with any trees already tracked).
fn register_self() -> io::Result<()> {
    write_entry()
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
        let (q, trees) = local_quiescence_counted(LOCAL_QUIET_DEADLINE);
        let pid = std::process::id();
        let body = format!("{g} {}\ntrees {trees}\n", q.word());
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
    /// verified-empty *tracked* trees, and every tree a dead process left
    /// behind (its `proc-<pid>` entry) was killed and verified gone
    /// (`orphans_killed` trees). Only what was tracked is covered (see the
    /// module docs for the double-fork gap).
    Quiescent {
        generation: u64,
        /// Session members (live joined processes, the stopping one
        /// included) that acknowledged with verified-empty tracked trees.
        members: usize,
        /// Tracked command trees those members saw stopped and gone.
        trees: usize,
        orphans_killed: usize,
    },
    /// The barrier is saved (`generation`), but not every process confirmed.
    Incomplete {
        generation: u64,
        /// Live processes that didn't acknowledge in time.
        unconfirmed: Vec<u32>,
        /// Processes that acknowledged with trees still alive (pid, count).
        busy: Vec<(u32, usize)>,
        /// Processes that acknowledged without being able to check.
        unverified: Vec<u32>,
        /// Dead (SIGKILLed / crashed) processes whose recorded command
        /// trees are still alive after being killed (pid, live count).
        orphaned: Vec<(u32, usize)>,
        /// Dead processes whose recorded trees can't be checked (unreadable
        /// or old-format entry, no `/proc`, permission error).
        unchecked: Vec<u32>,
        /// Live processes outside the session (`LILLITH_STOP_SESSION=off`,
        /// debug builds): never told, never checked.
        opted_out: Vec<u32>,
    },
}

impl StopOutcome {
    /// Only `Quiescent` is a confirmed stop.
    pub fn confirmed(&self) -> bool {
        matches!(self, StopOutcome::Quiescent { .. })
    }

    /// For `Quiescent`: exactly what was confirmed (counts of session
    /// members and tracked trees), never more. Empty otherwise.
    pub fn confirmed_detail(&self) -> String {
        let StopOutcome::Quiescent {
            members,
            trees,
            orphans_killed,
            ..
        } = self
        else {
            return String::new();
        };
        let mut l = format!(
            "{trees} tracked command tree(s) confirmed gone by {members} stop-session member(s) (this process included)"
        );
        if *orphans_killed > 0 {
            l.push_str(&format!(
                ", plus {orphans_killed} tree(s) left by a crashed Lillith process killed and checked"
            ));
        }
        l.push_str("; processes Lillith never tracked are not covered.");
        l
    }

    /// One truthful line for the UI.
    pub fn line(&self) -> String {
        match self {
            StopOutcome::Quiescent { .. } => format!("Stopped: {}", self.confirmed_detail()),
            StopOutcome::Incomplete {
                unconfirmed,
                busy,
                unverified,
                orphaned,
                unchecked,
                opted_out,
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
                if !orphaned.is_empty() {
                    let n: usize = orphaned.iter().map(|b| b.1).sum();
                    parts.push(format!(
                        "{n} process(es) left by a crashed Lillith process still alive"
                    ));
                }
                if !unchecked.is_empty() {
                    parts.push(format!(
                        "{} crashed Lillith process(es) whose commands could not be checked",
                        unchecked.len()
                    ));
                }
                if !opted_out.is_empty() {
                    parts.push(format!(
                        "{} Lillith process(es) outside the stop session (LILLITH_STOP_SESSION=off) not stopped or checked",
                        opted_out.len()
                    ));
                }
                format!(
                    "Stop saved but NOT confirmed ({}): treat that work as possibly still running.",
                    parts.join(", ")
                )
            }
            StopOutcome::LocalOnly { reason, local } => format!(
                "Stop applied in this process only ({}); other Lillith windows were NOT told: {reason}.",
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
///
/// A registered process that is dead (or whose pid was reused) is only
/// pruned once the command trees its entry recorded are killed and verified
/// gone; while any of them is alive, or the entry / `/proc` can't be read,
/// the outcome is `Incomplete` (never `Quiescent`) and the entry stays.
pub fn confirm_in(dir: &Path, generation: u64, deadline: Duration) -> StopOutcome {
    confirm_with(dir, generation, deadline, sigkill)
}

fn confirm_with(dir: &Path, generation: u64, deadline: Duration, kill: KillFn) -> StopOutcome {
    let t0 = Instant::now();
    let mut orphans_killed = 0usize;
    loop {
        let mut unconfirmed = Vec::new();
        let mut busy = Vec::new();
        let mut unverified = Vec::new();
        let mut orphaned = Vec::new();
        let mut unchecked = Vec::new();
        let mut ok = 0usize;
        let mut trees = 0usize;
        let outside = opted_out(dir);
        for (pid, start) in registered(dir) {
            if registered_alive(pid, start) == Some(false) {
                // Dead or a reused pid: kill what it left running first.
                match reap_dead_member(dir, pid, kill) {
                    Reap::Gone(n) => {
                        orphans_killed += n;
                        let _ = std::fs::remove_file(dir.join(format!("proc-{pid}")));
                        let _ = std::fs::remove_file(dir.join(format!("ack-{pid}")));
                    }
                    Reap::Alive(n) => orphaned.push((pid, n)),
                    Reap::Unchecked => unchecked.push(pid),
                }
                continue;
            }
            match read_ack(dir, pid) {
                Some((g, q, n)) if g >= generation => match q {
                    Quiet::Quiet => {
                        ok += 1;
                        trees += n;
                    }
                    Quiet::Unverified => unverified.push(pid),
                    Quiet::Busy(n) => busy.push((pid, n)),
                },
                _ => unconfirmed.push(pid),
            }
        }
        let clean = unconfirmed.is_empty()
            && busy.is_empty()
            && unverified.is_empty()
            && orphaned.is_empty()
            && unchecked.is_empty()
            && outside.is_empty();
        if clean {
            return StopOutcome::Quiescent {
                generation,
                members: ok,
                trees,
                orphans_killed,
            };
        }
        if t0.elapsed() >= deadline {
            unconfirmed.sort_unstable();
            return StopOutcome::Incomplete {
                generation,
                unconfirmed,
                busy,
                unverified,
                orphaned,
                unchecked,
                opted_out: outside,
            };
        }
        std::thread::sleep(WATCH_EVERY);
    }
}

// ------------------------------------------------------------ dead members

/// Sends SIGKILL to a pid (`group == false`) or a process group.
type KillFn = fn(target: u32, group: bool);

fn sigkill(target: u32, group: bool) {
    #[cfg(unix)]
    {
        if target <= 1 {
            return;
        }
        // SAFETY: kill(2)/killpg(2) on a target the caller verified (start
        // time / group identity) as one of a dead member's command trees.
        unsafe {
            if group {
                libc::killpg(target as libc::pid_t, libc::SIGKILL);
            } else {
                libc::kill(target as libc::pid_t, libc::SIGKILL);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (target, group);
    }
}

/// What killing a dead member's recorded trees achieved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reap {
    /// Everything recorded is verifiably gone (this many trees were listed).
    Gone(usize),
    /// This many recorded processes are still alive after the kill.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Alive(usize),
    /// The entry or `/proc` could not be read: nothing can be claimed.
    Unchecked,
}

/// Kills (SIGKILL: group, recorded descendants, their descendants and the
/// groups they moved to) everything the dead member `pid` recorded in its
/// entry, then checks it is gone.
fn reap_dead_member(dir: &Path, pid: u32, kill: KillFn) -> Reap {
    let Ok(body) = std::fs::read_to_string(dir.join(format!("proc-{pid}"))) else {
        return Reap::Unchecked;
    };
    let Some(entry) = parse_entry(&body) else {
        return Reap::Unchecked;
    };
    reap_entry(&entry, kill)
}

fn reap_entry(entry: &Entry, kill: KillFn) -> Reap {
    if entry.groups.is_empty() && entry.pids.is_empty() {
        return Reap::Gone(0);
    }
    #[cfg(target_os = "linux")]
    {
        if !proc_usable() {
            return Reap::Unchecked;
        }
        // SAFETY: getpgrp has no preconditions.
        let own_pgrp = unsafe { libc::getpgrp() } as u32;
        let me = std::process::id();
        let mut known: Vec<(u32, u64)> = entry.pids.clone();
        let mut found: Option<usize> = None;
        for _ in 0..3 {
            let Some(mut groups) = our_groups(&entry.groups) else {
                return Reap::Unchecked;
            };
            groups.retain(|&g| g != own_pgrp);
            // Expand: members of our groups and the descendants of every
            // live recorded process (orphans were re-parented, so a walk
            // from the dead member finds nothing).
            let mut roots: Vec<(u32, u64)> = Vec::new();
            let mut live_groups = 0usize;
            for &g in &groups {
                match group_members(g) {
                    Some(m) => {
                        live_groups += usize::from(!m.is_empty());
                        roots.extend(m);
                    }
                    None => return Reap::Unchecked,
                }
            }
            let live_pids: Vec<(u32, u64)> = known
                .iter()
                .copied()
                .filter(|&(p, st)| registered_alive(p, st) == Some(true))
                .collect();
            if found.is_none() {
                // How many recorded trees were still running.
                found = Some(if live_groups == 0 && !live_pids.is_empty() {
                    1
                } else {
                    live_groups
                });
            }
            roots.extend(live_pids);
            for &(r, _) in &roots {
                for d in descendants(r) {
                    if !known.contains(&d) {
                        known.push(d);
                    }
                }
            }
            for r in roots {
                if !known.contains(&r) {
                    known.push(r);
                }
            }
            let mut kill_groups = groups.clone();
            for &(p, st) in &known {
                if p == me || start_time(p) != Some(st) {
                    continue; // gone, or a reused pid: never touched
                }
                if let Some((_, _, g)) = proc_stat(p) {
                    if !kill_groups.contains(&g) {
                        kill_groups.push(g);
                    }
                }
                kill(p, false);
            }
            for g in kill_groups {
                if g > 1 && g != own_pgrp {
                    kill(g, true);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // Check (briefly) that it is all gone.
        let t0 = Instant::now();
        loop {
            let Some(groups) = our_groups(&entry.groups) else {
                return Reap::Unchecked;
            };
            // Distinct live processes (a group member may also be recorded).
            let mut alive: Vec<u32> = Vec::new();
            for g in groups {
                match group_members(g) {
                    Some(m) => alive.extend(m.into_iter().map(|(p, _)| p)),
                    None => return Reap::Unchecked,
                }
            }
            for &(p, st) in &known {
                match registered_alive(p, st) {
                    Some(true) => alive.push(p),
                    Some(false) => {}
                    None => return Reap::Unchecked,
                }
            }
            alive.sort_unstable();
            alive.dedup();
            let alive = alive.len();
            if alive == 0 {
                return Reap::Gone(found.unwrap_or(0));
            }
            if t0.elapsed() >= Duration::from_millis(500) {
                return Reap::Alive(alive);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = kill;
        Reap::Unchecked
    }
}

/// The recorded groups that are still ours. Pid reuse: Linux never hands
/// out a pid that is still some group's id, so a group with live members
/// whose leader pid is gone is still the recorded group; a live process
/// with the leader's pid but another start time means the group died and
/// the pid was reused (skipped). An unknown leader start time (0) with a
/// live leader can't be told apart: `None` (unchecked).
#[cfg(target_os = "linux")]
fn our_groups(groups: &[(u32, u64)]) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    for &(g, leader_start) in groups {
        if g <= 1 {
            continue;
        }
        if proc_stat(g).is_none() {
            // Leader gone: any live member is still the recorded group.
            out.push(g);
            continue;
        }
        let now = start_time(g)?;
        if now == leader_start {
            out.push(g);
        } else if leader_start == 0 {
            return None;
        }
        // Else: the pid was reused by an unrelated process; never touched.
    }
    Some(out)
}

/// Live (non-zombie) members of group `pgid` with their start times.
/// `None` when `/proc` can't be read.
#[cfg(target_os = "linux")]
fn group_members(pgid: u32) -> Option<Vec<(u32, u64)>> {
    let rd = std::fs::read_dir("/proc").ok()?;
    let mut out = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        if let Some((state, _ppid, pgrp)) = proc_stat(pid) {
            if pgrp == pgid && state != 'Z' && state != 'X' {
                if let Some(st) = start_time(pid) {
                    out.push((pid, st));
                }
            }
        }
    }
    Some(out)
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

/// `(generation, state, tracked trees confirmed gone)` from `ack-<pid>`.
fn read_ack(dir: &Path, pid: u32) -> Option<(u64, Quiet, usize)> {
    let s = std::fs::read_to_string(dir.join(format!("ack-{pid}"))).ok()?;
    let mut lines = s.lines();
    let (g, q) = lines.next()?.trim().split_once(' ')?;
    let trees = lines
        .find_map(|l| l.trim().strip_prefix("trees "))
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or(0);
    Some((g.parse().ok()?, Quiet::parse(q)?, trees))
}

/// Live processes that left an `optout-<pid>` marker (outside the session;
/// can't be told to stop or checked). Dead ones are pruned; a marker whose
/// process can't be checked counts as live (fail closed).
fn opted_out(dir: &Path) -> Vec<u32> {
    let mut v = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return v;
    };
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix("optout-"))
            .and_then(|p| p.parse::<u32>().ok())
        else {
            continue;
        };
        let start = std::fs::read_to_string(e.path())
            .ok()
            .and_then(|s| s.split_whitespace().nth(1)?.parse().ok())
            .unwrap_or(0);
        match registered_alive(pid, start) {
            Some(false) => {
                let _ = std::fs::remove_file(e.path());
            }
            _ => v.push(pid),
        }
    }
    v.sort_unstable();
    v
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

    #[test]
    fn only_a_scratch_runtime_tree_counts_as_scratch() {
        let tmp = Path::new("/tmp");
        // QA-style per-run tree: runtime and state share a private root.
        assert!(scratch_runtime(
            Path::new("/tmp/run-1/xdg/runtime"),
            Path::new("/tmp/run-1/xdg/state"),
            tmp
        ));
        // A test that set only HOME / XDG_*_HOME inherits the live runtime.
        assert!(!scratch_runtime(
            Path::new("/tmp/xdg-runtime-box-31"),
            Path::new("/tmp/setup-e2e/home/.local/state"),
            tmp
        ));
        assert!(!scratch_runtime(
            Path::new("/run/user/1000"),
            Path::new("/run/user/state"),
            tmp
        ));
        assert!(!scratch_runtime(
            Path::new("/tmp/a/runtime"),
            Path::new("/tmp/b/state"),
            tmp
        ));
    }

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
            members: 2,
            trees: 3,
            orphans_killed: 0,
        };
        assert!(q.confirmed());
        assert!(q.line().starts_with("Stopped"));
        assert!(
            q.line().contains("tracked"),
            "only tracked trees are claimed: {}",
            q.line()
        );
        assert!(!q.line().contains("every"), "{}", q.line());
        assert!(
            q.line().contains("3 tracked command tree(s)")
                && q.line().contains("2 stop-session member(s)"),
            "{}",
            q.line()
        );
        let i = StopOutcome::Incomplete {
            generation: 9,
            unconfirmed: vec![42],
            busy: vec![],
            unverified: vec![],
            orphaned: vec![],
            unchecked: vec![],
            opted_out: vec![],
        };
        assert!(!i.confirmed());
        assert!(i.line().contains("NOT confirmed"));
        assert!(!i.line().starts_with("Stopped"));
        for (orphaned, unchecked) in [(vec![(7, 2)], vec![]), (vec![], vec![7])] {
            let o = StopOutcome::Incomplete {
                generation: 9,
                unconfirmed: vec![],
                busy: vec![],
                unverified: vec![],
                orphaned,
                unchecked,
                opted_out: vec![],
            };
            assert!(!o.confirmed());
            assert!(o.line().contains("NOT confirmed") && o.line().contains("crashed"));
        }
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
        std::fs::write(d.join(format!("proc-{me}")), render_entry(me, st, &[])).unwrap();
        // A dead pid (> pid_max) whose entry recorded no running trees.
        std::fs::write(d.join("proc-4194399"), render_entry(4194399, 1, &[])).unwrap();
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

    #[test]
    fn entries_round_trip_and_anything_else_is_uncheckable() {
        let t = Tree {
            id: 1,
            pgid: 100,
            leader_start: 7,
            epoch: 0,
            pids: vec![(101, 8), (102, 9)],
        };
        let body = render_entry(5, 6, &[&t]);
        assert_eq!(
            parse_entry(&body),
            Some(Entry {
                groups: vec![(100, 7)],
                pids: vec![(101, 8), (102, 9)],
            })
        );
        // Old format (no trees recorded), truncated, garbage: uncheckable.
        assert_eq!(parse_entry("5 6\n"), None);
        assert_eq!(parse_entry(&body.replace("end\n", "")), None);
        assert_eq!(parse_entry("5 6\nv2\ng x 1\nend\n"), None);
        assert_eq!(parse_entry(""), None);
    }

    /// Never touches anything (stands in for a kill that fails, e.g. EPERM).
    fn no_kill(_: u32, _: bool) {}

    #[cfg(target_os = "linux")]
    fn spawn_group_sleep() -> std::process::Child {
        use std::os::unix::process::CommandExt;
        crate::proc::command("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap()
    }

    #[cfg(target_os = "linux")]
    fn wait_gone(c: &mut std::process::Child) -> bool {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(5) {
            if c.try_wait().unwrap().is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// A dead member's recorded tree that is still alive: killed when the
    /// kill works (then pruned, Quiescent), and never Quiescent when it
    /// doesn't (the entry stays).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_dead_members_live_tree_is_killed_before_pruning_or_reported() {
        let d =
            std::env::temp_dir().join(format!("lillith-stopsess-orphan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        private_dir(&d).unwrap();
        let mut orphan = spawn_group_sleep();
        let op = orphan.id();
        let ost = start_time(op).unwrap();
        let tree = Tree {
            id: 1,
            pgid: op,
            leader_start: ost,
            epoch: 0,
            pids: vec![(op, ost)],
        };
        let entry = d.join("proc-4194398");
        std::fs::write(&entry, render_entry(4194398, 1, &[&tree])).unwrap();

        // The kill fails: still alive, so Incomplete and the entry stays.
        match confirm_with(&d, 1, Duration::from_millis(300), no_kill) {
            StopOutcome::Incomplete {
                orphaned,
                unchecked,
                ..
            } => {
                assert_eq!(orphaned, vec![(4194398, 1)]);
                assert!(unchecked.is_empty());
            }
            o => panic!("a live orphan tree is never Quiescent: {o:?}"),
        }
        assert!(entry.exists(), "entry kept while its tree lives");
        assert_eq!(orphan.try_wait().unwrap(), None, "no_kill touched nothing");

        // The real kill: the tree dies, then the entry is pruned.
        match confirm_in(&d, 1, Duration::from_secs(3)) {
            StopOutcome::Quiescent { orphans_killed, .. } => assert_eq!(orphans_killed, 1),
            o => panic!("killed and verified gone: {o:?}"),
        }
        assert!(wait_gone(&mut orphan), "orphan sleep was killed");
        assert!(!entry.exists(), "pruned only after the tree was gone");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// An entry whose trees can't be checked is never treated as stopped.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_uncheckable_dead_member_is_never_quiescent() {
        let d = std::env::temp_dir().join(format!("lillith-stopsess-unchk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        private_dir(&d).unwrap();
        // Old format (records no trees) and a truncated v2 entry.
        std::fs::write(d.join("proc-4194397"), "4194397 1\n").unwrap();
        std::fs::write(d.join("proc-4194396"), "4194396 1\nv2\ng 4194395 3\n").unwrap();
        match confirm_in(&d, 1, Duration::from_millis(200)) {
            StopOutcome::Incomplete { unchecked, .. } => {
                assert_eq!(unchecked, vec![4194396, 4194397]);
            }
            o => panic!("uncheckable is never Quiescent: {o:?}"),
        }
        assert!(d.join("proc-4194397").exists() && d.join("proc-4194396").exists());
        // An entry that can't be read at all (a directory in its place).
        let _ = std::fs::remove_file(d.join("proc-4194397"));
        let _ = std::fs::remove_file(d.join("proc-4194396"));
        std::fs::create_dir(d.join("proc-4194394")).unwrap();
        assert!(matches!(
            confirm_in(&d, 1, Duration::from_millis(100)),
            StopOutcome::Incomplete { ref unchecked, .. } if unchecked == &vec![4194394]
        ));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Pid reuse: a recorded group / pid whose number now belongs to a
    /// process with another start time is never signalled.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_reused_pid_is_never_killed() {
        let d = std::env::temp_dir().join(format!("lillith-stopsess-reuse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        private_dir(&d).unwrap();
        let mut stranger = spawn_group_sleep();
        let sp = stranger.id();
        let real = start_time(sp).unwrap();
        // Recorded with another start time: "our" process died, the pid was
        // reused by `stranger`.
        let tree = Tree {
            id: 1,
            pgid: sp,
            leader_start: real + 1,
            epoch: 0,
            pids: vec![(sp, real + 1)],
        };
        std::fs::write(d.join("proc-4194393"), render_entry(4194393, 1, &[&tree])).unwrap();
        match confirm_in(&d, 1, Duration::from_millis(300)) {
            StopOutcome::Quiescent { orphans_killed, .. } => assert_eq!(orphans_killed, 0),
            o => panic!("our tree is gone (only the pid was reused): {o:?}"),
        }
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(stranger.try_wait().unwrap(), None, "reused pid untouched");
        let _ = stranger.kill();
        let _ = stranger.wait();
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A live process outside the session (LILLITH_STOP_SESSION=off) is
    /// never covered by a confirmed stop; a dead one's marker is pruned.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_opted_out_live_process_is_never_quiescent() {
        let d =
            std::env::temp_dir().join(format!("lillith-stopsess-optout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        private_dir(&d).unwrap();
        let me = std::process::id();
        let st = start_time(me).unwrap();
        std::fs::write(d.join(format!("optout-{me}")), format!("{me} {st}\n")).unwrap();
        std::fs::write(d.join("optout-4194390"), "4194390 1\n").unwrap();
        match confirm_in(&d, 1, Duration::from_millis(150)) {
            StopOutcome::Incomplete { ref opted_out, .. } => assert_eq!(opted_out, &vec![me]),
            o => panic!("an opted-out live process is never Quiescent: {o:?}"),
        }
        assert!(!d.join("optout-4194390").exists(), "dead marker pruned");
        let line = confirm_in(&d, 1, Duration::from_millis(50)).line();
        assert!(
            line.contains("LILLITH_STOP_SESSION=off") && line.contains("NOT confirmed"),
            "{line}"
        );
        std::fs::remove_file(d.join(format!("optout-{me}"))).unwrap();
        assert!(confirm_in(&d, 1, Duration::from_millis(50)).confirmed());
        let _ = std::fs::remove_dir_all(&d);
    }
}
