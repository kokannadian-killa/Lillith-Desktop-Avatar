//! Pure parsers for the `/proc` files the monitor reads (no I/O here, so
//! every parser is unit-tested against captured text).
//!
//! | File | Parser | Used for |
//! |---|---|---|
//! | `/proc/stat` | [`parse_stat`] → [`CpuTimes`], [`cpu_percent`] | `cpu.high` |
//! | `/proc/meminfo` | [`parse_meminfo`] → [`MemInfo`] | `ram.high` |
//! | `/proc/loadavg` | [`parse_loadavg`] | `lillith status` |
//! | `/proc/net/route`, `/proc/net/ipv6_route` | [`default_route_ifaces`], [`default_route6_ifaces`] | `net.lost` / `net.restored` |

/// Aggregate CPU jiffies from the first `cpu ` line of `/proc/stat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuTimes {
    /// user + nice + system + idle + iowait + irq + softirq + steal.
    pub total: u64,
    /// idle + iowait.
    pub idle: u64,
}

/// Parses the aggregate `cpu` line of `/proc/stat`.
pub fn parse_stat(text: &str) -> Option<CpuTimes> {
    let line = text.lines().find(|l| l.starts_with("cpu "))?;
    let f: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .take(8)
        .map(|v| v.parse().ok())
        .collect::<Option<_>>()?;
    if f.len() < 4 {
        return None;
    }
    // guest/guest_nice are already included in user/nice.
    let total = f.iter().sum();
    let idle = f[3] + f.get(4).copied().unwrap_or(0);
    Some(CpuTimes { total, idle })
}

/// Busy percentage between two `/proc/stat` samples (`None` if no time passed
/// or the counters went backwards, e.g. after a CPU was hot-unplugged).
pub fn cpu_percent(prev: CpuTimes, cur: CpuTimes) -> Option<f32> {
    let dt = cur.total.checked_sub(prev.total)?;
    let di = cur.idle.checked_sub(prev.idle)?;
    if dt == 0 || di > dt {
        return None;
    }
    Some((dt - di) as f32 / dt as f32 * 100.0)
}

/// The two `/proc/meminfo` numbers we need, in KiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemInfo {
    pub total_kb: u64,
    pub available_kb: u64,
}

impl MemInfo {
    /// Used = total − available (what `free` calls "used" + unreclaimable cache).
    pub fn used_percent(&self) -> f32 {
        if self.total_kb == 0 {
            return 0.0;
        }
        let used = self.total_kb.saturating_sub(self.available_kb);
        used as f32 / self.total_kb as f32 * 100.0
    }
}

/// Parses `/proc/meminfo`. Falls back to free + buffers + cached on kernels
/// without `MemAvailable` (< 3.14).
pub fn parse_meminfo(text: &str) -> Option<MemInfo> {
    let mut total = None;
    let mut avail = None;
    let (mut free, mut buffers, mut cached) = (0u64, 0u64, 0u64);
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(key), Some(val)) = (it.next(), it.next()) else {
            continue;
        };
        let Ok(v) = val.parse::<u64>() else {
            continue;
        };
        match key {
            "MemTotal:" => total = Some(v),
            "MemAvailable:" => avail = Some(v),
            "MemFree:" => free = v,
            "Buffers:" => buffers = v,
            "Cached:" => cached = v,
            _ => {}
        }
    }
    let total_kb = total?;
    let available_kb = avail.unwrap_or(free + buffers + cached).min(total_kb);
    Some(MemInfo {
        total_kb,
        available_kb,
    })
}

/// `/proc/loadavg` → (1, 5, 15 min).
pub fn parse_loadavg(text: &str) -> Option<(f32, f32, f32)> {
    let mut it = text.split_whitespace().map(|v| v.parse::<f32>().ok());
    Some((it.next()??, it.next()??, it.next()??))
}

/// Interfaces carrying an IPv4 default route that is up (`/proc/net/route`).
pub fn default_route_ifaces(text: &str) -> Vec<String> {
    const RTF_UP: u32 = 0x1;
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 8 {
            continue;
        }
        let flags = u32::from_str_radix(f[3], 16).unwrap_or(0);
        let mask = f[7];
        if f[1] == "00000000" && mask == "00000000" && flags & RTF_UP != 0 && f[0] != "lo" {
            push_unique(&mut out, f[0]);
        }
    }
    out
}

/// Interfaces carrying an IPv6 default route (`/proc/net/ipv6_route`;
/// columns: dst, dst_len, src, src_len, next_hop, metric, refcnt, use, flags, iface).
pub fn default_route6_ifaces(text: &str) -> Vec<String> {
    const RTF_UP: u32 = 0x1;
    const RTF_REJECT: u32 = 0x200;
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 10 {
            continue;
        }
        let flags = u32::from_str_radix(f[8], 16).unwrap_or(0);
        let default = f[0].bytes().all(|b| b == b'0') && f[1] == "00";
        if default && flags & RTF_UP != 0 && flags & RTF_REJECT == 0 && f[9] != "lo" {
            push_unique(&mut out, f[9]);
        }
    }
    out
}

fn push_unique(v: &mut Vec<String>, s: &str) {
    if !v.iter().any(|x| x == s) {
        v.push(s.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT_A: &str = "cpu  4705 356 584 3699 23 23 0 0 0 0\n\
cpu0 1393280 32966 572056 13343292 6130 0 17875 0 0 0\n\
intr 114930548 113199788 3 0 5 263 0 4 [...]\n";
    const STAT_B: &str = "cpu  5705 356 584 3799 123 23 0 0 0 0\ncpu0 1 2 3 4\n";

    #[test]
    fn stat_and_percent() {
        let a = parse_stat(STAT_A).unwrap();
        assert_eq!(a.total, 4705 + 356 + 584 + 3699 + 23 + 23);
        assert_eq!(a.idle, 3699 + 23);
        let b = parse_stat(STAT_B).unwrap();
        // +1000 user, +100 idle, +100 iowait → 1200 total, 200 idle → 83.3 %.
        let p = cpu_percent(a, b).unwrap();
        assert!((p - 83.333).abs() < 0.01, "{p}");
        assert_eq!(cpu_percent(b, a), None, "counters went backwards");
        assert_eq!(cpu_percent(a, a), None, "no time passed");
        assert_eq!(parse_stat("intr 1 2 3\n"), None);
        assert_eq!(parse_stat("cpu  a b c d\n"), None);
    }

    const MEMINFO: &str = "MemTotal:        7972236 kB\n\
MemFree:          367528 kB\n\
MemAvailable:    1195844 kB\n\
Buffers:          123456 kB\n\
Cached:          1500000 kB\n\
SwapCached:        12000 kB\n";

    #[test]
    fn meminfo() {
        let m = parse_meminfo(MEMINFO).unwrap();
        assert_eq!(m.total_kb, 7_972_236);
        assert_eq!(m.available_kb, 1_195_844);
        assert!(
            (m.used_percent() - 85.0).abs() < 0.1,
            "{}",
            m.used_percent()
        );
        // Old kernel: no MemAvailable.
        let old = "MemTotal: 1000 kB\nMemFree: 100 kB\nBuffers: 50 kB\nCached: 250 kB\n";
        let m = parse_meminfo(old).unwrap();
        assert_eq!(m.available_kb, 400);
        assert!((m.used_percent() - 60.0).abs() < 1e-3);
        assert_eq!(parse_meminfo("garbage"), None);
    }

    #[test]
    fn loadavg() {
        assert_eq!(
            parse_loadavg("0.52 0.58 0.59 1/1022 12345\n"),
            Some((0.52, 0.58, 0.59))
        );
        assert_eq!(parse_loadavg("x"), None);
    }

    const ROUTE: &str =
        "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
wlan0\t00000000\t0100A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n\
wlan0\t0000A8C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0\n\
docker0\t000011AC\t00000000\t0001\t0\t0\t0\t0000FFFF\t0\t0\t0\n";

    #[test]
    fn ipv4_default_route() {
        assert_eq!(default_route_ifaces(ROUTE), ["wlan0"]);
        let header_only = ROUTE.lines().next().unwrap();
        assert!(default_route_ifaces(header_only).is_empty());
        // A default route that is not up does not count.
        let down = format!(
            "{header_only}\neth0\t00000000\t0100A8C0\t0002\t0\t0\t100\t00000000\t0\t0\t0\n"
        );
        assert!(default_route_ifaces(&down).is_empty());
    }

    const ROUTE6: &str = "\
fe800000000000000000000000000000 40 00000000000000000000000000000000 00 00000000000000000000000000000000 00000100 00000001 00000000 00000001   wlan0\n\
00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe80000000000000022a10fffe000001 00000064 00000003 00000000 00450003   wlan0\n\
00000000000000000000000000000000 00 00000000000000000000000000000000 00 00000000000000000000000000000000 ffffffff 00000001 00000000 00200200       lo\n";

    #[test]
    fn ipv6_default_route() {
        assert_eq!(default_route6_ifaces(ROUTE6), ["wlan0"]);
        assert!(default_route6_ifaces("").is_empty());
    }
}
