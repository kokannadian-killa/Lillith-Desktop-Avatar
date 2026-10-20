//! Pointer gesture recognition (PLAN.md §3.3 `core::interaction`, v01
//! bindings): turns raw clicks and hover enter/leave from the overlay into
//! [`Gesture`]s, on a fake clock.
//!
//! - **Double click**: a second left click within `double_click_s` of the
//!   first gives [`Gesture::Double`]. Like Tk's `<Button-1>` +
//!   `<Double-Button-1>` in v01, the first click has already produced a
//!   [`Gesture::Left`].
//! - **Secret sequence** (README-SP): Left → Double → Right with the whole
//!   sequence inside `secret_window_s` (rules.toml
//!   `secret_sequence_window_s`, 3 s) gives [`Gesture::Secret`] *instead of*
//!   the right click. The double click's own first click does not count as
//!   the leading Left: the user clicks once, then double-clicks, then
//!   right-clicks.
//! - **Hover**: entering her silhouette gives [`Gesture::HoverEnter`]; staying
//!   `hover_long_s` gives one [`Gesture::HoverLong`] (v01: sit-swing on enter,
//!   peek once sustained). Leaving resets.

use serde::{Deserialize, Serialize};

/// Mouse buttons the overlay reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// A recognised pointer gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    Left,
    Double,
    Right,
    Middle,
    Secret,
    HoverEnter,
    HoverLong,
    HoverLeave,
}

/// Timing tunables.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GestureConfig {
    pub double_click_s: f32,
    pub secret_window_s: f32,
    pub hover_long_s: f32,
}

impl Default for GestureConfig {
    fn default() -> Self {
        Self {
            double_click_s: 0.35,
            secret_window_s: 3.0,
            hover_long_s: 1.2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Step {
    Left,
    Double,
}

/// Gesture recogniser. Feed it events with the current time `t` (s).
#[derive(Debug, Clone, Default)]
pub struct Gestures {
    pub cfg: GestureConfig,
    last_left: Option<f32>,
    /// Reduced click history for the secret sequence.
    history: Vec<(Step, f32)>,
    hover_since: Option<f32>,
    hover_long_sent: bool,
}

impl Gestures {
    pub fn new(cfg: GestureConfig) -> Self {
        Self {
            cfg,
            ..Self::default()
        }
    }

    /// A click (press without drag) of `button` at time `t`.
    pub fn click(&mut self, button: Button, t: f32) -> Gesture {
        match button {
            Button::Left => {
                if self
                    .last_left
                    .is_some_and(|t0| t - t0 <= self.cfg.double_click_s)
                {
                    self.last_left = None;
                    // The double's first click was recorded as a Left: fold it in.
                    if matches!(self.history.last(), Some((Step::Left, _))) {
                        self.history.pop();
                    }
                    self.history.push((Step::Double, t));
                    self.trim();
                    Gesture::Double
                } else {
                    self.last_left = Some(t);
                    self.history.push((Step::Left, t));
                    self.trim();
                    Gesture::Left
                }
            }
            Button::Right => {
                self.last_left = None;
                let n = self.history.len();
                let secret = n >= 2
                    && self.history[n - 2].0 == Step::Left
                    && self.history[n - 1].0 == Step::Double
                    && t - self.history[n - 2].1 <= self.cfg.secret_window_s;
                self.history.clear();
                if secret {
                    Gesture::Secret
                } else {
                    Gesture::Right
                }
            }
            Button::Middle => {
                self.last_left = None;
                self.history.clear();
                Gesture::Middle
            }
        }
    }

    fn trim(&mut self) {
        if self.history.len() > 4 {
            self.history.drain(..self.history.len() - 4);
        }
    }

    /// Pointer entered (`true`) or left (`false`) her silhouette.
    pub fn hover(&mut self, inside: bool, t: f32) -> Option<Gesture> {
        match (inside, self.hover_since.is_some()) {
            (true, false) => {
                self.hover_since = Some(t);
                self.hover_long_sent = false;
                Some(Gesture::HoverEnter)
            }
            (false, true) => {
                self.hover_since = None;
                Some(Gesture::HoverLeave)
            }
            _ => None,
        }
    }

    pub fn hovering(&self) -> bool {
        self.hover_since.is_some()
    }

    /// Call every tick; returns [`Gesture::HoverLong`] once per hover.
    pub fn tick(&mut self, t: f32) -> Option<Gesture> {
        let since = self.hover_since?;
        if !self.hover_long_sent && t - since >= self.cfg.hover_long_s {
            self.hover_long_sent = true;
            return Some(Gesture::HoverLong);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Button::*;

    #[test]
    fn double_click_window() {
        let mut g = Gestures::default();
        assert_eq!(g.click(Left, 0.0), Gesture::Left);
        assert_eq!(g.click(Left, 0.3), Gesture::Double);
        // A third quick click starts over.
        assert_eq!(g.click(Left, 0.5), Gesture::Left);
        // Too slow for a double.
        assert_eq!(g.click(Left, 1.0), Gesture::Left);
    }

    #[test]
    fn secret_sequence() {
        let mut g = Gestures::default();
        assert_eq!(g.click(Left, 10.0), Gesture::Left);
        assert_eq!(g.click(Left, 11.0), Gesture::Left);
        assert_eq!(g.click(Left, 11.2), Gesture::Double);
        assert_eq!(g.click(Right, 12.5), Gesture::Secret);
        // Consumed: the next right click is plain.
        assert_eq!(g.click(Right, 12.8), Gesture::Right);
    }

    #[test]
    fn secret_needs_order_and_window() {
        let mut g = Gestures::default();
        // Too slow (3.4 s from the first Left).
        g.click(Left, 0.0);
        g.click(Left, 2.0);
        g.click(Left, 2.2);
        assert_eq!(g.click(Right, 3.4), Gesture::Right);
        // A bare double click (no leading Left) is not enough.
        g.click(Left, 20.0);
        g.click(Left, 20.2);
        assert_eq!(g.click(Right, 21.0), Gesture::Right);
        // Middle in between breaks it.
        g.click(Left, 30.0);
        g.click(Left, 31.0);
        g.click(Left, 31.2);
        g.click(Middle, 31.5);
        assert_eq!(g.click(Right, 32.0), Gesture::Right);
    }

    #[test]
    fn hover_long_once_per_hover() {
        let mut g = Gestures::default();
        assert_eq!(g.tick(0.0), None);
        assert_eq!(g.hover(true, 1.0), Some(Gesture::HoverEnter));
        assert_eq!(g.hover(true, 1.1), None, "repeated enter ignored");
        assert_eq!(g.tick(2.1), None);
        assert_eq!(g.tick(2.3), Some(Gesture::HoverLong));
        assert_eq!(g.tick(9.0), None);
        assert_eq!(g.hover(false, 9.5), Some(Gesture::HoverLeave));
        assert_eq!(g.hover(true, 10.0), Some(Gesture::HoverEnter));
        assert_eq!(g.hover(false, 10.5), Some(Gesture::HoverLeave));
        assert_eq!(g.tick(20.0), None, "left before hover_long");
    }
}
