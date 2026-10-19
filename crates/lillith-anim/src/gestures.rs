//! Procedural gestures, ported from `aether_animator.py`.
//!
//! As of M2 every gesture has an authored clip in the generated set, played by
//! [`crate::animator::Animator`] with an impulse from
//! [`crate::animator::kick_for`]. Procedural tweens (spring-target sequences)
//! remain a TODO for clips that lack art.

use std::str::FromStr;

/// The aether_animator gesture library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Gesture {
    Float,
    Bounce,
    Wave,
    Stab,
    PointRight,
    Think,
    Tada,
    Dance,
    Salute,
    Nod,
    Shrug,
    Facepalm,
}

impl FromStr for Gesture {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "float" => Self::Float,
            "bounce" => Self::Bounce,
            "wave" => Self::Wave,
            "stab" => Self::Stab,
            "point_right" => Self::PointRight,
            "think" => Self::Think,
            "tada" => Self::Tada,
            "dance" => Self::Dance,
            "salute" => Self::Salute,
            "nod" => Self::Nod,
            "shrug" => Self::Shrug,
            "facepalm" => Self::Facepalm,
            other => return Err(format!("unknown gesture {other}")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lillith_core::rules::RuleSet;

    #[test]
    fn every_rule_gesture_is_known() {
        for r in RuleSet::bundled().rules {
            if let Some(g) = &r.gesture {
                g.parse::<Gesture>()
                    .unwrap_or_else(|e| panic!("{}: {e}", r.event));
            }
        }
    }
}
