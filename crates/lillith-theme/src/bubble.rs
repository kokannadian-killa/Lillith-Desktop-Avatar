//! The chat bubble look (feature `egui`): plum glass, pink outline, cute
//! rounded widgets. Used by `lillith-panel`'s chat bubble window and its
//! interim settings window; the settings pop-up uses the relic look in
//! [`crate::egui_theme`]. Fonts stay with the caller (the bubble ships its
//! own emoji-capable set).

use egui::{Color32, CornerRadius, Frame, Margin, Stroke};

/// Bubble glass (premultiplied, ~95 % opaque).
pub const BG: Color32 = Color32::from_rgba_premultiplied(24, 14, 34, 242);
pub const PLUM: Color32 = Color32::from_rgb(40, 22, 54);
pub const PLUM_HI: Color32 = Color32::from_rgb(74, 34, 92);
/// Panel / window fill behind the widgets.
pub const PLUM_PANEL: Color32 = Color32::from_rgb(28, 16, 38);
pub const PINK: Color32 = Color32::from_rgb(255, 105, 180);
pub const PINK_SOFT: Color32 = Color32::from_rgb(255, 196, 226);
pub const CYAN: Color32 = Color32::from_rgb(90, 230, 255);
pub const VIOLET: Color32 = Color32::from_rgb(124, 77, 255);
/// Text on her (pink) chat lines.
pub const HER_TEXT: Color32 = Color32::from_rgb(58, 16, 52);

/// Dark plum visuals with pink accents and round corners (style only; set
/// fonts separately).
pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    let v = &mut style.visuals;
    v.extreme_bg_color = PLUM;
    v.panel_fill = PLUM_PANEL;
    v.window_fill = PLUM_PANEL;
    v.selection.bg_fill = PINK;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.hyperlink_color = CYAN;
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(10);
    }
    v.widgets.inactive.weak_bg_fill = PLUM;
    v.widgets.inactive.bg_fill = PLUM;
    v.widgets.hovered.weak_bg_fill = PLUM_HI;
    v.widgets.hovered.bg_fill = PLUM_HI;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, PINK);
    v.widgets.active.weak_bg_fill = PINK;
    v.widgets.active.bg_fill = PINK;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, PINK_SOFT);
    style.spacing.item_spacing = egui::vec2(6.0, 5.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    ctx.set_style(style);
}

/// The rounded plum frame with the pink outline.
pub fn frame() -> Frame {
    Frame::new()
        .fill(BG)
        .stroke(Stroke::new(2.0, PINK))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::same(8))
}

/// A pink section title.
pub fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(PINK).strong().size(15.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_sets_the_plum_visuals() {
        let ctx = egui::Context::default();
        apply(&ctx);
        let v = ctx.style().visuals.clone();
        assert_eq!(v.selection.bg_fill, PINK);
        assert_eq!(v.panel_fill, PLUM_PANEL);
        assert_eq!(v.widgets.hovered.bg_stroke.color, PINK);
        assert_eq!(frame().fill, BG);
    }
}
