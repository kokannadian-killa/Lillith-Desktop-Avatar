//! # lillith-quantumsec
//!
//! QuantumSec shield for Lillith: light, local "protons" (named monitor
//! units), a status model with event callbacks, and a lockdown mode that
//! only **proposes** shell commands and never runs them without a
//! consent-approved [`ConsentToken`] from the blood-seal dialog.
//!
//! Phase 2 adds Red/Blue/Purple teams, personalization Markdown reports,
//! a bot registry (main + Tor-aware anon swarm), and memory-hook helpers.
//!
//! Data home is always under `~/.biotek` via [`lillith_setup::biotek`].
//! Red-team work is local simulation only and clearly labeled.

#![deny(missing_docs)]

pub mod consent;
pub mod lockdown;
pub mod memory_hook;
pub mod proton;
pub mod registry;
pub mod report;
pub mod shield;
pub mod status;
pub mod teams;

pub use consent::{ConsentDecision, ConsentLog, ConsentRecord};
pub use lockdown::{ConsentToken, LockdownError, LockdownProposal};
pub use proton::{Proton, ProtonId, ProtonKind, ProtonReading};
pub use registry::{BotEntry, BotLane};
pub use report::DeviceProfile;
pub use shield::{QuantumSecShield, ShieldConfig, ShieldEvent, ShieldHandle};
pub use status::{ProtonStatus, ShieldStatus, StatusLevel};
pub use teams::{TeamColor, TeamItem};
