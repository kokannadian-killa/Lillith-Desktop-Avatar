//! Desktop-wide presence: is the user at the machine at all (not just
//! touching Lillith)? Turns an [`IdleSource`]'s "seconds since last input"
//! into the events the brain already understands:
//!
//! - `user.activity`: input within `activity_window_s`, sent at most every
//!   `activity_every_s`. Resets her drowsy/asleep timers, so she does not
//!   doze off while you work in other windows.
//! - `user.returned`: the first input after being idle ≥ `away_after_s`
//!   (detail: how long you were away). Wakes her with the `peeking` rule.
//!
//! `idle.long` / `idle.asleep` are **not** emitted here: once activity stops
//! arriving, the brain's own `sleepy_after_s` / `asleep_after_s` timers make
//! her drowsy and fall asleep, so there is one source of truth for timing.
//!
//! Sources (see [`detect_source`]):
//! - Wayland (`wayland-idle` feature): `ext-idle-notify-v1`, which KWin
//!   implements. Idle inhibitors (fullscreen video) count as activity.
//! - X11 sessions (`x11` feature): the MIT-SCREEN-SAVER extension
//!   (`ms_since_user_input`).
//! - Anything else: no source; presence is disabled and logged. The
//!   `org.freedesktop.ScreenSaver.GetSessionIdleTime` call is not used: KDE
//!   answers `NotSupported` on Wayland and milliseconds on X11.

use lillith_core::config::MonitorConfig;
use lillith_core::Event;

/// Something that knows how long the desktop has been idle.
pub trait IdleSource: Send {
    /// Seconds since the last user input; `None` if currently unknown.
    fn idle_for(&mut self) -> Option<f64>;
    /// Short name for logs.
    fn name(&self) -> &'static str;
}

/// Pure presence state machine (fake-clock tested).
#[derive(Debug, Clone)]
pub struct Presence {
    activity_window_s: f64,
    activity_every_s: f64,
    away_after_s: f64,
    last_activity_event: Option<f64>,
    /// Longest idle stretch seen since the last input.
    idle_peak: f64,
}

impl Presence {
    pub fn new(cfg: &MonitorConfig) -> Self {
        Self {
            activity_window_s: cfg.activity_window_s.max(0.5) as f64,
            activity_every_s: cfg.activity_every_s.max(1.0) as f64,
            away_after_s: cfg.away_after_s.max(1.0) as f64,
            last_activity_event: None,
            idle_peak: 0.0,
        }
    }

    /// Feeds the idle time observed at `now`.
    pub fn update(&mut self, now: f64, idle_for: Option<f64>) -> Vec<Event> {
        let Some(idle) = idle_for else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if idle >= self.activity_window_s {
            self.idle_peak = self.idle_peak.max(idle);
            return out;
        }
        // Active now.
        if self.idle_peak >= self.away_after_s {
            out.push(Event::with_detail(
                "user.returned",
                format!("away {}", human(self.idle_peak)),
            ));
            self.last_activity_event = Some(now);
        } else if self
            .last_activity_event
            .is_none_or(|t| now - t >= self.activity_every_s)
        {
            out.push(Event::new("user.activity"));
            self.last_activity_event = Some(now);
        }
        self.idle_peak = idle;
        out
    }
}

fn human(s: f64) -> String {
    let s = s.round() as u64;
    if s >= 3600 {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

/// Picks the best idle source for this session, or `None`.
pub fn detect_source(cfg: &MonitorConfig) -> Option<Box<dyn IdleSource>> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
    if wayland {
        #[cfg(feature = "wayland-idle")]
        match crate::idle_wayland::WaylandIdle::connect(cfg.activity_window_s) {
            Ok(src) => return Some(Box::new(src)),
            Err(e) => tracing::warn!("desktop idle (Wayland ext-idle-notify): {e}"),
        }
        #[cfg(not(feature = "wayland-idle"))]
        tracing::warn!("desktop idle: built without the `wayland-idle` feature");
        // XWayland's screensaver counter only sees X clients' input: not used.
        return None;
    }
    let x11 = std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty());
    if x11 {
        #[cfg(feature = "x11")]
        match crate::idle_x11::X11Idle::connect() {
            Ok(src) => return Some(Box::new(src)),
            Err(e) => tracing::warn!("desktop idle (X11 MIT-SCREEN-SAVER): {e}"),
        }
        #[cfg(not(feature = "x11"))]
        tracing::warn!("desktop idle: built without the `x11` feature");
    }
    let _ = cfg;
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(ev: &[Event]) -> Vec<&str> {
        ev.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn activity_is_rate_limited() {
        let mut p = Presence::new(&MonitorConfig::default()); // window 10, every 30, away 120
        assert_eq!(names(&p.update(0.0, Some(0.5))), ["user.activity"]);
        assert!(p.update(2.0, Some(1.0)).is_empty());
        assert!(p.update(28.0, Some(3.0)).is_empty());
        assert_eq!(names(&p.update(30.0, Some(0.0))), ["user.activity"]);
        assert!(p.update(32.0, None).is_empty(), "unknown is ignored");
    }

    #[test]
    fn short_idle_is_not_away_long_idle_returns() {
        let mut p = Presence::new(&MonitorConfig::default());
        p.update(0.0, Some(0.0));
        // 60 s break: no user.returned, just activity again.
        assert!(p.update(40.0, Some(40.0)).is_empty());
        assert!(p.update(60.0, Some(60.0)).is_empty());
        assert_eq!(names(&p.update(62.0, Some(1.0))), ["user.activity"]);
        // ~7 min away (idle counter peaks at 418 s).
        for t in (70..=480).step_by(10) {
            assert!(p.update(t as f64, Some(t as f64 - 62.0)).is_empty());
        }
        let ev = p.update(482.0, Some(0.5));
        assert_eq!(ev, [Event::with_detail("user.returned", "away 6m")]);
        // Next activity is rate limited from the return.
        assert!(p.update(490.0, Some(0.5)).is_empty());
        assert_eq!(names(&p.update(512.0, Some(0.5))), ["user.activity"]);
    }

    #[test]
    fn human_durations() {
        assert_eq!(human(42.0), "42s");
        assert_eq!(human(125.0), "2m");
        assert_eq!(human(3720.0), "1h02m");
    }

    #[test]
    fn presence_events_reach_the_brain() {
        let rules = lillith_core::rules::RuleSet::bundled();
        assert!(rules.find("user.returned").is_some());
        // user.activity is handled by Brain::event directly (no rule needed).
    }
}
