//! What the wizard needs to know about this machine: CPU (physical cores,
//! threads, low-power laptop parts like the i5-7200U) and RAM. Pure std,
//! read from `/proc/cpuinfo` and `/proc/meminfo`; everything here can be
//! built by hand for tests, so ranking and launch lines are deterministic.

use serde::Serialize;

const GIB: u64 = 1024 * 1024 * 1024;

/// CPU facts relevant for picking a model size and llama-server `-t`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CpuInfo {
    /// `model name` from `/proc/cpuinfo` (empty when unknown).
    pub model_name: String,
    /// Hardware threads (logical CPUs).
    pub logical: u32,
    /// Distinct (physical id, core id) pairs, when `/proc/cpuinfo` lists them.
    pub physical: Option<u32>,
}

impl CpuInfo {
    /// Physical cores, else half the threads (SMT guess), else 1.
    pub fn physical_cores(&self) -> u32 {
        match self.physical {
            Some(n) if n > 0 => n,
            _ if self.logical >= 4 => self.logical / 2,
            _ => self.logical.max(1),
        }
    }

    /// Intel/AMD mobile "U" (15 W) parts such as `i5-7200U` or `Ryzen 5 3500U`.
    pub fn is_u_series(&self) -> bool {
        self.model_name
            .split(|c: char| c.is_whitespace() || "@(),".contains(c))
            .any(|w| {
                // "i5-7200U" -> "7200U"; "3500U" stays.
                let d = w.rsplit('-').next().unwrap_or(w);
                (5..=6).contains(&d.len())
                    && d.ends_with(['U', 'u'])
                    && d[..d.len() - 1].chars().all(|c| c.is_ascii_digit())
            })
    }

    /// A low-power CPU: a U-series part, or 4 threads / 4 cores or fewer.
    pub fn is_low_power(&self) -> bool {
        self.is_u_series() || (self.logical > 0 && self.logical <= 4)
    }
}

/// CPU + RAM, injectable for tests.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct HostInfo {
    pub cpu: CpuInfo,
    /// Total RAM in bytes, if known.
    pub total_ram: Option<u64>,
}

impl HostInfo {
    /// This machine.
    pub fn detect() -> Self {
        Self {
            cpu: cpu_info(),
            total_ram: crate::scan::total_ram_bytes(),
        }
    }

    /// An "8 GB" laptop reports ~7.6 GiB, so 7–17 GiB counts as 8–16 GB.
    pub fn ram_is_8_to_16g(&self) -> bool {
        self.total_ram
            .is_some_and(|r| (7 * GIB..=17 * GIB).contains(&r))
    }

    /// Low-power CPU with 8–16 GB: a 3B model is the right size (a 7B
    /// answers in tens of seconds there).
    pub fn prefers_3b(&self) -> bool {
        self.cpu.is_low_power() && self.ram_is_8_to_16g()
    }
}

/// Parses `/proc/cpuinfo` text. Physical cores are counted as distinct
/// `(physical id, core id)` pairs; `None` when those lines are missing
/// (some VMs and ARM kernels).
pub fn parse_cpuinfo(text: &str) -> CpuInfo {
    use std::collections::HashSet;
    let mut model_name = String::new();
    let mut logical = 0u32;
    let mut cores = HashSet::new();
    let mut have_ids = true;
    for block in text.split("\n\n") {
        let mut proc_seen = false;
        let (mut phys, mut core) = (None, None);
        for line in block.lines() {
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "processor" => proc_seen = true,
                "model name" if model_name.is_empty() => model_name = v.to_string(),
                "physical id" => phys = Some(v.to_string()),
                "core id" => core = Some(v.to_string()),
                _ => {}
            }
        }
        if !proc_seen {
            continue;
        }
        logical += 1;
        match (phys, core) {
            (Some(p), Some(c)) => {
                cores.insert((p, c));
            }
            (None, Some(c)) => {
                cores.insert(("0".into(), c));
            }
            _ => have_ids = false,
        }
    }
    CpuInfo {
        model_name,
        logical,
        physical: (have_ids && !cores.is_empty()).then_some(cores.len() as u32),
    }
}

/// This machine's CPU: `/proc/cpuinfo`, falling back to
/// `available_parallelism` (Windows, sandboxes).
pub fn cpu_info() -> CpuInfo {
    let mut c = std::fs::read_to_string("/proc/cpuinfo")
        .map(|t| parse_cpuinfo(&t))
        .unwrap_or_default();
    if c.logical == 0 {
        c.logical = std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(1);
    }
    c
}

/// Physical cores of this machine (llama-server `-t` default), at least 1.
pub fn physical_cores() -> u32 {
    cpu_info().physical_cores()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// i5-7200U: 2 cores, 4 threads.
    pub(crate) fn i5_7200u_cpuinfo() -> String {
        (0..4)
            .map(|i| {
                format!(
                    "processor\t: {i}\nvendor_id\t: GenuineIntel\nmodel name\t: Intel(R) Core(TM) i5-7200U CPU @ 2.50GHz\nphysical id\t: 0\nsiblings\t: 4\ncore id\t\t: {}\ncpu cores\t: 2\n",
                    i % 2
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn parses_i5_7200u() {
        let c = parse_cpuinfo(&i5_7200u_cpuinfo());
        assert_eq!(c.logical, 4);
        assert_eq!(c.physical, Some(2));
        assert_eq!(c.physical_cores(), 2);
        assert!(c.is_u_series() && c.is_low_power());
    }

    #[test]
    fn dual_socket_counts_cores_per_package() {
        let t = (0..8)
            .map(|i| {
                format!(
                    "processor\t: {i}\nmodel name\t: Intel(R) Xeon(R) CPU E5-2670 0 @ 2.60GHz\nphysical id\t: {}\ncore id\t\t: {}\n",
                    i / 4,
                    i % 4
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let c = parse_cpuinfo(&t);
        assert_eq!((c.logical, c.physical), (8, Some(8)));
        assert!(!c.is_u_series() && !c.is_low_power());
    }

    #[test]
    fn fallback_without_core_ids() {
        let t = "processor\t: 0\nmodel name\t: ARMv8\n\nprocessor\t: 1\n\nprocessor\t: 2\n\nprocessor\t: 3\n\nprocessor\t: 4\n\nprocessor\t: 5\n";
        let c = parse_cpuinfo(t);
        assert_eq!((c.logical, c.physical), (6, None));
        assert_eq!(c.physical_cores(), 3, "SMT guess: half the threads");
        let one = CpuInfo {
            logical: 1,
            ..Default::default()
        };
        assert_eq!(one.physical_cores(), 1);
        assert_eq!(CpuInfo::default().physical_cores(), 1);
        assert!(physical_cores() >= 1);
    }

    #[test]
    fn u_series_names() {
        let u = |n: &str| CpuInfo {
            model_name: n.into(),
            logical: 8,
            physical: Some(4),
        };
        assert!(u("Intel(R) Core(TM) i7-8550U CPU @ 1.80GHz").is_u_series());
        assert!(u("AMD Ryzen 5 3500U with Radeon Vega Mobile Gfx").is_u_series());
        assert!(!u("Intel(R) Core(TM) i7-1165G7 @ 2.80GHz").is_u_series());
        assert!(!u("AMD Ryzen 7 5800X 8-Core Processor").is_u_series());
        assert!(!u("Intel(R) Core(TM) i7-8700K CPU @ 3.70GHz").is_low_power());
    }

    #[test]
    fn prefers_3b_needs_low_power_and_8_to_16g() {
        let cpu = parse_cpuinfo(&i5_7200u_cpuinfo());
        let h = |gib: u64| HostInfo {
            cpu: cpu.clone(),
            total_ram: Some(gib * GIB - 400 * 1024 * 1024),
        };
        assert!(h(8).prefers_3b(), "8 GB laptop reports ~7.6 GiB");
        assert!(h(16).prefers_3b());
        assert!(!h(32).prefers_3b());
        assert!(!h(4).prefers_3b());
        let desktop = HostInfo {
            cpu: CpuInfo {
                model_name: "AMD Ryzen 7 5800X".into(),
                logical: 16,
                physical: Some(8),
            },
            total_ram: Some(16 * GIB),
        };
        assert!(!desktop.prefers_3b());
    }
}
