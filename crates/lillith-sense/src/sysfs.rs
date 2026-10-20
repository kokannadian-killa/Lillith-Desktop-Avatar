//! sysfs / procfs readers. Every path is resolved against a root directory
//! (`/` on a real machine) so tests run against fixture trees instead of
//! the live system.

use crate::procfs::{self, CpuTimes, MemInfo};
use std::fs;
use std::path::{Path, PathBuf};

/// One `/sys/class/power_supply/*` battery.
#[derive(Debug, Clone, PartialEq)]
pub struct Battery {
    pub name: String,
    /// 0..=100.
    pub capacity: Option<f32>,
    /// `Charging`, `Discharging`, `Full`, `Not charging`, `Unknown`.
    pub status: String,
    /// µWh (or µAh, whatever the driver reports, consistently per supply).
    pub energy_now: Option<f64>,
    pub energy_full: Option<f64>,
}

/// Batteries + mains adapter state. Empty on desktops.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PowerState {
    pub batteries: Vec<Battery>,
    /// `None` = no `Mains` supply found.
    pub ac_online: Option<bool>,
}

impl PowerState {
    /// Combined charge in percent (energy-weighted when every battery reports
    /// energy, otherwise the mean capacity). `None` without a battery.
    pub fn percent(&self) -> Option<f32> {
        if self.batteries.is_empty() {
            return None;
        }
        let energy: Option<(f64, f64)> = self.batteries.iter().try_fold((0.0, 0.0), |acc, b| {
            Some((acc.0 + b.energy_now?, acc.1 + b.energy_full?))
        });
        if let Some((now, full)) = energy {
            if full > 0.0 {
                return Some((now / full * 100.0).clamp(0.0, 100.0) as f32);
            }
        }
        let caps: Vec<f32> = self.batteries.iter().filter_map(|b| b.capacity).collect();
        if caps.is_empty() {
            None
        } else {
            Some(caps.iter().sum::<f32>() / caps.len() as f32)
        }
    }

    /// On AC or any battery charging.
    pub fn charging(&self) -> bool {
        self.ac_online == Some(true) || self.batteries.iter().any(|b| b.status == "Charging")
    }

    /// Running on battery: some battery discharging and AC not online.
    pub fn discharging(&self) -> bool {
        self.ac_online != Some(true) && self.batteries.iter().any(|b| b.status == "Discharging")
    }
}

/// Kind of network interface, from sysfs attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IfaceKind {
    Loopback,
    /// Backed by hardware (`device` link): Ethernet, Wi-Fi, WWAN, USB tethering.
    Physical,
    /// tun/tap, WireGuard, PPP and VPN client interfaces.
    Tunnel,
    /// Bridges, veth, docker, virbr, dummy …
    Virtual,
}

/// One `/sys/class/net/*` interface.
#[derive(Debug, Clone, PartialEq)]
pub struct Iface {
    pub name: String,
    /// `up`, `down`, `unknown`, `dormant`, `lowerlayerdown`, `notpresent`.
    pub operstate: String,
    pub kind: IfaceKind,
}

impl Iface {
    /// Carrier is present. Tunnels report `unknown` while working.
    pub fn is_up(&self) -> bool {
        matches!(self.operstate.as_str(), "up" | "unknown")
    }
}

/// A removable block device (USB stick, SD card with media).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Disk {
    pub name: String,
    /// `device/vendor` + `device/model`, trimmed; empty if unknown.
    pub label: String,
}

impl Disk {
    /// Human-readable detail for the event.
    pub fn describe(&self) -> String {
        if self.label.is_empty() {
            self.name.clone()
        } else {
            format!("{} ({})", self.label, self.name)
        }
    }
}

/// Reads the machine below `root`.
#[derive(Debug, Clone)]
pub struct Sys {
    root: PathBuf,
}

impl Default for Sys {
    fn default() -> Self {
        Self::new("/")
    }
}

impl Sys {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel.trim_start_matches('/'))
    }

    fn read(&self, rel: &str) -> Option<String> {
        fs::read_to_string(self.path(rel)).ok()
    }

    pub fn cpu(&self) -> Option<CpuTimes> {
        procfs::parse_stat(&self.read("proc/stat")?)
    }

    pub fn mem(&self) -> Option<MemInfo> {
        procfs::parse_meminfo(&self.read("proc/meminfo")?)
    }

    pub fn loadavg(&self) -> Option<(f32, f32, f32)> {
        procfs::parse_loadavg(&self.read("proc/loadavg")?)
    }

    /// Batteries and AC from `/sys/class/power_supply`. Peripheral batteries
    /// (`scope` = `Device`: mice, headsets) are ignored.
    pub fn power(&self) -> PowerState {
        let mut st = PowerState::default();
        for dir in sorted_dirs(&self.path("sys/class/power_supply")) {
            let typ = read_trim(&dir.join("type")).unwrap_or_default();
            match typ.as_str() {
                "Battery" => {
                    if read_trim(&dir.join("scope")).as_deref() == Some("Device") {
                        continue;
                    }
                    if read_trim(&dir.join("present")).as_deref() == Some("0") {
                        continue;
                    }
                    let num = |f: &str| read_trim(&dir.join(f)).and_then(|s| s.parse::<f64>().ok());
                    st.batteries.push(Battery {
                        name: file_name(&dir),
                        capacity: num("capacity").map(|v| v as f32),
                        status: read_trim(&dir.join("status")).unwrap_or_else(|| "Unknown".into()),
                        energy_now: num("energy_now").or_else(|| num("charge_now")),
                        energy_full: num("energy_full").or_else(|| num("charge_full")),
                    });
                }
                "Mains" | "USB" => {
                    // USB-C PD adapters show up as type USB on some laptops.
                    if let Some(on) = read_trim(&dir.join("online")) {
                        let on = on == "1";
                        if typ == "Mains" || on {
                            st.ac_online = Some(st.ac_online.unwrap_or(false) || on);
                        }
                    }
                }
                _ => {}
            }
        }
        st
    }

    /// Backlight brightness in percent (first device by name; `intel_backlight`
    /// on the Spectre). `None` without a backlight (desktops, most VMs).
    pub fn backlight_percent(&self) -> Option<f32> {
        sorted_dirs(&self.path("sys/class/backlight"))
            .into_iter()
            .find_map(|dir| {
                let max: f32 = read_trim(&dir.join("max_brightness"))?.parse().ok()?;
                let cur: f32 = read_trim(&dir.join("actual_brightness"))
                    .or_else(|| read_trim(&dir.join("brightness")))?
                    .parse()
                    .ok()?;
                (max > 0.0).then(|| (cur / max * 100.0).clamp(0.0, 100.0))
            })
    }

    /// All network interfaces.
    pub fn ifaces(&self) -> Vec<Iface> {
        sorted_dirs(&self.path("sys/class/net"))
            .into_iter()
            .map(|dir| {
                let name = file_name(&dir);
                let operstate =
                    read_trim(&dir.join("operstate")).unwrap_or_else(|| "unknown".into());
                let kind = classify_iface(&name, &dir);
                Iface {
                    name,
                    operstate,
                    kind,
                }
            })
            .collect()
    }

    /// `Some(true)` if a default route (v4 or v6) goes through an interface
    /// with carrier; `None` if `/proc/net/route` is unreadable.
    pub fn online(&self, ifaces: &[Iface]) -> Option<bool> {
        let v4 = self.read("proc/net/route")?;
        let mut routed = procfs::default_route_ifaces(&v4);
        if let Some(v6) = self.read("proc/net/ipv6_route") {
            routed.extend(procfs::default_route6_ifaces(&v6));
        }
        Some(routed.iter().any(|r| {
            // An interface missing from sysfs (namespaces) is trusted.
            ifaces
                .iter()
                .find(|i| &i.name == r)
                .is_none_or(|i| i.is_up() && i.kind != IfaceKind::Loopback)
        }))
    }

    /// Removable block devices with media: `removable` = 1, or the device
    /// sits on a USB bus (UAS SSDs report removable = 0), and size > 0.
    pub fn removable_disks(&self) -> Vec<Disk> {
        let mut out = Vec::new();
        let block = self.path("sys/block");
        for dir in sorted_dirs(&block) {
            let name = file_name(&dir);
            if ["loop", "ram", "zram", "dm-", "md", "sr", "nbd"]
                .iter()
                .any(|p| name.starts_with(p))
            {
                continue;
            }
            let size: u64 = read_trim(&dir.join("size"))
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            if size == 0 {
                continue;
            }
            let removable = read_trim(&dir.join("removable")).as_deref() == Some("1");
            let on_usb = fs::read_link(&dir)
                .map(|t| t.to_string_lossy().contains("/usb"))
                .unwrap_or(false);
            if !(removable || on_usb) {
                continue;
            }
            let vendor = read_trim(&dir.join("device/vendor")).unwrap_or_default();
            let model = read_trim(&dir.join("device/model")).unwrap_or_default();
            let label = format!("{vendor} {model}").trim().to_string();
            out.push(Disk { name, label });
        }
        out
    }
}

/// Interface classification from its name and sysfs attributes.
pub fn classify_iface(name: &str, dir: &Path) -> IfaceKind {
    // ARPHRD_* from <linux/if_arp.h>.
    const LOOPBACK: &str = "772";
    const PPP: &str = "512";
    const NONE: &str = "65534"; // tun (L3) and WireGuard
    let typ = read_trim(&dir.join("type")).unwrap_or_default();
    if name == "lo" || typ == LOOPBACK {
        return IfaceKind::Loopback;
    }
    const TUNNEL_PREFIXES: [&str; 10] = [
        "tun",
        "tap",
        "wg",
        "ppp",
        "tailscale",
        "nordlynx",
        "proton",
        "ipsec",
        "vpn",
        "mullvad",
    ];
    if TUNNEL_PREFIXES.iter().any(|p| name.starts_with(p))
        || dir.join("tun_flags").exists()
        || typ == PPP
        || typ == NONE
    {
        return IfaceKind::Tunnel;
    }
    if dir.join("device").exists() {
        IfaceKind::Physical
    } else {
        IfaceKind::Virtual
    }
}

fn read_trim(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Entries of `dir` (following symlinks, as sysfs class dirs are links), sorted.
fn sorted_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    v.sort();
    v
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A throwaway fixture root under the system temp dir.
    pub(crate) struct Fixture {
        pub root: PathBuf,
    }

    impl Fixture {
        pub(crate) fn new(tag: &str) -> Self {
            static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let root = std::env::temp_dir()
                .join(format!("lillith-sense-{tag}-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        pub(crate) fn write(&self, rel: &str, content: &str) -> &Self {
            let p = self.root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, content).unwrap();
            self
        }

        pub(crate) fn mkdir(&self, rel: &str) -> &Self {
            fs::create_dir_all(self.root.join(rel)).unwrap();
            self
        }

        pub(crate) fn sys(&self) -> Sys {
            Sys::new(&self.root)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    /// HP Spectre x360-like laptop: BAT0 + AC, intel_backlight, wlan0 online.
    pub(crate) fn laptop(tag: &str) -> Fixture {
        let f = Fixture::new(tag);
        f.write("sys/class/power_supply/BAT0/type", "Battery\n")
            .write("sys/class/power_supply/BAT0/capacity", "14\n")
            .write("sys/class/power_supply/BAT0/status", "Discharging\n")
            .write("sys/class/power_supply/AC/type", "Mains\n")
            .write("sys/class/power_supply/AC/online", "0\n")
            // Bluetooth mouse battery: ignored.
            .write("sys/class/power_supply/hidpp_battery_0/type", "Battery\n")
            .write("sys/class/power_supply/hidpp_battery_0/scope", "Device\n")
            .write("sys/class/power_supply/hidpp_battery_0/capacity", "90\n")
            .write("sys/class/backlight/intel_backlight/max_brightness", "1500\n")
            .write("sys/class/backlight/intel_backlight/actual_brightness", "750\n")
            .write("sys/class/net/lo/type", "772\n")
            .write("sys/class/net/lo/operstate", "unknown\n")
            .write("sys/class/net/wlan0/type", "1\n")
            .write("sys/class/net/wlan0/operstate", "up\n")
            .mkdir("sys/class/net/wlan0/device")
            .write("sys/class/net/docker0/type", "1\n")
            .write("sys/class/net/docker0/operstate", "down\n")
            .write(
                "proc/net/route",
                "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                 wlan0\t00000000\t0100A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n",
            )
            .write("proc/stat", "cpu  100 0 100 800 0 0 0 0 0 0\n")
            .write("proc/meminfo", "MemTotal: 1000 kB\nMemAvailable: 500 kB\n")
            .write("proc/loadavg", "0.50 0.40 0.30 1/200 999\n");
        f
    }

    #[test]
    fn power_supply() {
        let f = laptop("power");
        let p = f.sys().power();
        assert_eq!(p.batteries.len(), 1, "peripheral battery ignored");
        assert_eq!(p.percent(), Some(14.0));
        assert_eq!(p.ac_online, Some(false));
        assert!(p.discharging());
        assert!(!p.charging());
        f.write("sys/class/power_supply/AC/online", "1\n")
            .write("sys/class/power_supply/BAT0/status", "Charging\n");
        let p = f.sys().power();
        assert!(p.charging() && !p.discharging());
    }

    #[test]
    fn energy_weighted_two_batteries() {
        let f = Fixture::new("bat2");
        for (n, now, full) in [
            ("BAT0", "10000000", "40000000"),
            ("BAT1", "30000000", "40000000"),
        ] {
            f.write(&format!("sys/class/power_supply/{n}/type"), "Battery\n")
                .write(&format!("sys/class/power_supply/{n}/capacity"), "99\n")
                .write(&format!("sys/class/power_supply/{n}/energy_now"), now)
                .write(&format!("sys/class/power_supply/{n}/energy_full"), full)
                .write(&format!("sys/class/power_supply/{n}/status"), "Discharging");
        }
        assert_eq!(f.sys().power().percent(), Some(50.0));
    }

    #[test]
    fn desktop_has_no_battery() {
        let f = Fixture::new("desktop");
        let p = f.sys().power();
        assert_eq!(p, PowerState::default());
        assert_eq!(p.percent(), None);
        assert!(!p.discharging());
        assert_eq!(f.sys().backlight_percent(), None);
        assert_eq!(f.sys().online(&[]), None, "no /proc/net/route");
        assert_eq!(f.sys().cpu(), None);
    }

    #[test]
    fn backlight_and_procfs() {
        let f = laptop("bl");
        assert_eq!(f.sys().backlight_percent(), Some(50.0));
        assert_eq!(f.sys().mem().unwrap().used_percent(), 50.0);
        assert_eq!(f.sys().cpu().unwrap().total, 1000);
        assert_eq!(f.sys().loadavg(), Some((0.5, 0.4, 0.3)));
    }

    #[test]
    fn network_state() {
        let f = laptop("net");
        let ifs = f.sys().ifaces();
        let kind = |n: &str| ifs.iter().find(|i| i.name == n).unwrap().kind;
        assert_eq!(kind("lo"), IfaceKind::Loopback);
        assert_eq!(kind("wlan0"), IfaceKind::Physical);
        assert_eq!(kind("docker0"), IfaceKind::Virtual);
        assert_eq!(f.sys().online(&ifs), Some(true));
        // Wi-Fi drops: carrier gone (route lingers briefly) → offline.
        f.write("sys/class/net/wlan0/operstate", "down\n");
        let ifs = f.sys().ifaces();
        assert_eq!(f.sys().online(&ifs), Some(false));
        // Route removed as well.
        f.write("proc/net/route", "Iface\tDestination\n");
        assert_eq!(f.sys().online(&ifs), Some(false));
    }

    #[test]
    fn tunnels() {
        let f = laptop("tun");
        f.write("sys/class/net/wg0/type", "65534\n")
            .write("sys/class/net/wg0/operstate", "unknown\n")
            .write("sys/class/net/corpvpn/type", "1\n")
            .write("sys/class/net/corpvpn/tun_flags", "0x1002\n")
            .write("sys/class/net/corpvpn/operstate", "unknown\n");
        let ifs = f.sys().ifaces();
        let tunnels: Vec<&str> = ifs
            .iter()
            .filter(|i| i.kind == IfaceKind::Tunnel && i.is_up())
            .map(|i| i.name.as_str())
            .collect();
        assert_eq!(tunnels, ["corpvpn", "wg0"]);
    }

    #[test]
    fn removable_block_devices() {
        let f = Fixture::new("usb");
        f.write("sys/block/nvme0n1/size", "1000215216\n")
            .write("sys/block/nvme0n1/removable", "0\n")
            .write("sys/block/sda/size", "60063744\n")
            .write("sys/block/sda/removable", "1\n")
            .write("sys/block/sda/device/vendor", "SanDisk \n")
            .write("sys/block/sda/device/model", "Ultra Fit       \n")
            // Empty card reader: removable but no media.
            .write("sys/block/sdb/size", "0\n")
            .write("sys/block/sdb/removable", "1\n")
            .write("sys/block/loop0/size", "100\n")
            .write("sys/block/loop0/removable", "1\n");
        // UAS SSD: removable = 0 but the link goes through the USB bus.
        f.write(
            "sys/devices/pci0000:00/0000:00:14.0/usb2/2-1/host0/block/sdc/size",
            "500000000\n",
        )
        .write(
            "sys/devices/pci0000:00/0000:00:14.0/usb2/2-1/host0/block/sdc/removable",
            "0\n",
        );
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            f.root
                .join("sys/devices/pci0000:00/0000:00:14.0/usb2/2-1/host0/block/sdc"),
            f.root.join("sys/block/sdc"),
        )
        .unwrap();
        let disks = f.sys().removable_disks();
        let names: Vec<&str> = disks.iter().map(|d| d.name.as_str()).collect();
        #[cfg(unix)]
        assert_eq!(names, ["sda", "sdc"]);
        assert_eq!(disks[0].describe(), "SanDisk Ultra Fit (sda)");
    }
}
