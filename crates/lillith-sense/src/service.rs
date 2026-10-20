//! Runs the M4 monitors configured by `[monitor]` and forwards their events
//! to a [`Sink`]. Must be called from inside a tokio runtime.
//!
//! - **poller** (one task): every `poll_s` reads a [`Sample`] (≈ a dozen
//!   small procfs/sysfs files, no processes spawned), feeds the pure
//!   [`Monitor`], and asks the desktop [`IdleSource`] for [`Presence`].
//! - **notifications** (one task): session-bus monitor.
//! - **files**: an inotify watcher (kernel-driven, no polling).
//! - **KWin bridge** (one task): serves `org.lillith.Bridge`; `window.*`
//!   events only arrive when the KWin script is installed (M5), nothing faked.

use crate::monitor::{Monitor, Sample};
use crate::presence::{self, IdleSource, Presence};
use crate::sysfs::Sys;
use crate::Sink;
use lillith_core::config::MonitorConfig;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;

/// Keeps the monitors alive; dropping it stops them.
#[derive(Debug, Default)]
pub struct MonitorHandle {
    tasks: Vec<JoinHandle<()>>,
    files: Option<notify::RecommendedWatcher>,
}

impl MonitorHandle {
    /// Number of background tasks (poller, notifications, KWin bridge).
    pub fn tasks(&self) -> usize {
        self.tasks.len()
    }

    /// `true` if the inotify watcher is running.
    pub fn watching_files(&self) -> bool {
        self.files.is_some()
    }
}

impl Drop for MonitorHandle {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// Starts every monitor enabled in `cfg`. With `cfg.enabled == false` this
/// returns an empty handle.
pub fn spawn(cfg: MonitorConfig, sink: Sink) -> MonitorHandle {
    spawn_with(cfg, sink, Sys::default(), home_dir())
}

/// [`spawn`] against an explicit sysfs/procfs root and home directory (tests).
pub fn spawn_with(
    cfg: MonitorConfig,
    sink: Sink,
    sys: Sys,
    home: Option<PathBuf>,
) -> MonitorHandle {
    let mut h = MonitorHandle::default();
    if !cfg.enabled {
        tracing::info!("system monitor disabled");
        return h;
    }
    h.tasks
        .push(tokio::spawn(poll_loop(cfg.clone(), sink.clone(), sys)));

    if cfg.notifications {
        let sink = sink.clone();
        h.tasks.push(tokio::spawn(async move {
            if let Err(e) = crate::notifications::run(sink).await {
                tracing::warn!("notification monitor unavailable: {e}");
            }
        }));
    }

    if cfg.files {
        match home {
            Some(home) => {
                match crate::files::watch(&home, sink.clone(), cfg.event_cooldown_s.max(0.0) as f64)
                {
                    Ok(w) => h.files = Some(w),
                    Err(e) => tracing::warn!("file watcher unavailable: {e}"),
                }
            }
            None => tracing::warn!("file watcher: $HOME not set"),
        }
    }

    if cfg.kwin_bridge {
        // Receiver only: events arrive once the kwin/lillith-bridge script is
        // installed and enabled (packaging + Plasma 6 verification: M5).
        let sink = sink.clone();
        h.tasks.push(tokio::spawn(async move {
            if let Err(e) = crate::kwin::serve(sink).await {
                tracing::warn!("KWin bridge unavailable: {e}");
            }
        }));
    }
    h
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

async fn poll_loop(cfg: MonitorConfig, sink: Sink, sys: Sys) {
    let period = Duration::from_secs_f32(cfg.poll_s.clamp(0.25, 60.0));
    // Connecting to the compositor / X server may block briefly.
    let mut idle: Option<Box<dyn IdleSource>> = if cfg.idle {
        let c = cfg.clone();
        tokio::task::spawn_blocking(move || presence::detect_source(&c))
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    match &idle {
        Some(src) => tracing::info!(source = src.name(), "desktop idle detection"),
        None if cfg.idle => tracing::info!(
            "no desktop idle source: only input on Lillith and injected events count as activity"
        ),
        None => {}
    }
    let mut presence = Presence::new(&cfg);
    let mut monitor = Monitor::new(cfg);
    let start = Instant::now();
    let mut tick = tokio::time::interval(period);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut logged = false;
    loop {
        tick.tick().await;
        let now = start.elapsed().as_secs_f64();
        let sample = Sample::read(&sys, monitor.config());
        let mut events = monitor.step(now, &sample);
        if let Some(src) = idle.as_mut() {
            events.extend(presence.update(now, src.idle_for()));
        }
        if !logged && monitor.cpu_percent.is_some() {
            tracing::info!("system monitor: {}", monitor.summary(&sample));
            logged = true;
        }
        for ev in events {
            tracing::info!(event = %ev.name, detail = ?ev.detail, "monitor");
            sink(ev);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysfs::tests::laptop;
    use lillith_core::Event;
    use std::sync::{Arc, Mutex};

    fn collector() -> (Sink, Arc<Mutex<Vec<Event>>>) {
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        (Arc::new(move |e| g.lock().unwrap().push(e)), got)
    }

    #[tokio::test]
    async fn disabled_spawns_nothing() {
        let (sink, _) = collector();
        let h = spawn(
            MonitorConfig {
                enabled: false,
                ..MonitorConfig::default()
            },
            sink,
        );
        assert_eq!(h.tasks(), 0);
        assert!(!h.watching_files());
    }

    #[tokio::test]
    async fn poller_reads_fixture_tree() {
        let f = laptop("svc");
        let (sink, got) = collector();
        let cfg = MonitorConfig {
            poll_s: 0.25,
            idle: false,
            notifications: false,
            files: false,
            kwin_bridge: false,
            ..MonitorConfig::default()
        };
        let h = spawn_with(cfg, sink, f.sys(), None);
        assert_eq!(h.tasks(), 1);
        // Fixture battery is 14 % and discharging → battery.low on the first sample.
        for _ in 0..40 {
            if !got.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let names: Vec<String> = got.lock().unwrap().iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, ["battery.low"]);
        drop(h);
    }
}
