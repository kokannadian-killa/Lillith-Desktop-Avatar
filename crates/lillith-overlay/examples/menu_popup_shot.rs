//! Renders the right-click popup to a PNG (no window): a submenu open and
//! a row hovered, composited at 2x over a dark backdrop.
//!
//! `cargo run -p lillith-overlay --features text --example menu_popup_shot -- out.png [Submenu] [Row] [#accent]`

use lillith_core::config::MoveMode;
use lillith_core::menu::MenuState;
use lillith_core::mood::Mood;
use lillith_overlay::menu_popup::{Fonts, MenuPopup, Row, ROW_H};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "menu_popup.png".into());
    let sub = args.get(2).cloned().unwrap_or_else(|| "Mood".into());
    let hov = args.get(3).cloned().unwrap_or_else(|| "Alert".into());
    let mut p = MenuPopup::new(MenuState {
        mode: MoveMode::Stay,
        mood: Mood::Playful,
        muted: false,
        monitor_paused: false,
        scale: 0.55,
        extras: Default::default(),
    });
    let label = |r: &Row| match r {
        Row::Header { label, .. } | Row::Leaf { label, .. } => Some(label.clone()),
        Row::Separator => None,
    };
    if let Some(i) = p
        .rows()
        .iter()
        .position(|r| label(r).as_deref() == Some(&sub))
    {
        p.click(30.0, (p.row_top(i) + ROW_H / 2) as f64);
    }
    if let Some(i) = p
        .rows()
        .iter()
        .position(|r| label(r).as_deref() == Some(&hov))
    {
        p.pointer(Some((60.0, (p.row_top(i) + ROW_H / 2) as f64)));
    }
    if let Some(a) = args.get(4).and_then(|h| lillith_theme::Accent::from_hex(h)) {
        p.set_accent(a);
    }
    let mut fonts = Fonts::preload();
    let pm = p.render(&mut fonts).expect("render").clone();
    let (w, h) = (pm.width(), p.content_height() + 8);
    let mut img = image::RgbaImage::from_pixel(w + 16, h + 8, image::Rgba([8, 6, 10, 255]));
    for y in 0..pm.height().min(h) {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let d = &pm.data()[i..i + 4];
            let a = d[3] as u32;
            let px = img.get_pixel_mut(x + 8, y + 4);
            for (o, s) in px.0.iter_mut().zip(d).take(3) {
                *o = (*s as u32 + *o as u32 * (255 - a) / 255).min(255) as u8;
            }
        }
    }
    let big = image::imageops::resize(
        &img,
        img.width() * 2,
        img.height() * 2,
        image::imageops::FilterType::Nearest,
    );
    big.save(&out).expect("save");
    println!("{out}");
}
