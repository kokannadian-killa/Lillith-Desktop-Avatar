//! Layer-shell overlay on KDE Plasma 6 Wayland (`zwlr_layer_shell_v1`,
//! which KWin implements).
//!
//! - `Layer::Overlay`, anchor `BOTTOM | LEFT`, exclusive zone −1 (ignores
//!   panels' exclusive zones, never pushes windows), keyboard
//!   `OnDemand`.
//! - Position = margins (`bottom`, `left`), changed live while dragging.
//! - ARGB8888 `wl_shm` buffers from a slot pool, premultiplied.
//! - Click-through: `wl_surface.set_input_region` = the frame's mask
//!   rectangles; everything outside her silhouette goes to whatever is
//!   underneath.
//! - calloop drives a `render_hz` timer; frames are only re-uploaded when
//!   the stage says something changed.

use crate::drag::{Placement, Release};
use crate::pixels::rgba_to_argb8888_le;
use crate::run::{period, Controller, OverlayError, OverlayOptions, KEEP_ON_SCREEN};
use lillith_core::interaction::Button;
use lillith_core::state::Key;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            EventLoop, LoopSignal,
        },
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers},
        pointer::{
            CursorIcon, PointerEvent, PointerEventKind, PointerHandler, ThemeSpec, ThemedPointer,
            BTN_LEFT, BTN_MIDDLE, BTN_RIGHT,
        },
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use std::time::Instant;
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, QueueHandle,
};

pub use smithay_client_toolkit as sctk;
pub use wayland_client;

/// Layer-surface namespace (KWin window rules can match it).
pub const NAMESPACE: &str = "lillith";

struct App {
    registry: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    shm: Shm,
    pool: SlotPool,
    layer: LayerSurface,
    pointer: Option<ThemedPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    conn: Connection,
    ctl: Controller,
    configured: bool,
    /// Placement waiting to be committed (drag).
    pending: Option<Placement>,
    mask_dirty: bool,
    pixels_dirty: bool,
    signal: LoopSignal,
    last: Instant,
    started: Instant,
    exit_after: Option<std::time::Duration>,
    error: Option<OverlayError>,
}

/// Connects, maps the layer surface and runs until closed.
pub fn run(opts: &OverlayOptions) -> Result<(), OverlayError> {
    let conn = Connection::connect_to_env()
        .map_err(|e| OverlayError::WaylandUnavailable(format!("no Wayland display ({e})")))?;
    let (globals, queue) =
        registry_queue_init::<App>(&conn).map_err(|e| OverlayError::Wayland(e.to_string()))?;
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh)
        .map_err(|e| OverlayError::Wayland(format!("wl_compositor: {e}")))?;
    let layer_shell = LayerShell::bind(&globals, &qh).map_err(|e| {
        OverlayError::WaylandUnavailable(format!("compositor has no zwlr_layer_shell_v1 ({e})"))
    })?;
    let shm =
        Shm::bind(&globals, &qh).map_err(|e| OverlayError::Wayland(format!("wl_shm: {e}")))?;

    let ctl = Controller::new(opts)?;
    let (w, h) = ctl.stage.size();

    let surface = compositor.create_surface(&qh);
    let layer =
        layer_shell.create_layer_surface(&qh, surface, Layer::Overlay, Some(NAMESPACE), None);
    layer.set_anchor(Anchor::BOTTOM | Anchor::LEFT);
    layer.set_exclusive_zone(-1);
    layer.set_keyboard_interactivity(KeyboardInteractivity::OnDemand);
    layer.set_size(w, h);
    layer.set_margin(0, 0, ctl.placement.bottom, ctl.placement.left);
    // Empty input region until the first frame: never block clicks.
    if let Ok(r) = Region::new(&compositor) {
        layer.wl_surface().set_input_region(Some(r.wl_region()));
    }
    layer.commit();

    let pool = SlotPool::new((w * h * 4 * 2) as usize, &shm)
        .map_err(|e| OverlayError::Wayland(format!("shm pool: {e}")))?;

    let mut event_loop: EventLoop<App> =
        EventLoop::try_new().map_err(|e| OverlayError::Wayland(e.to_string()))?;
    let handle = event_loop.handle();
    WaylandSource::new(conn.clone(), queue)
        .insert(handle.clone())
        .map_err(|e| OverlayError::Wayland(format!("event source: {e}")))?;

    let frame = period(opts.render_hz);
    handle
        .insert_source(Timer::from_duration(frame), move |_, _, app: &mut App| {
            app.on_timer();
            TimeoutAction::ToDuration(frame)
        })
        .map_err(|e| OverlayError::Wayland(format!("timer: {e}")))?;

    let mut app = App {
        registry: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        shm,
        pool,
        layer,
        pointer: None,
        keyboard: None,
        conn,
        ctl,
        configured: false,
        pending: None,
        mask_dirty: true,
        pixels_dirty: true,
        signal: event_loop.get_signal(),
        last: Instant::now(),
        started: Instant::now(),
        exit_after: opts.exit_after,
        error: None,
    };
    tracing::info!(
        size = ?(w, h),
        left = app.ctl.placement.left,
        bottom = app.ctl.placement.bottom,
        "wayland layer-shell overlay (Overlay layer, anchor bottom|left)"
    );
    event_loop
        .run(None, &mut app, |_| {})
        .map_err(|e| OverlayError::Wayland(e.to_string()))?;
    app.ctl.log_stats();
    match app.error.take() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

impl App {
    fn on_timer(&mut self) {
        if let Some(limit) = self.exit_after {
            if self.started.elapsed() >= limit {
                self.signal.stop();
                return;
            }
        }
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.25);
        self.last = now;
        if !self.ctl.has_screen() {
            if let Some(out) = self.output_size() {
                self.ctl.set_screen(out);
            }
        }
        match self.ctl.tick(dt) {
            Ok(c) => {
                self.pixels_dirty |= c.pixels;
                self.mask_dirty |= c.mask;
            }
            Err(e) => {
                self.error = Some(e);
                self.signal.stop();
                return;
            }
        }
        if self.ctl.quit_requested() {
            self.signal.stop();
            return;
        }
        if let Some(p) = self.ctl.take_move() {
            if !self.ctl.drag.active() {
                self.pending = Some(p);
            }
        }
        if self.configured && (self.pixels_dirty || self.mask_dirty || self.pending.is_some()) {
            self.draw();
        }
    }

    fn output_size(&self) -> Option<(u32, u32)> {
        self.output_state.outputs().find_map(|o| {
            let info = self.output_state.info(&o)?;
            let (w, h) = info.logical_size?;
            Some((w.max(1) as u32, h.max(1) as u32))
        })
    }

    fn draw(&mut self) {
        let (w, h) = self.ctl.stage.size();
        let surface = self.layer.wl_surface().clone();
        if self.pixels_dirty {
            match self.pool.create_buffer(
                w as i32,
                h as i32,
                w as i32 * 4,
                wl_shm::Format::Argb8888,
            ) {
                Ok((buffer, canvas)) => {
                    rgba_to_argb8888_le(self.ctl.stage.canvas().data(), canvas, self.ctl.opacity);
                    surface.damage_buffer(0, 0, w as i32, h as i32);
                    if let Err(e) = buffer.attach_to(&surface) {
                        tracing::warn!("attach: {e}");
                    }
                    self.pixels_dirty = false;
                }
                // Pool busy (compositor still holds buffers): try next tick.
                Err(e) => tracing::debug!("shm buffer: {e}"),
            }
        }
        if self.mask_dirty {
            if let Ok(region) = Region::new(&self.compositor) {
                for r in self.ctl.stage.mask() {
                    region.add(r.x, r.y, r.w, r.h);
                }
                surface.set_input_region(Some(region.wl_region()));
                self.mask_dirty = false;
            }
        }
        if let Some(p) = self.pending.take() {
            let p = match self.output_size() {
                Some(out) => p.clamped((w, h), out, KEEP_ON_SCREEN),
                None => p,
            };
            self.layer.set_margin(0, 0, p.bottom, p.left);
            self.ctl.placement = p;
        }
        self.layer.commit();
    }

    fn set_cursor(&self, icon: CursorIcon) {
        if let Some(p) = &self.pointer {
            let _ = p.set_cursor(&self.conn, icon);
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        factor: i32,
    ) {
        // TODO(M6): render at `factor` × and set_buffer_scale for HiDPI.
        tracing::debug!(factor, "output scale changed");
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        tracing::info!("layer surface closed by the compositor");
        self.signal.stop();
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &LayerSurface,
        _: LayerSurfaceConfigure,
        _: u32,
    ) {
        // We asked for a fixed size and keep it whatever the compositor says.
        if !self.configured {
            self.configured = true;
            self.pixels_dirty = true;
            self.mask_dirty = true;
            if let Err(e) = self.ctl.stage.render() {
                self.error = Some(e.into());
                self.signal.stop();
                return;
            }
            self.draw();
        }
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            let cursor_surface = self.compositor.create_surface(qh);
            match self.seat_state.get_pointer_with_theme(
                qh,
                &seat,
                self.shm.wl_shm(),
                cursor_surface,
                ThemeSpec::default(),
            ) {
                Ok(p) => self.pointer = Some(p),
                Err(e) => tracing::warn!("pointer: {e}"),
            }
        }
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            match self.seat_state.get_keyboard(qh, &seat, None) {
                Ok(k) => self.keyboard = Some(k),
                Err(e) => tracing::warn!("keyboard: {e}"),
            }
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(p) = self.pointer.take() {
                p.pointer().release();
            }
        }
        if capability == Capability::Keyboard {
            if let Some(k) = self.keyboard.take() {
                k.release();
            }
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for ev in events {
            if &ev.surface != self.layer.wl_surface() {
                continue;
            }
            match ev.kind {
                PointerEventKind::Enter { .. } => {
                    self.set_cursor(CursorIcon::Grab);
                    self.ctl.hover(true);
                }
                PointerEventKind::Leave { .. } => {
                    if !self.ctl.drag.active() {
                        self.ctl.hover(false);
                    }
                }
                PointerEventKind::Press { button, .. } if button == BTN_LEFT => {
                    if self.ctl.press(ev.position, ev.position, true) {
                        self.set_cursor(CursorIcon::Grabbing);
                    }
                }
                PointerEventKind::Press { button, .. }
                    if button == BTN_RIGHT || button == BTN_MIDDLE =>
                {
                    let b = if button == BTN_RIGHT {
                        Button::Right
                    } else {
                        Button::Middle
                    };
                    self.ctl.button(b, ev.position);
                }
                PointerEventKind::Motion { .. } => {
                    if let Some(p) = self.ctl.motion(ev.position) {
                        self.pending = Some(p);
                    }
                }
                PointerEventKind::Release { button, .. } if button == BTN_LEFT => {
                    // Commit any last motion first so the saved spot matches.
                    if let Some(p) = self.pending.take() {
                        self.layer.set_margin(0, 0, p.bottom, p.left);
                        self.ctl.placement = p;
                        self.layer.commit();
                    }
                    if !matches!(self.ctl.release(), Release::None) {
                        self.set_cursor(CursorIcon::Grab);
                    }
                }
                _ => {}
            }
        }
    }
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
    }
    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
    }
    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        let key = match event.keysym {
            Keysym::Escape => Some(Key::Escape),
            Keysym::Return | Keysym::KP_Enter => Some(Key::Enter),
            _ => event
                .utf8
                .as_deref()
                .and_then(|s| s.chars().next())
                .filter(|c| !c.is_control())
                .map(Key::Char),
        };
        if let Some(k) = key {
            self.ctl.key(k);
            if self.ctl.quit_requested() {
                self.signal.stop();
            }
        }
    }
    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }
    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: Modifiers,
        _: u32,
    ) {
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_compositor!(App);
delegate_output!(App);
delegate_shm!(App);
delegate_seat!(App);
delegate_pointer!(App);
delegate_keyboard!(App);
delegate_layer!(App);
delegate_registry!(App);

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}
