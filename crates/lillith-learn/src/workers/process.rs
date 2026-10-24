//! Worker processes: spawn, read, kill.
//!
//! Each worker is a fresh child in **its own process group** (killed with
//! `SIGKILL` to the whole group, no asking), with a minimal environment
//! (no user, host, locale or time zone: `LANG=C`, `TZ=UTC`, `HOME` = its
//! scratch), its scratch dir as working directory, stderr discarded, and —
//! on Linux — `PR_SET_PDEATHSIG(SIGKILL)` so it dies with the parent
//! thread, `PR_SET_NO_NEW_PRIVS`, nice 19 and the idle I/O class.
//!
//! In production the worker runs under bubblewrap ([`bwrap_argv`]):
//! `--unshare-all` (no network namespace at all: workers fetch through the
//! parent), `--die-with-parent`, `--new-session`, `--clearenv`, read-only
//! `/usr`, a tmpfs `/tmp`, and only its scratch dir writable. The worker
//! binary calls [`deny_exec_after_start`] first thing (seccomp: `execve` /
//! `execveat` → `EPERM`), so it can't run any program. No `bwrap` → no
//! worker (never a weaker sandbox).

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

/// Longest line a worker may write.
pub const MAX_LINE: usize = 64 * 1024;

/// What to run as the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Run it under bwrap. Only tests may turn this off.
    sandboxed: bool,
}

impl WorkerCommand {
    /// `program args…` inside the bwrap sandbox.
    pub fn sandboxed(program: impl Into<PathBuf>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            sandboxed: true,
        }
    }

    /// A worker without bwrap (tests: the box may lack user namespaces).
    #[cfg(test)]
    pub(crate) fn unsandboxed(program: impl Into<PathBuf>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            sandboxed: false,
        }
    }

    pub fn is_sandboxed(&self) -> bool {
        self.sandboxed
    }
}

/// Where scratch is mounted inside the sandbox.
pub const SANDBOX_SCRATCH: &str = "/scratch";

/// The bubblewrap argv for `cmd` with `scratch` as its only writable dir.
pub fn bwrap_argv(bwrap: &Path, cmd: &WorkerCommand, scratch: &Path) -> Vec<String> {
    let exe = cmd.program.to_string_lossy().to_string();
    let mut a: Vec<String> = [
        bwrap.to_string_lossy().as_ref(),
        "--unshare-all",
        "--die-with-parent",
        "--new-session",
        "--clearenv",
        "--ro-bind",
        "/usr",
        "/usr",
        "--symlink",
        "usr/lib",
        "/lib",
        "--symlink",
        "usr/lib64",
        "/lib64",
        "--symlink",
        "usr/bin",
        "/bin",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--tmpfs",
        "/tmp",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if !exe.starts_with("/usr/") {
        a.extend(["--ro-bind".to_string(), exe.clone(), exe.clone()]);
    }
    a.extend(
        [
            "--bind",
            &scratch.to_string_lossy(),
            SANDBOX_SCRATCH,
            "--chdir",
            SANDBOX_SCRATCH,
            "--setenv",
            "HOME",
            SANDBOX_SCRATCH,
            "--setenv",
            "TMPDIR",
            SANDBOX_SCRATCH,
            "--setenv",
            "LANG",
            "C",
            "--setenv",
            "TZ",
            "UTC",
            "--",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    a.push(exe);
    a.extend(cmd.args.iter().cloned());
    a
}

/// `bwrap` on this machine, if any.
pub fn find_bwrap() -> Option<PathBuf> {
    ["/usr/bin/bwrap", "/bin/bwrap", "/usr/local/bin/bwrap"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

/// One thing read from the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    Line(String),
    /// A line over [`MAX_LINE`] (the reader stops there).
    Oversized,
    Eof,
}

/// A running worker. Dropping it kills the whole process group.
pub struct WorkerProc {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<Out>,
    done: bool,
}

impl WorkerProc {
    /// Spawns `cmd` with `scratch` as its cwd and only writable dir.
    pub fn spawn(
        cmd: &WorkerCommand,
        scratch: &Path,
        bwrap: Option<&Path>,
    ) -> std::io::Result<Self> {
        let mut c = if cmd.sandboxed {
            let Some(b) = bwrap else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "bwrap is not installed; helper workers stay off (never a weaker sandbox)",
                ));
            };
            let argv = bwrap_argv(b, cmd, scratch);
            let mut c = Command::new(&argv[0]);
            c.args(&argv[1..]);
            c
        } else {
            let mut c = Command::new(&cmd.program);
            c.args(&cmd.args);
            c
        };
        c.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", scratch)
            .env("TMPDIR", scratch)
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .env("TZ", "UTC")
            .current_dir(scratch)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        harden(&mut c);
        let mut child = c.spawn()?;
        super::count_spawn();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let (tx, rx) = std::sync::mpsc::channel();
        if let Some(out) = stdout {
            let spawned = std::thread::Builder::new()
                .name("lillith-worker-out".into())
                .spawn(move || read_lines(out, &tx));
            if let Err(e) = spawned {
                let mut p = Self {
                    child,
                    stdin,
                    rx,
                    done: false,
                };
                p.kill();
                return Err(e);
            }
        }
        Ok(Self {
            child,
            stdin,
            rx,
            done: false,
        })
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Sends one JSON line (a dead worker's broken pipe is ignored: the
    /// supervisor sees its end on stdout).
    pub fn send(&mut self, v: &serde_json::Value) {
        if let Some(i) = self.stdin.as_mut() {
            let mut l = v.to_string();
            l.push('\n');
            if i.write_all(l.as_bytes()).and_then(|_| i.flush()).is_err() {
                self.stdin = None;
            }
        }
    }

    pub fn recv(&self, d: Duration) -> Option<Out> {
        match self.rx.recv_timeout(d) {
            Ok(o) => Some(o),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Out::Eof),
        }
    }

    /// The exit status once it has exited on its own.
    pub fn exit_status(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// SIGKILL to the whole process group, then reap. Idempotent.
    pub fn kill(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        self.stdin = None;
        #[cfg(unix)]
        {
            // The child leads its own group (pgid = pid). Under bwrap this
            // reaches only bwrap's own group: `--new-session` puts the
            // sandboxed worker in a new session (and so a new group) inside
            // a new pid namespace. The worker dies anyway: bwrap runs it
            // with `--die-with-parent` (PR_SET_PDEATHSIG on the inner
            // process when bwrap goes) and killing bwrap, the pid
            // namespace's init, kills everything in that namespace.
            let pgid = self.child.id() as libc::pid_t;
            // SAFETY: plain syscall; a stale pgid only fails with ESRCH.
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for WorkerProc {
    fn drop(&mut self) {
        self.kill();
    }
}

fn read_lines(out: std::process::ChildStdout, tx: &std::sync::mpsc::Sender<Out>) {
    use std::io::Read;
    let mut r = std::io::BufReader::new(out);
    loop {
        let mut buf = Vec::new();
        match (&mut r)
            .take(MAX_LINE as u64 + 1)
            .read_until(b'\n', &mut buf)
        {
            Ok(0) => {
                let _ = tx.send(Out::Eof);
                return;
            }
            Ok(_) => {
                if buf.last() != Some(&b'\n') && buf.len() > MAX_LINE {
                    let _ = tx.send(Out::Oversized);
                    return;
                }
                let l = String::from_utf8_lossy(&buf).trim_end().to_string();
                if tx.send(Out::Line(l)).is_err() {
                    return;
                }
            }
            Err(_) => {
                let _ = tx.send(Out::Eof);
                return;
            }
        }
    }
}

#[cfg(unix)]
fn harden(c: &mut Command) {
    use std::os::unix::process::CommandExt;
    c.process_group(0);
    let parent = std::process::id();
    // SAFETY: the closure only makes async-signal-safe syscalls (prctl,
    // getppid, setpriority, syscall, _exit) between fork and exec.
    unsafe {
        c.pre_exec(move || child_setup(parent));
    }
}

#[cfg(not(unix))]
fn harden(_c: &mut Command) {}

#[cfg(unix)]
fn child_setup(parent: u32) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    // SAFETY: async-signal-safe syscalls on the forked child only.
    unsafe {
        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // The parent may have died before the prctl: then go now.
        if libc::getppid() as u32 != parent {
            libc::_exit(1);
        }
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // IOPRIO_WHO_PROCESS = 1, IOPRIO_CLASS_IDLE << 13; best effort.
        libc::syscall(
            libc::SYS_ioprio_set,
            1 as libc::c_long,
            0 as libc::c_long,
            (3 << 13) as libc::c_long,
        );
    }
    let _ = parent;
    // SAFETY: plain syscall on this (child) process.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 19);
    }
    Ok(())
}

/// Installs a seccomp filter on the calling thread that makes `execve` and
/// `execveat` fail with `EPERM`, and kills the process on any other
/// architecture's syscall (a foreign `AUDIT_ARCH`, e.g. i386 via `int 0x80`)
/// or an x86_64 x32-ABI syscall (`nr` with bit 30 set), so neither can
/// reach exec under another number. The worker binary calls this right
/// after it starts; nothing it does later can run a program. Linux x86_64 /
/// aarch64 only.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
pub fn deny_exec_after_start() -> std::io::Result<()> {
    let mut prog = exec_filter();
    let fprog = libc::sock_fprog {
        len: prog.len() as libc::c_ushort,
        filter: prog.as_mut_ptr(),
    };
    // SAFETY: `fprog` points at a live, well-formed filter for the call.
    unsafe {
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::prctl(
            libc::PR_SET_SECCOMP,
            libc::SECCOMP_MODE_FILTER,
            &fprog as *const libc::sock_fprog,
        ) != 0
        {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// `SECCOMP_RET_ALLOW`.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const RET_ALLOW: u32 = 0x7fff_0000;
/// `SECCOMP_RET_ERRNO` (the errno goes in the low 16 bits).
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const RET_ERRNO: u32 = 0x0005_0000;
/// `SECCOMP_RET_KILL_PROCESS`.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const RET_KILL_PROCESS: u32 = 0x8000_0000;

/// The BPF program [`deny_exec_after_start`] installs.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
fn exec_filter() -> Vec<libc::sock_filter> {
    // BPF_LD | BPF_W | BPF_ABS (BPF_LD = BPF_W = 0).
    const LD_W_ABS: u16 = 0x20;
    const JEQ_K: u16 = 0x15; // BPF_JMP | BPF_JEQ | BPF_K
    const JGE_K: u16 = 0x35; // BPF_JMP | BPF_JGE | BPF_K
    const RET_K: u16 = 0x06;
    #[cfg(target_arch = "x86_64")]
    const ARCH: u32 = 0xC000_003E;
    #[cfg(target_arch = "aarch64")]
    const ARCH: u32 = 0xC000_00B7;
    // x86_64: x32 syscalls carry bit 30; aarch64 has no such ABI.
    #[cfg(target_arch = "x86_64")]
    const FOREIGN: u32 = 0x4000_0000;
    #[cfg(target_arch = "aarch64")]
    const FOREIGN: u32 = u32::MAX;
    let f = |code, jt, jf, k| libc::sock_filter { code, jt, jf, k };
    // Jump offsets count from the next instruction.
    vec![
        f(LD_W_ABS, 0, 0, 4),                           // 0: arch
        f(JEQ_K, 1, 0, ARCH),                           // 1: ours → 3
        f(RET_K, 0, 0, RET_KILL_PROCESS),               // 2: foreign arch → kill
        f(LD_W_ABS, 0, 0, 0),                           // 3: nr
        f(JGE_K, 0, 1, FOREIGN),                        // 4: x32 → 5, else 6
        f(RET_K, 0, 0, RET_KILL_PROCESS),               // 5: x32 ABI → kill
        f(JEQ_K, 2, 0, libc::SYS_execve as u32),        // 6: → 9
        f(JEQ_K, 1, 0, libc::SYS_execveat as u32),      // 7: → 9
        f(RET_K, 0, 0, RET_ALLOW),                      // 8
        f(RET_K, 0, 0, RET_ERRNO | libc::EPERM as u32), // 9
    ]
}

/// Elsewhere there is no seccomp: helper workers stay off.
#[cfg(not(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
)))]
pub fn deny_exec_after_start() -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no seccomp on this platform",
    ))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::time::Instant;

    /// A tiny classic-BPF interpreter for the instructions the filter uses.
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    fn run_bpf(prog: &[libc::sock_filter], arch: u32, nr: u32) -> u32 {
        let (mut acc, mut pc) = (0u32, 0usize);
        loop {
            let i = prog[pc];
            let jump = |taken: bool| 1 + usize::from(if taken { i.jt } else { i.jf });
            match i.code {
                0x20 => {
                    acc = match i.k {
                        0 => nr,
                        4 => arch,
                        k => panic!("load of offset {k}"),
                    };
                    pc += 1;
                }
                0x15 => pc += jump(acc == i.k),
                0x35 => pc += jump(acc >= i.k),
                0x06 => return i.k,
                c => panic!("opcode {c:#x}"),
            }
        }
    }

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    #[test]
    fn seccomp_filter_denies_exec_and_kills_foreign_abis() {
        let prog = exec_filter();
        #[cfg(target_arch = "x86_64")]
        let arch = 0xC000_003E;
        #[cfg(target_arch = "aarch64")]
        let arch = 0xC000_00B7;
        let eperm = RET_ERRNO | libc::EPERM as u32;
        assert_eq!(run_bpf(&prog, arch, libc::SYS_execve as u32), eperm);
        assert_eq!(run_bpf(&prog, arch, libc::SYS_execveat as u32), eperm);
        assert_eq!(run_bpf(&prog, arch, libc::SYS_read as u32), RET_ALLOW);
        assert_eq!(run_bpf(&prog, arch, libc::SYS_write as u32), RET_ALLOW);
        // i386 (AUDIT_ARCH_I386) syscalls: killed, whatever the number.
        assert_eq!(run_bpf(&prog, 0x4000_0003, 11), RET_KILL_PROCESS);
        assert_eq!(run_bpf(&prog, 0x4000_0003, 3), RET_KILL_PROCESS);
        #[cfg(target_arch = "x86_64")]
        {
            // x32 execve (520 | __X32_SYSCALL_BIT) and any other x32 call.
            assert_eq!(run_bpf(&prog, arch, 0x4000_0208), RET_KILL_PROCESS);
            assert_eq!(run_bpf(&prog, arch, 0x4000_0000), RET_KILL_PROCESS);
        }
    }

    fn sh(script: &str) -> WorkerCommand {
        WorkerCommand::unsandboxed("/bin/sh", vec!["-c".into(), script.into()])
    }

    fn lines(p: &WorkerProc) -> Vec<String> {
        let mut v = Vec::new();
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(5) {
            match p.recv(Duration::from_millis(50)) {
                Some(Out::Line(l)) => v.push(l),
                Some(_) => break,
                None => {}
            }
        }
        v
    }

    fn alive(pid: u32) -> bool {
        // A zombie (killed, not yet reaped by init) counts as dead.
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .map(|s| {
                let state = s.rsplit(')').next().unwrap_or("").trim_start();
                !state.starts_with('Z') && !state.starts_with('X')
            })
            .unwrap_or(false)
    }

    fn wait_dead(pid: u32) -> bool {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(3) {
            if !alive(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn worker_env_is_minimal_cwd_is_scratch_and_nice_19() {
        let s = super::super::scratch::Scratch::new("env").unwrap();
        let p = WorkerProc::spawn(
            &sh("env | sort; pwd; cut -d' ' -f19 /proc/self/stat"),
            s.path(),
            None,
        )
        .unwrap();
        let out = lines(&p);
        let keys: Vec<&str> = out
            .iter()
            .filter_map(|l| l.split_once('=').map(|x| x.0))
            .collect();
        for k in &keys {
            assert!(
                ["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL", "TZ", "PWD", "SHLVL", "_", "OLDPWD"]
                    .contains(k),
                "unexpected env {k}: {out:?}"
            );
        }
        assert!(out.iter().any(|l| l == "TZ=UTC"), "{out:?}");
        assert!(
            out.iter().any(|l| l == &s.path().display().to_string()),
            "cwd: {out:?}"
        );
        assert_eq!(
            out.last().map(String::as_str),
            Some("19"),
            "nice 19: {out:?}"
        );
    }

    #[test]
    fn kill_takes_the_whole_process_group() {
        let s = super::super::scratch::Scratch::new("pg").unwrap();
        let mut p = WorkerProc::spawn(&sh("sleep 30 & echo $!; wait"), s.path(), None).unwrap();
        let grandchild: u32 = loop {
            if let Some(Out::Line(l)) = p.recv(Duration::from_secs(5)) {
                break l.trim().parse().unwrap();
            }
        };
        let pid = p.id();
        assert!(alive(grandchild));
        p.kill();
        assert!(wait_dead(pid), "worker dead");
        assert!(wait_dead(grandchild), "its background child is dead too");
    }

    #[test]
    fn drop_kills_the_worker() {
        let s = super::super::scratch::Scratch::new("drop").unwrap();
        let p = WorkerProc::spawn(&sh("sleep 30"), s.path(), None).unwrap();
        let pid = p.id();
        drop(p);
        assert!(wait_dead(pid));
    }

    #[test]
    fn worker_dies_with_the_parent_thread_pdeathsig() {
        let s = super::super::scratch::Scratch::new("pdeath").unwrap();
        let path = s.path().to_path_buf();
        // Spawn from a thread that then exits without killing it: the
        // kernel's PR_SET_PDEATHSIG delivers SIGKILL. `forget` skips Drop.
        let pid = std::thread::spawn(move || {
            let p = WorkerProc::spawn(&sh("sleep 30"), &path, None).unwrap();
            let pid = p.id();
            std::mem::forget(p);
            pid
        })
        .join()
        .unwrap();
        assert!(wait_dead(pid), "worker outlived its parent thread");
    }

    #[test]
    fn oversized_line_is_reported() {
        let s = super::super::scratch::Scratch::new("big").unwrap();
        let p =
            WorkerProc::spawn(&sh("head -c 70000 /dev/zero | tr '\\0' a"), s.path(), None).unwrap();
        let t = Instant::now();
        let mut got = None;
        while t.elapsed() < Duration::from_secs(5) && got.is_none() {
            got = p.recv(Duration::from_millis(50));
        }
        assert_eq!(got, Some(Out::Oversized));
    }

    #[test]
    fn seccomp_denies_exec() {
        use std::os::unix::process::CommandExt;
        let mut c = Command::new("/bin/true");
        // SAFETY: prctl only, between fork and exec.
        unsafe {
            c.pre_exec(deny_exec_after_start);
        }
        let e = c.spawn().expect_err("exec must be denied");
        assert_eq!(e.raw_os_error(), Some(libc::EPERM), "{e}");
        // Without the filter the same program runs.
        assert!(Command::new("/bin/true").status().unwrap().success());
    }

    #[test]
    fn sandboxed_worker_needs_bwrap_and_argv_is_locked_down() {
        let s = super::super::scratch::Scratch::new("bw").unwrap();
        let cmd = WorkerCommand::sandboxed("/opt/lillith/lillith", vec!["learn-worker".into()]);
        assert!(cmd.is_sandboxed());
        let e = WorkerProc::spawn(&cmd, s.path(), None)
            .err()
            .expect("no bwrap → no worker");
        assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
        let a = bwrap_argv(Path::new("/usr/bin/bwrap"), &cmd, Path::new("/dev/shm/w"));
        for need in [
            "--unshare-all",
            "--die-with-parent",
            "--new-session",
            "--clearenv",
        ] {
            assert!(a.iter().any(|x| x == need), "{need}");
        }
        assert!(
            !a.iter().any(|x| x == "--share-net"),
            "no network namespace"
        );
        // Only scratch is bound writable; the binary read-only.
        let binds: Vec<_> = a
            .iter()
            .enumerate()
            .filter(|(_, x)| *x == "--bind")
            .collect();
        assert_eq!(binds.len(), 1);
        assert_eq!(a[binds[0].0 + 1], "/dev/shm/w");
        assert_eq!(a[binds[0].0 + 2], SANDBOX_SCRATCH);
        let i = a.iter().position(|x| x == "--").unwrap();
        assert_eq!(&a[i + 1..], ["/opt/lillith/lillith", "learn-worker"]);
        assert!(a
            .windows(3)
            .any(|w| w == ["--ro-bind", "/opt/lillith/lillith", "/opt/lillith/lillith"]));
        assert!(
            !a.iter().any(|x| x.contains("/home")),
            "no home dir in the sandbox"
        );
    }
}

/// How one sandboxed step ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxEnd {
    /// It exited (or was killed by a signal from inside: `timeout`, a
    /// limit). `code` is the exit code, `signal` the killing signal.
    Exited {
        code: Option<i32>,
        signal: Option<i32>,
        stdout: String,
        stderr: String,
        /// Output past the cap was dropped.
        truncated: bool,
    },
    /// A stop reason fired: the whole process group was killed and the
    /// output discarded.
    Stopped(super::StopReason),
    /// It outlived the parent's own hard wall (the in-sandbox `timeout`
    /// should have fired first): killed, output discarded.
    WallKilled,
    /// The watchdog saw its folder over the size cap: the whole group was
    /// killed the moment it did, output discarded.
    OverCap,
}

/// Runs one fully built sandbox argv (`bwrap … -- prlimit … timeout …`):
/// its own process group, `PR_SET_PDEATHSIG`, `PR_SET_NO_NEW_PRIVS`, nice 19
/// and the idle I/O class (inherited by everything inside), stdin closed,
/// stdout / stderr kept up to `out_cap` bytes each. `stop` is polled every
/// 20 ms: a reason kills the whole group (SIGKILL) and discards the output;
/// so does running past `hard_wall`. Only [`super::build`] calls this, and
/// only with an argv that starts with `bwrap`. Tests only: production
/// always goes through [`run_sandboxed_watched`] (the size watchdog).
#[cfg(test)]
pub(crate) fn run_sandboxed(
    argv: &[String],
    cwd: &Path,
    out_cap: usize,
    hard_wall: Duration,
    stop: &dyn Fn() -> Option<super::StopReason>,
) -> std::io::Result<SandboxEnd> {
    run_sandboxed_watched(argv, cwd, out_cap, hard_wall, stop, &|| false)
}

/// [`run_sandboxed`] plus a watchdog: `over_cap` is polled with `stop`
/// (every 20 ms); `true` kills the whole group at once
/// ([`SandboxEnd::OverCap`]).
pub(crate) fn run_sandboxed_watched(
    argv: &[String],
    cwd: &Path,
    out_cap: usize,
    hard_wall: Duration,
    stop: &dyn Fn() -> Option<super::StopReason>,
    over_cap: &dyn Fn() -> bool,
) -> std::io::Result<SandboxEnd> {
    let Some((prog, args)) = argv.split_first() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "empty argv",
        ));
    };
    if Path::new(prog).file_name().is_none_or(|n| n != "bwrap") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "only a bwrap argv runs here (never a weaker sandbox)",
        ));
    }
    let mut c = Command::new(prog);
    c.args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C")
        .env("TZ", "UTC")
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    harden(&mut c);
    let mut child = c.spawn()?;
    super::count_sandbox_launch();
    let start = std::time::Instant::now();
    let (tx, rx) = std::sync::mpsc::channel::<(bool, Vec<u8>, bool)>();
    if let Some(o) = child.stdout.take() {
        let tx = tx.clone();
        let _ = std::thread::Builder::new()
            .name("lillith-sandbox-out".into())
            .spawn(move || {
                let (b, t) = read_capped(o, out_cap);
                let _ = tx.send((true, b, t));
            });
    }
    if let Some(e) = child.stderr.take() {
        let tx = tx.clone();
        let _ = std::thread::Builder::new()
            .name("lillith-sandbox-err".into())
            .spawn(move || {
                let (b, t) = read_capped(e, out_cap);
                let _ = tx.send((false, b, t));
            });
    }
    drop(tx);
    let kill = |child: &mut Child| {
        #[cfg(unix)]
        // SAFETY: plain syscall; the child leads its own group (pgid = pid).
        unsafe {
            libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
        }
        let _ = child.kill();
        let _ = child.wait();
    };
    let status = loop {
        if let Some(r) = stop() {
            kill(&mut child);
            return Ok(SandboxEnd::Stopped(r));
        }
        if start.elapsed() >= hard_wall {
            kill(&mut child);
            return Ok(SandboxEnd::WallKilled);
        }
        if over_cap() {
            kill(&mut child);
            return Ok(SandboxEnd::OverCap);
        }
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                kill(&mut child);
                return Err(e);
            }
        }
    };
    // Anything left in the group (there should be nothing: the pid
    // namespace died with bwrap) goes too.
    #[cfg(unix)]
    // SAFETY: as above; a gone group only fails with ESRCH.
    unsafe {
        libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
    }
    let (mut out, mut err, mut truncated) = (Vec::new(), Vec::new(), false);
    // The readers finish when the pipes close, which is right away once
    // bwrap and its pid namespace are gone. The 5 s bound is only for a
    // pipe some escaped holder still keeps open: we stop waiting (and keep
    // what was read) instead of hanging the step; the reader thread exits
    // on its own when that holder dies.
    for _ in 0..2 {
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok((true, b, t)) => {
                out = b;
                truncated |= t;
            }
            Ok((false, b, t)) => {
                err = b;
                truncated |= t;
            }
            Err(_) => break,
        }
    }
    #[cfg(unix)]
    let signal = std::os::unix::process::ExitStatusExt::signal(&status);
    #[cfg(not(unix))]
    let signal = None;
    Ok(SandboxEnd::Exited {
        code: status.code(),
        signal,
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
        truncated,
    })
}

/// Reads `r` to its end, keeping the first `cap` bytes (the rest is read
/// and dropped so the child never blocks on a full pipe).
fn read_capped(mut r: impl std::io::Read, cap: usize) -> (Vec<u8>, bool) {
    let mut kept = Vec::new();
    let mut buf = [0u8; 8192];
    let mut truncated = false;
    loop {
        match r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = cap.saturating_sub(kept.len());
                if n > room {
                    truncated = true;
                }
                kept.extend_from_slice(&buf[..n.min(room)]);
            }
        }
    }
    (kept, truncated)
}
