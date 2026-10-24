//! MP4 clip source: alpha-packed H.264 pose videos decoded by a local
//! `ffmpeg` into premultiplied frames.
//!
//! Packing (`tools/video-pipeline/pack_mp4.py`, layout [`LAYOUT`]): every
//! video frame is `2W × H`; the left half is the clip frame's straight
//! (un-premultiplied) colour, black where transparent, the right half its
//! alpha as grey. H.264 High, yuv420p, full range, BT.709, no B-frames, one
//! video frame per clip frame (timing stays in the manifest:
//! `frame_durations_ms` / `fps`).
//!
//! Decoding runs `ffmpeg` (found on `PATH`, or `$LILLITH_FFMPEG`) with the
//! file fed on stdin, so a *standby* process can be spawned ahead of time
//! and the ~150 ms of ffmpeg start-up (dynamic linking) is off the critical
//! path. Frames stream into a shared buffer from a reader thread as they
//! decode, so playback can start on frame 0 while the rest decodes.
//! Nothing touches the network (`-protocol_whitelist pipe,file`).
//!
//! Decoded clips are kept in an LRU cache with a byte budget
//! (`$LILLITH_VIDEO_CACHE_MB`, default [`DEFAULT_BUDGET_MB`]); a clip whose
//! video is missing, fails to decode or has the wrong frame count is
//! remembered as failed and the caller falls back to its PNG page.
//! `LILLITH_VIDEO=0` turns the video source off (PNG pages only).

use crate::Canvas;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tiny_skia::IntSize;

/// The only packing layout: colour left, alpha (grey) right.
pub const LAYOUT: &str = "alpha_side_by_side";
/// Decoded alpha at or below this is snapped to 0 (codec noise).
pub const ALPHA_LO: u8 = 3;
/// Decoded alpha at or above this is snapped to 255 (codec noise).
pub const ALPHA_HI: u8 = 251;
/// Default decoded-frame cache budget in MiB (≈ 300 cells of 400×400).
pub const DEFAULT_BUDGET_MB: usize = 192;
/// yuv -> rgb exactly as the packer encoded it.
const SCALE_VF: &str = "scale=in_color_matrix=bt709:in_range=pc:flags=accurate_rnd+full_chroma_int";

/// Video decode errors.
#[derive(Debug, thiserror::Error)]
pub enum VideoError {
    #[error("cannot run ffmpeg ({ffmpeg}): {err}")]
    Spawn { ffmpeg: String, err: std::io::Error },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Decode(String),
}

/// Video source settings.
#[derive(Debug, Clone)]
pub struct VideoConfig {
    /// Use MP4 sources at all (`LILLITH_VIDEO=0` turns them off).
    pub enabled: bool,
    /// ffmpeg binary (`$LILLITH_FFMPEG`, default `ffmpeg` on `PATH`).
    pub ffmpeg: PathBuf,
    /// Decoded-frame cache budget in bytes.
    pub budget_bytes: usize,
    /// Keep one ffmpeg process spawned and waiting, so a clip change does
    /// not pay ffmpeg's start-up time.
    pub standby: bool,
    /// How long a blocking [`VideoCache::wait_frame`] waits before giving up
    /// (the caller then uses the PNG page).
    pub max_wait: Duration,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ffmpeg: PathBuf::from("ffmpeg"),
            budget_bytes: DEFAULT_BUDGET_MB << 20,
            standby: true,
            max_wait: Duration::from_secs(5),
        }
    }
}

impl VideoConfig {
    /// Defaults overridden by `LILLITH_VIDEO`, `LILLITH_FFMPEG` and
    /// `LILLITH_VIDEO_CACHE_MB`.
    pub fn from_env() -> Self {
        let mut c = Self::default();
        if let Ok(v) = std::env::var("LILLITH_VIDEO") {
            c.enabled = !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "0" | "off" | "false" | "no" | "png"
            );
        }
        // Only an absolute path to an existing executable file; anything
        // else is refused (logged) and `ffmpeg` on PATH is used.
        if let Some(p) = lillith_core::launch_audit::env_exe("LILLITH_FFMPEG") {
            c.ffmpeg = p;
        }
        if let Some(mb) = std::env::var("LILLITH_VIDEO_CACHE_MB")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
        {
            c.budget_bytes = mb.max(8) << 20;
        }
        c
    }
}

/// ffmpeg arguments that decode `input` (a path, or `pipe:0`) to packed
/// rgb24 frames on stdout.
fn ffmpeg_args(input: &str) -> Vec<String> {
    [
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-threads",
        "1",
        "-flags",
        "low_delay",
        "-protocol_whitelist",
        "file,pipe",
        "-f",
        "mov",
        "-i",
        input,
        "-an",
        "-vf",
        SCALE_VF,
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "pipe:1",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// One packed rgb24 frame (`2w × h`) -> premultiplied `w × h` canvas.
pub fn unpack_frame(rgb: &[u8], w: u32, h: u32) -> Option<Canvas> {
    let (w, hh) = (w as usize, h as usize);
    let stride = 2 * w * 3;
    if w == 0 || rgb.len() < stride * hh {
        return None;
    }
    let mut out = vec![0u8; w * hh * 4];
    for y in 0..hh {
        let row = &rgb[y * stride..(y + 1) * stride];
        let (col, alp) = row.split_at(w * 3);
        let dst = &mut out[y * w * 4..(y + 1) * w * 4];
        for x in 0..w {
            // Grey alpha: take green (highest-precision channel after the
            // yuv -> rgb conversion), snap codec noise at both ends.
            let a = alp[x * 3 + 1];
            let a = if a <= ALPHA_LO {
                0
            } else if a >= ALPHA_HI {
                255
            } else {
                a
            };
            if a == 0 {
                continue;
            }
            let d = &mut dst[x * 4..x * 4 + 4];
            let a16 = a as u16;
            for c in 0..3 {
                d[c] = ((col[x * 3 + c] as u16 * a16 + 127) / 255) as u8;
            }
            d[3] = a;
        }
    }
    Canvas::from_vec(out, IntSize::from_wh(w as u32, h)?)
}

/// Decodes a whole packed MP4 (blocking) into premultiplied `w × h`
/// frames. Used by `import-manifest --check` and tests; the overlay uses
/// the streaming [`VideoCache`].
pub fn decode_all(ffmpeg: &Path, path: &Path, w: u32, h: u32) -> Result<Vec<Canvas>, VideoError> {
    let mut c = Command::new(ffmpeg);
    c.args(ffmpeg_args(&format!("file:{}", path.display())));
    lillith_core::launch_audit::record_command_once("video_decode", &c);
    let out = c
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|err| VideoError::Spawn {
            ffmpeg: ffmpeg.display().to_string(),
            err,
        })?;
    if !out.status.success() {
        return Err(VideoError::Decode(format!(
            "ffmpeg failed on {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let fsz = (2 * w * h * 3) as usize;
    if fsz == 0 || out.stdout.len() % fsz != 0 {
        return Err(VideoError::Decode(format!(
            "{}: {} bytes is not a whole number of {}x{} frames (is it {}x{}?)",
            path.display(),
            out.stdout.len(),
            2 * w,
            h,
            2 * w,
            h
        )));
    }
    out.stdout
        .chunks_exact(fsz)
        .map(|f| unpack_frame(f, w, h).ok_or_else(|| VideoError::Decode("empty frame".into())))
        .collect()
}

/// Streaming decode state shared with the reader thread.
#[derive(Debug, Default)]
struct Decoded {
    frames: Vec<Canvas>,
    done: bool,
    error: Option<String>,
}

type SharedDecode = Arc<(Mutex<Decoded>, Condvar)>;

#[derive(Debug)]
struct Entry {
    state: SharedDecode,
    last_used: u64,
    bytes: usize,
}

/// Result of [`VideoCache::poll`].
#[derive(Debug)]
pub enum Poll {
    /// The frame (premultiplied, `w × h`).
    Ready(Canvas),
    /// Still decoding: try again next tick.
    Pending,
    /// No usable video (off, missing ffmpeg, bad file): use the PNG page.
    Unavailable,
}

/// Cache counters (for logs, tests and the perf report).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VideoStats {
    /// Clips with decoded (or decoding) frames in memory.
    pub resident_clips: usize,
    /// Bytes reserved by them.
    pub resident_bytes: usize,
    /// Decodes started.
    pub decodes: u64,
    /// Clips that failed and fell back to PNG.
    pub failures: usize,
    /// Clips evicted to stay under the budget.
    pub evictions: u64,
}

/// Decoded MP4 clips, LRU-bounded, with a standby ffmpeg (see the module
/// docs).
#[derive(Debug)]
pub struct VideoCache {
    cfg: VideoConfig,
    entries: HashMap<PathBuf, Entry>,
    failed: HashSet<PathBuf>,
    /// ffmpeg could not be started at all: never try again.
    unavailable: bool,
    standby: Option<Child>,
    clock: u64,
    decodes: u64,
    evictions: u64,
}

impl Default for VideoCache {
    fn default() -> Self {
        Self::new(VideoConfig::from_env())
    }
}

impl Drop for VideoCache {
    fn drop(&mut self) {
        if let Some(mut c) = self.standby.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl VideoCache {
    pub fn new(cfg: VideoConfig) -> Self {
        Self {
            cfg,
            entries: HashMap::new(),
            failed: HashSet::new(),
            unavailable: false,
            standby: None,
            clock: 0,
            decodes: 0,
            evictions: 0,
        }
    }

    pub fn config(&self) -> &VideoConfig {
        &self.cfg
    }

    /// `false` once video is off or ffmpeg is missing.
    pub fn enabled(&self) -> bool {
        self.cfg.enabled && !self.unavailable
    }

    /// `true` if `path` failed before (its clip plays from PNG).
    pub fn has_failed(&self, path: &Path) -> bool {
        self.failed.contains(path)
    }

    pub fn stats(&self) -> VideoStats {
        VideoStats {
            resident_clips: self.entries.len(),
            resident_bytes: self.entries.values().map(|e| e.bytes).sum(),
            decodes: self.decodes,
            failures: self.failed.len(),
            evictions: self.evictions,
        }
    }

    fn spawn(&self) -> Result<Child, VideoError> {
        let mut c = Command::new(&self.cfg.ffmpeg);
        c.args(ffmpeg_args("pipe:0"));
        // One line per process: the standby decoder runs per clip change.
        lillith_core::launch_audit::record_command_once("video_decode", &c);
        c.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| VideoError::Spawn {
                ffmpeg: self.cfg.ffmpeg.display().to_string(),
                err,
            })
    }

    /// The standby process if it is still alive, else a fresh one; then a
    /// new standby is started for the next clip.
    fn take_process(&mut self) -> Result<Child, VideoError> {
        let child = match self.standby.take() {
            Some(mut c) => {
                if matches!(c.try_wait(), Ok(None)) {
                    c
                } else {
                    let _ = c.wait();
                    self.spawn()?
                }
            }
            None => self.spawn()?,
        };
        if self.cfg.standby {
            self.standby = self.spawn().ok();
        }
        Ok(child)
    }

    /// Starts decoding `path` (if not resident yet). `w × h` is the clip's
    /// cell size, `frames` its frame count.
    pub fn prefetch(&mut self, path: &Path, w: u32, h: u32, frames: usize) {
        if !self.enabled() || self.failed.contains(path) {
            return;
        }
        self.clock += 1;
        let now = self.clock;
        if let Some(e) = self.entries.get_mut(path) {
            e.last_used = now;
            return;
        }
        match self.start(path, w, h, frames) {
            Ok(state) => {
                self.entries.insert(
                    path.to_path_buf(),
                    Entry {
                        state,
                        last_used: now,
                        bytes: frames * (w * h * 4) as usize,
                    },
                );
                self.evict(path);
            }
            Err(e) => {
                if matches!(e, VideoError::Spawn { .. }) {
                    tracing::warn!(error = %e, "ffmpeg unavailable: pose videos off, using PNG pages");
                    self.unavailable = true;
                } else {
                    tracing::warn!(video = %path.display(), error = %e, "pose video unusable; PNG fallback");
                    self.failed.insert(path.to_path_buf());
                }
            }
        }
    }

    fn start(
        &mut self,
        path: &Path,
        w: u32,
        h: u32,
        frames: usize,
    ) -> Result<SharedDecode, VideoError> {
        let bytes = std::fs::read(path)?;
        let mut child = self.take_process()?;
        self.decodes += 1;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let mut stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let state: SharedDecode = Arc::default();
        // Writer: feeds the MP4 (ffmpeg's output must be drained meanwhile,
        // so this is its own thread).
        std::thread::Builder::new()
            .name("lillith-video-in".into())
            .spawn(move || {
                let _ = stdin.write_all(&bytes);
            })?;
        let st = Arc::clone(&state);
        let name = path.display().to_string();
        std::thread::Builder::new()
            .name("lillith-video-out".into())
            .spawn(move || {
                let fsz = (2 * w * h * 3) as usize;
                let mut buf = vec![0u8; fsz];
                let mut n = 0usize;
                let err = loop {
                    match read_full(&mut stdout, &mut buf) {
                        Ok(true) => {}
                        Ok(false) => break None,
                        Err(e) => break Some(format!("read: {e}")),
                    }
                    let Some(c) = unpack_frame(&buf, w, h) else {
                        break Some("bad frame size".into());
                    };
                    n += 1;
                    let (m, cv) = &*st;
                    m.lock().expect("video lock").frames.push(c);
                    cv.notify_all();
                };
                drop(stdout);
                let mut msg = String::new();
                let _ = stderr.read_to_string(&mut msg);
                let status = child.wait();
                let err = err
                    .or_else(|| match status {
                        Ok(s) if s.success() => None,
                        Ok(s) => Some(format!("ffmpeg {s}: {}", msg.trim())),
                        Err(e) => Some(format!("wait: {e}")),
                    })
                    .or_else(|| {
                        (n != frames).then(|| format!("{n} frames decoded, manifest has {frames}"))
                    });
                if let Some(e) = &err {
                    tracing::warn!(video = %name, error = %e, "pose video decode");
                }
                let (m, cv) = &*st;
                let mut d = m.lock().expect("video lock");
                d.done = true;
                d.error = err;
                cv.notify_all();
            })?;
        Ok(state)
    }

    /// Drops least-recently-used clips (never `keep`) until the reserved
    /// bytes fit the budget.
    fn evict(&mut self, keep: &Path) {
        loop {
            let total: usize = self.entries.values().map(|e| e.bytes).sum();
            if total <= self.cfg.budget_bytes {
                return;
            }
            let Some(victim) = self
                .entries
                .iter()
                .filter(|(p, _)| p.as_path() != keep)
                .min_by_key(|(_, e)| e.last_used)
                .map(|(p, _)| p.clone())
            else {
                return;
            };
            self.entries.remove(&victim);
            self.evictions += 1;
        }
    }

    /// Frame `frame` of the clip video at `path`, waiting at most `wait`.
    pub fn poll(
        &mut self,
        path: &Path,
        w: u32,
        h: u32,
        frames: usize,
        frame: usize,
        wait: Duration,
    ) -> Poll {
        self.prefetch(path, w, h, frames);
        let Some(e) = self.entries.get(path) else {
            return Poll::Unavailable;
        };
        let state = Arc::clone(&e.state);
        let (m, cv) = &*state;
        let deadline = Instant::now() + wait;
        let mut d = m.lock().expect("video lock");
        loop {
            if let Some(c) = d.frames.get(frame) {
                return Poll::Ready(c.clone());
            }
            if d.done || d.error.is_some() {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                return Poll::Pending;
            }
            d = cv.wait_timeout(d, deadline - now).expect("video lock").0;
        }
        drop(d);
        // Finished without this frame: broken video -> PNG from now on.
        self.entries.remove(path);
        self.failed.insert(path.to_path_buf());
        Poll::Unavailable
    }

    /// Blocking [`VideoCache::poll`] (up to [`VideoConfig::max_wait`]);
    /// `None` = use the PNG page.
    pub fn wait_frame(
        &mut self,
        path: &Path,
        w: u32,
        h: u32,
        frames: usize,
        frame: usize,
    ) -> Option<Canvas> {
        match self.poll(path, w, h, frames, frame, self.cfg.max_wait) {
            Poll::Ready(c) => Some(c),
            Poll::Pending => {
                tracing::warn!(video = %path.display(), "pose video decode too slow; PNG fallback");
                None
            }
            Poll::Unavailable => None,
        }
    }
}

/// Fills `buf` completely; `Ok(false)` on a clean EOF before the first byte.
fn read_full(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<bool> {
    let mut got = 0;
    while got < buf.len() {
        match r.read(&mut buf[got..]) {
            Ok(0) if got == 0 => return Ok(false),
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "truncated frame",
                ))
            }
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

/// Locates ffmpeg for tools/tests: `$LILLITH_FFMPEG` or `ffmpeg`; `None`
/// if it does not run.
pub fn find_ffmpeg() -> Option<PathBuf> {
    let p = VideoConfig::from_env().ffmpeg;
    let mut c = Command::new(&p);
    c.arg("-version");
    lillith_core::launch_audit::record_command_once("video_probe", &c);
    c.stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()
        .filter(|s| s.success())
        .map(|_| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes a tiny packed MP4 with ffmpeg (skips the caller if missing).
    pub(crate) fn write_test_video(path: &Path, w: u32, h: u32, n: usize) -> bool {
        let Some(ff) = find_ffmpeg() else {
            return false;
        };
        let mut raw = Vec::new();
        for i in 0..n {
            for y in 0..h {
                for x in 0..2 * w {
                    let inside = (x % w) >= w / 4 && (x % w) < 3 * w / 4 && y >= h / 4;
                    if x < w {
                        let v = if inside { 200u8 } else { 0 };
                        raw.extend_from_slice(&[v, (i * 20) as u8, 60]);
                    } else {
                        let a = if inside { 255u8 } else { 0 };
                        raw.extend_from_slice(&[a, a, a]);
                    }
                }
            }
        }
        let mut c = Command::new(ff)
            .args([
                "-v",
                "error",
                "-nostdin",
                "-y",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "-s",
                &format!("{}x{}", 2 * w, h),
                "-r",
                "12",
                "-i",
                "-",
                "-vf",
                "scale=out_color_matrix=bt709:out_range=pc",
                "-c:v",
                "libx264",
                "-crf",
                "10",
                "-bf",
                "0",
                "-pix_fmt",
                "yuv420p",
                "-color_range",
                "pc",
                "-movflags",
                "+faststart",
            ])
            .arg(path)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(&raw).unwrap();
        c.wait().unwrap().success()
    }

    #[test]
    fn unpack_premultiplies_and_snaps_alpha() {
        // 2x1 cell: px0 opaque red-ish, px1 alpha 128; packed as 4x1.
        let rgb = [
            200, 100, 50, 10, 20, 30, // colour half
            255, 254, 255, 128, 128, 128, // alpha half (green 254 -> 255)
        ];
        let c = unpack_frame(&rgb, 2, 1).unwrap();
        let p = c.pixels();
        assert_eq!(
            (p[0].red(), p[0].green(), p[0].blue(), p[0].alpha()),
            (200, 100, 50, 255)
        );
        assert_eq!(p[1].alpha(), 128);
        assert_eq!(p[1].green(), ((20u16 * 128 + 127) / 255) as u8);
        // Noise near 0 is cut.
        let rgb = [9, 9, 9, 2, 2, 2];
        assert_eq!(unpack_frame(&rgb, 1, 1).unwrap().pixels()[0].alpha(), 0);
        assert!(unpack_frame(&rgb[..3], 1, 1).is_none());
    }

    #[test]
    fn streams_frames_and_falls_back() {
        let dir = std::env::temp_dir().join(format!("lillith-video-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mp4 = dir.join("t.mp4");
        if !write_test_video(&mp4, 32, 32, 5) {
            eprintln!("ffmpeg/libx264 not available; skipping");
            return;
        }
        let all = decode_all(&find_ffmpeg().unwrap(), &mp4, 32, 32).unwrap();
        assert_eq!(all.len(), 5);
        let mut vc = VideoCache::new(VideoConfig::default());
        for i in 0..5 {
            let f = vc.wait_frame(&mp4, 32, 32, 5, i).expect("frame");
            let px = f.pixel(16, 20).unwrap();
            assert_eq!(px.alpha(), 255);
            assert!((px.red() as i32 - 200).abs() <= 4, "{}", px.red());
            assert_eq!(f.pixel(2, 2).unwrap().alpha(), 0, "transparent stays clear");
        }
        assert_eq!(vc.stats().decodes, 1);
        // A frame past the end: decode finished without it -> fallback.
        assert!(vc.wait_frame(&mp4, 32, 32, 5, 9).is_none());
        assert_eq!(vc.stats().failures, 1);
        // Wrong frame count is a failure too.
        let mut vc = VideoCache::new(VideoConfig::default());
        assert!(vc.wait_frame(&mp4, 32, 32, 7, 6).is_none());
        // Missing file / missing ffmpeg -> Unavailable, no panic.
        let mut vc = VideoCache::new(VideoConfig::default());
        assert!(vc.wait_frame(&dir.join("nope.mp4"), 32, 32, 5, 0).is_none());
        let mut vc = VideoCache::new(VideoConfig {
            ffmpeg: dir.join("no-such-ffmpeg"),
            ..VideoConfig::default()
        });
        assert!(vc.wait_frame(&mp4, 32, 32, 5, 0).is_none());
        assert!(!vc.enabled());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lru_budget_evicts_oldest() {
        let dir = std::env::temp_dir().join(format!("lillith-video-lru-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.mp4"), dir.join("b.mp4"));
        if !write_test_video(&a, 32, 32, 4) || !write_test_video(&b, 32, 32, 4) {
            eprintln!("ffmpeg/libx264 not available; skipping");
            return;
        }
        let mut vc = VideoCache::new(VideoConfig {
            budget_bytes: 4 * 32 * 32 * 4, // one clip
            standby: false,
            ..VideoConfig::default()
        });
        assert!(vc.wait_frame(&a, 32, 32, 4, 3).is_some());
        assert!(vc.wait_frame(&b, 32, 32, 4, 3).is_some());
        let s = vc.stats();
        assert_eq!((s.resident_clips, s.evictions), (1, 1));
        assert!(vc.wait_frame(&a, 32, 32, 4, 0).is_some(), "re-decodes");
        assert_eq!(vc.stats().decodes, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
