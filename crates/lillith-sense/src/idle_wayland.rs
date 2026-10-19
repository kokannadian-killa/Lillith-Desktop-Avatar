//! Wayland desktop idle via `ext-idle-notify-v1` (KWin, Sway, Hyprland …).
//!
//! The protocol is edge-triggered: the compositor sends `idled` after
//! `timeout` ms without input and `resumed` on the next input. A small
//! thread owns a dedicated Wayland connection and records when the session
//! went idle; [`WaylandIdle::idle_for`] turns that into seconds.

use crate::presence::IdleSource;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};

/// Shared "idle since" instant (`None` = active).
type Shared = Arc<Mutex<Option<Instant>>>;

struct State {
    shared: Shared,
    timeout: Duration,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotifierV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ExtIdleNotifierV1,
        _: <ExtIdleNotifierV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotificationV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut idle = state.shared.lock().unwrap_or_else(|p| p.into_inner());
        match event {
            ext_idle_notification_v1::Event::Idled => {
                // The last input happened `timeout` before this event.
                *idle = Some(
                    Instant::now()
                        .checked_sub(state.timeout)
                        .unwrap_or_else(Instant::now),
                );
            }
            ext_idle_notification_v1::Event::Resumed => *idle = None,
            _ => {}
        }
    }
}

/// `ext-idle-notify-v1` idle source.
#[derive(Debug)]
pub struct WaylandIdle {
    shared: Shared,
}

impl WaylandIdle {
    /// Connects to `$WAYLAND_DISPLAY` and asks for an idle notification
    /// after `timeout_s` seconds without input.
    pub fn connect(timeout_s: f32) -> Result<Self, String> {
        let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
        let (globals, mut queue) =
            registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let seat: wl_seat::WlSeat = globals
            .bind(&qh, 1..=1, ())
            .map_err(|e| format!("wl_seat: {e}"))?;
        let notifier: ExtIdleNotifierV1 = globals
            .bind(&qh, 1..=1, ())
            .map_err(|e| format!("compositor lacks ext_idle_notifier_v1: {e}"))?;
        let timeout = Duration::from_secs_f32(timeout_s.clamp(1.0, 3600.0));
        let notification =
            notifier.get_idle_notification(timeout.as_millis() as u32, &seat, &qh, ());
        let shared: Shared = Arc::new(Mutex::new(None));
        let mut state = State {
            shared: shared.clone(),
            timeout,
        };
        queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
        std::thread::Builder::new()
            .name("lillith-idle-wl".into())
            .spawn(move || {
                // Keep the protocol objects alive for the thread's lifetime.
                let _keep = (conn, seat, notifier, notification);
                loop {
                    if let Err(e) = queue.blocking_dispatch(&mut state) {
                        tracing::warn!("ext-idle-notify connection closed: {e}");
                        *state.shared.lock().unwrap_or_else(|p| p.into_inner()) = None;
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        tracing::info!(
            timeout_s = timeout.as_secs_f32(),
            "desktop idle: ext-idle-notify-v1"
        );
        Ok(Self { shared })
    }
}

impl IdleSource for WaylandIdle {
    fn idle_for(&mut self) -> Option<f64> {
        let idle = *self.shared.lock().unwrap_or_else(|p| p.into_inner());
        Some(idle.map_or(0.0, |t| t.elapsed().as_secs_f64()))
    }

    fn name(&self) -> &'static str {
        "ext-idle-notify-v1"
    }
}
