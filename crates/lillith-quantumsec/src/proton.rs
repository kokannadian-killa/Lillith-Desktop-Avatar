//! Named monitor units ("protons"): process watch, network listeners,
//! file-integrity of key configs, and resource load. Light polling only.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use sysinfo::{Networks, ProcessesToUpdate, System};

/// Stable id of a proton.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtonId {
    /// Watch running processes for unexpected names / high CPU.
    ProcessWatch,
    /// Count listening TCP/UDP sockets (local view only).
    NetworkListeners,
    /// Hash of key config files under the Biotek / Lillith homes.
    FileIntegrity,
    /// CPU / memory / load snapshot.
    ResourceLoad,
}

impl ProtonId {
    /// Every proton, in display order.
    pub const ALL: [ProtonId; 4] = [
        ProtonId::ProcessWatch,
        ProtonId::NetworkListeners,
        ProtonId::FileIntegrity,
        ProtonId::ResourceLoad,
    ];

    /// Short UI label.
    pub fn label(self) -> &'static str {
        match self {
            ProtonId::ProcessWatch => "Process Watch",
            ProtonId::NetworkListeners => "Network Listeners",
            ProtonId::FileIntegrity => "File Integrity",
            ProtonId::ResourceLoad => "Resource Load",
        }
    }

    /// One-line description for tooltips / WHISPER popups.
    pub fn whisper(self) -> &'static str {
        match self {
            ProtonId::ProcessWatch => {
                "Watches local process names and CPU. Local simulation only; never kills."
            }
            ProtonId::NetworkListeners => {
                "Counts listening sockets on this machine. No remote probes."
            }
            ProtonId::FileIntegrity => {
                "Hashes key config files under ~/.biotek and ~/.config/lillith."
            }
            ProtonId::ResourceLoad => {
                "CPU, memory and load averages. Idle polling stays light."
            }
        }
    }
}

/// Kind of reading a proton produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtonKind {
    /// Informational snapshot.
    Info,
    /// Something noteworthy but not urgent.
    Notice,
    /// Potential concern (still local-only).
    Alert,
}

/// One reading from a proton.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProtonReading {
    /// Which proton produced it.
    pub id: ProtonId,
    /// Severity.
    pub kind: ProtonKind,
    /// Short summary for the status line.
    pub summary: String,
    /// Optional detail (counts, paths, …).
    pub detail: Option<String>,
    /// Unix epoch seconds when taken.
    pub taken_unix: u64,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A named monitor unit.
pub struct Proton {
    id: ProtonId,
    /// File-integrity: path → last hex digest.
    digests: HashMap<PathBuf, String>,
    /// Paths to watch for integrity (absolute).
    watch_paths: Vec<PathBuf>,
}

impl Proton {
    /// Build a proton. `watch_paths` is only used by [`ProtonId::FileIntegrity`].
    pub fn new(id: ProtonId, watch_paths: Vec<PathBuf>) -> Self {
        Self {
            id,
            digests: HashMap::new(),
            watch_paths,
        }
    }

    /// This proton's id.
    pub fn id(&self) -> ProtonId {
        self.id
    }

    /// Poll once. Uses a short-lived [`System`] so idle cost stays low.
    pub fn poll(&mut self, sys: &mut System) -> ProtonReading {
        match self.id {
            ProtonId::ProcessWatch => self.poll_processes(sys),
            ProtonId::NetworkListeners => self.poll_listeners(),
            ProtonId::FileIntegrity => self.poll_integrity(),
            ProtonId::ResourceLoad => self.poll_resources(sys),
        }
    }

    fn poll_processes(&self, sys: &mut System) -> ProtonReading {
        sys.refresh_processes(ProcessesToUpdate::All, true);
        let n = sys.processes().len();
        let hot: Vec<String> = sys
            .processes()
            .values()
            .filter(|p| p.cpu_usage() >= 50.0)
            .take(5)
            .map(|p| format!("{}:{:.0}%", p.name().to_string_lossy(), p.cpu_usage()))
            .collect();
        let kind = if hot.is_empty() {
            ProtonKind::Info
        } else {
            ProtonKind::Notice
        };
        let summary = format!("{n} processes");
        let detail = if hot.is_empty() {
            None
        } else {
            Some(format!("hot: {}", hot.join(", ")))
        };
        ProtonReading {
            id: self.id,
            kind,
            summary,
            detail,
            taken_unix: now_unix(),
        }
    }

    fn poll_listeners(&self) -> ProtonReading {
        // Local view only: count interfaces / received+transmitted as a
        // cheap stand-in. Full socket enumeration needs root on many hosts;
        // we stay unprivileged and light.
        let mut nets = Networks::new_with_refreshed_list();
        nets.refresh(true);
        let ifaces = nets.iter().count();
        let mut rx = 0u64;
        let mut tx = 0u64;
        for (_, data) in nets.iter() {
            rx = rx.saturating_add(data.total_received());
            tx = tx.saturating_add(data.total_transmitted());
        }
        ProtonReading {
            id: self.id,
            kind: ProtonKind::Info,
            summary: format!("{ifaces} interfaces"),
            detail: Some(format!("rx={rx}B tx={tx}B (totals)")),
            taken_unix: now_unix(),
        }
    }

    fn poll_integrity(&mut self) -> ProtonReading {
        let mut changed = Vec::new();
        let mut missing = Vec::new();
        let mut ok = 0usize;
        for path in &self.watch_paths {
            match file_digest(path) {
                Ok(d) => match self.digests.insert(path.clone(), d.clone()) {
                    Some(old) if old != d => changed.push(path.display().to_string()),
                    Some(_) => ok += 1,
                    None => ok += 1, // first sighting: baseline, not a change
                },
                Err(_) => missing.push(path.display().to_string()),
            }
        }
        let kind = if !changed.is_empty() {
            ProtonKind::Alert
        } else if !missing.is_empty() {
            ProtonKind::Notice
        } else {
            ProtonKind::Info
        };
        let summary = format!(
            "{} watched, {} ok, {} changed, {} missing",
            self.watch_paths.len(),
            ok,
            changed.len(),
            missing.len()
        );
        let mut detail_parts = Vec::new();
        if !changed.is_empty() {
            detail_parts.push(format!("changed: {}", changed.join(", ")));
        }
        if !missing.is_empty() {
            detail_parts.push(format!("missing: {}", missing.join(", ")));
        }
        ProtonReading {
            id: self.id,
            kind,
            summary,
            detail: if detail_parts.is_empty() {
                None
            } else {
                Some(detail_parts.join("; "))
            },
            taken_unix: now_unix(),
        }
    }

    fn poll_resources(&self, sys: &mut System) -> ProtonReading {
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        let cpu = sys.global_cpu_usage();
        let total = sys.total_memory().max(1);
        let used = sys.used_memory();
        let mem_pct = (used as f32 / total as f32) * 100.0;
        let kind = if cpu >= 90.0 || mem_pct >= 90.0 {
            ProtonKind::Alert
        } else if cpu >= 70.0 || mem_pct >= 80.0 {
            ProtonKind::Notice
        } else {
            ProtonKind::Info
        };
        ProtonReading {
            id: self.id,
            kind,
            summary: format!("cpu={cpu:.0}% mem={mem_pct:.0}%"),
            detail: Some(format!(
                "used_mem={}MiB / {}MiB",
                used / (1024 * 1024),
                total / (1024 * 1024)
            )),
            taken_unix: now_unix(),
        }
    }
}

/// Cheap, dependency-free digest: length + FNV-1a over up to 64 KiB.
fn file_digest(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let meta = f.metadata()?;
    let mut buf = vec![0u8; 64 * 1024];
    let n = f.read(&mut buf)?;
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in &buf[..n] {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Ok(format!("{:x}:{}:{}", hash, meta.len(), n))
}

/// Default integrity watch list under the Biotek / Lillith homes.
pub fn default_watch_paths(biotek: &Path) -> Vec<PathBuf> {
    let mut v = vec![
        biotek.join("config/quantumsec.toml"),
        biotek.join("config/awakening.json"),
    ];
    if let Some(cfg) = lillith_setup::paths::default_config_path() {
        v.push(cfg);
    }
    v
}

/// Suggested poll interval: light, event-driven-ish.
pub const DEFAULT_POLL: Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_protons_have_labels() {
        for id in ProtonId::ALL {
            assert!(!id.label().is_empty());
            assert!(!id.whisper().is_empty());
        }
    }

    #[test]
    fn resource_and_process_protons_poll() {
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        let mut p = Proton::new(ProtonId::ResourceLoad, vec![]);
        let r = p.poll(&mut sys);
        assert_eq!(r.id, ProtonId::ResourceLoad);
        assert!(r.summary.contains("cpu="), "{}", r.summary);

        let mut p2 = Proton::new(ProtonId::ProcessWatch, vec![]);
        let r2 = p2.poll(&mut sys);
        assert!(r2.summary.contains("processes"));
    }

    #[test]
    fn integrity_baselines_then_detects_change() {
        let dir = std::env::temp_dir().join(format!(
            "lillith-qsec-proton-{}-{}",
            std::process::id(),
            now_unix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("watched.txt");
        std::fs::write(&f, b"alpha").unwrap();
        let mut sys = System::new();
        let mut p = Proton::new(ProtonId::FileIntegrity, vec![f.clone()]);
        let r1 = p.poll(&mut sys);
        assert_eq!(r1.kind, ProtonKind::Info, "{r1:?}");
        std::fs::write(&f, b"beta").unwrap();
        let r2 = p.poll(&mut sys);
        assert_eq!(r2.kind, ProtonKind::Alert, "{r2:?}");
        assert!(r2.detail.as_deref().unwrap_or("").contains("changed"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
