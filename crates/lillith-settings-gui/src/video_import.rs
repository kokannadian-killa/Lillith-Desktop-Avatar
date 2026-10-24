//! Pose Builder → Import Video: the plumbing behind the Import tab.
//!
//! The pipeline itself is `tools/video-pipeline/user_import.py` (built
//! separately); this module runs it as a child process and speaks its
//! contract:
//!
//! ```text
//! python3 tools/video-pipeline/user_import.py --input <file> --out <staging_dir> \
//!     --json-progress [--max-ram-mb 1500]
//! ```
//!
//! stdout is JSON lines:
//! `{"event":"progress","stage","pct","msg"}`,
//! `{"event":"candidate","id","mp4","preview_png","frames","fps","qa":{"pass","white_edge","fringe"},"suggested_name"}`,
//! `{"event":"done","candidates"}`, `{"event":"error","msg"}`. Anything
//! else on stdout is kept as a log line, never fatal. Exit 0 = ok, 1 =
//! error, 130 = cancelled (the pipeline then deletes everything that run
//! created). Cancel is `SIGTERM` (Linux) / `CTRL_BREAK_EVENT` to the
//! child's process group (Windows, started with `CREATE_NEW_PROCESS_GROUP`).
//! A Linux pipeline still running after [`CANCEL_GRACE`] is SIGKILLed with
//! its process group; on Windows the break is re-sent, never
//! TerminateProcess.
//!
//! The input file is never read here: only its path is passed on.
//! Staging is `<data>/lillith/import-staging/<job>/`; candidate files must
//! resolve inside it ([`resolve_in_staging`]) before anything is copied.

use serde_json::Value;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// File types the drop zone / picker accept (any size).
pub const ACCEPTED_EXT: &[&str] = &["mp4", "gif", "webm", "mov"];
/// `--max-ram-mb` passed to the pipeline.
pub const DEFAULT_MAX_RAM_MB: u32 = 1500;
/// `--threads` passed to the pipeline (always 2: the box stays usable).
pub const THREADS: u32 = 2;
/// Time the pipeline gets to exit after the cancel signal before it is
/// killed outright.
pub const CANCEL_GRACE: Duration = Duration::from_secs(5);
/// Exit code of a cancelled run.
pub const EXIT_CANCELLED: i32 = 130;
/// The script, relative to the repo / install root.
pub const SCRIPT_REL: &str = "tools/video-pipeline/user_import.py";
/// Longest stdout line kept (a broken script can't eat memory).
const MAX_LINE: usize = 64 * 1024;
/// stderr lines kept for the error message.
const STDERR_TAIL: usize = 20;

/// Whether `p` has an accepted video extension.
pub fn accepted(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ACCEPTED_EXT.iter().any(|a| a.eq_ignore_ascii_case(e)))
}

/// QA verdict of a candidate. `white_edge` / `fringe` are shown as the
/// pipeline reports them (number, bool or text).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Qa {
    pub pass: bool,
    pub white_edge: String,
    pub fringe: String,
}

/// One clip the pipeline proposes.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub mp4: PathBuf,
    pub preview_png: PathBuf,
    pub frames: u32,
    pub fps: f32,
    pub qa: Qa,
    pub suggested_name: String,
}

/// One stdout line of the pipeline.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Progress {
        stage: String,
        /// 0–100 (clamped).
        pct: f32,
        msg: String,
    },
    Candidate(Candidate),
    Done {
        candidates: u32,
        /// Peak memory of the run, when reported.
        peak_rss_mb: Option<u64>,
    },
    Error {
        msg: String,
    },
    /// Valid JSON with an unknown or incomplete event: logged.
    Unknown(String),
    /// Not a JSON object (a stray print, a traceback): logged.
    Malformed(String),
}

fn text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Parses one stdout line. Never fails: bad lines come back as
/// [`Event::Malformed`] / [`Event::Unknown`].
pub fn parse_line(line: &str) -> Event {
    let line = line.trim();
    let v: Value = match serde_json::from_str(line) {
        Ok(v @ Value::Object(_)) => v,
        _ => return Event::Malformed(line.to_string()),
    };
    let s = |k: &str| v.get(k).map(text).unwrap_or_default();
    let unknown = || Event::Unknown(line.to_string());
    match v.get("event").and_then(Value::as_str) {
        Some("progress") => Event::Progress {
            stage: s("stage"),
            pct: v
                .get("pct")
                .and_then(Value::as_f64)
                .filter(|p| p.is_finite())
                .unwrap_or(0.0)
                .clamp(0.0, 100.0) as f32,
            msg: s("msg"),
        },
        Some("candidate") => {
            let (Some(mp4), Some(png)) = (
                v.get("mp4")
                    .and_then(Value::as_str)
                    .filter(|p| !p.is_empty()),
                v.get("preview_png")
                    .and_then(Value::as_str)
                    .filter(|p| !p.is_empty()),
            ) else {
                return unknown();
            };
            let Some(frames) = v
                .get("frames")
                .and_then(Value::as_u64)
                .filter(|&n| n > 0 && n <= 100_000)
            else {
                return unknown();
            };
            let qa = v.get("qa").cloned().unwrap_or(Value::Null);
            let id = s("id");
            Event::Candidate(Candidate {
                id: if id.is_empty() { mp4.to_string() } else { id },
                mp4: PathBuf::from(mp4),
                preview_png: PathBuf::from(png),
                frames: frames as u32,
                fps: v
                    .get("fps")
                    .and_then(Value::as_f64)
                    .filter(|f| f.is_finite() && *f > 0.0)
                    .unwrap_or(12.0) as f32,
                qa: Qa {
                    pass: qa.get("pass").and_then(Value::as_bool).unwrap_or(false),
                    white_edge: qa.get("white_edge").map(text).unwrap_or_default(),
                    fringe: qa.get("fringe").map(text).unwrap_or_default(),
                },
                suggested_name: s("suggested_name"),
            })
        }
        Some("done") => Event::Done {
            candidates: v.get("candidates").and_then(Value::as_u64).unwrap_or(0) as u32,
            peak_rss_mb: v.get("peak_rss_mb").and_then(Value::as_u64),
        },
        Some("error") => Event::Error { msg: s("msg") },
        _ => unknown(),
    }
}

/// `<data>/lillith/import-staging` (Linux `~/.local/share/lillith/...`,
/// Windows `%LOCALAPPDATA%\lillith\...`).
pub fn staging_root() -> Option<PathBuf> {
    lillith_core::config::data_dir().map(|d| d.join("import-staging"))
}

/// A fresh job id: `job-<unix ms>-<pid>`.
pub fn new_job_id() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("job-{ms}-{}", std::process::id())
}

/// Creates `<root>/<job>` (must not exist yet).
pub fn create_staging(root: &Path, job: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    let d = root.join(job);
    std::fs::create_dir(&d)?;
    Ok(d)
}

/// Removes a job's staging folder, but only a direct `job-*` child of
/// `root` (never anything else).
pub fn remove_staging(root: &Path, dir: &Path) -> std::io::Result<()> {
    let ok = dir.parent() == Some(root)
        && dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("job-") && !n.contains(".."));
    if !ok {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} is not a staging folder", dir.display()),
        ));
    }
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

/// `p` (relative paths are relative to `staging`) as a real file inside
/// `staging`; `None` for anything outside it (`..`, symlinks out, absolute
/// paths elsewhere) or missing.
pub fn resolve_in_staging(staging: &Path, p: &Path) -> Option<PathBuf> {
    let full = if p.is_absolute() {
        p.to_path_buf()
    } else {
        staging.join(p)
    };
    let real = full.canonicalize().ok()?;
    let root = staging.canonicalize().ok()?;
    (real.starts_with(&root) && real.is_file()).then_some(real)
}

/// Finds `name` on `PATH` (with `PATHEXT` on Windows). No program is run.
pub fn which(name: &str) -> Option<PathBuf> {
    which_in(name, std::env::var_os("PATH").as_deref())
}

/// [`which`] over an explicit `PATH` value.
pub fn which_in(name: &str, path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.components().count() > 1 {
        return p.is_file().then(|| p.to_path_buf());
    }
    let exts: Vec<String> = if cfg!(windows) {
        let pe = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.BAT;.CMD".into());
        std::iter::once(String::new())
            .chain(pe.split(';').map(str::to_string))
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(path?).find_map(|d| {
        exts.iter()
            .map(|e| d.join(format!("{name}{e}")))
            .find(|c| c.is_file())
    })
}

/// The Python to run the pipeline with: `$LILLITH_PYTHON`, else `python3`
/// (Windows: `python`, then `python3`) on `PATH`.
pub fn find_python() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("LILLITH_PYTHON").filter(|v| !v.is_empty()) {
        return which(&p.to_string_lossy());
    }
    let names: &[&str] = if cfg!(windows) {
        &["python", "python3"]
    } else {
        &["python3", "python"]
    };
    names.iter().find_map(|n| which(n))
}

/// Where a media tool (`ffmpeg` / `ffprobe`) is looked for, in order:
/// 1. an explicit override (`$LILLITH_FFMPEG` / `$LILLITH_FFPROBE`),
/// 2. bundled next to the program: `<exe dir>/bin/<tool>` (the Windows
///    release zip ships `bin/ffmpeg.exe` + `bin/ffprobe.exe`), then
///    `<exe dir>/<tool>`,
/// 3. `PATH`.
///
/// No program is run to find it.
pub fn find_tool_in(
    tool: &str,
    override_path: Option<&std::ffi::OsStr>,
    exe_dir: Option<&Path>,
    path_env: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    if let Some(o) = override_path.filter(|v| !v.is_empty()) {
        let p = PathBuf::from(o);
        if p.components().count() > 1 {
            return p.is_file().then_some(p);
        }
        return which_in(&p.to_string_lossy(), path_env);
    }
    let file = format!("{tool}{}", std::env::consts::EXE_SUFFIX);
    if let Some(d) = exe_dir {
        for c in [d.join("bin").join(&file), d.join(&file)] {
            if c.is_file() {
                return Some(c);
            }
        }
    }
    which_in(tool, path_env)
}

/// The bundled copy of `tool` shipped in the install's `bin/`
/// (`<exe dir>/bin/ffmpeg(.exe)`), only if that file exists. Passed to the
/// pipeline as `--ffmpeg` / `--ffprobe`; without it the pipeline does its
/// own lookup (`LILLITH_FFMPEG` / `LILLITH_FFPROBE`, ffprobe next to ffmpeg,
/// then `PATH`).
pub fn bundled_tool_in(tool: &str, exe_dir: Option<&Path>) -> Option<PathBuf> {
    let p = exe_dir?
        .join("bin")
        .join(format!("{tool}{}", std::env::consts::EXE_SUFFIX));
    p.is_file().then_some(p)
}

/// [`bundled_tool_in`] for this program.
pub fn bundled_tool(tool: &str) -> Option<PathBuf> {
    bundled_tool_in(tool, exe_dir().as_deref())
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
}

fn find_media_tool(tool: &str, env: &str) -> Option<PathBuf> {
    find_tool_in(
        tool,
        std::env::var_os(env).as_deref(),
        exe_dir().as_deref(),
        std::env::var_os("PATH").as_deref(),
    )
}

/// ffmpeg for the pipeline (see [`find_tool_in`] for the order).
pub fn find_ffmpeg() -> Option<PathBuf> {
    find_media_tool("ffmpeg", "LILLITH_FFMPEG")
}

/// ffprobe for the pipeline (see [`find_tool_in`] for the order); like the
/// pipeline, also accepts one next to the ffmpeg that was found.
pub fn find_ffprobe() -> Option<PathBuf> {
    find_media_tool("ffprobe", "LILLITH_FFPROBE").or_else(|| {
        let ff = find_ffmpeg()?;
        let p = ff
            .parent()?
            .join(format!("ffprobe{}", std::env::consts::EXE_SUFFIX));
        p.is_file().then_some(p)
    })
}

/// The pipeline script: `$LILLITH_VIDEO_IMPORT`, then next to the binary
/// (`<exe dir>/tools/...`, `<exe dir>/../share/lillith/tools/...`), then the
/// working directory, then this source tree.
pub fn find_script() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("LILLITH_VIDEO_IMPORT").filter(|v| !v.is_empty()) {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let mut c = Vec::new();
    if let Some(dir) = exe_dir() {
        c.push(dir.join(SCRIPT_REL));
        c.push(dir.join("../share/lillith").join(SCRIPT_REL));
        c.push(dir.join("../..").join(SCRIPT_REL));
    }
    c.push(PathBuf::from(SCRIPT_REL));
    c.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(SCRIPT_REL),
    );
    c.into_iter().find(|p| p.is_file())
}

/// What's missing to run an import, as a message for Ko (`None`: ready).
pub fn missing_tools(
    python: Option<&Path>,
    ffmpeg: Option<&Path>,
    ffprobe: Option<&Path>,
    script: Option<&Path>,
) -> Option<String> {
    let mut miss = Vec::new();
    if python.is_none() {
        miss.push(if cfg!(windows) {
            "Python 3 isn't installed (or not on PATH). Install it from python.org and tick \"Add python.exe to PATH\", or set LILLITH_PYTHON."
        } else {
            "Python 3 isn't installed (or not on PATH). Install python3 (Arch: sudo pacman -S python), or set LILLITH_PYTHON."
        });
    }
    if ffmpeg.is_none() || ffprobe.is_none() {
        miss.push(if cfg!(windows) {
            "ffmpeg / ffprobe weren't found. The release zip has them in bin\\ next to Lillith; otherwise install ffmpeg (winget install ffmpeg) or set LILLITH_FFMPEG / LILLITH_FFPROBE."
        } else {
            "ffmpeg / ffprobe weren't found. Install ffmpeg (Arch/Garuda: sudo pacman -S --needed ffmpeg), put them in bin/ next to Lillith, or set LILLITH_FFMPEG / LILLITH_FFPROBE."
        });
    }
    if script.is_none() {
        miss.push("The import pipeline (tools/video-pipeline/user_import.py) isn't in this install. Set LILLITH_VIDEO_IMPORT to its path.");
    }
    (!miss.is_empty()).then(|| miss.join("\n"))
}

/// One pipeline run, fully specified (what the Soul Contract card shows).
#[derive(Debug, Clone, PartialEq)]
pub struct ImportCmd {
    pub python: PathBuf,
    pub script: PathBuf,
    pub input: PathBuf,
    pub out: PathBuf,
    pub max_ram_mb: u32,
    /// Bundled `bin/ffmpeg(.exe)` only; `None` lets the pipeline look.
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
    /// `--matte auto|isnet|key|alpha`.
    pub matte: Matte,
    /// `--max-candidates` (0 = no limit).
    pub max_candidates: u32,
}

/// Background removal mode of the pipeline (`--matte`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Matte {
    #[default]
    Auto,
    Isnet,
    Key,
    Alpha,
}

impl Matte {
    pub const ALL: [Matte; 4] = [Matte::Auto, Matte::Isnet, Matte::Key, Matte::Alpha];
    pub fn as_str(self) -> &'static str {
        match self {
            Matte::Auto => "auto",
            Matte::Isnet => "isnet",
            Matte::Key => "key",
            Matte::Alpha => "alpha",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Matte::Auto => "auto (best guess)",
            Matte::Isnet => "isnet (AI cut-out)",
            Matte::Key => "key (plain background)",
            Matte::Alpha => "alpha (video has transparency)",
        }
    }
}

impl ImportCmd {
    pub fn argv(&self) -> Vec<String> {
        let mut v = vec![
            self.python.display().to_string(),
            self.script.display().to_string(),
            "--input".into(),
            self.input.display().to_string(),
            "--out".into(),
            self.out.display().to_string(),
            "--json-progress".into(),
            "--max-ram-mb".into(),
            self.max_ram_mb.to_string(),
            "--threads".into(),
            THREADS.to_string(),
        ];
        if self.matte != Matte::Auto {
            v.extend(["--matte".into(), self.matte.as_str().into()]);
        }
        if self.max_candidates > 0 {
            v.extend(["--max-candidates".into(), self.max_candidates.to_string()]);
        }
        for (flag, p) in [("--ffmpeg", &self.ffmpeg), ("--ffprobe", &self.ffprobe)] {
            if let Some(p) = p {
                v.extend([flag.into(), p.display().to_string()]);
            }
        }
        v
    }

    /// The argv as one shell-quoted line (for the card / audit log only;
    /// the process is spawned from the argv, never through a shell).
    pub fn display(&self) -> String {
        self.argv()
            .iter()
            .map(|a| {
                if !a.is_empty()
                    && a.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_./:=+@%,\\".contains(c))
                {
                    a.clone()
                } else {
                    format!("'{}'", a.replace('\'', "'\\''"))
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The child process (unbuffered stdout, own process group).
    pub fn command(&self) -> Command {
        let argv = self.argv();
        let mut c = Command::new(&argv[0]);
        c.args(&argv[1..]).env("PYTHONUNBUFFERED", "1");
        c
    }
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Ok,
    Cancelled,
    /// Exit code (or a description) plus the stderr tail.
    Failed(String),
}

/// Messages from a running job.
#[derive(Debug, Clone, PartialEq)]
pub enum JobMsg {
    Event(Event),
    /// The process ended (sent after all stdout was delivered).
    Exited(Outcome),
}

/// A running pipeline.
pub struct Job {
    /// Shared with the waiter thread. Only Linux's last-resort kill touches
    /// it here: Windows never force-terminates (CTRL_BREAK only).
    #[cfg_attr(windows, allow(dead_code))]
    child: Arc<Mutex<Child>>,
    pid: u32,
    rx: Receiver<JobMsg>,
    pub staging: PathBuf,
    cancel_sent: Option<Instant>,
    killed: bool,
    finished: bool,
}

impl Job {
    /// Spawns `cmd` with piped stdout/stderr and starts the reader threads.
    pub fn spawn(
        mut cmd: Command,
        staging: PathBuf,
        notify: impl Fn() + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Own group: a hard kill takes its ffmpeg children too.
            cmd.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn()?;
        let pid = child.id();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let child = Arc::new(Mutex::new(child));
        let (tx, rx) = channel();
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(notify);
        let out_t = stdout.map(|o| {
            let (tx, n) = (tx.clone(), Arc::clone(&notify));
            std::thread::spawn(move || read_events(o, &tx, &*n))
        });
        let tail = Arc::new(Mutex::new(Vec::<String>::new()));
        let err_t = stderr.map(|e| {
            let tail = Arc::clone(&tail);
            std::thread::spawn(move || {
                let mut r = BufReader::new(e);
                let mut buf = Vec::new();
                while matches!(r.read_until(b'\n', &mut buf), Ok(n) if n > 0) {
                    let l = String::from_utf8_lossy(&buf[..buf.len().min(MAX_LINE)])
                        .trim_end()
                        .to_string();
                    buf.clear();
                    if let Ok(mut t) = tail.lock() {
                        t.push(l);
                        let n = t.len();
                        if n > STDERR_TAIL {
                            t.drain(..n - STDERR_TAIL);
                        }
                    }
                }
            })
        });
        let wchild = Arc::clone(&child);
        std::thread::spawn(move || {
            let status = loop {
                let st = wchild.lock().map(|mut c| c.try_wait());
                match st {
                    Ok(Ok(Some(s))) => break Ok(s),
                    Ok(Ok(None)) => std::thread::sleep(Duration::from_millis(50)),
                    Ok(Err(e)) => break Err(e.to_string()),
                    Err(_) => break Err("job lock poisoned".into()),
                }
            };
            for t in [out_t, err_t].into_iter().flatten() {
                let _ = t.join();
            }
            let tail = tail.lock().map(|t| t.join("\n")).unwrap_or_default();
            let _ = tx.send(JobMsg::Exited(outcome(status, &tail)));
            notify();
        });
        Ok(Self {
            child,
            pid,
            rx,
            staging,
            cancel_sent: None,
            killed: false,
            finished: false,
        })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Whether Cancel was pressed.
    pub fn cancelling(&self) -> bool {
        self.cancel_sent.is_some()
    }

    /// Messages since the last call (never blocks). Kills the process if it
    /// ignored the cancel signal for [`CANCEL_GRACE`].
    pub fn poll(&mut self) -> Vec<JobMsg> {
        let mut out: Vec<JobMsg> = self.rx.try_iter().collect();
        if let Some(JobMsg::Exited(o)) = out.last_mut() {
            self.finished = true;
            // A signal death after our cancel is a cancel.
            if self.cancel_sent.is_some() && *o != Outcome::Ok {
                *o = Outcome::Cancelled;
            }
        }
        if !self.finished
            && !self.killed
            && self
                .cancel_sent
                .is_some_and(|t| t.elapsed() >= CANCEL_GRACE)
        {
            self.kill();
        }
        out
    }

    /// Asks the pipeline to stop (SIGTERM / CTRL_BREAK). If it can't be
    /// signalled it is killed at once.
    pub fn cancel(&mut self) {
        if self.finished || self.cancel_sent.is_some() {
            return;
        }
        self.cancel_sent = Some(Instant::now());
        if let Err(e) = send_cancel(self.pid) {
            eprintln!("lillith-settings: import cancel signal: {e}");
            self.kill();
        }
    }

    /// Last resort after [`CANCEL_GRACE`]. Linux: SIGKILL to the process
    /// group (the pipeline and its ffmpeg). Windows: CTRL_BREAK is sent once
    /// more, never TerminateProcess (the pipeline must get to clean up).
    fn kill(&mut self) {
        self.killed = true;
        #[cfg(unix)]
        {
            // SAFETY: plain syscall on our own child's process group.
            unsafe {
                libc::killpg(self.pid as libc::pid_t, libc::SIGKILL);
            }
            if let Ok(mut c) = self.child.lock() {
                let _ = c.kill();
            }
        }
        #[cfg(windows)]
        if let Err(e) = send_cancel(self.pid) {
            eprintln!("lillith-settings: import still running after cancel: {e}");
        }
    }

    /// Waits up to `d` for the process to end (tests, shutdown).
    pub fn wait_exit(&mut self, d: Duration) -> Option<Outcome> {
        let end = Instant::now() + d;
        while Instant::now() < end {
            for m in self.poll() {
                if let JobMsg::Exited(o) = m {
                    return Some(o);
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }
}

impl Drop for Job {
    /// The window closed mid-run: the pipeline never outlives it, and its
    /// half-made staging folder goes with it.
    fn drop(&mut self) {
        if !self.finished {
            self.cancel();
            if self.wait_exit(Duration::from_secs(2)).is_none() {
                self.kill();
                let _ = self.wait_exit(Duration::from_secs(1));
            }
            if let Some(root) = self.staging.parent() {
                let _ = remove_staging(root, &self.staging);
            }
        }
    }
}

fn outcome(status: Result<ExitStatus, String>, tail: &str) -> Outcome {
    let with_tail = |head: String| {
        if tail.trim().is_empty() {
            head
        } else {
            format!("{head}\n{tail}")
        }
    };
    match status {
        Ok(s) if s.success() => Outcome::Ok,
        Ok(s) if s.code() == Some(EXIT_CANCELLED) => Outcome::Cancelled,
        Ok(s) => Outcome::Failed(with_tail(match s.code() {
            Some(c) => format!("the import stopped with exit code {c}"),
            None => format!("the import was ended by the system ({s})"),
        })),
        Err(e) => Outcome::Failed(with_tail(e)),
    }
}

fn read_events(o: impl Read, tx: &Sender<JobMsg>, notify: &dyn Fn()) {
    let mut r = BufReader::new(o);
    let mut buf = Vec::new();
    while matches!(r.read_until(b'\n', &mut buf), Ok(n) if n > 0) {
        let l = String::from_utf8_lossy(&buf[..buf.len().min(MAX_LINE)]).to_string();
        buf.clear();
        if l.trim().is_empty() {
            continue;
        }
        if tx.send(JobMsg::Event(parse_line(&l))).is_err() {
            return;
        }
        notify();
    }
}

/// Sends the contract's cancel signal (`SIGTERM`) to `pid`'s process group
/// (it was started with `process_group(0)`, so the group is the pipeline
/// and its ffmpeg children).
#[cfg(unix)]
pub fn send_cancel(pid: u32) -> std::io::Result<()> {
    // SAFETY: plain syscall; pid is our own child and its group's leader.
    if unsafe { libc::killpg(pid as libc::pid_t, libc::SIGTERM) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Sends the contract's cancel signal (`CTRL_BREAK`) to `pid`'s process
/// group (it was started with `CREATE_NEW_PROCESS_GROUP` and its own hidden
/// console): attach to that console for the call, ignoring the event
/// ourselves, then re-attach to our parent's console if we had one.
#[cfg(windows)]
pub fn send_cancel(pid: u32) -> std::io::Result<()> {
    use windows_sys::Win32::System::Console::{
        AttachConsole, FreeConsole, GenerateConsoleCtrlEvent, GetConsoleWindow,
        SetConsoleCtrlHandler, ATTACH_PARENT_PROCESS, CTRL_BREAK_EVENT,
    };
    // SAFETY: plain Win32 console calls, no pointers passed.
    unsafe {
        let had_console = !GetConsoleWindow().is_null();
        FreeConsole();
        if AttachConsole(pid) == 0 {
            let e = std::io::Error::last_os_error();
            if had_console {
                AttachConsole(ATTACH_PARENT_PROCESS);
            }
            return Err(e);
        }
        SetConsoleCtrlHandler(None, 1);
        let ok = GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid);
        let e = std::io::Error::last_os_error();
        std::thread::sleep(Duration::from_millis(50));
        FreeConsole();
        SetConsoleCtrlHandler(None, 0);
        if had_console {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
        if ok == 0 {
            Err(e)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_event() {
        assert_eq!(
            parse_line(r#"{"event":"progress","stage":"matte","pct":42.5,"msg":"frame 10/24"}"#),
            Event::Progress {
                stage: "matte".into(),
                pct: 42.5,
                msg: "frame 10/24".into()
            }
        );
        let c = parse_line(
            r#"{"event":"candidate","id":"c1","mp4":"c1/c1.mp4","preview_png":"c1/c1.png","frames":24,"fps":12,"qa":{"pass":true,"white_edge":0.01,"fringe":"low"},"suggested_name":"hip sway"}"#,
        );
        assert_eq!(
            c,
            Event::Candidate(Candidate {
                id: "c1".into(),
                mp4: "c1/c1.mp4".into(),
                preview_png: "c1/c1.png".into(),
                frames: 24,
                fps: 12.0,
                qa: Qa {
                    pass: true,
                    white_edge: "0.01".into(),
                    fringe: "low".into()
                },
                suggested_name: "hip sway".into(),
            })
        );
        assert_eq!(
            parse_line(r#"{"event":"done","candidates":3,"peak_rss_mb":812}"#),
            Event::Done {
                candidates: 3,
                peak_rss_mb: Some(812)
            }
        );
        assert_eq!(
            parse_line(r#"{"event":"error","msg":"no person found"}"#),
            Event::Error {
                msg: "no person found".into()
            }
        );
    }

    #[test]
    fn malformed_lines_never_fail() {
        for l in [
            "",
            "hello",
            "{not json",
            "[1,2]",
            "42",
            "\"str\"",
            "Traceback (most recent call last):",
        ] {
            assert!(matches!(parse_line(l), Event::Malformed(_)), "{l}");
        }
        for l in [
            r#"{"event":"party"}"#,
            r#"{"no":"event"}"#,
            r#"{"event":"candidate","mp4":"a.mp4"}"#,
            r#"{"event":"candidate","mp4":"a.mp4","preview_png":"a.png","frames":0}"#,
            r#"{"event":"candidate","mp4":"","preview_png":"a.png","frames":3}"#,
        ] {
            assert!(matches!(parse_line(l), Event::Unknown(_)), "{l}");
        }
        // Out-of-range / wrong-typed numbers are clamped or defaulted.
        assert!(matches!(
            parse_line(r#"{"event":"progress","pct":250}"#),
            Event::Progress { pct, .. } if pct == 100.0
        ));
        assert!(matches!(
            parse_line(r#"{"event":"progress","pct":"half"}"#),
            Event::Progress { pct, .. } if pct == 0.0
        ));
        let Event::Candidate(c) = parse_line(
            r#"{"event":"candidate","mp4":"a.mp4","preview_png":"a.png","frames":3,"fps":-1}"#,
        ) else {
            panic!()
        };
        assert_eq!((c.fps, c.qa.pass, c.id.as_str()), (12.0, false, "a.mp4"));
    }

    #[test]
    fn accepts_only_video_types() {
        for ok in ["a.mp4", "B.MOV", "c.webm", "d.gif", "/x/y z.Mp4"] {
            assert!(accepted(Path::new(ok)), "{ok}");
        }
        for no in ["a.png", "a", "a.mp4.txt", "mp4"] {
            assert!(!accepted(Path::new(no)), "{no}");
        }
    }

    #[test]
    fn staging_paths_stay_inside() {
        let root = std::env::temp_dir().join(format!("lillith-vi-stage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let job = create_staging(&root, "job-1").unwrap();
        assert!(create_staging(&root, "job-1").is_err(), "never reused");
        std::fs::create_dir_all(job.join("c1")).unwrap();
        std::fs::write(job.join("c1/a.mp4"), b"x").unwrap();
        std::fs::write(root.join("outside.mp4"), b"x").unwrap();
        assert!(resolve_in_staging(&job, Path::new("c1/a.mp4")).is_some());
        assert!(resolve_in_staging(&job, &job.join("c1/a.mp4")).is_some());
        assert!(resolve_in_staging(&job, Path::new("../outside.mp4")).is_none());
        assert!(resolve_in_staging(&job, &root.join("outside.mp4")).is_none());
        assert!(resolve_in_staging(&job, Path::new("c1/missing.mp4")).is_none());
        // Only a job-* child of the root is ever removed.
        assert!(remove_staging(&root, &root).is_err());
        assert!(remove_staging(&root, &root.join("c1")).is_err());
        assert!(remove_staging(&job, &job.join("c1")).is_err());
        remove_staging(&root, &job).unwrap();
        assert!(!job.exists());
        assert!(root.join("outside.mp4").exists());
        remove_staging(&root, &job).unwrap(); // already gone: fine
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_card_shows_the_exact_command() {
        let c = ImportCmd {
            python: "/usr/bin/python3".into(),
            script: "/opt/l/tools/video-pipeline/user_import.py".into(),
            input: "/home/ko/My Clip's.mp4".into(),
            out: "/home/ko/.local/share/lillith/import-staging/job-1".into(),
            max_ram_mb: 1500,
            ffmpeg: None,
            ffprobe: None,
            matte: Matte::Auto,
            max_candidates: 0,
        };
        assert_eq!(
            c.display(),
            "/usr/bin/python3 /opt/l/tools/video-pipeline/user_import.py --input '/home/ko/My Clip'\\''s.mp4' --out /home/ko/.local/share/lillith/import-staging/job-1 --json-progress --max-ram-mb 1500 --threads 2"
        );
        assert_eq!(c.argv()[3], "/home/ko/My Clip's.mp4", "argv is unquoted");
        // Bundled tools and the advanced options are passed only when set.
        let c = ImportCmd {
            ffmpeg: Some("/opt/l/bin/ffmpeg".into()),
            ffprobe: Some("/opt/l/bin/ffprobe".into()),
            matte: Matte::Key,
            max_candidates: 3,
            ..c
        };
        assert!(c.display().ends_with(
            "--threads 2 --matte key --max-candidates 3 --ffmpeg /opt/l/bin/ffmpeg --ffprobe /opt/l/bin/ffprobe"
        ), "{}", c.display());
    }

    #[test]
    fn missing_tools_are_named() {
        let p = Path::new("/x");
        assert_eq!(missing_tools(Some(p), Some(p), Some(p), Some(p)), None);
        let m = missing_tools(None, None, Some(p), Some(p)).unwrap();
        assert!(m.contains("Python") && m.contains("ffmpeg"), "{m}");
        assert!(missing_tools(Some(p), Some(p), None, Some(p))
            .unwrap()
            .contains("ffprobe"));
        assert!(missing_tools(Some(p), Some(p), Some(p), None)
            .unwrap()
            .contains("user_import.py"));
    }

    #[test]
    fn media_tools_bundled_first_then_path() {
        let root = std::env::temp_dir().join(format!("lillith-vi-tools-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (exe, sys) = (root.join("app"), root.join("sysbin"));
        std::fs::create_dir_all(exe.join("bin")).unwrap();
        std::fs::create_dir_all(&sys).unwrap();
        let file = |d: &Path, t: &str| {
            let p = d.join(format!("{t}{}", std::env::consts::EXE_SUFFIX));
            std::fs::write(&p, b"").unwrap();
            p
        };
        let path = std::env::join_paths([&sys]).unwrap();
        let find = |t: &str, o: Option<&str>| {
            find_tool_in(t, o.map(std::ffi::OsStr::new), Some(&exe), Some(&path))
        };
        // Nothing anywhere.
        assert_eq!(find("ffmpeg", None), None);
        // Only on PATH.
        let on_path = file(&sys, "ffmpeg");
        assert_eq!(find("ffmpeg", None), Some(on_path.clone()));
        // Next to the exe beats PATH.
        let beside = file(&exe, "ffmpeg");
        assert_eq!(find("ffmpeg", None), Some(beside));
        // bin/ next to the exe beats both.
        let bundled = file(&exe.join("bin"), "ffmpeg");
        assert_eq!(find("ffmpeg", None), Some(bundled));
        // ffprobe is looked up on its own.
        assert_eq!(find("ffprobe", None), None);
        let probe = file(&sys, "ffprobe");
        assert_eq!(find("ffprobe", None), Some(probe));
        // An explicit override wins (a path, or a name on PATH).
        assert_eq!(
            find("ffmpeg", Some(on_path.to_str().unwrap())),
            Some(on_path.clone())
        );
        assert_eq!(find("ffmpeg", Some("ffmpeg")), Some(on_path));
        assert_eq!(find("ffmpeg", Some("/nope/ffmpeg")), None);
        // Only a bundled bin/ copy is passed to the pipeline.
        assert_eq!(
            bundled_tool_in("ffmpeg", Some(&exe)),
            Some(
                exe.join("bin")
                    .join(format!("ffmpeg{}", std::env::consts::EXE_SUFFIX))
            )
        );
        assert_eq!(
            bundled_tool_in("ffprobe", Some(&exe)),
            None,
            "ffprobe only on PATH"
        );
        assert_eq!(bundled_tool_in("ffmpeg", None), None);
        let _ = std::fs::remove_dir_all(root);
    }

    /// A stub pipeline (sh) standing in for user_import.py.
    #[cfg(unix)]
    fn stub(body: &str) -> Command {
        let mut c = Command::new("sh");
        c.arg("-c").arg(body);
        c
    }

    #[cfg(unix)]
    #[test]
    fn a_run_streams_events_then_exits() {
        let mut j = Job::spawn(
            stub(
                r#"echo '{"event":"progress","stage":"probe","pct":5,"msg":"x"}'
echo 'stray print'
echo '{"event":"done","candidates":0}'"#,
            ),
            PathBuf::from("/nonexistent"),
            || {},
        )
        .unwrap();
        let mut evs = Vec::new();
        let end = Instant::now() + Duration::from_secs(10);
        let out = loop {
            assert!(Instant::now() < end, "timed out: {evs:?}");
            let mut done = None;
            for m in j.poll() {
                match m {
                    JobMsg::Event(e) => evs.push(e),
                    JobMsg::Exited(o) => done = Some(o),
                }
            }
            if let Some(o) = done {
                break o;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(out, Outcome::Ok);
        assert_eq!(evs.len(), 3, "every line arrives before the exit: {evs:?}");
        assert!(matches!(evs[1], Event::Malformed(_)));
        assert_eq!(
            evs[2],
            Event::Done {
                candidates: 0,
                peak_rss_mb: None
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn exit_codes_map_to_outcomes() {
        let run = |body: &str| {
            Job::spawn(stub(body), PathBuf::new(), || {})
                .unwrap()
                .wait_exit(Duration::from_secs(10))
                .unwrap()
        };
        assert_eq!(run("exit 130"), Outcome::Cancelled);
        let Outcome::Failed(m) = run("echo boom >&2; exit 1") else {
            panic!()
        };
        assert!(m.contains("exit code 1") && m.contains("boom"), "{m}");
    }

    #[cfg(unix)]
    #[test]
    fn cancel_sends_sigterm_and_the_run_ends_cancelled() {
        // The stub honours the contract: SIGTERM → exit 130.
        let mut j = Job::spawn(
            stub(r#"trap 'exit 130' TERM; echo '{"event":"progress","stage":"s","pct":1}'; while :; do sleep 0.05; done"#),
            PathBuf::new(),
            || {},
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let t0 = Instant::now();
        j.cancel();
        assert!(j.cancelling());
        assert_eq!(
            j.wait_exit(Duration::from_secs(4)),
            Some(Outcome::Cancelled)
        );
        assert!(
            t0.elapsed() < CANCEL_GRACE,
            "ended by the signal, not the kill"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_pipeline_that_ignores_cancel_is_killed_after_the_grace() {
        let mut j = Job::spawn(
            stub(r#"trap '' TERM; while :; do sleep 0.05; done"#),
            PathBuf::new(),
            || {},
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        j.cancel();
        let o = j.wait_exit(CANCEL_GRACE + Duration::from_secs(5));
        assert_eq!(o, Some(Outcome::Cancelled));
    }
}
