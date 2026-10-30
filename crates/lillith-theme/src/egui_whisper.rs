//! **WHISPER FROM THE CORE** — info popup for QuantumSec / Biotek chrome.
//!
//! Additive modal: does not replace tooltips. Call when the user clicks an
//! info rune next to a control; Esc or DISMISS closes.

use crate::egui_theme::{self as th, c32, neon_rect};
use crate::palette as pal;
use egui::{
    Align2, Color32, Context, FontFamily, FontId, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2,
};

/// Show a whisper modal. Returns `true` when the user dismissed it.
pub fn whisper_from_the_core(ctx: &Context, id: Id, title: &str, body: &str) -> bool {
    let screen = ctx.screen_rect();
    let w = (screen.width() - 32.0).clamp(260.0, 420.0);
    let mut dismissed = false;
    let modal = egui::Modal::new(id)
        .frame(egui::Frame::NONE)
        .backdrop_color(Color32::from_rgba_premultiplied(4, 2, 10, 180));
    let resp = modal.show(ctx, |ui| {
        let pad = 16.0;
        let frame_t = 6.0;
        let inner_w = w - 2.0 * (frame_t + pad);
        let title_font = th::display_font(11.0);
        let carved = th::carved_font(16.0);
        let body_font = FontId::new(13.0, FontFamily::Proportional);
        let painter = ui.painter().clone();
        let banner = painter.layout(
            "WHISPER FROM THE CORE".into(),
            title_font,
            c32(pal::CYBER_PURPLE),
            inner_w,
        );
        let heading = painter.layout(title.to_string(), carved, c32(pal::CYBER_CRIMSON), inner_w);
        let galley = painter.layout(body.to_string(), body_font, c32(pal::CYBER_TEXT), inner_w);
        let h = 10.0
            + banner.size().y
            + 6.0
            + heading.size().y
            + 8.0
            + galley.size().y
            + 14.0
            + 32.0
            + 10.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h + 2.0 * frame_t), Sense::hover());
        let inner = rect.shrink(frame_t);
        ui.painter()
            .rect_filled(rect, 0.0, c32(pal::CYBER_VOID.alpha(240)));
        ui.painter().rect_stroke(
            rect,
            0.0,
            Stroke::new(1.5, c32(pal::CYBER_PURPLE.alpha(200))),
            StrokeKind::Inside,
        );
        neon_rect(ui.painter(), inner.shrink(2.0), pal::CYBER_CYAN, 1.0, 3.0);
        let mut y = inner.top() + pad;
        let x = inner.left() + pad;
        ui.painter()
            .galley(Pos2::new(x, y), banner, c32(pal::CYBER_PURPLE));
        y += 18.0;
        ui.painter()
            .galley(Pos2::new(x, y), heading.clone(), c32(pal::CYBER_CRIMSON));
        y += heading.size().y + 8.0;
        ui.painter()
            .galley(Pos2::new(x, y), galley.clone(), c32(pal::CYBER_TEXT));
        y += galley.size().y + 14.0;
        let btn = Rect::from_min_size(Pos2::new(x, y), Vec2::new(inner_w, 30.0));
        let r = ui.interact(btn, id.with("dismiss"), Sense::click());
        ui.painter()
            .rect_filled(btn, 2.0, c32(pal::CYBER_PURPLE.alpha(160)));
        ui.painter().text(
            btn.center(),
            Align2::CENTER_CENTER,
            "DISMISS",
            th::display_font(12.0),
            c32(pal::CYBER_TEXT),
        );
        r.clicked()
    });
    let esc = resp.is_top_modal
        && !resp.any_popup_open
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    if esc || resp.inner {
        dismissed = true;
    }
    dismissed
}

/// Compact info rune button; returns true when clicked.
pub fn info_rune(ui: &mut egui::Ui, id: Id) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
    let hover = resp.hovered();
    let col = if hover {
        pal::CYBER_CYAN
    } else {
        pal::CYBER_PURPLE
    };
    ui.painter()
        .circle_stroke(rect.center(), 7.0, Stroke::new(1.0, c32(col)));
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        "i",
        FontId::new(11.0, FontFamily::Proportional),
        c32(col),
    );
    ui.interact(rect, id, Sense::click()).clicked() || resp.clicked()
}

#[cfg(test)]
mod tests {
    #[test]
    fn module_links() {
        // Compiling this module (and linking the test binary) is the check.
    }
}
