//! Drag and placement logic shared by both backends (compositor-free, so
//! it is unit-tested).
//!
//! Position is `(left, bottom)`: px from the output's left edge to the
//! window's left edge, and from the output's bottom edge to the window's
//! bottom edge. On Wayland these are the layer-surface margins (anchor
//! `BOTTOM | LEFT`); on X11 they convert to a window origin with
//! [`Placement::to_xy`].

use lillith_core::config::OverlayPos;

/// Pointer travel (px) before a press becomes a drag instead of a click.
pub const DRAG_THRESHOLD: f64 = 4.0;

/// Window placement on one output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub left: i32,
    pub bottom: i32,
}

impl Placement {
    pub fn from_config(p: &OverlayPos) -> Self {
        Self {
            left: p.margin_left,
            bottom: p.margin_bottom,
        }
    }

    /// Keeps at least `keep` px of the `win` window on an `output`-sized
    /// screen (so she can peek off an edge but never get lost).
    pub fn clamped(self, win: (u32, u32), output: (u32, u32), keep: i32) -> Self {
        let (ww, wh) = (win.0 as i32, win.1 as i32);
        let (ow, oh) = (output.0 as i32, output.1 as i32);
        let keep_x = keep.min(ww);
        let keep_y = keep.min(wh);
        Self {
            left: self.left.clamp(keep_x - ww, (ow - keep_x).max(keep_x - ww)),
            bottom: self
                .bottom
                .clamp(keep_y - wh, (oh - keep_y).max(keep_y - wh)),
        }
    }

    /// Top-left window origin on an output of height `output_h` (X11).
    pub fn to_xy(self, win_h: u32, output_h: u32) -> (i32, i32) {
        (self.left, output_h as i32 - self.bottom - win_h as i32)
    }

    /// Inverse of [`Placement::to_xy`].
    pub fn from_xy(x: i32, y: i32, win_h: u32, output_h: u32) -> Self {
        Self {
            left: x,
            bottom: output_h as i32 - y - win_h as i32,
        }
    }

    /// Moves by a screen-space delta (+y down).
    pub fn moved(self, dx: i32, dy: i32) -> Self {
        Self {
            left: self.left + dx,
            bottom: self.bottom - dy,
        }
    }
}

/// What a pointer release meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Release {
    /// Pressed and released without moving: a click on her.
    Click,
    /// The press moved her; persist the new placement.
    Dropped(Placement),
    /// No press was active.
    None,
}

/// Drag state machine. Feed it pointer positions in one consistent frame:
/// surface-local on Wayland (the surface moves with the drag, so the
/// press point stays put under the cursor), or root/global on X11.
#[derive(Debug, Clone, Default)]
pub struct Drag {
    press: Option<Press>,
}

#[derive(Debug, Clone, Copy)]
struct Press {
    at: (f64, f64),
    start: Placement,
    dragging: bool,
    /// Wayland: positions are surface-local (relative to `committed`).
    local: bool,
}

impl Drag {
    /// Starts a press at `at` while the window is at `placement`.
    /// `local` = positions are window-relative (Wayland).
    pub fn press(&mut self, at: (f64, f64), placement: Placement, local: bool) {
        self.press = Some(Press {
            at,
            start: placement,
            dragging: false,
            local,
        });
    }

    pub fn active(&self) -> bool {
        self.press.is_some()
    }

    /// `true` once the press has moved past [`DRAG_THRESHOLD`].
    pub fn dragging(&self) -> bool {
        self.press.is_some_and(|p| p.dragging)
    }

    /// Pointer moved to `at`; `current` is where the window is now. Returns
    /// the new placement while dragging.
    pub fn motion(&mut self, at: (f64, f64), current: Placement) -> Option<Placement> {
        let p = self.press.as_mut()?;
        let (dx, dy) = (at.0 - p.at.0, at.1 - p.at.1);
        if !p.dragging && dx.hypot(dy) < DRAG_THRESHOLD {
            return None;
        }
        p.dragging = true;
        let (dx, dy) = (dx.round() as i32, dy.round() as i32);
        if p.local {
            // The window already moved by everything up to `current`; the
            // press point is fixed in window coordinates.
            if dx == 0 && dy == 0 {
                return None;
            }
            Some(current.moved(dx, dy))
        } else {
            Some(p.start.moved(dx, dy))
        }
    }

    /// Button released while the window is at `current`.
    pub fn release(&mut self, current: Placement) -> Release {
        match self.press.take() {
            None => Release::None,
            Some(p) if p.dragging => Release::Dropped(current),
            Some(_) => Release::Click,
        }
    }

    pub fn cancel(&mut self) {
        self.press = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xy_round_trip_and_clamp() {
        let p = Placement {
            left: 100,
            bottom: 48,
        };
        let (x, y) = p.to_xy(300, 1080);
        assert_eq!((x, y), (100, 732));
        assert_eq!(Placement::from_xy(x, y, 300, 1080), p);
        let off = Placement {
            left: -5000,
            bottom: 9000,
        }
        .clamped((300, 300), (1920, 1080), 64);
        assert_eq!(
            off,
            Placement {
                left: -236,
                bottom: 1016
            }
        );
    }

    #[test]
    fn click_vs_drag_global() {
        let start = Placement {
            left: 10,
            bottom: 20,
        };
        let mut d = Drag::default();
        d.press((500.0, 500.0), start, false);
        assert_eq!(d.motion((502.0, 501.0), start), None, "below threshold");
        assert_eq!(d.release(start), Release::Click);

        d.press((500.0, 500.0), start, false);
        let p = d.motion((530.0, 480.0), start).unwrap();
        assert_eq!(
            p,
            Placement {
                left: 40,
                bottom: 40
            }
        );
        // Global positions are absolute from the press.
        let p2 = d.motion((540.0, 480.0), p).unwrap();
        assert_eq!(
            p2,
            Placement {
                left: 50,
                bottom: 40
            }
        );
        assert_eq!(d.release(p2), Release::Dropped(p2));
        assert_eq!(d.release(p2), Release::None);
    }

    #[test]
    fn drag_surface_local() {
        let mut cur = Placement { left: 0, bottom: 0 };
        let mut d = Drag::default();
        d.press((50.0, 50.0), cur, true);
        // Cursor moves +10,+10 relative to the surface; surface follows.
        cur = d.motion((60.0, 60.0), cur).unwrap();
        assert_eq!(
            cur,
            Placement {
                left: 10,
                bottom: -10
            }
        );
        // After the compositor applied the move, the cursor is back at the
        // press point in surface coordinates: no further movement.
        assert_eq!(d.motion((50.0, 50.0), cur), None);
        cur = d.motion((45.0, 50.0), cur).unwrap();
        assert_eq!(
            cur,
            Placement {
                left: 5,
                bottom: -10
            }
        );
        assert!(d.dragging());
        assert_eq!(d.release(cur), Release::Dropped(cur));
    }
}
