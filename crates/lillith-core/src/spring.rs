//! Spring-damper channels (PLAN.md §3.1, layer 5).
//!
//! One spring per channel (root x/y, angle, scale, chest, pigtails),
//! integrated with semi-implicit Euler at 120 Hz and velocity clamping.

use serde::{Deserialize, Serialize};

/// Stiffness/damping pair, as in README-SP `[springs]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpringParams {
    pub stiffness: f32,
    pub damping: f32,
}

/// A single 1-D spring-damper.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    pub params: SpringParams,
    pub position: f32,
    pub velocity: f32,
    pub target: f32,
    pub max_velocity: f32,
}

impl Spring {
    /// New spring at rest on `target`.
    pub fn new(params: SpringParams, target: f32) -> Self {
        Self {
            params,
            position: target,
            velocity: 0.0,
            target,
            max_velocity: 50.0,
        }
    }

    /// Adds an instantaneous velocity kick (reactions use impulses, not jitter).
    pub fn impulse(&mut self, dv: f32) {
        self.velocity = (self.velocity + dv).clamp(-self.max_velocity, self.max_velocity);
    }

    /// Semi-implicit Euler step.
    pub fn step(&mut self, dt: f32) {
        let accel = -self.params.stiffness * (self.position - self.target)
            - self.params.damping * self.velocity;
        self.velocity = (self.velocity + accel * dt).clamp(-self.max_velocity, self.max_velocity);
        self.position += self.velocity * dt;
    }

    /// Steps in fixed `1/hz` sub-steps covering `dt`.
    pub fn advance(&mut self, dt: f32, hz: f32) {
        let h = 1.0 / hz;
        let mut remaining = dt;
        while remaining > 0.0 {
            let s = remaining.min(h);
            self.step(s);
            remaining -= s;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settles_on_target_after_impulse() {
        let mut s = Spring::new(
            SpringParams {
                stiffness: 65.0,
                damping: 9.5,
            },
            0.0,
        );
        s.impulse(5.0);
        s.advance(5.0, 120.0);
        assert!(s.position.abs() < 1e-3, "position {}", s.position);
    }
}
