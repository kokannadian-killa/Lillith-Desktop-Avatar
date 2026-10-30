//! GGUF scanner and picker data: finds local `.gguf` files, reads their
//! headers and ranks them for a low-RAM CPU laptop. Read-only filesystem
//! walk; nothing is downloaded.

use crate::gguf::{self, GgufHeader};
use crate::host::HostInfo;
use crate::paths;
use crate::template::ChatTemplate;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Where a model file came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    /// A regular `*.gguf` file.
    File,
    /// An Ollama blob (`sha256-…`, no extension) that is a GGUF model.
    /// Usable by llama-server directly, but its name comes only from the header.
    OllamaBlob,
}

/// One model the picker can show.
#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub path: PathBuf,
    pub file_name: String,
    pub size_bytes: u64,
    pub source: ModelSource,
    /// `general.name`, else the file stem.
    pub name: String,
    pub architecture: Option<String>,
    /// Training context length (`<arch>.context_length`).
    pub context_length: Option<u64>,
    /// Quant type (`Q4_K_M`, ...), from `general.file_type` else the file name.
    pub quant: Option<String>,
    pub file_type: Option<u64>,
    /// Parameters in billions (from `general.size_label`, else estimated
    /// from file size and quant).
    pub params_b: Option<f64>,
    pub params_estimated: bool,
    pub chat_template: ChatTemplate,
    /// True if the GGUF embeds a Jinja chat template.
    pub has_embedded_template: bool,
    /// Rough resident memory at `ctx` 4096 (weights + KV cache + overhead).
    pub est_ram_bytes: u64,
    /// Set when the header couldn't be read (the file is still listed).
    pub header_error: Option<String>,
}

impl ModelInfo {
    /// Builds the info for one file, reading its header.
    pub fn from_path(path: &Path, source: ModelSource) -> std::io::Result<Self> {
        let size_bytes = std::fs::metadata(path)?.len();
        let file_name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (header, header_error) = match gguf::read_header(path) {
            Ok(h) => (Some(h), None),
            Err(e) => (None, Some(e.to_string())),
        };
        Ok(Self::build(
            path,
            file_name,
            size_bytes,
            source,
            header.as_ref(),
            header_error,
        ))
    }

    fn build(
        path: &Path,
        file_name: String,
        size_bytes: u64,
        source: ModelSource,
        h: Option<&GgufHeader>,
        header_error: Option<String>,
    ) -> Self {
        let stem = file_name
            .strip_suffix(".gguf")
            .unwrap_or(&file_name)
            .to_string();
        let name = h
            .and_then(|h| h.name())
            .map(str::to_string)
            .unwrap_or_else(|| stem.clone());
        let architecture = h.and_then(|h| h.architecture()).map(str::to_string);
        let file_type = h.and_then(|h| h.file_type());
        let quant = file_type
            .and_then(gguf::file_type_name)
            .map(str::to_string)
            .or_else(|| gguf::quant_from_filename(&file_name))
            .or_else(|| gguf::quant_from_filename(&name));
        let label = h
            .and_then(|h| h.get_str("general.size_label"))
            .and_then(parse_size_label);
        let (params_b, params_estimated) = match label {
            Some(p) => (Some(p), false),
            None => {
                let bpw = gguf::bits_per_weight(quant.as_deref().unwrap_or("Q4_K_M"));
                let p = size_bytes as f64 * 8.0 / bpw / 1e9;
                (Some((p * 100.0).round() / 100.0), true)
            }
        };
        let jinja = h.and_then(|h| h.chat_template());
        let chat_template = ChatTemplate::detect(
            jinja,
            architecture.as_deref(),
            &format!("{name} {file_name}"),
        );
        let est_ram_bytes = size_bytes + kv_cache_bytes(h, params_b, 4096) + OVERHEAD_BYTES;
        Self {
            path: path.to_path_buf(),
            file_name,
            size_bytes,
            source,
            name,
            architecture,
            context_length: h.and_then(|h| h.context_length()),
            quant,
            file_type,
            params_b,
            params_estimated,
            chat_template,
            has_embedded_template: jinja.is_some(),
            est_ram_bytes,
            header_error,
        }
    }
}

/// Compute buffers + runtime, roughly, for llama.cpp on CPU.
const OVERHEAD_BYTES: u64 = 350 * 1024 * 1024;

/// f16 KV cache size for `ctx` tokens, from the header when possible.
fn kv_cache_bytes(h: Option<&GgufHeader>, params_b: Option<f64>, ctx: u64) -> u64 {
    if let Some(h) = h {
        if let Some(a) = h.architecture() {
            let g = |k: &str| h.get_u64(&format!("{a}.{k}"));
            if let (Some(layers), Some(emb), Some(heads)) = (
                g("block_count"),
                g("embedding_length"),
                g("attention.head_count"),
            ) {
                let kv_heads = g("attention.head_count_kv").unwrap_or(heads).max(1);
                let kv_dim = emb * kv_heads / heads.max(1);
                return 2 * layers * ctx * kv_dim * 2;
            }
        }
    }
    // ~150 MB per billion params at 4k context (GQA models are lower).
    (params_b.unwrap_or(3.0) * 150.0 * 1024.0 * 1024.0 * ctx as f64 / 4096.0) as u64
}

/// `"3B"` -> 3.0, `"494M"` -> 0.494, `"8x7B"` -> 56.0.
pub fn parse_size_label(s: &str) -> Option<f64> {
    let s = s.trim().to_ascii_uppercase();
    let (mult, rest) = match s.split_once('X') {
        Some((n, r)) => (n.parse::<f64>().ok()?, r.to_string()),
        None => (1.0, s),
    };
    let (num, unit) = rest.split_at(rest.find(|c: char| c.is_ascii_alphabetic())?);
    let n: f64 = num.parse().ok()?;
    let scale = match unit.chars().next()? {
        'K' => 1e-6,
        'M' => 1e-3,
        'B' => 1.0,
        'T' => 1e3,
        _ => return None,
    };
    Some(mult * n * scale)
}

/// What to scan.
#[derive(Debug, Clone, Serialize)]
pub struct ScanOptions {
    pub dirs: Vec<PathBuf>,
    /// How deep to descend below each dir.
    pub max_depth: usize,
    /// Sniff extension-less files in Ollama `blobs/` dirs for the GGUF magic.
    pub include_ollama_blobs: bool,
    /// Files smaller than this are ignored (vocab-only / test GGUFs).
    pub min_size_bytes: u64,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            dirs: default_scan_dirs(),
            max_depth: 4,
            include_ollama_blobs: true,
            min_size_bytes: 16 * 1024 * 1024,
        }
    }
}

/// Default search locations, in order. Missing ones are reported, not errors.
/// A bundled `bundle/models/` next to the install comes first
/// ([`crate::bundle::model_dirs`], only when it exists), then `~/AI/models`.
pub fn default_scan_dirs() -> Vec<PathBuf> {
    default_scan_dirs_with(&crate::bundle::model_dirs())
}

/// [`default_scan_dirs`] with the bundled model folders given explicitly.
pub fn default_scan_dirs_with(bundled: &[PathBuf]) -> Vec<PathBuf> {
    let mut v = bundled.to_vec();
    if let Some(h) = paths::home_dir() {
        for d in [
            "AI/models",
            "models",
            ".cache/llama.cpp",
            ".ollama/models/blobs",
            ".lmstudio/models",
            "Downloads",
        ] {
            v.push(h.join(d));
        }
    }
    if let Some(c) = paths::cache_home() {
        v.push(c.join("llama.cpp"));
        v.push(c.join("huggingface/hub"));
    }
    if let Some(d) = paths::data_home() {
        v.push(d.join("lillith/models"));
        v.push(d.join("llama.cpp"));
        v.push(d.join("models"));
    }
    // System-wide Ollama service stores (Arch package: /var/lib/ollama).
    v.push(PathBuf::from("/var/lib/ollama/blobs"));
    v.push(PathBuf::from("/var/lib/ollama/.ollama/models/blobs"));
    v.push(PathBuf::from("/usr/share/ollama/.ollama/models/blobs"));
    dedup_paths(v)
}

fn dedup_paths(v: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    v.into_iter().filter(|p| seen.insert(p.clone())).collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct DirStatus {
    pub path: PathBuf,
    pub exists: bool,
    pub models_found: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    pub models: Vec<ModelInfo>,
    pub dirs: Vec<DirStatus>,
    /// Non-fatal problems (unreadable dirs, ...).
    pub warnings: Vec<String>,
}

/// Scans `opts.dirs`. Never fails as a whole; problems go into `warnings`.
pub fn scan(opts: &ScanOptions) -> ScanReport {
    scan_with_progress(opts, &CancelToken::new(), |_| {})
}

/// Cooperative cancel flag for [`scan_with_progress`] / [`spawn_scan`].
/// Cheap to clone; all clones share the flag.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
    /// The shared flag, for code that already uses an `Arc<AtomicBool>`.
    pub fn flag(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.0.clone()
    }
}

impl From<std::sync::Arc<std::sync::atomic::AtomicBool>> for CancelToken {
    fn from(f: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Self {
        Self(f)
    }
}

/// Progress of a running scan.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScanProgress {
    /// Directory being read right now.
    pub dir: PathBuf,
    /// Directory entries looked at so far (all dirs).
    pub files_seen: usize,
    /// Models found so far.
    pub models_found: usize,
}

/// [`scan`] with a progress callback (called on entering each directory and
/// for each model found) and a cancel flag. When cancelled it stops soon and
/// returns what it found so far (check `cancel.is_cancelled()`).
pub fn scan_with_progress(
    opts: &ScanOptions,
    cancel: &CancelToken,
    mut progress: impl FnMut(ScanProgress),
) -> ScanReport {
    let mut seen = HashSet::new();
    let mut report = ScanReport {
        models: Vec::new(),
        dirs: Vec::new(),
        warnings: Vec::new(),
    };
    let mut st = WalkState {
        cancel,
        progress: &mut progress,
        files_seen: 0,
    };
    for dir in &opts.dirs {
        if cancel.is_cancelled() {
            break;
        }
        let exists = dir.is_dir();
        let before = report.models.len();
        if exists {
            walk(dir, 0, opts, &mut seen, &mut report, &mut st);
        }
        report.dirs.push(DirStatus {
            path: dir.clone(),
            exists,
            models_found: report.models.len() - before,
        });
    }
    report
}

struct WalkState<'a> {
    cancel: &'a CancelToken,
    progress: &'a mut dyn FnMut(ScanProgress),
    files_seen: usize,
}

/// A scan running on a background thread: poll progress and the result
/// each frame. `Send`.
#[derive(Debug)]
pub struct ScanHandle {
    progress_rx: std::sync::mpsc::Receiver<ScanProgress>,
    done_rx: std::sync::mpsc::Receiver<ScanReport>,
    last: Option<ScanProgress>,
    cancel: CancelToken,
}

impl ScanHandle {
    /// Latest progress (drains the queue; keeps the last value).
    pub fn progress(&mut self) -> Option<&ScanProgress> {
        while let Ok(p) = self.progress_rx.try_recv() {
            self.last = Some(p);
        }
        self.last.as_ref()
    }
    /// The finished report (`Some` once).
    pub fn try_recv(&mut self) -> Option<ScanReport> {
        self.done_rx.try_recv().ok()
    }
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }
    /// Blocks until done.
    pub fn wait(self) -> Option<ScanReport> {
        self.done_rx.recv().ok()
    }
}

/// Runs [`scan_with_progress`] on a new thread.
pub fn spawn_scan(opts: ScanOptions) -> ScanHandle {
    let (ptx, progress_rx) = std::sync::mpsc::channel();
    let (dtx, done_rx) = std::sync::mpsc::channel();
    let cancel = CancelToken::new();
    let c2 = cancel.clone();
    std::thread::Builder::new()
        .name("lillith-scan".into())
        .spawn(move || {
            let r = scan_with_progress(&opts, &c2, |p| {
                let _ = ptx.send(p);
            });
            let _ = dtx.send(r);
        })
        .expect("spawn scan thread");
    ScanHandle {
        progress_rx,
        done_rx,
        last: None,
        cancel,
    }
}

fn walk(
    dir: &Path,
    depth: usize,
    opts: &ScanOptions,
    seen: &mut HashSet<PathBuf>,
    r: &mut ScanReport,
    st: &mut WalkState<'_>,
) {
    if st.cancel.is_cancelled() {
        return;
    }
    (st.progress)(ScanProgress {
        dir: dir.to_path_buf(),
        files_seen: st.files_seen,
        models_found: r.models.len(),
    });
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            r.warnings.push(format!("{}: {e}", dir.display()));
            return;
        }
    };
    let in_blobs = dir.file_name().is_some_and(|n| n == "blobs");
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if st.cancel.is_cancelled() {
            return;
        }
        st.files_seen += 1;
        let p = e.path();
        let Ok(meta) = std::fs::metadata(&p) else {
            continue;
        }; // follows symlinks (HF snapshots)
        if meta.is_dir() {
            let hidden = e.file_name().to_string_lossy().starts_with('.');
            if depth < opts.max_depth && !hidden && !e.file_type().is_ok_and(|t| t.is_symlink()) {
                walk(&p, depth + 1, opts, seen, r, st);
            }
            continue;
        }
        if !meta.is_file() || meta.len() < opts.min_size_bytes {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        let source = if name.to_ascii_lowercase().ends_with(".gguf") {
            ModelSource::File
        } else if opts.include_ollama_blobs
            && in_blobs
            && name.starts_with("sha256")
            && gguf::is_gguf(&p)
        {
            ModelSource::OllamaBlob
        } else {
            continue;
        };
        // Skip split shards after the first (`-00002-of-00003.gguf`).
        if is_later_shard(&name) {
            continue;
        }
        let canon = std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
        if !seen.insert(canon) {
            continue;
        }
        match ModelInfo::from_path(&p, source) {
            Ok(m) => {
                r.models.push(m);
                (st.progress)(ScanProgress {
                    dir: dir.to_path_buf(),
                    files_seen: st.files_seen,
                    models_found: r.models.len(),
                });
            }
            Err(err) => r.warnings.push(format!("{}: {err}", p.display())),
        }
    }
}

pub(crate) fn is_later_shard(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower
        .rsplit_once("-of-")
        .and_then(|(a, _)| a.rsplit('-').next())
        .and_then(|n| n.parse::<u32>().ok())
        .is_some_and(|n| n > 1)
}

/// How strongly a model is recommended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Recommended,
    Ok,
    NotRecommended,
}

#[derive(Debug, Clone, Serialize)]
pub struct Recommendation {
    /// Index into the `models` slice that was ranked.
    pub index: usize,
    pub path: PathBuf,
    pub name: String,
    pub score: i32,
    pub tier: Tier,
    pub reasons: Vec<String>,
}

/// Total RAM from `/proc/meminfo` (Windows: `GlobalMemoryStatusEx`), if readable.
#[cfg(windows)]
pub fn total_ram_bytes() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: plain out-struct with its size set, as the API requires.
    unsafe {
        let mut m: MEMORYSTATUSEX = std::mem::zeroed();
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        (GlobalMemoryStatusEx(&mut m) != 0).then_some(m.ullTotalPhys)
    }
}

/// Total RAM from `/proc/meminfo`, if readable.
#[cfg(not(windows))]
pub fn total_ram_bytes() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = s.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// Memory the model may use: half of RAM (the desktop, browser and Lillith
/// herself need the rest). Defaults to an 8 GB machine when unknown.
pub fn model_ram_budget(total_ram: Option<u64>) -> u64 {
    total_ram.unwrap_or(8 * 1024 * 1024 * 1024) / 2
}

/// Ranks `models` for this machine's CPU with `total_ram` bytes of RAM
/// (target: i5-7200U, 8 GB). Best first. Prefers ~3B Q4_K_M chat models,
/// uncensored/abliterated ones first within a size tier. See [`recommend_for`].
pub fn recommend(models: &[ModelInfo], total_ram: Option<u64>) -> Vec<Recommendation> {
    recommend_for(
        models,
        &HostInfo {
            cpu: crate::host::cpu_info(),
            total_ram,
        },
    )
}

/// Ranks `models` for `host` (injectable CPU + RAM). Best first.
///
/// - On a low-power CPU (U-series such as the i5-7200U, or 4 threads or
///   fewer) with 8-16 GB, ~3B models get a boost and 7-8B ones a penalty:
///   Dolphin 3.0 Llama 3.2 3B Q4_K_M is the pick there.
/// - Within a size tier, uncensored models ([`is_uncensored`]: dolphin,
///   uncensored, abliterated, hermes, nous) rank above stock instruct ones.
/// - If nothing would be recommended (e.g. the only GGUF is Mistral 7B on
///   8 GB), the best readable model that fits in RAM is kept as a fallback
///   (tier `Ok`) instead of being written off. Nothing is ever downloaded;
///   see [`preferred_model_hint`] for what to fetch by hand.
pub fn recommend_for(models: &[ModelInfo], host: &HostInfo) -> Vec<Recommendation> {
    let budget = model_ram_budget(host.total_ram);
    let mut out: Vec<Recommendation> = models
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let (score, reasons) = score_model(m, budget, host);
            let tier = if m.header_error.is_some() || score < 20 {
                Tier::NotRecommended
            } else if score >= 70 {
                Tier::Recommended
            } else {
                Tier::Ok
            };
            Recommendation {
                index: i,
                path: m.path.clone(),
                name: m.name.clone(),
                score,
                tier,
                reasons,
            }
        })
        .collect();
    out.sort_by(|a, b| b.score.cmp(&a.score).then(a.path.cmp(&b.path)));
    if !out.iter().any(|r| r.tier != Tier::NotRecommended) {
        let ram = host.total_ram.unwrap_or(8 * GIB_U64);
        if let Some(r) = out.iter_mut().find(|r| {
            let m = &models[r.index];
            m.header_error.is_none()
                && m.est_ram_bytes <= ram / 10 * 9
                && !matches!(
                    m.quant.as_deref().map(str::to_ascii_uppercase).as_deref(),
                    Some("F16" | "BF16" | "F32")
                )
        }) {
            r.tier = Tier::Ok;
            r.reasons.push(format!(
                "fallback: the best usable model found; for speed get {PREFERRED_3B_FILE} into ~/models/"
            ));
        }
    }
    out
}

const GIB_U64: u64 = 1024 * 1024 * 1024;

/// Ko's pick for a low-power laptop: Dolphin 3.0 Llama 3.2 3B, Q4_K_M (~2 GB).
pub const PREFERRED_3B_FILE: &str = "dolphin3.0-llama3.2-3b-q4_k_m.gguf";
/// Human name of [`PREFERRED_3B_FILE`].
pub const PREFERRED_3B_NAME: &str = "Dolphin 3.0 Llama 3.2 3B (Q4_K_M)";

/// Name keywords of uncensored (unmoderated) fine-tunes.
pub const UNCENSORED_KEYWORDS: &[&str] =
    &["dolphin", "uncensored", "abliterated", "hermes", "nous"];

/// Whether `m` is an uncensored / abliterated fine-tune, judged from the file
/// name and the GGUF `general.name`.
pub fn is_uncensored(m: &ModelInfo) -> bool {
    let n = format!("{} {}", m.name, m.file_name).to_ascii_lowercase();
    UNCENSORED_KEYWORDS.iter().any(|k| n.contains(k))
}

/// What to fetch by hand when the preferred 3B model is missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelHint {
    /// Exact file name to get.
    pub file_name: String,
    /// Where to put it (`~/models`, expanded when `$HOME` is known).
    pub dir: PathBuf,
    /// One line for the wizard to show.
    pub message: String,
}

/// If [`PREFERRED_3B_FILE`] is not among `models` and this host wants a 3B
/// model (or nothing found is recommended), the exact file name to get and
/// the folder to put it in. The wizard never downloads it.
pub fn preferred_model_hint(models: &[ModelInfo], host: &HostInfo) -> Option<ModelHint> {
    if models
        .iter()
        .any(|m| m.file_name.eq_ignore_ascii_case(PREFERRED_3B_FILE))
    {
        return None;
    }
    let need = host.prefers_3b()
        || !recommend_for(models, host)
            .iter()
            .any(|r| r.tier == Tier::Recommended);
    if !need {
        return None;
    }
    let dir = paths::home_dir()
        .map(|h| h.join("models"))
        .unwrap_or_else(|| PathBuf::from("~/models"));
    let why = if host.prefers_3b() {
        "is the right size for this low-power CPU"
    } else {
        "is the recommended model"
    };
    Some(ModelHint {
        file_name: PREFERRED_3B_FILE.into(),
        message: format!(
            "{PREFERRED_3B_NAME} {why}. Get {PREFERRED_3B_FILE} and put it in ~/models/ ({}), then rescan. Lillith never downloads models.",
            dir.display()
        ),
        dir,
    })
}

fn gib(b: u64) -> String {
    format!("{:.1} GB", b as f64 / (1024.0 * 1024.0 * 1024.0))
}

fn score_model(m: &ModelInfo, budget: u64, host: &HostInfo) -> (i32, Vec<String>) {
    let mut s = 0i32;
    let mut why = Vec::new();
    if let Some(e) = &m.header_error {
        return (-100, vec![format!("unreadable header: {e}")]);
    }
    match m.params_b {
        Some(p) if (2.5..=4.5).contains(&p) => {
            s += 40;
            why.push(format!("{p:.1}B: sweet spot for a 2-core CPU"));
        }
        Some(p) if (1.0..2.5).contains(&p) => {
            s += 30;
            why.push(format!("{p:.1}B: fast, a bit less smart"));
        }
        Some(p) if (4.5..=8.5).contains(&p) => {
            s += 12;
            why.push(format!("{p:.1}B: smart but slow on an i5-7200U"));
        }
        Some(p) if p < 1.0 => {
            s += 8;
            why.push(format!(
                "{p:.1}B: very fast, weak persona (good for smoke tests)"
            ));
        }
        Some(p) => {
            s -= 20;
            why.push(format!("{p:.1}B: too large for CPU-only chat"));
        }
        None => {}
    }
    if host.prefers_3b() {
        let cpu = if host.cpu.is_u_series() {
            "low-power U-series CPU"
        } else {
            "4-thread CPU"
        };
        match m.params_b {
            Some(p) if (2.5..=4.5).contains(&p) => {
                s += 20;
                why.push(format!("3B class: the right size for this {cpu}"));
            }
            Some(p) if (4.5..=8.5).contains(&p) => {
                s -= 10;
                why.push(format!("7-8B: slow replies on this {cpu} (fallback only)"));
            }
            _ => {}
        }
    }
    let q = m.quant.as_deref().unwrap_or("").to_ascii_uppercase();
    let (qs, qwhy) = match q.as_str() {
        "Q4_K_M" => (25, "Q4_K_M: best size/quality balance"),
        "Q4_K_S" | "Q5_K_M" | "IQ4_XS" | "IQ4_NL" => (18, "good quant"),
        "Q5_K_S" | "Q4_0" | "Q4_1" | "Q5_0" | "Q5_1" => (12, "ok quant"),
        "Q6_K" => (5, "Q6_K: larger/slower than needed"),
        "Q8_0" => (0, "Q8_0: twice the RAM of Q4 for little gain"),
        "F16" | "BF16" | "F32" => (-20, "unquantized: far too heavy for CPU"),
        "" => (0, "unknown quant"),
        _ => (-5, "very low-bit quant: quality drops"),
    };
    s += qs;
    why.push(qwhy.to_string());
    if m.est_ram_bytes <= budget {
        s += 20;
        why.push(format!(
            "~{} RAM, fits the {} budget",
            gib(m.est_ram_bytes),
            gib(budget)
        ));
    } else {
        s -= 50;
        why.push(format!(
            "~{} RAM, over the {} budget",
            gib(m.est_ram_bytes),
            gib(budget)
        ));
    }
    let lname = format!("{} {}", m.name, m.file_name).to_ascii_lowercase();
    if is_uncensored(m) {
        // Beats a stock instruct of the same size tier even one quant step
        // better (Q4_K_M vs Q4_K_S: 7), never a whole tier (10+).
        s += 9;
        why.push(if lname.contains("dolphin") {
            "Dolphin, uncensored: Ko's preferred brain".into()
        } else {
            "uncensored / abliterated: no refusals".into()
        });
    }
    if [
        "instruct",
        "chat",
        "dolphin",
        "hermes",
        "nous",
        "abliterated",
        "uncensored",
        "-it",
    ]
    .iter()
    .any(|k| lname.contains(k))
    {
        s += 5;
    } else if lname.contains("base") {
        s -= 10;
        why.push("base model, not chat-tuned".into());
    }
    if m.source == ModelSource::OllamaBlob {
        s -= 2;
        why.push("Ollama blob (prefer the named file or the Ollama model)".into());
    }
    (s, why)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gguf::tests::{build_gguf, TestVal};
    use crate::host::CpuInfo;

    fn fake(name: &str, size_mb: u64, quant: Option<&str>, params: f64) -> ModelInfo {
        ModelInfo {
            path: PathBuf::from(format!("/m/{name}")),
            file_name: name.into(),
            size_bytes: size_mb * 1024 * 1024,
            source: ModelSource::File,
            name: name.into(),
            architecture: Some("llama".into()),
            context_length: Some(8192),
            quant: quant.map(str::to_string),
            file_type: None,
            params_b: Some(params),
            params_estimated: false,
            chat_template: ChatTemplate::ChatMl,
            has_embedded_template: true,
            est_ram_bytes: size_mb * 1024 * 1024 + 800 * 1024 * 1024,
            header_error: None,
        }
    }

    #[test]
    fn ranks_3b_q4km_first_on_8gb() {
        let models = vec![
            fake(
                "qwen2.5-0.5b-instruct-q4_k_m.gguf",
                470,
                Some("Q4_K_M"),
                0.5,
            ),
            fake("llama-3.1-8b-instruct-f16.gguf", 15300, Some("F16"), 8.0),
            fake(
                "dolphin3.0-llama3.2-3b-q4_k_m.gguf",
                1930,
                Some("Q4_K_M"),
                3.2,
            ),
            fake("llama-3.2-3b-instruct-q8_0.gguf", 3300, Some("Q8_0"), 3.2),
        ];
        let r = recommend(&models, Some(8 * 1024 * 1024 * 1024));
        assert!(r[0].name.starts_with("dolphin3.0"), "{r:#?}");
        assert_eq!(r[0].tier, Tier::Recommended);
        assert_eq!(r.last().unwrap().tier, Tier::NotRecommended);
        assert!(r.last().unwrap().name.contains("f16"));
    }

    const GIB: u64 = 1024 * 1024 * 1024;

    /// Ko's HP Spectre x360: i5-7200U (2 cores / 4 threads), 8 GB (~7.6 GiB).
    fn spectre() -> HostInfo {
        HostInfo {
            cpu: CpuInfo {
                model_name: "Intel(R) Core(TM) i5-7200U CPU @ 2.50GHz".into(),
                logical: 4,
                physical: Some(2),
            },
            total_ram: Some(7 * GIB + 600 * 1024 * 1024),
        }
    }

    fn workstation() -> HostInfo {
        HostInfo {
            cpu: CpuInfo {
                model_name: "AMD Ryzen 9 7950X 16-Core Processor".into(),
                logical: 32,
                physical: Some(16),
            },
            total_ram: Some(64 * GIB),
        }
    }

    fn mistral7b() -> ModelInfo {
        fake(
            "mistral-7b-instruct-v0.3.Q4_K_M.gguf",
            4170,
            Some("Q4_K_M"),
            7.2,
        )
    }

    fn dolphin3b() -> ModelInfo {
        fake(PREFERRED_3B_FILE, 1930, Some("Q4_K_M"), 3.2)
    }

    #[test]
    fn low_power_8g_prefers_3b_over_7b() {
        let models = vec![
            mistral7b(),
            fake(
                "llama-3.2-3b-instruct-q4_k_m.gguf",
                1930,
                Some("Q4_K_M"),
                3.2,
            ),
            dolphin3b(),
        ];
        let r = recommend_for(&models, &spectre());
        assert_eq!(r[0].name, PREFERRED_3B_FILE, "{r:#?}");
        assert_eq!(r[0].tier, Tier::Recommended);
        assert!(
            r[0].reasons.iter().any(|w| w.contains("U-series")),
            "{:?}",
            r[0].reasons
        );
        assert!(r[1].name.starts_with("llama-3.2-3b"));
        let m = r.iter().find(|x| x.name.starts_with("mistral")).unwrap();
        assert!(
            m.reasons.iter().any(|w| w.contains("slow")),
            "{:?}",
            m.reasons
        );
        // 4-thread (non-U) part with 16 GB: still 3B first.
        let quad = HostInfo {
            cpu: CpuInfo {
                model_name: "Intel(R) Core(TM) i5-6500 CPU @ 3.20GHz".into(),
                logical: 4,
                physical: Some(4),
            },
            total_ram: Some(16 * GIB),
        };
        assert!(quad.prefers_3b());
        assert_eq!(recommend_for(&models, &quad)[0].name, PREFERRED_3B_FILE);
        assert!(
            preferred_model_hint(&models, &spectre()).is_none(),
            "already present"
        );
    }

    #[test]
    fn mistral_7b_alone_stays_as_fallback_with_hint() {
        let models = vec![mistral7b()];
        let host = spectre();
        let r = recommend_for(&models, &host);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tier, Tier::Ok, "{r:#?}");
        assert!(r[0].reasons.iter().any(|w| w.starts_with("fallback")));
        let h = preferred_model_hint(&models, &host).expect("hint");
        assert_eq!(h.file_name, "dolphin3.0-llama3.2-3b-q4_k_m.gguf");
        assert!(h.dir.ends_with("models"));
        assert!(
            h.message.contains("dolphin3.0-llama3.2-3b-q4_k_m.gguf"),
            "{}",
            h.message
        );
        assert!(h.message.contains("~/models/"), "{}", h.message);
        assert!(h.message.contains("never downloads"));
        // ~/models is scanned by default.
        if let Some(home) = paths::home_dir() {
            assert!(default_scan_dirs().contains(&home.join("models")));
        }
        // A big machine with a recommended model needs no hint.
        let big = vec![fake(
            "qwen2.5-7b-instruct-q4_k_m.gguf",
            4400,
            Some("Q4_K_M"),
            7.6,
        )];
        let r = recommend_for(&big, &workstation());
        assert_eq!(r[0].tier, Tier::Ok);
        assert!(!r[0].reasons.iter().any(|w| w.starts_with("fallback")));
        // An F16 that doesn't fit is never promoted.
        let f16 = vec![fake(
            "llama-3.1-8b-instruct-f16.gguf",
            15300,
            Some("F16"),
            8.0,
        )];
        assert_eq!(recommend_for(&f16, &host)[0].tier, Tier::NotRecommended);
    }

    #[test]
    fn uncensored_ranks_above_stock_within_a_size_tier() {
        let stock = fake(
            "Llama-3.2-3B-Instruct-Q4_K_M.gguf",
            1930,
            Some("Q4_K_M"),
            3.2,
        );
        let ablit = fake(
            "Llama-3.2-3B-Instruct-abliterated.Q4_K_S.gguf",
            1830,
            Some("Q4_K_S"),
            3.2,
        );
        // Detected from the GGUF general.name too, not just the file name.
        let mut hermes = fake("model-3b-q4_k_m.gguf", 1930, Some("Q4_K_M"), 3.2);
        hermes.name = "Hermes 3 Llama 3.2 3B".into();
        assert!(is_uncensored(&ablit) && is_uncensored(&hermes) && is_uncensored(&dolphin3b()));
        assert!(!is_uncensored(&stock) && !is_uncensored(&mistral7b()));
        for host in [spectre(), workstation()] {
            let models = vec![stock.clone(), ablit.clone()];
            let r = recommend_for(&models, &host);
            assert!(r[0].name.contains("abliterated"), "{r:#?}");
            let models = vec![stock.clone(), hermes.clone()];
            assert_eq!(
                recommend_for(&models, &host)[0].name,
                "Hermes 3 Llama 3.2 3B"
            );
            // ...but the size tier still wins: a 1.5B uncensored never beats a 3B.
            let small = fake(
                "qwen2.5-1.5b-instruct-uncensored-q4_k_m.gguf",
                990,
                Some("Q4_K_M"),
                1.5,
            );
            let r = recommend_for(&[small, stock.clone()], &host);
            assert!(r[0].name.starts_with("Llama-3.2-3B"), "{r:#?}");
        }
        // Dolphin 3.0 3B stays the top pick on Ko's laptop.
        let all = vec![stock, ablit, hermes, mistral7b(), dolphin3b()];
        assert_eq!(recommend_for(&all, &spectre())[0].name, PREFERRED_3B_FILE);
    }

    #[test]
    fn size_labels() {
        assert_eq!(parse_size_label("3B"), Some(3.0));
        assert_eq!(parse_size_label("494M"), Some(0.494));
        assert_eq!(parse_size_label("8x7B"), Some(56.0));
        assert_eq!(parse_size_label("nope"), None);
    }

    #[test]
    fn shards() {
        assert!(is_later_shard("m-00002-of-00003.gguf"));
        assert!(!is_later_shard("m-00001-of-00003.gguf"));
        assert!(!is_later_shard("dolphin3.0-llama3.2-3b-q4_k_m.gguf"));
    }

    #[test]
    fn scans_a_temp_tree() {
        let root = std::env::temp_dir().join(format!("lillith-setup-scan-{}", std::process::id()));
        let sub = root.join("a/b");
        let blobs = root.join("blobs");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(&blobs).unwrap();
        let mut g = build_gguf(&[
            ("general.architecture", TestVal::Str("qwen2")),
            ("general.name", TestVal::Str("Tiny Test")),
            ("qwen2.context_length", TestVal::U32(32768)),
            ("general.file_type", TestVal::U32(15)),
        ]);
        g.resize(4096, 0);
        std::fs::write(sub.join("tiny-q4_k_m.gguf"), &g).unwrap();
        std::fs::write(blobs.join("sha256-abc"), &g).unwrap();
        std::fs::write(blobs.join("sha256-notgguf"), vec![1u8; 4096]).unwrap();
        std::fs::write(root.join("readme.txt"), b"x").unwrap();
        let opts = ScanOptions {
            dirs: vec![root.clone(), root.join("missing")],
            max_depth: 4,
            include_ollama_blobs: true,
            min_size_bytes: 1,
        };
        let r = scan(&opts);
        std::fs::remove_dir_all(&root).ok();
        assert_eq!(r.models.len(), 2, "{r:#?}");
        let f = r
            .models
            .iter()
            .find(|m| m.source == ModelSource::File)
            .unwrap();
        assert_eq!(f.name, "Tiny Test");
        assert_eq!(f.quant.as_deref(), Some("Q4_K_M"));
        assert_eq!(f.context_length, Some(32768));
        assert_eq!(f.chat_template, ChatTemplate::ChatMl);
        assert!(r.models.iter().any(|m| m.source == ModelSource::OllamaBlob));
        assert!(!r.dirs[1].exists);
    }

    #[test]
    fn default_dirs_have_no_build_box_paths() {
        // Only the user's own folders (and the system Ollama stores); no
        // leftover build-machine path such as /workspace/models.
        let dirs = default_scan_dirs_with(&[]);
        assert!(
            dirs.iter().all(|d| !d.starts_with("/workspace")),
            "{dirs:?}"
        );
    }

    #[test]
    fn bundled_models_dir_is_scanned_first() {
        let root =
            std::env::temp_dir().join(format!("lillith-setup-bmodels-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pkg = root.join("Lillith");
        let exe_dir = pkg.join("bin");
        std::fs::create_dir_all(&exe_dir).unwrap();
        let roots = crate::bundle::install_roots_from(Some(&exe_dir), None, &[]);
        // No bundle/models folder: nothing extra, ~/AI/models still leads.
        let plain = default_scan_dirs_with(&crate::bundle::model_dirs_from(&roots));
        if let Some(home) = paths::home_dir() {
            assert_eq!(plain.first(), Some(&home.join("AI/models")));
            assert!(plain.contains(&home.join("AI/models")));
        }
        let models = pkg.join("bundle/models");
        std::fs::create_dir_all(&models).unwrap();
        let mut g = build_gguf(&[("general.architecture", TestVal::Str("llama"))]);
        g.resize(2048, 0);
        std::fs::write(models.join("dolphin3.0-llama3.2-3b-q4_k_m.gguf"), &g).unwrap();
        let bundled = crate::bundle::model_dirs_from(&roots);
        assert_eq!(bundled, vec![models.clone()]);
        let dirs = default_scan_dirs_with(&bundled);
        assert_eq!(dirs.first(), Some(&models));
        if let Some(home) = paths::home_dir() {
            assert_eq!(dirs.get(1), Some(&home.join("AI/models")));
        }
        let r = scan(&ScanOptions {
            dirs: bundled,
            max_depth: 4,
            include_ollama_blobs: false,
            min_size_bytes: 1,
        });
        std::fs::remove_dir_all(&root).ok();
        assert_eq!(r.models.len(), 1, "{r:#?}");
        assert_eq!(r.models[0].file_name, "dolphin3.0-llama3.2-3b-q4_k_m.gguf");
    }

    fn tree_with_models(tag: &str, n: usize) -> PathBuf {
        let root = std::env::temp_dir().join(format!("lillith-setup-{tag}-{}", std::process::id()));
        let mut g = build_gguf(&[("general.architecture", TestVal::Str("qwen2"))]);
        g.resize(2048, 0);
        for i in 0..n {
            let d = root.join(format!("d{i}"));
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(format!("m{i}-q4_k_m.gguf")), &g).unwrap();
        }
        root
    }

    #[test]
    fn progress_is_reported() {
        let root = tree_with_models("progress", 3);
        let opts = ScanOptions {
            dirs: vec![root.clone()],
            max_depth: 3,
            include_ollama_blobs: false,
            min_size_bytes: 1,
        };
        let mut events = Vec::new();
        let r = scan_with_progress(&opts, &CancelToken::new(), |p| events.push(p));
        std::fs::remove_dir_all(&root).ok();
        assert_eq!(r.models.len(), 3);
        assert_eq!(events.first().unwrap().dir, root);
        assert_eq!(events.last().unwrap().models_found, 3);
        assert!(events.last().unwrap().files_seen >= 6);
        assert!(events
            .windows(2)
            .all(|w| w[0].files_seen <= w[1].files_seen));
    }

    #[test]
    fn cancel_stops_the_scan() {
        let root = tree_with_models("cancel", 4);
        let opts = ScanOptions {
            dirs: vec![root.clone(), root.clone()],
            max_depth: 3,
            include_ollama_blobs: false,
            min_size_bytes: 1,
        };
        let cancel = CancelToken::new();
        let c = cancel.clone();
        let r = scan_with_progress(&opts, &cancel, |p| {
            if p.models_found >= 1 {
                c.cancel();
            }
        });
        // Pre-cancelled: nothing at all.
        let r2 = scan_with_progress(&opts, &cancel, |_| {});
        std::fs::remove_dir_all(&root).ok();
        assert!(cancel.is_cancelled());
        assert_eq!(r.models.len(), 1);
        assert!(r2.models.is_empty());
    }

    #[test]
    fn spawn_scan_runs_in_background() {
        fn assert_send<T: Send>() {}
        assert_send::<ScanHandle>();
        assert_send::<CancelToken>();
        let root = tree_with_models("spawn", 2);
        let opts = ScanOptions {
            dirs: vec![root.clone()],
            max_depth: 3,
            include_ollama_blobs: false,
            min_size_bytes: 1,
        };
        let mut h = spawn_scan(opts);
        let start = std::time::Instant::now();
        let r = loop {
            h.progress();
            if let Some(r) = h.try_recv() {
                break r;
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(10));
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        std::fs::remove_dir_all(&root).ok();
        assert_eq!(r.models.len(), 2);
        assert_eq!(h.progress().map(|p| p.models_found), Some(2));
    }
}
