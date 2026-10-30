//! QuantumSec early-usage **awakening** popup (feature `egui`).
//!
//! Shown once on early use (caller persists the flag under
//! `~/.biotek/config/awakening.json`). Cyberpunk palette: void black,
//! crimson, cyan, phantom purple; JetBrains Mono for the terminal line,
//! Cinzel Decorative (`carved`) for the demon lettering title. Additive:
//! does not replace the Soul Contract or any existing modal.

use crate::egui_theme::{self as th, c32, glow_blob, neon_rect};
use crate::palette::{self as pal, Rgba};
use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align2, Color32, Context, FontFamily, FontId, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2,
};

/// Click the awakening card reported this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AwakeningChoice {
    /// User accepted / continues (primary).
    Continue,
    /// User dismissed (Esc or secondary).
    Dismiss,
}

/// Show the awakening modal. Call every frame while it should be visible;
/// stop once you've acted on the returned choice.
pub fn quantumsec_awakening_popup(ctx: &Context, id: Id) -> Option<AwakeningChoice> {
    let screen = ctx.screen_rect();
    let w = (screen.width() - 24.0).clamp(280.0, 460.0);
    let t = ctx.input(|i| i.time) as f32;
    let modal = egui::Modal::new(id)
        .frame(egui::Frame::NONE)
        .backdrop_color(Color32::from_rgba_premultiplied(4, 2, 10, 210));
    let resp = modal.show(ctx, |ui| plate(ui, id, w, t));
    ctx.request_repaint_after(std::time::Duration::from_millis(60));
    let esc = resp.is_top_modal
        && !resp.any_popup_open
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    if esc {
        return Some(AwakeningChoice::Dismiss);
    }
    resp.inner
}

fn plate(ui: &mut egui::Ui, id: Id, w: f32, t: f32) -> Option<AwakeningChoice> {
    let mut choice = None;
    let pad = 18.0;
    let frame_t = 8.0;
    let inner_w = w - 2.0 * (frame_t + pad);

    let title_font = th::carved_font(22.0);
    let body_font = FontId::new(14.0, FontFamily::Proportional);
    let mono = th::mono_font(12.0);
    let label_font = th::display_font(10.0);

    let painter = ui.painter().clone();
    let title = painter.layout(
        "QUANTUMSEC AWAKENS".into(),
        title_font,
        c32(pal::CYBER_CRIMSON),
        inner_w,
    );
    let whisper = painter.layout(
        "A whisper from the core: the lattice is listening. Protons will watch \
processes, listeners, key configs and load — all local, all light."
            .into(),
        body_font,
        c32(pal::CYBER_TEXT),
        inner_w,
    );
    let term = painter.layout(
        "$ biotek quantumsec --awaken\n# consent gate armed · ~/.biotek/quantumsec".into(),
        mono,
        c32(pal::CYBER_CYAN),
        inner_w - 16.0,
    );
    let h = 12.0
        + title.size().y
        + 10.0
        + whisper.size().y
        + 12.0
        + (term.size().y + 16.0)
        + 16.0
        + 40.0
        + 12.0;

    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h + 2.0 * frame_t), Sense::hover());
    let outer = rect;
    let inner = outer.shrink(frame_t);

    ui.painter()
        .rect_filled(outer, 0.0, c32(pal::CYBER_VOID.alpha(240)));
    // Cyber iron-ish rim
    ui.painter().rect_stroke(
        outer,
        0.0,
        Stroke::new(2.0, c32(pal::CYBER_PURPLE.alpha(200))),
        StrokeKind::Inside,
    );
    neon_rect(ui.painter(), inner.shrink(2.0), pal::CYBER_CYAN, 1.0, 4.0);
    let pulse = 0.5 + 0.5 * (t * 2.2).sin();
    glow_blob(
        ui.painter(),
        inner.center_top() + Vec2::new(0.0, 28.0),
        40.0 + 8.0 * pulse,
        pal::CYBER_PURPLE,
        0.35 + 0.25 * pulse,
    );

    let mut y = inner.top() + pad;
    let x = inner.left() + pad;

    ui.painter()
        .galley(Pos2::new(x, y), title.clone(), c32(pal::CYBER_CRIMSON));
    y += title.size().y + 4.0;
    ui.painter().text(
        Pos2::new(x, y),
        Align2::LEFT_TOP,
        "EARLY USAGE · BLOOD SEAL ARMED",
        label_font,
        c32(pal::CYBER_PURPLE),
    );
    y += 16.0;
    ui.painter()
        .galley(Pos2::new(x, y), whisper.clone(), c32(pal::CYBER_TEXT));
    y += whisper.size().y + 12.0;

    let term_r = Rect::from_min_size(Pos2::new(x, y), Vec2::new(inner_w, term.size().y + 16.0));
    ui.painter()
        .rect_filled(term_r, 2.0, c32(Rgba::new(0x00, 0x18, 0x1c, 0xcc)));
    ui.painter().rect_stroke(
        term_r,
        2.0,
        Stroke::new(1.0, c32(pal::CYBER_CYAN.alpha(160))),
        StrokeKind::Inside,
    );
    ui.painter().galley(
        Pos2::new(term_r.left() + 8.0, term_r.top() + 8.0),
        term,
        c32(pal::CYBER_CYAN),
    );
    y = term_r.bottom() + 16.0;

    let btn_w = (inner_w - 10.0) / 2.0;
    let btn_h = 36.0;
    let dismiss_r = Rect::from_min_size(Pos2::new(x, y), Vec2::new(btn_w, btn_h));
    let cont_r = Rect::from_min_size(Pos2::new(x + btn_w + 10.0, y), Vec2::new(btn_w, btn_h));

    if cyber_button(ui, id.with("dismiss"), dismiss_r, "DISMISS", false, t) {
        choice = Some(AwakeningChoice::Dismiss);
    }
    if cyber_button(ui, id.with("continue"), cont_r, "AWAKEN", true, t) {
        choice = Some(AwakeningChoice::Continue);
    }
    choice
}

fn cyber_button(
    ui: &mut egui::Ui,
    id: Id,
    rect: Rect,
    label: &str,
    primary: bool,
    t: f32,
) -> bool {
    let resp = ui.interact(rect, id, Sense::click());
    let hover = resp.hovered();
    let fill = if primary {
        if hover {
            pal::CYBER_CRIMSON
        } else {
            pal::CYBER_CRIMSON.alpha(200)
        }
    } else if hover {
        pal::CYBER_PURPLE.alpha(180)
    } else {
        Rgba::new(0x1a, 0x0a, 0x22, 0xcc)
    };
    let stroke_c = if primary {
        pal::CYBER_CYAN
    } else {
        pal::CYBER_PURPLE
    };
    ui.painter().rect_filled(rect, 2.0, c32(fill));
    let wobble = if hover {
        0.5 * (t * 6.0).sin().abs()
    } else {
        0.0
    };
    ui.painter().rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0 + wobble, c32(stroke_c)),
        StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        th::display_font(13.0),
        c32(pal::CYBER_TEXT),
    );
    resp.clicked()
}

/// Layout-job helper for styled body copy on cyber panels.
pub fn cyber_body_job(text: &str, width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = width;
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: FontId::new(14.0, FontFamily::Proportional),
            color: c32(pal::CYBER_TEXT),
            ..Default::default()
        },
    );
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice_enum_is_distinct() {
        assert_ne!(AwakeningChoice::Continue, AwakeningChoice::Dismiss);
    }
}
