//! The M4 polling monitor: one [`Sample`] of procfs/sysfs state per tick,
//! turned into rule events by the pure [`Monitor`] (thresholds, hysteresis,
//! debounce, cooldowns), so the whole decision layer is tested on a fake
//! clock without touching the machine.
//!
//! | Event | Condition (defaults from `[monitor]`) | Source |
//! |---|---|---|
//! | `cpu.high` | CPU ≥ 72 % for 10 s; re-arms after < 62 % for 10 s | `/proc/stat` deltas |
//! | `ram.high` | used ≥ 85 % for 10 s; re-arms < 80 % | `/proc/meminfo` (MemTotal − MemAvailable) |
//! | `battery.low` | ≤ 15 % while discharging; re-arms ≥ 20 % or on AC | `/sys/class/power_supply` |
//! | `net.lost` / `net.restored` | default route with carrier gone / back, held 4 s | `/proc/net/route`, `ipv6_route`, `/sys/class/net/*/operstate` |
//! | `vpn.up` | a tun/WireGuard/PPP interface comes up | `/sys/class/net` |
//! | `brightness.changed` | backlight moved ≥ 2 points | `/sys/class/backlight` |
//! | `usb.added` / `usb.removed` | removable block device with media appears / goes | `/sys/block` |
//!
//! Edge events (net, vpn, usb, brightness) are baselined silently on the
//! first sample; level events (cpu, ram, battery) can fire at startup.

use crate::detect::{Cooldowns, Debounce, Edge, Hysteresis};
use crate::procfs::{cpu_percent, CpuTimes};
use crate::sysfs::{Disk, IfaceKind, PowerState, Sys};
use lillith_core::config::MonitorConfig;
use lillith_core::Event;
use std::collections::BTreeSet;

/// One snapshot of the machine.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sample {
    pub cpu: Option<CpuTimes>,
    /// Used RAM percent.
    pub ram_percent: Option<f32>,
    pub power: PowerState,
    pub backlight_percent: Option<f32>,
    /// `None` = unknown (no `/proc/net/route`).
    pub online: Option<bool>,
    /// Names of tunnel interfaces that are up.
    pub tunnels: Vec<String>,
    pub disks: Vec<Disk>,
    pub load: Option<(f32, f32, f32)>,
}

impl Sample {
    /// Reads everything from `sys` (≈ a dozen small sysfs/procfs reads).
    pub fn read(sys: &Sys, cfg: &MonitorConfig) -> Self {
        let ifaces = sys.ifaces();
        Self {
            cpu: sys.cpu(),
            ram_percent: sys.mem().map(|m| m.used_percent()),
            power: sys.power(),
            backlight_percent: sys.backlight_percent(),
            online: sys.online(&ifaces),
            tunnels: if cfg.vpn {
                ifaces
                    .iter()
                    .filter(|i| i.kind == IfaceKind::Tunnel && i.is_up())
                    .map(|i| i.name.clone())
                    .collect()
            } else {
                Vec::new()
            },
            disks: if cfg.usb {
                sys.removable_disks()
            } else {
                Vec::new()
            },
            load: sys.loadavg(),
        }
    }
}

/// Pure decision layer: samples in, events out.
#[derive(Debug, Clone)]
pub struct Monitor {
    cfg: MonitorConfig,
    prev_cpu: Option<CpuTimes>,
    /// Last computed CPU percent (for `lillith status`).
    pub cpu_percent: Option<f32>,
    cpu: Hysteresis,
    ram: Hysteresis,
    battery_armed: bool,
    net: Debounce,
    backlight: Option<f32>,
    tunnels: Option<BTreeSet<String>>,
    disks: Option<BTreeSet<Disk>>,
    cooldowns: Cooldowns,
}

impl Monitor {
    pub fn new(cfg: MonitorConfig) -> Self {
        Self {
            cpu: Hysteresis::new(
                cfg.cpu_high_percent,
                cfg.cpu_rearm_percent,
                cfg.cpu_high_sustain_s,
                cfg.cpu_rearm_sustain_s,
            ),
            ram: Hysteresis::new(
                cfg.ram_high_percent,
                cfg.ram_rearm_percent,
                cfg.ram_high_sustain_s,
                cfg.ram_high_sustain_s,
            ),
            battery_armed: true,
            net: Debounce::new(cfg.net_debounce_s),
            prev_cpu: None,
            cpu_percent: None,
            backlight: None,
            tunnels: None,
            disks: None,
            cooldowns: Cooldowns::default(),
            cfg,
        }
    }

    pub fn config(&self) -> &MonitorConfig {
        &self.cfg
    }

    /// Processes the sample taken at `now` (s, monotonic).
    pub fn step(&mut self, now: f64, s: &Sample) -> Vec<Event> {
        let mut out = Vec::new();

        // CPU (needs two samples for a delta).
        if let Some(cur) = s.cpu {
            if let Some(prev) = self.prev_cpu {
                if let Some(p) = cpu_percent(prev, cur) {
                    self.cpu_percent = Some(p);
                    if self.cpu.update(now, p) == Some(Edge::Rise) {
                        out.push(Event::with_detail("cpu.high", format!("{p:.0}%")));
                    }
                }
            }
            self.prev_cpu = Some(cur);
        }

        if let Some(p) = s.ram_percent {
            if self.ram.update(now, p) == Some(Edge::Rise) {
                out.push(Event::with_detail("ram.high", format!("{p:.0}%")));
            }
        }

        // Battery: level-triggered while discharging, re-armed by charge or AC.
        if let Some(pct) = s.power.percent() {
            if s.power.charging() || pct >= self.cfg.battery_rearm_percent {
                self.battery_armed = true;
            }
            if self.battery_armed && s.power.discharging() && pct <= self.cfg.battery_low_percent {
                self.battery_armed = false;
                out.push(Event::with_detail("battery.low", format!("{pct:.0}%")));
            }
        }

        if let Some(online) = s.online {
            match self.net.update(now, online) {
                Some(true) => out.push(Event::new("net.restored")),
                Some(false) => out.push(Event::new("net.lost")),
                None => {}
            }
        }

        if let Some(b) = s.backlight_percent {
            match self.backlight {
                Some(prev) if (b - prev).abs() >= self.cfg.brightness_min_delta_percent => {
                    self.backlight = Some(b);
                    out.push(Event::with_detail("brightness.changed", format!("{b:.0}%")));
                }
                Some(_) => {}
                None => self.backlight = Some(b),
            }
        }

        let tunnels: BTreeSet<String> = s.tunnels.iter().cloned().collect();
        if let Some(prev) = &self.tunnels {
            for t in tunnels.difference(prev) {
                out.push(Event::with_detail("vpn.up", t.clone()));
            }
        }
        self.tunnels = Some(tunnels);

        let disks: BTreeSet<Disk> = s.disks.iter().cloned().collect();
        if let Some(prev) = &self.disks {
            for d in disks.difference(prev) {
                out.push(Event::with_detail("usb.added", d.describe()));
            }
            for d in prev.difference(&disks) {
                out.push(Event::with_detail("usb.removed", d.describe()));
            }
        }
        self.disks = Some(disks);

        let cd = self.cfg.event_cooldown_s.max(0.0) as f64;
        out.retain(|e| self.cooldowns.ready(&e.name, now, cd));
        out
    }

    /// One-line human summary of the last sample (for `lillith status`).
    pub fn summary(&self, s: &Sample) -> String {
        let mut parts = Vec::new();
        match self.cpu_percent {
            Some(p) => parts.push(format!("cpu {p:.0}%")),
            None => parts.push("cpu ?".into()),
        }
        if let Some(r) = s.ram_percent {
            parts.push(format!("ram {r:.0}%"));
        }
        if let Some((l1, l5, l15)) = s.load {
            parts.push(format!("load {l1:.2} {l5:.2} {l15:.2}"));
        }
        match s.power.percent() {
            Some(p) => parts.push(format!(
                "battery {p:.0}% {}",
                if s.power.charging() {
                    "charging"
                } else if s.power.discharging() {
                    "discharging"
                } else {
                    "idle"
                }
            )),
            None => parts.push("battery none".into()),
        }
        if let Some(b) = s.backlight_percent {
            parts.push(format!("backlight {b:.0}%"));
        }
        parts.push(match s.online {
            Some(true) => "net up".into(),
            Some(false) => "net down".into(),
            None => "net ?".into(),
        });
        if !s.tunnels.is_empty() {
            parts.push(format!("vpn {}", s.tunnels.join(",")));
        }
        if !s.disks.is_empty() {
            let d: Vec<String> = s.disks.iter().map(Disk::describe).collect();
            parts.push(format!("removable {}", d.join(", ")));
        }
        parts.join(" | ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysfs::tests::laptop;
    use crate::sysfs::Battery;

    fn names(ev: &[Event]) -> Vec<&str> {
        ev.iter().map(|e| e.name.as_str()).collect()
    }

    fn cpu(busy: u64, idle: u64) -> Option<CpuTimes> {
        Some(CpuTimes {
            total: busy + idle,
            idle,
        })
    }

    fn quiet() -> MonitorConfig {
        MonitorConfig {
            event_cooldown_s: 0.0,
            ..MonitorConfig::default()
        }
    }

    #[test]
    fn cpu_high_sustained_then_rearmed() {
        let mut m = Monitor::new(quiet());
        let (mut busy, mut idle) = (0u64, 0u64);
        let mut fired = Vec::new();
        // 2 s polls: 90 % busy for 14 s, 20 % for 14 s, 90 % again for 14 s.
        for (i, pct) in [90u64; 8]
            .iter()
            .chain([20u64; 8].iter())
            .chain([90u64; 8].iter())
            .enumerate()
        {
            busy += pct;
            idle += 100 - pct;
            let ev = m.step(
                i as f64 * 2.0,
                &Sample {
                    cpu: cpu(busy, idle),
                    ..Sample::default()
                },
            );
            for e in ev {
                fired.push((i, e));
            }
        }
        assert_eq!(fired.len(), 2, "{fired:?}");
        assert_eq!(fired[0].1, Event::with_detail("cpu.high", "90%"));
        // First delta at i=1 (t=2), sustained 10 s → i=6.
        assert_eq!(fired[0].0, 6);
        assert_eq!(fired[1].0, 21);
        assert_eq!(m.cpu_percent, Some(90.0));
    }

    #[test]
    fn ram_high() {
        let mut m = Monitor::new(quiet());
        let s = |p| Sample {
            ram_percent: Some(p),
            ..Sample::default()
        };
        assert!(m.step(0.0, &s(90.0)).is_empty());
        assert_eq!(names(&m.step(10.0, &s(91.0))), ["ram.high"]);
        assert!(m.step(20.0, &s(84.0)).is_empty(), "hysteresis band");
        assert!(m.step(40.0, &s(95.0)).is_empty(), "not re-armed");
    }

    fn battery(pct: f32, status: &str, ac: bool) -> Sample {
        Sample {
            power: PowerState {
                batteries: vec![Battery {
                    name: "BAT0".into(),
                    capacity: Some(pct),
                    status: status.into(),
                    energy_now: None,
                    energy_full: None,
                }],
                ac_online: Some(ac),
            },
            ..Sample::default()
        }
    }

    #[test]
    fn battery_low_once_per_discharge() {
        let mut m = Monitor::new(quiet());
        assert!(m.step(0.0, &battery(40.0, "Discharging", false)).is_empty());
        assert_eq!(
            m.step(1.0, &battery(15.0, "Discharging", false)),
            [Event::with_detail("battery.low", "15%")]
        );
        assert!(m.step(2.0, &battery(12.0, "Discharging", false)).is_empty());
        // Plugged in → re-armed, but no event while charging.
        assert!(m.step(3.0, &battery(12.0, "Charging", true)).is_empty());
        assert_eq!(
            names(&m.step(4.0, &battery(12.0, "Discharging", false))),
            ["battery.low"]
        );
        // Desktop: nothing.
        assert!(m.step(5.0, &Sample::default()).is_empty());
    }

    #[test]
    fn net_debounced_edges() {
        let mut m = Monitor::new(quiet());
        let s = |on| Sample {
            online: Some(on),
            ..Sample::default()
        };
        assert!(
            m.step(0.0, &s(false)).is_empty(),
            "baseline offline: silent"
        );
        assert!(m.step(2.0, &s(true)).is_empty());
        assert_eq!(names(&m.step(6.0, &s(true))), ["net.restored"]);
        assert!(m.step(8.0, &s(false)).is_empty());
        assert!(m.step(10.0, &s(true)).is_empty(), "blip ignored");
        assert!(m.step(12.0, &s(false)).is_empty());
        assert_eq!(names(&m.step(16.0, &s(false))), ["net.lost"]);
        assert!(
            m.step(18.0, &Sample::default()).is_empty(),
            "unknown ignored"
        );
    }

    #[test]
    fn brightness_vpn_usb_edges() {
        let mut m = Monitor::new(quiet());
        let disk = Disk {
            name: "sda".into(),
            label: "SanDisk Ultra".into(),
        };
        let mut s = Sample {
            backlight_percent: Some(50.0),
            tunnels: vec![],
            disks: vec![],
            ..Sample::default()
        };
        assert!(m.step(0.0, &s).is_empty(), "baseline");
        s.backlight_percent = Some(51.0);
        assert!(m.step(1.0, &s).is_empty(), "below min delta");
        s.backlight_percent = Some(70.0);
        s.tunnels = vec!["wg0".into()];
        s.disks = vec![disk.clone()];
        let ev = m.step(2.0, &s);
        assert_eq!(
            ev,
            [
                Event::with_detail("brightness.changed", "70%"),
                Event::with_detail("vpn.up", "wg0"),
                Event::with_detail("usb.added", "SanDisk Ultra (sda)"),
            ]
        );
        assert!(m.step(3.0, &s).is_empty(), "steady state");
        s.disks.clear();
        s.tunnels.clear();
        assert_eq!(names(&m.step(4.0, &s)), ["usb.removed"]);
    }

    #[test]
    fn monitor_cooldown_drops_repeats() {
        let mut m = Monitor::new(MonitorConfig {
            event_cooldown_s: 20.0,
            brightness_min_delta_percent: 1.0,
            ..MonitorConfig::default()
        });
        let s = |b| Sample {
            backlight_percent: Some(b),
            ..Sample::default()
        };
        m.step(0.0, &s(10.0));
        assert_eq!(names(&m.step(1.0, &s(20.0))), ["brightness.changed"]);
        assert!(m.step(2.0, &s(30.0)).is_empty(), "cooldown");
        assert_eq!(names(&m.step(25.0, &s(40.0))), ["brightness.changed"]);
    }

    #[test]
    fn every_emitted_event_has_a_rule() {
        let rules = lillith_core::rules::RuleSet::bundled();
        for ev in [
            "cpu.high",
            "ram.high",
            "battery.low",
            "net.lost",
            "net.restored",
            "vpn.up",
            "brightness.changed",
            "usb.added",
            "usb.removed",
        ] {
            assert!(rules.find(ev).is_some(), "no rule for {ev}");
        }
    }

    #[test]
    fn sample_from_fixture_tree() {
        let f = laptop("sample");
        let cfg = MonitorConfig::default();
        let sys = f.sys();
        let s = Sample::read(&sys, &cfg);
        assert_eq!(s.power.percent(), Some(14.0));
        assert_eq!(s.online, Some(true));
        assert_eq!(s.backlight_percent, Some(50.0));
        assert_eq!(s.ram_percent, Some(50.0));
        let mut m = Monitor::new(cfg);
        // Battery at 14 % discharging fires right away.
        assert_eq!(names(&m.step(0.0, &s)), ["battery.low"]);
        f.write("proc/stat", "cpu  200 0 200 1000 0 0 0 0 0 0\n");
        let s2 = Sample::read(&sys, m.config());
        m.step(2.0, &s2);
        // Busy 200 → 400, idle 800 → 1000: 200 of 400 jiffies busy = 50 %.
        assert_eq!(m.cpu_percent, Some(50.0));
        let line = m.summary(&s2);
        assert!(line.contains("battery 14% discharging"), "{line}");
        assert!(line.contains("net up"), "{line}");
    }
}
