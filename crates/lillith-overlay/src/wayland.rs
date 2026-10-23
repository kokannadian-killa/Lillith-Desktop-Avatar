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
//! - Right-click menu: an `xdg_popup` parented to her layer surface
//!   (`zwlr_layer_surface_v1.get_popup`) at the pointer, showing
//!   [`crate::menu_popup`], with an explicit popup grab, so the compositor
//!   closes it on a click anywhere else (`popup_done`) and gives it the
//!   keyboard (Esc / arrows / Enter). If the compositor never configures
//!   the popup within [`POPUP_CONFIGURE_TIMEOUT`] (or `xdg_wm_base` is
//!   missing, or `LILLITH_MENU=layer`), the menu is shown as its own
//!   layer surface (Overlay layer, keyboard `OnDemand`) at the pointer.
//! - Buttons delivered to her surface are trusted as "on her": the
//!   compositor only delivers them inside the input region, so they are
//!   not hit-tested a second time (the logged `mask_hit` is diagnostic).

use crate::drag::{Placement, Release};
use crate::menu_popup::MenuKey;
use crate::pixels::rgba_to_argb8888_le;
use crate::run::{period, Controller, OverlayError, OverlayOptions, KEEP_ON_SCREEN};
use crate::wl_geom::{
    alpha_stats, menu_layer_margins, popup_timed_out, size_mismatch, POPUP_CONFIGURE_TIMEOUT,
};
use lillith_core::interaction::Button;
use lillith_core::state::Key;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm, delegate_xdg_popup, delegate_xdg_shell,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            EventLoop, LoopSignal,
        },
        calloop_wayland_source::WaylandSource,
        protocols::xdg::shell::client::xdg_positioner,
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
        xdg::{
            popup::{Popup, PopupConfigure, PopupHandler},
            XdgPositioner, XdgShell,
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

/// The open right-click menu surface.
enum MenuSurface {
    /// `xdg_popup` on her layer surface (preferred: compositor grab).
    Popup {
        popup: Popup,
        configured: bool,
        opened: Instant,
    },
    /// Fallback: a separate Overlay layer surface at the pointer.
    Layer {
        layer: LayerSurface,
        configured: bool,
    },
}

impl MenuSurface {
    fn wl_surface(&self) -> &wl_surface::WlSurface {
        match self {
            MenuSurface::Popup { popup, .. } => popup.wl_surface(),
            MenuSurface::Layer { layer, .. } => layer.wl_surface(),
        }
    }
    fn configured(&self) -> bool {
        match self {
            MenuSurface::Popup { configured, .. } | MenuSurface::Layer { configured, .. } => {
                *configured
            }
        }
    }
    fn kind(&self) -> &'static str {
        match self {
            MenuSurface::Popup { .. } => "xdg_popup",
            MenuSurface::Layer { .. } => "layer-surface",
        }
    }
}

struct App {
    registry: RegistryState,
    /// `xdg_wm_base` for the menu popup (`None`: the menu only logs).
    xdg: Option<XdgShell>,
    layer_shell: LayerShell,
    menu: Option<MenuSurface>,
    /// The popup path failed once: use the layer-surface menu from now on.
    menu_use_layer: bool,
    /// Last integer output scale the compositor reported for her surface.
    output_scale: i32,
    /// The first frame's transparency self-check was logged.
    alpha_logged: bool,
    seat: Option<wl_seat::WlSeat>,
    /// Serial + surface-local position of the last button press on her
    /// (the popup grab needs the serial).
    last_press: Option<(u32, (f64, f64))>,
    qh: QueueHandle<App>,
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
    /// Size last requested with `set_size`, always together with a buffer
    /// of that size (see [`App::apply_resize`]).
    surface_size: (u32, u32),
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

    let xdg = match XdgShell::bind(&globals, &qh) {
        Ok(x) => Some(x),
        Err(e) => {
            tracing::warn!("no xdg_wm_base ({e}): right-click menu disabled on Wayland");
            None
        }
    };
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
    // Buffers are drawn at 1 buffer px per surface px; say so explicitly
    // (KWin scales the surface for HiDPI outputs).
    layer.wl_surface().set_buffer_scale(1);
    // Never declare any part of the surface opaque: the compositor must
    // blend every pixel with its alpha (transparent background).
    layer.wl_surface().set_opaque_region(None);
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
            // The rate can change (custom avatars redraw more often).
            TimeoutAction::ToDuration(period(app.ctl.render_hz()))
        })
        .map_err(|e| OverlayError::Wayland(format!("timer: {e}")))?;

    let mut app = App {
        registry: RegistryState::new(&globals),
        xdg,
        layer_shell,
        menu: None,
        menu_use_layer: std::env::var("LILLITH_MENU").is_ok_and(|v| v == "layer"),
        output_scale: 1,
        alpha_logged: false,
        seat: None,
        last_press: None,
        qh: qh.clone(),
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
        surface_size: (w, h),
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
    tracing::info!(
        backend = "wayland",
        format = "argb8888",
        alpha = true,
        "transparency: premultiplied ARGB8888 wl_shm buffers cleared to 0x00000000, no opaque region (the compositor blends per-pixel alpha)"
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
        self.apply_resize();
        if let Some(on_top) = self.ctl.take_level() {
            self.layer.set_layer(if on_top {
                Layer::Overlay
            } else {
                Layer::Bottom
            });
            self.layer.commit();
        }
        if let Some(p) = self.ctl.take_move() {
            if !self.ctl.drag.active() {
                self.pending = Some(p);
            }
        }
        self.sync_menu();
        if self.configured && (self.pixels_dirty || self.mask_dirty || self.pending.is_some()) {
            self.draw();
        }
    }

    /// Resizes the layer surface when the stage changed size (Resize,
    /// peek, speech area). Anchored bottom|left: the feet stay put.
    fn apply_resize(&mut self) {
        if !self.ctl.take_resized() {
            return;
        }
        // The new size is requested in `draw`, in the same commit as the
        // first buffer of that size: the surface never shows a size whose
        // pixels were not drawn yet (undefined / black on some compositors).
        self.pending.get_or_insert(self.ctl.placement);
        if let Err(e) = self.ctl.stage.render() {
            tracing::warn!("render after resize: {e}");
        }
        self.pixels_dirty = true;
        self.mask_dirty = true;
    }

    /// Shows / hides / redraws the menu popup to match the controller.
    fn sync_menu(&mut self) {
        if self.ctl.take_menu_opened() {
            self.menu = None;
            let first = if self.menu_use_layer {
                self.open_menu_layer()
            } else {
                self.open_menu().or_else(|e| {
                    tracing::warn!("menu popup: xdg_popup failed ({e}); using a layer surface");
                    self.menu_use_layer = true;
                    self.open_menu_layer()
                })
            };
            match first {
                Ok(m) => self.menu = Some(m),
                Err(e) => {
                    tracing::warn!("menu popup: {e}");
                    self.ctl.close_menu();
                }
            }
        }
        if self.ctl.menu().is_none() {
            if let Some(m) = self.menu.take() {
                tracing::info!(kind = m.kind(), "menu closed");
            }
            return;
        }
        let stalled = matches!(
            &self.menu,
            Some(MenuSurface::Popup { configured, opened, .. })
                if popup_timed_out(opened.elapsed(), *configured)
        );
        if stalled {
            tracing::warn!(
                timeout_ms = POPUP_CONFIGURE_TIMEOUT.as_millis() as u64,
                "menu popup: xdg_popup never configured; showing the menu as a layer surface"
            );
            self.menu = None;
            self.menu_use_layer = true;
            match self.open_menu_layer() {
                Ok(m) => self.menu = Some(m),
                Err(e) => {
                    tracing::warn!("menu layer surface: {e}");
                    self.ctl.close_menu();
                    return;
                }
            }
        }
        self.draw_menu();
    }

    /// Fallback menu: its own Overlay layer surface with its top-left at
    /// the pointer (no compositor grab: it closes on a pick, Esc, a click
    /// on her, or when it loses keyboard focus).
    fn open_menu_layer(&mut self) -> Result<MenuSurface, String> {
        let (w, h) = self.ctl.menu().map(|m| m.size()).ok_or("no menu")?;
        let at = self.last_press.map(|p| p.1).unwrap_or((0.0, 0.0));
        let (left, bottom) = menu_layer_margins(
            self.ctl.placement,
            self.ctl.stage.size(),
            at,
            (w, h),
            self.output_size(),
        );
        let surface = self.compositor.create_surface(&self.qh);
        let layer = self.layer_shell.create_layer_surface(
            &self.qh,
            surface,
            Layer::Overlay,
            Some("lillith-menu"),
            None,
        );
        layer.set_anchor(Anchor::BOTTOM | Anchor::LEFT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::OnDemand);
        layer.set_size(w, h);
        layer.set_margin(0, 0, bottom, left);
        layer.wl_surface().set_buffer_scale(1);
        layer.wl_surface().set_opaque_region(None);
        layer.commit();
        tracing::info!(
            at = ?at,
            size = ?(w, h),
            left,
            bottom,
            "wayland menu popup created (layer-surface fallback, Overlay layer)"
        );
        Ok(MenuSurface::Layer {
            layer,
            configured: false,
        })
    }

    fn open_menu(&mut self) -> Result<MenuSurface, String> {
        let xdg = self.xdg.as_ref().ok_or("no xdg_wm_base")?;
        let (w, h) = self.ctl.menu().map(|m| m.size()).ok_or("no menu")?;
        let at = self.last_press.map(|p| p.1).unwrap_or((0.0, 0.0));
        let pos = XdgPositioner::new(xdg).map_err(|e| e.to_string())?;
        pos.set_size(w as i32, h as i32);
        pos.set_anchor_rect(at.0 as i32, at.1 as i32, 1, 1);
        pos.set_anchor(xdg_positioner::Anchor::BottomRight);
        pos.set_gravity(xdg_positioner::Gravity::BottomRight);
        pos.set_constraint_adjustment(
            xdg_positioner::ConstraintAdjustment::FlipX
                | xdg_positioner::ConstraintAdjustment::FlipY
                | xdg_positioner::ConstraintAdjustment::SlideX
                | xdg_positioner::ConstraintAdjustment::SlideY,
        );
        let surface = self.compositor.create_surface(&self.qh);
        let popup =
            Popup::from_surface(None, &pos, &self.qh, surface, xdg).map_err(|e| e.to_string())?;
        self.layer.get_popup(popup.xdg_popup());
        // Explicit grab: the compositor dismisses it on outside clicks and
        // gives it keyboard focus.
        let serial = match (&self.seat, self.last_press) {
            (Some(seat), Some((serial, _))) => {
                popup.xdg_popup().grab(seat, serial);
                Some(serial)
            }
            _ => {
                tracing::warn!("menu popup without grab (no seat or press serial)");
                None
            }
        };
        popup.wl_surface().set_buffer_scale(1);
        popup.wl_surface().set_opaque_region(None);
        popup.wl_surface().commit();
        tracing::info!(
            at = ?at,
            size = ?(w, h),
            grab_serial = ?serial,
            "wayland menu popup created (xdg_popup on the layer surface); waiting for configure"
        );
        Ok(MenuSurface::Popup {
            popup,
            configured: false,
            opened: Instant::now(),
        })
    }

    fn draw_menu(&mut self) {
        let Some(m) = self.menu.as_ref() else {
            return;
        };
        if !m.configured() {
            return;
        }
        let surface = m.wl_surface().clone();
        let region = self.ctl.menu().map(|m| m.region()).unwrap_or_default();
        let Some(pm) = self.ctl.render_menu() else {
            return;
        };
        let (w, h) = (pm.width(), pm.height());
        match self
            .pool
            .create_buffer(w as i32, h as i32, w as i32 * 4, wl_shm::Format::Argb8888)
        {
            Ok((buffer, canvas)) => {
                rgba_to_argb8888_le(pm.data(), canvas, 255);
                surface.damage_buffer(0, 0, w as i32, h as i32);
                if let Err(e) = buffer.attach_to(&surface) {
                    tracing::warn!("menu attach: {e}");
                }
            }
            Err(e) => {
                tracing::debug!("menu shm buffer: {e}");
                self.ctl.invalidate_menu();
                return;
            }
        }
        if let Ok(r) = Region::new(&self.compositor) {
            for rc in &region {
                r.add(rc.x, rc.y, rc.w, rc.h);
            }
            surface.set_input_region(Some(r.wl_region()));
        }
        surface.commit();
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
                    if !self.alpha_logged {
                        self.alpha_logged = true;
                        let st = alpha_stats(canvas, w, h);
                        tracing::info!(
                            size = ?(w, h),
                            clear = st.clear,
                            partial = st.partial,
                            opaque = st.opaque,
                            bad_premul = st.bad_premul,
                            corners = ?st.corners,
                            opacity = self.ctl.opacity,
                            output_scale = self.output_scale,
                            "transparency self-check of the first buffer (corners should be 0)"
                        );
                    }
                    surface.damage_buffer(0, 0, w as i32, h as i32);
                    if let Err(e) = buffer.attach_to(&surface) {
                        tracing::warn!("attach: {e}");
                    }
                    if self.surface_size != (w, h) {
                        self.layer.set_size(w, h);
                        self.surface_size = (w, h);
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
        // Never opaque (cheap; protects against any stale state).
        surface.set_opaque_region(None);
        self.layer.commit();
    }

    /// Closes the menu (no popup grab on the layer-surface fallback).
    fn dismiss_menu(&mut self, why: &str) {
        if self.menu.is_some() {
            tracing::info!(why, "menu dismissed");
            self.ctl.close_menu();
            self.sync_menu();
        }
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
        // Buffers stay at scale 1 (KWin upscales them on HiDPI outputs);
        // logged so a black box / offset clicks can be tied to the scale.
        // TODO(M6): render at `factor` × with set_buffer_scale for crisp HiDPI.
        self.output_scale = factor;
        tracing::info!(
            factor,
            "output scale for her surface (buffers stay at scale 1)"
        );
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
        layer: &LayerSurface,
        cfg: LayerSurfaceConfigure,
        _: u32,
    ) {
        // The fallback menu surface.
        if let Some(MenuSurface::Layer {
            layer: ml,
            configured,
        }) = self.menu.as_mut()
        {
            if ml.wl_surface() == layer.wl_surface() {
                tracing::info!(size = ?cfg.new_size, "menu layer surface configured");
                *configured = true;
                self.ctl.invalidate_menu();
                self.draw_menu();
                return;
            }
        }
        if layer.wl_surface() != self.layer.wl_surface() {
            return;
        }
        let want = self.surface_size;
        if let Some(got) = size_mismatch(want, cfg.new_size) {
            // A buffer smaller than the configured surface can leave the
            // rest undefined (black on some compositors): ask again.
            tracing::warn!(requested = ?want, configured = ?got, "layer configure size differs from her buffer; re-requesting");
            self.layer.set_size(want.0, want.1);
            self.layer.commit();
        }
        if !self.configured {
            tracing::info!(size = ?cfg.new_size, "layer surface configured");
            self.configured = true;
            let argb = self.shm.formats().contains(&wl_shm::Format::Argb8888);
            if argb {
                tracing::info!(formats = ?self.shm.formats(), "wl_shm formats");
            } else {
                tracing::warn!(formats = ?self.shm.formats(), "compositor did not advertise wl_shm ARGB8888; transparency may fail");
            }
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
        self.seat = Some(seat.clone());
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
            let on_menu = self
                .menu
                .as_ref()
                .is_some_and(|m| &ev.surface == m.wl_surface());
            if on_menu {
                match ev.kind {
                    PointerEventKind::Enter { .. } => {
                        self.set_cursor(CursorIcon::Default);
                        self.ctl.menu_pointer(Some(ev.position));
                    }
                    PointerEventKind::Motion { .. } => {
                        self.ctl.menu_pointer(Some(ev.position));
                    }
                    PointerEventKind::Leave { .. } => {
                        self.ctl.menu_pointer(None);
                    }
                    PointerEventKind::Press { button, .. }
                        if button == BTN_LEFT || button == BTN_RIGHT =>
                    {
                        let hit = self.ctl.menu_click(ev.position);
                        tracing::info!(?hit, pos = ?ev.position, "menu click");
                    }
                    _ => {}
                }
                continue;
            }
            if &ev.surface != self.layer.wl_surface() {
                tracing::debug!(kind = ?ev.kind, "pointer event on an unknown surface");
                continue;
            }
            if let PointerEventKind::Press { serial, button, .. } = ev.kind {
                self.last_press = Some((serial, ev.position));
                tracing::info!(
                    button = button_name(button),
                    code = format_args!("{button:#x}"),
                    pos = ?ev.position,
                    mask_hit = self.ctl.stage.hit(ev.position.0, ev.position.1),
                    serial,
                    "pointer press on her surface"
                );
            }
            if let PointerEventKind::Release { button, .. } = ev.kind {
                tracing::info!(
                    button = button_name(button),
                    pos = ?ev.position,
                    dragging = self.ctl.drag.dragging(),
                    "pointer release on her surface"
                );
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
                    // Delivered inside her input region: it is on her.
                    self.ctl.press_trusted(ev.position, true);
                    self.set_cursor(CursorIcon::Grabbing);
                }
                PointerEventKind::Press { button, .. }
                    if button == BTN_RIGHT || button == BTN_MIDDLE =>
                {
                    let b = if button == BTN_RIGHT {
                        Button::Right
                    } else {
                        Button::Middle
                    };
                    self.ctl.button_trusted(b);
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
                    let r = self.ctl.release();
                    tracing::info!(release = ?r, "click classification");
                    if !matches!(r, Release::None) {
                        self.set_cursor(CursorIcon::Grab);
                    }
                }
                _ => {}
            }
        }
        // Open / redraw / close the menu right away (not on the next tick).
        self.sync_menu();
        if self.ctl.quit_requested() {
            self.signal.stop();
        }
    }
}

/// Human-readable evdev button name for the logs.
fn button_name(code: u32) -> &'static str {
    match code {
        BTN_LEFT => "left",
        BTN_RIGHT => "right",
        BTN_MIDDLE => "middle",
        _ => "other",
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
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        // The layer-surface menu has no popup grab: losing focus (a click
        // elsewhere) dismisses it.
        let layer_menu =
            matches!(&self.menu, Some(m @ MenuSurface::Layer { .. }) if m.wl_surface() == surface);
        if layer_menu {
            self.dismiss_menu("menu lost keyboard focus");
        }
    }
    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        if self.ctl.menu().is_some() {
            let mk = match event.keysym {
                Keysym::Escape => Some(MenuKey::Escape),
                Keysym::Up => Some(MenuKey::Up),
                Keysym::Down => Some(MenuKey::Down),
                Keysym::Left => Some(MenuKey::Left),
                Keysym::Right => Some(MenuKey::Right),
                Keysym::Return | Keysym::KP_Enter | Keysym::space => Some(MenuKey::Enter),
                _ => None,
            };
            if let Some(k) = mk {
                let _ = self.ctl.menu_key(k);
                self.sync_menu();
                if self.ctl.quit_requested() {
                    self.signal.stop();
                }
                return;
            }
        }
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

impl PopupHandler for App {
    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        popup: &Popup,
        _: PopupConfigure,
    ) {
        if let Some(MenuSurface::Popup {
            popup: p,
            configured,
            opened,
        }) = self.menu.as_mut()
        {
            if p.wl_surface() == popup.wl_surface() {
                tracing::info!(
                    after_ms = opened.elapsed().as_millis() as u64,
                    "wayland menu popup configured"
                );
                *configured = true;
                self.ctl.invalidate_menu();
                self.draw_menu();
            }
        }
    }

    fn done(&mut self, _: &Connection, _: &QueueHandle<Self>, popup: &Popup) {
        // Dismissed by the compositor (click outside the grab).
        if self
            .menu
            .as_ref()
            .is_some_and(|m| m.wl_surface() == popup.wl_surface())
        {
            tracing::info!("menu popup dismissed by the compositor (popup_done)");
            self.menu = None;
            self.ctl.close_menu();
        }
    }
}

/// `delegate_xdg_shell!` needs it; we only create popups, never toplevels.
impl smithay_client_toolkit::shell::xdg::window::WindowHandler for App {
    fn request_close(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &smithay_client_toolkit::shell::xdg::window::Window,
    ) {
    }
    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &smithay_client_toolkit::shell::xdg::window::Window,
        _: smithay_client_toolkit::shell::xdg::window::WindowConfigure,
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
delegate_xdg_shell!(App);
delegate_xdg_popup!(App);

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}
