//! Click-through: turn a frame's alpha channel into input-region rectangles.
//!
//! On Wayland these become `wl_region.add(x, y, w, h)` calls on every frame
//! change; on X11 the same mask drives `set_cursor_hittest()`. The asset
//! pipeline precomputes the runs per frame at build time; this function is
//! the reference implementation.

/// A horizontal run of opaque pixels: `(x, y, width)`, height 1.
pub type Run = (u32, u32, u32);

/// Scans an RGBA8 buffer and returns the opaque runs (alpha ≥ `threshold`).
pub fn alpha_runs(rgba: &[u8], width: u32, height: u32, threshold: u8) -> Vec<Run> {
    let mut runs = Vec::new();
    for y in 0..height {
        let mut start: Option<u32> = None;
        for x in 0..width {
            let a = rgba[((y * width + x) * 4 + 3) as usize];
            match (a >= threshold, start) {
                (true, None) => start = Some(x),
                (false, Some(s)) => {
                    runs.push((s, y, x - s));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            runs.push((s, y, width - s));
        }
    }
    runs
}

/// An input-region rectangle `(x, y, w, h)` in surface pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// Scans a premultiplied/straight RGBA8 buffer row by row and returns the
/// silhouette as rectangles: opaque runs, snapped outward to a `grid`-px
/// horizontal grid (1 = exact), with identical runs on consecutive rows
/// merged into one taller rectangle. That keeps a 300 px tall sprite to a
/// few dozen `wl_region.add` / XShape rectangles instead of thousands.
pub fn mask_rects(rgba: &[u8], width: u32, height: u32, threshold: u8, grid: u32) -> Vec<Rect> {
    let grid = grid.max(1) as i32;
    let w = width as i32;
    let mut done: Vec<Rect> = Vec::new();
    // Rects still growing downward, keyed by (x, w).
    let mut open: Vec<Rect> = Vec::new();
    let mut row: Vec<(i32, i32)> = Vec::new();
    for y in 0..height as i32 {
        row.clear();
        let base = (y * w) as usize * 4;
        let mut start: Option<i32> = None;
        for x in 0..=w {
            let opaque = x < w && rgba[base + x as usize * 4 + 3] >= threshold;
            match (opaque, start) {
                (true, None) => start = Some(x),
                (false, Some(s)) => {
                    let x0 = (s / grid) * grid;
                    let x1 = ((x + grid - 1) / grid * grid).min(w);
                    // Snapping can make neighbours touch: merge them.
                    match row.last_mut() {
                        Some(last) if last.0 + last.1 >= x0 => last.1 = x1 - last.0,
                        _ => row.push((x0, x1 - x0)),
                    }
                    start = None;
                }
                _ => {}
            }
        }
        let mut next_open = Vec::with_capacity(row.len());
        for &(x, rw) in &row {
            if let Some(i) = open.iter().position(|r| r.x == x && r.w == rw) {
                let mut r = open.swap_remove(i);
                r.h += 1;
                next_open.push(r);
            } else {
                next_open.push(Rect { x, y, w: rw, h: 1 });
            }
        }
        done.append(&mut open);
        open = next_open;
    }
    done.append(&mut open);
    done.sort_by_key(|r| (r.y, r.x));
    done
}

/// `true` if the point is inside any rectangle.
pub fn rects_hit(rects: &[Rect], px: i32, py: i32) -> bool {
    rects.iter().any(|r| r.contains(px, py))
}

/// `true` if the point is inside her silhouette.
pub fn hit_test(runs: &[Run], px: u32, py: u32) -> bool {
    runs.iter()
        .any(|&(x, y, w)| y == py && px >= x && px < x + w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_and_hit_test() {
        // 4x2 image: row 0 = [opaque, opaque, clear, opaque], row 1 all clear.
        let mut px = vec![0u8; 4 * 2 * 4];
        for x in [0usize, 1, 3] {
            px[x * 4 + 3] = 255;
        }
        let runs = alpha_runs(&px, 4, 2, 128);
        assert_eq!(runs, vec![(0, 0, 2), (3, 0, 1)]);
        assert!(hit_test(&runs, 1, 0));
        assert!(!hit_test(&runs, 2, 0));
        assert!(!hit_test(&runs, 0, 1));
    }

    fn img(w: u32, h: u32, opaque: impl Fn(u32, u32) -> bool) -> Vec<u8> {
        let mut px = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                if opaque(x, y) {
                    px[((y * w + x) * 4 + 3) as usize] = 255;
                }
            }
        }
        px
    }

    #[test]
    fn rects_merge_rows_and_cover_exactly() {
        // A 10x10 image with a 4x6 block at (3,2) and a 1px dot at (9,9).
        let px = img(10, 10, |x, y| {
            ((3..7).contains(&x) && (2..8).contains(&y)) || (x == 9 && y == 9)
        });
        let rects = mask_rects(&px, 10, 10, 128, 1);
        assert_eq!(
            rects,
            vec![
                Rect {
                    x: 3,
                    y: 2,
                    w: 4,
                    h: 6
                },
                Rect {
                    x: 9,
                    y: 9,
                    w: 1,
                    h: 1
                }
            ]
        );
        for y in 0..10 {
            for x in 0..10 {
                let want = px[((y * 10 + x) * 4 + 3) as usize] >= 128;
                assert_eq!(rects_hit(&rects, x, y), want, "({x},{y})");
            }
        }
    }

    #[test]
    fn grid_snaps_outward_and_never_loses_pixels() {
        // Diagonal-ish blob: every opaque pixel must stay clickable.
        let px = img(37, 23, |x, y| (x as i32 - y as i32 - 5).abs() < 4);
        let exact = mask_rects(&px, 37, 23, 1, 1);
        let coarse = mask_rects(&px, 37, 23, 1, 4);
        for y in 0..23 {
            for x in 0..37 {
                if px[((y * 37 + x) * 4 + 3) as usize] > 0 {
                    assert!(rects_hit(&coarse, x, y));
                    assert!(rects_hit(&exact, x, y));
                }
            }
        }
        assert!(coarse.iter().all(|r| r.x % 4 == 0 && r.x + r.w <= 37));
        // Empty image -> no rects (fully click-through).
        assert!(mask_rects(&[0; 16 * 4], 4, 4, 1, 2).is_empty());
    }
}
