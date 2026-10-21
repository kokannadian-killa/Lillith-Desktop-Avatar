//! Toolkit-free ornament geometry in plain `f32` pixel coordinates
//! (`y` grows downwards, like egui and tiny-skia).
//!
//! Everything here returns points / polygons / segments, never pixels, so
//! the egui painter helpers (`egui_theme`) and a tiny-skia renderer can draw
//! the same shapes. Random-looking ornaments (drips, cracks) are seeded and
//! deterministic, so they don't shimmer between frames.

/// A point `[x, y]`.
pub type Pt = [f32; 2];

/// Rectangle `[x0, y0, x1, y1]`.
pub type RectF = [f32; 4];

// ---------------------------------------------------------------------------
// small math
// ---------------------------------------------------------------------------

/// Tiny deterministic xorshift RNG (no `rand` dependency).
#[derive(Debug, Clone)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9).wrapping_add(0x7F4A_7C15) | 1)
    }
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    /// Uniform in `[0, 1)`.
    pub fn f(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }
    /// Uniform in `[a, b)`.
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f()
    }
}

fn lerp(a: Pt, b: Pt, t: f32) -> Pt {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// Cubic Bézier sampled at `n + 1` points.
pub fn cubic(p0: Pt, p1: Pt, p2: Pt, p3: Pt, n: usize) -> Vec<Pt> {
    let n = n.max(1);
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let a = lerp(p0, p1, t);
            let b = lerp(p1, p2, t);
            let c = lerp(p2, p3, t);
            lerp(lerp(a, b, t), lerp(b, c, t), t)
        })
        .collect()
}

/// Is the closed polygon convex (either winding)? Collinear points allowed.
pub fn is_convex(poly: &[Pt]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut sign = 0.0f32;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        let c = poly[(i + 2) % n];
        let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
        if cross.abs() < 1e-4 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}

/// Splits a polyline into convex quads whose width tapers from `w0` at the
/// first point to `w1` at the last (horns, talons, tails).
pub fn tapered_quads(line: &[Pt], w0: f32, w1: f32) -> Vec<[Pt; 4]> {
    let n = line.len();
    if n < 2 {
        return Vec::new();
    }
    let normal = |i: usize| -> Pt {
        let a = line[i.saturating_sub(1)];
        let b = line[(i + 1).min(n - 1)];
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = (dx * dx + dy * dy).sqrt().max(1e-6);
        [-dy / l, dx / l]
    };
    let side = |i: usize| -> (Pt, Pt) {
        let t = i as f32 / (n - 1) as f32;
        let w = (w0 + (w1 - w0) * t) * 0.5;
        let nm = normal(i);
        let p = line[i];
        (
            [p[0] + nm[0] * w, p[1] + nm[1] * w],
            [p[0] - nm[0] * w, p[1] - nm[1] * w],
        )
    };
    (0..n - 1)
        .map(|i| {
            let (a, b) = side(i);
            let (c, d) = side(i + 1);
            [a, c, d, b]
        })
        .collect()
}

// ---------------------------------------------------------------------------
// neon glow
// ---------------------------------------------------------------------------

/// Stroke stack for a neon glow: `(width, alpha 0..=1)`, widest and faintest
/// first, ending with the crisp core `(core_width, 1.0)`. Draw them in order.
pub fn glow_layers(core_width: f32, spread: f32, layers: usize) -> Vec<(f32, f32)> {
    let mut v: Vec<(f32, f32)> = (1..=layers)
        .rev()
        .map(|i| {
            let t = i as f32 / layers as f32; // 1 = outermost
            let w = core_width + spread * t * 2.0;
            let a = 0.42 * (1.0 - t * 0.85).powi(2);
            (w, a)
        })
        .collect();
    v.push((core_width, 1.0));
    v
}

// ---------------------------------------------------------------------------
// blood
// ---------------------------------------------------------------------------

/// One drip hanging from a top edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drip {
    pub x: f32,
    /// Length below the edge (to the bulb centre).
    pub len: f32,
    /// Width at the top.
    pub width: f32,
}

impl Drip {
    /// Tapered body quad for a drip hanging from `top_y`.
    pub fn body(&self, top_y: f32) -> [Pt; 4] {
        let w = self.width * 0.5;
        let wb = self.width * 0.26;
        let yb = top_y + self.len;
        [
            [self.x - w, top_y],
            [self.x + w, top_y],
            [self.x + wb, yb],
            [self.x - wb, yb],
        ]
    }
    /// Bulb (drop) at the end: centre and radius.
    pub fn bulb(&self, top_y: f32) -> (Pt, f32) {
        ([self.x, top_y + self.len], (self.width * 0.42).max(1.2))
    }
}

/// Seeded drips along `[x0, x1]`: about one per `spacing` px, mostly short
/// with a few long runs up to `max_len`.
pub fn blood_drips(seed: u32, x0: f32, x1: f32, spacing: f32, max_len: f32) -> Vec<Drip> {
    let mut r = Rng::new(seed);
    let mut out = Vec::new();
    let mut x = x0 + r.range(0.2, 1.0) * spacing;
    while x < x1 - 2.0 {
        let k = r.f();
        let len = max_len * (0.12 + 0.88 * k * k * k);
        let width = (2.2 + 4.5 * r.f()) * (0.7 + 0.6 * (len / max_len));
        out.push(Drip { x, len, width });
        x += spacing * r.range(0.35, 1.65);
    }
    out
}

/// The wet band along a top edge: `(x, thickness)` samples every `step` px,
/// to be filled as a strip from the edge down to `y + thickness`.
pub fn blood_edge(seed: u32, x0: f32, x1: f32, step: f32, base: f32) -> Vec<(f32, f32)> {
    let mut r = Rng::new(seed ^ 0xB10D);
    let mut out = Vec::new();
    let mut x = x0;
    let mut t = base;
    while x <= x1 {
        t = (t + r.range(-0.9, 0.9)).clamp(base * 0.45, base * 1.6);
        out.push((x, t));
        x += step;
    }
    if out.last().map(|l| l.0 < x1).unwrap_or(true) {
        out.push((x1, t));
    }
    out
}

// ---------------------------------------------------------------------------
// knife blade
// ---------------------------------------------------------------------------

/// Convex outline of a knife blade lying horizontally, spine on top, tip
/// pointing right at `x1`, `h` tall, centred on `cy`.
pub fn blade_outline(x0: f32, x1: f32, cy: f32, h: f32) -> Vec<Pt> {
    let top = cy - h * 0.5;
    let bot = cy + h * 0.5;
    let tip_len = (h * 2.2).min((x1 - x0) * 0.4);
    vec![
        [x0, top],
        [x1 - tip_len, top],
        [x1, cy - h * 0.18],
        [x1 - tip_len * 0.35, cy + h * 0.22],
        [x1 - tip_len * 0.9, bot],
        [x0, bot],
    ]
}

/// The bright cutting edge of [`blade_outline`] (a polyline along the belly).
pub fn blade_edge(x0: f32, x1: f32, cy: f32, h: f32) -> Vec<Pt> {
    let b = blade_outline(x0, x1, cy, h);
    vec![b[5], b[4], b[3], b[2]]
}

/// Cross-guard and grip rectangles behind a blade starting at `x0`.
pub fn blade_hilt(x0: f32, cy: f32, h: f32) -> (RectF, RectF) {
    let guard = [x0 - h * 0.28, cy - h * 0.85, x0, cy + h * 0.85];
    let grip = [x0 - h * 2.0, cy - h * 0.32, x0 - h * 0.28, cy + h * 0.32];
    (guard, grip)
}

// ---------------------------------------------------------------------------
// carved shapes
// ---------------------------------------------------------------------------

/// Octagon: `r` with its corners cut by `c` (convex).
pub fn chamfer_rect(r: RectF, c: f32) -> Vec<Pt> {
    let [x0, y0, x1, y1] = r;
    let c = c.min((x1 - x0) * 0.5).min((y1 - y0) * 0.5).max(0.0);
    vec![
        [x0 + c, y0],
        [x1 - c, y0],
        [x1, y0 + c],
        [x1, y1 - c],
        [x1 - c, y1],
        [x0 + c, y1],
        [x0, y1 - c],
        [x0, y0 + c],
    ]
}

/// A carved stone tab: flat bottom, bevelled top corners (convex).
pub fn tab_outline(r: RectF, bevel: f32) -> Vec<Pt> {
    let [x0, y0, x1, y1] = r;
    let b = bevel.min((x1 - x0) * 0.3).min(y1 - y0);
    vec![
        [x0, y1],
        [x0, y0 + b],
        [x0 + b, y0],
        [x1 - b, y0],
        [x1, y0 + b],
        [x1, y1],
    ]
}

/// L-shaped terminal corner bracket at `corner`, arms of `len` pointing
/// along `dir` (`[±1, ±1]`, towards the panel's inside).
pub fn bracket(corner: Pt, len: f32, dir: [f32; 2]) -> [Pt; 3] {
    [
        [corner[0] + dir[0] * len, corner[1]],
        corner,
        [corner[0], corner[1] + dir[1] * len],
    ]
}

/// Seeded cracks in a stone rectangle: jagged polylines starting on an edge.
pub fn cracks(seed: u32, r: RectF, count: usize) -> Vec<Vec<Pt>> {
    let mut rng = Rng::new(seed ^ 0xC4AC);
    let [x0, y0, x1, y1] = r;
    let (w, h) = (x1 - x0, y1 - y0);
    let mut out = Vec::new();
    for _ in 0..count {
        let edge = rng.next_u32() % 4;
        let (mut p, mut ang): (Pt, f32) = match edge {
            0 => ([x0 + rng.f() * w, y0], std::f32::consts::FRAC_PI_2),
            1 => ([x1, y0 + rng.f() * h], std::f32::consts::PI),
            2 => ([x0 + rng.f() * w, y1], -std::f32::consts::FRAC_PI_2),
            _ => ([x0, y0 + rng.f() * h], 0.0),
        };
        let steps = 4 + (rng.next_u32() % 7) as usize;
        let mut line = vec![p];
        for _ in 0..steps {
            ang += rng.range(-0.75, 0.75);
            let len = rng.range(5.0, 14.0);
            p = [
                (p[0] + ang.cos() * len).clamp(x0, x1),
                (p[1] + ang.sin() * len).clamp(y0, y1),
            ];
            line.push(p);
        }
        out.push(line);
    }
    out
}

// ---------------------------------------------------------------------------
// runes (Elder Futhark, drawn as straight carved strokes)
// ---------------------------------------------------------------------------

/// A rune: its Unicode character and strokes `[x0, y0, x1, y1]` in a unit
/// cell (`x` 0..1 across the cell width, `y` 0..1 down).
pub struct Rune {
    pub ch: char,
    pub name: &'static str,
    pub strokes: &'static [[f32; 4]],
}

macro_rules! rune {
    ($ch:literal, $name:literal, [$($s:expr),* $(,)?]) => {
        Rune { ch: $ch, name: $name, strokes: &[$($s),*] }
    };
}

/// Elder Futhark subset, all straight strokes.
pub const RUNES: &[Rune] = &[
    rune!(
        'ᚠ',
        "fehu",
        [
            [0.2, 0.0, 0.2, 1.0],
            [0.2, 0.45, 0.9, 0.12],
            [0.2, 0.2, 0.8, 0.0]
        ]
    ),
    rune!(
        'ᚢ',
        "uruz",
        [
            [0.15, 1.0, 0.15, 0.0],
            [0.15, 0.0, 0.85, 0.35],
            [0.85, 0.35, 0.85, 1.0]
        ]
    ),
    rune!(
        'ᚦ',
        "thurisaz",
        [
            [0.2, 0.0, 0.2, 1.0],
            [0.2, 0.25, 0.75, 0.5],
            [0.75, 0.5, 0.2, 0.75]
        ]
    ),
    rune!(
        'ᚨ',
        "ansuz",
        [
            [0.2, 0.0, 0.2, 1.0],
            [0.2, 0.0, 0.8, 0.3],
            [0.2, 0.3, 0.8, 0.6]
        ]
    ),
    rune!(
        'ᚱ',
        "raido",
        [
            [0.2, 0.0, 0.2, 1.0],
            [0.2, 0.0, 0.8, 0.25],
            [0.8, 0.25, 0.2, 0.5],
            [0.2, 0.5, 0.85, 1.0]
        ]
    ),
    rune!(
        'ᚲ',
        "kenaz",
        [[0.8, 0.15, 0.25, 0.5], [0.25, 0.5, 0.8, 0.85]]
    ),
    rune!('ᚷ', "gebo", [[0.1, 0.1, 0.9, 0.9], [0.9, 0.1, 0.1, 0.9]]),
    rune!(
        'ᚹ',
        "wunjo",
        [
            [0.2, 0.0, 0.2, 1.0],
            [0.2, 0.0, 0.8, 0.25],
            [0.8, 0.25, 0.2, 0.5]
        ]
    ),
    rune!(
        'ᚺ',
        "hagalaz",
        [
            [0.2, 0.0, 0.2, 1.0],
            [0.8, 0.0, 0.8, 1.0],
            [0.2, 0.35, 0.8, 0.6]
        ]
    ),
    rune!('ᚾ', "naudiz", [[0.5, 0.0, 0.5, 1.0], [0.2, 0.35, 0.8, 0.6]]),
    rune!('ᛁ', "isa", [[0.5, 0.0, 0.5, 1.0]]),
    rune!(
        'ᛃ',
        "jera",
        [
            [0.45, 0.1, 0.15, 0.4],
            [0.15, 0.4, 0.45, 0.7],
            [0.55, 0.3, 0.85, 0.6],
            [0.85, 0.6, 0.55, 0.9]
        ]
    ),
    rune!(
        'ᛇ',
        "eihwaz",
        [
            [0.5, 0.0, 0.5, 1.0],
            [0.5, 0.0, 0.82, 0.2],
            [0.5, 1.0, 0.18, 0.8]
        ]
    ),
    rune!(
        'ᛉ',
        "algiz",
        [
            [0.5, 0.0, 0.5, 1.0],
            [0.5, 0.45, 0.1, 0.05],
            [0.5, 0.45, 0.9, 0.05]
        ]
    ),
    rune!(
        'ᛊ',
        "sowilo",
        [
            [0.75, 0.05, 0.25, 0.45],
            [0.25, 0.45, 0.75, 0.55],
            [0.75, 0.55, 0.25, 0.95]
        ]
    ),
    rune!(
        'ᛏ',
        "tiwaz",
        [
            [0.5, 0.0, 0.5, 1.0],
            [0.5, 0.0, 0.1, 0.35],
            [0.5, 0.0, 0.9, 0.35]
        ]
    ),
    rune!(
        'ᛒ',
        "berkano",
        [
            [0.2, 0.0, 0.2, 1.0],
            [0.2, 0.0, 0.8, 0.25],
            [0.8, 0.25, 0.2, 0.5],
            [0.2, 0.5, 0.8, 0.75],
            [0.8, 0.75, 0.2, 1.0]
        ]
    ),
    rune!(
        'ᛖ',
        "ehwaz",
        [
            [0.15, 1.0, 0.15, 0.0],
            [0.15, 0.0, 0.5, 0.35],
            [0.5, 0.35, 0.85, 0.0],
            [0.85, 0.0, 0.85, 1.0]
        ]
    ),
    rune!(
        'ᛗ',
        "mannaz",
        [
            [0.15, 0.0, 0.15, 1.0],
            [0.85, 0.0, 0.85, 1.0],
            [0.15, 0.0, 0.85, 0.45],
            [0.85, 0.0, 0.15, 0.45]
        ]
    ),
    rune!(
        'ᛜ',
        "ingwaz",
        [
            [0.5, 0.15, 0.85, 0.5],
            [0.85, 0.5, 0.5, 0.85],
            [0.5, 0.85, 0.15, 0.5],
            [0.15, 0.5, 0.5, 0.15]
        ]
    ),
    rune!(
        'ᛞ',
        "dagaz",
        [
            [0.1, 0.0, 0.1, 1.0],
            [0.9, 0.0, 0.9, 1.0],
            [0.1, 0.0, 0.9, 1.0],
            [0.9, 0.0, 0.1, 1.0]
        ]
    ),
    rune!(
        'ᛟ',
        "othala",
        [
            [0.5, 0.0, 0.85, 0.35],
            [0.5, 0.0, 0.15, 0.35],
            [0.15, 0.35, 0.8, 1.0],
            [0.85, 0.35, 0.2, 1.0]
        ]
    ),
];

/// Rune `idx` (wrapping) as segments in a cell at `origin` (top-left) of
/// `height`; the cell is `0.62 * height` wide.
pub fn rune_segments(idx: usize, origin: Pt, height: f32) -> Vec<(Pt, Pt)> {
    let r = &RUNES[idx % RUNES.len()];
    let w = height * 0.62;
    r.strokes
        .iter()
        .map(|s| {
            (
                [origin[0] + s[0] * w, origin[1] + s[1] * height],
                [origin[0] + s[2] * w, origin[1] + s[3] * height],
            )
        })
        .collect()
}

/// Width of a rune cell of `height`.
pub fn rune_width(height: f32) -> f32 {
    height * 0.62
}

// ---------------------------------------------------------------------------
// skull + horns ornament
// ---------------------------------------------------------------------------

/// A horned skull built from simple primitives (circles, rects, triangles,
/// tapered horn strips), centred on `c`, `s` ≈ skull height.
#[derive(Debug, Clone)]
pub struct Skull {
    pub cranium: (Pt, f32),
    /// Jaw / cheek block `[x0, y0, x1, y1]`.
    pub jaw: RectF,
    pub eyes: [(Pt, f32); 2],
    pub nose: [Pt; 3],
    pub teeth: Vec<(Pt, Pt)>,
    /// Horn centrelines (left, right), base first.
    pub horns: [Vec<Pt>; 2],
    /// Horn width at the base (tapers to 0 at the tip).
    pub horn_width: f32,
}

pub fn skull(c: Pt, s: f32) -> Skull {
    let [cx, cy] = c;
    let horn = |side: f32| {
        cubic(
            [cx + side * 0.26 * s, cy - 0.36 * s],
            [cx + side * 0.62 * s, cy - 0.52 * s],
            [cx + side * 0.82 * s, cy - 0.30 * s],
            [cx + side * 0.78 * s, cy - 0.86 * s],
            14,
        )
    };
    Skull {
        cranium: ([cx, cy - 0.1 * s], 0.4 * s),
        jaw: [cx - 0.24 * s, cy + 0.05 * s, cx + 0.24 * s, cy + 0.43 * s],
        eyes: [
            ([cx - 0.15 * s, cy - 0.04 * s], 0.115 * s),
            ([cx + 0.15 * s, cy - 0.04 * s], 0.115 * s),
        ],
        nose: [
            [cx, cy + 0.09 * s],
            [cx - 0.055 * s, cy + 0.21 * s],
            [cx + 0.055 * s, cy + 0.21 * s],
        ],
        teeth: (-2..=2)
            .map(|k| {
                let x = cx + k as f32 * 0.085 * s;
                ([x, cy + 0.29 * s], [x, cy + 0.43 * s])
            })
            .collect(),
        horns: [horn(-1.0), horn(1.0)],
        horn_width: 0.15 * s,
    }
}

// ---------------------------------------------------------------------------
// sigil: neon horns + spade tail (abstract emblem, not the character)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Sigil {
    /// Horn centrelines (left, right), base first.
    pub horns: [Vec<Pt>; 2],
    pub horn_width: f32,
    /// Halo ring (centre, radius).
    pub ring: (Pt, f32),
    /// S-curved tail from the ring down to the spade.
    pub tail: Vec<Pt>,
    /// Convex spade tip.
    pub spade: Vec<Pt>,
    /// Inner inverted triangle "eye" in the ring.
    pub eye: [Pt; 3],
}

/// The emblem centred on `c`, `s` ≈ total height.
pub fn sigil(c: Pt, s: f32) -> Sigil {
    let [cx, cy] = c;
    let cy = cy - 0.08 * s;
    let horn = |side: f32| {
        cubic(
            [cx + side * 0.12 * s, cy - 0.06 * s],
            [cx + side * 0.46 * s, cy - 0.04 * s],
            [cx + side * 0.52 * s, cy - 0.34 * s],
            [cx + side * 0.30 * s, cy - 0.50 * s],
            24,
        )
    };
    let tip = [cx, cy + 0.50 * s];
    let k = 0.075 * s;
    Sigil {
        horns: [horn(-1.0), horn(1.0)],
        horn_width: 0.085 * s,
        ring: ([cx, cy], 0.15 * s),
        tail: cubic(
            [cx, cy + 0.15 * s],
            [cx + 0.26 * s, cy + 0.24 * s],
            [cx - 0.24 * s, cy + 0.34 * s],
            [cx, tip[1] - 0.04 * s],
            24,
        ),
        spade: vec![
            [tip[0], tip[1] - k * 0.9],
            [tip[0] + k, tip[1]],
            [tip[0] + k * 0.75, tip[1] + k * 0.55],
            [tip[0], tip[1] + k * 1.6],
            [tip[0] - k * 0.75, tip[1] + k * 0.55],
            [tip[0] - k, tip[1]],
        ],
        eye: [
            [cx - 0.07 * s, cy - 0.04 * s],
            [cx + 0.07 * s, cy - 0.04 * s],
            [cx, cy + 0.07 * s],
        ],
    }
}

/// Circuit traces fanning out from the sigil (orthogonal runs with 45°
/// bends, ending in node dots): polylines.
pub fn sigil_traces(seed: u32, c: Pt, s: f32, count: usize) -> Vec<Vec<Pt>> {
    let mut r = Rng::new(seed ^ 0x51C1);
    let mut out = Vec::new();
    for i in 0..count {
        let side = if i % 2 == 0 { -1.0 } else { 1.0 };
        let y = c[1] + r.range(-0.42, 0.42) * s;
        let x_start = c[0] + side * r.range(0.30, 0.42) * s;
        let run1 = r.range(0.08, 0.22) * s;
        let bend = r.range(-0.12, 0.12) * s;
        let run2 = r.range(0.10, 0.34) * s;
        let p0 = [x_start, y];
        let p1 = [x_start + side * run1, y];
        let p2 = [p1[0] + side * bend.abs(), y + bend];
        let p3 = [p2[0] + side * run2, p2[1]];
        out.push(vec![p0, p1, p2, p3]);
    }
    out
}

// ---------------------------------------------------------------------------
// claw resize grip
// ---------------------------------------------------------------------------

/// Three curved talons for a resize grip at window `corner`; `dir` points
/// into the window (`[-1, -1]` for bottom-right). Returns centrelines
/// (base first, tip last) and the knuckle (centre, radius).
pub fn claw(corner: Pt, size: f32, dir: [f32; 2]) -> (Vec<Vec<Pt>>, (Pt, f32)) {
    let knuckle = [
        corner[0] + dir[0] * size * 0.62,
        corner[1] + dir[1] * size * 0.62,
    ];
    let mut talons = Vec::new();
    for k in [-1.0f32, 0.0, 1.0] {
        // spread the three talons around the diagonal towards the corner
        let ang = (-dir[1]).atan2(-dir[0]) + k * 0.62;
        let (ca, sa) = (ang.cos(), ang.sin());
        let reach = size * if k == 0.0 { 0.78 } else { 0.62 };
        let base = [knuckle[0] + ca * size * 0.12, knuckle[1] + sa * size * 0.12];
        let tip = [knuckle[0] + ca * reach, knuckle[1] + sa * reach];
        // hook: bend the tip back towards the window inside
        let hook = [tip[0] + dir[0] * size * 0.10, tip[1] + dir[1] * size * 0.10];
        let mid = [
            knuckle[0] + ca * reach * 0.62 - sa * size * 0.08,
            knuckle[1] + sa * reach * 0.62 + ca * size * 0.08,
        ];
        talons.push(cubic(base, mid, tip, hook, 10));
    }
    (talons, (knuckle, size * 0.16))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic_and_in_range() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..1000 {
            let x = a.f();
            assert_eq!(x, b.f());
            assert!((0.0..1.0).contains(&x));
        }
    }

    #[test]
    fn glow_fades_outwards() {
        let g = glow_layers(2.0, 6.0, 5);
        assert_eq!(g.len(), 6);
        assert_eq!(*g.last().unwrap(), (2.0, 1.0));
        for w in g.windows(2) {
            assert!(w[0].0 > w[1].0, "widths must shrink: {g:?}");
            assert!(w[0].1 < w[1].1, "alpha must grow inwards: {g:?}");
        }
    }

    #[test]
    fn drips_are_seeded_and_bounded() {
        let a = blood_drips(3, 0.0, 800.0, 30.0, 40.0);
        assert_eq!(a, blood_drips(3, 0.0, 800.0, 30.0, 40.0));
        assert_ne!(a, blood_drips(4, 0.0, 800.0, 30.0, 40.0));
        assert!(a.len() > 10);
        for d in &a {
            assert!(d.x >= 0.0 && d.x <= 800.0);
            assert!(d.len > 0.0 && d.len <= 40.0);
            assert!(is_convex(&d.body(0.0)));
        }
        let e = blood_edge(1, 0.0, 100.0, 4.0, 5.0);
        assert_eq!(e.first().unwrap().0, 0.0);
        assert_eq!(e.last().unwrap().0, 100.0);
        assert!(e.iter().all(|(_, t)| *t > 0.0 && *t <= 8.0));
    }

    #[test]
    fn carved_shapes_are_convex() {
        assert!(is_convex(&blade_outline(0.0, 200.0, 10.0, 12.0)));
        assert!(is_convex(&blade_outline(0.0, 20.0, 10.0, 12.0)));
        assert!(is_convex(&chamfer_rect([0.0, 0.0, 100.0, 40.0], 8.0)));
        assert!(is_convex(&tab_outline([0.0, 0.0, 120.0, 34.0], 10.0)));
        assert!(is_convex(&sigil([0.0, 0.0], 200.0).spade));
        for q in tapered_quads(
            &cubic([0.0, 0.0], [10.0, 0.0], [20.0, 10.0], [20.0, 30.0], 12),
            8.0,
            1.0,
        ) {
            assert!(is_convex(&q));
        }
    }

    #[test]
    fn runes_fit_their_cell() {
        assert!(RUNES.len() >= 20);
        for r in RUNES {
            assert!(!r.strokes.is_empty(), "{}", r.name);
            for s in r.strokes {
                assert!(
                    s.iter().all(|v| (0.0..=1.0).contains(v)),
                    "{} out of cell",
                    r.name
                );
            }
        }
        let segs = rune_segments(0, [10.0, 20.0], 30.0);
        for (a, b) in segs {
            for p in [a, b] {
                assert!(p[0] >= 10.0 && p[0] <= 10.0 + rune_width(30.0));
                assert!(p[1] >= 20.0 && p[1] <= 50.0);
            }
        }
    }

    #[test]
    fn sigil_and_skull_are_symmetric() {
        let s = sigil([100.0, 100.0], 200.0);
        for (l, r) in s.horns[0].iter().zip(&s.horns[1]) {
            assert!((100.0 - l[0] - (r[0] - 100.0)).abs() < 1e-3);
            assert!((l[1] - r[1]).abs() < 1e-3);
        }
        let k = skull([50.0, 50.0], 40.0);
        assert!((k.eyes[0].0[0] + k.eyes[1].0[0] - 100.0).abs() < 1e-3);
        assert_eq!(k.teeth.len(), 5);
    }

    #[test]
    fn claw_points_at_its_corner() {
        let (talons, (k, _)) = claw([100.0, 100.0], 40.0, [-1.0, -1.0]);
        assert_eq!(talons.len(), 3);
        assert!(k[0] < 100.0 && k[1] < 100.0);
        // the middle talon's tip region is closer to the corner than the knuckle
        let tip = talons[1][talons[1].len() - 2];
        let d = |p: Pt| ((p[0] - 100.0).powi(2) + (p[1] - 100.0).powi(2)).sqrt();
        assert!(d(tip) < d(k));
    }

    #[test]
    fn cracks_stay_inside() {
        let r = [10.0, 10.0, 210.0, 110.0];
        for line in cracks(9, r, 12) {
            for p in line {
                assert!(p[0] >= r[0] && p[0] <= r[2] && p[1] >= r[1] && p[1] <= r[3]);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// hellforge emblem (chat box badge)
// ---------------------------------------------------------------------------

/// The hellforge emblem: an anvil under a three-tongued flame inside a
/// rune ring. Every polygon is convex (fills directly as a convex shape).
#[derive(Debug, Clone)]
pub struct Hellforge {
    pub center: Pt,
    /// Rune ring radius.
    pub ring_r: f32,
    /// Rune cells around the ring: `(index, origin, height)`.
    pub runes: Vec<(usize, Pt, f32)>,
    /// Anvil pieces: horn, face, waist, foot (convex each).
    pub anvil: Vec<Vec<Pt>>,
    /// Flame tongues, outer (ember) to inner (core).
    pub flames: Vec<Vec<Pt>>,
    /// Hot line along the anvil face (the struck edge).
    pub face_edge: [Pt; 2],
}

/// Hellforge emblem centred at `c`, `s` = overall diameter.
pub fn hellforge(c: Pt, s: f32) -> Hellforge {
    let u = s / 100.0;
    let at = |x: f32, y: f32| [c[0] + x * u, c[1] + y * u];
    let anvil = vec![
        // Horn (pointing left) as a triangle.
        vec![at(-36.0, 6.0), at(-18.0, 4.0), at(-18.0, 12.0)],
        // Face (top slab).
        vec![
            at(-18.0, 4.0),
            at(26.0, 4.0),
            at(26.0, 13.0),
            at(-18.0, 13.0),
        ],
        // Waist (narrowing trapezoid).
        vec![
            at(-10.0, 13.0),
            at(16.0, 13.0),
            at(10.0, 24.0),
            at(-5.0, 24.0),
        ],
        // Foot (widening trapezoid).
        vec![
            at(-5.0, 24.0),
            at(10.0, 24.0),
            at(20.0, 32.0),
            at(-16.0, 32.0),
        ],
    ];
    // Flame tongues: kite shapes rising from the face.
    let tongue = |x: f32, w: f32, h: f32| {
        vec![
            at(x, 3.0),
            at(x + w * 0.5, -h * 0.35),
            at(x + w * 0.12, -h),
            at(x - w * 0.5, -h * 0.4),
        ]
    };
    let flames = vec![
        tongue(-6.0, 22.0, 34.0),
        tongue(8.0, 18.0, 28.0),
        tongue(1.0, 14.0, 40.0),
        tongue(1.5, 7.0, 26.0),
    ];
    let ring_r = 46.0 * u;
    let rh = 7.5 * u;
    let n = 12;
    let runes = (0..n)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / n as f32 - std::f32::consts::FRAC_PI_2;
            let rr = ring_r - 7.0 * u;
            let o = [
                c[0] + a.cos() * rr - rune_width(rh) * 0.5,
                c[1] + a.sin() * rr - rh * 0.5,
            ];
            (i * 5 + 3, o, rh)
        })
        .collect();
    Hellforge {
        center: c,
        ring_r,
        runes,
        anvil,
        flames,
        face_edge: [at(-17.0, 4.5), at(25.0, 4.5)],
    }
}

#[cfg(test)]
mod hellforge_tests {
    use super::*;

    #[test]
    fn hellforge_parts_are_convex_and_inside_the_ring() {
        let h = hellforge([50.0, 50.0], 100.0);
        assert_eq!(h.anvil.len(), 4);
        assert!(h.flames.len() >= 3);
        assert_eq!(h.runes.len(), 12);
        for poly in h.anvil.iter().chain(&h.flames) {
            assert!(is_convex(poly), "{poly:?}");
            for p in poly {
                let d = ((p[0] - 50.0).powi(2) + (p[1] - 50.0).powi(2)).sqrt();
                assert!(d < h.ring_r - 4.0, "{p:?} outside the ring");
            }
        }
        // Flames rise above the anvil face.
        let top = h
            .flames
            .iter()
            .flatten()
            .map(|p| p[1])
            .fold(f32::MAX, f32::min);
        assert!(top < h.face_edge[0][1] - 30.0);
    }
}
