//! Independent QA: cross-process stop, end to end, with real OS processes.
//!
//! `docs/BOTS.md:30-31` says Stop, Stop all, the kill switch and Ctrl-C /
//! SIGTERM on `lillith-settings` stop bots "and any command they are
//! running". `kill_switch.rs:1` says the switches are "two process-wide
//! epochs" (plain statics). This file checks what happens when the stop comes
//! from a *different* process than the one running the command.
//!
//! The test binary re-runs itself with `LILLITH_QA_XSTOP_ROLE` set, so every
//! role is a separate OS process:
//!
//! - `worker` stands in for the overlay / `lillith chat`. It runs a synthetic
//!   job through the real `commands::run_child` (what `run_captured` and the
//!   Soul Contract use). The job is an `sh` child that starts a `sh`
//!   grandchild. Both append heartbeats to files in a fresh temp dir. The
//!   worker also polls trigger files so a test can fire `stop_all` / `hush`
//!   *inside* the worker (control cases).
//! - `settings-stop-all` makes exactly the call the Settings "Stop all" button
//!   makes (`tabs/bots.rs:505-506`), the per-bot Stop with a running step makes
//!   (`tabs/bots.rs:237-239`) and the Ctrl-C/SIGTERM watcher makes
//!   (`bots/mod.rs:1029-1031`): `kill_switch::stop_all()`. Then it writes the
//!   `StopReport` and the chat line it would show, and exits.
//! - `settings-hush` makes the safeword call (`kill_switch::hush()`).
//! - `observer` is a fresh process started after the stop. It reads the
//!   kill-switch state and tries one synthetic dispatch, which checks whether
//!   the barrier is durable and whether dispatch is disarmed.
//!
//! Ctrl-C routing (from code at 1fcac410; not executed here):
//! - `lillith-settings`: `bin/lillith-settings.rs:71-73` installs
//!   `bots::install_ctrlc_stop` (`bots/mod.rs:1015-1037`). SIGINT/SIGTERM sets
//!   an atomic flag, and a watcher thread calls `kill_switch::stop_all()` *in
//!   the Settings process* and then exits 130. No IPC is involved.
//! - overlay / `lillith chat`: `apps/lillith/src/chat.rs:401-412`
//!   (`stop_on_signal`) and `chat.rs:1174-1177` (`console_stop`) call
//!   `stop_all()` in the app process. `main.rs:648-654` (`lillith dream`) does
//!   the same.
//! - Command children run in their own process group (`commands.rs:533-537`),
//!   so a terminal Ctrl-C never reaches them directly.
//! - No cross-process route exists. The code has no socket, pipe or file for
//!   the stop epoch. Ctrl-C on Settings therefore has the same cross-process
//!   effect as the `settings-stop-all` role. Sending a real SIGINT to the
//!   GUI binary is NOT exercised here.
//!
//! Only PIDs this test spawned are signalled. Before SIGKILL, each one is
//! checked against `/proc/<pid>/cmdline`, which must contain this run's unique
//! temp dir. Nothing broader is ever signalled.
//!
//! API assumptions (main 1fcac410). To retarget at Engineering's
//! `feat/swarm-cross-process-stop`, change only these seams:
//! - stop call: `role "settings-stop-all"` calls `kill_switch::stop_all()`
//!   and reads `StopReport { epoch, helper }` plus `chat_line()`. Swap in the
//!   new authenticated Settings→supervisor client call. Write its
//!   acknowledgement state (e.g. Accepted / Quiescent / Incomplete /
//!   Unknown) into `settings.out`, and make check (4) fail if anything other
//!   than a verified-quiescent state is reported as "stopped".
//! - durable barrier: the `observer` role reads `kill_switch::epoch()` /
//!   `stopped_since(0)` in a fresh process. Point it at the new persisted
//!   barrier read API (or the supervisor's state query).
//! - dispatch: the `observer` dispatches through `commands::run_child`. Point
//!   it at the supervisor's submit path; expect refusal while disarmed.
//! - worker: runs the job via `commands::run_child` in-process. If the new
//!   design moves spawning into a supervisor, submit the same heartbeat
//!   script as a registered job instead. The tree/heartbeat/PID checks are
//!   implementation-agnostic.
//! - hush: `kill_switch::hush()` / `hush_epoch()` / `is_safeword()`.
//!
//! Run the P0 (expected FAIL on main 1fcac410):
//!   cargo test -p lillith-core --test cross_process_stop -- --ignored --nocapture --test-threads=1
#![cfg(target_os = "linux")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use lillith_core::access::{commands, kill_switch};

const ROLE: &str = "LILLITH_QA_XSTOP_ROLE";
const DIR: &str = "LILLITH_QA_XSTOP_DIR";
const ESCAPE: &str = "LILLITH_QA_XSTOP_ESCAPE";
/// How long a stop gets to take effect before we call it failed.
const DEADLINE: Duration = Duration::from_secs(5);

// ------------------------------------------------------------------ roles

/// The role dispatcher. Without the role env var (a normal `cargo test`) it
/// does nothing.
#[test]
fn qa_xstop_role_entry() {
    let Ok(role) = std::env::var(ROLE) else {
        return;
    };
    let dir = PathBuf::from(std::env::var(DIR).expect("role needs dir"));
    match role.as_str() {
        "worker" => role_worker(&dir),
        "settings-stop-all" => {
            // Exactly what Settings does (tabs/bots.rs:506, bots/mod.rs:1031).
            let r = kill_switch::stop_all();
            fs::write(
                dir.join("settings.out"),
                format!(
                    "pid={}\nepoch={}\nhelper={:?}\nchat_line={}\n",
                    std::process::id(),
                    r.epoch,
                    r.helper,
                    r.chat_line()
                ),
            )
            .unwrap();
        }
        "settings-hush" => {
            assert!(kill_switch::is_safeword("red"));
            assert!(kill_switch::is_safeword("OOC: stop"));
            let e = kill_switch::hush();
            fs::write(
                dir.join("settings-hush.out"),
                format!("hush_epoch={e}\nepoch={}\n", kill_switch::epoch()),
            )
            .unwrap();
        }
        "observer" => {
            let epoch = kill_switch::epoch();
            // Synthetic dispatch after a global stop: should be refused or
            // stopped if dispatch is disarmed.
            let marker = dir.join("post_stop_dispatch_ran");
            let mut c = Command::new("sh");
            c.arg("-c").arg(format!("echo RAN > '{}'", marker.display()));
            let out = commands::run_child(c, Duration::from_secs(5), 4096).unwrap_or_default();
            fs::write(
                dir.join("observer.out"),
                format!(
                    "epoch={epoch}\nstopped_since_0={}\ndispatch_ran={}\ndispatch_out={}\n",
                    kill_switch::stopped_since(0),
                    marker.exists(),
                    out.trim().replace('\n', " | ")
                ),
            )
            .unwrap();
        }
        other => panic!("unknown role {other}"),
    }
}

fn role_worker(dir: &Path) {
    fs::write(dir.join("worker.pid"), std::process::id().to_string()).unwrap();
    // Trigger watcher: control cases fire the stop/hush *inside* this process.
    {
        let dir = dir.to_path_buf();
        std::thread::spawn(move || {
            let (mut stopped, mut hushed) = (false, false);
            loop {
                if !stopped && dir.join("trigger-inproc-stop").exists() {
                    stopped = true;
                    kill_switch::stop_all();
                }
                if !hushed && dir.join("trigger-inproc-hush").exists() {
                    hushed = true;
                    kill_switch::hush();
                }
                let _ = fs::write(
                    dir.join("worker.epoch"),
                    format!(
                        "epoch={}\nhush_epoch={}\n",
                        kill_switch::epoch(),
                        kill_switch::hush_epoch()
                    ),
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        });
    }
    let d = dir.display().to_string();
    let beat = |name: &str| {
        format!("sh -c 'echo $$ > \"{d}/{name}.pid\"; while :; do echo b >> \"{d}/hb.{name}\"; sleep 0.1; done'")
    };
    let escape = if std::env::var(ESCAPE).is_ok() {
        // A grandchild that leaves the process group (setsid).
        format!("setsid {} &", beat("esc"))
    } else {
        String::new()
    };
    let script = format!(
        "echo $$ > \"{d}/child.pid\"; {gc} & {escape} while :; do echo b >> \"{d}/hb.child\"; sleep 0.1; done",
        gc = beat("gc"),
    );
    let mut c = Command::new("sh");
    c.arg("-c").arg(script);
    let t0 = Instant::now();
    let out = commands::run_child(c, Duration::from_secs(60), 64 * 1024)
        .unwrap_or_else(|e| format!("run_child error: {e}"));
    fs::write(
        dir.join("worker.result"),
        format!(
            "elapsed_ms={}\nwas_stopped={}\n{out}\n",
            t0.elapsed().as_millis(),
            commands::was_stopped(&out)
        ),
    )
    .unwrap();
}

// ---------------------------------------------------------------- harness

struct Run {
    dir: PathBuf,
    worker: Option<Child>,
}

impl Run {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "lillith-qa-xstop-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        Run { dir, worker: None }
    }

    fn role(&self, role: &str) -> Command {
        let mut c = Command::new(std::env::current_exe().unwrap());
        c.args([
            "qa_xstop_role_entry",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ROLE, role)
        .env(DIR, &self.dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        c
    }

    /// A separate process that runs `role` to completion.
    fn run_role(&self, role: &str) {
        let st = self.role(role).status().unwrap();
        assert!(st.success(), "role {role} failed: {st}");
    }

    fn start_worker(&mut self, escape: bool) {
        let mut c = self.role("worker");
        if escape {
            c.env(ESCAPE, "1");
        }
        self.worker = Some(c.spawn().unwrap());
        let mut need = vec!["child.pid", "gc.pid", "hb.child", "hb.gc"];
        if escape {
            need.extend(["esc.pid", "hb.esc"]);
        }
        let t = Instant::now();
        while !need.iter().all(|f| self.dir.join(f).exists()) {
            assert!(t.elapsed() < Duration::from_secs(20), "worker tree did not start");
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(300));
    }

    fn read(&self, f: &str) -> String {
        fs::read_to_string(self.dir.join(f)).unwrap_or_default()
    }

    fn pid(&self, f: &str) -> Option<i32> {
        self.read(f).trim().parse().ok()
    }

    fn beats(&self, name: &str) -> u64 {
        fs::metadata(self.dir.join(format!("hb.{name}")))
            .map(|m| m.len())
            .unwrap_or(0)
    }

    /// Whether heartbeat file `name` grows over `window`.
    fn beating(&self, name: &str, window: Duration) -> bool {
        let a = self.beats(name);
        std::thread::sleep(window);
        self.beats(name) > a
    }

    /// Alive = exists and is not a zombie.
    fn alive(pid: i32) -> bool {
        match fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(s) => s
                .rsplit_once(')')
                .map(|(_, rest)| !rest.trim_start().starts_with('Z'))
                .unwrap_or(true),
            Err(_) => false,
        }
    }

    /// Waits up to `d` for every tracked tree PID to be gone and every
    /// heartbeat to stop. Returns (dead, quiet).
    fn wait_dead(&self, names: &[&str], d: Duration) -> (bool, bool) {
        let t = Instant::now();
        loop {
            let dead = names
                .iter()
                .all(|n| self.pid(&format!("{n}.pid")).map_or(true, |p| !Self::alive(p)));
            if dead || t.elapsed() >= d {
                let quiet = names
                    .iter()
                    .all(|n| !self.beating(n, Duration::from_millis(400)));
                return (dead, quiet);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn wait_file(&self, f: &str, d: Duration) -> bool {
        let t = Instant::now();
        while t.elapsed() < d {
            if self.dir.join(f).exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    fn evidence(&self) -> String {
        let mut s = String::new();
        for f in [
            "settings.out",
            "settings-hush.out",
            "worker.epoch",
            "observer.out",
            "worker.result",
        ] {
            let t = self.read(f);
            if !t.is_empty() {
                s.push_str(&format!("--- {f}\n{}\n", t.trim_end()));
            }
        }
        s
    }
}

impl Drop for Run {
    /// Kills only what this run spawned: the tracked tree PIDs, each checked
    /// against its cmdline (it must contain our unique temp dir), then the
    /// worker `Child` we own.
    fn drop(&mut self) {
        let marker = self.dir.display().to_string();
        for f in ["esc.pid", "gc.pid", "child.pid"] {
            if let Some(p) = self.pid(f) {
                let cmd = fs::read(format!("/proc/{p}/cmdline")).unwrap_or_default();
                if Run::alive(p) && String::from_utf8_lossy(&cmd).contains(&marker) {
                    // SAFETY: plain kill(2) on a PID verified as ours.
                    unsafe {
                        libc::kill(p, libc::SIGKILL);
                    }
                }
            }
        }
        if let Some(mut w) = self.worker.take() {
            let _ = w.kill();
            let _ = w.wait();
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

// ------------------------------------------------------------------ tests

/// P0 contract: a stop fired by a *separate* Settings process (Stop / Stop
/// all / kill switch; Ctrl-C ends in the same call) stops the worker's job.
#[test]
#[ignore = "P0 cross-process stop not implemented: kill_switch.rs:1 epochs are process-local; docs/BOTS.md:30 claims Settings stops running commands. Run: cargo test -p lillith-core --test cross_process_stop -- --ignored --nocapture --test-threads=1"]
fn p0_settings_stop_all_from_separate_process_stops_worker_tree() {
    let mut r = Run::new("p0");
    r.start_worker(false);
    assert!(r.beating("child", Duration::from_millis(400)));
    assert!(r.beating("gc", Duration::from_millis(400)));

    let t_stop = Instant::now();
    r.run_role("settings-stop-all");
    let settings = r.read("settings.out");
    let claimed_stopped = settings.contains("chat_line=Stopped.");

    let (dead, quiet) = r.wait_dead(&["child", "gc"], DEADLINE);
    let waited = t_stop.elapsed();
    // Fresh process after the Settings process exited.
    r.run_role("observer");
    let obs = r.read("observer.out");

    let mut fails = Vec::new();
    // (1) durable barrier: a fresh process must see the stop.
    if obs.contains("epoch=0\n") || obs.contains("stopped_since_0=false") {
        fails.push("(1) no durable cancellation barrier: a fresh process sees kill_switch epoch 0 after Settings' stop_all returned");
    }
    // (2) dispatch disarmed after a global stop.
    if obs.contains("dispatch_ran=true") {
        fails.push("(2) dispatch not disarmed: a synthetic command dispatched after the global stop ran to completion");
    }
    // (3) the whole child tree dies.
    if !dead {
        fails.push("(3a) worker child/grandchild PIDs still alive after the deadline");
    }
    if !quiet {
        fails.push("(3b) worker heartbeats still growing after the deadline");
    }
    if r.read("worker.result").contains("was_stopped=true") == false && dead {
        fails.push("(3c) tree died but worker result is not marked stopped");
    }
    // (4) no stop claim without a dead tree.
    if claimed_stopped && !(dead && quiet) {
        fails.push("(4) Settings reported 'Stopped. Every running command is dead.' while the worker tree was still running");
    }
    let ev = r.evidence();
    drop(r);
    assert!(
        fails.is_empty(),
        "cross-process stop FAILED after {waited:?} (deadline {DEADLINE:?}):\n  {}\n{ev}",
        fails.join("\n  ")
    );
}

/// Control (expected PASS on main): the same worker tree dies when
/// `stop_all` fires inside the worker's own process. This shows the harness
/// can see a working stop, so a P0 failure is not a harness artifact.
#[test]
fn control_in_process_stop_all_kills_worker_tree() {
    let mut r = Run::new("ctl");
    r.start_worker(false);
    assert!(r.beating("gc", Duration::from_millis(400)));
    fs::write(r.dir.join("trigger-inproc-stop"), "").unwrap();
    let (dead, quiet) = r.wait_dead(&["child", "gc"], DEADLINE);
    let have_result = r.wait_file("worker.result", DEADLINE);
    let res = r.read("worker.result");
    let ev = r.evidence();
    drop(r);
    assert!(dead && quiet && have_result, "in-process stop: dead={dead} quiet={quiet}\n{ev}");
    assert!(res.contains("was_stopped=true"), "{ev}");
}

/// Safeword contract (expected PASS on main): "red" / OOC stop, both from a
/// separate Settings process and inside the worker, only hushes. The task
/// tree keeps beating, the stop epoch does not move, and the job is not
/// marked stopped.
#[test]
fn red_hush_only_hushes_worker_tree_keeps_running() {
    let mut r = Run::new("hush");
    r.start_worker(false);
    r.run_role("settings-hush");
    fs::write(r.dir.join("trigger-inproc-hush"), "").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let we = r.read("worker.epoch");
    let child_beats = r.beating("child", Duration::from_secs(1));
    let gc_beats = r.beating("gc", Duration::from_millis(500));
    let alive = ["child", "gc"]
        .iter()
        .all(|n| r.pid(&format!("{n}.pid")).is_some_and(Run::alive));
    let no_result = !r.dir.join("worker.result").exists();
    let ev = r.evidence();
    drop(r);
    assert!(
        we.contains("epoch=0\n") && we.contains("hush_epoch=1"),
        "worker: hush must move hush_epoch only\n{ev}"
    );
    assert!(
        child_beats && gc_beats && alive && no_result,
        "hush killed or ended the task: child_beats={child_beats} gc_beats={gc_beats} alive={alive} no_result={no_result}\n{ev}"
    );
}

/// Containment gap (expected FAIL on main): a grandchild that leaves the
/// process group with `setsid` survives an in-process stop_all, because
/// `kill_tree` only signals the pgid (`commands.rs:632-650`). It also keeps the
/// stdout pipe open, so `run_child` cannot return a stopped result.
#[test]
#[ignore = "containment gap: kill_tree is process-group only; setsid descendant escapes. Run: cargo test -p lillith-core --test cross_process_stop -- --ignored --nocapture --test-threads=1"]
fn gap_in_process_stop_all_kills_setsid_escaped_descendant() {
    let mut r = Run::new("esc");
    r.start_worker(true);
    assert!(r.beating("esc", Duration::from_millis(400)));
    fs::write(r.dir.join("trigger-inproc-stop"), "").unwrap();
    let (dead, quiet) = r.wait_dead(&["child", "gc", "esc"], DEADLINE);
    let have_result = r.wait_file("worker.result", Duration::from_secs(1));
    let esc_alive = r.pid("esc.pid").is_some_and(Run::alive);
    let ev = r.evidence();
    drop(r);
    assert!(
        dead && quiet && have_result,
        "setsid descendant escaped stop_all: all_dead={dead} quiet={quiet} esc_alive={esc_alive} worker_returned={have_result}\n{ev}"
    );
}
