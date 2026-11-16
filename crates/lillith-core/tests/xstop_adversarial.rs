//! Independent QA: adversarial tests for the cross-process stop
//! (`feat/swarm-cross-process-stop` @ b291da82).
//!
//! Modeled on `cross_process_stop.rs`: the test binary re-runs itself with
//! `LILLITH_QA_XADV_ROLE` set, so every role is a separate OS process with
//! its own fresh, private (0700) XDG_RUNTIME/DATA/CONFIG/CACHE/STATE dirs
//! under one per-run temp dir (that is what lets a role join the stop session
//! under cargo; see `stop_session::env_allows_join`). Children are synthetic
//! `sh` heartbeat loops that append to `hb.<name>` and write `<name>.pid`.
//! Nothing outside the per-run temp dir is ever touched or signalled: `Drop`
//! SIGKILLs only the role processes this test spawned and the tree PIDs whose
//! `/proc/<pid>/cmdline` contains the run's unique temp dir.
//!
//! Roles:
//! - `worker`: joins the session, runs `VARIANT`'s script through the real
//!   `commands::run_child`. With `QA_SIG=1` it also installs the Ctrl-C /
//!   SIGTERM handler (a copy of `install_ctrlc_stop`, see below).
//! - `stopper`: a separate Settings-like process. `stop_all_confirmed(QA_DEADLINE_MS)`
//!   and writes the `StopReport` / `StopOutcome` to `stop.out`.
//! - `sigsettings`: Settings stand-in that joins and installs the signal
//!   handler, then idles until signalled.
//! - `observer`: a fresh process started after the stop: `disarmed()`, one
//!   synthetic dispatch, and with `QA_REARM=1` a `rearm()` then a dispatch.
//!
//! Signal seam (test 3): the real handlers live in
//! `lillith-settings-gui::bots::install_ctrlc_stop` (bots/mod.rs:1024, a
//! `libc::signal` handler that sets an atomic plus a watcher thread calling
//! `stop_all_confirmed(2s)` then `exit(130)`) and in
//! `apps/lillith/src/chat.rs:406` (`stop_on_signal`, tokio signal then
//! `stop_all()` then `exit(130)`). Neither is reachable from a
//! `lillith-core` integration test: `lillith-settings-gui` and `apps/lillith`
//! depend on `lillith-core`, not the other way round, and adding them as
//! dev-deps would change Cargo.lock (builds are `--locked`). The
//! `sigsettings` role therefore installs a line-for-line copy of
//! `install_ctrlc_stop` and receives a real SIGINT / SIGTERM from `kill(2)`.
//! What is NOT covered: the real `lillith-settings` / `lillith` binaries'
//! start-up ordering (join before handler) and the tokio path.
//!
//! Windows cannot be tested here (no `/proc`; BOTS.md says a stop is always
//! "not confirmed" there).
//!
//! Run (each test also prints an `EVIDENCE` line):
//!   cargo test -p lillith-core --test xstop_adversarial -- --include-ignored --nocapture --test-threads=1
#![cfg(target_os = "linux")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use lillith_core::access::{commands, kill_switch, stop_session};

const ROLE: &str = "LILLITH_QA_XADV_ROLE";
const DIR: &str = "LILLITH_QA_XADV_DIR";
const VARIANT: &str = "LILLITH_QA_XADV_VARIANT";
const DEADLINE: Duration = Duration::from_secs(5);

// ------------------------------------------------------------------ roles

#[test]
fn qa_xadv_role_entry() {
    let Ok(role) = std::env::var(ROLE) else {
        return;
    };
    let dir = PathBuf::from(std::env::var(DIR).expect("role needs dir"));
    let joined = match kill_switch::join_session() {
        Ok(p) => format!("ok {}", p.display()),
        Err(e) => format!("err {e}"),
    };
    fs::write(dir.join(format!("{role}.joined")), &joined).unwrap();
    match role.as_str() {
        "worker" => role_worker(&dir),
        "stopper" => {
            let ms: u64 = std::env::var("QA_DEADLINE_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5000);
            let t = Instant::now();
            let (r, out) = kill_switch::stop_all_confirmed(Duration::from_millis(ms));
            fs::write(
                dir.join("stop.out"),
                format!(
                    "pid={}\nelapsed_ms={}\nbarrier={:?}\nconfirmed={}\noutcome={:?}\nchat_line={}\n",
                    std::process::id(),
                    t.elapsed().as_millis(),
                    r.barrier,
                    out.confirmed(),
                    out,
                    r.chat_line()
                ),
            )
            .unwrap();
        }
        "sigsettings" => {
            install_ctrlc_stop_copy(&dir);
            fs::write(dir.join("sig.ready"), std::process::id().to_string()).unwrap();
            loop {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        "observer" => {
            let tag = std::env::var("QA_TAG").unwrap_or_else(|_| "obs".into());
            let mut s = format!(
                "joined={joined}\ndisarmed={}\ngeneration={}\n",
                kill_switch::disarmed(),
                stop_session::generation()
            );
            s.push_str(&format!("dispatch1={}\n", dispatch(&dir, &format!("{tag}-1"))));
            if std::env::var("QA_REARM").is_ok() {
                s.push_str(&format!("rearm={:?}\n", kill_switch::rearm().map_err(|e| e.to_string())));
                s.push_str(&format!("disarmed_after_rearm={}\n", kill_switch::disarmed()));
                s.push_str(&format!("dispatch2={}\n", dispatch(&dir, &format!("{tag}-2"))));
            }
            fs::write(dir.join(format!("{tag}.out")), s).unwrap();
        }
        "reaper" => role_reaper(&dir),
        other => panic!("unknown role {other}"),
    }
}

/// One synthetic dispatch: `ran=<marker exists> out=<runner text or error>`.
fn dispatch(dir: &Path, tag: &str) -> String {
    let marker = dir.join(format!("ran-{tag}"));
    let mut c = Command::new("sh");
    c.arg("-c").arg(format!("echo RAN > '{}'", marker.display()));
    let out = match commands::run_child(c, Duration::from_secs(5), 4096) {
        Ok(t) => format!("ok:{}", t.trim().replace('\n', " | ")),
        Err(e) => format!("err:{e}"),
    };
    format!("ran={} {out}", marker.exists())
}

static SIG: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    SIG.store(true, Ordering::SeqCst);
}

/// Line-for-line copy of `lillith_settings_gui::bots::install_ctrlc_stop`
/// (b291da82, bots/mod.rs:1024-1043), plus one write of the outcome to
/// `sig.out` so the test can read it.
fn install_ctrlc_stop_copy(dir: &Path) {
    let h = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
    // SAFETY: the handler only stores to an atomic (async-signal-safe).
    unsafe {
        libc::signal(libc::SIGINT, h);
        libc::signal(libc::SIGTERM, h);
    }
    let dir = dir.to_path_buf();
    let _ = std::thread::Builder::new()
        .name("qa-ctrlc".into())
        .spawn(move || loop {
            if SIG.load(Ordering::SeqCst) {
                let (r, out) = kill_switch::stop_all_confirmed(Duration::from_secs(2));
                let _ = fs::write(
                    dir.join("sig.out"),
                    format!(
                        "pid={}\nbarrier={:?}\nconfirmed={}\noutcome={:?}\nline={}\n",
                        std::process::id(),
                        r.barrier,
                        out.confirmed(),
                        out,
                        out.line()
                    ),
                );
                std::process::exit(130);
            }
            std::thread::sleep(Duration::from_millis(100));
        });
}

fn role_worker(dir: &Path) {
    fs::write(dir.join("worker.pid"), std::process::id().to_string()).unwrap();
    if std::env::var("QA_SIG").is_ok() {
        install_ctrlc_stop_copy(dir);
    }
    let d = dir.display().to_string();
    let beat = |name: &str| {
        format!("sh -c 'echo $$ > \"{d}/{name}.pid\"; while :; do echo b >> \"{d}/hb.{name}\"; sleep 0.1; done'")
    };
    let variant = std::env::var(VARIANT).unwrap_or_else(|_| "plain".into());
    let mut parts = variant.split(':');
    let script = match parts.next() {
        // child + in-group grandchild.
        Some("plain") => format!(
            "echo $$ > \"{d}/child.pid\"; {gc} & while :; do echo b >> \"{d}/hb.child\"; sleep 0.1; done",
            gc = beat("gc"),
        ),
        // dfork:<delay s>:<linger s>: after <delay>, a subshell (the
        // intermediate) starts `setsid sh` (new session + group) in the
        // background and exits after <linger>; the grandchild reparents to
        // init / the nearest subreaper. Its stdio is /dev/null so it never
        // holds the runner's pipe.
        Some("dfork") => {
            let delay = parts.next().unwrap_or("0");
            let linger = parts.next().unwrap_or("0");
            let linger = if linger == "0" {
                String::new()
            } else {
                format!(" sleep {linger};")
            };
            format!(
                "echo $$ > \"{d}/child.pid\"; sleep {delay}; ( setsid {orph} </dev/null >/dev/null 2>&1 &{linger} ); while :; do echo b >> \"{d}/hb.child\"; sleep 0.1; done",
                orph = beat("orphan"),
            )
        }
        other => panic!("unknown variant {other:?}"),
    };
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
    if std::env::var("QA_SIG").is_ok() {
        // A real app keeps running after its runner returns; without this
        // the role's main thread would exit 0 before the handler thread
        // finishes `stop_all_confirmed` and calls `exit(130)`.
        std::thread::sleep(Duration::from_secs(10));
    }
}

/// Reaping / subreaper probe (see `adv6`). The command's top process forks
/// a middle `sh`, then `exec`s `sleep 2` (a top process that never calls
/// wait). The middle `sh` double-forks a short-lived `setsid` orphan. A
/// sampler records the orphan's parent and state while the runner waits.
fn role_reaper(dir: &Path) {
    let d = dir.display().to_string();
    fs::write(
        dir.join("orphan.sh"),
        format!("echo $$ > \"{d}/orphan.pid\"; sleep 0.4\n"),
    )
    .unwrap();
    fs::write(
        dir.join("mid.sh"),
        format!(
            "echo $$ > \"{d}/mid.pid\"; ( setsid sh \"{d}/orphan.sh\" </dev/null >/dev/null 2>&1 & ); sleep 1\n"
        ),
    )
    .unwrap();
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let sampler = {
        let dir = dir.to_path_buf();
        let stop = stop.clone();
        std::thread::spawn(move || {
            let rd = |f: &str| -> Option<i32> {
                fs::read_to_string(dir.join(f)).ok()?.trim().parse().ok()
            };
            let mut ppids: Vec<i32> = Vec::new();
            let mut zombie_under_top = false;
            while !stop.load(Ordering::SeqCst) {
                if let (Some(o), Some(t)) = (rd("orphan.pid"), rd("top.pid")) {
                    if let Ok(st) = fs::read_to_string(format!("/proc/{o}/stat")) {
                        if let Some((_, rest)) = st.rsplit_once(')') {
                            let mut it = rest.split_whitespace();
                            let state = it.next().unwrap_or("");
                            let pp: i32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
                            if !ppids.contains(&pp) {
                                ppids.push(pp);
                            }
                            if state == "Z" && pp == t {
                                zombie_under_top = true;
                            }
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            (ppids, zombie_under_top)
        })
    };
    let mut c = Command::new("sh");
    c.arg("-c").arg(format!(
        "echo $$ > \"{d}/top.pid\"; sh \"{d}/mid.sh\" & exec sleep 2"
    ));
    let t0 = Instant::now();
    let out = commands::run_child(c, Duration::from_secs(20), 4096)
        .unwrap_or_else(|e| format!("run_child error: {e}"));
    let elapsed = t0.elapsed().as_millis();
    std::thread::sleep(Duration::from_millis(500));
    stop.store(true, Ordering::SeqCst);
    let (ppids, zombie_under_top) = sampler.join().unwrap();
    let me = std::process::id() as i32;
    // Zombie children of this (runner) process, and the orphan's fate.
    let mut my_zombies = Vec::new();
    if let Ok(rd) = fs::read_dir("/proc") {
        for e in rd.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else {
                continue;
            };
            if let Ok(st) = fs::read_to_string(format!("/proc/{pid}/stat")) {
                if let Some((_, rest)) = st.rsplit_once(')') {
                    let mut it = rest.split_whitespace();
                    let state = it.next().unwrap_or("");
                    let pp: i32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
                    if state == "Z" && pp == me {
                        my_zombies.push(pid);
                    }
                }
            }
        }
    }
    let rd = |f: &str| -> i32 {
        fs::read_to_string(dir.join(f))
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0)
    };
    let orphan = rd("orphan.pid");
    let orphan_after = fs::read_to_string(format!("/proc/{orphan}/stat"))
        .map(|s| s.rsplit_once(')').map(|(_, r)| r.trim().chars().next().unwrap_or('?')).unwrap_or('?').to_string())
        .unwrap_or_else(|_| "gone".into());
    // SAFETY: PR_GET_CHILD_SUBREAPER writes one c_int to the pointer.
    let mut sr: libc::c_int = -1;
    unsafe {
        libc::prctl(libc::PR_GET_CHILD_SUBREAPER, &mut sr as *mut libc::c_int, 0, 0, 0);
    }
    fs::write(
        dir.join("reaper.out"),
        format!(
            "top={}\nmid={}\norphan={orphan}\norphan_ppids={ppids:?}\nzombie_under_top={zombie_under_top}\n\
             orphan_after={orphan_after}\nmy_zombies={my_zombies:?}\nrunner_is_subreaper={sr}\n\
             elapsed_ms={elapsed}\nout={}\n",
            rd("top.pid"),
            rd("mid.pid"),
            out.trim().replace('\n', " | ")
        ),
    )
    .unwrap();
}

// ---------------------------------------------------------------- harness

struct Run {
    dir: PathBuf,
    kids: Vec<Child>,
}

impl Run {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "lillith-qa-xadv-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        for x in ["runtime", "data", "config", "cache", "state"] {
            let d = dir.join("xdg").join(x);
            fs::create_dir_all(&d).unwrap();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&d, fs::Permissions::from_mode(0o700)).unwrap();
        }
        Run {
            dir,
            kids: Vec::new(),
        }
    }

    fn stop_dir(&self) -> PathBuf {
        self.dir.join("xdg/runtime/lillith/stop")
    }

    fn role(&self, role: &str) -> Command {
        let mut c = Command::new(std::env::current_exe().unwrap());
        c.args([
            "qa_xadv_role_entry",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ROLE, role)
        .env(DIR, &self.dir)
        .env("XDG_RUNTIME_DIR", self.dir.join("xdg/runtime"))
        .env("XDG_DATA_HOME", self.dir.join("xdg/data"))
        .env("XDG_CONFIG_HOME", self.dir.join("xdg/config"))
        .env("XDG_CACHE_HOME", self.dir.join("xdg/cache"))
        .env("XDG_STATE_HOME", self.dir.join("xdg/state"))
        .env_remove("LILLITH_STOP_SESSION")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        c
    }

    fn run_role(&self, role: &str, env: &[(&str, &str)]) -> i32 {
        let mut c = self.role(role);
        for (k, v) in env {
            c.env(k, v);
        }
        c.status().unwrap().code().unwrap_or(-1)
    }

    fn spawn_role(&mut self, role: &str, env: &[(&str, &str)]) -> usize {
        let mut c = self.role(role);
        for (k, v) in env {
            c.env(k, v);
        }
        self.kids.push(c.spawn().unwrap());
        self.kids.len() - 1
    }

    /// Starts a worker and waits until every file in `need` exists.
    fn start_worker(&mut self, env: &[(&str, &str)], need: &[&str]) -> usize {
        let i = self.spawn_role("worker", env);
        let t = Instant::now();
        while !need.iter().all(|f| self.dir.join(f).exists()) {
            assert!(
                t.elapsed() < Duration::from_secs(20),
                "worker tree did not start\n{}",
                self.evidence()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(300));
        i
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

    fn beating(&self, name: &str, window: Duration) -> bool {
        let a = self.beats(name);
        std::thread::sleep(window);
        self.beats(name) > a
    }

    fn alive(pid: i32) -> bool {
        match fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(s) => s
                .rsplit_once(')')
                .is_none_or(|(_, rest)| !rest.trim_start().starts_with('Z')),
            Err(_) => false,
        }
    }

    fn ppid(pid: i32) -> Option<i32> {
        let s = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let (_, rest) = s.rsplit_once(')')?;
        rest.split_whitespace().nth(1)?.parse().ok()
    }

    /// `name=pid(alive|dead)` for each tree member.
    fn pids_state(&self, names: &[&str]) -> String {
        names
            .iter()
            .map(|n| match self.pid(&format!("{n}.pid")) {
                Some(p) => format!(
                    "{n}={p}({})",
                    if Self::alive(p) { "ALIVE" } else { "dead" }
                ),
                None => format!("{n}=none"),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn wait_dead(&self, names: &[&str], d: Duration) -> (bool, bool) {
        let t = Instant::now();
        loop {
            let dead = names
                .iter()
                .all(|n| self.pid(&format!("{n}.pid")).is_none_or(|p| !Self::alive(p)));
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

    /// Waits for spawned role `i` to exit; its code (`None` = still running
    /// or killed by a signal).
    fn wait_kid(&mut self, i: usize, d: Duration) -> Option<i32> {
        let t = Instant::now();
        while t.elapsed() < d {
            if let Ok(Some(st)) = self.kids[i].try_wait() {
                return st.code();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }

    fn signal_kid(&self, i: usize, sig: libc::c_int) {
        // SAFETY: plain kill(2) on a Child this test spawned and still owns.
        unsafe {
            libc::kill(self.kids[i].id() as libc::pid_t, sig);
        }
    }

    fn evidence(&self) -> String {
        let mut s = String::new();
        let mut names: Vec<String> = fs::read_dir(&self.dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|n| {
                        n.ends_with(".out") || n.ends_with(".joined") || n == "worker.result"
                    })
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        for f in names {
            s.push_str(&format!("--- {f}\n{}\n", self.read(&f).trim_end()));
        }
        s
    }
}

impl Drop for Run {
    /// Kills only what this run spawned: tree PIDs whose cmdline contains our
    /// unique temp dir, then the role `Child`ren we own.
    fn drop(&mut self) {
        let marker = self.dir.display().to_string();
        let pid_files: Vec<String> = fs::read_dir(&self.dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|n| n.ends_with(".pid") && n != "worker.pid")
                    .collect()
            })
            .unwrap_or_default();
        for _ in 0..2 {
            for f in &pid_files {
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
            std::thread::sleep(Duration::from_millis(150));
        }
        for k in &mut self.kids {
            let _ = k.kill();
            let _ = k.wait();
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn kv<'a>(text: &'a str, key: &str) -> &'a str {
    text.lines()
        .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
        .unwrap_or("")
}

// ------------------------------------------------------------------ tests

/// 1. Double fork that orphans between the runner's ~200 ms `/proc`
///    snapshots. The worker's command forks an intermediate subshell that
///    starts `setsid sh` (new session and group) and exits at once, so the
///    grandchild reparents to init. A separate process then fires the stop.
///    Contract: the orphan dies, and the stop is never reported confirmed
///    (Quiescent) while it still heartbeats. Several timing variants.
#[test]
fn adv1_double_fork_orphan_between_snapshots() {
    let mut fails = Vec::new();
    let mut ev_all = String::new();
    for delay in ["0", "0.05", "0.15", "0.35", "0.8"] {
        let mut r = Run::new("dfork");
        let variant = format!("dfork:{delay}:0");
        r.start_worker(&[(VARIANT, &variant)], &["child.pid", "orphan.pid", "hb.orphan"]);
        let orphan = r.pid("orphan.pid").unwrap();
        let child = r.pid("child.pid").unwrap();
        let ppid = Run::ppid(orphan);
        assert!(r.beating("orphan", Duration::from_millis(400)), "orphan not beating");
        r.run_role("stopper", &[("QA_DEADLINE_MS", "3000")]);
        let so = r.read("stop.out");
        let (dead, quiet) = r.wait_dead(&["child", "orphan"], DEADLINE);
        let orphan_alive = Run::alive(orphan);
        let orphan_beats = r.beating("orphan", Duration::from_millis(400));
        let confirmed = kv(&so, "confirmed") == "true";
        let line = format!(
            "delay={delay}s child={child} orphan={orphan} orphan_ppid_before_stop={ppid:?} \
             after: {} orphan_beating={orphan_beats} all_dead={dead} quiet={quiet} \
             confirmed={confirmed} outcome={}",
            r.pids_state(&["child", "orphan"]),
            kv(&so, "outcome")
        );
        eprintln!("EVIDENCE adv1 {line}");
        if orphan_alive || orphan_beats {
            fails.push(format!("orphan survived the stop: {line}"));
        }
        if confirmed && (orphan_alive || orphan_beats) {
            fails.push(format!("reported confirmed (Quiescent) while orphan alive: {line}"));
        }
        ev_all.push_str(&format!("=== {variant}\n{}\n", r.evidence()));
    }
    assert!(fails.is_empty(), "{}\n{ev_all}", fails.join("\n"));
}

/// 1b. Control for (1): the same double fork, but the intermediate lingers
/// 0.6 s, so the runner's snapshot sees the parent chain. The orphan must be
/// killed and the stop confirmed. Shows test 1 measures the snapshot race,
/// not a harness artifact.
#[test]
fn adv1b_control_double_fork_seen_by_snapshot_is_killed() {
    let mut r = Run::new("dfctl");
    r.start_worker(&[(VARIANT, "dfork:0.3:0.6")], &["child.pid", "orphan.pid", "hb.orphan"]);
    // Let the intermediate exit so the orphan is reparented before the stop.
    std::thread::sleep(Duration::from_millis(900));
    let orphan = r.pid("orphan.pid").unwrap();
    let ppid = Run::ppid(orphan);
    r.run_role("stopper", &[("QA_DEADLINE_MS", "3000")]);
    let so = r.read("stop.out");
    let (dead, quiet) = r.wait_dead(&["child", "orphan"], DEADLINE);
    let st = r.pids_state(&["child", "orphan"]);
    let ev = r.evidence();
    eprintln!(
        "EVIDENCE adv1b orphan_ppid_before_stop={ppid:?} after: {st} dead={dead} quiet={quiet} outcome={}",
        kv(&so, "outcome")
    );
    drop(r);
    assert!(dead && quiet, "lingering-intermediate orphan survived: {st}\n{ev}");
    assert!(so.contains("confirmed=true"), "control stop not confirmed\n{ev}");
}

/// 2. SIGKILLed Lillith parent. The worker (runner) process is SIGKILLed
///    while its command tree runs, then a separate process fires the stop.
///    Contract: never reported as confirmed while any descendant still
///    heartbeats (Incomplete is fine). Also records whether the tree dies.
#[test]
fn adv2_sigkilled_runner_tree_never_reported_stopped() {
    let mut r = Run::new("sigkill");
    let w = r.start_worker(&[], &["child.pid", "gc.pid", "hb.child", "hb.gc"]);
    let wpid = r.kids[w].id();
    r.signal_kid(w, libc::SIGKILL);
    let _ = r.wait_kid(w, Duration::from_secs(2));
    std::thread::sleep(Duration::from_millis(300));
    let beating_before = r.beating("child", Duration::from_millis(300));
    r.run_role("stopper", &[("QA_DEADLINE_MS", "3000")]);
    let so = r.read("stop.out");
    let (dead, quiet) = r.wait_dead(&["child", "gc"], DEADLINE);
    let confirmed = kv(&so, "confirmed") == "true";
    let st = r.pids_state(&["child", "gc"]);
    let ev = r.evidence();
    eprintln!(
        "EVIDENCE adv2 worker={wpid}(SIGKILLed) beating_before_stop={beating_before} after: {st} \
         all_dead={dead} quiet={quiet} confirmed={confirmed} outcome={} chat_line={}",
        kv(&so, "outcome"),
        kv(&so, "chat_line")
    );
    drop(r);
    assert!(
        !(confirmed && !(dead && quiet)),
        "stop reported confirmed while the SIGKILLed runner's tree still runs: {st}\n{ev}"
    );
    assert!(
        dead && quiet,
        "SIGKILLed runner's tree survived the stop (not reported confirmed): {st}\n{ev}"
    );
}

fn signal_case(sig: libc::c_int, name: &str) -> Result<String, String> {
    let mut r = Run::new(name);
    r.start_worker(&[], &["child.pid", "gc.pid", "hb.child", "hb.gc"]);
    let s = r.spawn_role("sigsettings", &[]);
    assert!(r.wait_file("sig.ready", Duration::from_secs(20)), "sigsettings not ready");
    std::thread::sleep(Duration::from_millis(200));
    r.signal_kid(s, sig);
    let code = r.wait_kid(s, Duration::from_secs(10));
    let (dead, quiet) = r.wait_dead(&["child", "gc"], DEADLINE);
    r.run_role("observer", &[]);
    let obs = r.read("obs.out");
    let sd = r.stop_dir();
    let barrier = fs::metadata(sd.join(stop_session::BARRIER_FILE)).map(|m| m.len()).unwrap_or(0);
    let disarmed_file = sd.join(stop_session::DISARMED_FILE).exists();
    let wres = r.read("worker.result");
    let line = format!(
        "{name}: sig_role_exit={code:?} barrier_len={barrier} disarmed_file={disarmed_file} \
         observer_disarmed={} observer_dispatch=[{}] tree: {} dead={dead} quiet={quiet} \
         worker_was_stopped={} sig_outcome={}",
        kv(&obs, "disarmed"),
        kv(&obs, "dispatch1"),
        r.pids_state(&["child", "gc"]),
        wres.contains("was_stopped=true"),
        kv(&r.read("sig.out"), "outcome")
    );
    let ok = code == Some(130)
        && barrier > 0
        && disarmed_file
        && obs.contains("disarmed=true")
        && obs.contains("dispatch1=ran=false err:")
        && dead
        && quiet;
    let ev = r.evidence();
    if ok {
        Ok(line)
    } else {
        Err(format!("{line}\n{ev}"))
    }
}

/// 3. Real SIGINT / SIGTERM (kill(2)) to a separate Settings stand-in that
///    installed a copy of the real handler (see module docs for why the real
///    binary is not reachable). The kill switch must be pulled durably (barrier
///    and disarm visible to a fresh process) and the worker's tree must die.
#[test]
fn adv3_real_sigint_sigterm_pull_the_kill_switch_durably() {
    let mut fails = Vec::new();
    for (sig, name) in [(libc::SIGINT, "sigint"), (libc::SIGTERM, "sigterm")] {
        match signal_case(sig, name) {
            Ok(l) => eprintln!("EVIDENCE adv3 {l}"),
            Err(e) => {
                eprintln!("EVIDENCE adv3 FAIL {e}");
                fails.push(e);
            }
        }
    }
    assert!(fails.is_empty(), "{}", fails.join("\n"));
}

/// 3b. SIGINT to the worker itself (the overlay / `lillith chat` case: the
/// process running the command gets Ctrl-C). Its own tree must die, it exits
/// 130, and a fresh process sees the barrier and the disarm.
#[test]
fn adv3b_sigint_to_the_runner_process_itself() {
    let mut r = Run::new("sigself");
    let w = r.start_worker(&[("QA_SIG", "1")], &["child.pid", "gc.pid", "hb.child", "hb.gc"]);
    r.signal_kid(w, libc::SIGINT);
    let code = r.wait_kid(w, Duration::from_secs(10));
    let (dead, quiet) = r.wait_dead(&["child", "gc"], DEADLINE);
    r.run_role("observer", &[]);
    let obs = r.read("obs.out");
    let so = r.read("sig.out");
    let st = r.pids_state(&["child", "gc"]);
    let ev = r.evidence();
    eprintln!(
        "EVIDENCE adv3b exit={code:?} tree: {st} dead={dead} quiet={quiet} observer_disarmed={} \
         dispatch=[{}] outcome={}",
        kv(&obs, "disarmed"),
        kv(&obs, "dispatch1"),
        kv(&so, "outcome")
    );
    drop(r);
    assert_eq!(code, Some(130), "{ev}");
    assert!(dead && quiet, "own tree survived SIGINT: {st}\n{ev}");
    assert!(obs.contains("disarmed=true"), "{ev}");
    assert!(obs.contains("dispatch1=ran=false err:"), "{ev}");
}

/// 4a. "Accepted" is never "stopped": a stop with a zero confirmation
/// deadline returns while the worker's tree is certainly still alive. The
/// outcome must not be Quiescent and the chat line must not say Stopped.
#[test]
fn adv4a_zero_deadline_with_live_tree_is_not_stopped() {
    let mut r = Run::new("acc");
    r.start_worker(&[], &["child.pid", "gc.pid", "hb.child", "hb.gc"]);
    r.run_role("stopper", &[("QA_DEADLINE_MS", "0")]);
    let so = r.read("stop.out");
    let (dead, quiet) = r.wait_dead(&["child", "gc"], DEADLINE);
    let ev = r.evidence();
    eprintln!(
        "EVIDENCE adv4a confirmed={} outcome={} chat_line={} tree_later_dead={dead} quiet={quiet}",
        kv(&so, "confirmed"),
        kv(&so, "outcome"),
        kv(&so, "chat_line")
    );
    drop(r);
    assert!(so.contains("confirmed=false"), "{ev}");
    assert!(!kv(&so, "chat_line").starts_with("Stopped"), "{ev}");
    assert!(dead && quiet, "tree did not die after the stop\n{ev}");
}

/// 4b. A joined member that cannot answer (SIGSTOPped: alive, registered,
/// its runner frozen, its tree still heartbeating) must leave the outcome
/// Incomplete with that pid unconfirmed. After SIGCONT the stop must still
/// take effect (the barrier is durable).
#[test]
fn adv4b_frozen_member_is_unconfirmed_not_stopped() {
    let mut r = Run::new("frozen");
    let w = r.start_worker(&[], &["child.pid", "gc.pid", "hb.child", "hb.gc"]);
    let wpid = r.kids[w].id();
    r.signal_kid(w, libc::SIGSTOP);
    std::thread::sleep(Duration::from_millis(200));
    r.run_role("stopper", &[("QA_DEADLINE_MS", "1500")]);
    let so = r.read("stop.out");
    let beating_during = r.beating("child", Duration::from_millis(400));
    r.signal_kid(w, libc::SIGCONT);
    let (dead, quiet) = r.wait_dead(&["child", "gc"], DEADLINE);
    let st = r.pids_state(&["child", "gc"]);
    let ev = r.evidence();
    let outcome = kv(&so, "outcome").to_string();
    eprintln!(
        "EVIDENCE adv4b worker={wpid}(SIGSTOP) confirmed={} outcome={outcome} \
         child_beating_while_frozen={beating_during} after SIGCONT: {st} dead={dead} quiet={quiet}",
        kv(&so, "confirmed")
    );
    drop(r);
    assert!(so.contains("confirmed=false"), "frozen member reported stopped\n{ev}");
    assert!(
        outcome.contains("Incomplete") && outcome.contains(&wpid.to_string()),
        "frozen worker {wpid} not listed\n{ev}"
    );
    assert!(dead && quiet, "after SIGCONT the tree survived: {st}\n{ev}");
}

/// 4c. A Lillith process that runs commands but is outside the session
/// (`LILLITH_STOP_SESSION=off`, honoured in debug builds only). The stop
/// must be honest about it: `Incomplete` with that pid in `opted_out` and a
/// line saying NOT confirmed, never `Quiescent` / "Stopped". Its tree is
/// expected to keep running (by design in debug); that is recorded only.
#[test]
fn adv4c_unjoined_member_is_reported_not_confirmed() {
    let mut r = Run::new("unjoined");
    let w = r.start_worker(
        &[("LILLITH_STOP_SESSION", "off")],
        &["child.pid", "gc.pid", "hb.child", "hb.gc"],
    );
    let wpid = r.kids[w].id();
    r.run_role("stopper", &[("QA_DEADLINE_MS", "2000")]);
    let so = r.read("stop.out");
    let (dead, quiet) = r.wait_dead(&["child", "gc"], Duration::from_secs(2));
    let st = r.pids_state(&["child", "gc"]);
    let outcome = kv(&so, "outcome").to_string();
    let line = kv(&so, "chat_line").to_string();
    let ev = r.evidence();
    eprintln!(
        "EVIDENCE adv4c worker={wpid} worker_joined={} confirmed={} outcome={outcome} chat_line={line} \
         tree (alive by design in debug): {st} dead={dead} quiet={quiet}",
        r.read("worker.joined").trim(),
        kv(&so, "confirmed"),
    );
    drop(r);
    assert!(so.contains("confirmed=false"), "opted-out member reported confirmed\n{ev}");
    assert!(!outcome.contains("Quiescent"), "{ev}");
    let opted = outcome
        .split_once("opted_out: [")
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(list, _)| list.to_string())
        .unwrap_or_default();
    assert!(
        outcome.starts_with("Incomplete")
            && opted.split(',').any(|p| p.trim() == wpid.to_string()),
        "worker {wpid} not in opted_out\n{ev}"
    );
    assert!(
        line.contains("NOT confirmed") && !line.starts_with("Stopped"),
        "line must say NOT confirmed\n{ev}"
    );
}

/// 5. Disarm persists: after a stop, a fresh process cannot dispatch until
///    `rearm()`. Re-arm here is an API call from a non-mouse path (fine at the
///    API level; the mouse-only rule is enforced in Settings ▸ Bots, not in
///    core). After re-arm a fresh process dispatches and the old barrier does
///    not stop the new command. Also checks the session dir/file modes.
#[test]
fn adv5_disarm_persists_until_rearm() {
    let mut r = Run::new("disarm");
    r.start_worker(&[], &["child.pid", "gc.pid", "hb.child", "hb.gc"]);
    r.run_role("stopper", &[("QA_DEADLINE_MS", "3000")]);
    let _ = r.wait_dead(&["child", "gc"], DEADLINE);
    r.run_role("observer", &[("QA_TAG", "o1")]);
    r.run_role("observer", &[("QA_TAG", "o2")]);
    r.run_role("observer", &[("QA_TAG", "o3"), ("QA_REARM", "1")]);
    r.run_role("observer", &[("QA_TAG", "o4")]);
    let (o1, o2, o3, o4) = (r.read("o1.out"), r.read("o2.out"), r.read("o3.out"), r.read("o4.out"));
    use std::os::unix::fs::PermissionsExt;
    let mode = |p: PathBuf| fs::metadata(p).map(|m| m.permissions().mode() & 0o777).unwrap_or(0);
    let sd = r.stop_dir();
    let (dm, bm) = (mode(sd.clone()), mode(sd.join(stop_session::BARRIER_FILE)));
    let ev = r.evidence();
    eprintln!(
        "EVIDENCE adv5 o1[{}|{}] o2[{}|{}] o3[{}|{}|{}|{}] o4[{}|{}] stop_dir_mode={dm:o} barrier_mode={bm:o}",
        kv(&o1, "disarmed"),
        kv(&o1, "dispatch1"),
        kv(&o2, "disarmed"),
        kv(&o2, "dispatch1"),
        kv(&o3, "dispatch1"),
        kv(&o3, "rearm"),
        kv(&o3, "disarmed_after_rearm"),
        kv(&o3, "dispatch2"),
        kv(&o4, "disarmed"),
        kv(&o4, "dispatch1"),
    );
    drop(r);
    for o in [&o1, &o2] {
        assert!(o.contains("disarmed=true"), "{ev}");
        assert!(o.contains("dispatch1=ran=false err:"), "{ev}");
    }
    assert!(o3.contains("dispatch1=ran=false err:"), "{ev}");
    assert!(o3.contains("rearm=Ok(())") && o3.contains("disarmed_after_rearm=false"), "{ev}");
    assert!(o3.contains("dispatch2=ran=true ok:") && o3.contains("[exit 0]"), "{ev}");
    assert!(o4.contains("disarmed=false") && o4.contains("dispatch1=ran=true ok:"), "{ev}");
    assert_eq!((dm, bm), (0o700, 0o600), "{ev}");
}

/// 6. Subreaper / reaping (55ea8c6d makes each command's top process a
///    child subreaper). Normal exit and wait semantics must hold: the runner
///    returns `[exit 0]` when the top process exits, the subreaper bit is not
///    inherited by the middle `sh` (the orphan reparents to the top process,
///    not to the middle one), the runner process itself is not a subreaper,
///    and no zombie is left behind afterwards. A zombie held by a top process
///    that never waits is recorded (bounded by that process's lifetime).
#[test]
fn adv6_subreaper_keeps_exit_semantics_and_leaks_no_zombies() {
    let r = Run::new("reap");
    let code = r.run_role("reaper", &[]);
    let o = r.read("reaper.out");
    let ev = r.evidence();
    eprintln!("EVIDENCE adv6 exit={code} {}", o.replace('\n', " "));
    let top = kv(&o, "top").to_string();
    let mid = kv(&o, "mid").to_string();
    let ppids = kv(&o, "orphan_ppids").to_string();
    let elapsed: u128 = kv(&o, "elapsed_ms").parse().unwrap_or(0);
    drop(r);
    assert_eq!(code, 0, "{ev}");
    assert!(kv(&o, "out").ends_with("[exit 0]"), "normal exit not reported\n{ev}");
    assert!((1800..6000).contains(&elapsed), "runner wait changed: {elapsed} ms\n{ev}");
    assert!(
        ppids.contains(&top) && !ppids.contains(&format!("{mid},")) && !ppids.contains(&format!("{mid}]")),
        "orphan should reparent to the top process {top}, not the middle {mid}: {ppids}\n{ev}"
    );
    assert_eq!(kv(&o, "runner_is_subreaper"), "0", "{ev}");
    assert_eq!(kv(&o, "my_zombies"), "[]", "{ev}");
    assert_eq!(kv(&o, "orphan_after"), "gone", "orphan left as zombie\n{ev}");
}
