//! The default model pick for unattended setup (`lillith setup --auto-gguf`,
//! the Arch `lillith-firstrun`, the Windows setup's model page default).
//!
//! Order (Ko's rule):
//! 1. the preferred file at its preferred place,
//!    `~/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf`, always wins;
//! 2. else a Dolphin model (`dolphin*` in the file name) that fits in RAM,
//!    the smallest first;
//! 3. else the smallest model that fits in RAM;
//! 4. else (nothing fits) the smallest model, **with a warning**.
//!
//! "Fits" = file size + [`RAM_HEADROOM`] (the desktop, the browser and
//! Lillith herself) <= total RAM. A model too big for RAM is never chosen
//! silently. Nothing is downloaded; only files already on disk are looked at.

use crate::paths;
use crate::scan::PREFERRED_3B_FILE;
use std::path::{Path, PathBuf};

/// RAM kept free next to the model.
pub const RAM_HEADROOM: u64 = 3 * 1024 * 1024 * 1024;
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// One `.gguf` on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    pub size_bytes: u64,
}

/// Why a model was picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The preferred file at the preferred path.
    Preferred,
    /// A Dolphin model that fits.
    Dolphin,
    /// The smallest model that fits.
    SmallestFits,
    /// Nothing fits: the smallest one (see [`Pick::warning`]).
    SmallestTooBig,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub reason: Reason,
    /// Set when the pick is too big for this computer's RAM.
    pub warning: Option<String>,
}

impl Pick {
    pub fn describe(&self) -> String {
        let why = match self.reason {
            Reason::Preferred => "the preferred Dolphin 3B model",
            Reason::Dolphin => "a Dolphin model that fits in RAM",
            Reason::SmallestFits => "the smallest model that fits in RAM",
            Reason::SmallestTooBig => "the smallest model found (none fits in RAM)",
        };
        format!(
            "{} ({:.1} GB): {why}",
            self.path.display(),
            self.size_bytes as f64 / GIB
        )
    }
}

/// `~/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf` (Windows: under
/// `%USERPROFILE%`).
pub fn preferred_path() -> Option<PathBuf> {
    paths::home_dir().map(|h| h.join("AI").join("models").join(PREFERRED_3B_FILE))
}

/// Whether a model of `size` bytes leaves [`RAM_HEADROOM`] free.
/// Unknown RAM counts as an 8 GB computer.
pub fn fits(size: u64, total_ram: Option<u64>) -> bool {
    let ram = total_ram.unwrap_or(8 * 1024 * 1024 * 1024);
    size.saturating_add(RAM_HEADROOM) <= ram
}

fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (std::fs::canonicalize(a), std::fs::canonicalize(b)),
            (Ok(x), Ok(y)) if x == y
        )
}

fn is_dolphin(p: &Path) -> bool {
    p.file_name()
        .is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().contains("dolphin"))
}

/// Picks the default model (see the module docs). `None` = no candidates.
pub fn pick_default(
    cands: &[Candidate],
    preferred: Option<&Path>,
    total_ram: Option<u64>,
) -> Option<Pick> {
    let too_big = |c: &Candidate| {
        (!fits(c.size_bytes, total_ram)).then(|| {
            format!(
                "{} is {:.1} GB, too big for this computer's {} of RAM (it needs about {:.0} GB free \
                 next to the desktop); she may be very slow or fail to start. A ~3B Q4_K_M model \
                 such as {PREFERRED_3B_FILE} fits better.",
                c.path.display(),
                c.size_bytes as f64 / GIB,
                total_ram.map_or("unknown amount".to_string(), |r| format!("{:.1} GB", r as f64 / GIB)),
                RAM_HEADROOM as f64 / GIB,
            )
        })
    };
    let mk = |c: &Candidate, reason: Reason| Pick {
        path: c.path.clone(),
        size_bytes: c.size_bytes,
        reason,
        warning: too_big(c),
    };
    if let Some(p) = preferred {
        if let Some(c) = cands.iter().find(|c| same_file(&c.path, p)) {
            return Some(mk(c, Reason::Preferred));
        }
    }
    let smallest = |it: &mut dyn Iterator<Item = &Candidate>| {
        it.min_by(|a, b| a.size_bytes.cmp(&b.size_bytes).then(a.path.cmp(&b.path)))
            .cloned()
    };
    let fitting = || cands.iter().filter(|c| fits(c.size_bytes, total_ram));
    if let Some(c) = smallest(&mut fitting().filter(|c| is_dolphin(&c.path))) {
        return Some(mk(&c, Reason::Dolphin));
    }
    if let Some(c) = smallest(&mut fitting()) {
        return Some(mk(&c, Reason::SmallestFits));
    }
    smallest(&mut cands.iter()).map(|c| mk(&c, Reason::SmallestTooBig))
}

/// Where unattended setup looks: `~/AI/models`, `~/models`,
/// `<data>/lillith/models` (2 levels deep).
pub fn default_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = paths::home_dir() {
        v.push(h.join("AI").join("models"));
        v.push(h.join("models"));
    }
    if let Some(d) = paths::data_home() {
        v.push(d.join("lillith").join("models"));
    }
    v
}

/// `.gguf` files under `dirs` (`depth` levels below each), skipping
/// vision projectors (`mmproj`), later shards (`-00002-of-...`) and files
/// under 16 MB (vocab-only / test files).
pub fn find_ggufs(dirs: &[PathBuf], depth: usize) -> Vec<Candidate> {
    fn walk(d: &Path, depth: usize, out: &mut Vec<Candidate>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        let mut ents: Vec<_> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
        ents.sort();
        for p in ents {
            if p.is_dir() {
                if depth > 0 {
                    walk(&p, depth - 1, out);
                }
                continue;
            }
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if !name.ends_with(".gguf")
                || name.contains("mmproj")
                || crate::scan::is_later_shard(&name)
            {
                continue;
            }
            let Ok(m) = std::fs::metadata(&p) else {
                continue;
            };
            if m.len() < 16 * 1024 * 1024 {
                continue;
            }
            if !out.iter().any(|c| same_file(&c.path, &p)) {
                out.push(Candidate {
                    path: p,
                    size_bytes: m.len(),
                });
            }
        }
    }
    let mut out = Vec::new();
    for d in dirs {
        walk(d, depth, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    fn c(p: &str, gb10: u64) -> Candidate {
        Candidate {
            path: PathBuf::from(p),
            size_bytes: gb10 * GB / 10,
        }
    }

    /// Ko's laptop: 7.6 GB RAM, Mistral 7B in ~/models listed first.
    #[test]
    fn kos_laptop_picks_dolphin_not_mistral() {
        let ram = Some(76 * GB / 10);
        let mistral = c("/home/ko/models/mistral-7b-instruct-v0.2.Q4_K_M.gguf", 41);
        let dolphin = c("/home/ko/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf", 20);
        let pref = PathBuf::from("/home/ko/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf");
        let p = pick_default(&[mistral.clone(), dolphin.clone()], Some(&pref), ram).unwrap();
        assert_eq!(p.path, dolphin.path);
        assert_eq!(p.reason, Reason::Preferred);
        assert!(p.warning.is_none());
        // Same file somewhere else: still a Dolphin, still beats Mistral.
        let moved = c("/home/ko/Downloads/dolphin3.0-llama3.2-3b-q4_k_m.gguf", 20);
        let p = pick_default(&[mistral.clone(), moved.clone()], Some(&pref), ram).unwrap();
        assert_eq!((p.path, p.reason), (moved.path, Reason::Dolphin));
    }

    #[test]
    fn preferred_wins_even_over_a_smaller_model_and_warns_if_too_big() {
        let pref = c("/h/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf", 20);
        let tiny = c("/h/models/qwen-0.5b.gguf", 4);
        let p = pick_default(
            &[tiny.clone(), pref.clone()],
            Some(&pref.path),
            Some(16 * GB),
        )
        .unwrap();
        assert_eq!(p.reason, Reason::Preferred);
        let p = pick_default(&[tiny, pref.clone()], Some(&pref.path), Some(4 * GB)).unwrap();
        assert_eq!(p.reason, Reason::Preferred);
        assert!(p.warning.is_some(), "never too big without a warning");
    }

    #[test]
    fn then_smallest_dolphin_that_fits_then_smallest_that_fits() {
        let ram = Some(8 * GB);
        let big_dolphin = c("/m/dolphin-2.9-llama3-8b.Q8_0.gguf", 85);
        let small_dolphin = c("/m/Dolphin3.0-Qwen2.5-1.5B.gguf", 11);
        let mistral = c("/m/mistral-7b.Q4_K_M.gguf", 41);
        let phi = c("/m/phi-3-mini.Q4_K_M.gguf", 23);
        let all = [
            mistral.clone(),
            big_dolphin.clone(),
            small_dolphin.clone(),
            phi.clone(),
        ];
        let p = pick_default(&all, None, ram).unwrap();
        assert_eq!((p.path, p.reason), (small_dolphin.path, Reason::Dolphin));
        // A Dolphin too big for RAM loses to a model that fits.
        let p = pick_default(
            &[mistral.clone(), big_dolphin.clone(), phi.clone()],
            None,
            ram,
        )
        .unwrap();
        assert_eq!((p.path, p.reason), (phi.path, Reason::SmallestFits));
        assert!(p.warning.is_none());
    }

    #[test]
    fn nothing_fits_gives_the_smallest_with_a_warning() {
        let a = c("/m/llama-70b.Q4_K_M.gguf", 400);
        let b = c("/m/mistral-7b.Q4_K_M.gguf", 41);
        let p = pick_default(&[a, b.clone()], None, Some(6 * GB)).unwrap();
        assert_eq!((p.path.clone(), p.reason), (b.path, Reason::SmallestTooBig));
        let w = p.warning.unwrap();
        assert!(w.contains("too big"), "{w}");
        assert!(pick_default(&[], None, Some(8 * GB)).is_none());
    }

    #[test]
    fn fits_leaves_3_gb() {
        assert!(fits(46 * GB / 10, Some(76 * GB / 10)));
        assert!(!fits(47 * GB / 10, Some(76 * GB / 10)));
    }

    #[test]
    fn finds_ggufs_on_disk_skipping_projectors_and_tiny_files() {
        let d = std::env::temp_dir().join(format!("lillith-pick-{}", std::process::id()));
        let sub = d.join("AI/models/sub");
        std::fs::create_dir_all(&sub).unwrap();
        let big = |p: &Path, mb: u64| {
            let f = std::fs::File::create(p).unwrap();
            f.set_len(mb * 1024 * 1024).unwrap();
        };
        big(&d.join("AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf"), 20);
        big(&sub.join("mmproj-x.gguf"), 20);
        big(&sub.join("tiny.gguf"), 1);
        big(&d.join("AI/models/notes.txt"), 20);
        let f = find_ggufs(&[d.join("AI/models")], 2);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].path.ends_with("dolphin3.0-llama3.2-3b-q4_k_m.gguf"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
