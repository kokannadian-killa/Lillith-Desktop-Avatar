//! Compositor-free geometry and policy helpers for the Wayland backend
//! (unit-tested here because KWin can't run in CI).
//!
//! - [`menu_layer_margins`]: where the fallback menu layer surface goes
//!   (anchor `BOTTOM | LEFT`) so its top-left sits at the pointer and it
//!   stays on screen.
//! - [`popup_timed_out`]: the `xdg_popup` never configured in time, switch
//!   to the layer-surface menu.
//! - [`size_mismatch`]: the layer `configure` size differs from the buffer.
//! - [`alpha_stats`]: transparency self-check of a premultiplied buffer
//!   (logged once so a user's log shows whether the corners really are
//!   clear when the compositor shows a black box).

use crate::drag::Placement;
use std::time::Duration;

/// How long the menu `xdg_popup` may stay unconfigured before the overlay
/// shows the menu as its own layer surface instead.
pub const POPUP_CONFIGURE_TIMEOUT: Duration = Duration::from_millis(500);

/// Margins `(left, bottom)` for a `menu`-sized layer surface anchored
/// `BOTTOM | LEFT` whose top-left is at `at` (surface-local px on her
/// `stage`-sized surface placed at `placement`), clamped to `output`.
pub fn menu_layer_margins(
    placement: Placement,
    stage: (u32, u32),
    at: (f64, f64),
    menu: (u32, u32),
    output: Option<(u32, u32)>,
) -> (i32, i32) {
    let (mw, mh) = (menu.0 as i32, menu.1 as i32);
    let x = placement.left + at.0.round() as i32;
    // Distance from the output bottom to the pointer.
    let from_bottom = placement.bottom + stage.1 as i32 - at.1.round() as i32;
    let mut bottom = from_bottom - mh;
    let mut left = x;
    if let Some((ow, oh)) = output {
        let (ow, oh) = (ow as i32, oh as i32);
        if left + mw > ow {
            // Flip to the pointer's left side.
            left = x - mw;
        }
        if bottom < 0 {
            // Not enough room below the pointer: open upward.
            bottom = from_bottom;
        }
        left = left.clamp(0, (ow - mw).max(0));
        bottom = bottom.clamp(0, (oh - mh).max(0));
    } else {
        left = left.max(0);
        bottom = bottom.max(0);
    }
    (left, bottom)
}

/// `true` when an `xdg_popup` opened `elapsed` ago still has no configure.
pub fn popup_timed_out(elapsed: Duration, configured: bool) -> bool {
    !configured && elapsed >= POPUP_CONFIGURE_TIMEOUT
}

/// `Some(configured)` when the compositor configured the layer surface with
/// a size other than the `buffer` we attach (`0` = "client decides").
pub fn size_mismatch(buffer: (u32, u32), configured: (u32, u32)) -> Option<(u32, u32)> {
    let w_ok = configured.0 == 0 || configured.0 == buffer.0;
    let h_ok = configured.1 == 0 || configured.1 == buffer.1;
    (!(w_ok && h_ok)).then_some(configured)
}

/// Alpha summary of an `Argb8888` little-endian buffer (`B, G, R, A`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AlphaStats {
    pub clear: usize,
    pub partial: usize,
    pub opaque: usize,
    /// Pixels with alpha 0 but a colour (invalid premultiplied data).
    pub bad_premul: usize,
    /// Alpha of the four corners (top-left, top-right, bottom-left, bottom-right).
    pub corners: [u8; 4],
}

/// Scans an `Argb8888` LE buffer of `w`×`h`.
pub fn alpha_stats(buf: &[u8], w: u32, h: u32) -> AlphaStats {
    let mut s = AlphaStats::default();
    for px in buf.chunks_exact(4) {
        match px[3] {
            0 => {
                s.clear += 1;
                if px[0] | px[1] | px[2] != 0 {
                    s.bad_premul += 1;
                }
            }
            255 => s.opaque += 1,
            _ => s.partial += 1,
        }
    }
    let (w, h) = (w as usize, h as usize);
    if w > 0 && h > 0 && buf.len() >= w * h * 4 {
        let a = |x: usize, y: usize| buf[(y * w + x) * 4 + 3];
        s.corners = [a(0, 0), a(w - 1, 0), a(0, h - 1), a(w - 1, h - 1)];
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_opens_at_the_pointer_and_stays_on_screen() {
        let p = Placement {
            left: 100,
            bottom: 50,
        };
        // Pointer at (40, 60) on a 220x271 surface: screen x 140, 261 px
        // above the bottom edge; a 232x200 menu hangs below it.
        let m = menu_layer_margins(p, (220, 271), (40.0, 60.0), (232, 200), Some((1920, 1080)));
        assert_eq!(m, (140, 61));
        // Near the right edge: flips left of the pointer.
        let p2 = Placement {
            left: 1800,
            bottom: 50,
        };
        let m = menu_layer_margins(p2, (220, 271), (40.0, 60.0), (232, 200), Some((1920, 1080)));
        assert_eq!(m.0, 1840 - 232);
        // Near the bottom: opens upward, never below 0.
        let p3 = Placement { left: 0, bottom: 0 };
        let m = menu_layer_margins(
            p3,
            (220, 271),
            (10.0, 250.0),
            (232, 200),
            Some((1920, 1080)),
        );
        assert_eq!(m, (10, 21));
        // Off-screen placement still lands inside.
        let p4 = Placement {
            left: -500,
            bottom: 5000,
        };
        let (l, b) = menu_layer_margins(p4, (220, 271), (0.0, 0.0), (232, 200), Some((1366, 768)));
        assert!((0..=1366 - 232).contains(&l) && (0..=768 - 200).contains(&b));
    }

    #[test]
    fn popup_timeout_only_when_unconfigured() {
        assert!(!popup_timed_out(Duration::from_millis(100), false));
        assert!(popup_timed_out(Duration::from_millis(600), false));
        assert!(!popup_timed_out(Duration::from_secs(5), true));
    }

    #[test]
    fn buffer_size_must_match_the_configured_surface() {
        assert_eq!(size_mismatch((220, 271), (220, 271)), None);
        assert_eq!(size_mismatch((220, 271), (0, 0)), None);
        assert_eq!(size_mismatch((220, 271), (440, 542)), Some((440, 542)));
    }

    #[test]
    fn alpha_stats_finds_clear_corners_and_bad_premul() {
        // 2x2: clear, opaque red, partial, alpha-0-with-colour.
        let buf = [0, 0, 0, 0, 0, 0, 255, 255, 10, 10, 10, 128, 5, 0, 0, 0];
        let s = alpha_stats(&buf, 2, 2);
        assert_eq!((s.clear, s.opaque, s.partial, s.bad_premul), (2, 1, 1, 1));
        assert_eq!(s.corners, [0, 255, 128, 0]);
    }
}
