//! The chat bubble's look (plum glass, pink outline, cute rounded
//! widgets), shared by the right-click menu and the Settings window.

use eframe::egui;

pub const BG: egui::Color32 = egui::Color32::from_rgba_premultiplied(24, 14, 34, 242);
pub const PLUM: egui::Color32 = egui::Color32::from_rgb(40, 22, 54);
pub const PLUM_HI: egui::Color32 = egui::Color32::from_rgb(74, 34, 92);
pub const PINK: egui::Color32 = egui::Color32::from_rgb(255, 105, 180);
pub const PINK_SOFT: egui::Color32 = egui::Color32::from_rgb(255, 196, 226);
pub const CYAN: egui::Color32 = egui::Color32::from_rgb(90, 230, 255);
pub const VIOLET: egui::Color32 = egui::Color32::from_rgb(124, 77, 255);

/// Dark plum visuals with pink accents and round corners.
pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    let v = &mut style.visuals;
    v.extreme_bg_color = PLUM;
    v.panel_fill = egui::Color32::from_rgb(28, 16, 38);
    v.window_fill = egui::Color32::from_rgb(28, 16, 38);
    v.selection.bg_fill = PINK;
    v.selection.stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
    v.hyperlink_color = CYAN;
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = egui::CornerRadius::same(10);
    }
    v.widgets.inactive.weak_bg_fill = PLUM;
    v.widgets.inactive.bg_fill = PLUM;
    v.widgets.hovered.weak_bg_fill = PLUM_HI;
    v.widgets.hovered.bg_fill = PLUM_HI;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, PINK);
    v.widgets.active.weak_bg_fill = PINK;
    v.widgets.active.bg_fill = PINK;
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, PINK_SOFT);
    style.spacing.item_spacing = egui::vec2(6.0, 5.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    ctx.set_style(style);
    ctx.set_fonts(crate::chat_bubble::bubble_fonts());
}

/// The rounded plum frame with the pink outline.
pub fn frame() -> egui::Frame {
    egui::Frame::new()
        .fill(BG)
        .stroke(egui::Stroke::new(2.0, PINK))
        .corner_radius(egui::CornerRadius::same(16))
        .inner_margin(egui::Margin::same(8))
}

/// A pink section title.
pub fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(PINK).strong().size(15.0));
}
