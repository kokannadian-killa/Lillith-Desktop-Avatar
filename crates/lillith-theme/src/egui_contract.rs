//! The shared **Soul Contract** Yes/No popup (feature `egui`).
//!
//! Every egui surface that holds one of Lillith's gated actions (chat
//! bubble: skill / file writes and shell; settings Build tab: scaffold /
//! build / run) raises this one modal instead of drawing its own buttons.
//! It is a DemonOS hellforge plate (crimson and ember on near-black): an
//! iron-banded slab with blood dripping off the top edge, a horned skull
//! lit by a forge glow, the action heading on a bleeding blade, the summary,
//! the touched path, a terminal slab with what would be written / run, and
//! two plates: **REFUSE — NO** and **SIGN IN BLOOD — YES**.
//!
//! Colours are fixed crimson / ember / bone; the avatar accent (often
//! magenta) is deliberately not used here so a contract always reads as a
//! warning.
//!
//! The popup only *reports* the click. The caller turns it into
//! `SoulContract::answer` (Yes → run + audit `CONFIRM`, No → discard + audit
//! `DENY`). `Esc` counts as **No**; a click on the dimmed backdrop does
//! nothing (a stray click must neither run nor silently drop the action);
//! there is no keyboard shortcut for Yes.
//!
//! The text is plain strings ([`ContractView`]) so this crate stays free of
//! `lillith-core`; `ContractPrompt` maps onto it field by field.

use crate::egui_theme::{
    additive, blade_divider, blood_drips, c32, glow_blob, gradient_rect, iron_frame, neon_line,
    neon_rect,
};
use crate::geom::{self, Pt};
use crate::palette::{self as pal, Rgba};
use egui::{
    Align2, Color32, Context, CornerRadius, FontFamily, FontId, Id, Painter, Pos2, Rect, Sense,
    Stroke, StrokeKind, Vec2,
};

/// Who asked for the action; picks the summary wording.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Initiator {
    /// Lillith proposed it (chat tools, skills): "She wants to …".
    #[default]
    Her,
    /// Ko started it himself (e.g. a Build tab button): "You asked to …".
    You,
}

/// What the popup shows. Mirrors `lillith_core::soul_contract::ContractPrompt`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ContractView<'a> {
    /// Upper-case heading (`WRITE FILE`, `RUN SHELL`, …).
    pub title: &'a str,
    /// One-line description of the action.
    pub summary: &'a str,
    /// The path the action touches, if any.
    pub target: Option<&'a str>,
    /// What would be written / run (already capped by the caller).
    pub detail: Option<&'a str>,
    /// Something is still running: Yes is shown but disabled.
    pub busy: bool,
    /// Who asked for it (default: Lillith).
    pub initiator: Initiator,
}

/// The sentence under the heading: "She wants to …." when Lillith proposed
/// the action, "You asked to …." when Ko started it himself.
pub fn summary_line(view: &ContractView<'_>) -> String {
    let what = lower_first(view.summary);
    match view.initiator {
        Initiator::Her => format!("She wants to {what}."),
        Initiator::You => format!("You asked to {what}."),
    }
}

/// Clear space between the horn tips and the top of the plate's inner
/// frame (logical px).
pub const SKULL_TOP_GAP: f32 = 6.0;

/// How far the drawn skull reaches above and below its centre at size `s`:
/// `(up, down)`, measured from the real horn outline (tips included) and the
/// jaw, so the header can be laid out without the horns hitting the frame.
fn skull_extent(s: f32) -> (f32, f32) {
    let k = crate::geom::death_skull([0.0, 0.0], s);
    (-k.top(), k.bottom())
}

/// Header block for a skull of size `s`: `(centre offset from the inner
/// top, total height down to where the title starts)`.
fn skull_header(s: f32) -> (f32, f32) {
    let (up, down) = skull_extent(s);
    let centre = SKULL_TOP_GAP + up;
    (centre, centre + down + 0.07 * s + 6.0)
}

/// The click (or `Esc`) the popup saw this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractChoice {
    /// Sign: run the action.
    Yes,
    /// Refuse: discard the action (also `Esc`).
    No,
}

/// Widest the plate gets (logical px).
pub const MAX_WIDTH: f32 = 440.0;
/// Narrowest the plate gets (logical px); the 300 px chat bubble fits.
pub const MIN_WIDTH: f32 = 220.0;
/// Most preview lines shown before the slab is cut with "…".
pub const DETAIL_LINES: usize = 9;

/// Plate width for a window `screen_w` px wide.
pub fn plate_width(screen_w: f32) -> f32 {
    (screen_w - 16.0).clamp(MIN_WIDTH, MAX_WIDTH)
}

/// `detail` cut to at most `max_lines` lines (and 120 chars per line), with
/// a closing "…" line when anything was dropped.
pub fn clip_detail(detail: &str, max_lines: usize) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut cut = false;
    for (i, line) in detail.lines().enumerate() {
        if i >= max_lines {
            cut = true;
            break;
        }
        if line.chars().count() > 120 {
            out.push(line.chars().take(119).chain(['…']).collect());
        } else {
            out.push(line.to_string());
        }
    }
    if cut {
        out.push("…".into());
    }
    out.join("\n")
}

/// The family if it is installed in this context, else `fallback`
/// (the chat bubble runs on egui's default fonts).
fn family_or(ctx: &Context, name: &str, fallback: FontFamily) -> FontFamily {
    let fam = FontFamily::Name(name.into());
    if ctx.fonts(|f| f.families().contains(&fam)) {
        fam
    } else {
        fallback
    }
}

struct Fonts {
    head: FontId,
    label: FontId,
    body: FontId,
    mono: FontId,
    button: FontId,
}

impl Fonts {
    fn new(ctx: &Context, narrow: bool) -> Self {
        let carved = family_or(ctx, "carved", FontFamily::Proportional);
        let display = family_or(ctx, "display", FontFamily::Monospace);
        let bold = family_or(ctx, "bold", FontFamily::Proportional);
        let k = if narrow { 0.88 } else { 1.0 };
        Self {
            head: FontId::new(22.0 * k, carved),
            label: FontId::new(11.0 * k, display.clone()),
            body: FontId::new(14.0 * k, FontFamily::Proportional),
            mono: FontId::new(11.5 * k, FontFamily::Monospace),
            button: FontId::new(12.0 * k, bold),
        }
    }
}

/// Raises the Soul Contract popup over the whole window for one frame and
/// returns the choice made this frame, if any. Call it every frame while an
/// action is pending; stop calling it once you've acted on the answer.
pub fn soul_contract_popup(
    ctx: &Context,
    id: Id,
    view: &ContractView<'_>,
) -> Option<ContractChoice> {
    let screen = ctx.screen_rect();
    let w = plate_width(screen.width());
    let narrow = w < 340.0;
    let t = ctx.input(|i| i.time) as f32;
    let modal = egui::Modal::new(id)
        .frame(egui::Frame::NONE)
        .backdrop_color(Color32::from_rgba_premultiplied(4, 0, 2, 200));
    let resp = modal.show(ctx, |ui| plate(ui, id, w, narrow, t, view));
    ctx.request_repaint_after(std::time::Duration::from_millis(60));
    let esc = resp.is_top_modal
        && !resp.any_popup_open
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    if esc {
        return Some(ContractChoice::No);
    }
    resp.inner
}

fn plate(
    ui: &mut egui::Ui,
    id: Id,
    w: f32,
    narrow: bool,
    t: f32,
    view: &ContractView<'_>,
) -> Option<ContractChoice> {
    let ctx = ui.ctx().clone();
    let f = Fonts::new(&ctx, narrow);
    let frame_t = if narrow { 7.0 } else { 9.0 };
    let pad = if narrow { 12.0 } else { 18.0 };
    let inner_w = w - 2.0 * (frame_t + pad);
    let painter = ui.painter().clone();

    // --- measure ---------------------------------------------------------
    let skull_s = if narrow { 36.0 } else { 46.0 };
    let (skull_c, skull_h) = skull_header(skull_s);
    let head = painter.layout_no_wrap("SOUL CONTRACT".into(), f.head.clone(), c32(pal::TEXT));
    let label = painter.layout_no_wrap(
        if view.title.is_empty() {
            "PACT".into()
        } else {
            view.title.to_string()
        },
        f.label.clone(),
        c32(pal::EMBER_CORE),
    );
    let summary = painter.layout(summary_line(view), f.body.clone(), c32(pal::TEXT), inner_w);
    let target = view
        .target
        .filter(|s| !s.is_empty())
        .map(|p| painter.layout(format!("⛧ {p}"), f.mono.clone(), c32(pal::EMBER), inner_w));
    let detail = view.detail.filter(|s| !s.trim().is_empty()).map(|d| {
        painter.layout(
            clip_detail(d, DETAIL_LINES),
            f.mono.clone(),
            c32(pal::BONE),
            inner_w - 16.0,
        )
    });
    let oath = painter.layout(
        "Nothing happens without your mark. Every answer is written to the audit log.".into(),
        FontId::new(f.body.size * 0.78, FontFamily::Proportional),
        c32(pal::TEXT_MUTE),
        inner_w,
    );
    let btn_h = if narrow { 30.0 } else { 34.0 };
    let mut h = frame_t + skull_h + head.size().y + 8.0 + 14.0 + 10.0;
    h += summary.size().y + 8.0;
    if let Some(g) = &target {
        h += g.size().y + 8.0;
    }
    if let Some(g) = &detail {
        h += g.size().y + 16.0 + 10.0;
    }
    h += oath.size().y + 12.0 + btn_h + pad * 0.8 + frame_t;

    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    let p = ui.painter_at(rect.expand(24.0));

    // --- slab ---------------------------------------------------------------
    // outer forge glow
    for i in 0..5 {
        let e = 2.0 + i as f32 * 4.0;
        p.rect_stroke(
            rect.expand(e),
            CornerRadius::same(3),
            Stroke::new(4.0, additive(pal::CRIMSON, 0.10 - i as f32 * 0.018)),
            StrokeKind::Outside,
        );
    }
    gradient_rect(&p, rect, c32(Rgba::hex(0x14060a)), c32(Rgba::hex(0x060103)));
    let inner = iron_frame(&p, rect, frame_t, 0x5011);
    // ember heat rising from the bottom of the slab
    gradient_rect(
        &p,
        Rect::from_min_max(
            Pos2::new(inner.left(), inner.bottom() - inner.height() * 0.45),
            inner.right_bottom(),
        ),
        Color32::TRANSPARENT,
        additive(pal::EMBER, 0.10),
    );
    let flick = 0.5 + 0.5 * (t * 2.3).sin() * (t * 1.3).cos();
    neon_rect(
        &p,
        inner.shrink(1.5),
        pal::BLOOD_NEON.alpha(170),
        1.0,
        3.0 + flick,
    );
    corner_spikes(&p, inner.shrink(4.0), if narrow { 9.0 } else { 12.0 });
    blood_drips(
        &p,
        rect.left() + 18.0,
        rect.right() - 18.0,
        rect.top() + 1.0,
        0x666,
        if narrow { 14.0 } else { 22.0 },
        30.0,
    );

    // --- header: death-metal skull over a smouldering forge ------------------
    let cx = rect.center().x;
    let mut y = inner.top();
    let sc = Pos2::new(cx, y + skull_c);
    glow_blob(
        &p,
        sc + Vec2::new(0.0, skull_s * 0.25),
        skull_s * 1.15,
        pal::BLOOD,
        0.9 + 0.4 * flick,
    );
    glow_blob(&p, sc, skull_s * 0.75, pal::CRIMSON, 0.55);
    smoke_and_embers(&p, sc + Vec2::new(0.0, skull_s * 0.55), skull_s, t);
    crate::egui_theme::death_skull(
        &p,
        sc,
        skull_s,
        pal::BLOOD_NEON.lerp(pal::EMBER, 0.35 + flick * 0.3),
        flick,
    );
    y += skull_h;

    // title with a crimson neon bleed
    let hp = Pos2::new(cx - head.size().x / 2.0, y);
    for (dx, dy, a) in [
        (-1.5, 0.0, 0.35),
        (1.5, 0.0, 0.35),
        (0.0, -1.5, 0.3),
        (0.0, 1.5, 0.3),
    ] {
        p.galley_with_override_text_color(
            hp + Vec2::new(dx, dy),
            head.clone(),
            additive(pal::BLOOD_NEON, a),
        );
    }
    p.galley_with_override_text_color(
        hp + Vec2::new(0.0, 2.0),
        head.clone(),
        c32(Rgba::hex(0x000000).alpha(200)),
    );
    p.galley_with_override_text_color(
        hp,
        head.clone(),
        c32(pal::CRIMSON.lerp(pal::BLOOD_SHEEN, 0.35)),
    );
    y += head.size().y + 8.0;

    // action heading on a bleeding blade
    let lx = inner.left() + pad;
    let rx = inner.right() - pad;
    blade_divider(&p, lx, rx, y + 3.0);
    let lw = label.size().x + 18.0;
    let lr = Rect::from_center_size(Pos2::new(cx, y + 5.0), Vec2::new(lw, label.size().y + 6.0));
    p.rect_filled(lr, CornerRadius::same(2), c32(Rgba::hex(0x0a0204)));
    p.rect_stroke(
        lr,
        CornerRadius::same(2),
        Stroke::new(1.0, c32(pal::EMBER.alpha(200))),
        StrokeKind::Inside,
    );
    p.galley(
        lr.center() - label.size() / 2.0,
        label.clone(),
        c32(pal::EMBER_CORE),
    );
    y += 14.0 + 10.0;

    // --- body -------------------------------------------------------------------
    p.galley(Pos2::new(lx, y), summary.clone(), c32(pal::TEXT));
    y += summary.size().y + 8.0;
    if let Some(g) = &target {
        p.galley(Pos2::new(lx, y), g.clone(), c32(pal::EMBER));
        y += g.size().y + 8.0;
    }
    if let Some(g) = &detail {
        let r = Rect::from_min_size(Pos2::new(lx, y), Vec2::new(inner_w, g.size().y + 16.0));
        terminal_slab(&p, r);
        p.galley(r.min + Vec2::new(8.0, 8.0), g.clone(), c32(pal::BONE));
        y += r.height() + 10.0;
    }
    p.galley(Pos2::new(lx, y), oath.clone(), c32(pal::TEXT_MUTE));
    y += oath.size().y + 12.0;

    // --- the two plates -------------------------------------------------------
    let gap = 10.0;
    let no_w = (inner_w - gap) * 0.42;
    let yes_w = inner_w - gap - no_w;
    let no_r = Rect::from_min_size(Pos2::new(lx, y), Vec2::new(no_w, btn_h));
    let yes_r = Rect::from_min_size(Pos2::new(lx + no_w + gap, y), Vec2::new(yes_w, btn_h));
    let no = ui.interact(no_r, id.with("no"), Sense::click());
    let yes = ui.interact(
        yes_r,
        id.with("yes"),
        if view.busy {
            Sense::hover()
        } else {
            Sense::click()
        },
    );
    let (no_txt, yes_txt) = if narrow {
        (
            "✕ REFUSE",
            if view.busy {
                "FORGING…"
            } else {
                "✓ SIGN · YES"
            },
        )
    } else {
        (
            "✕ REFUSE — NO",
            if view.busy {
                "THE FORGE IS BUSY…"
            } else {
                "✓ SIGN IN BLOOD — YES"
            },
        )
    };
    refuse_plate(&p, no_r, no.hovered(), no_txt, &f.button);
    sign_plate(
        &p,
        yes_r,
        yes.hovered() && !view.busy,
        view.busy,
        flick,
        yes_txt,
        &f.button,
    );
    if no.hovered() || (yes.hovered() && !view.busy) {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    if no.clicked() {
        Some(ContractChoice::No)
    } else if yes.clicked() && !view.busy {
        Some(ContractChoice::Yes)
    } else {
        None
    }
}

/// "Write skill …" → "write skill …" so it reads after "She wants to" /
/// "You asked to".
fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Short iron spikes in the four inner corners.
fn corner_spikes(p: &Painter, r: Rect, s: f32) {
    let corners = [
        (r.left_top(), [1.0, 1.0]),
        (r.right_top(), [-1.0, 1.0]),
        (r.left_bottom(), [1.0, -1.0]),
        (r.right_bottom(), [-1.0, -1.0]),
    ];
    for (c, d) in corners {
        let tri: [Pt; 3] = [
            [c.x, c.y],
            [c.x + d[0] * s, c.y + d[1] * s * 0.28],
            [c.x + d[0] * s * 0.28, c.y + d[1] * s],
        ];
        let mut poly = tri.to_vec();
        if !geom::is_convex(&poly) {
            poly.reverse();
        }
        crate::egui_theme::fill_convex(p, &poly, c32(pal::IRON_MID));
        neon_line(p, &[tri[1], tri[0], tri[2]], pal::CRIMSON, 1.0, 2.0, false);
    }
}

/// Black smoke rolling up behind the skull, with sparse embers drifting out.
fn smoke_and_embers(p: &Painter, base: Pos2, s: f32, t: f32) {
    // slow black smoke rolling up behind the skull, breaking up the glow
    for i in 0..9 {
        let fi = i as f32;
        let ph = (t * 0.08 + fi * 0.173).fract();
        let x = base.x + ((fi * 2.39).sin() * 0.75 + (t * 0.3 + fi).sin() * 0.06) * s;
        let y = base.y - ph * s * 1.25;
        let r = s * (0.16 + 0.22 * ph);
        let a = (1.0 - ph) * ph * 4.0;
        for k in 0..3 {
            p.circle_filled(
                Pos2::new(x, y),
                r * (1.0 - k as f32 * 0.28),
                c32(Rgba::hex(0x050003).fade(0.12 * a)),
            );
        }
    }
    // sparse embers drifting up out of it
    let mut rng = geom::Rng::new(0x13);
    for _ in 0..16 {
        let (u, speed, off) = (rng.range(-0.8, 0.8), rng.range(0.12, 0.3), rng.f());
        let ph = (t * speed + off).fract();
        let x = base.x + (u + (t * 1.7 + off * 9.0).sin() * 0.05) * s;
        let y = base.y + s * 0.1 - ph * s * 1.4;
        let a = (1.0 - ph) * (ph * 6.0).min(1.0);
        let col = if off > 0.5 {
            pal::EMBER
        } else {
            pal::BLOOD_NEON
        };
        p.circle_filled(Pos2::new(x, y), 0.6 + off, additive(col, 0.9 * a));
        p.circle_filled(Pos2::new(x, y), 2.0 + off * 1.5, additive(col, 0.18 * a));
    }
}

/// The dark-red terminal slab behind the preview.
fn terminal_slab(p: &Painter, r: Rect) {
    p.rect_filled(r, CornerRadius::same(2), c32(Rgba::hex(0x0a0204)));
    p.rect_filled(r, CornerRadius::same(2), c32(pal::BLOOD_DRIED.alpha(120)));
    let mut y = r.top() + 3.0;
    while y < r.bottom() {
        p.line_segment(
            [Pos2::new(r.left() + 1.0, y), Pos2::new(r.right() - 1.0, y)],
            Stroke::new(1.0, c32(pal::BLOOD_NEON.alpha(10))),
        );
        y += 3.0;
    }
    p.rect_stroke(
        r,
        CornerRadius::same(2),
        Stroke::new(1.0, c32(pal::BLOOD.alpha(220))),
        StrokeKind::Inside,
    );
    p.line_segment(
        [
            r.left_top() + Vec2::new(0.0, 0.5),
            r.right_top() + Vec2::new(0.0, 0.5),
        ],
        Stroke::new(1.0, c32(pal::CRIMSON.alpha(200))),
    );
}

/// The No plate: cold black iron, crimson lettering.
fn refuse_plate(p: &Painter, r: Rect, hot: bool, text: &str, font: &FontId) {
    gradient_rect(p, r, c32(pal::IRON_MID), c32(Rgba::hex(0x0b0709)));
    p.rect_stroke(
        r,
        CornerRadius::ZERO,
        Stroke::new(1.0, c32(if hot { pal::CRIMSON } else { pal::IRON_EDGE })),
        StrokeKind::Inside,
    );
    if hot {
        neon_rect(p, r.shrink(1.0), pal::CRIMSON, 1.0, 3.0);
    }
    plate_text(
        p,
        r,
        text,
        font,
        if hot { pal::BLOOD_SHEEN } else { pal::CRIMSON },
    );
}

/// The Yes plate: molten blood with an ember edge.
fn sign_plate(p: &Painter, r: Rect, hot: bool, busy: bool, flick: f32, text: &str, font: &FontId) {
    if busy {
        gradient_rect(p, r, c32(pal::BLOOD_DEEP), c32(pal::BLOOD_DRIED));
        p.rect_stroke(
            r,
            CornerRadius::ZERO,
            Stroke::new(1.0, c32(pal::BLOOD)),
            StrokeKind::Inside,
        );
        plate_text(p, r, text, font, pal::TEXT_MUTE);
        return;
    }
    let top = if hot {
        pal::CRIMSON.lerp(pal::BLOOD_NEON, 0.5)
    } else {
        pal::CRIMSON
    };
    gradient_rect(p, r, c32(top), c32(pal::BLOOD_DEEP));
    // molten seam along the bottom
    p.line_segment(
        [
            r.left_bottom() + Vec2::new(2.0, -2.0),
            r.right_bottom() + Vec2::new(-2.0, -2.0),
        ],
        Stroke::new(1.5, c32(pal::EMBER.alpha(200))),
    );
    neon_rect(
        p,
        r.shrink(0.5),
        if hot { pal::EMBER } else { pal::BLOOD_NEON },
        1.0,
        if hot { 5.0 } else { 2.5 + flick * 1.5 },
    );
    plate_text(
        p,
        r,
        text,
        font,
        if hot {
            Rgba::hex(0xffffff)
        } else {
            pal::EMBER_CORE
        },
    );
}

fn plate_text(p: &Painter, r: Rect, text: &str, font: &FontId, color: Rgba) {
    p.text(
        r.center() + Vec2::new(0.0, 1.0),
        Align2::CENTER_CENTER,
        text,
        font.clone(),
        c32(Rgba::hex(0x000000).alpha(200)),
    );
    p.text(
        r.center(),
        Align2::CENTER_CENTER,
        text,
        font.clone(),
        c32(color),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> ContractView<'static> {
        ContractView {
            title: "WRITE FILE",
            summary: "Write file /tmp/x.txt (5 bytes)",
            target: Some("/tmp/x.txt"),
            detail: Some("hello\nworld"),
            busy: false,
            initiator: Initiator::Her,
        }
    }

    #[test]
    fn summary_wording_follows_who_asked() {
        let her = view();
        assert_eq!(
            summary_line(&her),
            "She wants to write file /tmp/x.txt (5 bytes)."
        );
        let you = ContractView {
            initiator: Initiator::You,
            ..view()
        };
        assert_eq!(
            summary_line(&you),
            "You asked to write file /tmp/x.txt (5 bytes)."
        );
        assert_eq!(ContractView::default().initiator, Initiator::Her);
    }

    #[test]
    fn horn_tips_clear_the_top_of_the_plate() {
        for s in [36.0, 46.0] {
            let (centre, height) = skull_header(s);
            let k = crate::geom::death_skull([0.0, centre], s);
            for h in &k.horns {
                for q in crate::geom::tapered_quads(h, k.horn_width, k.horn_tip) {
                    for pt in q {
                        assert!(
                            pt[1] >= SKULL_TOP_GAP - 0.01,
                            "size {s}: horn reaches y={} (gap {SKULL_TOP_GAP})",
                            pt[1]
                        );
                    }
                }
            }
            assert!((k.top() - SKULL_TOP_GAP).abs() < 0.01, "size {s}: gap");
            // The title starts below the chin.
            assert!(height > k.bottom());
        }
    }

    #[test]
    fn width_fits_bubble_and_caps_wide_windows() {
        assert_eq!(plate_width(300.0), 284.0);
        assert_eq!(plate_width(1600.0), MAX_WIDTH);
        assert_eq!(plate_width(100.0), MIN_WIDTH);
    }

    #[test]
    fn detail_is_clipped_by_lines_and_width() {
        let long = (0..20).fold(String::new(), |mut acc, i| {
            acc.push_str(&format!("line {i}\n"));
            acc
        });
        let c = clip_detail(&long, 3);
        assert_eq!(c, "line 0\nline 1\nline 2\n…");
        let wide = "x".repeat(300);
        let c = clip_detail(&wide, 3);
        assert_eq!(c.chars().count(), 120);
        assert!(c.ends_with('…'));
        assert_eq!(clip_detail("a\nb", 9), "a\nb");
    }

    #[test]
    fn lower_first_reads_after_she_wants_to() {
        assert_eq!(lower_first("Write file x"), "write file x");
        assert_eq!(lower_first(""), "");
    }

    fn frame(ctx: &Context, input: egui::RawInput, v: &ContractView<'_>) -> Option<ContractChoice> {
        let mut out = None;
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |_| {});
            out = soul_contract_popup(ctx, Id::new("contract-test"), v);
        });
        out
    }

    fn raw(size: Vec2, events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            events,
            ..Default::default()
        }
    }

    #[test]
    fn paints_headless_with_default_and_themed_fonts() {
        for themed in [false, true] {
            let ctx = Context::default();
            if themed {
                crate::egui_theme::install(&ctx);
            }
            for size in [Vec2::new(300.0, 380.0), Vec2::new(1000.0, 700.0)] {
                let v = view();
                assert_eq!(frame(&ctx, raw(size, vec![]), &v), None);
                let busy = ContractView {
                    busy: true,
                    detail: None,
                    target: None,
                    ..view()
                };
                assert_eq!(frame(&ctx, raw(size, vec![]), &busy), None);
            }
        }
    }

    #[test]
    fn escape_means_no() {
        let ctx = Context::default();
        let size = Vec2::new(800.0, 600.0);
        let v = view();
        let _ = frame(&ctx, raw(size, vec![]), &v);
        let esc = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        assert_eq!(
            frame(&ctx, raw(size, vec![esc]), &v),
            Some(ContractChoice::No)
        );
    }

    /// Clicks at `pos` over two frames (press, release) and returns the choice.
    fn click(ctx: &Context, size: Vec2, v: &ContractView<'_>, pos: Pos2) -> Option<ContractChoice> {
        let _ = frame(ctx, raw(size, vec![egui::Event::PointerMoved(pos)]), v);
        let press = egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        };
        let release = egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        };
        let a = frame(ctx, raw(size, vec![press]), v);
        let b = frame(ctx, raw(size, vec![release]), v);
        a.or(b)
    }

    /// Finds the plate rects by reading the ids' last interaction rects.
    fn warm(ctx: &Context, size: Vec2, v: &ContractView<'_>) {
        // Areas spend their first frame in a sizing pass; let layout settle.
        for _ in 0..3 {
            let _ = frame(ctx, raw(size, vec![]), v);
        }
    }

    fn plate_rect(ctx: &Context, which: &str) -> Rect {
        let id = Id::new("contract-test").with(which);
        ctx.read_response(id).expect("plate laid out").rect
    }

    #[test]
    fn plates_answer_yes_and_no_and_backdrop_does_nothing() {
        let size = Vec2::new(900.0, 700.0);
        let v = view();
        let ctx = Context::default();
        warm(&ctx, size, &v);
        let yes = plate_rect(&ctx, "yes");
        let no = plate_rect(&ctx, "no");
        assert!(yes.left() > no.right(), "No sits left of Yes");
        assert_eq!(click(&ctx, size, &v, no.center()), Some(ContractChoice::No));
        let ctx = Context::default();
        warm(&ctx, size, &v);
        assert_eq!(
            click(&ctx, size, &v, yes.center()),
            Some(ContractChoice::Yes)
        );
        // The dimmed backdrop: neither answer.
        assert_eq!(click(&ctx, size, &v, Pos2::new(5.0, 5.0)), None);
        // Busy: Yes can't be clicked, No still can.
        let busy = ContractView {
            busy: true,
            ..view()
        };
        let ctx = Context::default();
        warm(&ctx, size, &busy);
        assert_eq!(click(&ctx, size, &busy, yes.center()), None);
        assert_eq!(
            click(&ctx, size, &busy, no.center()),
            Some(ContractChoice::No)
        );
    }
}
