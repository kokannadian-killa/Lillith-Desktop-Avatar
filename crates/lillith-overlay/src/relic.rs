//! tiny-skia painters for the *ancient demon relic* / DemonOS look shared
//! with the settings window ([`lillith_theme`]): black-iron frames with
//! carved runes, horned skulls, blood drips, the red terminal grid, the
//! horns + spade-tail sigil and knife blades. Only the toolkit-free
//! [`lillith_theme::palette`] and [`lillith_theme::geom`] are used (no egui
//! in the overlay).
//!
//! Neon glow is faked with stacked strokes at falling alpha
//! ([`geom::glow_layers`]); everything is drawn with anti-aliasing at
//! integer-friendly sizes so it stays crisp in a ~240 px popup.

use lillith_theme::geom::{self, Pt};
use lillith_theme::palette as pal;
use lillith_theme::Rgba;
use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Paint, Path, PathBuilder,
    Pixmap, Point, SpreadMode, Stroke, Transform,
};

pub use lillith_theme::Accent;

pub fn color(c: Rgba) -> Color {
    Color::from_rgba8(c.r, c.g, c.b, c.a)
}

pub fn paint(c: Rgba) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color(c));
    p.anti_alias = true;
    p
}

/// Polyline / polygon path.
pub fn path(pts: &[Pt], close: bool) -> Option<Path> {
    let (first, rest) = pts.split_first()?;
    let mut pb = PathBuilder::new();
    pb.move_to(first[0], first[1]);
    for q in rest {
        pb.line_to(q[0], q[1]);
    }
    if close {
        pb.close();
    }
    pb.finish()
}

pub fn fill(pm: &mut Pixmap, pts: &[Pt], c: Rgba) {
    if let Some(p) = path(pts, true) {
        pm.fill_path(
            &p,
            &paint(c),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// Fills `pts` with a linear gradient from `a` (at `p0`) to `b` (at `p1`).
pub fn fill_gradient(pm: &mut Pixmap, pts: &[Pt], p0: Pt, a: Rgba, p1: Pt, b: Rgba) {
    let Some(p) = path(pts, true) else { return };
    let Some(shader) = LinearGradient::new(
        Point::from_xy(p0[0], p0[1]),
        Point::from_xy(p1[0], p1[1]),
        vec![
            GradientStop::new(0.0, color(a)),
            GradientStop::new(1.0, color(b)),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    ) else {
        fill(pm, pts, a);
        return;
    };
    let paint = Paint {
        shader,
        anti_alias: true,
        ..Paint::default()
    };
    pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
}

pub fn stroke(pm: &mut Pixmap, pts: &[Pt], close: bool, c: Rgba, width: f32) {
    if c.a == 0 {
        return;
    }
    if let Some(p) = path(pts, close) {
        let s = Stroke {
            width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        pm.stroke_path(&p, &paint(c), &s, Transform::identity(), None);
    }
}

/// Straight segment with butt caps (crisp 1 px rules).
pub fn rule(pm: &mut Pixmap, a: Pt, b: Pt, c: Rgba, width: f32) {
    if let Some(p) = path(&[a, b], false) {
        let s = Stroke {
            width,
            ..Stroke::default()
        };
        pm.stroke_path(&p, &paint(c), &s, Transform::identity(), None);
    }
}

/// Neon line: stacked strokes at falling alpha, then the crisp core.
pub fn glow(
    pm: &mut Pixmap,
    pts: &[Pt],
    close: bool,
    c: Rgba,
    core: f32,
    spread: f32,
    layers: usize,
) {
    for (w, a) in geom::glow_layers(core, spread, layers) {
        stroke(pm, pts, close, c.fade(a), w);
    }
}

pub fn circle(pm: &mut Pixmap, c: Pt, r: f32, col: Rgba) {
    if let Some(p) = PathBuilder::from_circle(c[0], c[1], r) {
        pm.fill_path(
            &p,
            &paint(col),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

pub fn ring(pm: &mut Pixmap, c: Pt, r: f32, col: Rgba, width: f32) {
    if let Some(p) = PathBuilder::from_circle(c[0], c[1], r) {
        let s = Stroke {
            width,
            ..Stroke::default()
        };
        pm.stroke_path(&p, &paint(col), &s, Transform::identity(), None);
    }
}

/// A strip tapering from `w0` to `w1` along `line` (horns).
pub fn tapered(pm: &mut Pixmap, line: &[Pt], w0: f32, w1: f32, col: Rgba) {
    // One polygon (left side forward, right side back) avoids AA seams
    // between quads.
    let quads = geom::tapered_quads(line, w0, w1);
    if quads.is_empty() {
        return;
    }
    let mut poly: Vec<Pt> = quads.iter().map(|q| q[0]).collect();
    poly.push(quads[quads.len() - 1][1]);
    poly.push(quads[quads.len() - 1][2]);
    poly.extend(quads.iter().rev().map(|q| q[3]));
    fill(pm, &poly, col);
}

// ---------------------------------------------------------------------------
// ornaments
// ---------------------------------------------------------------------------

/// Black-iron frame with bevelled edges and carved runes on the side and
/// bottom bands; returns nothing (the caller knows its inset).
/// `r` is the outer rect, `t` the band thickness (top band `t` too, the
/// bottom band `b`).
pub fn iron_frame(pm: &mut Pixmap, r: [f32; 4], t: f32, b: f32, seed: usize) {
    let [x0, y0, x1, y1] = r;
    let outer = geom::chamfer_rect(r, 5.0);
    fill_gradient(
        pm,
        &outer,
        [x0, y0],
        pal::IRON_MID,
        [x0, y1],
        Rgba::hex(0x0e0a0c),
    );
    // bevel: lit outer edge, black groove, lit inner lip
    stroke(
        pm,
        &geom::chamfer_rect([x0 + 0.5, y0 + 0.5, x1 - 0.5, y1 - 0.5], 4.6),
        true,
        pal::IRON_EDGE,
        1.0,
    );
    stroke(
        pm,
        &geom::chamfer_rect([x0 + 2.5, y0 + 2.5, x1 - 2.5, y1 - 2.5], 3.4),
        true,
        Rgba::hex(0x000000).alpha(200),
        1.0,
    );
    let inner = [x0 + t, y0 + t, x1 - t, y1 - b];
    stroke(
        pm,
        &rect_pts([
            inner[0] - 0.5,
            inner[1] - 0.5,
            inner[2] + 0.5,
            inner[3] + 0.5,
        ]),
        true,
        pal::IRON_HI,
        1.0,
    );
    // carved runes: bottom band and both sides (skipping the corners)
    let rh = (b * 0.5).clamp(5.0, 7.0);
    let mut idx = seed;
    let mut x = x0 + 26.0;
    let y = y1 - b * 0.5 - rh * 0.5;
    while x < x1 - 26.0 - geom::rune_width(rh) {
        carved_rune(pm, idx, [x, y], rh);
        idx += 7;
        x += geom::rune_width(rh) + rh * 1.1;
    }
    let rs = (t * 0.72).clamp(4.0, 6.0);
    for cx in [x0 + t * 0.5, x1 - t * 0.5] {
        let mut y = y0 + 52.0;
        while y < y1 - b - 22.0 {
            carved_rune(pm, idx, [cx - geom::rune_width(rs) * 0.5, y], rs);
            idx += 5;
            y += rs * 3.2;
        }
    }
}

pub fn rect_pts(r: [f32; 4]) -> [Pt; 4] {
    [[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]]
}

/// A rune carved into iron: lit lower lip, dark groove.
pub fn carved_rune(pm: &mut Pixmap, idx: usize, origin: Pt, h: f32) {
    let segs = geom::rune_segments(idx, origin, h);
    let w = (h * 0.16).max(1.1);
    // faint blood light leaking out of the groove
    for (a, b) in &segs {
        stroke(pm, &[*a, *b], false, pal::BLOOD_NEON.alpha(34), w + 2.2);
    }
    for (a, b) in &segs {
        stroke(
            pm,
            &[[a[0] + 0.6, a[1] + 0.7], [b[0] + 0.6, b[1] + 0.7]],
            false,
            pal::IRON_EDGE.alpha(120),
            w,
        );
    }
    for (a, b) in &segs {
        stroke(pm, &[*a, *b], false, Rgba::hex(0x050203), w);
    }
}

/// A rune drawn as neon strokes.
pub fn neon_rune(pm: &mut Pixmap, idx: usize, origin: Pt, h: f32, c: Rgba) {
    for (a, b) in geom::rune_segments(idx, origin, h) {
        glow(pm, &[a, b], false, c, (h * 0.11).max(1.0), 1.6, 3);
    }
}

/// Red terminal panel: near-black with a blood tint, a faint red grid,
/// light scanlines and an inner crimson glow.
pub fn terminal_panel(pm: &mut Pixmap, r: [f32; 4]) {
    let [x0, y0, x1, y1] = r;
    fill_gradient(
        pm,
        &rect_pts(r),
        [x0, y0],
        Rgba::hex(0x1a0307),
        [x0, y1],
        Rgba::hex(0x0b0204),
    );
    fill(pm, &rect_pts(r), pal::TERMINAL_FILL.alpha(0x30));
    let grid = pal::BLOOD_NEON.alpha(0x16);
    let step = 8.0;
    let mut x = x0 + 4.0;
    while x < x1 {
        rule(pm, [x.floor() + 0.5, y0], [x.floor() + 0.5, y1], grid, 1.0);
        x += step;
    }
    let mut y = y0 + 4.0;
    while y < y1 {
        rule(pm, [x0, y.floor() + 0.5], [x1, y.floor() + 0.5], grid, 1.0);
        y += step;
    }
    let mut y = y0 + 1.0;
    while y < y1 {
        rule(
            pm,
            [x0, y + 0.5],
            [x1, y + 0.5],
            Rgba::hex(0x000000).alpha(22),
            1.0,
        );
        y += 3.0;
    }
    // inner glow along the edge: stacked strokes, alpha falling inwards
    for (i, a) in [0.42f32, 0.2, 0.09].iter().enumerate() {
        let k = 0.5 + i as f32 * 1.5;
        stroke(
            pm,
            &rect_pts([x0 + k, y0 + k, x1 - k, y1 - k]),
            true,
            pal::CRIMSON.fade(*a),
            1.4,
        );
    }
}

/// Bright terminal corner brackets inside `r`.
pub fn brackets(pm: &mut Pixmap, r: [f32; 4], len: f32, c: Rgba, top: bool, bottom: bool) {
    let [x0, y0, x1, y1] = r;
    let mut corners = Vec::new();
    if top {
        corners.push(([x0 + 1.5, y0 + 1.5], [1.0, 1.0]));
        corners.push(([x1 - 1.5, y0 + 1.5], [-1.0, 1.0]));
    }
    if bottom {
        corners.push(([x0 + 1.5, y1 - 1.5], [1.0, -1.0]));
        corners.push(([x1 - 1.5, y1 - 1.5], [-1.0, -1.0]));
    }
    for (c0, d) in corners {
        let b = geom::bracket(c0, len, d);
        glow(pm, &b, false, c, 1.2, 1.5, 2);
    }
}

/// Blood along a top edge at `y`: a wet band plus seeded drips.
pub fn blood_drips(
    pm: &mut Pixmap,
    x0: f32,
    x1: f32,
    y: f32,
    seed: u32,
    max_len: f32,
    spacing: f32,
) {
    let edge = geom::blood_edge(seed, x0, x1, 3.0, 2.2);
    let mut band: Vec<Pt> = edge.iter().map(|(x, _)| [*x, y]).collect();
    band.extend(edge.iter().rev().map(|(x, t)| [*x, y + t]));
    fill_gradient(
        pm,
        &band,
        [x0, y],
        pal::BLOOD_DEEP,
        [x0, y + 4.0],
        pal::BLOOD,
    );
    for d in geom::blood_drips(seed, x0 + 6.0, x1 - 6.0, spacing, max_len) {
        let body = d.body(y + 1.0);
        let (c, r) = d.bulb(y + 1.0);
        fill_gradient(
            pm,
            &body,
            [d.x, y],
            pal::BLOOD_DEEP,
            [d.x, c[1]],
            pal::CRIMSON,
        );
        circle(pm, c, r, pal::CRIMSON);
        circle(pm, [c[0], c[1] + r * 0.25], r * 0.78, Rgba::hex(0xa3001a));
        if d.len > 6.0 {
            stroke(
                pm,
                &[
                    [d.x - d.width * 0.16, y + 2.0],
                    [d.x - d.width * 0.1, y + d.len * 0.7],
                ],
                false,
                pal::BLOOD_SHEEN.alpha(80),
                (d.width * 0.16).max(0.7),
            );
        }
        circle(
            pm,
            [c[0] - r * 0.35, c[1] - r * 0.3],
            (r * 0.3).max(0.55),
            pal::BLOOD_SHEEN.alpha(170),
        );
    }
}

/// A single blood drop hanging from `top` (a short run ending in a bulb).
pub fn blood_drop(pm: &mut Pixmap, top: Pt, len: f32, r: f32) {
    let body = [
        [top[0] - r * 0.55, top[1]],
        [top[0] + r * 0.55, top[1]],
        [top[0] + r * 0.4, top[1] + len],
        [top[0] - r * 0.4, top[1] + len],
    ];
    fill(pm, &body, pal::CRIMSON);
    circle(pm, [top[0], top[1] + len], r, pal::CRIMSON);
    circle(
        pm,
        [top[0], top[1] + len + r * 0.2],
        r * 0.75,
        Rgba::hex(0xa3001a),
    );
    circle(
        pm,
        [top[0] - r * 0.35, top[1] + len - r * 0.3],
        (r * 0.32).max(0.5),
        pal::BLOOD_SHEEN.alpha(200),
    );
}

/// Small horned skull, `s` ≈ skull height; `eye` lights the sockets.
pub fn skull(pm: &mut Pixmap, c: Pt, s: f32, eye: Rgba) {
    let k = geom::skull(c, s);
    for h in &k.horns {
        let sh: Vec<Pt> = h.iter().map(|q| [q[0] + 1.0, q[1] + 1.0]).collect();
        tapered(pm, &sh, k.horn_width, 0.3, Rgba::hex(0x000000).alpha(170));
        tapered(pm, h, k.horn_width, 0.3, pal::IRON_EDGE);
        tapered(
            pm,
            h,
            k.horn_width * 0.45,
            0.15,
            pal::BONE_SHADOW.lerp(pal::BONE, 0.5),
        );
    }
    let (cc, cr) = k.cranium;
    circle(
        pm,
        [cc[0] + 1.0, cc[1] + 1.2],
        cr,
        Rgba::hex(0x000000).alpha(170),
    );
    let jaw = k.jaw;
    fill(
        pm,
        &rect_pts([jaw[0] + 1.0, jaw[1] + 1.2, jaw[2] + 1.0, jaw[3] + 1.2]),
        Rgba::hex(0x000000).alpha(170),
    );
    circle(pm, cc, cr, pal::BONE_SHADOW);
    circle(
        pm,
        [cc[0] - cr * 0.12, cc[1] - cr * 0.14],
        cr * 0.86,
        pal::BONE,
    );
    fill(
        pm,
        &geom::chamfer_rect(jaw, s * 0.08),
        pal::BONE_SHADOW.lerp(pal::BONE, 0.6),
    );
    for (ec, er) in k.eyes {
        circle(pm, ec, er, Rgba::hex(0x0a0204));
        circle(pm, ec, er * 1.5, eye.alpha(40));
        circle(pm, ec, (er * 0.42).max(0.6), eye);
    }
    fill(pm, &k.nose, Rgba::hex(0x0a0204));
    for (a, b) in &k.teeth {
        rule(pm, *a, *b, pal::BONE_SHADOW, (s * 0.05).max(0.7));
    }
}

/// The horns + spade-tail sigil (abstract emblem) in neon `c`.
pub fn sigil(pm: &mut Pixmap, c: Pt, s: f32, col: Rgba, core: Rgba) {
    let g = geom::sigil(c, s);
    circle(pm, c, s * 0.42, col.alpha(18));
    circle(pm, c, s * 0.26, col.alpha(22));
    let w = g.horn_width;
    for h in &g.horns {
        for (k, a) in [(3.0, 0.10), (2.0, 0.18)] {
            tapered(pm, h, w * k, w * k * 0.35, col.fade(a));
        }
        tapered(pm, h, w, 0.5, col);
        tapered(pm, h, w * 0.4, 0.2, core);
    }
    let (rc, rr) = g.ring;
    for (k, a) in [(5.0, 0.12), (3.0, 0.25)] {
        ring(pm, rc, rr, col.fade(a), k);
    }
    ring(pm, rc, rr, col, 1.5);
    ring(pm, rc, rr, core, 0.6);
    fill(pm, &g.eye, pal::BLOOD_NEON);
    glow(pm, &g.tail, false, col, 1.4, 2.0, 3);
    stroke(pm, &g.tail, false, core, 0.6);
    fill(pm, &g.spade, col);
    stroke(pm, &g.spade, true, core, 0.6);
}

/// Serrated blade divider from `x0` (hilt) to `x1` (tip) on `cy`; `hot`
/// makes it a glowing red blade (the Quit fence).
pub fn serrated_blade(pm: &mut Pixmap, x0: f32, x1: f32, cy: f32, hot: bool) {
    let h = 4.0;
    let hilt = 9.0;
    let bx = x0 + hilt;
    let outline = geom::blade_outline(bx, x1, cy, h);
    // teeth along the spine: replace the flat top with a saw edge
    let top = cy - h * 0.5;
    let mut poly: Vec<Pt> = vec![[bx, top]];
    let tip_start = outline[1][0];
    let mut x = bx + 3.0;
    let mut up = true;
    while x < tip_start - 2.0 {
        poly.push([x, if up { top - 1.6 } else { top }]);
        up = !up;
        x += 2.5;
    }
    poly.push(outline[1]);
    poly.extend_from_slice(&outline[2..]);
    if hot {
        glow(pm, &poly, true, pal::BLOOD_NEON, 0.8, 3.0, 4);
        fill_gradient(pm, &poly, [bx, cy], pal::BLOOD, [x1, cy], pal::BLOOD_NEON);
        stroke(
            pm,
            &geom::blade_edge(bx, x1, cy, h),
            false,
            pal::BLOOD_SHEEN,
            0.8,
        );
    } else {
        fill_gradient(
            pm,
            &poly,
            [bx, top],
            pal::IRON_EDGE,
            [bx, cy + h * 0.5],
            Rgba::hex(0x2a2730),
        );
        stroke(
            pm,
            &geom::blade_edge(bx, x1, cy, h),
            false,
            pal::BLOOD_NEON.alpha(170),
            0.8,
        );
    }
    // hilt: guard + grip
    let guard = [bx - 2.0, cy - h * 0.95, bx, cy + h * 0.95];
    fill(
        pm,
        &rect_pts(guard),
        if hot { pal::CRIMSON } else { pal::IRON_HI },
    );
    let grip = [x0, cy - 1.2, bx - 2.0, cy + 1.2];
    fill(
        pm,
        &rect_pts(grip),
        if hot { pal::BLOOD } else { pal::BLOOD_DEEP },
    );
    circle(
        pm,
        [x0, cy],
        1.4,
        if hot { pal::CRIMSON } else { pal::IRON_HI },
    );
}

/// Knife-blade hover highlight across row `r` (`[x0, y0, x1, y1]`), blade
/// coloured from `edge_col` (bright edge) over `deep` (body), with a blood
/// drop falling off the tip.
pub fn knife_hover(pm: &mut Pixmap, r: [f32; 4], deep: Rgba, edge_col: Rgba, core: Rgba) {
    let [x0, y0, x1, y1] = r;
    let cy = (y0 + y1) * 0.5;
    let h = (y1 - y0) - 7.0;
    let bx = x0 + 6.0;
    let outline = geom::blade_outline(bx, x1, cy, h);
    // soft glow around the whole blade
    glow(pm, &outline, true, edge_col, 0.6, 2.4, 3);
    fill_gradient(
        pm,
        &outline,
        [bx, cy],
        deep.alpha(245),
        [x1, cy],
        deep.lerp(edge_col, 0.45).alpha(225),
    );
    // lit upper half (steel sheen) fading towards the tip
    let upper = [
        outline[0],
        outline[1],
        [outline[1][0], cy - h * 0.12],
        [outline[0][0], cy - h * 0.12],
    ];
    fill_gradient(
        pm,
        &upper,
        [bx, cy],
        core.alpha(34),
        [x1, cy],
        core.alpha(6),
    );
    stroke(
        pm,
        &[outline[0], outline[1], outline[2]],
        false,
        core.alpha(150),
        1.0,
    );
    let e = geom::blade_edge(bx, x1, cy, h);
    glow(pm, &e, false, edge_col, 1.1, 2.2, 3);
    stroke(pm, &e, false, core.alpha(200), 0.5);
    // hilt
    fill(
        pm,
        &rect_pts([bx - 3.0, cy - h * 0.62, bx, cy + h * 0.62]),
        pal::IRON_EDGE,
    );
    rule(
        pm,
        [bx - 0.5, cy - h * 0.62],
        [bx - 0.5, cy + h * 0.62],
        pal::IRON_HI,
        1.0,
    );
    fill(
        pm,
        &rect_pts([x0, cy - h * 0.2, bx - 3.0, cy + h * 0.2]),
        pal::BLOOD_DEEP,
    );
    // blood drop off the belly near the tip
    let b3 = outline[3];
    blood_drop(
        pm,
        [b3[0], b3[1] - 0.5],
        (y1 - 2.5 - b3[1] - 2.0).max(1.5),
        1.7,
    );
}

/// ▸ / ▾ as a small glowing blade tip.
pub fn arrow(pm: &mut Pixmap, cx: f32, cy: f32, open: bool, col: Rgba, core: Rgba) {
    let pts: [Pt; 3] = if open {
        [[cx - 4.5, cy - 2.5], [cx + 4.5, cy - 2.5], [cx, cy + 4.0]]
    } else {
        [[cx - 2.5, cy - 4.5], [cx + 4.0, cy], [cx - 2.5, cy + 4.5]]
    };
    glow(pm, &pts, true, col, 0.8, 1.8, 2);
    fill(pm, &pts, col);
    stroke(pm, &pts, true, core.alpha(160), 0.5);
}

/// Radio: a carved diamond socket, lit with the accent when on.
pub fn radio(pm: &mut Pixmap, x: f32, cy: f32, on: bool, col: Rgba, core: Rgba) {
    let c = [x + 7.0, cy];
    let d = |r: f32| -> [Pt; 4] {
        [
            [c[0], c[1] - r],
            [c[0] + r, c[1]],
            [c[0], c[1] + r],
            [c[0] - r, c[1]],
        ]
    };
    fill(pm, &d(6.5), Rgba::hex(0x050203));
    stroke(pm, &d(6.5), true, pal::IRON_EDGE, 1.2);
    if on {
        glow(pm, &d(3.4), true, col, 0.6, 2.0, 2);
        fill(pm, &d(3.4), col);
        fill(pm, &d(1.3), core);
    }
}

/// Checkbox: a chamfered iron socket with a neon tick.
pub fn checkbox(pm: &mut Pixmap, x: f32, cy: f32, on: bool, col: Rgba, core: Rgba) {
    let r = [x + 0.5, cy - 6.5, x + 13.5, cy + 6.5];
    fill(pm, &geom::chamfer_rect(r, 3.0), Rgba::hex(0x050203));
    stroke(pm, &geom::chamfer_rect(r, 3.0), true, pal::IRON_EDGE, 1.2);
    if on {
        let tick = [[x + 3.2, cy], [x + 6.0, cy + 3.2], [x + 11.0, cy - 3.6]];
        glow(pm, &tick, false, col, 1.8, 1.8, 2);
        stroke(pm, &tick, false, core, 0.7);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn painters_stay_inside_and_draw() {
        let mut pm = Pixmap::new(120, 60).unwrap();
        iron_frame(&mut pm, [0.0, 0.0, 120.0, 60.0], 7.0, 12.0, 3);
        terminal_panel(&mut pm, [7.0, 7.0, 113.0, 48.0]);
        blood_drips(&mut pm, 5.0, 115.0, 0.0, 9, 12.0, 20.0);
        skull(&mut pm, [11.0, 13.0], 12.0, pal::BLOOD_NEON);
        sigil(&mut pm, [30.0, 24.0], 26.0, pal::PINK, pal::TEXT);
        serrated_blade(&mut pm, 12.0, 108.0, 40.0, true);
        knife_hover(
            &mut pm,
            [9.0, 20.0, 111.0, 46.0],
            Accent::LILLITH.deep,
            pal::PINK,
            pal::TEXT,
        );
        let a = |x: u32, y: u32| pm.data()[((y * 120 + x) * 4 + 3) as usize];
        assert_eq!(a(60, 30), 255);
        // chamfered corner stays transparent
        assert_eq!(a(0, 0), 0);
    }
}
