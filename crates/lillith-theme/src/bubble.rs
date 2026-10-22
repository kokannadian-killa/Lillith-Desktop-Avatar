//! The chat bubble look (feature `egui`): DemonOS hellforge — near-black
//! glass, a blood-crimson frame with a faint neon edge, ember accents,
//! sharp (chamfer-like) corners and a subtle blood drip under the header.
//! Used by `lillith-panel`'s chat bubble window and its interim settings
//! window; the settings pop-up uses the relic look in [`crate::egui_theme`].
//! Fonts stay with the caller (the bubble ships its own emoji-capable set).
//!
//! The old plum/pink constant names are kept as aliases of the new colours
//! so existing callers keep compiling with the new look.

use crate::egui_theme::c32;
use crate::palette as pal;
use egui::{Color32, CornerRadius, Frame, Margin, Stroke};

/// Bubble glass: near-black, ~96 % opaque (premultiplied).
pub const BG: Color32 = Color32::from_rgba_premultiplied(9, 3, 5, 245);
/// Input / extreme background: obsidian.
pub const OBSIDIAN: Color32 = Color32::from_rgb(0x0c, 0x08, 0x0b);
/// Hovered widget fill: dark blood.
pub const BLOOD_HI: Color32 = Color32::from_rgb(0x3a, 0x00, 0x06);
/// Panel / window fill behind the widgets.
pub const PANEL: Color32 = Color32::from_rgb(0x07, 0x02, 0x04);
/// Frame + title: blood crimson.
pub const CRIMSON: Color32 = Color32::from_rgb(0xc4, 0x00, 0x1a);
/// Neon red edge / selection.
pub const NEON: Color32 = Color32::from_rgb(0xff, 0x1a, 0x2e);
/// Ember accent (status, hints, cursor).
pub const EMBER: Color32 = Color32::from_rgb(0xff, 0x5a, 0x1a);
/// Her chat lines: dried-blood fill with light text.
pub const HER_FILL: Color32 = Color32::from_rgb(0x3a, 0x00, 0x08);
/// Your chat lines: black iron fill.
pub const ME_FILL: Color32 = Color32::from_rgb(0x2c, 0x24, 0x29);
/// Text on her lines.
pub const HER_TEXT: Color32 = Color32::from_rgb(0xff, 0xd6, 0xda);
/// Text on your lines.
pub const ME_TEXT: Color32 = Color32::from_rgb(0xf3, 0xe4, 0xe6);
/// Sharp corners (a hint of bevel only).
pub const CORNER: u8 = 2;

// Legacy names (plum/pink look) → hellforge colours.
pub const PLUM: Color32 = OBSIDIAN;
pub const PLUM_HI: Color32 = BLOOD_HI;
pub const PLUM_PANEL: Color32 = PANEL;
pub const PINK: Color32 = CRIMSON;
pub const PINK_SOFT: Color32 = HER_TEXT;
pub const CYAN: Color32 = EMBER;
pub const VIOLET: Color32 = ME_FILL;

/// Near-black visuals with crimson/ember accents and sharp corners (style
/// only; set fonts separately).
pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals = visuals();
    style.spacing.item_spacing = egui::vec2(6.0, 5.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    ctx.set_style(style);
}

/// The hellforge [`egui::Visuals`].
pub fn visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.extreme_bg_color = OBSIDIAN;
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.selection.bg_fill = CRIMSON;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.hyperlink_color = EMBER;
    v.text_cursor.stroke = Stroke::new(2.0, EMBER);
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
        &mut v.widgets.noninteractive,
    ] {
        w.corner_radius = CornerRadius::same(CORNER);
    }
    v.widgets.inactive.weak_bg_fill = OBSIDIAN;
    v.widgets.inactive.bg_fill = OBSIDIAN;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, c32(pal::BLOOD_DEEP));
    v.widgets.hovered.weak_bg_fill = BLOOD_HI;
    v.widgets.hovered.bg_fill = BLOOD_HI;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, NEON);
    v.widgets.active.weak_bg_fill = CRIMSON;
    v.widgets.active.bg_fill = CRIMSON;
    v.widgets.active.bg_stroke = Stroke::new(1.0, EMBER);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, HER_TEXT);
    v
}

/// The near-black frame with the crimson outline and sharp corners.
pub fn frame() -> Frame {
    Frame::new()
        .fill(BG)
        .stroke(Stroke::new(2.0, CRIMSON))
        .corner_radius(CornerRadius::same(CORNER + 1))
        .inner_margin(Margin::same(8))
}

/// Chat line frame: hers hangs from the left (blood), yours from the
/// right (iron); one squared corner points at the speaker.
pub fn line_frame(me: bool) -> Frame {
    let (fill, edge) = if me {
        (ME_FILL, c32(pal::IRON_HI))
    } else {
        (HER_FILL, c32(pal::BLOOD))
    };
    Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, edge))
        .corner_radius(CornerRadius {
            nw: if me { CORNER } else { 0 },
            ne: if me { 0 } else { CORNER },
            sw: CORNER,
            se: CORNER,
        })
        .inner_margin(Margin::symmetric(9, 6))
}

/// A crimson section title.
pub fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(CRIMSON).strong().size(15.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_sets_the_hellforge_visuals() {
        let ctx = egui::Context::default();
        apply(&ctx);
        let v = ctx.style().visuals.clone();
        assert_eq!(v.selection.bg_fill, CRIMSON);
        assert_eq!(v.panel_fill, PANEL);
        assert_eq!(v.widgets.hovered.bg_stroke.color, NEON);
        assert_eq!(v.widgets.inactive.corner_radius, CornerRadius::same(CORNER));
        assert_eq!(frame().fill, BG);
    }

    #[test]
    fn no_pink_and_readable_lines() {
        // Nothing pink/plum is left in the look.
        for c in [BG, OBSIDIAN, PANEL, HER_FILL, ME_FILL, CRIMSON, EMBER] {
            let [r, _, b, _] = c.to_array();
            assert!(!(r > 200 && b > 150), "pinkish {c:?}");
        }
        let lum = |c: Color32| {
            let [r, g, b, _] = c.to_array();
            crate::Rgba::new(r, g, b, 255)
        };
        assert!(lum(HER_TEXT).contrast(lum(HER_FILL)) >= 7.0);
        assert!(lum(ME_TEXT).contrast(lum(ME_FILL)) >= 7.0);
    }
}
