//! winit + softbuffer fallback for X11 and XWayland (`--backend x11`, or
//! automatically when there is no `WAYLAND_DISPLAY` / no layer-shell).
//!
//! - Undecorated, transparent (32-bit ARGB visual), `AlwaysOnTop`,
//!   `_NET_WM_WINDOW_TYPE_DOCK` (see [`crate::kde_effects`]: keeps KDE's
//!   Better Blur / Rounded Corners panel off her), skip taskbar/pager,
//!   all desktops.
//! - Click-through: the X Shape extension **input** shape is set to the
//!   frame's mask rectangles, so the X server itself routes clicks on
//!   transparent pixels to the window below (winit's
//!   `set_cursor_hittest` is all-or-nothing, so it is not used).
//! - Drag: global pointer position from `QueryPointer`, window moved with
//!   `set_outer_position`; position saved as `(left, bottom)` relative to
//!   the monitor so it matches the Wayland margins.
//! - Transparency has two modes, picked at startup and re-checked every
//!   [`CM_RECHECK`]:
//!   - **composited** (32-bit ARGB visual *and* a compositing manager owns
//!     `_NET_WM_CM_S<n>`, e.g. KWin or picom): softbuffer presents
//!     premultiplied `0xAARRGGBB`, transparent pixels are `0x00000000` and
//!     the compositor blends the soft edges;
//!   - **shaped** (no compositor running / compositing suspended, or no ARGB
//!     visual): the X server ignores the alpha byte and would paint every
//!     transparent pixel black, i.e. the "black box" around her. The X Shape
//!     **bounding** shape is then set to her silhouette (alpha ≥
//!     [`SHAPE_ALPHA`]) so nothing outside her is drawn at all, and the
//!     pixels are presented opaque and un-premultiplied (hard edge, no box).
//! - Right-click menu: a second, override-redirect `_NET_WM_WINDOW_TYPE_POPUP_MENU`
//!   window at the pointer showing [`crate::menu_popup`], with the same
//!   transparency mode. While it is open our own X connection grabs the
//!   pointer and keyboard (like any X11 menu): clicks inside pick rows,
//!   a click anywhere else or Esc closes it, arrows/Enter navigate.

use crate::drag::{Placement, Release};
use crate::input_region::mask_rects;
use crate::kde_effects::{self, WindowRole};
use crate::menu_popup::MenuKey;
use crate::pixels::{rgba_to_argb_u32, rgba_to_opaque_u32, SHAPE_ALPHA};
use crate::run::{period, Controller, OverlayError, OverlayOptions, KEEP_ON_SCREEN};
use lillith_core::interaction::Button;
use lillith_core::state::Key;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WKey, NamedKey};
use winit::platform::x11::{EventLoopBuilderExtX11, WindowAttributesExtX11, WindowType};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{CursorIcon, Window, WindowId, WindowLevel};
use x11rb::connection::{Connection as _, RequestConnection as _};
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{self, ClientMessageEvent, ConnectionExt as _, EventMask, Rectangle};
use x11rb::protocol::Event as XEvent;
use x11rb::rust_connection::RustConnection;

pub use softbuffer;
pub use winit;

/// How often the compositing-manager selection is re-checked (KWin can
/// suspend/resume compositing at runtime, e.g. Alt+Shift+F12).
pub const CM_RECHECK: Duration = Duration::from_secs(2);

/// How the window gets a transparent background (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlphaMode {
    /// ARGB visual + compositing manager: true per-pixel alpha.
    Composited,
    /// Bounding-shape silhouette: no compositor or no ARGB visual.
    Shaped,
}

impl AlphaMode {
    /// The mode for a window of `depth` bits with/without a compositor.
    pub fn pick(depth: u8, compositor: bool) -> Self {
        if depth == 32 && compositor {
            Self::Composited
        } else {
            Self::Shaped
        }
    }
}

/// Our own X connection for Shape/QueryPointer/EWMH on winit's window.
struct Xc {
    conn: RustConnection,
    root: u32,
    win: u32,
    shape: bool,
    /// `_NET_WM_CM_S<screen>`: owned while a compositing manager runs.
    cm_atom: Option<u32>,
}

impl Xc {
    fn new(win: u32) -> Result<Self, OverlayError> {
        let (conn, screen) =
            x11rb::connect(None).map_err(|e| OverlayError::X11(format!("connect: {e}")))?;
        let root = conn.setup().roots[screen].root;
        let shape = conn
            .extension_information(shape::X11_EXTENSION_NAME)
            .ok()
            .flatten()
            .is_some();
        if !shape {
            tracing::warn!("X server has no SHAPE extension: no click-through");
        }
        let cm_atom = conn
            .intern_atom(false, format!("_NET_WM_CM_S{screen}").as_bytes())
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.atom);
        Ok(Self {
            conn,
            root,
            win,
            shape,
            cm_atom,
        })
    }

    /// `true` while a compositing manager (KWin, picom…) is running.
    fn compositor(&self) -> bool {
        let Some(atom) = self.cm_atom else {
            return false;
        };
        self.conn
            .get_selection_owner(atom)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|r| r.owner != x11rb::NONE)
    }

    /// Bit depth and visual of the window winit created (32 = ARGB).
    fn depth_visual(&self) -> (u8, u32) {
        let depth = self
            .conn
            .get_geometry(self.win)
            .ok()
            .and_then(|c| c.reply().ok())
            .map_or(0, |g| g.depth);
        let visual = self
            .conn
            .get_window_attributes(self.win)
            .ok()
            .and_then(|c| c.reply().ok())
            .map_or(0, |a| a.visual);
        (depth, visual)
    }

    /// Bounding shape = these rectangles (`None` = the whole window).
    fn set_bounding(&self, rects: Option<&[crate::input_region::Rect]>) {
        self.set_bounding_of(self.win, rects);
    }

    /// Bounding shape of window `win` (hers or the menu's).
    fn set_bounding_of(&self, win: u32, rects: Option<&[crate::input_region::Rect]>) {
        if !self.shape {
            return;
        }
        match rects {
            None => {
                let _ = self.conn.shape_mask(
                    shape::SO::SET,
                    shape::SK::BOUNDING,
                    win,
                    0,
                    0,
                    x11rb::NONE,
                );
            }
            Some(rects) => {
                let rs = to_x_rects(rects);
                let _ = self.conn.shape_rectangles(
                    shape::SO::SET,
                    shape::SK::BOUNDING,
                    xproto::ClipOrdering::UNSORTED,
                    win,
                    0,
                    0,
                    &rs,
                );
            }
        }
        let _ = self.conn.flush();
    }

    /// Input shape = these rectangles (empty = fully click-through).
    fn set_input(&self, rects: &[crate::input_region::Rect]) {
        self.set_input_of(self.win, rects);
    }

    /// Input shape of window `win`.
    fn set_input_of(&self, win: u32, rects: &[crate::input_region::Rect]) {
        if !self.shape {
            return;
        }
        let rs = to_x_rects(rects);
        let _ = self.conn.shape_rectangles(
            shape::SO::SET,
            shape::SK::INPUT,
            xproto::ClipOrdering::UNSORTED,
            win,
            0,
            0,
            &rs,
        );
        let _ = self.conn.flush();
    }

    /// Grabs pointer + keyboard to `win` for the menu (events come to this
    /// connection, not winit). `true` once the pointer grab succeeded.
    fn grab(&self, win: u32) -> bool {
        let mask = EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION;
        let ok = self
            .conn
            .grab_pointer(
                false,
                win,
                mask,
                xproto::GrabMode::ASYNC,
                xproto::GrabMode::ASYNC,
                x11rb::NONE,
                x11rb::NONE,
                x11rb::CURRENT_TIME,
            )
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|r| r.status == xproto::GrabStatus::SUCCESS);
        if ok {
            let _ = self
                .conn
                .grab_keyboard(
                    false,
                    win,
                    x11rb::CURRENT_TIME,
                    xproto::GrabMode::ASYNC,
                    xproto::GrabMode::ASYNC,
                )
                .ok()
                .and_then(|c| c.reply().ok());
        }
        ok
    }

    fn ungrab(&self) {
        let _ = self.conn.ungrab_pointer(x11rb::CURRENT_TIME);
        let _ = self.conn.ungrab_keyboard(x11rb::CURRENT_TIME);
        let _ = self.conn.flush();
    }

    /// First keysym of `keycode` (menu navigation keys only need that).
    fn keysym(&self, keycode: u8) -> Option<u32> {
        let r = self
            .conn
            .get_keyboard_mapping(keycode, 1)
            .ok()?
            .reply()
            .ok()?;
        r.keysyms.first().copied()
    }

    fn pointer(&self) -> Option<(f64, f64)> {
        let r = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        Some((r.root_x as f64, r.root_y as f64))
    }

    fn atom(&self, name: &str) -> Option<u32> {
        Some(
            self.conn
                .intern_atom(false, name.as_bytes())
                .ok()?
                .reply()
                .ok()?
                .atom,
        )
    }

    /// EWMH: skip taskbar + pager, sticky on all desktops.
    fn ewmh_hints(&self) {
        let Some(state) = self.atom("_NET_WM_STATE") else {
            return;
        };
        for name in [
            "_NET_WM_STATE_SKIP_TASKBAR",
            "_NET_WM_STATE_SKIP_PAGER",
            "_NET_WM_STATE_STICKY",
        ] {
            if let Some(a) = self.atom(name) {
                let ev = ClientMessageEvent::new(32, self.win, state, [1, a, 0, 1, 0]);
                let _ = self.conn.send_event(
                    false,
                    self.root,
                    EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                    ev,
                );
            }
        }
        if let Some(desk) = self.atom("_NET_WM_DESKTOP") {
            let ev = ClientMessageEvent::new(32, self.win, desk, [0xFFFF_FFFF, 1, 0, 0, 0]);
            let _ = self.conn.send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                ev,
            );
        }
        let _ = self.conn.flush();
    }
}

fn to_x_rects(rects: &[crate::input_region::Rect]) -> Vec<Rectangle> {
    rects
        .iter()
        .map(|r| Rectangle {
            x: r.x as i16,
            y: r.y as i16,
            width: r.w as u16,
            height: r.h as u16,
        })
        .collect()
}

struct Live {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    xc: Xc,
    /// Monitor rect (x, y, w, h) her placement is relative to.
    monitor: (i32, i32, u32, u32),
    /// Window depth (32 = ARGB visual obtained).
    depth: u8,
    alpha: AlphaMode,
    next_cm_check: Instant,
}

/// The open right-click menu window.
struct Popup {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    xid: u32,
    /// Pointer + keyboard grabbed on our X connection.
    grabbed: bool,
    /// Give up grabbing after this (the menu still works by clicks on it).
    grab_deadline: Instant,
}

struct App {
    ctl: Controller,
    live: Option<Live>,
    popup: Option<Popup>,
    frame: Duration,
    next: Instant,
    last: Instant,
    started: Instant,
    exit_after: Option<Duration>,
    pixels_dirty: bool,
    mask_dirty: bool,
    cursor: (f64, f64),
    error: Option<OverlayError>,
}

/// Opens the window and runs until closed.
pub fn run(opts: &OverlayOptions) -> Result<(), OverlayError> {
    let ctl = Controller::new(opts)?;
    // Always X11: on a Wayland session this means XWayland, even when
    // WAYLAND_DISPLAY is set (winit would otherwise pick Wayland).
    let event_loop = EventLoop::builder()
        .with_x11()
        .build()
        .map_err(|e| OverlayError::X11(e.to_string()))?;
    let now = Instant::now();
    let mut app = App {
        ctl,
        live: None,
        popup: None,
        frame: period(opts.render_hz),
        next: now,
        last: now,
        started: now,
        exit_after: opts.exit_after,
        pixels_dirty: true,
        mask_dirty: true,
        cursor: (0.0, 0.0),
        error: None,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|e| OverlayError::X11(e.to_string()))?;
    app.ctl.log_stats();
    match app.error.take() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

impl App {
    fn create(&mut self, el: &ActiveEventLoop) -> Result<Live, OverlayError> {
        let (w, h) = self.ctl.stage.size();
        let mon = el
            .primary_monitor()
            .or_else(|| el.available_monitors().next());
        let monitor = mon
            .map(|m| {
                let p = m.position();
                let s = m.size();
                (p.x, p.y, s.width, s.height)
            })
            .unwrap_or((0, 0, 1920, 1080));
        let p = self
            .ctl
            .placement
            .clamped((w, h), (monitor.2, monitor.3), KEEP_ON_SCREEN);
        self.ctl.placement = p;
        self.ctl.brain.set_position(p.left as f32);
        self.ctl.set_screen((monitor.2, monitor.3));
        let (x, y) = p.to_xy(h, monitor.3);
        let role = WindowRole::from_env();
        kde_effects::log_startup(role);
        let attrs = Window::default_attributes()
            .with_title("Lillith")
            .with_name("lillith", "Lillith")
            .with_transparent(true)
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(if self.ctl.always_on_top {
                WindowLevel::AlwaysOnTop
            } else {
                WindowLevel::Normal
            })
            .with_x11_window_type(vec![x11_window_type(role)])
            .with_inner_size(PhysicalSize::new(w, h))
            .with_position(PhysicalPosition::new(monitor.0 + x, monitor.1 + y));
        let window = Rc::new(
            el.create_window(attrs)
                .map_err(|e| OverlayError::X11(format!("create window: {e}")))?,
        );
        let xid = match window.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Xlib(h)) => h.window as u32,
            Ok(RawWindowHandle::Xcb(h)) => h.window.get(),
            Ok(other) => {
                return Err(OverlayError::X11(format!(
                    "not an X11 window ({other:?}); run under X11/XWayland or use --backend wayland"
                )))
            }
            Err(e) => return Err(OverlayError::X11(e.to_string())),
        };
        let xc = Xc::new(xid)?;
        xc.set_input(&[]);
        xc.ewmh_hints();
        let ctx = softbuffer::Context::new(window.clone())
            .map_err(|e| OverlayError::X11(format!("softbuffer: {e}")))?;
        let mut surface = softbuffer::Surface::new(&ctx, window.clone())
            .map_err(|e| OverlayError::X11(format!("softbuffer: {e}")))?;
        surface
            .resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap())
            .map_err(|e| OverlayError::X11(format!("softbuffer resize: {e}")))?;
        window.set_cursor(CursorIcon::Grab);
        let (depth, visual) = xc.depth_visual();
        let compositor = xc.compositor();
        let alpha = AlphaMode::pick(depth, compositor);
        if alpha == AlphaMode::Shaped {
            // Nothing drawn until the first frame's silhouette is known.
            xc.set_bounding(Some(&[]));
        }
        tracing::info!(
            size = ?(w, h),
            left = p.left,
            bottom = p.bottom,
            ?monitor,
            xid,
            "x11 overlay window (always-on-top, input-shape click-through)"
        );
        log_alpha(depth, visual, compositor, alpha, xc.shape);
        Ok(Live {
            window,
            surface,
            xc,
            monitor,
            depth,
            alpha,
            next_cm_check: Instant::now() + CM_RECHECK,
        })
    }

    /// Re-checks the compositor and switches between composited / shaped.
    fn check_compositor(&mut self, now: Instant) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        if now < live.next_cm_check {
            return;
        }
        live.next_cm_check = now + CM_RECHECK;
        let compositor = live.xc.compositor();
        let alpha = AlphaMode::pick(live.depth, compositor);
        if alpha != live.alpha {
            live.alpha = alpha;
            if alpha == AlphaMode::Composited {
                live.xc.set_bounding(None);
            }
            tracing::info!(
                compositor,
                ?alpha,
                "x11 compositing changed; transparency mode switched"
            );
            self.pixels_dirty = true;
        }
    }

    fn draw(&mut self) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        if self.pixels_dirty {
            let canvas = self.ctl.stage.canvas();
            if live.alpha == AlphaMode::Shaped {
                // Clip the window to her silhouette before the pixels land,
                // so no transparent (= black without a compositor) pixel shows.
                let rects = mask_rects(
                    canvas.data(),
                    canvas.width(),
                    canvas.height(),
                    SHAPE_ALPHA,
                    1,
                );
                live.xc.set_bounding(Some(&rects));
            }
            match live.surface.buffer_mut() {
                Ok(mut buf) => {
                    match live.alpha {
                        AlphaMode::Composited => {
                            rgba_to_argb_u32(canvas.data(), &mut buf, self.ctl.opacity)
                        }
                        AlphaMode::Shaped => {
                            rgba_to_opaque_u32(canvas.data(), &mut buf, SHAPE_ALPHA)
                        }
                    }
                    if let Err(e) = buf.present() {
                        tracing::warn!("present: {e}");
                    }
                    self.pixels_dirty = false;
                }
                Err(e) => tracing::warn!("buffer: {e}"),
            }
        }
        if self.mask_dirty {
            live.xc.set_input(self.ctl.stage.mask());
            self.mask_dirty = false;
        }
    }

    fn move_to(&mut self, p: Placement) {
        let Some(live) = self.live.as_ref() else {
            return;
        };
        let (w, h) = self.ctl.stage.size();
        let (mx, my, mw, mh) = live.monitor;
        let p = p.clamped((w, h), (mw, mh), KEEP_ON_SCREEN);
        let (x, y) = p.to_xy(h, mh);
        live.window
            .set_outer_position(PhysicalPosition::new(mx + x, my + y));
        self.ctl.placement = p;
    }

    /// Resizes her window when the stage changed size (Resize, peek,
    /// speech area); the bottom-left stays put.
    fn apply_resize(&mut self) {
        if !self.ctl.take_resized() {
            return;
        }
        let (w, h) = self.ctl.stage.size();
        if let Some(live) = self.live.as_mut() {
            let _ = live.window.request_inner_size(PhysicalSize::new(w, h));
            if let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) {
                if let Err(e) = live.surface.resize(nw, nh) {
                    tracing::warn!("softbuffer resize: {e}");
                }
            }
        }
        if let Err(e) = self.ctl.stage.render() {
            tracing::warn!("render after resize: {e}");
        }
        // Bottom-based placement: the feet stay put (peek moves too).
        let p = self.ctl.take_move().unwrap_or(self.ctl.placement);
        self.move_to(p);
        self.pixels_dirty = true;
        self.mask_dirty = true;
        tracing::debug!(size = ?(w, h), "overlay resized");
    }

    /// Shows / hides / redraws the menu window to match the controller.
    fn sync_menu(&mut self, el: &ActiveEventLoop) {
        if self.ctl.take_menu_opened() {
            self.close_popup();
            match self.open_popup(el) {
                Ok(p) => self.popup = Some(p),
                Err(e) => {
                    tracing::warn!("menu popup: {e}");
                    self.ctl.close_menu();
                }
            }
        }
        if self.ctl.menu().is_none() {
            self.close_popup();
            return;
        }
        self.pump_popup_events();
        if self.ctl.menu().is_none() {
            self.close_popup();
            return;
        }
        self.draw_popup();
    }

    fn open_popup(&mut self, el: &ActiveEventLoop) -> Result<Popup, OverlayError> {
        let live = self
            .live
            .as_ref()
            .ok_or_else(|| OverlayError::X11("no overlay window yet".into()))?;
        let (w, h) = self.ctl.menu().map(|m| m.size()).unwrap_or((1, 1));
        let (mx, my, mw, mh) = live.monitor;
        let (px, py) = live.xc.pointer().unwrap_or((mx as f64, my as f64));
        let (px, py) = (px as i32, py as i32);
        // At the pointer, shifted so every expansion stays on the monitor.
        let x = px.min(mx + mw as i32 - w as i32).max(mx);
        let y = py.min(my + mh as i32 - h as i32).max(my);
        let attrs = Window::default_attributes()
            .with_title("Lillith menu")
            .with_name("lillith", "Lillith")
            .with_transparent(true)
            .with_decorations(false)
            .with_resizable(false)
            .with_override_redirect(true)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_x11_window_type(vec![WindowType::PopupMenu])
            .with_inner_size(PhysicalSize::new(w, h))
            .with_position(PhysicalPosition::new(x, y));
        let window = Rc::new(
            el.create_window(attrs)
                .map_err(|e| OverlayError::X11(format!("create menu window: {e}")))?,
        );
        let xid = match window.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Xlib(h)) => h.window as u32,
            Ok(RawWindowHandle::Xcb(h)) => h.window.get(),
            _ => return Err(OverlayError::X11("menu: not an X11 window".into())),
        };
        let ctx = softbuffer::Context::new(window.clone())
            .map_err(|e| OverlayError::X11(format!("softbuffer: {e}")))?;
        let mut surface = softbuffer::Surface::new(&ctx, window.clone())
            .map_err(|e| OverlayError::X11(format!("softbuffer: {e}")))?;
        surface
            .resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap())
            .map_err(|e| OverlayError::X11(format!("softbuffer resize: {e}")))?;
        if live.alpha == AlphaMode::Shaped {
            live.xc.set_bounding_of(xid, Some(&[]));
        }
        tracing::info!(x, y, size = ?(w, h), xid, "x11 menu popup (override-redirect)");
        Ok(Popup {
            window,
            surface,
            xid,
            grabbed: false,
            grab_deadline: Instant::now() + Duration::from_secs(1),
        })
    }

    fn close_popup(&mut self) {
        if let Some(p) = self.popup.take() {
            if p.grabbed {
                if let Some(l) = &self.live {
                    l.xc.ungrab();
                }
            }
            drop(p);
        }
    }

    /// Pointer / key events our grab delivered to our own X connection.
    fn pump_popup_events(&mut self) {
        let (Some(live), Some(popup)) = (self.live.as_ref(), self.popup.as_mut()) else {
            return;
        };
        if !popup.grabbed && Instant::now() < popup.grab_deadline {
            popup.grabbed = live.xc.grab(popup.xid);
            if popup.grabbed {
                tracing::debug!("menu grabbed pointer + keyboard");
            }
        }
        let mut evs = Vec::new();
        while let Ok(Some(ev)) = live.xc.conn.poll_for_event() {
            evs.push(ev);
        }
        for ev in evs {
            match ev {
                XEvent::MotionNotify(m) if m.event == popup.xid => {
                    self.ctl
                        .menu_pointer(Some((m.event_x as f64, m.event_y as f64)));
                }
                XEvent::ButtonPress(b) if b.event == popup.xid => {
                    // Wheel "buttons" 4-7 are ignored.
                    if (1..=3).contains(&b.detail) {
                        let hit = self.ctl.menu_click((b.event_x as f64, b.event_y as f64));
                        tracing::debug!(?hit, x = b.event_x, y = b.event_y, "menu click");
                    }
                }
                XEvent::KeyPress(k) => {
                    let key = match live.xc.keysym(k.detail) {
                        Some(0xff1b) => Some(MenuKey::Escape),
                        Some(0xff52) => Some(MenuKey::Up),
                        Some(0xff54) => Some(MenuKey::Down),
                        Some(0xff51) => Some(MenuKey::Left),
                        Some(0xff53) => Some(MenuKey::Right),
                        Some(0xff0d | 0xff8d | 0x20) => Some(MenuKey::Enter),
                        _ => None,
                    };
                    if let Some(k) = key {
                        let _ = self.ctl.menu_key(k);
                    }
                }
                _ => {}
            }
            if self.ctl.menu().is_none() {
                break;
            }
        }
    }

    fn draw_popup(&mut self) {
        let (Some(live), Some(popup)) = (self.live.as_ref(), self.popup.as_mut()) else {
            return;
        };
        let alpha = live.alpha;
        let Some(pm) = self.ctl.render_menu() else {
            return;
        };
        let (data, w, h) = (pm.data().to_vec(), pm.width(), pm.height());
        let menu = self.ctl.menu().expect("menu open");
        let content = menu.region();
        if alpha == AlphaMode::Shaped {
            let rects = mask_rects(&data, w, h, SHAPE_ALPHA, 1);
            live.xc.set_bounding_of(popup.xid, Some(&rects));
        } else {
            live.xc.set_bounding_of(popup.xid, None);
        }
        live.xc.set_input_of(popup.xid, &content);
        match popup.surface.buffer_mut() {
            Ok(mut buf) => {
                match alpha {
                    AlphaMode::Composited => rgba_to_argb_u32(&data, &mut buf, 255),
                    AlphaMode::Shaped => rgba_to_opaque_u32(&data, &mut buf, SHAPE_ALPHA),
                }
                if let Err(e) = buf.present() {
                    tracing::warn!("menu present: {e}");
                }
            }
            Err(e) => tracing::warn!("menu buffer: {e}"),
        }
    }

    fn fail(&mut self, el: &ActiveEventLoop, e: OverlayError) {
        self.error = Some(e);
        el.exit();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.live.is_some() {
            return;
        }
        if let Err(e) = self.ctl.stage.render() {
            return self.fail(el, e.into());
        }
        match self.create(el) {
            Ok(live) => {
                self.live = Some(live);
                self.pixels_dirty = true;
                self.mask_dirty = true;
                self.draw();
            }
            Err(e) => self.fail(el, e),
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.popup.as_ref().is_some_and(|p| p.window.id() == id) {
            // The menu's input arrives through our grab (pump_popup_events);
            // winit only tells us when it needs repainting.
            if matches!(event, WindowEvent::RedrawRequested) {
                self.ctl.invalidate_menu();
                self.draw_popup();
            }
            return;
        }
        self.window_event_main(el, event);
        self.sync_menu(el);
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.about_to_wait_main(el);
        if self.ctl.quit_requested() {
            el.exit();
            return;
        }
        self.sync_menu(el);
        self.apply_resize();
        if self.pixels_dirty || self.mask_dirty {
            self.draw();
        }
        if self.popup.is_some() {
            // Grabbed menu events arrive on our own connection, which does
            // not wake winit: poll it at ~100 Hz while the menu is open.
            let soon = Instant::now() + Duration::from_millis(10);
            el.set_control_flow(ControlFlow::WaitUntil(self.next.min(soon)));
        }
    }
}

impl App {
    fn window_event_main(&mut self, el: &ActiveEventLoop, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::RedrawRequested => {
                self.pixels_dirty = true;
                self.draw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                if self.ctl.drag.active() {
                    let at = self.live.as_ref().and_then(|l| l.xc.pointer());
                    if let Some(p) = at.and_then(|g| self.ctl.motion(g)) {
                        self.move_to(p);
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                // Press point in root coords from the event's own window
                // position (a QueryPointer here would race the pointer).
                let global = self.live.as_ref().map(|l| {
                    let h = self.ctl.stage.size().1;
                    let (x, y) = self.ctl.placement.to_xy(h, l.monitor.3);
                    (
                        (l.monitor.0 + x) as f64 + self.cursor.0,
                        (l.monitor.1 + y) as f64 + self.cursor.1,
                    )
                });
                if let Some(g) = global {
                    if self.ctl.press(g, self.cursor, false) {
                        if let Some(l) = &self.live {
                            l.window.set_cursor(CursorIcon::Grabbing);
                        }
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                // Apply the final pointer position (motion events coalesce).
                if self.ctl.drag.active() {
                    let at = self.live.as_ref().and_then(|l| l.xc.pointer());
                    if let Some(p) = at.and_then(|g| self.ctl.motion(g)) {
                        self.move_to(p);
                    }
                }
                if !matches!(self.ctl.release(), Release::None) {
                    if let Some(l) = &self.live {
                        l.window.set_cursor(CursorIcon::Grab);
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: b @ (MouseButton::Right | MouseButton::Middle),
                ..
            } => {
                let b = if b == MouseButton::Right {
                    Button::Right
                } else {
                    Button::Middle
                };
                self.ctl.button(b, self.cursor);
            }
            WindowEvent::CursorEntered { .. } => self.ctl.hover(true),
            WindowEvent::CursorLeft { .. } => {
                if !self.ctl.drag.active() {
                    self.ctl.hover(false);
                }
            }
            // Synthetic presses are winit replaying held keys on focus-in.
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } if event.state == ElementState::Pressed && !event.repeat => {
                tracing::debug!(key = ?event.logical_key, "key");
                let key = match &event.logical_key {
                    WKey::Named(NamedKey::Escape) => Some(Key::Escape),
                    WKey::Named(NamedKey::Enter) => Some(Key::Enter),
                    WKey::Character(s) => s.chars().next().map(Key::Char),
                    _ => None,
                };
                if let Some(k) = key {
                    self.ctl.key(k);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait_main(&mut self, el: &ActiveEventLoop) {
        if self.ctl.quit_requested() {
            el.exit();
            return;
        }
        let now = Instant::now();
        self.check_compositor(now);
        if let Some(limit) = self.exit_after {
            if now - self.started >= limit {
                el.exit();
                return;
            }
        }
        if now >= self.next {
            let dt = (now - self.last).as_secs_f32().min(0.25);
            self.last = now;
            match self.ctl.tick(dt) {
                Ok(c) => {
                    self.pixels_dirty |= c.pixels;
                    self.mask_dirty |= c.mask;
                }
                Err(e) => return self.fail(el, e),
            }
            self.apply_resize();
            if let Some(on_top) = self.ctl.take_level() {
                if let Some(l) = &self.live {
                    l.window.set_window_level(if on_top {
                        WindowLevel::AlwaysOnTop
                    } else {
                        WindowLevel::Normal
                    });
                }
            }
            self.frame = period(self.ctl.render_hz());
            if let Some(p) = self.ctl.take_move() {
                if !self.ctl.drag.active() {
                    self.move_to(p);
                }
            }
            if self.pixels_dirty || self.mask_dirty {
                self.draw();
            }
            // Skip missed frames instead of bursting to catch up.
            self.next += self.frame;
            if self.next < now {
                self.next = now + self.frame;
            }
        }
        el.set_control_flow(ControlFlow::WaitUntil(self.next));
    }
}

/// `_NET_WM_WINDOW_TYPE` for her window: `DOCK` by default, which KDE's
/// Better Blur (force blur) and Rounded Corners effects skip.
pub fn x11_window_type(role: WindowRole) -> WindowType {
    match role {
        WindowRole::Dock => WindowType::Dock,
        WindowRole::Utility | WindowRole::Normal => WindowType::Utility,
    }
}

/// One startup line saying which transparency path runs (for bug reports).
fn log_alpha(depth: u8, visual: u32, compositor: bool, alpha: AlphaMode, shape: bool) {
    let argb = depth == 32;
    match alpha {
        AlphaMode::Composited => tracing::info!(
            backend = "x11",
            argb_visual = argb,
            depth,
            visual = format_args!("{visual:#x}"),
            compositor,
            ?alpha,
            "transparency: 32-bit ARGB visual + compositing manager (per-pixel alpha)"
        ),
        AlphaMode::Shaped => tracing::warn!(
            backend = "x11",
            argb_visual = argb,
            depth,
            visual = format_args!("{visual:#x}"),
            compositor,
            ?alpha,
            shape_ext = shape,
            "transparency: {} -> clipping the window to her silhouette with the X Shape \
             bounding mask (hard edges). Enable compositing (KWin: Alt+Shift+F12, or \
             System Settings > Display > Compositor) or use the Wayland backend for soft edges",
            if argb {
                "no compositing manager running"
            } else {
                "no 32-bit ARGB visual"
            }
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dock_type_by_default() {
        assert_eq!(x11_window_type(WindowRole::default()), WindowType::Dock);
        assert_eq!(x11_window_type(WindowRole::Utility), WindowType::Utility);
    }

    #[test]
    fn alpha_mode_needs_argb_and_a_compositor() {
        assert_eq!(AlphaMode::pick(32, true), AlphaMode::Composited);
        assert_eq!(AlphaMode::pick(32, false), AlphaMode::Shaped);
        assert_eq!(AlphaMode::pick(24, true), AlphaMode::Shaped);
        assert_eq!(AlphaMode::pick(24, false), AlphaMode::Shaped);
    }
}
