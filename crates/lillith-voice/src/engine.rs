//! TTS engines (feature `voice`): the local voice daemon ([`DaemonEngine`],
//! Unix socket / named pipe), and subprocesses: Piper piped to
//! `aplay`/`paplay`, espeak-ng, spd-say, and a silent [`NullEngine`].
//!
//! Text is always passed as a single argv entry or on stdin, never through
//! a shell. [`detect`] picks the engine once at start-up and says why.

use crate::config::{EngineChoice, PlayerChoice, VoiceConfig};
use crate::daemon::{DaemonClient, PingInfo, Request, SayReply};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;

/// Per-line speaking options (from [`VoiceConfig`]).
#[derive(Debug, Clone, PartialEq)]
pub struct SpeakOpts {
    pub rate: f32,
    pub volume: f32,
    pub voice: Option<String>,
}

impl SpeakOpts {
    pub fn from_config(c: &VoiceConfig) -> Self {
        Self {
            rate: c.rate,
            volume: c.volume,
            voice: c.voice.clone(),
        }
    }
}

/// One line being spoken.
pub trait Playback: Send {
    /// `true` once it has finished (or failed).
    fn poll(&mut self) -> bool;
    /// Stops it now (interrupt / mute / shutdown).
    fn stop(&mut self);
}

/// A TTS backend. Runs on the speech worker thread only.
pub trait Engine: Send {
    fn name(&self) -> &'static str;
    /// `false` for [`NullEngine`]: the voice then spawns no worker at all.
    fn is_real(&self) -> bool {
        true
    }
    fn start(&mut self, text: &str, opts: &SpeakOpts) -> std::io::Result<Box<dyn Playback>>;
    /// Degraded-voice notice for the UI (e.g. cloned voice unavailable).
    fn voice_warning(&self) -> Option<String> {
        None
    }
}

/// Says nothing (no engine installed or `engine = "none"`).
#[derive(Debug, Clone, Copy, Default)]
pub struct NullEngine;

struct Done;
impl Playback for Done {
    fn poll(&mut self) -> bool {
        true
    }
    fn stop(&mut self) {}
}

impl Engine for NullEngine {
    fn name(&self) -> &'static str {
        "none"
    }
    fn is_real(&self) -> bool {
        false
    }
    fn start(&mut self, _: &str, _: &SpeakOpts) -> std::io::Result<Box<dyn Playback>> {
        Ok(Box::new(Done))
    }
}

/// Child processes (and an optional audio pump thread) for one line.
struct ChildPlayback {
    children: Vec<Child>,
    pump: Option<JoinHandle<()>>,
    /// Run after a kill (spd-say needs `spd-say -C` to cancel the daemon).
    on_stop: Option<(PathBuf, Vec<String>)>,
    done: bool,
}

impl ChildPlayback {
    fn finish(&mut self) {
        for c in &mut self.children {
            let _ = c.wait();
        }
        if let Some(p) = self.pump.take() {
            let _ = p.join();
        }
        self.done = true;
    }
}

impl Playback for ChildPlayback {
    fn poll(&mut self) -> bool {
        if self.done {
            return true;
        }
        // The last child is the one producing sound.
        let last = self.children.len() - 1;
        match self.children[last].try_wait() {
            Ok(Some(_)) | Err(_) => {
                self.finish();
                true
            }
            Ok(None) => false,
        }
    }

    fn stop(&mut self) {
        if self.done {
            return;
        }
        for c in &mut self.children {
            let _ = c.kill();
        }
        self.finish();
        if let Some((bin, args)) = self.on_stop.take() {
            let mut c = Command::new(bin);
            c.args(args);
            audit_once("tts_stop", &c, None);
            let _ = c
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

impl Drop for ChildPlayback {
    fn drop(&mut self) {
        self.stop();
    }
}

/// One audit line per process for each engine program (legacy-launches
/// #24–30: a line per spoken sentence would flood the log). The spoken
/// text itself is never logged: it shows as `<text>`.
fn audit_once(kind: &str, c: &Command, text: Option<&str>) {
    let mut argv = lillith_core::launch_audit::argv_of_command(c);
    if let Some(t) = text {
        for a in argv.iter_mut().skip(1) {
            if a == t {
                *a = "<text>".into();
            }
        }
    }
    lillith_core::launch_audit::record_once(kind, &argv);
}

/// Removes control characters and leading dashes (so a line can never be
/// read as a command-line option).
pub fn arg_safe(text: &str) -> String {
    let t: String = text.chars().filter(|c| !c.is_control()).collect();
    t.trim_start_matches(|c: char| c == '-' || c.is_whitespace())
        .to_string()
}

/// espeak-ng (`-v en+f3 -s 145 -p 50`, the Python prototype's voice).
#[derive(Debug, Clone)]
pub struct EspeakNg {
    pub bin: PathBuf,
}

impl Engine for EspeakNg {
    fn name(&self) -> &'static str {
        "espeak-ng"
    }
    fn start(&mut self, text: &str, o: &SpeakOpts) -> std::io::Result<Box<dyn Playback>> {
        let voice = o.voice.clone().unwrap_or_else(|| "en+f3".into());
        let safe = arg_safe(text);
        let mut c = Command::new(&self.bin);
        c.arg("-v")
            .arg(voice)
            .arg("-s")
            .arg(format!("{}", (145.0 * o.rate).round() as i32))
            .arg("-p")
            .arg("50")
            .arg("-a")
            .arg(format!("{}", (o.volume * 100.0).round() as i32))
            .arg(&safe);
        audit_once("tts_engine", &c, Some(&safe));
        let child = c
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Box::new(ChildPlayback {
            children: vec![child],
            pump: None,
            on_stop: None,
            done: false,
        }))
    }
}

/// speech-dispatcher's `spd-say -w`.
#[derive(Debug, Clone)]
pub struct SpdSay {
    pub bin: PathBuf,
}

impl SpdSay {
    /// `-r` / `-i` take -100..=100.
    pub fn rate_arg(rate: f32) -> i32 {
        ((rate - 1.0) * 100.0).round().clamp(-100.0, 100.0) as i32
    }
    pub fn volume_arg(volume: f32) -> i32 {
        (volume * 200.0 - 100.0).round().clamp(-100.0, 100.0) as i32
    }
}

impl Engine for SpdSay {
    fn name(&self) -> &'static str {
        "spd-say"
    }
    fn start(&mut self, text: &str, o: &SpeakOpts) -> std::io::Result<Box<dyn Playback>> {
        let mut cmd = Command::new(&self.bin);
        cmd.arg("-w")
            .arg("-r")
            .arg(Self::rate_arg(o.rate).to_string())
            .arg("-i")
            .arg(Self::volume_arg(o.volume).to_string());
        if let Some(v) = &o.voice {
            cmd.arg("-t").arg(v);
        }
        let safe = arg_safe(text);
        cmd.arg("--").arg(&safe);
        audit_once("tts_engine", &cmd, Some(&safe));
        let child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Box::new(ChildPlayback {
            children: vec![child],
            pump: None,
            on_stop: Some((self.bin.clone(), vec!["-C".into()])),
            done: false,
        }))
    }
}

/// The local voice daemon (`lillith-voiced`, PROTOCOL.md v2). `start`
/// opens a connection and sends `say` with `wait:true`; a reader thread
/// waits for the reply (it comes when playback ends), so [`Playback::poll`]
/// never blocks. [`Playback::stop`] sends `stop` on a second connection
/// (barge-in / mute / speakers off), which also makes the pending `say`
/// reply at once.
#[derive(Debug, Clone)]
pub struct DaemonEngine {
    pub client: DaemonClient,
    /// Preset to request (`None` = the daemon's default).
    pub preset: Option<String>,
    /// Set when the cloned 'lillith' voice is unavailable (fallback p1).
    pub fallback: Option<String>,
}

impl DaemonEngine {
    /// For a daemon that answered `ping` with `info`; `want` is
    /// `[voice] preset` (falls back to `p1`).
    pub fn new(client: DaemonClient, info: &PingInfo, want: &str) -> Self {
        let fallback = info.fallback_warning(want);
        if let Some(w) = &fallback {
            tracing::warn!("voice: {w}");
        }
        Self {
            client,
            preset: info.pick_preset(want),
            fallback,
        }
    }
}

struct DaemonPlayback {
    client: DaemonClient,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    reply: std::sync::mpsc::Receiver<Result<SayReply, String>>,
    finished: bool,
}

impl Playback for DaemonPlayback {
    fn poll(&mut self) -> bool {
        if self.finished {
            return true;
        }
        match self.reply.try_recv() {
            Ok(r) => {
                if let Err(e) = r {
                    tracing::debug!("voice daemon say: {e}");
                }
                self.finished = true;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.finished = true,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
        self.finished
    }

    fn stop(&mut self) {
        if self.finished || self.done.load(std::sync::atomic::Ordering::SeqCst) {
            self.finished = true;
            return;
        }
        if let Err(e) = self.client.stop(Some("interrupt")) {
            tracing::debug!("voice daemon stop: {e}");
        }
        self.finished = true;
    }
}

impl Engine for DaemonEngine {
    fn name(&self) -> &'static str {
        "voiced"
    }
    fn voice_warning(&self) -> Option<String> {
        self.fallback.clone()
    }
    fn start(&mut self, text: &str, o: &SpeakOpts) -> std::io::Result<Box<dyn Playback>> {
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        let req = Request::Say {
            text,
            wait: true,
            preset: self.preset.clone(),
            speed: Some(o.rate),
        };
        // Blocks until playback ends: no read timeout on this connection.
        let conn = self
            .client
            .send(&req, None)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let (tx, rx) = std::sync::mpsc::channel();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = std::sync::Arc::clone(&done);
        std::thread::Builder::new()
            .name("lillith-voiced-say".into())
            .spawn(move || {
                use std::io::BufRead;
                let mut r = std::io::BufReader::new(conn);
                let mut line = String::new();
                let res = match r.read_line(&mut line) {
                    Ok(0) => Err("connection closed".to_string()),
                    Ok(_) => crate::daemon::parse_reply(&line)
                        .map(|v| SayReply::from_value(&v))
                        .map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                };
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
                let _ = tx.send(res);
            })?;
        Ok(Box::new(DaemonPlayback {
            client: self.client.clone(),
            done,
            reply: rx,
            finished: false,
        }))
    }
}

/// Raw S16LE mono player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Player {
    Aplay(PathBuf),
    Paplay(PathBuf),
}

impl Player {
    fn command(&self, sample_rate: u32) -> Command {
        match self {
            Player::Aplay(b) => {
                let mut c = Command::new(b);
                c.args(["-q", "-t", "raw", "-f", "S16_LE", "-c", "1", "-r"])
                    .arg(sample_rate.to_string());
                c
            }
            Player::Paplay(b) => {
                let mut c = Command::new(b);
                c.args(["--raw", "--format=s16le", "--channels=1"])
                    .arg(format!("--rate={sample_rate}"));
                c
            }
        }
    }
}

/// Piper (`piper --model M --output_raw`) piped through a volume-scaling
/// pump into `aplay` / `paplay`.
#[derive(Debug, Clone)]
pub struct Piper {
    pub bin: PathBuf,
    pub model: PathBuf,
    pub sample_rate: u32,
    pub player: Player,
}

/// Reads `"sample_rate": N` from the model's `.onnx.json` (22050 if absent).
pub fn piper_sample_rate(model: &Path) -> u32 {
    let mut json = model.as_os_str().to_owned();
    json.push(".json");
    std::fs::read_to_string(PathBuf::from(json))
        .ok()
        .and_then(|s| {
            let i = s.find("\"sample_rate\"")?;
            let rest = &s[i + 13..];
            let rest = rest.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
            let n: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            n.parse().ok()
        })
        .filter(|r| (8000..=96000).contains(r))
        .unwrap_or(22050)
}

/// Scales S16LE samples in place by `gain` (an odd trailing byte is carried).
pub fn scale_s16le(buf: &mut [u8], gain: f32) {
    if (gain - 1.0).abs() < 1e-4 {
        return;
    }
    for ch in buf.chunks_exact_mut(2) {
        let s = i16::from_le_bytes([ch[0], ch[1]]) as f32 * gain;
        let s = s.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        ch.copy_from_slice(&s.to_le_bytes());
    }
}

impl Engine for Piper {
    fn name(&self) -> &'static str {
        "piper"
    }
    fn start(&mut self, text: &str, o: &SpeakOpts) -> std::io::Result<Box<dyn Playback>> {
        let mut cmd = Command::new(&self.bin);
        cmd.arg("--model").arg(&self.model).arg("--output_raw");
        if (o.rate - 1.0).abs() > 1e-3 {
            cmd.arg("--length_scale")
                .arg(format!("{:.3}", 1.0 / o.rate.max(0.1)));
        }
        audit_once("tts_engine", &cmd, None);
        let mut piper = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut pc = self.player.command(self.sample_rate);
        audit_once("tts_player", &pc, None);
        let mut player = match pc
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(p) => p,
            Err(e) => {
                let _ = piper.kill();
                let _ = piper.wait();
                return Err(e);
            }
        };
        if let Some(mut stdin) = piper.stdin.take() {
            let _ = stdin.write_all(arg_safe(text).as_bytes());
            let _ = stdin.write_all(b"\n");
        } // dropped = EOF: piper speaks the one line and exits
        let mut src = piper.stdout.take().expect("piped");
        let mut dst = player.stdin.take().expect("piped");
        let gain = o.volume;
        let pump = std::thread::Builder::new()
            .name("lillith-voice-pump".into())
            .spawn(move || {
                let mut buf = vec![0u8; 8192];
                let mut carry: Option<u8> = None;
                loop {
                    let start = usize::from(carry.is_some());
                    if let Some(b) = carry.take() {
                        buf[0] = b;
                    }
                    let n = match src.read(&mut buf[start..]) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n + start,
                    };
                    let even = n & !1;
                    if even < n {
                        carry = Some(buf[n - 1]);
                    }
                    scale_s16le(&mut buf[..even], gain);
                    if dst.write_all(&buf[..even]).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Box::new(ChildPlayback {
            children: vec![piper, player],
            pump: Some(pump),
            on_stop: None,
            done: false,
        }))
    }
}

/// `bin` on `$PATH` (executable regular file), if any.
#[cfg(unix)]
pub fn which(bin: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(bin)).find(|p| {
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    })
}

/// `bin.exe` on `%PATH%`, if any.
#[cfg(not(unix))]
pub fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let file = format!("{bin}{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(&path)
        .map(|d| d.join(&file))
        .find(|p| p.is_file())
}

/// Windows' built-in offline voices (SAPI, `System.Speech`) through a
/// hidden `powershell.exe`. The line goes in on stdin, never on the
/// command line; nothing leaves the computer.
#[cfg(windows)]
#[derive(Debug, Clone)]
pub struct Sapi {
    pub bin: PathBuf,
}

#[cfg(windows)]
impl Sapi {
    /// SAPI `Rate` takes -10..=10 (0 = normal).
    pub fn rate_arg(rate: f32) -> i32 {
        ((rate - 1.0) * 10.0).round().clamp(-10.0, 10.0) as i32
    }
}

#[cfg(windows)]
impl Engine for Sapi {
    fn name(&self) -> &'static str {
        "sapi"
    }
    fn start(&mut self, text: &str, o: &SpeakOpts) -> std::io::Result<Box<dyn Playback>> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let voice = o
            .voice
            .as_deref()
            .map(|v| v.replace('\'', "''"))
            .map(|v| format!("try {{ $s.SelectVoice('{v}') }} catch {{}};"))
            .unwrap_or_default();
        let script = format!(
            "Add-Type -AssemblyName System.Speech; \
             $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
             $s.Rate = {}; $s.Volume = {}; {voice} \
             $t = [Console]::In.ReadToEnd(); if ($t.Trim()) {{ $s.Speak($t) }}",
            Self::rate_arg(o.rate),
            (o.volume * 100.0).round().clamp(0.0, 100.0) as i32,
        );
        let mut c = Command::new(&self.bin);
        c.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
        audit_once("tts_engine", &c, None);
        let mut child = c
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            let t: String = text.chars().filter(|c| !c.is_control()).collect();
            let _ = stdin.write_all(t.as_bytes());
        }
        Ok(Box::new(ChildPlayback {
            children: vec![child],
            pump: None,
            on_stop: None,
            done: false,
        }))
    }
}

/// What [`detect`] chose and why.
pub struct Detected {
    pub engine: Box<dyn Engine>,
    /// One human-readable line for the log.
    pub reason: String,
}

/// Picks the engine for `cfg` with the real `$PATH` and the real daemon
/// socket.
pub fn detect(cfg: &VoiceConfig) -> Detected {
    detect_full(cfg, &which, &ping_daemon)
}

/// Asks the daemon at `[voice] daemon_socket` (or the default endpoint)
/// for `ping`; `None` when it is missing or does not answer.
pub fn ping_daemon(cfg: &VoiceConfig) -> Option<(DaemonClient, PingInfo)> {
    let client = DaemonClient::from_config(cfg)?;
    if !client.present() {
        return None;
    }
    match client.ping() {
        Ok(info) => Some((client, info)),
        Err(e) => {
            tracing::debug!("voice daemon at {}: {e}", client.endpoint().display());
            None
        }
    }
}

/// A daemon probe for [`detect_full`] ([`ping_daemon`], or a fake in tests).
pub type DaemonProbe = dyn Fn(&VoiceConfig) -> Option<(DaemonClient, PingInfo)>;

/// [`detect`] with an injectable binary lookup and no daemon (tests).
pub fn detect_with(cfg: &VoiceConfig, lookup: &dyn Fn(&str) -> Option<PathBuf>) -> Detected {
    detect_full(cfg, lookup, &|_| None)
}

/// [`detect`] with an injectable binary lookup and daemon probe. The daemon
/// goes first (`auto` / `daemon`) only when the probe answers; otherwise
/// the subprocess chain is used, so a missing socket falls back cleanly.
pub fn detect_full(
    cfg: &VoiceConfig,
    lookup: &dyn Fn(&str) -> Option<PathBuf>,
    probe: &DaemonProbe,
) -> Detected {
    let none = |reason: String| Detected {
        engine: Box::new(NullEngine),
        reason,
    };
    if !cfg.enabled {
        return none("voice disabled in config".into());
    }
    let mut why = Vec::new();
    if matches!(cfg.engine, EngineChoice::Auto | EngineChoice::Daemon) {
        match probe(cfg) {
            Some((client, info)) => {
                let engine = DaemonEngine::new(client, &info, cfg.preset());
                return Detected {
                    reason: format!(
                        "voice daemon ({}, preset {})",
                        engine.client.endpoint().display(),
                        engine.preset.as_deref().unwrap_or("daemon default")
                    ),
                    engine: Box::new(engine),
                };
            }
            None => why.push("voice daemon not running".to_string()),
        }
    }
    let order: &[EngineChoice] = match cfg.engine {
        EngineChoice::None => return none("engine = \"none\"".into()),
        EngineChoice::Auto | EngineChoice::Daemon => &[
            EngineChoice::Piper,
            EngineChoice::EspeakNg,
            EngineChoice::SpdSay,
        ],
        EngineChoice::Piper => &[EngineChoice::Piper],
        EngineChoice::EspeakNg => &[EngineChoice::EspeakNg],
        EngineChoice::SpdSay => &[EngineChoice::SpdSay],
    };
    for e in order {
        match e {
            EngineChoice::Piper => {
                let Some(bin) = lookup("piper") else {
                    why.push("piper not on PATH".to_string());
                    continue;
                };
                let Some(model) = cfg.model.clone().filter(|m| m.is_file()) else {
                    why.push(match &cfg.model {
                        Some(m) => format!("piper model {} not found", m.display()),
                        None => "piper needs [voice] model".into(),
                    });
                    continue;
                };
                let aplay = || lookup("aplay").map(Player::Aplay);
                let paplay = || lookup("paplay").map(Player::Paplay);
                let player = match cfg.player {
                    PlayerChoice::Auto => aplay().or_else(paplay),
                    PlayerChoice::Aplay => aplay(),
                    PlayerChoice::Paplay => paplay(),
                };
                let Some(player) = player else {
                    why.push("piper needs aplay or paplay".into());
                    continue;
                };
                let sample_rate = piper_sample_rate(&model);
                return Detected {
                    reason: format!("piper ({}, {sample_rate} Hz, {player:?})", model.display()),
                    engine: Box::new(Piper {
                        bin,
                        model,
                        sample_rate,
                        player,
                    }),
                };
            }
            EngineChoice::EspeakNg => match lookup("espeak-ng") {
                Some(bin) => {
                    return Detected {
                        reason: "espeak-ng".into(),
                        engine: Box::new(EspeakNg { bin }),
                    }
                }
                None => why.push("espeak-ng not on PATH".into()),
            },
            EngineChoice::SpdSay => match lookup("spd-say") {
                Some(bin) => {
                    return Detected {
                        reason: "spd-say".into(),
                        engine: Box::new(SpdSay { bin }),
                    }
                }
                None => why.push("spd-say not on PATH".into()),
            },
            _ => {}
        }
    }
    // Windows: the built-in SAPI voices when nothing else was found.
    #[cfg(windows)]
    if matches!(cfg.engine, EngineChoice::Auto | EngineChoice::Daemon) {
        if let Some(bin) = lookup("powershell") {
            return Detected {
                reason: "sapi (Windows built-in voices)".into(),
                engine: Box::new(Sapi { bin }),
            };
        }
        why.push("powershell not on PATH".into());
    }
    none(format!(
        "no TTS engine ({}); staying silent",
        why.join("; ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(found: &'static [&'static str]) -> impl Fn(&str) -> Option<PathBuf> {
        move |b: &str| {
            found
                .contains(&b)
                .then(|| PathBuf::from(format!("/usr/bin/{b}")))
        }
    }

    #[test]
    fn auto_falls_through_to_silence() {
        let d = detect_with(&VoiceConfig::default(), &fake(&[]));
        assert_eq!(d.engine.name(), "none");
        assert!(!d.engine.is_real());
        assert!(
            d.reason.contains("piper not on PATH") && d.reason.contains("spd-say"),
            "{}",
            d.reason
        );
    }

    #[test]
    fn auto_skips_piper_without_model() {
        let d = detect_with(
            &VoiceConfig::default(),
            &fake(&["piper", "aplay", "spd-say"]),
        );
        assert_eq!(d.engine.name(), "spd-say");
        let d = detect_with(
            &VoiceConfig::default(),
            &fake(&["piper", "espeak-ng", "spd-say"]),
        );
        assert_eq!(d.engine.name(), "espeak-ng");
    }

    #[test]
    fn piper_with_model_and_player() {
        let dir = std::env::temp_dir().join(format!("lillith-voice-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let model = dir.join("amy.onnx");
        std::fs::write(&model, b"x").unwrap();
        std::fs::write(
            dir.join("amy.onnx.json"),
            br#"{"audio": {"sample_rate": 16000}}"#,
        )
        .unwrap();
        let cfg = VoiceConfig {
            model: Some(model.clone()),
            ..VoiceConfig::default()
        };
        let d = detect_with(&cfg, &fake(&["piper", "paplay"]));
        assert_eq!(d.engine.name(), "piper");
        assert!(d.reason.contains("16000"), "{}", d.reason);
        // No player → next engine.
        let d = detect_with(&cfg, &fake(&["piper", "spd-say"]));
        assert_eq!(d.engine.name(), "spd-say");
        assert_eq!(piper_sample_rate(&dir.join("missing.onnx")), 22050);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disabled_and_none_never_probe() {
        let probe = |_: &str| -> Option<PathBuf> { panic!("must not probe") };
        let off = VoiceConfig {
            enabled: false,
            ..VoiceConfig::default()
        };
        assert_eq!(detect_with(&off, &probe).engine.name(), "none");
        let none = VoiceConfig {
            engine: EngineChoice::None,
            ..VoiceConfig::default()
        };
        assert_eq!(detect_with(&none, &probe).engine.name(), "none");
    }

    #[test]
    fn arg_and_sample_helpers() {
        assert_eq!(arg_safe("--rm -rf\n hi"), "rm -rf hi");
        assert_eq!(arg_safe("Hey!\u{7}"), "Hey!");
        assert_eq!(
            (
                SpdSay::rate_arg(1.0),
                SpdSay::rate_arg(2.0),
                SpdSay::rate_arg(0.5)
            ),
            (0, 100, -50)
        );
        assert_eq!(
            (
                SpdSay::volume_arg(0.5),
                SpdSay::volume_arg(1.0),
                SpdSay::volume_arg(0.0)
            ),
            (0, 100, -100)
        );
        let mut b = [0x00, 0x40, 0x00, 0xC0]; // 16384, -16384
        scale_s16le(&mut b, 0.5);
        assert_eq!(
            [
                i16::from_le_bytes([b[0], b[1]]),
                i16::from_le_bytes([b[2], b[3]])
            ],
            [8192, -8192]
        );
    }

    #[test]
    fn daemon_missing_falls_back_to_espeak() {
        for engine in [EngineChoice::Auto, EngineChoice::Daemon] {
            let cfg = VoiceConfig {
                engine,
                daemon_socket: Some(PathBuf::from("/nonexistent/lillith/voiced.sock")),
                ..VoiceConfig::default()
            };
            let d = detect_full(&cfg, &fake(&["espeak-ng"]), &ping_daemon);
            assert_eq!(d.engine.name(), "espeak-ng", "{engine:?}");
            let d = detect_full(&cfg, &fake(&[]), &ping_daemon);
            assert_eq!(d.engine.name(), "none");
            assert!(
                d.reason.contains("voice daemon not running"),
                "{}",
                d.reason
            );
        }
        // Explicit engines never probe the daemon.
        let cfg = VoiceConfig {
            engine: EngineChoice::EspeakNg,
            ..VoiceConfig::default()
        };
        let probe = |_: &VoiceConfig| -> Option<(DaemonClient, PingInfo)> { panic!("probed") };
        assert_eq!(
            detect_full(&cfg, &fake(&["espeak-ng"]), &probe)
                .engine
                .name(),
            "espeak-ng"
        );
    }

    #[cfg(unix)]
    #[test]
    fn daemon_engine_says_waits_and_stops_on_a_second_connection() {
        use crate::daemon::{fake::FakeDaemon, request_command};
        use serde_json::Value;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::time::Duration;
        let release = Arc::new(AtomicBool::new(false));
        let r = Arc::clone(&release);
        let d = FakeDaemon::start(
            "engine",
            Box::new(move |v: &Value| match request_command(v) {
                "ping" => vec![r#"{"ok":true,"protocol":2,"presets":["p1","bella"]}"#.into()],
                "say" => {
                    // Blocks like a real wait:true say until stopped.
                    for _ in 0..400 {
                        if r.load(Ordering::SeqCst) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    vec![r#"{"ok":true,"utt":7,"stopped":true}"#.into()]
                }
                "stop" => {
                    r.store(true, Ordering::SeqCst);
                    vec![r#"{"ok":true}"#.into()]
                }
                _ => vec![r#"{"ok":false,"error":"?"}"#.into()],
            }),
        );
        let cfg = VoiceConfig {
            daemon_socket: Some(d.sock.clone()),
            ..VoiceConfig::default()
        };
        let mut det = detect_full(&cfg, &fake(&["espeak-ng"]), &ping_daemon);
        assert_eq!(det.engine.name(), "voiced", "{}", det.reason);
        assert!(
            det.reason.contains("preset p1"),
            "lillith missing → p1: {}",
            det.reason
        );
        let mut p = det
            .engine
            .start("Hiii babe.", &SpeakOpts::from_config(&cfg))
            .unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert!(!p.poll(), "still speaking");
        p.stop();
        assert!(p.poll());
        let seen = d.seen.lock().unwrap().clone();
        let say = seen.iter().find(|v| request_command(v) == "say").unwrap();
        assert_eq!(say["text"], "Hiii babe.");
        assert_eq!(say["wait"], true);
        assert_eq!(say["preset"], "p1");
        assert!(d.commands().contains(&"stop".to_string()));
        // A finished say polls done without a stop.
        let mut p = det
            .engine
            .start("again", &SpeakOpts::from_config(&cfg))
            .unwrap();
        for _ in 0..200 {
            if p.poll() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(p.poll());
    }

    #[test]
    fn null_engine_is_instant() {
        let mut e = NullEngine;
        let mut p = e
            .start("hi", &SpeakOpts::from_config(&VoiceConfig::default()))
            .unwrap();
        assert!(p.poll());
    }
}
