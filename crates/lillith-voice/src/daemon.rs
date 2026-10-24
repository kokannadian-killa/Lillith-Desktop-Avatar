//! Client for the local voice daemon `lillith-voiced`
//! (`lillith-voice-kit/PROTOCOL.md`, protocol v2).
//!
//! JSON lines over a **local, per-user** transport only: an `AF_UNIX`
//! stream socket on Linux/macOS, the `\\.\pipe\lillith-voice` named pipe on
//! Windows. There is no TCP code here at all, so nothing can leave the
//! machine; endpoints that are not a local socket path / local pipe are
//! refused ([`DaemonClient::new`]). No downloads, no telemetry.
//!
//! - [`Request`]: what we send (`cmd` key; the daemon also accepts `op`).
//! - [`request_command`]: reads `cmd` or `op` from a request (fake daemons
//!   in tests, and to document the alias).
//! - [`DaemonClient`]: one request per connection (`ping`, `say`, `stop`
//!   on a second connection for barge-in, `listen` push-to-talk, `stt`).
//! - [`LipsyncReader`]: a thread holding a dedicated `subscribe` connection
//!   that forwards every `lipsync` event line over a channel.
//! - [`route_transcript`]: the client-side safety rules for a voice
//!   transcript: the safeword ("red" / "OOC: stop") stops everything, and a
//!   voice transcript is **never** consent to a Soul Contract.
//!
//! Timeouts (a hung daemon must never freeze the caller):
//! - connect: Unix [`CONNECT_TIMEOUT`] (2 s); Windows retries
//!   `ERROR_PIPE_BUSY` for at most [`PIPE_BUSY_RETRY`] (~1 s).
//! - writes: at most [`SHORT_TIMEOUT`] (3 s) per connection.
//! - `ping` [`PING_TIMEOUT`] (800 ms); `stop` [`SHORT_TIMEOUT`] (3 s);
//!   `listen start` [`MIC_CARD_TIMEOUT`] (90 s: the daemon holds the reply
//!   up to ~70 s while the mic card is shown); `listen stop` / `stt`: the
//!   caller's timeout.
//! - `say wait:true`: [`say_timeout`] per line (30 s + 250 ms/char, at most
//!   10 min), enforced by the playback poll, which then stops the line.
//! - `subscribe lipsync`: no idle timeout (it is idle whenever she is
//!   quiet); Unix reads wake every `SUB_POLL` to check for shutdown and
//!   Windows reads are cancelled on shutdown.
//!
//! Windows pipes have no read timeout, so timed requests run on a helper
//! thread (`run_with_deadline`) whose I/O is cancelled on expiry.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

/// Protocol version this client speaks.
pub const PROTOCOL: u64 = 2;
/// Socket path under `$XDG_RUNTIME_DIR`.
pub const SOCKET_REL_PATH: &str = "lillith-voice/voiced.sock";
/// Windows named pipe.
pub const WINDOWS_PIPE: &str = r"\\.\pipe\lillith-voice";
/// The lip-sync subscription line (same as `lillith_anim::lipsync::SUBSCRIBE_LINE`).
pub const SUBSCRIBE_LIPSYNC: &str = r#"{"op":"subscribe","topics":["lipsync"]}"#;
/// `ping` must answer within this (start-up probe).
pub const PING_TIMEOUT: Duration = Duration::from_millis(800);
/// Short control commands (`stop`, `listen stop` without STT), and the
/// write timeout of every connection.
pub const SHORT_TIMEOUT: Duration = Duration::from_secs(3);
/// Opening a connection: a Unix `connect` to a daemon whose backlog is
/// full would otherwise block forever. (Windows: the `ERROR_PIPE_BUSY`
/// retry is capped at [`PIPE_BUSY_RETRY`] instead.)
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Requests the daemon may hold while it shows Ko the mic card
/// (`listen start`, `listen_mode on`: up to ~70 s, PROTOCOL.md), plus a
/// margin. Never call those from the UI thread.
pub const MIC_CARD_TIMEOUT: Duration = Duration::from_secs(90);
/// `say wait:true` replies when playback ends, so it gets a per-line
/// deadline instead of a short timeout: [`SAY_BASE_TIMEOUT`] plus
/// [`SAY_PER_CHAR`] for every character, capped at [`SAY_MAX_TIMEOUT`]
/// (see [`say_timeout`]). On expiry the line is stopped and dropped.
pub const SAY_BASE_TIMEOUT: Duration = Duration::from_secs(30);
/// Per character of a `say` line (speech is ~15 chars/s; this is 4x slack).
pub const SAY_PER_CHAR: Duration = Duration::from_millis(250);
/// Longest any one `say` may take before it is treated as hung.
pub const SAY_MAX_TIMEOUT: Duration = Duration::from_secs(600);

/// The deadline for one `say` of `text` (see [`SAY_BASE_TIMEOUT`]).
pub fn say_timeout(text: &str) -> Duration {
    let chars = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
    SAY_BASE_TIMEOUT
        .saturating_add(SAY_PER_CHAR.saturating_mul(chars))
        .min(SAY_MAX_TIMEOUT)
}
/// Longest line accepted from the daemon (a big envelope is ~10 KB).
pub const MAX_LINE: usize = 1 << 20;

/// One request line.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Ping,
    /// `wait:true` blocks the connection until playback ends.
    Say {
        text: String,
        wait: bool,
        preset: Option<String>,
        speed: Option<f32>,
    },
    /// Stop playback and drop the queue (`reason` is echoed, e.g. `barge_in`).
    Stop {
        reason: Option<String>,
    },
    /// Push-to-talk: open the mic (stops her speaking: barge-in).
    ListenStart,
    /// [`Request::ListenStart`] owned by its connection (`hold:true`): the
    /// client keeps the connection open while the press lasts, and the
    /// daemon treats the connection closing as a release (card closed, mic
    /// stopped, tokens dropped). See [`DaemonClient::listen_start_held`].
    ListenStartHeld,
    /// Push-to-talk: close the mic and transcribe.
    ListenStop,
    /// Push-to-talk: close the mic and drop what it heard (nothing is
    /// transcribed or broadcast).
    ListenDrop,
    /// Transcribe a WAV file (absolute path).
    Stt {
        wav: PathBuf,
    },
    /// Turn this connection into an event stream.
    Subscribe {
        topics: Vec<String>,
    },
}

impl Request {
    /// The command name (`cmd` / `op` value).
    pub fn command(&self) -> &'static str {
        match self {
            Request::Ping => "ping",
            Request::Say { .. } => "say",
            Request::Stop { .. } => "stop",
            Request::ListenStart
            | Request::ListenStartHeld
            | Request::ListenStop
            | Request::ListenDrop => "listen",
            Request::Stt { .. } => "stt",
            Request::Subscribe { .. } => "subscribe",
        }
    }

    /// The JSON object (always with a `cmd` key; `subscribe` uses `op`, as
    /// in the protocol's example; the daemon treats them alike).
    pub fn to_value(&self) -> Value {
        let mut v = match self {
            Request::Ping => json!({}),
            Request::Say {
                text,
                wait,
                preset,
                speed,
            } => {
                let mut v = json!({ "text": text, "wait": wait, "actions": "drop" });
                if let Some(p) = preset {
                    v["preset"] = json!(p);
                }
                if let Some(s) = speed.filter(|s| s.is_finite() && (*s - 1.0).abs() > 1e-3) {
                    v["speed"] = json!(s);
                }
                v
            }
            Request::Stop { reason } => match reason {
                Some(r) => json!({ "reason": r }),
                None => json!({}),
            },
            Request::ListenStart => json!({ "action": "start" }),
            Request::ListenStartHeld => json!({ "action": "start", "hold": true }),
            Request::ListenStop => json!({ "action": "stop" }),
            Request::ListenDrop => json!({ "action": "stop", "drop": true }),
            Request::Stt { wav } => json!({ "wav": wav.to_string_lossy() }),
            Request::Subscribe { topics } => json!({ "topics": topics }),
        };
        let key = if matches!(self, Request::Subscribe { .. }) {
            "op"
        } else {
            "cmd"
        };
        v[key] = json!(self.command());
        v
    }

    /// One line, without the trailing `\n`.
    pub fn encode(&self) -> String {
        self.to_value().to_string()
    }
}

/// The command of a request object: `cmd`, else `op`, else `say` (the
/// protocol's default).
pub fn request_command(v: &Value) -> &str {
    v.get("cmd")
        .or_else(|| v.get("op"))
        .and_then(Value::as_str)
        .unwrap_or("say")
}

/// Errors talking to the daemon.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("voice daemon endpoint {0} is not a local socket/pipe; refused")]
    NotLocal(String),
    #[error("voice daemon unavailable: {0}")]
    Io(#[from] std::io::Error),
    #[error("voice daemon sent bad JSON: {0}")]
    Json(String),
    #[error("voice daemon closed the connection")]
    Closed,
    #[error("voice daemon: {0}")]
    Daemon(String),
}

/// Parses one reply line: `ok:false` becomes [`DaemonError::Daemon`].
pub fn parse_reply(line: &str) -> Result<Value, DaemonError> {
    let v: Value =
        serde_json::from_str(line.trim()).map_err(|e| DaemonError::Json(e.to_string()))?;
    if !v.is_object() {
        return Err(DaemonError::Json("reply is not an object".into()));
    }
    if v.get("ok").and_then(Value::as_bool) == Some(false) {
        let e = v
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(DaemonError::Daemon(e.to_string()));
    }
    Ok(v)
}

/// An utterance id: an integer per the protocol (a numeric string is
/// tolerated).
pub fn utt_of(v: &Value) -> Option<u64> {
    match v.get("utt")? {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// `ping` reply.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PingInfo {
    pub protocol: Option<u64>,
    pub presets: Vec<String>,
    pub default: Option<String>,
    pub speaking: bool,
    pub stt_loaded: bool,
    /// `cloned_voice` from `ping`: whether the cloned 'lillith' voice
    /// (Pocket TTS) loaded. `None` = not reported (older daemon).
    pub cloned_voice: Option<bool>,
}

impl PingInfo {
    pub fn from_value(v: &Value) -> Self {
        Self {
            protocol: v.get("protocol").and_then(Value::as_u64),
            presets: v
                .get("presets")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|p| p.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            default: v.get("default").and_then(Value::as_str).map(str::to_string),
            speaking: v.get("speaking").and_then(Value::as_bool).unwrap_or(false),
            stt_loaded: v
                .get("stt_loaded")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            cloned_voice: v.get("cloned_voice").and_then(Value::as_bool),
        }
    }

    /// `true` for a preset that needs the cloned voice (`lillith*`).
    fn is_clone_preset(p: &str) -> bool {
        p.starts_with(crate::config::DEFAULT_PRESET)
    }

    /// Warning to log / show when `want` is the cloned voice but the daemon
    /// can't give it (`cloned_voice:false`, or the preset is missing).
    pub fn fallback_warning(&self, want: &str) -> Option<String> {
        if !Self::is_clone_preset(want) {
            return None;
        }
        let picked = self.pick_preset(want);
        if picked.as_deref() == Some(want) {
            return None;
        }
        let fb = picked.unwrap_or_else(|| "daemon default".into());
        Some(format!(
            "using fallback voice {fb} (cloned '{}' voice unavailable)",
            crate::config::DEFAULT_PRESET
        ))
    }

    /// The preset to request: `want` if the daemon has it (or lists none),
    /// else [`crate::config::FALLBACK_PRESET`] if it has that, else `None`
    /// (the daemon's own default).
    pub fn pick_preset(&self, want: &str) -> Option<String> {
        let clone_gone = self.cloned_voice == Some(false) && Self::is_clone_preset(want);
        if !clone_gone && (self.presets.is_empty() || self.presets.iter().any(|p| p == want)) {
            return Some(want.to_string());
        }
        let fb = crate::config::FALLBACK_PRESET;
        self.presets.iter().any(|p| p == fb).then(|| fb.to_string())
    }
}

/// `say` reply.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SayReply {
    pub utt: Option<u64>,
    pub stopped: bool,
    pub queued: bool,
}

impl SayReply {
    pub fn from_value(v: &Value) -> Self {
        Self {
            utt: utt_of(v),
            stopped: v.get("stopped").and_then(Value::as_bool).unwrap_or(false),
            queued: v.get("queued").and_then(Value::as_bool).unwrap_or(false),
        }
    }
}

/// A speech-to-text result (`listen` stop / `listen` once / `stt`, or a
/// `transcript` event).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Transcript {
    pub text: String,
    /// The daemon flagged the safeword.
    pub safeword: bool,
    /// Starts with yes / okay / I agree… (informational only: NEVER consent).
    pub affirmation: bool,
    pub timeout: bool,
}

impl Transcript {
    pub fn from_value(v: &Value) -> Self {
        let b = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        Self {
            text: v
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            safeword: b("safeword"),
            affirmation: b("affirmation"),
            timeout: b("timeout"),
        }
    }
}

/// Whether a voice transcript is the safeword, by the app's own rule
/// ([`lillith_core::access::kill_switch::is_safeword`]) plus the spoken
/// forms the protocol lists ("O.O.C. stop", "out of character stop").
pub fn is_spoken_safeword(text: &str) -> bool {
    if lillith_core::access::kill_switch::is_safeword(text) {
        return true;
    }
    let t: String = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    t.contains("ooc stop") || t.contains("out of character stop")
}

/// What to do with a voice transcript. There is deliberately no
/// "approve" variant: a voice transcript can never answer a Soul Contract,
/// a plan or a deletion; only a real click / keypress can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceRoute {
    /// The safeword: stop speech and end the scene (it kills nothing:
    /// running commands stop only on Ctrl-C / shutdown). The transcript is NOT sent to the model as chat.
    Safeword,
    /// Ordinary words: put them in the chat input (Ko still presses Enter).
    /// Leading `/` is removed so a transcript can never form a typed
    /// command such as `/contract yes`.
    Text(String),
    /// A gated popup is waiting and the transcript is not the safeword:
    /// dropped (voice never answers it).
    Refused,
    /// Nothing was said.
    Nothing,
}

/// Applies the voice safety rules to `t`. `gate_pending` = a Soul
/// Contract, plan or deletion popup is up.
pub fn route_transcript(t: &Transcript, gate_pending: bool) -> VoiceRoute {
    if t.safeword || is_spoken_safeword(&t.text) {
        return VoiceRoute::Safeword;
    }
    let text: String = t.text.chars().filter(|c| !c.is_control()).collect();
    let text = text
        .trim()
        .trim_start_matches(|c: char| c == '/' || c.is_whitespace())
        .trim()
        .to_string();
    if text.is_empty() {
        return VoiceRoute::Nothing;
    }
    if gate_pending {
        return VoiceRoute::Refused;
    }
    VoiceRoute::Text(text)
}

/// A connection (Unix stream / named pipe).
pub trait Conn: Read + Write + Send {
    /// Read timeout (`None` = block). A no-op on Windows pipes.
    fn set_timeout(&self, t: Option<Duration>) -> std::io::Result<()>;
}

#[cfg(unix)]
impl Conn for std::os::unix::net::UnixStream {
    fn set_timeout(&self, t: Option<Duration>) -> std::io::Result<()> {
        self.set_read_timeout(t)?;
        // Writes are one short line: never wait longer than a control op.
        self.set_write_timeout(Some(t.map_or(SHORT_TIMEOUT, |t| t.min(SHORT_TIMEOUT))))
    }
}

#[cfg(windows)]
impl Conn for std::fs::File {
    fn set_timeout(&self, _: Option<Duration>) -> std::io::Result<()> {
        Ok(())
    }
}

/// Whether `ep` is a local endpoint: an absolute filesystem path (Unix
/// socket) or a local named pipe `\\.\pipe\…` (Windows). Never a host,
/// port or URL.
pub fn is_local_endpoint(ep: &Path) -> bool {
    let s = ep.to_string_lossy();
    if s.is_empty() || s.contains("://") {
        return false;
    }
    if cfg!(windows) {
        let l = s.to_ascii_lowercase();
        return l.starts_with(r"\\.\pipe\") && l.len() > 9;
    }
    ep.is_absolute()
}

/// The endpoint for `configured` (`[voice] daemon_socket`), else the
/// protocol's defaults (`$LILLITH_VOICE_SOCKET`, `$XDG_RUNTIME_DIR/…`,
/// `/tmp/lillith-voice-<uid>/voiced.sock`; Windows `$LILLITH_VOICE_PIPE`
/// or [`WINDOWS_PIPE`]).
pub fn resolve_endpoint(configured: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = configured.filter(|p| !p.as_os_str().is_empty()) {
        return Some(crate::config::expand_home(p));
    }
    if cfg!(windows) {
        return Some(
            std::env::var_os("LILLITH_VOICE_PIPE")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(WINDOWS_PIPE)),
        );
    }
    if let Some(p) = std::env::var_os("LILLITH_VOICE_SOCKET").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(d).join(SOCKET_REL_PATH));
    }
    my_uid().map(|u| PathBuf::from(format!("/tmp/lillith-voice-{u}/voiced.sock")))
}

#[cfg(unix)]
fn my_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").ok().map(|m| m.uid())
}

#[cfg(not(unix))]
fn my_uid() -> Option<u32> {
    None
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn CancelSynchronousIo(thread: *mut std::ffi::c_void) -> i32;
}

/// Unblocks a thread stuck in a synchronous pipe read/write (Windows pipe
/// handles have no read timeout). A no-op elsewhere: Unix sockets use
/// read timeouts instead.
pub(crate) fn cancel_blocking_io<T>(thread: &std::thread::JoinHandle<T>) {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: the JoinHandle keeps the thread handle open; cancelling
        // I/O on it only makes that thread's pending call fail.
        unsafe {
            CancelSynchronousIo(thread.as_raw_handle());
        }
    }
    #[cfg(not(windows))]
    let _ = thread;
}

/// Timed-out requests whose helper thread has not returned yet.
static STUCK_REQUESTS: AtomicUsize = AtomicUsize::new(0);
/// More stuck requests than this: the daemon is treated as hung and new
/// requests fail at once (callers fall back) instead of piling up threads.
pub const MAX_STUCK_REQUESTS: usize = 2;

/// Whether the daemon looks healthy (no request is stuck past its timeout).
pub fn daemon_healthy() -> bool {
    STUCK_REQUESTS.load(Ordering::SeqCst) == 0
}

/// Runs `work` on a helper thread and waits at most `timeout`. On timeout
/// the helper's blocking I/O is cancelled and it is counted as stuck until
/// it returns; with [`MAX_STUCK_REQUESTS`] stuck, no thread is started.
/// (Used for Windows pipes, which have no read timeout.)
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn run_with_deadline<T: Send + 'static>(
    timeout: Duration,
    work: impl FnOnce() -> Result<T, DaemonError> + Send + 'static,
) -> Result<T, DaemonError> {
    if STUCK_REQUESTS.load(Ordering::SeqCst) >= MAX_STUCK_REQUESTS {
        return Err(DaemonError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "voice daemon is hung (earlier requests never returned)",
        )));
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let timed_out = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&timed_out);
    let h = std::thread::Builder::new()
        .name("lillith-voiced-req".into())
        .spawn(move || {
            let r = work();
            // Counted stuck only if the caller gave up on us.
            if flag.swap(true, Ordering::SeqCst) {
                STUCK_REQUESTS.fetch_sub(1, Ordering::SeqCst);
            }
            let _ = tx.send(r);
        })?;
    match rx.recv_timeout(timeout) {
        Ok(r) => r,
        Err(_) => {
            if !timed_out.swap(true, Ordering::SeqCst) {
                STUCK_REQUESTS.fetch_add(1, Ordering::SeqCst);
                cancel_blocking_io(&h);
            }
            // The helper may have finished in between.
            rx.try_recv().unwrap_or_else(|_| {
                Err(DaemonError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "voice daemon did not answer",
                )))
            })
        }
    }
}

/// Windows `ERROR_PIPE_BUSY`: every pipe instance is taken right now.
pub const ERROR_PIPE_BUSY: i32 = 231;
/// Windows `ERROR_FILE_NOT_FOUND`: no daemon (fall back to the local engine).
pub const ERROR_FILE_NOT_FOUND: i32 = 2;
/// How long a busy pipe is retried (PROTOCOL.md: about 1 s).
pub const PIPE_BUSY_RETRY: Duration = Duration::from_millis(1000);
const PIPE_BUSY_STEP: Duration = Duration::from_millis(50);

/// Calls `open` until it stops failing with `ERROR_PIPE_BUSY`, for about
/// [`PIPE_BUSY_RETRY`] (`sleep` between tries). Any other error, e.g.
/// `ERROR_FILE_NOT_FOUND` (no daemon), returns at once.
pub fn open_retrying_busy<T>(
    mut open: impl FnMut() -> std::io::Result<T>,
    mut sleep: impl FnMut(Duration),
) -> std::io::Result<T> {
    let mut waited = Duration::ZERO;
    loop {
        match open() {
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && waited < PIPE_BUSY_RETRY => {
                sleep(PIPE_BUSY_STEP);
                waited += PIPE_BUSY_STEP;
            }
            r => return r,
        }
    }
}

/// Opens a connection to `ep` (local only, see [`is_local_endpoint`]). On
/// Unix the socket must belong to the current user.
pub fn connect(ep: &Path) -> Result<Box<dyn Conn>, DaemonError> {
    if !is_local_endpoint(ep) {
        return Err(DaemonError::NotLocal(ep.display().to_string()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let m = std::fs::metadata(ep)?;
        if !m.file_type().is_socket() {
            return Err(DaemonError::NotLocal(format!(
                "{} (not a socket)",
                ep.display()
            )));
        }
        if let Some(uid) = my_uid() {
            if m.uid() != uid {
                return Err(DaemonError::NotLocal(format!(
                    "{} (owned by another user)",
                    ep.display()
                )));
            }
        }
        let s = connect_unix_bounded(ep, CONNECT_TIMEOUT)?;
        Ok(Box::new(s))
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // The daemon's pipe ACL admits the current user only. We also
        // forbid it from impersonating us (identification level only).
        const SECURITY_IDENTIFICATION: u32 = 1 << 16;
        let f = open_retrying_busy(
            || {
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .security_qos_flags(SECURITY_IDENTIFICATION)
                    .open(ep)
            },
            std::thread::sleep,
        )?;
        Ok(Box::new(f))
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(DaemonError::Io(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no local voice daemon transport on this platform",
        )))
    }
}

/// `UnixStream::connect` with a deadline (std has no `connect_timeout`
/// for Unix sockets): the connect runs on a helper thread, which is left
/// to finish on its own if the daemon never accepts.
#[cfg(unix)]
fn connect_unix_bounded(
    ep: &Path,
    timeout: Duration,
) -> std::io::Result<std::os::unix::net::UnixStream> {
    let (tx, rx) = std::sync::mpsc::channel();
    let ep = ep.to_path_buf();
    std::thread::Builder::new()
        .name("lillith-voiced-connect".into())
        .spawn(move || {
            let _ = tx.send(std::os::unix::net::UnixStream::connect(&ep));
        })?;
    rx.recv_timeout(timeout).unwrap_or_else(|_| {
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "voice daemon did not accept the connection",
        ))
    })
}

/// Reads one `\n`-terminated line (at most [`MAX_LINE`] bytes).
fn read_line(r: &mut impl BufRead) -> Result<String, DaemonError> {
    let mut buf = Vec::new();
    let n = r.take(MAX_LINE as u64).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Err(DaemonError::Closed);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Talks to the daemon at one endpoint, one request per connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonClient {
    endpoint: PathBuf,
}

impl DaemonClient {
    /// Refuses non-local endpoints.
    pub fn new(endpoint: impl Into<PathBuf>) -> Result<Self, DaemonError> {
        let endpoint = endpoint.into();
        if !is_local_endpoint(&endpoint) {
            return Err(DaemonError::NotLocal(endpoint.display().to_string()));
        }
        Ok(Self { endpoint })
    }

    /// [`resolve_endpoint`] + [`DaemonClient::new`].
    pub fn from_config(cfg: &crate::config::VoiceConfig) -> Option<Self> {
        resolve_endpoint(cfg.daemon_socket.as_deref()).and_then(|p| Self::new(p).ok())
    }

    pub fn endpoint(&self) -> &Path {
        &self.endpoint
    }

    /// Whether the endpoint exists (cheap; no connection).
    pub fn present(&self) -> bool {
        cfg!(windows) || self.endpoint.exists()
    }

    /// Opens a connection and writes `req`.
    pub fn send(
        &self,
        req: &Request,
        timeout: Option<Duration>,
    ) -> Result<Box<dyn Conn>, DaemonError> {
        let mut line = req.encode();
        if line.len() >= MAX_LINE {
            return Err(DaemonError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "request line over 1 MiB",
            )));
        }
        let mut c = connect(&self.endpoint)?;
        c.set_timeout(timeout)?;
        line.push('\n');
        c.write_all(line.as_bytes())?;
        c.flush()?;
        Ok(c)
    }

    /// Sends `req` and returns its one reply.
    pub fn request(&self, req: &Request, timeout: Option<Duration>) -> Result<Value, DaemonError> {
        #[cfg(windows)]
        if let Some(t) = timeout {
            // Pipe handles have no read timeout: bounded helper thread.
            let (me, req) = (self.clone(), req.clone());
            return run_with_deadline(t, move || me.request_blocking(&req, None));
        }
        self.request_blocking(req, timeout)
    }

    fn request_blocking(
        &self,
        req: &Request,
        timeout: Option<Duration>,
    ) -> Result<Value, DaemonError> {
        let c = self.send(req, timeout)?;
        let mut r = BufReader::new(c);
        parse_reply(&read_line(&mut r)?)
    }

    /// Liveness probe.
    pub fn ping(&self) -> Result<PingInfo, DaemonError> {
        let v = self.request(&Request::Ping, Some(PING_TIMEOUT))?;
        Ok(PingInfo::from_value(&v))
    }

    /// Stops playback now (a fresh connection, so it also works while
    /// another connection is blocked in `say wait:true`).
    pub fn stop(&self, reason: Option<&str>) -> Result<(), DaemonError> {
        self.request(
            &Request::Stop {
                reason: reason.map(str::to_string),
            },
            Some(SHORT_TIMEOUT),
        )
        .map(|_| ())
    }

    /// Push-to-talk down: opens the mic (the daemon stops her: barge-in).
    /// The daemon may hold the reply while Ko answers its mic card, so
    /// this waits up to [`MIC_CARD_TIMEOUT`]: call it off the UI thread.
    pub fn listen_start(&self) -> Result<(), DaemonError> {
        self.request(&Request::ListenStart, Some(MIC_CARD_TIMEOUT))
            .map(|_| ())
    }

    /// [`DaemonClient::listen_start`], but the press belongs to the
    /// returned connection: dropping the [`HeldPress`] (or this process
    /// exiting, or the connection breaking while the card waits) releases
    /// the press on the daemon: a waiting card is closed (`CANCELLED`), an
    /// open mic stops, unused mic tokens are dropped. Waits up to
    /// [`MIC_CARD_TIMEOUT`] for the card: call it off the UI thread. Older
    /// daemons ignore `hold` (then only `listen stop` releases).
    pub fn listen_start_held(&self) -> Result<HeldPress, DaemonError> {
        let work = {
            let me = self.clone();
            move || -> Result<HeldPress, DaemonError> {
                let c = me.send(&Request::ListenStartHeld, Some(MIC_CARD_TIMEOUT))?;
                let mut r = BufReader::new(c);
                parse_reply(&read_line(&mut r)?)?;
                Ok(HeldPress { _conn: r })
            }
        };
        #[cfg(windows)]
        return run_with_deadline(MIC_CARD_TIMEOUT, work);
        #[cfg(not(windows))]
        work()
    }

    /// Push-to-talk up: closes the mic and waits for the transcript.
    pub fn listen_stop(&self, timeout: Duration) -> Result<Transcript, DaemonError> {
        let v = self.request(&Request::ListenStop, Some(timeout))?;
        Ok(Transcript::from_value(&v))
    }

    /// Push-to-talk: closes the mic (or cancels a waiting card) and drops
    /// the audio: nothing is transcribed or broadcast. An older daemon that
    /// doesn't know `drop` treats it as a plain `listen stop`.
    pub fn listen_drop(&self, timeout: Duration) -> Result<Transcript, DaemonError> {
        let v = self.request(&Request::ListenDrop, Some(timeout))?;
        Ok(Transcript::from_value(&v))
    }

    /// Transcribes a WAV file.
    pub fn stt_file(&self, wav: &Path, timeout: Duration) -> Result<Transcript, DaemonError> {
        let v = self.request(
            &Request::Stt {
                wav: wav.to_path_buf(),
            },
            Some(timeout),
        )?;
        Ok(Transcript::from_value(&v))
    }
}

/// The open connection that owns one push-to-talk press (from
/// [`DaemonClient::listen_start_held`]). Dropping it closes the connection,
/// which the daemon treats as a release.
pub struct HeldPress {
    _conn: BufReader<Box<dyn Conn>>,
}

impl std::fmt::Debug for HeldPress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HeldPress")
    }
}

/// What the lip-sync reader thread forwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LipsyncMsg {
    /// One event line (feed it to `AudioLipSync::feed_line`).
    Line(String),
    /// The subscription dropped (daemon gone): reset the mouth.
    Disconnected,
}

/// A thread holding a `subscribe lipsync` connection. Reconnects with
/// back-off while the daemon is away. Dropping it stops the thread.
pub struct LipsyncReader {
    rx: Receiver<LipsyncMsg>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for LipsyncReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LipsyncReader")
    }
}

/// Read timeout of the subscription (how often the thread checks for stop).
const SUB_POLL: Duration = Duration::from_millis(500);
const BACKOFF_MIN: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(5);

impl LipsyncReader {
    /// Starts the reader for `client`.
    pub fn spawn(client: DaemonClient) -> std::io::Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("lillith-lipsync".into())
            .spawn(move || lipsync_loop(&client, &tx, &flag))?;
        Ok(Self {
            rx,
            stop,
            thread: Some(thread),
        })
    }

    /// Everything received so far (never blocks).
    pub fn drain(&self) -> Vec<LipsyncMsg> {
        self.rx.try_iter().collect()
    }

    /// Throws away everything received so far; returns whether the daemon
    /// disconnected meanwhile (speakers off / muted: nothing to animate,
    /// but the queue must not grow).
    pub fn discard(&self) -> bool {
        // Not `any`: that would stop early and leave the rest queued.
        let mut gone = false;
        for m in self.rx.try_iter() {
            gone |= m == LipsyncMsg::Disconnected;
        }
        gone
    }

    /// Stops the thread and waits briefly for it (a blocked Windows pipe
    /// read is cancelled; Unix reads time out every [`SUB_POLL`]).
    /// Returns whether it exited.
    pub fn shutdown(&mut self, wait: Duration) -> bool {
        self.stop.store(true, Ordering::SeqCst);
        let Some(t) = self.thread.take() else {
            return true;
        };
        let deadline = std::time::Instant::now() + wait;
        while !t.is_finished() {
            if std::time::Instant::now() >= deadline {
                self.thread = Some(t);
                return false;
            }
            cancel_blocking_io(&t);
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = t.join();
        true
    }
}

impl Drop for LipsyncReader {
    fn drop(&mut self) {
        // Don't hang the caller; the flag alone ends the thread later.
        let _ = self.shutdown(Duration::from_millis(if cfg!(windows) { 200 } else { 0 }));
    }
}

fn sleep_unless_stopped(d: Duration, stop: &AtomicBool) {
    let step = Duration::from_millis(50);
    let mut left = d;
    while !left.is_zero() && !stop.load(Ordering::SeqCst) {
        let s = left.min(step);
        std::thread::sleep(s);
        left = left.saturating_sub(s);
    }
}

fn lipsync_loop(client: &DaemonClient, tx: &Sender<LipsyncMsg>, stop: &AtomicBool) {
    let mut backoff = BACKOFF_MIN;
    while !stop.load(Ordering::SeqCst) {
        if !client.present() {
            sleep_unless_stopped(backoff, stop);
            backoff = (backoff * 2).min(BACKOFF_MAX);
            continue;
        }
        let conn = connect(client.endpoint()).and_then(|mut c| {
            c.set_timeout(Some(SUB_POLL))?;
            c.write_all(SUBSCRIBE_LIPSYNC.as_bytes())?;
            c.write_all(b"\n")?;
            c.flush()?;
            Ok(c)
        });
        let Ok(conn) = conn else {
            sleep_unless_stopped(backoff, stop);
            backoff = (backoff * 2).min(BACKOFF_MAX);
            continue;
        };
        tracing::debug!(endpoint = %client.endpoint().display(), "lip-sync subscribed");
        backoff = BACKOFF_MIN;
        let mut r = BufReader::new(conn);
        let mut buf: Vec<u8> = Vec::new();
        loop {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            match r.read_until(b'\n', &mut buf) {
                Ok(0) => break,
                Ok(_) if buf.ends_with(b"\n") => {
                    let line = String::from_utf8_lossy(&buf).trim().to_string();
                    buf.clear();
                    if !line.is_empty() && tx.send(LipsyncMsg::Line(line)).is_err() {
                        return;
                    }
                }
                Ok(_) => {} // partial line; keep reading
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    if buf.len() > MAX_LINE {
                        buf.clear();
                    }
                }
                Err(_) => break,
            }
        }
        if tx.send(LipsyncMsg::Disconnected).is_err() {
            return;
        }
        sleep_unless_stopped(backoff, stop);
    }
}

/// A scripted fake daemon on a Unix socket in a temp dir (tests here and
/// in other crates). Each connection's request is answered by `reply`
/// (`None` = keep the connection open without answering, e.g. a
/// subscriber; extra lines can be pushed with [`FakeDaemon::push`]).
#[cfg(all(unix, test))]
pub mod fake {
    use super::*;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::Mutex;

    pub type Handler = dyn Fn(&Value) -> Vec<String> + Send + Sync;

    pub struct FakeDaemon {
        pub dir: PathBuf,
        pub sock: PathBuf,
        /// Every request received (parsed).
        pub seen: Arc<Mutex<Vec<Value>>>,
        subscribers: Arc<Mutex<Vec<UnixStream>>>,
        stop: Arc<AtomicBool>,
    }

    impl FakeDaemon {
        /// Starts a fake daemon; `handler` returns the reply lines for one
        /// request (an empty vec for `subscribe` keeps it as a subscriber).
        pub fn start(tag: &str, handler: Box<Handler>) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("lillith-fake-voiced-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let sock = dir.join("voiced.sock");
            let listener = UnixListener::bind(&sock).unwrap();
            listener.set_nonblocking(true).unwrap();
            let seen = Arc::new(Mutex::new(Vec::new()));
            let subscribers = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let handler: Arc<Handler> = Arc::from(handler);
            {
                let (seen, subs, stop) = (seen.clone(), subscribers.clone(), stop.clone());
                std::thread::spawn(move || {
                    while !stop.load(Ordering::SeqCst) {
                        match listener.accept() {
                            Ok((s, _)) => {
                                let (seen, subs, h) = (seen.clone(), subs.clone(), handler.clone());
                                std::thread::spawn(move || serve(s, &seen, &subs, &*h));
                            }
                            Err(_) => std::thread::sleep(Duration::from_millis(5)),
                        }
                    }
                });
            }
            Self {
                dir,
                sock,
                seen,
                subscribers,
                stop,
            }
        }

        /// Writes `line` to every subscriber.
        pub fn push(&self, line: &str) {
            if let Ok(mut subs) = self.subscribers.lock() {
                subs.retain_mut(|s| s.write_all(format!("{line}\n").as_bytes()).is_ok());
            }
        }

        pub fn subscriber_count(&self) -> usize {
            self.subscribers.lock().map(|s| s.len()).unwrap_or(0)
        }

        /// Drops every subscriber connection (daemon restart).
        pub fn kick_subscribers(&self) {
            if let Ok(mut subs) = self.subscribers.lock() {
                for s in subs.drain(..) {
                    let _ = s.shutdown(std::net::Shutdown::Both);
                }
            }
        }

        pub fn commands(&self) -> Vec<String> {
            self.seen
                .lock()
                .map(|v| v.iter().map(|r| request_command(r).to_string()).collect())
                .unwrap_or_default()
        }
    }

    impl Drop for FakeDaemon {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            self.kick_subscribers();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn serve(s: UnixStream, seen: &Mutex<Vec<Value>>, subs: &Mutex<Vec<UnixStream>>, h: &Handler) {
        let Ok(mut w) = s.try_clone() else { return };
        let mut r = BufReader::new(s);
        let mut line = String::new();
        while r.read_line(&mut line).map(|n| n > 0).unwrap_or(false) {
            let v: Value = serde_json::from_str(line.trim()).unwrap_or(Value::Null);
            line.clear();
            if let Ok(mut s) = seen.lock() {
                s.push(v.clone());
            }
            let replies = h(&v);
            if request_command(&v) == "subscribe" {
                for l in &replies {
                    let _ = w.write_all(format!("{l}\n").as_bytes());
                }
                if let (Ok(mut subs), Ok(c)) = (subs.lock(), w.try_clone()) {
                    subs.push(c);
                }
                // Keep the connection: events are pushed from outside.
                return;
            }
            for l in replies {
                if w.write_all(format!("{l}\n").as_bytes()).is_err() {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_encode_per_protocol() {
        let v: Value = serde_json::from_str(&Request::Ping.encode()).unwrap();
        assert_eq!(v, json!({"cmd":"ping"}));
        let v: Value = serde_json::from_str(
            &Request::Say {
                text: "Hiii \"babe\".\n".into(),
                wait: true,
                preset: Some("lillith".into()),
                speed: Some(1.0),
            }
            .encode(),
        )
        .unwrap();
        assert_eq!(v["cmd"], "say");
        assert_eq!(v["text"], "Hiii \"babe\".\n");
        assert_eq!(v["wait"], true);
        assert_eq!(v["preset"], "lillith");
        assert!(v.get("speed").is_none(), "1.0 = daemon default");
        let v = Request::Stop {
            reason: Some("barge_in".into()),
        }
        .to_value();
        assert_eq!(v, json!({"cmd":"stop","reason":"barge_in"}));
        assert_eq!(
            Request::ListenStart.to_value(),
            json!({"cmd":"listen","action":"start"})
        );
        assert_eq!(
            Request::ListenStartHeld.to_value(),
            json!({"cmd":"listen","action":"start","hold":true})
        );
        assert_eq!(
            Request::ListenStop.to_value(),
            json!({"cmd":"listen","action":"stop"})
        );
        assert_eq!(
            Request::ListenDrop.to_value(),
            json!({"cmd":"listen","action":"stop","drop":true})
        );
        let sub = Request::Subscribe {
            topics: vec!["lipsync".into()],
        };
        let a: Value = serde_json::from_str(&sub.encode()).unwrap();
        let b: Value = serde_json::from_str(SUBSCRIBE_LIPSYNC).unwrap();
        assert_eq!(a, b);
        assert!(!Request::Ping.encode().contains('\n'), "one line");
    }

    #[test]
    fn cmd_and_op_are_synonyms_and_say_is_default() {
        for line in [r#"{"cmd":"ping"}"#, r#"{"op":"ping"}"#] {
            let v: Value = serde_json::from_str(line).unwrap();
            assert_eq!(request_command(&v), "ping");
        }
        let v: Value = serde_json::from_str(r#"{"text":"hi"}"#).unwrap();
        assert_eq!(request_command(&v), "say");
        // Every encoded request names its command under one of the keys.
        for r in [
            Request::Ping,
            Request::ListenStart,
            Request::ListenStartHeld,
            Request::ListenDrop,
            Request::Subscribe { topics: vec![] },
            Request::Stop { reason: None },
        ] {
            assert_eq!(request_command(&r.to_value()), r.command());
        }
    }

    #[test]
    fn replies_decode_utt_numbers_and_errors() {
        let v = parse_reply(r#"{"ok":true,"utt":7,"audio_s":4.2,"stopped":true}"#).unwrap();
        let s = SayReply::from_value(&v);
        assert_eq!(s.utt, Some(7));
        assert!(s.stopped && !s.queued);
        let v = parse_reply(r#"{"ok":true,"queued":true,"utt":"12"}"#).unwrap();
        assert_eq!(SayReply::from_value(&v).utt, Some(12));
        match parse_reply(r#"{"ok":false,"error":"no such preset"}"#) {
            Err(DaemonError::Daemon(e)) => assert_eq!(e, "no such preset"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(parse_reply("nope"), Err(DaemonError::Json(_))));
        assert!(matches!(parse_reply("[1]"), Err(DaemonError::Json(_))));
        let p = PingInfo::from_value(
            &parse_reply(
                r#"{"ok":true,"pid":1,"protocol":2,"presets":["lillith","p1"],"default":"lillith","speaking":false,"unknown":1}"#,
            )
            .unwrap(),
        );
        assert_eq!(p.protocol, Some(PROTOCOL));
        assert_eq!(p.pick_preset("lillith").as_deref(), Some("lillith"));
        let p = PingInfo {
            presets: vec!["p1".into(), "bella".into()],
            ..Default::default()
        };
        assert_eq!(
            p.pick_preset("lillith").as_deref(),
            Some("p1"),
            "falls back"
        );
        let p = PingInfo {
            presets: vec!["bella".into()],
            ..Default::default()
        };
        assert_eq!(p.pick_preset("lillith"), None, "daemon default");
        let t = Transcript::from_value(
            &parse_reply(
                r#"{"ok":true,"text":" hey lillith ","safeword":false,"affirmation":false,"voice":true,"consent_ok":false}"#,
            )
            .unwrap(),
        );
        assert_eq!(t.text, "hey lillith");
    }

    #[test]
    fn safeword_from_stt_wins_and_voice_never_approves() {
        let t = |text: &str, safeword: bool, affirmation: bool| Transcript {
            text: text.into(),
            safeword,
            affirmation,
            timeout: false,
        };
        for s in [
            "Red.",
            "red",
            "OOC: stop",
            "O.O.C. stop",
            "out of character, stop",
        ] {
            assert_eq!(
                route_transcript(&t(s, false, false), false),
                VoiceRoute::Safeword,
                "{s}"
            );
            assert_eq!(
                route_transcript(&t(s, false, false), true),
                VoiceRoute::Safeword,
                "{s}"
            );
        }
        // The daemon's flag alone is enough.
        assert_eq!(
            route_transcript(&t("whatever", true, false), false),
            VoiceRoute::Safeword
        );
        assert_eq!(
            route_transcript(&t("I read a red book", false, false), false),
            VoiceRoute::Text("I read a red book".into())
        );
        // "yes" with a popup up: refused, never an approval.
        for s in ["yes", "Yes, do it", "/contract yes", "I agree"] {
            assert_eq!(
                route_transcript(&t(s, false, true), true),
                VoiceRoute::Refused,
                "{s}"
            );
        }
        // Without a popup it is only text, and a slash command can't form.
        assert_eq!(
            route_transcript(&t("/contract yes", false, true), false),
            VoiceRoute::Text("contract yes".into())
        );
        assert_eq!(
            route_transcript(&t("  ", false, false), false),
            VoiceRoute::Nothing
        );
    }

    #[test]
    fn busy_pipe_is_retried_about_a_second_and_missing_daemon_is_not() {
        use std::io::{Error, ErrorKind};
        // Busy twice, then free.
        let mut n = 0;
        let mut slept = Duration::ZERO;
        let r = open_retrying_busy(
            || {
                n += 1;
                if n < 3 {
                    Err(Error::from_raw_os_error(ERROR_PIPE_BUSY))
                } else {
                    Ok(n)
                }
            },
            |d| slept += d,
        );
        assert_eq!(r.unwrap(), 3);
        assert_eq!(slept, Duration::from_millis(100));
        // Busy forever: gives up after ~1 s.
        let mut slept = Duration::ZERO;
        let r: std::io::Result<()> = open_retrying_busy(
            || Err(Error::from_raw_os_error(ERROR_PIPE_BUSY)),
            |d| slept += d,
        );
        assert_eq!(r.unwrap_err().raw_os_error(), Some(ERROR_PIPE_BUSY));
        assert_eq!(slept, PIPE_BUSY_RETRY);
        // No daemon: no retry, a plain error (callers fall back).
        let mut tries = 0;
        let r: std::io::Result<()> = open_retrying_busy(
            || {
                tries += 1;
                Err(Error::from_raw_os_error(ERROR_FILE_NOT_FOUND))
            },
            |_| panic!("must not wait"),
        );
        assert_eq!(tries, 1);
        assert!(r.is_err());
        let r: std::io::Result<()> =
            open_retrying_busy(|| Err(Error::from(ErrorKind::NotFound)), |_| unreachable!());
        assert!(r.is_err());
    }

    #[test]
    fn hung_daemon_requests_time_out_and_never_pile_up_threads() {
        let release = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicUsize::new(0));
        let hang = || {
            let (r, s) = (Arc::clone(&release), Arc::clone(&started));
            move || {
                s.fetch_add(1, Ordering::SeqCst);
                while !r.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Ok(Value::Null)
            }
        };
        // A quick answer passes through.
        assert!(run_with_deadline(Duration::from_secs(2), || Ok(json!({"ok": true}))).is_ok());
        assert!(daemon_healthy());
        for _ in 0..MAX_STUCK_REQUESTS {
            let e = run_with_deadline(Duration::from_millis(30), hang()).unwrap_err();
            assert!(e.to_string().contains("did not answer"), "{e}");
        }
        assert!(!daemon_healthy(), "marked unhealthy");
        // Now refused at once: no new thread.
        let e = run_with_deadline(Duration::from_secs(5), hang()).unwrap_err();
        assert!(e.to_string().contains("hung"), "{e}");
        assert_eq!(started.load(Ordering::SeqCst), MAX_STUCK_REQUESTS);
        // The daemon recovers: healthy again once the stuck calls return.
        release.store(true, Ordering::SeqCst);
        let t0 = std::time::Instant::now();
        while !daemon_healthy() && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(daemon_healthy());
    }

    #[test]
    fn missing_cloned_voice_falls_back_to_p1_with_a_warning() {
        let ok: Value = serde_json::from_str(
            r#"{"ok":true,"protocol":2,"presets":["lillith","p1"],"default":"lillith","cloned_voice":true}"#,
        )
        .unwrap();
        let info = PingInfo::from_value(&ok);
        assert_eq!(info.cloned_voice, Some(true));
        assert_eq!(info.pick_preset("lillith").as_deref(), Some("lillith"));
        assert_eq!(info.fallback_warning("lillith"), None);
        let gone: Value = serde_json::from_str(
            r#"{"ok":true,"protocol":2,"presets":["lillith","p1"],"default":"lillith","cloned_voice":false}"#,
        )
        .unwrap();
        let info = PingInfo::from_value(&gone);
        assert_eq!(info.pick_preset("lillith").as_deref(), Some("p1"));
        assert_eq!(
            info.fallback_warning("lillith").as_deref(),
            Some("using fallback voice p1 (cloned 'lillith' voice unavailable)")
        );
        // A non-clone preset the user chose is not a fallback.
        assert_eq!(info.fallback_warning("bella"), None);
        // Older daemon (no field) but no lillith preset: also a fallback.
        let old = PingInfo {
            presets: vec!["p1".into()],
            ..PingInfo::default()
        };
        assert!(old.fallback_warning("lillith").is_some());
    }

    #[test]
    fn oversized_request_is_refused_before_sending() {
        let c = DaemonClient::new(std::env::temp_dir().join("lillith-no-such.sock")).unwrap();
        let big = Request::Say {
            text: "a".repeat(MAX_LINE + 10),
            wait: false,
            preset: None,
            speed: None,
        };
        let e = c.send(&big, None).err().expect("refused");
        assert!(e.to_string().contains("1 MiB"), "{e}");
    }

    #[test]
    fn only_local_endpoints() {
        assert!(!is_local_endpoint(Path::new("")));
        assert!(!is_local_endpoint(Path::new("tcp://127.0.0.1:5000")));
        assert!(!is_local_endpoint(Path::new("127.0.0.1:5000")));
        assert!(DaemonClient::new("http://example.com/voiced").is_err());
        #[cfg(unix)]
        {
            assert!(is_local_endpoint(Path::new(
                "/run/user/1000/lillith-voice/voiced.sock"
            )));
            assert!(!is_local_endpoint(Path::new("relative.sock")));
        }
        assert!(resolve_endpoint(Some(Path::new("/x/v.sock"))).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn missing_socket_is_a_clean_error() {
        let c = DaemonClient::new("/nonexistent/lillith-voice/voiced.sock").unwrap();
        assert!(!c.present());
        assert!(matches!(c.ping(), Err(DaemonError::Io(_))));
        // A plain file is not a socket.
        let f = std::env::temp_dir().join(format!("lillith-notsock-{}", std::process::id()));
        std::fs::write(&f, b"x").unwrap();
        assert!(matches!(
            DaemonClient::new(&f).unwrap().ping(),
            Err(DaemonError::NotLocal(_))
        ));
        let _ = std::fs::remove_file(&f);
    }

    #[cfg(unix)]
    #[test]
    fn client_talks_to_a_fake_daemon() {
        let d = fake::FakeDaemon::start(
            "client",
            Box::new(|v: &Value| {
                match (request_command(v), v["action"].as_str()) {
                ("ping", _) => vec![r#"{"ok":true,"protocol":2,"presets":["lillith","p1"]}"#.into()],
                ("listen", Some("start")) => vec![r#"{"ok":true,"listening":true}"#.into()],
                ("listen", Some("stop")) => vec![
                    r#"{"ok":true,"text":"Red.","safeword":true,"affirmation":false,"voice":true,"consent_ok":false}"#.into(),
                ],
                ("stop", _) => vec![r#"{"ok":true}"#.into()],
                _ => vec![r#"{"ok":false,"error":"unknown"}"#.into()],
            }
            }),
        );
        let c = DaemonClient::new(&d.sock).unwrap();
        assert!(c.present());
        assert_eq!(c.ping().unwrap().presets, vec!["lillith", "p1"]);
        c.listen_start().unwrap();
        let t = c.listen_stop(Duration::from_secs(2)).unwrap();
        assert!(t.safeword);
        assert_eq!(route_transcript(&t, false), VoiceRoute::Safeword);
        c.stop(Some("barge_in")).unwrap();
        assert!(matches!(
            c.stt_file(Path::new("/a.wav"), Duration::from_secs(1)),
            Err(DaemonError::Daemon(_))
        ));
        let seen = d.seen.lock().unwrap().clone();
        assert_eq!(seen[3]["reason"], "barge_in");
        assert_eq!(
            d.commands(),
            vec!["ping", "listen", "listen", "stop", "stt"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn lipsync_reader_subscribes_forwards_and_reconnects() {
        let d = fake::FakeDaemon::start(
            "lipsub",
            Box::new(|v: &Value| match request_command(v) {
                "subscribe" => vec![r#"{"ok":true,"subscribed":["lipsync"]}"#.into()],
                _ => vec![r#"{"ok":true}"#.into()],
            }),
        );
        let reader = LipsyncReader::spawn(DaemonClient::new(&d.sock).unwrap()).unwrap();
        let wait = |n: usize| {
            for _ in 0..200 {
                if d.subscriber_count() >= n {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("no subscriber");
        };
        wait(1);
        let first = d.seen.lock().unwrap()[0].clone();
        assert_eq!(
            first,
            serde_json::from_str::<Value>(SUBSCRIBE_LIPSYNC).unwrap()
        );
        d.push(r#"{"ev":"chunk_start","utt":7,"chunk":0,"mono_ms":81234567,"dur_ms":2480}"#);
        let mut got = Vec::new();
        for _ in 0..200 {
            got.extend(reader.drain());
            if got.len() >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            got[0],
            LipsyncMsg::Line(r#"{"ok":true,"subscribed":["lipsync"]}"#.into())
        );
        assert!(matches!(&got[1], LipsyncMsg::Line(l) if l.contains("chunk_start")));
        d.kick_subscribers();
        let mut disc = false;
        for _ in 0..300 {
            disc |= reader.drain().contains(&LipsyncMsg::Disconnected);
            if disc && d.subscriber_count() >= 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(disc, "disconnect reported");
        wait(1);
        // Speakers off / muted: lines are thrown away, nothing queues up.
        let mut reader = reader;
        for i in 0..50 {
            d.push(&format!(r#"{{"ev":"chunk_end","utt":7,"chunk":{i}}}"#));
        }
        std::thread::sleep(Duration::from_millis(100));
        assert!(!reader.discard(), "no disconnect in there");
        assert!(reader.drain().is_empty(), "queue emptied");
        // Stopping is prompt (reads poll the stop flag every SUB_POLL).
        assert!(reader.shutdown(SUB_POLL * 3), "reader thread exits");
    }
}
