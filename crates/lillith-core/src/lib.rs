//! # lillith-core
//!
//! Pure, I/O-free behaviour logic for the Lillith desktop companion
//! (PLAN.md §1.2 and §3). Everything here is deterministic and unit-testable
//! with a fake clock:
//!
//! - [`event`]: the [`Event`] type every sensor and input source emits onto the bus.
//! - [`command`]: the [`Command`]s the core sends back to the overlay/voice/mind.
//! - [`mood`]: moods (decaying strength + baseline) and the mischief meter.
//! - [`interaction`]: pointer gestures (double click, secret sequence, hover).
//! - [`menu`]: the right-click menu as data ([`menu::build_menu`] / [`menu::on_menu_action`]).
//! - [`spring`]: spring-damper channels ([`spring::Physics`]) and breathing for the "alive" physics.
//! - [`physics_config`]: serde model of the bundled `assets/physics.toml` (tuning, recipes, moods).
//! - [`physics_driver`]: [`physics_driver::PhysicsDriver`], which turns clips/events/motion/mood
//!   into spring impulses and targets, plus the procedural tail/horn overlay.
//! - [`idle`]: idle-life scheduler (blink, idle transitions, idle reactions).
//! - [`tease`]: the "come closer" tease (scale envelope + scheduler).
//! - [`rng`]: tiny seedable PRNG so idle timing is deterministic in tests.
//! - [`rules`]: the data-driven event → reaction table (`assets/rules.toml`).
//! - [`desktop`]: z-ordered window model from the KWin bridge and the pure
//!   [`desktop::landing_target`] gravity uses to land her on window tops.
//! - [`config`]: user config (`~/.config/lillith/config.toml`) with README-SP defaults.
//! - [`state`]: the M3 behaviour state machine ([`state::Brain`]): input
//!   mapping, moods, mischief, sleep/wake, Wander/Stay, pose cycle, rules.

pub mod access;
pub mod bag;
pub mod bounce;
pub mod build;
pub mod command;
pub mod config;
pub mod desktop;
pub mod event;
pub mod icons;
pub mod idle;
pub mod interaction;
pub mod menu;
pub mod mood;
pub mod notes;
pub mod physics_config;
pub mod physics_driver;
pub mod rng;
pub mod rules;
pub mod soul_contract;
pub mod spring;
pub mod state;
pub mod tease;

pub use command::Command;
pub use event::Event;
pub use mood::Mood;
