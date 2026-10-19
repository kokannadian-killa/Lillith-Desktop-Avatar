//! winit + softbuffer fallback for X11 and XWayland (`--backend x11`, or
//! automatically when there is no `WAYLAND_DISPLAY` / no layer-shell).
//!
//! - Undecorated, transparent (32-bit ARGB visual), `AlwaysOnTop`,
//!   `_NET_WM_WINDOW_TYPE_UTILITY`, skip taskbar/pager, all desktops.
//! - Click-through: the X Shape extension **input** shape is set to the
//!   frame's mask rectangles, so the X server itself routes clicks on
//!   transparent pixels to the window below (winit's
//!   `set_cursor_hittest` is all-or-nothing, so it is not used).
//! - Drag: global pointer position from `QueryPointer`, window moved with
//!   `set_outer_position`; position saved as `(left, bottom)` relative to
//!   the monitor so it matches the Wayland margins.
//! - Softbuffer presents premultiplied `0xAARRGGBB`; a compositing manager
//!   (KWin, picom…) is needed for real transparency, otherwise the
//!   background shows black.

use crate::drag::{Placement, Release};
use crate::pixels::rgba_to_argb_u32;
use crate::run::{period, Controller, OverlayError, OverlayOptions, KEEP_ON_SCREEN};
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::platform::x11::{EventLoopBuilderExtX11, WindowAttributesExtX11, WindowType};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{CursorIcon, Window, WindowId, WindowLevel};
use x11rb::connection::{Connection as _, RequestConnection as _};
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{self, ClientMessageEvent, ConnectionExt as _, EventMask, Rectangle};
use x11rb::rust_connection::RustConnection;

pub use softbuffer;
pub use winit;

/// Our own X connection for Shape/QueryPointer/EWMH on winit's window.
struct Xc {
    conn: RustConnection,
    root: u32,
    win: u32,
    shape: bool,
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
        Ok(Self {
            conn,
            root,
            win,
            shape,
        })
    }

    /// Input shape = these rectangles (empty = fully click-through).
    fn set_input(&self, rects: &[crate::input_region::Rect]) {
        if !self.shape {
            return;
        }
        let rs: Vec<Rectangle> = rects
            .iter()
            .map(|r| Rectangle {
                x: r.x as i16,
                y: r.y as i16,
                width: r.w as u16,
                height: r.h as u16,
            })
            .collect();
        let _ = self.conn.shape_rectangles(
            shape::SO::SET,
            shape::SK::INPUT,
            xproto::ClipOrdering::UNSORTED,
            self.win,
            0,
            0,
            &rs,
        );
        let _ = self.conn.flush();
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

struct Live {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    xc: Xc,
    /// Monitor rect (x, y, w, h) her placement is relative to.
    monitor: (i32, i32, u32, u32),
}

struct App {
    ctl: Controller,
    live: Option<Live>,
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
        let (x, y) = p.to_xy(h, monitor.3);
        let attrs = Window::default_attributes()
            .with_title("Lillith")
            .with_name("lillith", "Lillith")
            .with_transparent(true)
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_x11_window_type(vec![WindowType::Utility])
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
        tracing::info!(
            size = ?(w, h),
            left = p.left,
            bottom = p.bottom,
            ?monitor,
            xid,
            "x11 overlay window (always-on-top, input-shape click-through)"
        );
        Ok(Live {
            window,
            surface,
            xc,
            monitor,
        })
    }

    fn draw(&mut self) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        if self.pixels_dirty {
            match live.surface.buffer_mut() {
                Ok(mut buf) => {
                    rgba_to_argb_u32(self.ctl.stage.canvas().data(), &mut buf, self.ctl.opacity);
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

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
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
                button: MouseButton::Right,
                ..
            } if self.ctl.right_click_quits && self.ctl.stage.hit(self.cursor.0, self.cursor.1) => {
                tracing::info!("right-click: quitting");
                el.exit();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        let now = Instant::now();
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
