//! Native Windows backend (Windows 10/11), plain Win32, no winit.
//!
//! - Her window is a `WS_POPUP` with `WS_EX_LAYERED | WS_EX_TOOLWINDOW`
//!   (no taskbar button) and `WS_EX_TOPMOST` when `always_on_top`. Every
//!   frame goes to the screen with `UpdateLayeredWindow` from a 32-bit
//!   premultiplied BGRA DIB, so DWM blends her soft edges and fully
//!   transparent pixels are click-through by themselves.
//! - Click-through outside the input mask (e.g. `click_through` = head
//!   only): each frame the cursor is tested against [`Stage::mask`] and
//!   `WS_EX_TRANSPARENT` is toggled, so clicks there reach the window below.
//! - Drag: `SetCapture` + `GetCursorPos`; placement is `(left, bottom)` on
//!   the primary monitor, like the other backends.
//! - Gravity: the floor is the work area (`SPI_GETWORKAREA`, i.e. above the
//!   taskbar), and the window stack comes from `EnumWindows` + DWM frame
//!   bounds ([`crate::win_desktop`]), written into
//!   [`lillith_core::desktop::shared`] every [`STACK_POLL`] like the KWin
//!   bridge would.
//! - Right-click menu: a second layered top-most window at the pointer
//!   showing [`crate::menu_popup`]; it takes the keyboard (arrows / Enter /
//!   Esc) and closes when it loses activation (a click anywhere else).
//! - Per-monitor DPI aware (v2): coordinates are physical pixels.
//!
//! Window messages only enqueue [`Ev`]s; all logic runs in the loop
//! below, so nothing re-enters the [`Controller`] from inside a Win32 call.

use crate::drag::{Placement, Release};
use crate::menu_popup::MenuKey;
use crate::pixels::rgba_to_argb_u32;
use crate::run::{period, Controller, OverlayError, OverlayOptions, KEEP_ON_SCREEN};
use crate::win_desktop::{self, RawWindow, LILLITH_CLASS, LILLITH_MENU_CLASS};
use lillith_core::desktop::Rect as DRect;
use lillith_core::interaction::Button;
use lillith_core::state::Key;
use std::cell::RefCell;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
    HBITMAP, HDC, HGDIOBJ,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_DOWN, VK_ESCAPE,
    VK_LEFT, VK_RETURN, VK_RIGHT, VK_SPACE, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// How often the window stack is re-read for perching.
pub const STACK_POLL: Duration = Duration::from_millis(250);

/// Which of our windows a message was for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Win {
    Main,
    Menu,
}

/// Input from the window procedure (client coordinates).
#[derive(Debug, Clone, Copy)]
enum Ev {
    Move(f64, f64),
    LeftDown(f64, f64),
    LeftUp,
    Other(Button, f64, f64),
    Leave,
    KeyDown(u16),
    Char(char),
    Inactive,
    Close,
}

thread_local! {
    static QUEUE: RefCell<Vec<(HWND, Ev)>> = const { RefCell::new(Vec::new()) };
}

fn push(hwnd: HWND, ev: Ev) {
    QUEUE.with(|q| q.borrow_mut().push((hwnd, ev)));
}

fn xy(lp: LPARAM) -> (f64, f64) {
    let x = (lp & 0xffff) as u16 as i16;
    let y = ((lp >> 16) & 0xffff) as u16 as i16;
    (x as f64, y as f64)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let (x, y) = xy(lp);
            push(hwnd, Ev::Move(x, y));
            0
        }
        WM_LBUTTONDOWN => {
            let (x, y) = xy(lp);
            push(hwnd, Ev::LeftDown(x, y));
            0
        }
        WM_LBUTTONUP => {
            push(hwnd, Ev::LeftUp);
            0
        }
        WM_RBUTTONDOWN => {
            let (x, y) = xy(lp);
            push(hwnd, Ev::Other(Button::Right, x, y));
            0
        }
        WM_MBUTTONDOWN => {
            let (x, y) = xy(lp);
            push(hwnd, Ev::Other(Button::Middle, x, y));
            0
        }
        WM_MOUSELEAVE => {
            push(hwnd, Ev::Leave);
            0
        }
        WM_KEYDOWN | WM_SYSKEYDOWN if lp & (1 << 30) == 0 => {
            push(hwnd, Ev::KeyDown(wp as u16));
            0
        }
        WM_CHAR => {
            if let Some(c) = char::from_u32(wp as u32).filter(|c| !c.is_control()) {
                push(hwnd, Ev::Char(c));
            }
            0
        }
        WM_ACTIVATE => {
            if (wp & 0xffff) as u32 == WA_INACTIVE {
                push(hwnd, Ev::Inactive);
            }
            0
        }
        WM_CLOSE => {
            push(hwnd, Ev::Close);
            0
        }
        // The menu must not steal focus back from a click elsewhere, but it
        // does take the keyboard when shown (SetForegroundWindow).
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A top-down 32-bit DIB selected into a memory DC.
struct Dib {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    w: u32,
    h: u32,
}

impl Dib {
    fn new(w: u32, h: u32) -> Result<Self, OverlayError> {
        let (w, h) = (w.max(1), h.max(1));
        // SAFETY: standard GDI calls; every handle is checked and released in Drop.
        unsafe {
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            if dc.is_null() {
                return Err(OverlayError::Win32("CreateCompatibleDC failed".into()));
            }
            let mut bi: BITMAPINFO = std::mem::zeroed();
            bi.bmiHeader = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32), // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            if bmp.is_null() || bits.is_null() {
                DeleteDC(dc);
                return Err(OverlayError::Win32("CreateDIBSection failed".into()));
            }
            let old = SelectObject(dc, bmp);
            Ok(Self {
                dc,
                bmp,
                old,
                bits: bits as *mut u32,
                w,
                h,
            })
        }
    }

    fn pixels(&mut self) -> &mut [u32] {
        // SAFETY: the DIB holds exactly w*h u32 pixels and lives as long as self.
        unsafe {
            GdiFlush();
            std::slice::from_raw_parts_mut(self.bits, (self.w * self.h) as usize)
        }
    }

    /// Shows the DIB on the layered `hwnd` at `pos` (screen px).
    fn present(&self, hwnd: HWND, pos: (i32, i32)) -> bool {
        let dst = POINT { x: pos.0, y: pos.1 };
        let size = SIZE {
            cx: self.w as i32,
            cy: self.h as i32,
        };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        // SAFETY: valid window, DC and stack structs for the call's duration.
        unsafe {
            UpdateLayeredWindow(
                hwnd,
                std::ptr::null_mut(),
                &dst,
                &size,
                self.dc,
                &src,
                0,
                &blend,
                ULW_ALPHA,
            ) != 0
        }
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        // SAFETY: handles created in `new`, released once.
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bmp);
            DeleteDC(self.dc);
        }
    }
}

fn register_class(name: &str, cursor: *const u16) -> Result<Vec<u16>, OverlayError> {
    let cls = wide(name);
    // SAFETY: class struct filled per the docs; the name outlives the class.
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(std::ptr::null()),
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), cursor),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: cls.as_ptr(),
            hIconSm: std::ptr::null_mut(),
        };
        if RegisterClassExW(&wc) == 0 {
            return Err(OverlayError::Win32(format!(
                "RegisterClassExW({name}) failed"
            )));
        }
    }
    Ok(cls)
}

fn create_window(
    cls: &[u16],
    title: &str,
    ex: u32,
    pos: (i32, i32),
    size: (u32, u32),
) -> Result<HWND, OverlayError> {
    let title = wide(title);
    // SAFETY: registered class, valid strings.
    let hwnd = unsafe {
        CreateWindowExW(
            ex,
            cls.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            pos.0,
            pos.1,
            size.0 as i32,
            size.1 as i32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        return Err(OverlayError::Win32(format!(
            "CreateWindowExW({}) failed",
            String::from_utf16_lossy(&title)
        )));
    }
    Ok(hwnd)
}

fn cursor_pos() -> Option<(i32, i32)> {
    let mut p = POINT { x: 0, y: 0 };
    // SAFETY: out-param.
    (unsafe { GetCursorPos(&mut p) } != 0).then_some((p.x, p.y))
}

/// Primary screen and its work area (taskbar excluded), screen px.
pub fn screen_and_work_area() -> (DRect, DRect) {
    // SAFETY: plain metric queries / out-param.
    unsafe {
        let screen = DRect::new(
            0,
            0,
            GetSystemMetrics(SM_CXSCREEN).max(1),
            GetSystemMetrics(SM_CYSCREEN).max(1),
        );
        let mut r: RECT = std::mem::zeroed();
        let ok = SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut r as *mut RECT as *mut _, 0) != 0;
        let work = if ok && r.right > r.left && r.bottom > r.top {
            DRect::new(r.left, r.top, r.right - r.left, r.bottom - r.top)
        } else {
            screen
        };
        (screen, work)
    }
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lp: LPARAM) -> BOOL {
    let v = &mut *(lp as *mut Vec<HWND>);
    v.push(hwnd);
    1
}

/// Every top-level window, top → bottom, as [`RawWindow`]s.
fn enum_windows() -> Vec<RawWindow> {
    let mut hwnds: Vec<HWND> = Vec::new();
    // SAFETY: the callback only pushes into the Vec passed as LPARAM,
    // which outlives the synchronous EnumWindows call.
    unsafe {
        EnumWindows(Some(enum_proc), &mut hwnds as *mut Vec<HWND> as LPARAM);
    }
    hwnds
        .into_iter()
        .map(|h| unsafe {
            // SAFETY: read-only queries on a window handle (a window that
            // vanished meanwhile just returns failures / zeros).
            let mut buf = [0u16; 128];
            let n = GetClassNameW(h, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
            let class = String::from_utf16_lossy(&buf[..n]);
            let mut cloaked: u32 = 0;
            let _ = DwmGetWindowAttribute(
                h,
                DWMWA_CLOAKED as u32,
                &mut cloaked as *mut u32 as *mut _,
                4,
            );
            let mut r: RECT = std::mem::zeroed();
            if DwmGetWindowAttribute(
                h,
                DWMWA_EXTENDED_FRAME_BOUNDS as u32,
                &mut r as *mut RECT as *mut _,
                std::mem::size_of::<RECT>() as u32,
            ) != 0
            {
                GetWindowRect(h, &mut r);
            }
            RawWindow {
                hwnd: h as usize,
                class,
                title_len: GetWindowTextLengthW(h).max(0) as usize,
                ex_style: GetWindowLongPtrW(h, GWL_EXSTYLE) as u32,
                owned: !GetWindow(h, GW_OWNER).is_null(),
                visible: IsWindowVisible(h) != 0,
                minimized: IsIconic(h) != 0,
                cloaked: cloaked != 0,
                rect: DRect::new(r.left, r.top, r.right - r.left, r.bottom - r.top),
            }
        })
        .collect()
}

/// Refreshes the shared desktop model (what the KWin bridge does on Linux).
fn poll_desktop() {
    let (screen, work) = screen_and_work_area();
    let windows = win_desktop::build(&enum_windows(), Some(screen));
    if let Ok(mut d) = lillith_core::desktop::shared().lock() {
        if d.screen != Some(screen) || d.work_area != Some(work) {
            d.set_area(screen, work);
        }
        if d.windows != windows || d.generation == 0 {
            d.set_windows(windows);
        }
    }
}

struct Menu {
    hwnd: HWND,
    dib: Dib,
    pos: (i32, i32),
}

struct App {
    ctl: Controller,
    hwnd: HWND,
    dib: Dib,
    menu_cls: Vec<u16>,
    menu: Option<Menu>,
    /// Primary monitor (x, y, w, h).
    monitor: (i32, i32, u32, u32),
    pos: (i32, i32),
    frame: Duration,
    next: Instant,
    last: Instant,
    started: Instant,
    exit_after: Option<Duration>,
    pixels_dirty: bool,
    tracking: bool,
    hovered: bool,
    /// `WS_EX_TRANSPARENT` currently set (pointer outside the mask).
    passthrough: bool,
    next_stack: Instant,
    quit: bool,
    error: Option<OverlayError>,
}

/// Opens her window and runs until she is closed.
pub fn run(opts: &OverlayOptions) -> Result<(), OverlayError> {
    // SAFETY: process-wide flag; harmless if already set by a manifest.
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let mut ctl = Controller::new(opts)?;
    ctl.stage.render()?;
    let main_cls = register_class(LILLITH_CLASS, IDC_HAND)?;
    let menu_cls = register_class(LILLITH_MENU_CLASS, IDC_ARROW)?;
    let (screen, _) = screen_and_work_area();
    let monitor = (screen.x, screen.y, screen.w as u32, screen.h as u32);
    poll_desktop();
    let (w, h) = ctl.stage.size();
    let p = ctl
        .placement
        .clamped((w, h), (monitor.2, monitor.3), KEEP_ON_SCREEN);
    ctl.placement = p;
    ctl.brain.set_position(p.left as f32);
    ctl.set_screen((monitor.2, monitor.3));
    let (x, y) = p.to_xy(h, monitor.3);
    let pos = (monitor.0 + x, monitor.1 + y);
    let mut ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW;
    if ctl.always_on_top {
        ex |= WS_EX_TOPMOST;
    }
    let hwnd = create_window(&main_cls, "Lillith", ex, pos, (w, h))?;
    let now = Instant::now();
    let mut app = App {
        dib: Dib::new(w, h)?,
        ctl,
        hwnd,
        menu_cls,
        menu: None,
        monitor,
        pos,
        frame: period(opts.render_hz),
        next: now,
        last: now,
        started: now,
        exit_after: opts.exit_after,
        pixels_dirty: true,
        tracking: false,
        hovered: false,
        passthrough: false,
        next_stack: now + STACK_POLL,
        quit: false,
        error: None,
    };
    app.draw();
    // SAFETY: our window.
    unsafe {
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    tracing::info!(
        size = ?(w, h),
        left = p.left,
        bottom = p.bottom,
        ?monitor,
        "win32 overlay window (layered, per-pixel alpha, top-most = {})",
        app.ctl.always_on_top
    );
    app.run_loop();
    app.close_menu();
    // SAFETY: our window; destroyed once.
    unsafe {
        DestroyWindow(app.hwnd);
    }
    app.ctl.log_stats();
    match app.error.take() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

impl App {
    fn run_loop(&mut self) {
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        while !self.quit {
            // SAFETY: standard message pump on this thread.
            unsafe {
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    if msg.message == WM_QUIT {
                        self.quit = true;
                    }
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            let evs = QUEUE.with(|q| std::mem::take(&mut *q.borrow_mut()));
            for (hwnd, ev) in evs {
                let which = if hwnd == self.hwnd {
                    Win::Main
                } else if self.menu.as_ref().is_some_and(|m| m.hwnd == hwnd) {
                    Win::Menu
                } else {
                    continue;
                };
                match which {
                    Win::Main => self.main_event(ev),
                    Win::Menu => self.menu_event(ev),
                }
                self.sync_menu();
            }
            if self.quit {
                break;
            }
            self.tick();
            let wait = self
                .next
                .saturating_duration_since(Instant::now())
                .as_millis()
                .min(100) as u32;
            // SAFETY: waits for input or the next frame.
            unsafe {
                MsgWaitForMultipleObjects(0, std::ptr::null(), 0, wait, QS_ALLINPUT);
            }
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        if self.ctl.quit_requested() {
            self.quit = true;
            return;
        }
        if let Some(limit) = self.exit_after {
            if now - self.started >= limit {
                self.quit = true;
                return;
            }
        }
        if now >= self.next_stack {
            self.next_stack = now + STACK_POLL;
            poll_desktop();
        }
        if now < self.next {
            return;
        }
        let dt = (now - self.last).as_secs_f32().min(0.25);
        self.last = now;
        match self.ctl.tick(dt) {
            Ok(c) => self.pixels_dirty |= c.pixels || c.mask,
            Err(e) => {
                self.error = Some(e);
                self.quit = true;
                return;
            }
        }
        self.apply_resize();
        if let Some(on_top) = self.ctl.take_level() {
            let after = if on_top { HWND_TOPMOST } else { HWND_NOTOPMOST };
            // SAFETY: our window.
            unsafe {
                SetWindowPos(
                    self.hwnd,
                    after,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
        self.frame = period(self.ctl.render_hz());
        if let Some(p) = self.ctl.take_move() {
            if !self.ctl.drag.active() {
                self.move_to(p);
            }
        }
        self.sync_menu();
        if self.pixels_dirty {
            self.draw();
        }
        self.update_passthrough();
        self.next += self.frame;
        if self.next < now {
            self.next = now + self.frame;
        }
    }

    fn draw(&mut self) {
        let opacity = self.ctl.opacity;
        let canvas = self.ctl.stage.canvas();
        rgba_to_argb_u32(canvas.data(), self.dib.pixels(), opacity);
        if !self.dib.present(self.hwnd, self.pos) {
            tracing::warn!("UpdateLayeredWindow failed");
        }
        self.pixels_dirty = false;
    }

    fn move_to(&mut self, p: Placement) {
        let (w, h) = self.ctl.stage.size();
        let (mx, my, mw, mh) = self.monitor;
        let p = p.clamped((w, h), (mw, mh), KEEP_ON_SCREEN);
        let (x, y) = p.to_xy(h, mh);
        self.pos = (mx + x, my + y);
        // SAFETY: our window.
        unsafe {
            SetWindowPos(
                self.hwnd,
                std::ptr::null_mut(),
                self.pos.0,
                self.pos.1,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        self.ctl.placement = p;
    }

    fn apply_resize(&mut self) {
        if !self.ctl.take_resized() {
            return;
        }
        let (w, h) = self.ctl.stage.size();
        match Dib::new(w, h) {
            Ok(d) => self.dib = d,
            Err(e) => tracing::warn!("resize: {e}"),
        }
        if let Err(e) = self.ctl.stage.render() {
            tracing::warn!("render after resize: {e}");
        }
        let p = self.ctl.take_move().unwrap_or(self.ctl.placement);
        self.move_to(p);
        self.pixels_dirty = true;
        tracing::info!(size = ?(w, h), "overlay resized");
    }

    /// Lets clicks outside her input mask fall through to the window below.
    fn update_passthrough(&mut self) {
        let want = if self.ctl.drag.active() || self.menu.is_some() {
            false
        } else {
            match cursor_pos() {
                Some((cx, cy)) => {
                    let (lx, ly) = (cx - self.pos.0, cy - self.pos.1);
                    !self
                        .ctl
                        .stage
                        .mask()
                        .iter()
                        .any(|r| lx >= r.x && lx < r.x + r.w && ly >= r.y && ly < r.y + r.h)
                }
                None => false,
            }
        };
        if want == self.passthrough {
            return;
        }
        self.passthrough = want;
        // SAFETY: our window; only toggles WS_EX_TRANSPARENT.
        unsafe {
            let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) as u32;
            let ex = if want {
                ex | WS_EX_TRANSPARENT
            } else {
                ex & !WS_EX_TRANSPARENT
            };
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex as isize);
        }
        if want && self.hovered {
            self.hovered = false;
            self.ctl.hover(false);
        }
    }

    fn global(&self, local: (f64, f64)) -> (f64, f64) {
        cursor_pos()
            .map(|(x, y)| (x as f64, y as f64))
            .unwrap_or((self.pos.0 as f64 + local.0, self.pos.1 as f64 + local.1))
    }

    fn main_event(&mut self, ev: Ev) {
        match ev {
            Ev::Move(x, y) => {
                if !self.tracking {
                    let mut t = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: self.hwnd,
                        dwHoverTime: 0,
                    };
                    // SAFETY: valid struct for our window.
                    self.tracking = unsafe { TrackMouseEvent(&mut t) } != 0;
                }
                if !self.hovered {
                    self.hovered = true;
                    self.ctl.hover(true);
                }
                if self.ctl.drag.active() {
                    if let Some(p) = self.ctl.motion(self.global((x, y))) {
                        self.move_to(p);
                    }
                }
            }
            Ev::LeftDown(x, y) => {
                let g = self.global((x, y));
                if self.ctl.press(g, (x, y), false) {
                    // SAFETY: our window.
                    unsafe {
                        SetCapture(self.hwnd);
                    }
                }
            }
            Ev::LeftUp => {
                if self.ctl.drag.active() {
                    if let Some((cx, cy)) = cursor_pos() {
                        if let Some(p) = self.ctl.motion((cx as f64, cy as f64)) {
                            self.move_to(p);
                        }
                    }
                }
                let _: Release = self.ctl.release();
                // SAFETY: releases our own capture (no-op otherwise).
                unsafe {
                    ReleaseCapture();
                }
            }
            Ev::Other(b, x, y) => {
                self.ctl.button(b, (x, y));
            }
            Ev::Leave => {
                self.tracking = false;
                if !self.ctl.drag.active() && self.hovered {
                    self.hovered = false;
                    self.ctl.hover(false);
                }
            }
            Ev::KeyDown(vk) => match vk {
                VK_ESCAPE => self.ctl.key(Key::Escape),
                VK_RETURN => self.ctl.key(Key::Enter),
                _ => {}
            },
            Ev::Char(c) => self.ctl.key(Key::Char(c)),
            Ev::Inactive => {}
            Ev::Close => self.quit = true,
        }
    }

    fn menu_event(&mut self, ev: Ev) {
        match ev {
            Ev::Move(x, y) => {
                self.ctl.menu_pointer(Some((x, y)));
            }
            Ev::LeftDown(x, y) | Ev::Other(_, x, y) => {
                let hit = self.ctl.menu_click((x, y));
                tracing::debug!(?hit, x, y, "menu click");
            }
            Ev::Leave => {
                self.ctl.menu_pointer(None);
            }
            Ev::KeyDown(vk) => {
                let k = match vk {
                    VK_ESCAPE => Some(MenuKey::Escape),
                    VK_UP => Some(MenuKey::Up),
                    VK_DOWN => Some(MenuKey::Down),
                    VK_LEFT => Some(MenuKey::Left),
                    VK_RIGHT => Some(MenuKey::Right),
                    VK_RETURN | VK_SPACE => Some(MenuKey::Enter),
                    _ => None,
                };
                if let Some(k) = k {
                    let _ = self.ctl.menu_key(k);
                }
            }
            // A click anywhere else (or Alt-Tab) closes it.
            Ev::Inactive | Ev::Close => self.ctl.close_menu(),
            Ev::LeftUp | Ev::Char(_) => {}
        }
    }

    /// Shows / hides / redraws the menu window to match the controller.
    fn sync_menu(&mut self) {
        if self.ctl.take_menu_opened() {
            self.close_menu();
            match self.open_menu() {
                Ok(m) => self.menu = Some(m),
                Err(e) => {
                    tracing::warn!("menu popup: {e}");
                    self.ctl.close_menu();
                }
            }
        }
        if self.ctl.menu().is_none() {
            self.close_menu();
            return;
        }
        self.draw_menu();
    }

    fn open_menu(&mut self) -> Result<Menu, OverlayError> {
        let (w, h) = self.ctl.menu().map(|m| m.size()).unwrap_or((1, 1));
        let (_, work) = screen_and_work_area();
        let (px, py) = cursor_pos().unwrap_or((work.x, work.y));
        let x = px.min(work.right() - w as i32).max(work.x);
        let y = py.min(work.bottom() - h as i32).max(work.y);
        let hwnd = create_window(
            &self.menu_cls,
            "Lillith menu",
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            (x, y),
            (w, h),
        )?;
        let menu = Menu {
            hwnd,
            dib: Dib::new(w, h)?,
            pos: (x, y),
        };
        self.menu = Some(menu);
        self.draw_menu();
        let menu = self.menu.take().expect("just set");
        // SAFETY: our window; foreground so arrows / Esc reach it and a
        // click elsewhere deactivates (closes) it.
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            SetForegroundWindow(hwnd);
        }
        tracing::info!(x, y, size = ?(w, h), "win32 menu popup");
        Ok(menu)
    }

    fn draw_menu(&mut self) {
        if self.menu.is_none() {
            return;
        }
        let Some(pm) = self.ctl.render_menu() else {
            return;
        };
        let (data, w, h) = (pm.data().to_vec(), pm.width(), pm.height());
        let Some(m) = self.menu.as_mut() else { return };
        if (m.dib.w, m.dib.h) != (w, h) {
            match Dib::new(w, h) {
                Ok(d) => m.dib = d,
                Err(e) => {
                    tracing::warn!("menu resize: {e}");
                    return;
                }
            }
        }
        rgba_to_argb_u32(&data, m.dib.pixels(), 255);
        m.dib.present(m.hwnd, m.pos);
    }

    fn close_menu(&mut self) {
        if let Some(m) = self.menu.take() {
            // SAFETY: our menu window, destroyed once.
            unsafe {
                DestroyWindow(m.hwnd);
            }
            // Messages still queued for it are dropped by the hwnd check.
        }
    }
}
