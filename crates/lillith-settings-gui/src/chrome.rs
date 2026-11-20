//! Relic-styled widgets for the settings window, built on `lillith_theme`.

use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, Layout, Pos2, Rect, Response, Sense, Stroke,
    StrokeKind, Ui, UiBuilder,
};
use lillith_theme::egui_theme::{self as th, c32};
use lillith_theme::geom;
use lillith_theme::palette::{self as pal, Rgba};

/// A DemonOS terminal panel around `add`. The panel is sized from the
/// content's height on the previous frame (egui is immediate mode), which
/// settles within a frame.
pub fn section<R>(
    ui: &mut Ui,
    title: &str,
    tag: Option<&str>,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    let id = ui.id().with(("lillith-section", title));
    let width = ui.available_width();
    let top = ui.cursor().min;
    let prev_h: f32 = ui.ctx().data(|d| d.get_temp(id)).unwrap_or(64.0);
    let bg = Rect::from_min_size(top, vec2(width, prev_h));
    let content = th::terminal_panel(ui.painter(), bg, title, tag);
    let inner = Rect::from_min_max(
        pos2(content.left(), content.top()),
        pos2(content.right(), top.y + 20_000.0),
    );
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(inner)
            .layout(Layout::top_down(Align::Min)),
    );
    let r = add(&mut child);
    let h = (child.min_rect().bottom() - top.y + 12.0).max(36.0);
    if (h - prev_h).abs() > 0.5 {
        ui.ctx().data_mut(|d| d.insert_temp(id, h));
        ui.ctx().request_repaint();
    }
    ui.allocate_rect(Rect::from_min_size(top, vec2(width, h)), Sense::hover());
    ui.add_space(4.0);
    r
}

/// Small label in the DemonOS face.
pub fn label_os(ui: &mut Ui, text: &str, color: Rgba) -> Response {
    ui.label(
        egui::RichText::new(text)
            .font(th::display_font(11.0))
            .color(c32(color)),
    )
}

/// Dim helper text.
pub fn hint(ui: &mut Ui, text: &str) -> Response {
    ui.label(egui::RichText::new(text).small().color(c32(pal::TEXT_MUTE)))
}

/// Width of a [`row`] label column: 134 px, or the text plus a gap when it
/// is longer.
fn row_label_width(text_w: f32) -> f32 {
    (text_w + 8.0).max(134.0)
}

/// A row label of fixed width (wider only when the label needs it), then
/// `add` on the same line.
pub fn row<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        let font = egui::FontId::monospace(12.5);
        // A label longer than the column pushes the control right instead
        // of being painted underneath it.
        let text_w = ui
            .painter()
            .layout_no_wrap(label.to_string(), font.clone(), c32(pal::TEXT_DIM))
            .size()
            .x;
        let (r, _) = ui.allocate_exact_size(vec2(row_label_width(text_w), 22.0), Sense::hover());
        ui.painter().text(
            pos2(r.left(), r.center().y),
            Align2::LEFT_CENTER,
            label,
            font,
            c32(pal::TEXT_DIM),
        );
        add(ui)
    })
    .inner
}

/// Width [`choice`] takes for `label`.
pub fn choice_width(ui: &Ui, label: &str) -> f32 {
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_string(), th::bold_font(14.0), c32(pal::TEXT));
    galley.size().x + 42.0
}

/// Selectable choice with a rune and the knife highlight when selected or
/// hovered.
pub fn choice(ui: &mut Ui, label: &str, selected: bool, rune: usize) -> Response {
    let font = th::bold_font(14.0);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font, c32(pal::TEXT));
    let size = vec2(galley.size().x + 42.0, 30.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hover = ui.ctx().animate_bool(resp.id.with("h"), resp.hovered());
    let p = ui.painter();
    let poly = geom::chamfer_rect([rect.left(), rect.top(), rect.right(), rect.bottom()], 6.0);
    th::fill_convex(
        p,
        &poly,
        c32(if selected {
            Rgba::hex(0x2a0510)
        } else {
            Rgba::hex(0x12060a)
        }),
    );
    if selected {
        th::knife_highlight(p, rect.shrink2(vec2(2.0, 3.0)), 1.0);
        th::neon_line(p, &poly, th::accent().base, 1.2, 3.5, true);
    } else {
        th::knife_highlight(p, rect.shrink2(vec2(2.0, 3.0)), hover * 0.55);
        p.add(egui::Shape::closed_line(
            poly.iter().map(|q| pos2(q[0], q[1])).collect(),
            Stroke::new(1.0, c32(pal::BLOOD.lerp(pal::BLOOD_NEON, hover))),
        ));
    }
    let rh = 13.0;
    let ro = pos2(rect.left() + 11.0, rect.center().y - rh * 0.5);
    if selected {
        th::neon_rune(p, rune, ro, rh, pal::NEON_BLUE_CORE);
    } else {
        th::neon_rune(p, rune, ro, rh, pal::BLOOD.lerp(pal::BLOOD_NEON, hover));
    }
    let tc = if selected {
        Rgba::hex(0xffffff)
    } else {
        pal::TEXT.lerp(Rgba::hex(0xffffff), hover)
    };
    p.galley_with_override_text_color(
        pos2(rect.left() + 30.0, rect.center().y - galley.size().y * 0.5),
        galley,
        c32(tc),
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Rune checkbox: a carved socket that fills with blood and a pink algiz
/// rune when on.
pub fn rune_check(ui: &mut Ui, on: bool, label: &str) -> Response {
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(14.5),
        c32(pal::TEXT),
    );
    let size = vec2(galley.size().x + 34.0, 26.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hover = ui.ctx().animate_bool(resp.id.with("h"), resp.hovered());
    let p = ui.painter();
    let b = Rect::from_center_size(pos2(rect.left() + 11.0, rect.center().y), vec2(18.0, 18.0));
    let poly = geom::chamfer_rect([b.left(), b.top(), b.right(), b.bottom()], 4.0);
    th::fill_convex(
        p,
        &poly,
        c32(if on { pal::BLOOD } else { Rgba::hex(0x0a0306) }),
    );
    p.add(egui::Shape::closed_line(
        poly.iter().map(|q| pos2(q[0], q[1])).collect(),
        Stroke::new(
            1.0,
            c32(if on {
                th::accent().base
            } else {
                pal::IRON_HI.lerp(pal::BLOOD_NEON, hover)
            }),
        ),
    ));
    if on {
        th::neon_rune(
            p,
            13,
            pos2(b.center().x - 4.3, b.top() + 3.0),
            12.0,
            th::accent().base,
        );
    }
    p.galley(
        pos2(rect.left() + 28.0, rect.center().y - galley.size().y * 0.5),
        galley,
        c32(pal::TEXT),
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Pill badge.
pub fn chip(ui: &mut Ui, text: &str, color: Rgba, dot: bool) -> Response {
    let font = th::display_font(10.5);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, c32(color));
    let w = galley.size().x + if dot { 30.0 } else { 16.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
    paint_chip(ui.painter(), rect, galley, color, dot);
    resp
}

pub fn paint_chip(
    p: &egui::Painter,
    rect: Rect,
    galley: std::sync::Arc<egui::Galley>,
    color: Rgba,
    dot: bool,
) {
    p.rect_filled(rect, CornerRadius::same(10), c32(Rgba::hex(0x0a0306)));
    p.rect_filled(rect, CornerRadius::same(10), c32(color.alpha(28)));
    p.rect_stroke(
        rect,
        CornerRadius::same(10),
        Stroke::new(1.0, c32(color.alpha(160))),
        StrokeKind::Inside,
    );
    let mut x = rect.left() + 8.0;
    if dot {
        let c = pos2(x + 4.0, rect.center().y);
        th::glow_blob(p, c, 8.0, color, 1.6);
        p.circle_filled(c, 3.2, c32(color));
        x += 15.0;
    }
    p.galley(
        pos2(x, rect.center().y - galley.size().y * 0.5),
        galley,
        c32(color),
    );
}

/// Primary action button shaped like a blade (pointing right).
pub fn blade_button(ui: &mut Ui, text: &str, enabled: bool) -> Response {
    let font = th::carved_font(15.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, Color32::WHITE);
    let size = vec2(galley.size().x + 70.0, 34.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let hover = ui
        .ctx()
        .animate_bool(resp.id.with("h"), resp.hovered() && enabled);
    let p = ui.painter();
    let x0 = rect.left() + 12.0;
    let outline = geom::blade_outline(x0, rect.right(), rect.center().y, rect.height() - 4.0);
    let mut m = egui::epaint::Mesh::default();
    for v in &outline {
        let t = (v[0] - x0) / rect.width();
        let col = if enabled {
            pal::CRIMSON.lerp(th::accent().base, t * (0.55 + 0.45 * hover))
        } else {
            pal::IRON_MID
        };
        m.colored_vertex(pos2(v[0], v[1]), c32(col));
    }
    for i in 1..outline.len() as u32 - 1 {
        m.add_triangle(0, i, i + 1);
    }
    if enabled {
        th::glow_blob(
            p,
            rect.center(),
            rect.height() * 1.1,
            th::accent().base,
            0.25 + 0.5 * hover,
        );
    }
    p.add(egui::Shape::mesh(m));
    if enabled {
        th::neon_line(
            p,
            &geom::blade_edge(x0, rect.right(), rect.center().y, rect.height() - 4.0),
            Rgba::hex(0xffd6f4),
            1.2,
            3.0,
            false,
        );
    }
    // hilt
    p.rect_filled(
        Rect::from_center_size(pos2(x0 - 2.0, rect.center().y), vec2(4.0, rect.height())),
        CornerRadius::ZERO,
        c32(pal::IRON_EDGE),
    );
    p.rect_filled(
        Rect::from_min_max(
            pos2(rect.left(), rect.center().y - 4.0),
            pos2(x0 - 4.0, rect.center().y + 4.0),
        ),
        CornerRadius::same(1),
        c32(pal::BLOOD_DEEP),
    );
    let tc = if enabled {
        Color32::WHITE
    } else {
        c32(pal::TEXT_MUTE)
    };
    p.galley_with_override_text_color(
        pos2(x0 + 14.0, rect.center().y - galley.size().y * 0.5),
        galley,
        tc,
    );
    let tip = if enabled {
        format!("{text} — click to act")
    } else {
        format!("{text} (disabled)")
    };
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(tip)
    } else {
        resp.on_hover_text(tip)
    }
}

/// Secondary button in carved stone.
pub fn stone_button(ui: &mut Ui, text: &str) -> Response {
    let font = th::bold_font(13.5);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, c32(pal::TEXT));
    let size = vec2(galley.size().x + 22.0, 28.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hover = ui.ctx().animate_bool(resp.id.with("h"), resp.hovered());
    let p = ui.painter();
    let poly = geom::chamfer_rect([rect.left(), rect.top(), rect.right(), rect.bottom()], 5.0);
    th::fill_convex(p, &poly, c32(pal::STONE.lerp(pal::STONE_HI, hover)));
    p.line_segment(
        [
            rect.left_top() + vec2(5.0, 1.0),
            rect.right_top() + vec2(-5.0, 1.0),
        ],
        Stroke::new(1.0, c32(pal::IRON_EDGE.alpha(120))),
    );
    p.add(egui::Shape::closed_line(
        poly.iter().map(|q| pos2(q[0], q[1])).collect(),
        Stroke::new(1.0, c32(pal::IRON_HI.lerp(pal::BLOOD_NEON, hover))),
    ));
    p.galley(
        pos2(rect.left() + 11.0, rect.center().y - galley.size().y * 0.5),
        galley,
        c32(pal::TEXT),
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(format!("{text} — click to act"))
}

/// Live validation badge under / next to a field: green "LOOPBACK ONLY"
/// or red with the reason.
pub fn validity(ui: &mut Ui, res: &Result<(), String>, ok_text: &str) {
    match res {
        Ok(()) => {
            chip(ui, ok_text, pal::OK, true);
        }
        Err(e) => {
            ui.label(
                egui::RichText::new(format!("⚠ {e}"))
                    .small()
                    .color(c32(pal::BLOOD_SHEEN)),
            );
        }
    }
}

/// Monospace block (terminal text) with a copy button.
pub fn code_block(ui: &mut Ui, text: &str) -> bool {
    let mut copied = false;
    egui::Frame::new()
        .fill(c32(Rgba::hex(0x070103)))
        .stroke(Stroke::new(1.0, c32(pal::BLOOD_DEEP)))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(text)
                            .monospace()
                            .color(c32(pal::TERMINAL_TEXT)),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                if stone_button(ui, "⧉ copy").clicked() {
                    ui.ctx().copy_text(text.to_string());
                    copied = true;
                }
            });
        });
    copied
}

/// Tiny vertical spacer + blade divider across the available width.
pub fn divider(ui: &mut Ui) {
    ui.add_space(4.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 14.0), Sense::hover());
    th::blade_divider(ui.painter(), r.left(), r.right(), r.center().y - 2.0);
    ui.add_space(2.0);
}

/// Center of a rect helper for painters.
pub fn center(r: Rect) -> Pos2 {
    r.center()
}
