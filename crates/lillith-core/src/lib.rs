//! # lillith-core
//!
//! Pure, I/O-free behaviour logic for the Lillith desktop companion
//! (PLAN.md §1.2 and §3). Everything here is deterministic and unit-testable
//! with a fake clock:
//!
//! - [`event`]: the [`Event`] type every sensor and input source emits onto the bus.
//! - [`command`]: the [`Command`]s the core sends back to the overlay/voice/mind.
//! - [`mood`]: moods, mischief meter and warmth.
//! - [`spring`]: spring-damper channels used for the "alive" physics.
//! - [`rules`]: the data-driven event → reaction table (`assets/rules.toml`).
//! - [`config`]: user config (`~/.config/lillith/config.toml`) with README-SP defaults.
//! - [`state`]: the locomotion/reaction state machine (TODO, M3).

pub mod command;
pub mod config;
pub mod event;
pub mod mood;
pub mod rules;
pub mod spring;
pub mod state;

pub use command::Command;
pub use event::Event;
pub use mood::Mood;
