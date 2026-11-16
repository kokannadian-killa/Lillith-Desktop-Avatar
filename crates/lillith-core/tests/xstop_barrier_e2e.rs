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
        // A restarted process: dispatch stays disarmed after the global
        // stop (nothing starts) until an explicit re-arm; then a new job
        // runs normally (the old barrier is not a stop for new work).
        "fresh" => {
            assert!(
                kill_switch::disarmed(),
                "disarm survives into a new process"
            );
            let refused = commands::run_captured(
                "echo should-not-run > ran",
                Some(&d),
                Duration::from_secs(5),
                4096,
            );
            assert!(
                refused.is_err(),
                "disarmed dispatch must be refused: {refused:?}"
            );
            assert!(!d.join("ran").exists(), "nothing started while disarmed");
            kill_switch::rearm().expect("rearm");
            let out =
                commands::run_captured("echo fresh-ok", Some(&d), Duration::from_secs(20), 4096)
                    .expect("run");
            fs::write(d.join("fresh-result"), &out).unwrap();
        }
        // An overlay that will be SIGKILLed mid-command: a group with a
        // looping child plus a `setsid` escapee, through the real runner.
        "orphan-worker" => {
            let cmd = format!(
                "echo $$ > '{0}/opgid'; \
                 (while :; do date +%s%N > '{0}/ohb1'; sleep 0.05; done) & \
                 setsid sh -c 'echo $$ > \"{0}/oesc\"; while :; do date +%s%N > \"{0}/ohb2\"; sleep 0.05; done' & \
                 wait",
                d.display()
            );
            let _ = commands::run_captured(&cmd, Some(&d), Duration::from_secs(120), 4096);
            // Never reached in the test: the process is SIGKILLed first.
            fs::write(d.join("orphan-worker-returned"), "1").unwrap();
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
    println!("RESTART: dispatch refused while disarmed; after explicit re-arm a fresh job ran to [exit 0]; cancelled job not resurrected");

    cleanup_group(&d, &pgid);
    let _ = fs::remove_dir_all(&d);
}

fn pid_alive(pid: u32) -> bool {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(st) => {
            let state = st[st.rfind(')').unwrap() + 2..].chars().next();
            !matches!(state, Some('Z') | Some('X'))
        }
        Err(_) => false,
    }
}

fn pgrp_of(pid: u32) -> Option<u32> {
    let st = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    st[st.rfind(')')? + 2..]
        .split_whitespace()
        .nth(2)?
        .parse()
        .ok()
}

/// QA blocker (b291da82): a Lillith process SIGKILLed mid-command leaves its
/// command tree running. The next stop (from another process) must kill the
/// tree it recorded (group + `setsid` escapee) and never report a false
/// "Stopped" while any of it is alive.
#[test]
fn stop_kills_the_tree_of_a_sigkilled_member_and_never_claims_a_false_stop() {
    if std::env::var(ROLE).is_ok() {
        return;
    }
    let d = std::env::temp_dir().join(format!("lillith-xstop-orphan-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();

    // 1. The member starts its job; its entry records the group and the
    //    escapee (descendant snapshots are persisted).
    let mut w = spawn(&d, "orphan-worker");
    let wp = w.id();
    wait_for("orphan job", Duration::from_secs(30), || {
        ["opgid", "oesc", "ohb1", "ohb2"].iter().all(|f| {
            !fs::read_to_string(d.join(f))
                .unwrap_or_default()
                .trim()
                .is_empty()
        })
    });
    let read_pid = |f: &str| -> u32 {
        fs::read_to_string(d.join(f))
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    };
    let pgid = read_pid("opgid");
    let esc = read_pid("oesc");
    assert_ne!(pgrp_of(esc), Some(pgid), "the escapee left the group");
    let entry = session(&d).join(format!("proc-{wp}"));
    wait_for("entry records the tree", Duration::from_secs(10), || {
        let e = fs::read_to_string(&entry).unwrap_or_default();
        e.contains(&format!("\ng {pgid} ")) && e.contains(&format!("\np {esc} "))
    });
    println!("ENTRY:\n{}", fs::read_to_string(&entry).unwrap_or_default());

    // 2. SIGKILL the member mid-command: its tree survives it.
    w.kill().unwrap();
    let _ = w.wait();
    assert!(!pid_alive(wp));
    assert!(
        beating(&d, "ohb1", Duration::from_millis(400)),
        "orphan group survives SIGKILL"
    );
    assert!(
        beating(&d, "ohb2", Duration::from_millis(300)),
        "orphan escapee survives SIGKILL"
    );
    assert!(!d.join("orphan-worker-returned").exists());

    // 3. Stop from a separate process.
    let mut s = spawn(&d, "stopper");
    assert!(s.wait().unwrap().success(), "stopper ran");
    let report = fs::read_to_string(d.join(format!("report-{}", s.id()))).unwrap();
    println!("ORPHAN STOP REPORT:\n{report}");
    assert!(report.starts_with("Saved("), "{report}");

    // 4. The orphaned tree is dead.
    assert!(
        !beating(&d, "ohb1", Duration::from_millis(500)),
        "ohb1 frozen"
    );
    assert!(
        !beating(&d, "ohb2", Duration::from_millis(200)),
        "ohb2 frozen"
    );
    assert_eq!(
        stop_session::group_alive(pgid),
        Some(false),
        "group {pgid} empty"
    );
    assert!(!pid_alive(esc), "setsid escapee {esc} killed");

    // 5. Never a false Stopped: Quiescent only with the orphan tree counted
    //    as killed and verified (entry pruned); otherwise NOT confirmed and
    //    the entry kept.
    let line = report.lines().nth(2).unwrap_or("");
    if report.contains("Quiescent") {
        assert!(report.contains("orphans_killed: 1"), "{report}");
        assert!(
            line.contains("left by a crashed Lillith process killed and checked"),
            "{line}"
        );
        assert!(!entry.exists(), "entry pruned once its tree is gone");
    } else {
        assert!(
            report.contains("Incomplete") && line.contains("NOT confirmed"),
            "{report}"
        );
        assert!(!line.starts_with("Stopped"));
        assert!(entry.exists());
    }
    println!("ORPHAN: SIGKILLed member's group and setsid escapee killed by the next stop; outcome {line}");

    // Belt and braces: never leave anything of ours running.
    cleanup_group(&d, &pgid.to_string());
    if pid_alive(esc) {
        let _ = Command::new("kill")
            .args(["-s", "KILL", &esc.to_string()])
            .status();
    }
    let _ = fs::remove_dir_all(&d);
}
