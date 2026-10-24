//! egui glue (feature `egui`): fonts, Visuals/Style, and Painter helpers
//! that draw the [`crate::geom`] ornaments in the [`crate::palette`] colours.

use crate::geom::{self, Pt};
use crate::palette::{self as pal, Rgba};
use egui::epaint::{Mesh, PathShape, PathStroke};
use egui::{
    Color32, Context, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Painter, Pos2,
    Rect, Shape, Stroke, StrokeKind, TextStyle, Vec2,
};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// colours
// ---------------------------------------------------------------------------

/// Palette colour → egui colour.
pub fn c32(c: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

/// Additive (light-emitting) version of `c` at strength `a` (0..=1): glows
/// brighten what's under them instead of covering it.
pub fn additive(c: Rgba, a: f32) -> Color32 {
    let a = a.clamp(0.0, 1.0) * (c.a as f32 / 255.0);
    let m = |v: u8| (v as f32 * a).round() as u8;
    Color32::from_rgba_premultiplied(m(c.r), m(c.g), m(c.b), 0)
}

// ---------------------------------------------------------------------------
// avatar-matched accent
// ---------------------------------------------------------------------------

thread_local! {
    static ACCENT: std::cell::Cell<crate::Accent> = const { std::cell::Cell::new(crate::Accent::LILLITH) };
}

/// The accent the painters use on this (UI) thread; Lillith's magenta
/// unless [`set_accent`] / [`use_accent`] changed it.
pub fn accent() -> crate::Accent {
    ACCENT.with(|a| a.get())
}

/// Sets the accent for later painting (call before [`install`]).
pub fn use_accent(a: crate::Accent) {
    ACCENT.with(|c| c.set(a));
}

/// Switches the accent live: painters and the Visuals follow.
pub fn set_accent(ctx: &Context, a: crate::Accent) {
    use_accent(a);
    ctx.set_visuals(visuals());
    ctx.request_repaint();
}

fn p2(p: Pt) -> Pos2 {
    Pos2::new(p[0], p[1])
}

fn pts(v: &[Pt]) -> Vec<Pos2> {
    v.iter().map(|p| p2(*p)).collect()
}

// ---------------------------------------------------------------------------
// fonts + style
// ---------------------------------------------------------------------------

/// Carved display face (Cinzel Decorative): tabs, window title.
pub fn carved() -> FontFamily {
    FontFamily::Name("carved".into())
}
/// DemonOS face (Michroma): status line, terminal labels.
pub fn display() -> FontFamily {
    FontFamily::Name("display".into())
}
/// Body semibold (Chakra Petch SemiBold).
pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

pub fn carved_font(size: f32) -> FontId {
    FontId::new(size, carved())
}
pub fn display_font(size: f32) -> FontId {
    FontId::new(size, display())
}
pub fn bold_font(size: f32) -> FontId {
    FontId::new(size, bold())
}
pub fn mono_font(size: f32) -> FontId {
    FontId::monospace(size)
}

/// Bundled fonts in front of egui's defaults (which stay as fallbacks for
/// symbols such as ✓ ▸ ⚠).
pub fn font_definitions() -> FontDefinitions {
    let mut f = FontDefinitions::default();
    let add = |f: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        f.font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
    };
    add(&mut f, "cinzel", crate::fonts::CINZEL_DECORATIVE_BOLD);
    add(&mut f, "chakra", crate::fonts::CHAKRA_PETCH_REGULAR);
    add(&mut f, "chakra-semi", crate::fonts::CHAKRA_PETCH_SEMIBOLD);
    add(&mut f, "michroma", crate::fonts::MICHROMA);
    add(&mut f, "jbmono", crate::fonts::JETBRAINS_MONO);

    let fallbacks: Vec<String> = f
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    f.families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "chakra".into());
    f.families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "jbmono".into());
    for (fam, first) in [
        (carved(), "cinzel"),
        (display(), "michroma"),
        (bold(), "chakra-semi"),
    ] {
        let mut v = vec![first.to_string()];
        v.extend(fallbacks.iter().cloned());
        f.families.insert(fam, v);
    }
    f
}

/// Dark relic Visuals: obsidian panels, iron strokes, crimson selection,
/// pink only on the active/hovered widget.
pub fn visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.override_text_color = None;
    v.panel_fill = c32(pal::VOID);
    v.window_fill = c32(pal::OBSIDIAN);
    v.window_stroke = Stroke::new(1.0, c32(pal::IRON_HI));
    v.window_corner_radius = CornerRadius::same(2);
    v.menu_corner_radius = CornerRadius::same(2);
    v.extreme_bg_color = c32(Rgba::hex(0x0a0306));
    v.faint_bg_color = c32(Rgba::hex(0x120509));
    v.code_bg_color = c32(Rgba::hex(0x14040a));
    v.hyperlink_color = c32(pal::NEON_BLUE);
    v.warn_fg_color = c32(pal::WARN);
    v.error_fg_color = c32(pal::BLOOD_NEON);
    v.selection.bg_fill = c32(pal::CRIMSON.alpha(170));
    v.selection.stroke = Stroke::new(1.0, c32(accent().base));
    v.text_cursor.stroke = Stroke::new(2.0, c32(accent().base));
    v.striped = true;
    v.slider_trailing_fill = true;
    let w = &mut v.widgets;
    let cr = CornerRadius::same(2);
    w.noninteractive.bg_fill = c32(pal::OBSIDIAN);
    w.noninteractive.weak_bg_fill = c32(pal::OBSIDIAN);
    w.noninteractive.bg_stroke = Stroke::new(1.0, c32(pal::IRON_MID));
    w.noninteractive.fg_stroke = Stroke::new(1.0, c32(pal::TEXT_DIM));
    w.noninteractive.corner_radius = cr;
    w.inactive.bg_fill = c32(Rgba::hex(0x2a1218));
    w.inactive.weak_bg_fill = c32(Rgba::hex(0x1c0b11));
    w.inactive.bg_stroke = Stroke::new(1.0, c32(pal::BLOOD));
    w.inactive.fg_stroke = Stroke::new(1.0, c32(pal::TEXT));
    w.inactive.corner_radius = cr;
    w.hovered.bg_fill = c32(Rgba::hex(0x3a0812));
    w.hovered.weak_bg_fill = c32(Rgba::hex(0x3a0812));
    w.hovered.bg_stroke = Stroke::new(1.0, c32(pal::BLOOD_NEON));
    w.hovered.fg_stroke = Stroke::new(1.5, c32(pal::TEXT));
    w.hovered.corner_radius = cr;
    w.hovered.expansion = 0.0;
    w.active.bg_fill = c32(pal::CRIMSON);
    w.active.weak_bg_fill = c32(pal::CRIMSON);
    w.active.bg_stroke = Stroke::new(1.0, c32(accent().base));
    w.active.fg_stroke = Stroke::new(2.0, Color32::WHITE);
    w.active.corner_radius = cr;
    w.open = w.hovered;
    v
}

/// Installs fonts, Visuals and text styles on `ctx`.
pub fn install(ctx: &Context) {
    ctx.set_fonts(font_definitions());
    ctx.style_mut(|s| {
        s.visuals = visuals();
        s.text_styles = [
            (TextStyle::Small, FontId::proportional(12.5)),
            (TextStyle::Body, FontId::proportional(15.0)),
            (TextStyle::Button, FontId::proportional(15.0)),
            (TextStyle::Heading, carved_font(20.0)),
            (TextStyle::Monospace, mono_font(13.0)),
        ]
        .into();
        s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        s.spacing.button_padding = Vec2::new(10.0, 4.0);
        s.spacing.interact_size.y = 24.0;
        s.spacing.slider_width = 180.0;
        s.spacing.scroll.bar_width = 8.0;
    });
}

// ---------------------------------------------------------------------------
// primitives
// ---------------------------------------------------------------------------

/// Filled convex polygon.
pub fn fill_convex(p: &Painter, poly: &[Pt], fill: Color32) {
    p.add(Shape::convex_polygon(pts(poly), fill, Stroke::NONE));
}

/// Neon polyline: stacked strokes with falling alpha (additive) under a
/// crisp core with a bright centre.
pub fn neon_line(p: &Painter, line: &[Pt], color: Rgba, width: f32, spread: f32, closed: bool) {
    let points = pts(line);
    let shape = |w: f32, c: Color32| -> Shape {
        let s = PathStroke::new(w, c);
        if closed {
            Shape::Path(PathShape::closed_line(points.clone(), s))
        } else {
            Shape::Path(PathShape::line(points.clone(), s))
        }
    };
    for (w, a) in geom::glow_layers(width, spread, 4) {
        if a < 1.0 {
            p.add(shape(w, additive(color, a)));
        }
    }
    p.add(shape(width, c32(color)));
    if width >= 1.5 {
        p.add(shape(
            width * 0.4,
            c32(color.lerp(Rgba::hex(0xffffff), 0.6)),
        ));
    }
}

/// Neon rectangle outline.
pub fn neon_rect(p: &Painter, r: Rect, color: Rgba, width: f32, spread: f32) {
    let v = [
        [r.left(), r.top()],
        [r.right(), r.top()],
        [r.right(), r.bottom()],
        [r.left(), r.bottom()],
    ];
    neon_line(p, &v, color, width, spread, true);
}

/// Soft additive glow blob (concentric circles).
pub fn glow_blob(p: &Painter, c: Pos2, radius: f32, color: Rgba, strength: f32) {
    for i in 0..8 {
        let t = i as f32 / 8.0;
        p.circle_filled(
            c,
            radius * (1.0 - t * 0.85),
            additive(color, strength * 0.07 * (1.0 + t)),
        );
    }
}

/// Tapered strip (horn, talon) filled solid.
pub fn tapered(p: &Painter, line: &[Pt], w0: f32, w1: f32, fill: Color32) {
    for q in geom::tapered_quads(line, w0, w1) {
        fill_convex(p, &q, fill);
    }
}

/// Mesh quad with a vertical gradient (top → bottom).
pub fn gradient_rect(p: &Painter, r: Rect, top: Color32, bottom: Color32) {
    let mut m = Mesh::default();
    m.colored_vertex(r.left_top(), top);
    m.colored_vertex(r.right_top(), top);
    m.colored_vertex(r.right_bottom(), bottom);
    m.colored_vertex(r.left_bottom(), bottom);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    p.add(Shape::mesh(m));
}

/// Mesh quad with a horizontal gradient (left → right).
pub fn gradient_rect_h(p: &Painter, r: Rect, left: Color32, right: Color32) {
    let mut m = Mesh::default();
    m.colored_vertex(r.left_top(), left);
    m.colored_vertex(r.right_top(), right);
    m.colored_vertex(r.right_bottom(), right);
    m.colored_vertex(r.left_bottom(), left);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    p.add(Shape::mesh(m));
}

/// Text with a chromatic "glitch" split (cyan left, red right) under it.
pub fn glitch_text(
    p: &Painter,
    pos: Pos2,
    anchor: egui::Align2,
    text: &str,
    font: FontId,
    color: Rgba,
) -> Rect {
    p.text(
        pos + Vec2::new(-1.0, 0.0),
        anchor,
        text,
        font.clone(),
        additive(Rgba::hex(0x28e0ff), 0.28),
    );
    p.text(
        pos + Vec2::new(1.0, 0.3),
        anchor,
        text,
        font.clone(),
        additive(pal::BLOOD_NEON, 0.45),
    );
    p.text(pos, anchor, text, font, c32(color))
}

// ---------------------------------------------------------------------------
// blood
// ---------------------------------------------------------------------------

/// Heavy blood along a top edge at `y` from `x0` to `x1`: a wet band plus
/// seeded drips up to `max_len` long with bulbs and a wet sheen.
pub fn blood_drips(p: &Painter, x0: f32, x1: f32, y: f32, seed: u32, max_len: f32, spacing: f32) {
    // band
    let edge = geom::blood_edge(seed, x0, x1, 3.0, (max_len * 0.09).clamp(2.0, 6.0));
    let mut m = Mesh::default();
    let (top, bot) = (c32(pal::BLOOD_DEEP), c32(pal::BLOOD));
    for (x, t) in &edge {
        m.colored_vertex(Pos2::new(*x, y - 1.0), top);
        m.colored_vertex(Pos2::new(*x, y + t), bot);
    }
    for i in 0..edge.len().saturating_sub(1) as u32 {
        let k = i * 2;
        m.add_triangle(k, k + 1, k + 2);
        m.add_triangle(k + 1, k + 3, k + 2);
    }
    p.add(Shape::mesh(m));
    // drips
    for d in geom::blood_drips(seed, x0 + 4.0, x1 - 4.0, spacing, max_len) {
        let body = d.body(y);
        let mut m = Mesh::default();
        m.colored_vertex(p2(body[0]), c32(pal::BLOOD_DEEP));
        m.colored_vertex(p2(body[1]), c32(pal::BLOOD_DEEP));
        m.colored_vertex(p2(body[2]), c32(pal::CRIMSON));
        m.colored_vertex(p2(body[3]), c32(pal::CRIMSON));
        m.add_triangle(0, 1, 2);
        m.add_triangle(0, 2, 3);
        p.add(Shape::mesh(m));
        let (c, r) = d.bulb(y);
        p.circle_filled(p2(c), r, c32(pal::CRIMSON));
        p.circle_filled(
            p2(c) + Vec2::new(0.0, r * 0.25),
            r * 0.8,
            c32(Rgba::hex(0xa3001a)),
        );
        // sheen
        if d.len > 6.0 {
            p.line_segment(
                [
                    Pos2::new(d.x - d.width * 0.18, y + 2.0),
                    Pos2::new(d.x - d.width * 0.12, y + d.len * 0.75),
                ],
                Stroke::new((d.width * 0.18).max(0.7), c32(pal::BLOOD_SHEEN.alpha(90))),
            );
        }
        p.circle_filled(
            p2(c) + Vec2::new(-r * 0.35, -r * 0.3),
            (r * 0.28).max(0.6),
            c32(pal::BLOOD_SHEEN.alpha(150)),
        );
    }
}

// ---------------------------------------------------------------------------
// blades
// ---------------------------------------------------------------------------

fn steel_blade(p: &Painter, x0: f32, x1: f32, cy: f32, h: f32, edge: Rgba, edge_glow: f32) {
    let outline = geom::blade_outline(x0, x1, cy, h);
    // steel: dark spine, lighter towards the edge
    fill_convex(p, &outline, c32(Rgba::hex(0x2a2730)));
    let upper = [
        outline[0],
        outline[1],
        [outline[1][0], cy],
        [outline[0][0], cy],
    ];
    fill_convex(p, &upper, c32(Rgba::hex(0x4b4752)));
    p.line_segment(
        [p2(outline[0]), p2(outline[1])],
        Stroke::new(1.0, c32(Rgba::hex(0x8d8896))),
    );
    let e = geom::blade_edge(x0, x1, cy, h);
    neon_line(p, &e, edge, 1.0, edge_glow, false);
    let (guard, grip) = geom::blade_hilt(x0, cy, h);
    let gr = Rect::from_min_max(Pos2::new(guard[0], guard[1]), Pos2::new(guard[2], guard[3]));
    let hr = Rect::from_min_max(Pos2::new(grip[0], grip[1]), Pos2::new(grip[2], grip[3]));
    p.rect_filled(hr, CornerRadius::same(1), c32(Rgba::hex(0x1a0f14)));
    for i in 1..4 {
        let x = hr.left() + hr.width() * i as f32 / 4.0;
        p.line_segment(
            [Pos2::new(x, hr.top()), Pos2::new(x, hr.bottom())],
            Stroke::new(1.0, c32(pal::BLOOD_DEEP)),
        );
    }
    p.rect_filled(gr, CornerRadius::same(1), c32(pal::IRON_HI));
    p.rect_stroke(
        gr,
        CornerRadius::same(1),
        Stroke::new(1.0, c32(pal::IRON_EDGE)),
        StrokeKind::Inside,
    );
}

/// Horizontal divider drawn as a long thin knife with a bleeding edge.
pub fn blade_divider(p: &Painter, x0: f32, x1: f32, y: f32) {
    let h = 5.0;
    let hilt = h * 2.1;
    p.line_segment(
        [Pos2::new(x0, y + 4.0), Pos2::new(x1, y + 4.0)],
        Stroke::new(1.0, c32(pal::BLOOD_DRIED)),
    );
    steel_blade(p, x0 + hilt, x1, y, h, pal::BLOOD_NEON, 3.0);
    // a few drops falling off the edge
    for (i, d) in geom::blood_drips(
        (x0 as u32) ^ (y as u32),
        x0 + hilt + 20.0,
        x1 - 30.0,
        90.0,
        9.0,
    )
    .iter()
    .enumerate()
    {
        if i % 2 == 0 {
            p.line_segment(
                [
                    Pos2::new(d.x, y + h * 0.5),
                    Pos2::new(d.x, y + h * 0.5 + d.len),
                ],
                Stroke::new(1.4, c32(pal::CRIMSON)),
            );
            p.circle_filled(Pos2::new(d.x, y + h * 0.5 + d.len), 1.6, c32(pal::CRIMSON));
        }
    }
}

/// Knife-blade highlight behind a hovered / selected row or tab.
/// `strength` 0..=1 (hover fade).
pub fn knife_highlight(p: &Painter, r: Rect, strength: f32) {
    if strength <= 0.01 {
        return;
    }
    let h = r.height() * 0.86;
    let hilt = h * 0.55;
    let x0 = r.left() + hilt;
    let outline = geom::blade_outline(x0, r.right(), r.center().y, h);
    let mut m = Mesh::default();
    let a = |c: Rgba, k: f32| c32(c.fade(k * strength));
    // gradient along the blade: crimson at the hilt → pink towards the tip
    for (i, v) in outline.iter().enumerate() {
        let t = (v[0] - x0) / (r.right() - x0).max(1.0);
        let col = pal::CRIMSON.lerp(accent().base, t * 0.7);
        m.colored_vertex(p2(*v), a(col, if i == 0 || i == 5 { 0.55 } else { 0.32 }));
    }
    for i in 1..outline.len() as u32 - 1 {
        m.add_triangle(0, i, i + 1);
    }
    p.add(Shape::mesh(m));
    neon_line(
        p,
        &geom::blade_edge(x0, r.right(), r.center().y, h),
        accent().base.fade(strength),
        1.2,
        4.0,
        false,
    );
    let spine = [outline[0], outline[1], outline[2]];
    p.add(Shape::line(
        pts(&spine),
        Stroke::new(1.0, c32(Rgba::hex(0xffd0f0).fade(0.5 * strength))),
    ));
    // hilt
    let gr = Rect::from_center_size(Pos2::new(x0 - 1.5, r.center().y), Vec2::new(3.0, h * 1.05));
    p.rect_filled(gr, CornerRadius::ZERO, c32(pal::IRON_EDGE.fade(strength)));
    let hr = Rect::from_min_max(
        Pos2::new(r.left() + 2.0, r.center().y - h * 0.18),
        Pos2::new(x0 - 3.0, r.center().y + h * 0.18),
    );
    p.rect_filled(
        hr,
        CornerRadius::same(1),
        c32(pal::BLOOD_DEEP.fade(strength)),
    );
}

// ---------------------------------------------------------------------------
// relic ornaments
// ---------------------------------------------------------------------------

/// Horned skull ornament centred on `c`, `s` ≈ skull height. `eye` lights
/// the sockets (ember, violet...).
pub fn skull(p: &Painter, c: Pos2, s: f32, eye: Rgba) {
    let k = geom::skull([c.x, c.y], s);
    for h in &k.horns {
        let sh: Vec<Pt> = h.iter().map(|q| [q[0] + 1.5, q[1] + 1.5]).collect();
        tapered(
            p,
            &sh,
            k.horn_width,
            0.5,
            c32(Rgba::hex(0x000000).alpha(160)),
        );
        tapered(p, h, k.horn_width, 0.5, c32(pal::IRON_MID));
        tapered(p, h, k.horn_width * 0.45, 0.2, c32(pal::IRON_EDGE));
    }
    let (cc, cr) = k.cranium;
    p.circle_filled(
        p2(cc) + Vec2::new(1.5, 2.0),
        cr,
        c32(Rgba::hex(0x000000).alpha(170)),
    );
    let jaw = Rect::from_min_max(Pos2::new(k.jaw[0], k.jaw[1]), Pos2::new(k.jaw[2], k.jaw[3]));
    p.rect_filled(
        jaw.translate(Vec2::new(1.5, 2.0)),
        CornerRadius::same((s * 0.08) as u8),
        c32(Rgba::hex(0x000000).alpha(170)),
    );
    p.circle_filled(p2(cc), cr, c32(pal::BONE_SHADOW));
    p.circle_filled(
        p2(cc) + Vec2::new(-cr * 0.12, -cr * 0.14),
        cr * 0.86,
        c32(pal::BONE),
    );
    p.rect_filled(
        jaw,
        CornerRadius::same((s * 0.08) as u8),
        c32(pal::BONE_SHADOW.lerp(pal::BONE, 0.6)),
    );
    for (ec, er) in k.eyes {
        p.circle_filled(p2(ec), er, c32(Rgba::hex(0x0a0204)));
        glow_blob(p, p2(ec), er * 1.6, eye, 1.4);
        p.circle_filled(p2(ec), er * 0.32, c32(eye));
    }
    fill_convex(p, &k.nose, c32(Rgba::hex(0x0a0204)));
    for (a, b) in &k.teeth {
        p.line_segment(
            [p2(*a), p2(*b)],
            Stroke::new((s * 0.03).max(1.0), c32(pal::BONE_SHADOW)),
        );
    }
    // a little blood from the crown
    p.line_segment(
        [
            p2(cc) + Vec2::new(cr * 0.2, -cr * 0.95),
            p2(cc) + Vec2::new(cr * 0.25, -cr * 0.2),
        ],
        Stroke::new((s * 0.05).max(1.0), c32(pal::BLOOD.alpha(200))),
    );
}

/// Triangle fan from `c` to the closed `ring`, shaded `inner` → `outer`
/// (a cheap radial gradient). `ring` must be star-shaped about `c`.
pub fn fan(p: &Painter, c: Pt, ring: &[Pt], inner: Color32, outer: Color32) {
    let mut m = Mesh::default();
    m.colored_vertex(p2(c), inner);
    for q in ring {
        m.colored_vertex(p2(*q), outer);
    }
    let n = ring.len() as u32;
    for i in 0..n {
        m.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    p.add(Shape::mesh(m));
}

/// Anti-aliased outline of a closed polygon.
fn outline(p: &Painter, ring: &[Pt], width: f32, color: Color32) {
    p.add(Shape::Path(PathShape::closed_line(
        pts(ring),
        PathStroke::new(width, color),
    )));
}

/// Anti-aliased open polyline.
fn polyline(p: &Painter, line: &[Pt], width: f32, color: Color32) {
    p.add(Shape::Path(PathShape::line(
        pts(line),
        PathStroke::new(width, color),
    )));
}

/// Soft dark smudge (grime, recesses): stacked translucent black discs.
fn grime(p: &Painter, c: Pos2, radius: f32, strength: f32) {
    for i in 0..5 {
        let t = i as f32 / 5.0;
        p.circle_filled(
            c,
            radius * (1.0 - t * 0.7),
            c32(Rgba::hex(0x0a0204).fade(strength * 0.16)),
        );
    }
}

/// The Soul Contract's death-metal skull (see [`geom::DeathSkull`]),
/// centred on `c`, `s` ≈ crown-to-chin height. `eye` is the ember far back
/// in the sockets; `pulse` (0..=1) breathes it.
pub fn death_skull(p: &Painter, c: Pos2, s: f32, eye: Rgba, pulse: f32) {
    let k = geom::death_skull([c.x, c.y], s);
    let ink = Rgba::hex(0x070103);
    let edge_w = (s * 0.02).max(0.9);
    let fine = (s * 0.018).max(0.7);
    let shift =
        |v: &[Pt], d: f32| -> Vec<Pt> { v.iter().map(|q| [q[0] + d, q[1] + d * 1.3]).collect() };

    // drop shadow
    let sh = c32(Rgba::hex(0x000000).alpha(150));
    for h in &k.horns {
        tapered(p, &shift(h, 1.5), k.horn_width, k.horn_tip, sh);
    }
    let sc = [k.cranium_c[0] + 1.5, k.cranium_c[1] + 2.0];
    fan(p, sc, &shift(&k.cranium, 1.5), sh, sh);
    let sm = [k.mandible_c[0] + 1.5, k.mandible_c[1] + 2.0];
    fan(p, sm, &shift(&k.mandible, 1.5), sh, sh);

    // horns: dark keratin, ringed, burnt black at the tips
    let horn_base = Rgba::hex(0x564738);
    let horn_mid = Rgba::hex(0x2e2420);
    for h in &k.horns {
        let quads = geom::tapered_quads(h, k.horn_width, k.horn_tip);
        let n = quads.len().max(1) as f32;
        for (i, q) in quads.iter().enumerate() {
            let t = i as f32 / n;
            let col = if t < 0.55 {
                horn_base.lerp(horn_mid, t / 0.55)
            } else {
                horn_mid.lerp(ink, (t - 0.55) / 0.45)
            };
            fill_convex(p, q, c32(col));
        }
        // a hot crimson rim along the underside, lit by the forge below
        let lower = |i: usize| -> Vec<Pt> { quads.iter().map(|q| q[i]).collect() };
        let (a, b) = (lower(0), lower(3));
        let sum = |v: &[Pt]| v.iter().map(|q| q[1]).sum::<f32>();
        let rim = if sum(&a) > sum(&b) { a } else { b };
        polyline(
            p,
            &rim[..rim.len() * 2 / 3],
            fine,
            additive(pal::CRIMSON, 0.55),
        );
        // highlight down the spine
        tapered(
            p,
            &h[..h.len() * 3 / 4],
            k.horn_width * 0.22,
            0.3,
            c32(Rgba::hex(0xb8a68f).alpha(35)),
        );
        // segment ridges
        for (i, q) in quads.iter().enumerate().take(quads.len() - 2) {
            let w = if i % 2 == 0 { fine * 1.2 } else { fine * 0.7 };
            p.line_segment([p2(q[0]), p2(q[3])], Stroke::new(w, c32(ink.alpha(170))));
        }
        let mut ring: Vec<Pt> = quads.iter().map(|q| q[0]).collect();
        ring.push(quads[quads.len() - 1][1]);
        ring.push(quads[quads.len() - 1][2]);
        ring.extend(quads.iter().rev().map(|q| q[3]));
        outline(p, &ring, (edge_w * 0.8).max(0.8), c32(ink.alpha(230)));
    }

    // lower jaw
    fan(
        p,
        k.mandible_c,
        &k.mandible,
        c32(Rgba::hex(0x9c8f7c)),
        c32(Rgba::hex(0x3a2f27)),
    );
    outline(p, &k.mandible, edge_w, c32(ink));

    // cranium: aged bone, bright forehead, filthy edges
    fan(
        p,
        k.cranium_c,
        &k.cranium,
        c32(Rgba::hex(0xd2c6b1)),
        c32(Rgba::hex(0x4a3e34)),
    );
    glow_blob(
        p,
        c + Vec2::new(-0.07 * s, -0.36 * s),
        0.2 * s,
        Rgba::hex(0xfff1dc),
        0.3,
    );
    for (x, y, r, a) in [
        (-0.33, -0.12, 0.09, 0.9),
        (0.33, -0.12, 0.09, 0.9),
        (-0.24, 0.19, 0.075, 1.0),
        (0.24, 0.19, 0.075, 1.0),
        (0.0, 0.24, 0.08, 0.8),
    ] {
        grime(p, c + Vec2::new(x * s, y * s), r * s, a);
    }
    for g in &k.grooves {
        polyline(p, g, fine, c32(ink.alpha(70)));
    }

    // brow ridge catches the light; sockets sink into black
    for b in &k.brow {
        tapered(
            p,
            b,
            (s * 0.05).max(1.2),
            0.6,
            c32(Rgba::hex(0xeee4d2).alpha(170)),
        );
    }
    for (i, ring) in k.sockets.iter().enumerate() {
        let cc = k.socket_c[i];
        let wide: Vec<Pt> = ring
            .iter()
            .map(|q| [cc[0] + (q[0] - cc[0]) * 1.3, cc[1] + (q[1] - cc[1]) * 1.3])
            .collect();
        fan(
            p,
            cc,
            &wide,
            c32(Rgba::hex(0x1a0e0a).alpha(220)),
            c32(Rgba::hex(0x2a1e18).alpha(0)),
        );
        fan(
            p,
            cc,
            ring,
            c32(Rgba::hex(0x000000)),
            c32(Rgba::hex(0x140608)),
        );
        outline(p, ring, fine, c32(ink));
        let e = p2(cc) + Vec2::new(0.0, 0.012 * s);
        glow_blob(p, e, 0.075 * s, pal::BLOOD_NEON, 0.35 + 0.25 * pulse);
        p.circle_filled(e, (0.016 * s).max(0.7), c32(eye));
    }

    // inverted-heart nasal cavity
    for lobe in &k.nose {
        fill_convex(p, lobe, c32(Rgba::hex(0x050003)));
        outline(p, lobe, fine * 0.8, c32(ink));
    }

    // mouth + fangs
    let mouth = Rect::from_min_max(
        Pos2::new(k.mouth[0], k.mouth[1]),
        Pos2::new(k.mouth[2], k.mouth[3]),
    );
    p.rect_filled(mouth, CornerRadius::same(1), c32(Rgba::hex(0x050003)));
    for f in &k.fangs {
        fill_convex(p, f, c32(Rgba::hex(0xd8cdb8)));
        let mid = [(f[0][0] + f[1][0]) * 0.5, (f[0][1] + f[1][1]) * 0.5];
        fill_convex(p, &[mid, f[1], f[2]], c32(Rgba::hex(0x7d6f5e)));
        outline(p, f, (fine * 0.7).max(0.6), c32(ink.alpha(220)));
    }

    // fissures: a dark split with a lit lip
    for cr in &k.cracks {
        polyline(
            p,
            &shift(cr, 0.5),
            (fine * 0.6).max(0.5),
            c32(Rgba::hex(0xf2e8d6).alpha(60)),
        );
        polyline(p, cr, fine * 1.1, c32(Rgba::hex(0x1a0c08).alpha(235)));
    }

    // crisp silhouette so it reads at popup size
    outline(p, &k.cranium, edge_w, c32(ink));

    // blood weeping from the right socket
    tapered(
        p,
        &k.drip,
        (s * 0.045).max(1.2),
        (s * 0.03).max(1.0),
        c32(pal::BLOOD),
    );
    let end = p2(*k.drip.last().unwrap_or(&k.socket_c[1]));
    p.circle_filled(
        end + Vec2::new(0.0, 0.01 * s),
        (s * 0.03).max(1.0),
        c32(pal::BLOOD),
    );
    polyline(
        p,
        &k.drip
            .iter()
            .map(|q| [q[0] - 0.4, q[1]])
            .collect::<Vec<_>>(),
        0.6,
        c32(pal::BLOOD_SHEEN.alpha(140)),
    );
}

/// A rune carved into stone/iron: dark groove, lit lower edge, optional
/// inner glow (`glow` > 0).
pub fn carved_rune(p: &Painter, idx: usize, origin: Pos2, h: f32, glow: Option<Rgba>) {
    let segs = geom::rune_segments(idx, [origin.x, origin.y], h);
    let w = (h * 0.12).max(1.4);
    for (a, b) in &segs {
        p.line_segment(
            [p2(*a) + Vec2::new(0.8, 0.9), p2(*b) + Vec2::new(0.8, 0.9)],
            Stroke::new(w, c32(pal::IRON_EDGE.alpha(110))),
        );
        p.line_segment([p2(*a), p2(*b)], Stroke::new(w, c32(Rgba::hex(0x050203))));
    }
    if let Some(g) = glow {
        for (a, b) in &segs {
            neon_line(p, &[*a, *b], g, (w * 0.45).max(0.8), 2.5, false);
        }
    }
}

/// A rune drawn as neon strokes.
pub fn neon_rune(p: &Painter, idx: usize, origin: Pos2, h: f32, color: Rgba) {
    for (a, b) in geom::rune_segments(idx, [origin.x, origin.y], h) {
        neon_line(p, &[a, b], color, (h * 0.09).max(1.0), 3.0, false);
    }
}

/// Cracked obsidian/stone fill with speckles and ember-lit cracks.
pub fn stone_fill(p: &Painter, r: Rect, seed: u32, cracks: usize) {
    gradient_rect(p, r, c32(Rgba::hex(0x1a1216)), c32(Rgba::hex(0x0b0709)));
    let mut rng = geom::Rng::new(seed);
    let n = ((r.width() * r.height()) / 900.0) as usize;
    for _ in 0..n.min(900) {
        let q = Pos2::new(
            rng.range(r.left(), r.right()),
            rng.range(r.top(), r.bottom()),
        );
        let light = rng.f() > 0.5;
        let c = if light {
            Rgba::hex(0x3a2e33).alpha(90)
        } else {
            Rgba::hex(0x000000).alpha(110)
        };
        p.circle_filled(q, rng.range(0.5, 1.6), c32(c));
    }
    ember_cracks(
        p,
        &geom::cracks(seed, [r.left(), r.top(), r.right(), r.bottom()], cracks),
    );
}

/// Cracks with a molten ember core.
pub fn ember_cracks(p: &Painter, lines: &[Vec<Pt>]) {
    for l in lines {
        let v = pts(l);
        p.add(Shape::line(
            v.clone(),
            Stroke::new(3.2, additive(pal::EMBER, 0.12)),
        ));
        p.add(Shape::line(
            v.clone(),
            Stroke::new(1.6, c32(Rgba::hex(0x050203))),
        ));
        p.add(Shape::line(v, Stroke::new(0.7, c32(pal::EMBER.alpha(170)))));
    }
}

/// Black-iron band around `outer` (frame `t` px thick) with bevels, rivets
/// and carved runes. Returns the inner rect.
pub fn iron_frame(p: &Painter, outer: Rect, t: f32, seed: u32) -> Rect {
    let inner = outer.shrink(t);
    // four bands
    let bands = [
        Rect::from_min_max(outer.left_top(), Pos2::new(outer.right(), inner.top())),
        Rect::from_min_max(
            Pos2::new(outer.left(), inner.bottom()),
            outer.right_bottom(),
        ),
        Rect::from_min_max(
            Pos2::new(outer.left(), inner.top()),
            Pos2::new(inner.left(), inner.bottom()),
        ),
        Rect::from_min_max(
            Pos2::new(inner.right(), inner.top()),
            Pos2::new(outer.right(), inner.bottom()),
        ),
    ];
    for (i, b) in bands.iter().enumerate() {
        if i < 2 {
            gradient_rect(p, *b, c32(pal::IRON_MID), c32(Rgba::hex(0x0e0a0c)));
        } else {
            gradient_rect_h(p, *b, c32(pal::IRON_MID), c32(Rgba::hex(0x0e0a0c)));
        }
    }
    p.rect_stroke(
        outer,
        CornerRadius::ZERO,
        Stroke::new(1.0, c32(pal::IRON_EDGE)),
        StrokeKind::Inside,
    );
    p.rect_stroke(
        outer.shrink(2.0),
        CornerRadius::ZERO,
        Stroke::new(1.0, c32(Rgba::hex(0x000000))),
        StrokeKind::Inside,
    );
    p.rect_stroke(
        inner.expand(1.0),
        CornerRadius::ZERO,
        Stroke::new(1.0, c32(pal::IRON_HI)),
        StrokeKind::Inside,
    );
    p.rect_stroke(
        inner,
        CornerRadius::ZERO,
        Stroke::new(1.0, c32(Rgba::hex(0x000000))),
        StrokeKind::Outside,
    );
    // carved rune bands on the long sides (skip corners)
    let rh = (t * 0.48).max(5.0);
    let mut idx = seed as usize;
    for (y, x0, x1) in [(
        outer.bottom() - t * 0.5 - rh * 0.5,
        outer.left() + t * 4.0,
        outer.right() - t * 4.0,
    )] {
        let mut x = x0;
        while x < x1 {
            carved_rune(p, idx, Pos2::new(x, y), rh, None);
            idx += 7;
            x += geom::rune_width(rh) + rh * 0.9;
        }
    }
    for (x, y0, y1) in [
        (
            outer.left() + t * 0.5 - geom::rune_width(rh) * 0.5,
            outer.top() + t * 5.0,
            outer.bottom() - t * 5.0,
        ),
        (
            outer.right() - t * 0.5 - geom::rune_width(rh) * 0.5,
            outer.top() + t * 5.0,
            outer.bottom() - t * 5.0,
        ),
    ] {
        let mut y = y0;
        while y < y1 - rh {
            carved_rune(p, idx, Pos2::new(x, y), rh, None);
            idx += 5;
            y += rh * 2.4;
        }
    }
    // rivets
    for b in [outer.left() + t * 0.5, outer.right() - t * 0.5] {
        for y in [outer.top() + t * 2.5, outer.bottom() - t * 2.5] {
            p.circle_filled(Pos2::new(b, y), t * 0.22, c32(pal::IRON_HI));
            p.circle_filled(Pos2::new(b - 0.6, y - 0.6), t * 0.1, c32(pal::IRON_EDGE));
        }
    }
    inner
}

/// DemonOS terminal panel: translucent blood-red fill, red grid,
/// scanlines, neon border with bright corner brackets and a glitchy title.
/// Returns the content rect.
pub fn terminal_panel(p: &Painter, r: Rect, title: &str, tag: Option<&str>) -> Rect {
    let poly = geom::chamfer_rect([r.left(), r.top(), r.right(), r.bottom()], 7.0);
    fill_convex(p, &poly, c32(Rgba::new(0x0c, 0x01, 0x04, 0xe8)));
    gradient_rect(
        p,
        r.shrink(1.0),
        c32(pal::TERMINAL_FILL.alpha(22)),
        c32(pal::TERMINAL_FILL.alpha(6)),
    );
    let clip = p.with_clip_rect(r.shrink(2.0).intersect(p.clip_rect()));
    let step = 16.0;
    let mut x = r.left() + step;
    while x < r.right() {
        clip.line_segment(
            [Pos2::new(x, r.top()), Pos2::new(x, r.bottom())],
            Stroke::new(1.0, c32(pal::TERMINAL_GRID)),
        );
        x += step;
    }
    let mut y = r.top() + step;
    while y < r.bottom() {
        clip.line_segment(
            [Pos2::new(r.left(), y), Pos2::new(r.right(), y)],
            Stroke::new(1.0, c32(pal::TERMINAL_GRID)),
        );
        y += step;
    }
    neon_line(p, &poly, pal::BLOOD_NEON.alpha(170), 1.0, 2.5, true);
    let bl = 12.0;
    for (c, d) in [
        (r.left_top(), [1.0, 1.0]),
        (r.right_top(), [-1.0, 1.0]),
        (r.left_bottom(), [1.0, -1.0]),
        (r.right_bottom(), [-1.0, -1.0]),
    ] {
        let corner = [c.x + d[0] * 3.0, c.y + d[1] * 3.0];
        neon_line(
            p,
            &geom::bracket(corner, bl, d),
            pal::BLOOD_SHEEN,
            1.4,
            3.0,
            false,
        );
    }
    let mut content_top = r.top() + 8.0;
    if !title.is_empty() {
        let tr = glitch_text(
            p,
            Pos2::new(r.left() + 14.0, r.top() + 8.0),
            egui::Align2::LEFT_TOP,
            title,
            display_font(10.5),
            pal::BLOOD_SHEEN,
        );
        // flicker bar after the title
        p.rect_filled(
            Rect::from_min_size(
                Pos2::new(tr.right() + 8.0, tr.center().y - 1.0),
                Vec2::new(18.0, 2.0),
            ),
            CornerRadius::ZERO,
            c32(pal::BLOOD_NEON.alpha(160)),
        );
        if let Some(tag) = tag {
            p.text(
                Pos2::new(r.right() - 14.0, r.top() + 9.0),
                egui::Align2::RIGHT_TOP,
                tag,
                mono_font(11.0),
                c32(pal::TEXT_MUTE),
            );
        }
        content_top = tr.bottom() + 8.0;
    }
    Rect::from_min_max(
        Pos2::new(r.left() + 12.0, content_top),
        Pos2::new(r.right() - 12.0, r.bottom() - 10.0),
    )
}

/// The electric-blue horns + spade-tail sigil with circuit traces.
/// `pulse` 0..=1 modulates the glow.
pub fn sigil(p: &Painter, c: Pos2, s: f32, pulse: f32, traces: bool) {
    let g = geom::sigil([c.x, c.y], s);
    let blue = pal::NEON_BLUE;
    glow_blob(p, c, s * 0.55, blue, 0.6 + 0.4 * pulse);
    if traces {
        for t in geom::sigil_traces(11, [c.x, c.y], s, 10) {
            p.add(Shape::line(pts(&t), Stroke::new(1.0, additive(blue, 0.55))));
            p.add(Shape::line(pts(&t), Stroke::new(3.0, additive(blue, 0.10))));
            let end = t[t.len() - 1];
            p.circle_filled(p2(end), 2.2, c32(blue));
            p.circle_stroke(p2(end), 4.0, Stroke::new(1.0, additive(blue, 0.5)));
        }
    }
    let w = g.horn_width;
    for h in &g.horns {
        for (k, a) in [(3.4, 0.07), (2.4, 0.12), (1.6, 0.22)] {
            tapered(
                p,
                h,
                w * k,
                w * k * 0.35,
                additive(blue, a * (0.7 + 0.5 * pulse)),
            );
        }
        tapered(p, h, w, 0.8, c32(blue));
        tapered(p, h, w * 0.4, 0.3, c32(pal::NEON_BLUE_CORE));
    }
    let (rc, rr) = g.ring;
    for (k, a) in [(10.0, 0.08), (6.0, 0.14), (3.5, 0.3)] {
        p.circle_stroke(p2(rc), rr, Stroke::new(k, additive(blue, a)));
    }
    p.circle_stroke(p2(rc), rr, Stroke::new(2.2, c32(blue)));
    p.circle_stroke(p2(rc), rr, Stroke::new(0.9, c32(pal::NEON_BLUE_CORE)));
    neon_line(p, &g.eye, pal::VIOLET, 1.4, 3.0, true);
    neon_line(p, &g.tail, blue, 2.2, 5.0, false);
    p.add(Shape::line(
        pts(&g.tail),
        Stroke::new(0.9, c32(pal::NEON_BLUE_CORE)),
    ));
    let sp: Vec<Pt> = g.spade.clone();
    let (sc, _) = (
        sp.iter()
            .fold([0.0, 0.0], |a, b| [a[0] + b[0], a[1] + b[1]]),
        0,
    );
    glow_blob(
        p,
        Pos2::new(sc[0] / sp.len() as f32, sc[1] / sp.len() as f32),
        s * 0.09,
        blue,
        1.5,
    );
    fill_convex(p, &sp, c32(blue));
    neon_line(p, &sp, pal::NEON_BLUE_CORE, 1.0, 2.0, true);
}

/// Claw-shaped resize grip at window `corner`, `dir` into the window.
/// `hot` lights it violet (hover / drag).
pub fn claw_grip(p: &Painter, corner: Pos2, size: f32, dir: [f32; 2], hot: bool) {
    let (talons, (k, kr)) = geom::claw([corner.x, corner.y], size, dir);
    if hot {
        glow_blob(p, p2(k), size * 0.9, pal::VIOLET, 1.2);
    }
    p.circle_filled(
        p2(k) + Vec2::new(1.0, 1.5),
        kr * 1.2,
        c32(Rgba::hex(0x000000).alpha(150)),
    );
    for t in &talons {
        let sh: Vec<Pt> = t.iter().map(|q| [q[0] + 1.0, q[1] + 1.5]).collect();
        tapered(
            p,
            &sh,
            size * 0.15,
            0.4,
            c32(Rgba::hex(0x000000).alpha(150)),
        );
        tapered(p, t, size * 0.15, 0.4, c32(pal::IRON_MID));
        tapered(
            p,
            t,
            size * 0.06,
            0.2,
            c32(if hot { pal::VIOLET } else { pal::IRON_EDGE }),
        );
        let tip = t[t.len() - 1];
        p.circle_filled(
            p2(tip),
            1.0,
            c32(if hot { accent().base } else { pal::BLOOD }),
        );
    }
    p.circle_filled(p2(k), kr, c32(pal::IRON_HI));
    p.circle_filled(
        p2(k),
        kr * 0.45,
        c32(if hot { pal::VIOLET } else { pal::BLOOD }),
    );
}

/// Darkens the edges of `r` (cheap vignette).
pub fn vignette(p: &Painter, r: Rect, depth: f32) {
    let dark = c32(Rgba::hex(0x000000).alpha(150));
    let clear = Color32::TRANSPARENT;
    let d = depth;
    gradient_rect(
        p,
        Rect::from_min_max(r.left_top(), Pos2::new(r.right(), r.top() + d)),
        dark,
        clear,
    );
    gradient_rect(
        p,
        Rect::from_min_max(Pos2::new(r.left(), r.bottom() - d), r.right_bottom()),
        clear,
        dark,
    );
    gradient_rect_h(
        p,
        Rect::from_min_max(r.left_top(), Pos2::new(r.left() + d, r.bottom())),
        dark,
        clear,
    );
    gradient_rect_h(
        p,
        Rect::from_min_max(Pos2::new(r.right() - d, r.top()), r.right_bottom()),
        clear,
        dark,
    );
}

/// The hellforge emblem (anvil, flame, rune ring) centred at `c`, `s` px
/// across: crimson/ember on near-black with a faint neon edge. `flicker`
/// (0..=1) breathes the flame and the ring glow.
pub fn hellforge_emblem(p: &Painter, c: Pos2, s: f32, flicker: f32) {
    let h = geom::hellforge([c.x, c.y], s);
    let f = flicker.clamp(0.0, 1.0);
    // Disc: near-black with a blood rim and a faint neon edge.
    p.circle_filled(c, h.ring_r + s * 0.03, c32(pal::VOID));
    glow_blob(
        p,
        c + egui::vec2(0.0, -s * 0.08),
        s * 0.42,
        pal::EMBER,
        0.55 + 0.35 * f,
    );
    let ring: Vec<Pt> = (0..48)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / 48.0;
            [c.x + a.cos() * h.ring_r, c.y + a.sin() * h.ring_r]
        })
        .collect();
    neon_line(
        p,
        &ring,
        pal::BLOOD_NEON,
        (s * 0.022).max(1.0),
        s * 0.05 * (0.7 + 0.3 * f),
        true,
    );
    p.circle_stroke(
        c,
        h.ring_r - s * 0.14,
        Stroke::new((s * 0.01).max(0.6), c32(pal::BLOOD_DEEP)),
    );
    for (idx, o, rh) in &h.runes {
        for (a, b) in geom::rune_segments(*idx, *o, *rh) {
            p.line_segment(
                [p2(a), p2(b)],
                Stroke::new(
                    (s * 0.014).max(0.7),
                    c32(pal::EMBER.lerp(pal::CRIMSON, 0.5)),
                ),
            );
        }
    }
    // Flame: outer ember to hot core.
    let cols = [pal::CRIMSON, pal::EMBER, pal::EMBER, pal::EMBER_CORE];
    for (i, poly) in h.flames.iter().enumerate() {
        let col = cols[i.min(cols.len() - 1)];
        let a = if i == 0 { 0.85 + 0.15 * f } else { 1.0 };
        fill_convex(p, poly, c32(col.fade(a)));
    }
    // Anvil: black iron with a blood edge and a white-hot struck face.
    for poly in &h.anvil {
        fill_convex(p, poly, c32(pal::IRON));
        let pts: Vec<Pt> = poly.clone();
        p.add(Shape::closed_line(
            pts.iter().map(|q| p2(*q)).collect(),
            Stroke::new((s * 0.012).max(0.6), c32(pal::BLOOD)),
        ));
    }
    neon_line(
        p,
        &h.face_edge,
        pal::EMBER_CORE,
        (s * 0.018).max(0.8),
        s * 0.03,
        false,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_convert() {
        assert_eq!(c32(pal::VOID), Color32::from_rgb(0x05, 0x00, 0x03));
        let a = additive(Rgba::hex(0xff0000), 0.5);
        assert_eq!(a.a(), 0);
        assert_eq!(a.r(), 128);
    }

    #[test]
    fn fonts_register_all_families() {
        let f = font_definitions();
        for fam in [
            carved(),
            display(),
            bold(),
            FontFamily::Proportional,
            FontFamily::Monospace,
        ] {
            let list = f.families.get(&fam).expect("family");
            assert!(!list.is_empty());
        }
        assert_eq!(f.families[&FontFamily::Proportional][0], "chakra");
        assert_eq!(f.families[&FontFamily::Monospace][0], "jbmono");
        assert!(f.families[&carved()].len() > 1, "symbol fallbacks kept");
    }

    #[test]
    fn painters_run_headless() {
        let ctx = Context::default();
        install(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let p = ui.painter();
                let r = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(400.0, 300.0));
                stone_fill(p, r, 1, 6);
                let inner = iron_frame(p, r, 14.0, 3);
                terminal_panel(p, inner.shrink(10.0), "SOUL_SYNTAX", Some("tag"));
                blood_drips(p, r.left(), r.right(), r.top(), 7, 30.0, 26.0);
                blade_divider(p, 20.0, 380.0, 200.0);
                knife_highlight(
                    p,
                    Rect::from_min_size(Pos2::new(20.0, 50.0), Vec2::new(200.0, 26.0)),
                    1.0,
                );
                skull(p, Pos2::new(60.0, 60.0), 30.0, pal::EMBER);
                sigil(p, r.center(), 160.0, 0.5, true);
                claw_grip(p, r.right_bottom(), 28.0, [-1.0, -1.0], true);
                carved_rune(p, 3, Pos2::new(30.0, 30.0), 16.0, Some(pal::VIOLET));
                vignette(p, r, 30.0);
            });
        });
    }
}
