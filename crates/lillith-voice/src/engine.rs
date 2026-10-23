//! TTS engines run as subprocesses (feature `voice`): Piper piped to
//! `aplay`/`paplay`, espeak-ng, spd-say, and a silent [`NullEngine`].
//!
//! Text is always passed as a single argv entry or on stdin, never through
//! a shell. [`detect`] picks the engine once at start-up and says why.

use crate::config::{EngineChoice, PlayerChoice, VoiceConfig};
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
            let _ = Command::new(bin)
                .args(args)
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
        let child = Command::new(&self.bin)
            .arg("-v")
            .arg(voice)
            .arg("-s")
            .arg(format!("{}", (145.0 * o.rate).round() as i32))
            .arg("-p")
            .arg("50")
            .arg("-a")
            .arg(format!("{}", (o.volume * 100.0).round() as i32))
            .arg(arg_safe(text))
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
        let child = cmd
            .arg("--")
            .arg(arg_safe(text))
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
        let mut piper = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut player = match self
            .player
            .command(self.sample_rate)
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
        let mut child = Command::new(&self.bin)
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
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

/// Picks the engine for `cfg` with the real `$PATH`.
pub fn detect(cfg: &VoiceConfig) -> Detected {
    detect_with(cfg, &which)
}

/// [`detect`] with an injectable binary lookup (tests).
pub fn detect_with(cfg: &VoiceConfig, lookup: &dyn Fn(&str) -> Option<PathBuf>) -> Detected {
    let none = |reason: String| Detected {
        engine: Box::new(NullEngine),
        reason,
    };
    if !cfg.enabled {
        return none("voice disabled in config".into());
    }
    let mut why = Vec::new();
    let order: &[EngineChoice] = match cfg.engine {
        EngineChoice::None => return none("engine = \"none\"".into()),
        EngineChoice::Auto => &[
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
    if matches!(cfg.engine, EngineChoice::Auto) {
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
    fn null_engine_is_instant() {
        let mut e = NullEngine;
        let mut p = e
            .start("hi", &SpeakOpts::from_config(&VoiceConfig::default()))
            .unwrap();
        assert!(p.poll());
    }
}
