//! The desktop as Lillith's gravity sees it: a z-ordered model of the
//! visible normal windows (frame geometry) plus the usable work area, and
//! the pure [`landing_target`] that decides where she lands after a drop.
//!
//! The model is filled by `lillith-sense`'s KWin bridge receiver
//! (`kwin/lillith-bridge` sends `Stack` and `Area` snapshots) and read by
//! the overlay's gravity each frame through [`shared`]. Coordinates are
//! KWin's global logical px, +y down. Without the bridge the model stays
//! empty ([`Desktop::has_bridge`] = `false`) and she lands on the floor.

use std::sync::{Mutex, OnceLock};

/// Axis-aligned rectangle, global logical px (+y down).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
    /// Half-open containment: `[x, right) × [y, bottom)`.
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.right() && py >= self.y && py < self.bottom()
    }
}

/// One visible normal window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeskWindow {
    /// KWin `internalId` (stable while the window lives).
    pub id: String,
    pub class: String,
    /// Frame geometry (decorations included).
    pub rect: Rect,
}

/// Snapshot of the desktop.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Desktop {
    /// Visible normal windows, bottom → top (KWin stacking order).
    pub windows: Vec<DeskWindow>,
    /// Full geometry of the screen she is on (`FullScreenArea`).
    pub screen: Option<Rect>,
    /// Usable area of that screen, panels excluded (`MaximizeArea`).
    pub work_area: Option<Rect>,
    /// A bridge snapshot arrived at least once.
    pub has_bridge: bool,
    /// Bumped on every update (cheap change detection).
    pub generation: u64,
}

impl Desktop {
    pub fn window(&self, id: &str) -> Option<&DeskWindow> {
        self.windows.iter().find(|w| w.id == id)
    }

    /// Replaces the window stack (a `Stack` snapshot).
    pub fn set_windows(&mut self, windows: Vec<DeskWindow>) {
        self.windows = windows;
        self.has_bridge = true;
        self.generation = self.generation.wrapping_add(1);
    }

    /// Sets screen + work area (an `Area` snapshot).
    pub fn set_area(&mut self, screen: Rect, work_area: Rect) {
        self.screen = Some(screen);
        self.work_area = Some(work_area);
        self.has_bridge = true;
        self.generation = self.generation.wrapping_add(1);
    }
}

/// The process-wide model (written by the bridge task, read by the overlay).
pub fn shared() -> &'static Mutex<Desktop> {
    static D: OnceLock<Mutex<Desktop>> = OnceLock::new();
    D.get_or_init(|| Mutex::new(Desktop::default()))
}

/// A clone of the shared model (a handful of windows; cheap at 30 Hz).
pub fn snapshot() -> Desktop {
    shared().lock().map(|d| d.clone()).unwrap_or_default()
}

/// Her own surface and shell surfaces never count as windows.
pub fn ignored_class(class: &str) -> bool {
    let c = class.to_ascii_lowercase();
    ["lillith", "plasmashell", "org.kde.plasmashell", "krunner"]
        .iter()
        .any(|k| c == *k)
}

/// Parses a `Stack` snapshot: one window per line, bottom → top,
/// `id \t class \t x \t y \t w \t h` (numbers may be floats). Malformed
/// lines, empty or degenerate rects and [`ignored_class`] windows are dropped.
pub fn parse_stack(s: &str) -> Vec<DeskWindow> {
    s.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() != 6 || f[0].is_empty() {
                return None;
            }
            let n = |i: usize| f[i].trim().parse::<f64>().ok().map(|v| v.round() as i32);
            let rect = Rect::new(n(2)?, n(3)?, n(4)?, n(5)?);
            (rect.w > 0 && rect.h > 0 && !ignored_class(f[1])).then(|| DeskWindow {
                id: f[0].to_string(),
                class: f[1].to_string(),
                rect,
            })
        })
        .collect()
}

/// Parses an `Area` snapshot: `sx sy sw sh ax ay aw ah` (screen, then work
/// area; whitespace separated, floats allowed).
pub fn parse_area(s: &str) -> Option<(Rect, Rect)> {
    let v: Vec<i32> = s
        .split_whitespace()
        .map(|t| t.parse::<f64>().ok().map(|v| v.round() as i32))
        .collect::<Option<_>>()?;
    if v.len() != 8 || v[2] <= 0 || v[3] <= 0 || v[6] <= 0 || v[7] <= 0 {
        return None;
    }
    Some((
        Rect::new(v[0], v[1], v[2], v[3]),
        Rect::new(v[4], v[5], v[6], v[7]),
    ))
}

/// Where a fall that starts with her feet at `(x, y)` ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Landing {
    /// The floor (bottom of the usable area) at feet line `y`.
    Floor { y: i32 },
    /// The top edge (`top`) of window `id`, which spans `[x0, x1)`.
    Window {
        id: String,
        top: i32,
        x0: i32,
        x1: i32,
    },
}

impl Landing {
    /// Feet line she lands on.
    pub fn y(&self) -> i32 {
        match self {
            Self::Floor { y } => *y,
            Self::Window { top, .. } => *top,
        }
    }
}

/// The first visible surface under her feet: among `windows` (bottom →
/// top) whose x-span contains `x`, at least `min_width` wide, with a top
/// edge at or below `y` and above `floor_y`, the highest edge (smallest
/// `top`) that no window stacked above it covers at `x` (the edge pixel or
/// the one just above it). Equal edges go to the topmost window. Nothing
/// found: [`Landing::Floor`].
pub fn landing_target(
    windows: &[DeskWindow],
    x: i32,
    y: i32,
    floor_y: i32,
    min_width: i32,
) -> Landing {
    let mut best: Option<(usize, i32)> = None;
    for (i, w) in windows.iter().enumerate() {
        let r = &w.rect;
        let top = r.y;
        if r.w < min_width || x < r.x || x >= r.right() || top < y || top >= floor_y {
            continue;
        }
        let covered = windows[i + 1..]
            .iter()
            .any(|o| o.rect.contains(x, top) || o.rect.contains(x, top - 1));
        if covered {
            continue;
        }
        if best.is_none_or(|(_, t)| top <= t) {
            best = Some((i, top));
        }
    }
    match best {
        Some((i, top)) => {
            let r = windows[i].rect;
            Landing::Window {
                id: windows[i].id.clone(),
                top,
                x0: r.x,
                x1: r.right(),
            }
        }
        None => Landing::Floor { y: floor_y },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(id: &str, x: i32, y: i32, w: i32, h: i32) -> DeskWindow {
        DeskWindow {
            id: id.into(),
            class: "app".into(),
            rect: Rect::new(x, y, w, h),
        }
    }

    const FLOOR: i32 = 1032;

    #[test]
    fn empty_desktop_lands_on_the_floor() {
        assert_eq!(
            landing_target(&[], 500, 100, FLOOR, 40),
            Landing::Floor { y: FLOOR }
        );
    }

    #[test]
    fn lands_on_the_window_below_her_feet() {
        let ws = [win("a", 100, 400, 800, 500)];
        let l = landing_target(&ws, 500, 120, FLOOR, 40);
        assert_eq!(
            l,
            Landing::Window {
                id: "a".into(),
                top: 400,
                x0: 100,
                x1: 900
            }
        );
        // Released exactly on the edge: still lands there.
        assert_eq!(landing_target(&ws, 500, 400, FLOOR, 40).y(), 400);
        // Feet already below the edge (in front of the window): floor.
        assert_eq!(landing_target(&ws, 500, 401, FLOOR, 40).y(), FLOOR);
        // Outside the x-span (right edge is exclusive).
        assert_eq!(landing_target(&ws, 900, 120, FLOOR, 40).y(), FLOOR);
        assert_eq!(landing_target(&ws, 99, 120, FLOOR, 40).y(), FLOOR);
        assert_eq!(landing_target(&ws, 100, 120, FLOOR, 40).y(), 400);
    }

    #[test]
    fn nearest_visible_edge_wins() {
        // b (top 300) is above a (top 600) in z and does not cover a's edge at x=800.
        let ws = [win("a", 0, 600, 1000, 300), win("b", 0, 300, 500, 200)];
        assert_eq!(landing_target(&ws, 200, 50, FLOOR, 40).y(), 300);
        assert_eq!(landing_target(&ws, 800, 50, FLOOR, 40).y(), 600);
        // Below b's top already: she falls on through to a.
        assert_eq!(landing_target(&ws, 200, 350, FLOOR, 40).y(), 600);
    }

    #[test]
    fn occluded_edges_are_skipped() {
        // a's top edge (y 500) at x 300 is behind b (stacked above a).
        let ws = [win("a", 0, 500, 1000, 300), win("b", 200, 400, 300, 400)];
        match landing_target(&ws, 300, 100, FLOOR, 40) {
            Landing::Window { id, top, .. } => assert_eq!((id.as_str(), top), ("b", 400)),
            l => panic!("{l:?}"),
        }
        // Same windows, reverse z: a is on top, so b's edge at 400 is
        // visible (a starts at 500) and wins as the higher one.
        let ws = [win("b", 200, 400, 300, 400), win("a", 0, 500, 1000, 300)];
        assert_eq!(landing_target(&ws, 300, 100, FLOOR, 40).y(), 400);
        // b fully behind a big window c whose top is higher: only c counts.
        let ws = [win("b", 200, 400, 300, 100), win("c", 0, 200, 1000, 600)];
        match landing_target(&ws, 300, 100, FLOOR, 40) {
            Landing::Window { id, .. } => assert_eq!(id, "c"),
            l => panic!("{l:?}"),
        }
        // Released inside c's face (below its top): b's edge is hidden, floor.
        assert_eq!(landing_target(&ws, 300, 250, FLOOR, 40).y(), FLOOR);
        // A window sitting flush on the edge (bottom == edge) covers the
        // pixel just above it.
        let ws = [win("a", 0, 500, 1000, 300), win("d", 250, 300, 100, 200)];
        assert_eq!(landing_target(&ws, 300, 100, FLOOR, 40).y(), 300);
    }

    #[test]
    fn equal_edges_go_to_the_topmost_window() {
        let ws = [
            win("low", 0, 400, 600, 300),
            win("high", 300, 400, 600, 300),
        ];
        match landing_target(&ws, 450, 0, FLOOR, 40) {
            Landing::Window { id, .. } => assert_eq!(id, "high"),
            l => panic!("{l:?}"),
        }
    }

    #[test]
    fn narrow_and_below_floor_windows_are_ignored() {
        let ws = [win("thin", 480, 300, 30, 200), win("low", 0, 1040, 900, 40)];
        assert_eq!(landing_target(&ws, 490, 0, FLOOR, 40).y(), FLOOR);
        assert_eq!(landing_target(&ws, 490, 0, FLOOR, 20).y(), 300);
    }

    #[test]
    fn parses_bridge_snapshots() {
        let s = "11\tfirefox\t10\t20.4\t800\t600\n\
                 bad line\n\
                 12\tlillith\t0\t0\t300\t300\n\
                 13\tkonsole\t5\t6\t0\t100\n\
                 14\torg.kde.konsole\t-20\t700\t640.6\t300\n";
        let ws = parse_stack(s);
        assert_eq!(
            ws,
            [
                DeskWindow {
                    id: "11".into(),
                    class: "firefox".into(),
                    rect: Rect::new(10, 20, 800, 600)
                },
                DeskWindow {
                    id: "14".into(),
                    class: "org.kde.konsole".into(),
                    rect: Rect::new(-20, 700, 641, 300)
                },
            ]
        );
        assert_eq!(parse_stack(""), []);
        assert_eq!(
            parse_area("0 0 1920 1080 0 0 1920 1036"),
            Some((Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1036)))
        );
        assert_eq!(parse_area("0 0 1920"), None);
        assert_eq!(parse_area("0 0 0 1080 0 0 1920 1036"), None);
        assert_eq!(parse_area("a b c d e f g h"), None);
    }

    #[test]
    fn model_updates_bump_the_generation() {
        let mut d = Desktop::default();
        assert!(!d.has_bridge);
        d.set_windows(vec![win("a", 0, 0, 10, 10)]);
        assert!(d.has_bridge && d.generation == 1);
        assert_eq!(d.window("a").map(|w| w.rect.w), Some(10));
        d.set_area(Rect::new(0, 0, 10, 10), Rect::new(0, 0, 10, 9));
        assert_eq!(d.generation, 2);
        assert!(ignored_class("Lillith") && !ignored_class("dolphin"));
    }
}
