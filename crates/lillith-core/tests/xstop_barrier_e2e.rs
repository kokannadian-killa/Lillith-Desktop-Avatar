//! End to end: a Stop fired in one process (standing in for the detached
//! `lillith-settings` window: the exact calls its "Stop all" button and its
//! Ctrl-C / SIGTERM watcher make) stops a command tree running in another
//! process (standing in for the overlay / `lillith chat`), through the
//! per-user stop barrier (`access::stop_session`).
//!
//! The test binary re-runs itself with `XSTOP_E2E_ROLE` set, so every role
//! is a separate OS process. Everything lives in a fresh temp dir; no real
//! runtime dir, config or user data is touched, and only processes this test
//! started are signalled. No network.
//!
//! Run: cargo test -p lillith-core --test xstop_barrier_e2e -- --nocapture --test-threads=1
#![cfg(target_os = "linux")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant, SystemTime};

use lillith_core::access::{commands, kill_switch, stop_session};

const ROLE: &str = "XSTOP_E2E_ROLE";
const DIR: &str = "XSTOP_E2E_DIR";

fn session(d: &Path) -> PathBuf {
    d.join("session")
}

fn wait_for(what: &str, limit: Duration, mut f: impl FnMut() -> bool) {
    let t0 = Instant::now();
    while !f() {
        assert!(t0.elapsed() < limit, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Role dispatcher; does nothing in a normal `cargo test` run.
#[test]
fn xstop_role() {
    let Ok(role) = std::env::var(ROLE) else {
        return;
    };
    let d = PathBuf::from(std::env::var(DIR).expect("dir"));
    kill_switch::join_session_at(&session(&d)).expect("join");
    match role.as_str() {
        // The overlay side: one long synthetic job (sh + two looping
        // grandchildren writing heartbeats) through the real command runner.
        "worker" => {
            let dd = d.clone();
            std::thread::spawn(move || {
                // Control: the safeword inside this process.
                wait_for("do-hush", Duration::from_secs(60), || {
                    dd.join("do-hush").exists()
                });
                kill_switch::hush();
                fs::write(dd.join("hushed"), "1").unwrap();
            });
            let cmd = format!(
                "echo $$ > '{0}/pgid'; \
                 (while :; do date +%s%N > '{0}/hb1'; sleep 0.05; done) & \
                 (while :; do date +%s%N > '{0}/hb2'; sleep 0.05; done) & \
                 wait",
                d.display()
            );
            let out = commands::run_captured(&cmd, Some(&d), Duration::from_secs(120), 4096)
                .expect("run");
            fs::write(d.join("result"), &out).unwrap();
            // Stay joined (and acking) until told to exit.
            wait_for("exit", Duration::from_secs(60), || d.join("exit").exists());
        }
        // The Settings side: exactly what "Stop all" / Ctrl-C now call.
        "stopper" => {
            let (r, out) = kill_switch::stop_all_confirmed(Duration::from_secs(8));
            fs::write(
                d.join(format!("report-{}", std::process::id())),
                format!("{:?}\n{:?}\n{}\n", r.barrier, out, out.line()),
            )
            .unwrap();
        }
        // A restarted process: a fresh job must run normally (the old
        // barrier is not a stop for new work) and nothing old is re-run.
        "fresh" => {
            let out =
                commands::run_captured("echo fresh-ok", Some(&d), Duration::from_secs(20), 4096)
                    .expect("run");
            fs::write(d.join("fresh-result"), &out).unwrap();
        }
        r => panic!("unknown role {r}"),
    }
}

fn spawn(d: &Path, role: &str) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "xstop_role", "--nocapture", "--test-threads=1"])
        .env(ROLE, role)
        .env(DIR, d)
        .spawn()
        .unwrap()
}

fn mtime(p: &Path) -> Option<SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Heartbeat `name` still moves over `over`.
fn beating(d: &Path, name: &str, over: Duration) -> bool {
    let a = fs::read_to_string(d.join(name)).unwrap_or_default();
    std::thread::sleep(over);
    let b = fs::read_to_string(d.join(name)).unwrap_or_default();
    a != b
}

fn barrier_len(d: &Path) -> u64 {
    fs::metadata(session(d).join(stop_session::BARRIER_FILE))
        .map(|m| m.len())
        .unwrap_or(0)
}

/// Kills group `pgid` only if one of its members runs inside our temp dir.
fn cleanup_group(d: &Path, pgid: &str) {
    if pgid.is_empty() || stop_session::group_alive(pgid.parse().unwrap_or(0)) != Some(true) {
        return;
    }
    let _ = Command::new("kill")
        .args(["-s", "KILL", "--", &format!("-{pgid}")])
        .status();
    let _ = d;
}

#[test]
fn stop_from_a_separate_process_persists_the_barrier_and_kills_the_tree() {
    if std::env::var(ROLE).is_ok() {
        return;
    }
    let d = std::env::temp_dir().join(format!("lillith-xstop-e2e-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();

    // 1. The overlay-side worker starts its long job.
    let mut w = spawn(&d, "worker");
    wait_for("heartbeats", Duration::from_secs(30), || {
        d.join("hb1").exists() && d.join("hb2").exists() && d.join("pgid").exists()
    });
    let pgid = fs::read_to_string(d.join("pgid"))
        .unwrap()
        .trim()
        .to_string();
    assert!(
        beating(&d, "hb1", Duration::from_millis(400)),
        "job is running"
    );

    // 2. The safeword only hushes: no barrier, the tree keeps running.
    fs::write(d.join("do-hush"), "1").unwrap();
    wait_for("hushed", Duration::from_secs(10), || {
        d.join("hushed").exists()
    });
    assert_eq!(barrier_len(&d), 0, "the safeword never writes the barrier");
    assert!(
        beating(&d, "hb1", Duration::from_millis(600)),
        "hush left hb1 running"
    );
    assert!(
        beating(&d, "hb2", Duration::from_millis(300)),
        "hush left hb2 running"
    );
    assert!(!d.join("result").exists(), "hush did not end the job");
    println!("HUSH: barrier untouched, heartbeats still advancing, job still running");

    // 3. Stop from a separate process.
    let mut s = spawn(&d, "stopper");
    assert!(s.wait().unwrap().success(), "stopper ran");
    let sp = s.id();
    let report = fs::read_to_string(d.join(format!("report-{sp}"))).unwrap();
    println!("STOPPER REPORT:\n{report}");
    assert!(
        report.starts_with("Saved("),
        "barrier saved before reporting: {report}"
    );
    assert!(
        report.contains("Quiescent"),
        "every live process confirmed: {report}"
    );
    let log = fs::read_to_string(session(&d).join(stop_session::BARRIER_FILE)).unwrap();
    assert!(
        log.contains(&format!("pid={sp}")),
        "durable barrier names the stopper: {log}"
    );

    // 4. The tree is gone: heartbeats stop, no live member of the group.
    wait_for("worker result", Duration::from_secs(10), || {
        d.join("result").exists()
    });
    let result = fs::read_to_string(d.join("result")).unwrap();
    assert!(
        commands::was_stopped(&result),
        "runner says stopped: {result:?}"
    );
    assert!(
        !beating(&d, "hb1", Duration::from_millis(500)),
        "hb1 stopped"
    );
    assert!(
        !beating(&d, "hb2", Duration::from_millis(200)),
        "hb2 stopped"
    );
    assert_eq!(
        stop_session::group_alive(pgid.parse().unwrap()),
        Some(false),
        "no live process left in the job's group {pgid}"
    );
    println!(
        "STOP: barrier persisted, runner reported stopped, heartbeats frozen, group {pgid} empty"
    );

    // 5. A live process that never answers is never counted as stopped.
    let mut mute = Command::new("sleep").arg("30").spawn().unwrap();
    let mp = mute.id();
    let st = fs::read_to_string(format!("/proc/{mp}/stat")).unwrap();
    let start: u64 = st[st.rfind(')').unwrap() + 2..]
        .split_whitespace()
        .nth(19)
        .unwrap()
        .parse()
        .unwrap();
    fs::write(
        session(&d).join(format!("proc-{mp}")),
        format!("{mp} {start}\n"),
    )
    .unwrap();
    let mut s2 = spawn(&d, "stopper");
    assert!(s2.wait().unwrap().success());
    let report2 = fs::read_to_string(d.join(format!("report-{}", s2.id()))).unwrap();
    println!("UNREACHABLE REPORT:\n{report2}");
    assert!(report2.contains("Incomplete"), "{report2}");
    assert!(
        report2.contains(&mp.to_string()),
        "names the silent pid: {report2}"
    );
    assert!(report2.contains("NOT confirmed"), "{report2}");
    assert!(!report2.lines().nth(2).unwrap_or("").starts_with("Stopped"));
    let _ = mute.kill();
    let _ = mute.wait();

    // 6. Restart: the worker exits, a fresh process joins. New work runs
    //    (the old barrier is not a stop for it); the cancelled job is not
    //    resurrected (heartbeats stay frozen).
    fs::write(d.join("exit"), "1").unwrap();
    let _ = w.wait();
    let before = (mtime(&d.join("hb1")), mtime(&d.join("hb2")));
    let mut f = spawn(&d, "fresh");
    assert!(f.wait().unwrap().success());
    let fresh = fs::read_to_string(d.join("fresh-result")).unwrap();
    assert!(
        fresh.contains("fresh-ok") && fresh.trim_end().ends_with("[exit 0]"),
        "{fresh:?}"
    );
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        before,
        (mtime(&d.join("hb1")), mtime(&d.join("hb2"))),
        "nothing re-ran"
    );
    println!("RESTART: fresh job ran to [exit 0]; cancelled job not resurrected");

    cleanup_group(&d, &pgid);
    let _ = fs::remove_dir_all(&d);
}
