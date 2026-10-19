//! Click-through probe for the X11 backend: opens a striped window and
//! prints every button press it receives. Put it behind Lillith and click
//! next to her: the probe must report the click; click on her body and it
//! must not.
//!
//! `cargo run -p lillith-overlay --features x11 --example x11_click_probe -- X Y W H`

use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::COPY_DEPTH_FROM_PARENT;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<i32> = std::env::args()
        .skip(1)
        .filter_map(|s| s.parse().ok())
        .collect();
    let (x, y, w, h) = match a[..] {
        [x, y, w, h] => (x, y, w, h),
        _ => (0, 600, 800, 600),
    };
    let (conn, screen) = x11rb::connect(None)?;
    let s = &conn.setup().roots[screen];
    let win = conn.generate_id()?;
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        win,
        s.root,
        x as i16,
        y as i16,
        w as u16,
        h as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new()
            .background_pixel(0x00_20_60_C0)
            .event_mask(EventMask::BUTTON_PRESS | EventMask::EXPOSURE),
    )?;
    conn.change_property8(
        PropMode::REPLACE,
        win,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        b"lillith-click-probe",
    )?;
    let gc = conn.generate_id()?;
    conn.create_gc(gc, win, &CreateGCAux::new().foreground(0x00_F0_C0_20))?;
    conn.map_window(win)?;
    conn.flush()?;
    println!("probe window {win} at {x},{y} {w}x{h}");
    loop {
        match conn.wait_for_event()? {
            Event::Expose(_) => {
                let bars: Vec<Rectangle> = (0..w / 40)
                    .map(|i| Rectangle {
                        x: (i * 40) as i16,
                        y: 0,
                        width: 20,
                        height: h as u16,
                    })
                    .collect();
                conn.poly_fill_rectangle(win, gc, &bars)?;
                conn.flush()?;
            }
            Event::ButtonPress(e) => {
                println!(
                    "PROBE CLICK button={} root=({},{}) local=({},{})",
                    e.detail, e.root_x, e.root_y, e.event_x, e.event_y
                );
            }
            _ => {}
        }
    }
}
