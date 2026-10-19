//! Locomotion + reaction state machine (PLAN.md §3.1). TODO(M3).
//!
//! Layers: exclusive base/locomotion state, a priority queue of one-shot
//! reactions (replacing v01's `is_reacting` flag), idle life timers, and
//! mood/drives. Driven by a fake clock in tests.

use glam::Vec2;

/// Base locomotion state.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Locomotion {
    #[default]
    IdleSitting,
    IdleStanding,
    Walking {
        dir: Vec2,
    },
    Running {
        dir: Vec2,
    },
    /// Perched on the title bar of the window with this KWin id.
    Perched {
        window: String,
    },
    Dragged,
    Sleepy,
    Asleep,
    Hidden,
}

/// A queued one-shot reaction resolved from a rule.
#[derive(Debug, Clone, PartialEq)]
pub struct Reaction {
    pub pose: String,
    pub priority: u8,
    pub remaining_s: f32,
    pub interruptible: bool,
}

/// Behaviour state. TODO(M3): idle timers, reaction queue, mood transitions.
#[derive(Debug, Default)]
pub struct Behavior {
    pub locomotion: Locomotion,
    pub current: Option<Reaction>,
}

impl Behavior {
    /// Offers a reaction; it replaces the current one only if it has equal or
    /// higher priority or the current one is interruptible.
    pub fn offer(&mut self, r: Reaction) -> bool {
        let accept = match &self.current {
            None => true,
            Some(cur) => cur.interruptible || r.priority >= cur.priority,
        };
        if accept {
            self.current = Some(r);
        }
        accept
    }

    /// Advances timers by `dt` seconds.
    pub fn tick(&mut self, dt: f32) {
        if let Some(cur) = &mut self.current {
            cur.remaining_s -= dt;
            if cur.remaining_s <= 0.0 {
                self.current = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(pose: &str, priority: u8) -> Reaction {
        Reaction {
            pose: pose.into(),
            priority,
            remaining_s: 1.0,
            interruptible: false,
        }
    }

    #[test]
    fn higher_priority_interrupts_and_expires() {
        let mut b = Behavior::default();
        assert!(b.offer(r("busy", 60)));
        assert!(!b.offer(r("clipboard", 15)));
        assert!(b.offer(r("battery_low", 80)));
        b.tick(1.5);
        assert!(b.current.is_none());
    }
}
