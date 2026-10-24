//! Gravity and window perching, compositor-free (unit-tested).
//!
//! After a drag-release she keeps the `drag_held` pose and falls
//! ([`Fall`]: `[behavior.gravity] g_px_s2`, capped at `max_fall_speed`),
//! bounces softly (`bounce`) and lands on the first visible surface under
//! her feet: the top edge of a window (KWin bridge window stack, see
//! [`lillith_core::desktop::landing_target`]) or the floor, the bottom of
//! the usable area above the panel (bridge work area, else
//! `[behavior] floor_offset_px`). On a window she wanders along its top,
//! clamped to its x-span ([`perch_span`]), sits on the edge now and then
//! (`sit_swing`, `sit_base` clips and their variants, seat line from the
//! clip's `seat_y`) or lounges along it (`lounge_base` clips that fit the
//! window, [`lounge_shift`]), and falls again when the window moves,
//! minimizes or closes ([`check_perch`]). Landing on a window from a drag or
//! fall plays the manifest's `drag_held -> <sit>` blend first when there is
//! one ([`LandBlend`]).
//!
//! Everything here works in her `(left, bottom)` [`Placement`] space plus
//! a [`Frame`] that maps it to KWin's global coordinates (+y down).

use crate::drag::Placement;
use lillith_core::desktop::{Desktop, Landing, Rect};
use lillith_core::rng::Rng;
use lillith_core::state::GravityConfig;

/// Rebounds slower than this (px/s) settle instead of bouncing.
pub const MIN_BOUNCE_SPEED: f32 = 60.0;
/// At most this many bounces per landing.
pub const MAX_BOUNCES: u8 = 2;
/// Narrowest window top she lands on (px).
pub const MIN_PERCH_WIDTH: i32 = 80;
/// Feet within this many px of a surface count as standing on it.
pub const SNAP_PX: i32 = 3;
/// Seconds to wait for the bridge's first snapshot at start-up before
/// settling on the floor.
pub const BRIDGE_WAIT_S: f32 = 2.5;

/// Maps her placement to global desktop coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    /// Global position of her output's top-left corner.
    pub origin: (i32, i32),
    /// Output height (px), the reference of `bottom`.
    pub screen_h: i32,
    /// Feet anchor: x from the window's left edge, y up from its bottom edge.
    pub feet: (i32, i32),
}

impl Frame {
    /// Her feet (global x, y) at placement `p`.
    pub fn feet_at(&self, p: Placement) -> (i32, i32) {
        (
            self.origin.0 + p.left + self.feet.0,
            self.origin.1 + self.screen_h - p.bottom - self.feet.1,
        )
    }

    /// The `bottom` margin that puts her feet on global line `y`.
    pub fn bottom_for_feet(&self, y: i32) -> i32 {
        self.screen_h - (y - self.origin.1) - self.feet.1
    }

    /// The `left` margin that puts her feet on global column `x`.
    pub fn left_for_feet(&self, x: i32) -> i32 {
        x - self.origin.0 - self.feet.0
    }
}

/// The floor's feet line (global y): the bridge's work-area bottom, else
/// the window-bottom margin `floor_offset_px` (like `margin_bottom`).
pub fn floor_y(desk: &Desktop, frame: &Frame, floor_offset_px: i32) -> i32 {
    match desk.work_area {
        Some(a) => a.bottom(),
        None => frame.origin.1 + frame.screen_h - floor_offset_px - frame.feet.1,
    }
}

/// Wander range (`left` margins) on a window top `rect`, keeping her feet
/// at least `margin` px inside its x-span; a span too short for that
/// pins her at its centre.
pub fn perch_span(frame: &Frame, rect: Rect, margin: i32) -> (f32, f32) {
    let lo = frame.left_for_feet(rect.x + margin);
    let hi = frame.left_for_feet(rect.right() - margin);
    if hi < lo {
        let c = frame.left_for_feet(rect.x + rect.w / 2) as f32;
        (c, c)
    } else {
        (lo as f32, hi as f32)
    }
}

/// One step of a fall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallStep {
    /// Still in the air at this `bottom` (falling or bouncing).
    Moving(i32),
    /// On the ground at this `bottom`; the fall is over.
    Landed(i32),
}

/// Vertical motion in `bottom` space (+up).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fall {
    pub bottom: f32,
    /// Vertical speed, px/s, +up.
    pub vy: f32,
    pub bounces: u8,
}

impl Fall {
    pub fn new(bottom: i32) -> Self {
        Self {
            bottom: bottom as f32,
            vy: 0.0,
            bounces: 0,
        }
    }

    /// Advances by `dt` s towards `ground` (the landing `bottom`).
    pub fn step(&mut self, dt: f32, ground: f32, cfg: &GravityConfig) -> FallStep {
        if self.bottom <= ground && self.vy <= 0.0 {
            self.bottom = ground;
            return FallStep::Landed(ground.round() as i32);
        }
        let g = cfg.g_px_s2.max(0.0);
        let vmax = cfg.max_fall_speed.max(1.0);
        self.vy = (self.vy - g * dt).max(-vmax);
        let next = self.bottom + self.vy * dt;
        if next > ground {
            self.bottom = next;
            return FallStep::Moving(next.round() as i32);
        }
        let rebound = -self.vy * cfg.bounce.clamp(0.0, 0.9);
        self.bottom = ground;
        if self.bounces < MAX_BOUNCES && rebound >= MIN_BOUNCE_SPEED {
            self.bounces += 1;
            self.vy = rebound;
            FallStep::Moving(ground.round() as i32)
        } else {
            self.vy = 0.0;
            FallStep::Landed(ground.round() as i32)
        }
    }
}

/// Progress of a blend her window height follows (sit-down / stand-up), so
/// the seat meets the edge as the blend ends instead of jumping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ease {
    /// Seconds left.
    pub left_s: f32,
    /// Blend length (s).
    pub total_s: f32,
}

impl Ease {
    pub fn new(secs: f32) -> Self {
        let s = secs.max(0.0);
        Self {
            left_s: s,
            total_s: s,
        }
    }

    pub fn step(&mut self, dt: f32) {
        self.left_s = (self.left_s - dt).max(0.0);
    }

    /// Part done, 0..=1 (linear).
    pub fn fraction(&self) -> f32 {
        if self.total_s <= 0.0 {
            1.0
        } else {
            (1.0 - self.left_s / self.total_s).clamp(0.0, 1.0)
        }
    }

    pub fn done(&self) -> bool {
        self.left_s <= 0.0
    }
}

/// Screen px for `cell` px at overlay `scale`, times `part` (0..=1).
pub fn scaled_px(cell: i32, scale: f32, part: f32) -> i32 {
    (cell as f32 * scale * part).round() as i32
}

/// A sit on the window edge in progress.
#[derive(Debug, Clone, PartialEq)]
pub struct Sit {
    /// The seated base clip (one of the controller's sit clips).
    pub clip: String,
    /// Seconds left.
    pub left_s: f32,
    /// Seat line above the feet anchor in cell px (`anchor y - seat_y`):
    /// her window goes down this much (× scale) to put the seat on the edge.
    pub seat_cell: i32,
    /// A sit-down blend is playing: the drop follows it.
    pub sink: Option<Ease>,
    /// She is landing on the window through a landing blend
    /// (`blend_drag_held_to_<clip>`): until it hands over to `clip` the
    /// blend's own seat line rests on the edge.
    pub landing: Option<LandBlend>,
}

/// A landing blend in progress (see [`Sit::landing`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandBlend {
    /// The blend clip playing over the seated base.
    pub clip: String,
    /// The blend's seat line above its feet anchor in cell px.
    pub seat_cell: i32,
}

impl Sit {
    /// Px her window is lowered at `scale` right now.
    pub fn lowered_px(&self, scale: f32) -> i32 {
        if let Some(l) = &self.landing {
            // The blend drops her in from above onto its seat line: the
            // window sits at that line from the first frame.
            return scaled_px(l.seat_cell, scale, 1.0);
        }
        scaled_px(
            self.seat_cell,
            scale,
            self.sink.map_or(1.0, |e| e.fraction()),
        )
    }
}

/// Horizontal fit of a lounge (a clip lying along the window top) on a
/// window: the span her body rests on (`seat_x_range`, widened to include her
/// feet anchor so the perch check still finds her over the window), scaled to
/// screen px around her feet column `feet_x`, has to lie within `[x0, x1)`
/// shrunk by `edge` px on both sides. Returns the shift (screen px, +right)
/// that puts it there (0 if it already fits), `None` without a
/// `seat_x_range` or if the window is too narrow for the span.
pub fn lounge_shift(
    seat_x_range: Option<(i32, i32)>,
    anchor_x: i32,
    scale: f32,
    feet_x: i32,
    (x0, x1): (i32, i32),
    edge: i32,
) -> Option<i32> {
    let (a, b) = seat_x_range?;
    let lo = (a.min(b).min(anchor_x) - anchor_x) as f32 * scale;
    let hi = (a.max(b).max(anchor_x) - anchor_x) as f32 * scale;
    let (lo, hi) = (lo.floor() as i32, hi.ceil() as i32);
    let (wl, wr) = (x0 + edge, x1 - edge);
    if hi - lo > wr - wl {
        return None;
    }
    let (left, right) = (feet_x + lo, feet_x + hi);
    Some(if left < wl {
        wl - left
    } else if right > wr {
        wr - right
    } else {
        0
    })
}

/// Standing back up through a blend: the rest of a sit's drop going away.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rise {
    pub seat_cell: i32,
    pub ease: Ease,
}

/// Standing (or sitting) on top of a window.
#[derive(Debug, Clone, PartialEq)]
pub struct Perch {
    pub id: String,
    /// The window's frame when she landed (moves are detected against it).
    pub rect: Rect,
    pub sit: Option<Sit>,
    /// Seconds of calm until the next sit.
    pub next_sit_s: f32,
    /// Raising her back up while a stand-up blend plays.
    pub rise: Option<Rise>,
}

impl Perch {
    /// Px her window is below standing height at `scale` (a sit, or a rise
    /// in progress).
    pub fn lowered_px(&self, scale: f32) -> i32 {
        self.sit.as_ref().map_or(0, |s| s.lowered_px(scale))
            + self.rise.map_or(0, |r| {
                scaled_px(r.seat_cell, scale, 1.0 - r.ease.fraction())
            })
    }

    /// The `bottom` margin that rests her on this window at `scale`: feet on
    /// its top, or (sitting) the seat line on it.
    pub fn bottom(&self, frame: &Frame, scale: f32) -> i32 {
        frame.bottom_for_feet(self.rect.y) - self.lowered_px(scale)
    }
}

/// What happened to the window she stands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerchCheck {
    Ok,
    /// Same top-left, new size: keep standing, re-clamp the walk range.
    Resized(Rect),
    /// Moved, minimized, closed, or her edge is now covered at her feet.
    Lost(&'static str),
}

/// Checks the perch against the latest window stack (`feet_x` = her feet column).
pub fn check_perch(desk: &Desktop, perch: &Perch, feet_x: i32) -> PerchCheck {
    let Some(i) = desk.windows.iter().position(|w| w.id == perch.id) else {
        return PerchCheck::Lost("window closed, minimized or hidden");
    };
    let r = desk.windows[i].rect;
    if r.x != perch.rect.x || r.y != perch.rect.y {
        return PerchCheck::Lost("window moved");
    }
    if feet_x < r.x || feet_x >= r.right() {
        return PerchCheck::Lost("window shrank under her");
    }
    if desk.windows[i + 1..]
        .iter()
        .any(|o| o.rect.contains(feet_x, r.y) || o.rect.contains(feet_x, r.y - 1))
    {
        return PerchCheck::Lost("another window covers her edge");
    }
    if r != perch.rect {
        PerchCheck::Resized(r)
    } else {
        PerchCheck::Ok
    }
}

/// Gravity mode.
#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    /// Find her footing (start-up waits up to `wait` s for the bridge).
    Settle {
        wait: f32,
    },
    /// Being dragged.
    Held,
    Falling(Fall),
    /// Standing on the floor.
    Floor,
    Perched(Perch),
}

/// Gravity state owned by the overlay controller.
#[derive(Debug, Clone)]
pub struct Gravity {
    pub cfg: GravityConfig,
    pub floor_offset_px: i32,
    pub mode: Mode,
    rng: Rng,
    /// The "no bridge" hint was logged.
    pub hinted: bool,
}

impl Gravity {
    pub fn new(cfg: GravityConfig, floor_offset_px: i32, seed: u64) -> Self {
        Self {
            cfg,
            floor_offset_px,
            mode: Mode::Settle {
                wait: BRIDGE_WAIT_S,
            },
            rng: Rng::new(seed ^ 0x6A_7F17),
            hinted: false,
        }
    }

    /// Falling is on (`g_px_s2 > 0`).
    pub fn enabled(&self) -> bool {
        self.cfg.g_px_s2 > 0.0
    }

    pub fn sitting(&self) -> bool {
        matches!(&self.mode, Mode::Perched(p) if p.sit.is_some())
    }

    pub fn perch(&self) -> Option<&Perch> {
        match &self.mode {
            Mode::Perched(p) => Some(p),
            _ => None,
        }
    }

    /// Where a fall from feet `(x, y)` lands (window tops only with
    /// `perch_enabled`).
    pub fn landing(&self, desk: &Desktop, x: i32, y: i32, floor_y: i32) -> Landing {
        let windows: &[_] = if self.cfg.perch_enabled {
            &desk.windows
        } else {
            &[]
        };
        lillith_core::desktop::landing_target(windows, x, y, floor_y, MIN_PERCH_WIDTH)
    }

    /// A fresh perch on window `id` with a randomized first sit.
    pub fn new_perch(&mut self, id: String, rect: Rect) -> Perch {
        let next_sit_s = self.sit_pause();
        Perch {
            id,
            rect,
            sit: None,
            next_sit_s,
            rise: None,
        }
    }

    /// Index of the sit clip to use next, `None` without any (`n == 0`).
    pub fn pick_sit(&mut self, n: usize) -> Option<usize> {
        (n > 0).then(|| if n == 1 { 0 } else { self.rng.index(n) })
    }

    pub fn sit_pause(&mut self) -> f32 {
        let (a, b) = self.cfg.sit_every_s;
        self.rng.range(a.max(0.0), b.max(a))
    }

    pub fn sit_length(&mut self) -> f32 {
        let (a, b) = self.cfg.sit_for_s;
        self.rng.range(a.max(0.5), b.max(a))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lillith_core::desktop::DeskWindow;

    fn frame() -> Frame {
        Frame {
            origin: (0, 0),
            screen_h: 1080,
            feet: (110, 4),
        }
    }

    #[test]
    fn frame_round_trips() {
        let f = Frame {
            origin: (1920, 0),
            ..frame()
        };
        let p = Placement {
            left: 300,
            bottom: 44,
        };
        let (x, y) = f.feet_at(p);
        assert_eq!((x, y), (1920 + 410, 1032));
        assert_eq!(f.bottom_for_feet(y), 44);
        assert_eq!(f.left_for_feet(x), 300);
    }

    #[test]
    fn floor_prefers_the_bridge_work_area() {
        let mut d = Desktop::default();
        // No bridge: window bottom at floor_offset_px.
        let y = floor_y(&d, &frame(), 48);
        assert_eq!(frame().bottom_for_feet(y), 48);
        d.set_area(Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1036));
        assert_eq!(floor_y(&d, &frame(), 48), 1036);
    }

    #[test]
    fn fall_accelerates_caps_bounces_and_lands() {
        let cfg = GravityConfig::default();
        let mut f = Fall::new(900);
        let mut last = 900;
        let mut steps = 0;
        let mut bounced = false;
        let landed = loop {
            steps += 1;
            assert!(steps < 1000, "never landed");
            match f.step(1.0 / 60.0, 44.0, &cfg) {
                FallStep::Moving(b) => {
                    assert!(b >= 44, "never below the ground");
                    bounced |= b > last;
                    last = b;
                    assert!(f.vy >= -cfg.max_fall_speed - 1e-3);
                }
                FallStep::Landed(b) => break b,
            }
        };
        assert_eq!(landed, 44);
        assert!(bounced, "a soft landing bounce");
        assert!(f.bounces >= 1 && f.bounces <= MAX_BOUNCES);
        // ~856 px at 2600 px/s² is about 0.8 s plus a short bounce.
        assert!((40..120).contains(&steps), "{steps}");

        // No bounce configured: lands on first contact.
        let cfg = GravityConfig {
            bounce: 0.0,
            ..GravityConfig::default()
        };
        let mut f = Fall::new(100);
        while let FallStep::Moving(_) = f.step(0.05, 0.0, &cfg) {}
        assert_eq!(f.bounces, 0);
        // Already on/below the ground: lands at once.
        assert_eq!(Fall::new(40).step(0.016, 44.0, &cfg), FallStep::Landed(44));
    }

    #[test]
    fn terminal_speed_is_respected() {
        let cfg = GravityConfig {
            max_fall_speed: 300.0,
            ..GravityConfig::default()
        };
        let mut f = Fall::new(10_000);
        for _ in 0..100 {
            f.step(0.05, 0.0, &cfg);
        }
        assert_eq!(f.vy, -300.0);
    }

    fn desk(ws: &[(&str, Rect)]) -> Desktop {
        let mut d = Desktop::default();
        d.set_windows(
            ws.iter()
                .map(|(id, r)| DeskWindow {
                    id: id.to_string(),
                    class: "app".into(),
                    rect: *r,
                })
                .collect(),
        );
        d
    }

    #[test]
    fn perch_checks() {
        let r = Rect::new(100, 400, 800, 500);
        let p = Perch {
            id: "a".into(),
            rect: r,
            sit: None,
            next_sit_s: 1.0,
            rise: None,
        };
        assert_eq!(check_perch(&desk(&[("a", r)]), &p, 500), PerchCheck::Ok);
        let wider = Rect::new(100, 400, 900, 500);
        assert_eq!(
            check_perch(&desk(&[("a", wider)]), &p, 500),
            PerchCheck::Resized(wider)
        );
        assert!(matches!(
            check_perch(&desk(&[("a", Rect::new(120, 400, 800, 500))]), &p, 500),
            PerchCheck::Lost("window moved")
        ));
        assert!(matches!(
            check_perch(&desk(&[]), &p, 500),
            PerchCheck::Lost(_)
        ));
        assert!(matches!(
            check_perch(&desk(&[("a", Rect::new(100, 400, 300, 500))]), &p, 500),
            PerchCheck::Lost(_)
        ));
        assert!(matches!(
            check_perch(
                &desk(&[("a", r), ("b", Rect::new(450, 300, 200, 300))]),
                &p,
                500
            ),
            PerchCheck::Lost("another window covers her edge")
        ));
        // A window stacked below never covers her.
        assert_eq!(
            check_perch(
                &desk(&[("b", Rect::new(450, 300, 200, 300)), ("a", r)]),
                &p,
                500
            ),
            PerchCheck::Ok
        );
    }

    #[test]
    fn perch_span_clamps_to_the_window() {
        let f = frame();
        assert_eq!(
            perch_span(&f, Rect::new(100, 400, 800, 500), 40),
            (30.0, 750.0)
        );
        // Too narrow: centred.
        assert_eq!(
            perch_span(&f, Rect::new(100, 400, 60, 500), 40),
            (20.0, 20.0)
        );
    }

    #[test]
    fn perching_can_be_disabled() {
        let d = desk(&[("a", Rect::new(0, 400, 800, 500))]);
        let mut g = Gravity::new(GravityConfig::default(), 48, 1);
        assert_eq!(g.landing(&d, 300, 100, 1036).y(), 400);
        g.cfg.perch_enabled = false;
        assert_eq!(g.landing(&d, 300, 100, 1036), Landing::Floor { y: 1036 });
        let s = g.sit_pause();
        assert!((8.0..=20.0).contains(&s));
    }

    #[test]
    fn eases_and_seat_drops_follow_the_scale() {
        let mut e = Ease::new(0.5);
        assert_eq!(e.fraction(), 0.0);
        e.step(0.25);
        assert!((e.fraction() - 0.5).abs() < 1e-6);
        e.step(1.0);
        assert!(e.done() && e.fraction() == 1.0);
        assert_eq!(Ease::new(0.0).fraction(), 1.0);
        // Seat 162 cell px above the feet (sit_swing: 392 - 230).
        let mut sit = Sit {
            clip: "sit_swing".into(),
            left_s: 5.0,
            seat_cell: 162,
            sink: None,
            landing: None,
        };
        assert_eq!(sit.lowered_px(0.55), 89);
        assert_eq!(sit.lowered_px(1.0), 162);
        sit.sink = Some(Ease {
            left_s: 0.25,
            total_s: 1.0,
        });
        assert_eq!(sit.lowered_px(1.0), 122);
        // Window top at y 300 of an 800 px output, feet 4 px above the
        // window bottom: standing bottom 496; seated at 0.55 / 1.0.
        let frame = Frame {
            origin: (0, 0),
            screen_h: 800,
            feet: (20, 4),
        };
        let mut p = Perch {
            id: "a".into(),
            rect: Rect::new(0, 300, 600, 400),
            sit: None,
            next_sit_s: 1.0,
            rise: None,
        };
        assert_eq!(p.bottom(&frame, 0.55), 496);
        assert_eq!(p.bottom(&frame, 1.0), 496);
        p.sit = Some(Sit { sink: None, ..sit });
        assert_eq!(p.bottom(&frame, 0.55), 496 - 89);
        assert_eq!(p.bottom(&frame, 1.0), 496 - 162);
        // Rising: the part of the drop not yet undone.
        p.sit = None;
        p.rise = Some(Rise {
            seat_cell: 162,
            ease: Ease {
                left_s: 0.25,
                total_s: 1.0,
            },
        });
        assert_eq!(p.lowered_px(1.0), 41);
    }

    #[test]
    fn landing_blend_rests_on_its_own_seat_line_until_it_hands_over() {
        // Blend seat 152 cell px above the feet (392 - 240), the base's 142.
        let mut sit = Sit {
            clip: "perch_sit".into(),
            left_s: 10.0,
            seat_cell: 142,
            sink: None,
            landing: Some(LandBlend {
                clip: "blend_drag_held_to_perch_sit".into(),
                seat_cell: 152,
            }),
        };
        // From the first frame at any scale (no sink: she drops in from
        // above onto the line).
        assert_eq!(sit.lowered_px(0.5), 76);
        assert_eq!(sit.lowered_px(0.559), 85);
        assert_eq!(sit.lowered_px(1.0), 152);
        // A sink, if any, does not apply while landing.
        sit.sink = Some(Ease::new(1.0));
        assert_eq!(sit.lowered_px(1.0), 152);
        // Handed over: the base's seat line.
        sit.sink = None;
        sit.landing = None;
        assert_eq!(sit.lowered_px(0.5), 71);
        assert_eq!(sit.lowered_px(1.0), 142);
        // On a window top at y 300 (standing bottom 496, as above).
        let frame = Frame {
            origin: (0, 0),
            screen_h: 800,
            feet: (20, 4),
        };
        let p = Perch {
            id: "a".into(),
            rect: Rect::new(0, 300, 600, 400),
            sit: Some(Sit {
                landing: Some(LandBlend {
                    clip: "b".into(),
                    seat_cell: 152,
                }),
                ..sit
            }),
            next_sit_s: 1.0,
            rise: None,
        };
        assert_eq!(p.bottom(&frame, 0.5), 496 - 76);
        assert_eq!(p.bottom(&frame, 1.0), 496 - 152);
    }

    #[test]
    fn lounge_shift_fits_the_window_or_refuses() {
        // perch_lounge: rests on x 197..372 of a 400 px cell, anchor x 200.
        let r = Some((197, 372));
        let win = (100, 900);
        // At scale 1: span -3..172 around the feet; fits mid-window.
        assert_eq!(lounge_shift(r, 200, 1.0, 400, win, 8), Some(0));
        // Too far right: moved left so the span ends 8 px inside.
        assert_eq!(lounge_shift(r, 200, 1.0, 800, win, 8), Some(892 - 972));
        // Too far left (the span widened to her feet): moved right.
        assert_eq!(lounge_shift(r, 200, 1.0, 100, win, 8), Some(108 - 97));
        // Scale shrinks the span: 0.559 -> -2..97 (floor/ceil).
        assert_eq!(lounge_shift(r, 200, 0.559, 790, win, 8), Some(0));
        assert_eq!(lounge_shift(r, 200, 0.559, 850, win, 8), Some(892 - 947));
        // Window narrower than the span (+ edges): no lounge.
        assert_eq!(lounge_shift(r, 200, 1.0, 400, (300, 480), 8), None);
        assert_eq!(lounge_shift(r, 200, 0.5, 400, (300, 400), 8), None);
        // Exactly fits: 175 px span in a 191 px top.
        assert_eq!(lounge_shift(r, 200, 1.0, 300, (300, 491), 8), Some(11));
        // A reversed range counts the same; no range: no lounge.
        assert_eq!(
            lounge_shift(Some((372, 197)), 200, 1.0, 800, win, 8),
            Some(-80)
        );
        assert_eq!(lounge_shift(None, 200, 1.0, 400, win, 8), None);
    }

    #[test]
    fn picks_a_sit_clip_only_when_there_is_one() {
        let mut g = Gravity::new(GravityConfig::default(), 0, 7);
        assert_eq!(g.pick_sit(0), None);
        assert_eq!(g.pick_sit(1), Some(0));
        let mut seen = [false; 3];
        for _ in 0..64 {
            seen[g.pick_sit(3).unwrap()] = true;
        }
        assert_eq!(seen, [true; 3]);
    }
}
