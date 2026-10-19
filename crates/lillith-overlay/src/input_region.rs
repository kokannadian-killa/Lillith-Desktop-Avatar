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
}
