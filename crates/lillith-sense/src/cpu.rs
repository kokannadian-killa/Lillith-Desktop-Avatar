//! CPU / RAM monitor (port of `mod_system_monitor.py`).
//!
//! Polls `sysinfo` every `cpu_poll_s`, fires `cpu.high` when usage stays
//! above `cpu_high_percent` for `cpu_high_sustain_s`, with hysteresis so it
//! re-arms only after dropping 10 points below the threshold.

use crate::bus::EventBus;
use lillith_core::Event;
use std::time::Duration;
use sysinfo::System;

/// Edge detector with hysteresis + sustain time.
#[derive(Debug, Clone, PartialEq)]
pub struct Threshold {
    pub high: f32,
    pub rearm_below: f32,
    pub sustain_s: f32,
    above_for: f32,
    armed: bool,
}

impl Threshold {
    pub fn new(high: f32, sustain_s: f32) -> Self {
        Self {
            high,
            rearm_below: high - 10.0,
            sustain_s,
            above_for: 0.0,
            armed: true,
        }
    }

    /// Feeds one sample taken `dt` seconds after the previous one.
    /// Returns `true` on the rising edge.
    pub fn sample(&mut self, value: f32, dt: f32) -> bool {
        if value >= self.high {
            self.above_for += dt;
        } else {
            self.above_for = 0.0;
            if value < self.rearm_below {
                self.armed = true;
            }
        }
        if self.armed && self.above_for >= self.sustain_s {
            self.armed = false;
            return true;
        }
        false
    }
}

/// Runs the CPU/RAM loop until the task is cancelled.
pub async fn run(bus: EventBus, cpu_high: f32, ram_high: f32, sustain_s: f32, poll_s: f32) {
    let mut sys = System::new();
    let mut cpu = Threshold::new(cpu_high, sustain_s);
    let mut ram = Threshold::new(ram_high, sustain_s);
    let mut tick = tokio::time::interval(Duration::from_secs_f32(poll_s));
    loop {
        tick.tick().await;
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        let cpu_pct = sys.global_cpu_usage();
        let ram_pct = if sys.total_memory() == 0 {
            0.0
        } else {
            sys.used_memory() as f32 / sys.total_memory() as f32 * 100.0
        };
        if cpu.sample(cpu_pct, poll_s) {
            bus.publish(Event::with_detail("cpu.high", format!("{cpu_pct:.0}%")));
        }
        if ram.sample(ram_pct, poll_s) {
            bus.publish(Event::with_detail("ram.high", format!("{ram_pct:.0}%")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hysteresis_and_sustain() {
        let mut t = Threshold::new(72.0, 10.0);
        assert!(!t.sample(90.0, 5.5));
        assert!(t.sample(90.0, 5.5)); // sustained 11 s
        assert!(!t.sample(90.0, 5.5)); // no re-fire while high
        assert!(!t.sample(70.0, 5.5)); // still within hysteresis band
        assert!(!t.sample(90.0, 5.5)); // not re-armed
        assert!(!t.sample(50.0, 5.5)); // re-arm
        assert!(!t.sample(90.0, 5.5));
        assert!(t.sample(90.0, 5.5));
    }
}
