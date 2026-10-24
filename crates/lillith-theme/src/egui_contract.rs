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
    additive, blood_drips, c32, glow_blob, gradient_rect, gradient_rect_h, iron_frame, neon_line,
    neon_rect,
};
use crate::geom::{self, Pt};
use crate::palette::{self as pal, Rgba};
use egui::text::{LayoutJob, TextFormat};
use egui::Galley;
use egui::{
    Align2, Color32, Context, CornerRadius, FontFamily, FontId, Id, Painter, Pos2, Rect, Sense,
    Stroke, StrokeKind, Vec2,
};
use std::sync::Arc;

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
    /// Show `detail` whole instead of clipping it: every line wraps (never
    /// cut off) and the slab scrolls past [`DETAIL_LINES`] rows. Program
    /// launch cards set this so the full argv can always be read.
    pub scroll_detail: bool,
}

/// The detail text the plate lays out: whole when `scroll_detail` is set,
/// else clipped by [`clip_detail`].
pub fn detail_text(view: &ContractView<'_>) -> Option<String> {
    let d = view.detail.filter(|s| !s.trim().is_empty())?;
    Some(if view.scroll_detail {
        d.to_string()
    } else {
        clip_detail(d, DETAIL_LINES)
    })
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

/// Clear space between the horn tips and the top of the skull emblem's
/// inner well (logical px).
pub const SKULL_TOP_GAP: f32 = 5.0;

/// The photoreal horned skull on the emblem: a jawless cranium (upper teeth
/// only, no mandible) turned slightly to the viewer's left, as an RGBA PNG
/// cut out of the approved art (black keyed to a feathered alpha edge,
/// sockets kept opaque), aged to desaturated bone and trimmed to its bounds
/// so its top row is the horn crowns.
const SKULL_PNG: &[u8] = include_bytes!("../assets/soul_skull.png");
/// Pixel size of [`SKULL_PNG`] (`[w, h]`); a test keeps this in sync.
const SKULL_PX: [u32; 2] = [507, 320];
/// Thickness of the emblem's iron frame around its dark well.
const EMBLEM_INSET: f32 = 5.0;
/// Padding inside the gold-ruled header plate.
const HEADER_PAD: f32 = 8.0;
/// Gap between the emblem and the header text column.
const HEADER_GUTTER: f32 = 12.0;

/// Antique gold, as on LifeCraft's Soul Forge card.
const GOLD_PALE: Rgba = Rgba::hex(0xf6e7b2);
const GOLD: Rgba = Rgba::hex(0xc9a24a);
const GOLD_DARK: Rgba = Rgba::hex(0x8a6420);
const GOLD_RULE: Rgba = Rgba::hex(0x8a7044);

/// Size of the skull emblem that sits at the left of the header.
fn emblem_size(narrow: bool) -> Vec2 {
    if narrow {
        Vec2::new(80.0, 68.0)
    } else {
        Vec2::new(108.0, 90.0)
    }
}

/// Where the skull art is drawn inside an emblem: centred in the well, at
/// the art's own aspect, as large as fits with [`SKULL_TOP_GAP`] clear above.
fn skull_rect(emblem: Rect) -> Rect {
    let well = emblem.shrink(EMBLEM_INSET);
    let aspect = SKULL_PX[0] as f32 / SKULL_PX[1] as f32;
    let h = (well.height() - SKULL_TOP_GAP - 2.0).min((well.width() - 4.0) / aspect);
    let size = Vec2::new(h * aspect, h);
    // The jawless cranium is wider than tall, so it is width-bound: centre
    // it in the spare height, never closer than SKULL_TOP_GAP to the top.
    let top = well.top() + SKULL_TOP_GAP.max((well.height() - size.y) / 2.0);
    Rect::from_min_size(Pos2::new(well.center().x - size.x / 2.0, top), size)
}

/// Width left for the header text column on a plate whose inner width is
/// `inner_w`.
fn header_text_width(inner_w: f32, narrow: bool) -> f32 {
    inner_w - 2.0 * HEADER_PAD - emblem_size(narrow).x - HEADER_GUTTER
}

/// Small spaced capitals, like the Soul Forge card's header row.
fn caps(painter: &Painter, text: &str, size: f32, spacing: f32, color: Rgba) -> Arc<Galley> {
    let job = LayoutJob::single_section(
        text.to_string(),
        TextFormat {
            font_id: FontId::new(size, FontFamily::Proportional),
            color: c32(color),
            extra_letter_spacing: spacing,
            ..Default::default()
        },
    );
    painter.layout_job(job)
}

/// Decodes [`SKULL_PNG`] to straight-alpha RGBA8 (`[w, h]`, pixels).
fn decode_skull() -> ([usize; 2], Vec<u8>) {
    let mut dec = png::Decoder::new(SKULL_PNG);
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec.read_info().expect("bundled skull PNG header");
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("bundled skull PNG data");
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width as usize, info.height as usize);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .chunks(3)
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buf
            .chunks(2)
            .flat_map(|c| [c[0], c[0], c[0], c[1]])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => unreachable!("EXPAND turns palettes into RGB(A)"),
    };
    ([w, h], rgba)
}

/// The skull texture, uploaded once per egui context and kept in its
/// temp data.
fn skull_texture(ctx: &Context) -> egui::TextureHandle {
    let key = Id::new("lillith-soul-contract-skull");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(key)) {
        return t;
    }
    let (size, rgba) = decode_skull();
    let img = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
    let opts = egui::TextureOptions {
        mipmap_mode: Some(egui::TextureFilter::Linear),
        ..egui::TextureOptions::LINEAR
    };
    let t = ctx.load_texture("lillith-soul-skull", img, opts);
    ctx.data_mut(|d| d.insert_temp(key, t.clone()));
    t
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
    body: FontId,
    mono: FontId,
    button: FontId,
}

impl Fonts {
    fn new(ctx: &Context, narrow: bool) -> Self {
        let carved = family_or(ctx, "carved", FontFamily::Proportional);
        let bold = family_or(ctx, "bold", FontFamily::Proportional);
        let k = if narrow { 0.88 } else { 1.0 };
        Self {
            head: FontId::new(22.0 * k, carved),
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
    let k = if narrow { 0.88 } else { 1.0 };
    let em = emblem_size(narrow);
    let col_w = header_text_width(inner_w, narrow);
    let kind = if view.title.is_empty() {
        "PACT"
    } else {
        view.title
    };
    let caps_l = caps(&painter, "SOUL CONTRACT", 10.0 * k, 1.4 * k, GOLD);
    let caps_r = caps(&painter, &kind.to_uppercase(), 10.0 * k, 1.4 * k, GOLD_PALE);
    // Split left / right like the forge card when it fits, else stack.
    let split = caps_l.size().x + caps_r.size().x + 10.0 <= col_w;
    let caps_h = if split {
        caps_l.size().y.max(caps_r.size().y)
    } else {
        caps_l.size().y + 2.0 + caps_r.size().y
    };
    let heading = {
        let mut job = LayoutJob::default();
        job.wrap.max_width = col_w;
        let fmt = |c: Rgba| TextFormat {
            font_id: FontId::new(16.0 * k, f.head.family.clone()),
            color: c32(c),
            ..Default::default()
        };
        job.append("Seal it in ", 0.0, fmt(GOLD_PALE));
        job.append("blood", 0.0, fmt(pal::EMBER));
        painter.layout_job(job)
    };
    let fine_l = caps(&painter, "BLOOD OATH", 8.5 * k, 0.7 * k, GOLD_DARK);
    let fine_r = caps(
        &painter,
        if view.busy { "SEALING" } else { "UNSIGNED" },
        8.5 * k,
        0.7 * k,
        GOLD_DARK,
    );
    let text_h = caps_h + 6.0 + heading.size().y + 6.0 + 4.0 + 6.0 + fine_l.size().y;
    let plate_h = em.y.max(text_h) + 2.0 * HEADER_PAD;
    let header_top = if narrow { 8.0 } else { 12.0 };
    let summary = painter.layout(summary_line(view), f.body.clone(), c32(pal::TEXT), inner_w);
    let target = view
        .target
        .filter(|s| !s.is_empty())
        .map(|p| painter.layout(format!("⛧ {p}"), f.mono.clone(), c32(pal::EMBER), inner_w));
    let detail = detail_text(view)
        .map(|d| painter.layout(d, f.mono.clone(), c32(pal::BONE), inner_w - 16.0));
    // A scrolling slab is at most DETAIL_LINES rows tall; the rest scrolls.
    let detail_max_h = ctx.fonts(|fo| fo.row_height(&f.mono)) * DETAIL_LINES as f32;
    let detail_h = detail.as_ref().map(|g| {
        if view.scroll_detail {
            g.size().y.min(detail_max_h)
        } else {
            g.size().y
        }
    });
    let oath = painter.layout(
        "Nothing happens without your mark. Every answer is written to the audit log.".into(),
        FontId::new(f.body.size * 0.78, FontFamily::Proportional),
        c32(pal::TEXT_MUTE),
        inner_w,
    );
    let btn_h = if narrow { 30.0 } else { 34.0 };
    let mut h = frame_t + header_top + plate_h + 14.0;
    h += summary.size().y + 8.0;
    if let Some(g) = &target {
        h += g.size().y + 8.0;
    }
    if let Some(dh) = detail_h {
        h += dh + 16.0 + 10.0;
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

    // --- header: a Soul Forge card with a memento-mori skull emblem ---------
    let lx = inner.left() + pad;
    let rx = inner.right() - pad;
    let mut y = inner.top() + header_top;
    let hr = Rect::from_min_max(Pos2::new(lx, y), Pos2::new(rx, y + plate_h));
    // iron card, thin antique-gold border, inset dark double rule
    gradient_rect(
        &p,
        hr,
        c32(Rgba::hex(0x201a14).alpha(245)),
        c32(Rgba::hex(0x0e0b0e).alpha(245)),
    );
    p.rect_stroke(
        hr,
        CornerRadius::ZERO,
        Stroke::new(1.0, c32(GOLD_RULE)),
        StrokeKind::Inside,
    );
    p.rect_stroke(
        hr.shrink(1.0),
        CornerRadius::ZERO,
        Stroke::new(1.0, c32(Rgba::hex(0x120e0a))),
        StrokeKind::Inside,
    );
    p.rect_stroke(
        hr.shrink(2.0),
        CornerRadius::ZERO,
        Stroke::new(2.0, c32(Rgba::hex(0x000000).alpha(90))),
        StrokeKind::Inside,
    );

    // the emblem: a dark iron frame round a shadowed well, skull inside
    let er = Rect::from_min_size(hr.min + Vec2::splat(HEADER_PAD), em);
    gradient_rect(&p, er, c32(pal::IRON_HI), c32(pal::IRON));
    p.rect_stroke(
        er,
        CornerRadius::ZERO,
        Stroke::new(1.0, c32(GOLD_DARK)),
        StrokeKind::Inside,
    );
    let well = er.shrink(EMBLEM_INSET);
    p.rect_filled(well, CornerRadius::ZERO, c32(Rgba::hex(0x070405)));
    // a dying ember low in the well, nothing brighter
    glow_blob(
        &p,
        Pos2::new(well.center().x, well.bottom() - well.height() * 0.2),
        well.width() * 0.42,
        pal::BLOOD,
        0.35 + 0.15 * flick,
    );
    // vignette: shadow closing in from the well's edges (behind the skull)
    let step = well.width().min(well.height()) * 0.03;
    for i in 0..12 {
        p.rect_stroke(
            well.shrink(i as f32 * step),
            CornerRadius::ZERO,
            Stroke::new(step + 0.5, c32(Rgba::hex(0x000000).alpha(60 - i * 5))),
            StrokeKind::Inside,
        );
    }
    let tex = skull_texture(&ctx);
    let sr = skull_rect(er);
    p.image(
        tex.id(),
        sr,
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        Color32::WHITE,
    );
    // ember deep in the sockets (the art's own sockets sit ~45% down)
    for dx in [-0.16, 0.16] {
        glow_blob(
            &p,
            Pos2::new(
                sr.center().x + dx * sr.width(),
                sr.top() + sr.height() * 0.45,
            ),
            sr.height() * 0.08,
            pal::EMBER,
            0.12 + 0.12 * flick,
        );
    }
    p.rect_stroke(
        well,
        CornerRadius::ZERO,
        Stroke::new(1.0, c32(Rgba::hex(0x000000))),
        StrokeKind::Outside,
    );
    // rivets in the frame corners
    for c in [
        er.left_top(),
        er.right_top(),
        er.left_bottom(),
        er.right_bottom(),
    ] {
        let q = c + (er.center() - c).normalized() * (EMBLEM_INSET * 0.9);
        p.circle_filled(q, 1.4, c32(GOLD_DARK));
        p.circle_filled(q - Vec2::splat(0.4), 0.6, c32(GOLD));
    }
    // blood running off the bottom of the emblem
    blood_drips(
        &p,
        er.left() + 6.0,
        er.right() - 6.0,
        er.bottom() - 1.0,
        0x5a11,
        if narrow { 7.0 } else { 10.0 },
        26.0,
    );

    // text column: caps row, serif heading, forge rule, fine row
    let tx = er.right() + HEADER_GUTTER;
    let tr = hr.right() - HEADER_PAD;
    let mut ty = hr.center().y - text_h / 2.0;
    p.galley(Pos2::new(tx, ty), caps_l.clone(), c32(GOLD));
    if split {
        p.galley(
            Pos2::new(tr - caps_r.size().x, ty),
            caps_r.clone(),
            c32(GOLD_PALE),
        );
    } else {
        p.galley(
            Pos2::new(tx, ty + caps_l.size().y + 2.0),
            caps_r.clone(),
            c32(GOLD_PALE),
        );
    }
    ty += caps_h + 6.0;
    p.galley(Pos2::new(tx, ty), heading.clone(), c32(GOLD_PALE));
    ty += heading.size().y + 6.0;
    let bar = Rect::from_min_max(Pos2::new(tx, ty), Pos2::new(tr, ty + 4.0));
    let mid = bar.center().x;
    gradient_rect_h(
        &p,
        Rect::from_min_max(bar.min, Pos2::new(mid, bar.bottom())),
        c32(Rgba::hex(0x9a2a4a)),
        c32(pal::EMBER),
    );
    gradient_rect_h(
        &p,
        Rect::from_min_max(Pos2::new(mid, bar.top()), bar.max),
        c32(pal::EMBER),
        c32(GOLD),
    );
    ty += 4.0 + 6.0;
    p.galley(Pos2::new(tx, ty), fine_l.clone(), c32(GOLD_DARK));
    p.galley(
        Pos2::new(tr - fine_r.size().x, ty),
        fine_r.clone(),
        c32(GOLD_DARK),
    );
    y += plate_h + 14.0;

    // --- body -------------------------------------------------------------------
    p.galley(Pos2::new(lx, y), summary.clone(), c32(pal::TEXT));
    y += summary.size().y + 8.0;
    if let Some(g) = &target {
        p.galley(Pos2::new(lx, y), g.clone(), c32(pal::EMBER));
        y += g.size().y + 8.0;
    }
    if let (Some(g), Some(dh)) = (&detail, detail_h) {
        let r = Rect::from_min_size(Pos2::new(lx, y), Vec2::new(inner_w, dh + 16.0));
        terminal_slab(&p, r);
        let text_r =
            Rect::from_min_size(r.min + Vec2::new(8.0, 8.0), Vec2::new(inner_w - 16.0, dh));
        if g.size().y > dh + 0.5 {
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(text_r)
                    .id_salt(id.with("detail")),
            );
            egui::ScrollArea::vertical()
                .id_salt(id.with("detail-scroll"))
                .max_height(dh)
                .auto_shrink([false, false])
                .show(&mut child, |ui| {
                    let (gr, _) = ui.allocate_exact_size(g.size(), Sense::hover());
                    ui.painter().galley(gr.min, g.clone(), c32(pal::BONE));
                });
        } else {
            p.galley(text_r.min, g.clone(), c32(pal::BONE));
        }
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
            scroll_detail: false,
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
    fn skull_art_decodes_at_its_declared_size() {
        let (size, rgba) = decode_skull();
        assert_eq!(size, [SKULL_PX[0] as usize, SKULL_PX[1] as usize]);
        assert_eq!(rgba.len(), size[0] * size[1] * 4);
        // Cut out: transparent corners, opaque bone in the middle (sockets
        // and all, nothing keyed out of the interior).
        let a = |x: usize, y: usize| rgba[(y * size[0] + x) * 4 + 3];
        for (x, y) in [
            (0, 0),
            (size[0] - 1, 0),
            (0, size[1] - 1),
            (size[0] - 1, size[1] - 1),
        ] {
            assert_eq!(a(x, y), 0, "corner {x},{y}");
        }
        assert_eq!(a(size[0] / 2, size[1] / 2), 255);
        assert!(SKULL_PNG.len() < 300 * 1024);
    }

    /// Topmost and bottommost rows with visible art, and the widest columns.
    fn opaque_bounds() -> (usize, usize, usize, usize) {
        let ([w, h], rgba) = decode_skull();
        let vis = |x: usize, y: usize| rgba[(y * w + x) * 4 + 3] > 8;
        let rows: Vec<usize> = (0..h).filter(|&y| (0..w).any(|x| vis(x, y))).collect();
        let cols: Vec<usize> = (0..w).filter(|&x| (0..h).any(|y| vis(x, y))).collect();
        (
            rows[0],
            *rows.last().unwrap(),
            cols[0],
            *cols.last().unwrap(),
        )
    }

    #[test]
    fn horn_tips_clear_the_top_of_the_plate() {
        let (top, bottom, left, right) = opaque_bounds();
        assert!(top <= 4, "skull art is trimmed (top row {top})");
        for narrow in [true, false] {
            let er = Rect::from_min_size(Pos2::new(10.0, 20.0), emblem_size(narrow));
            let well = er.shrink(EMBLEM_INSET);
            let sr = skull_rect(er);
            let aspect = SKULL_PX[0] as f32 / SKULL_PX[1] as f32;
            assert!((sr.width() / sr.height() - aspect).abs() < 1e-4);
            assert!(
                well.contains_rect(sr),
                "narrow={narrow}: {sr:?} in {well:?}"
            );
            let per_px = sr.height() / SKULL_PX[1] as f32;
            let horn_top = sr.top() + top as f32 * per_px;
            assert!(
                horn_top >= well.top() + SKULL_TOP_GAP - 0.01,
                "narrow={narrow}: horn reaches y={horn_top}"
            );
            assert!(sr.top() + bottom as f32 * per_px <= well.bottom());
            assert!(sr.left() + left as f32 * per_px >= well.left());
            assert!(sr.left() + right as f32 * per_px <= well.right());
        }
    }

    #[test]
    fn header_text_column_has_room_on_every_plate() {
        for w in [MIN_WIDTH, plate_width(300.0), MAX_WIDTH] {
            let narrow = w < 340.0;
            let (frame_t, pad) = if narrow { (7.0, 12.0) } else { (9.0, 18.0) };
            let inner_w = w - 2.0 * (frame_t + pad);
            assert!(header_text_width(inner_w, narrow) > 70.0, "plate {w}");
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
    fn a_scrolling_detail_is_never_cut() {
        let long = (0..40)
            .map(|i| format!("    --arg-{i} {}", "x".repeat(200)))
            .collect::<Vec<_>>()
            .join("\n");
        let v = ContractView {
            detail: Some(&long),
            scroll_detail: true,
            ..view()
        };
        assert_eq!(detail_text(&v).as_deref(), Some(long.as_str()));
        let clipped = ContractView {
            scroll_detail: false,
            ..v
        };
        assert_ne!(detail_text(&clipped).as_deref(), Some(long.as_str()));
        // Paints (with the inner scroll area) and still answers.
        for size in [Vec2::new(300.0, 380.0), Vec2::new(1000.0, 700.0)] {
            let ctx = Context::default();
            assert_eq!(frame(&ctx, raw(size, vec![]), &v), None);
            assert_eq!(frame(&ctx, raw(size, vec![]), &v), None);
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
