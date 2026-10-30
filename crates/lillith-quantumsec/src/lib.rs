//! # lillith-quantumsec
//!
//! QuantumSec shield for Lillith: light, local "protons" (named monitor
//! units), a status model with event callbacks, and a lockdown mode that
//! only **proposes** shell commands and never runs them without a
//! consent-approved [`ConsentToken`] from the blood-seal dialog.
//!
//! Data home is always under `~/.biotek` via [`lillith_setup::biotek`].
//! Red-team work is local simulation only and clearly labeled.

#![deny(missing_docs)]

pub mod consent;
pub mod lockdown;
pub mod proton;
pub mod shield;
pub mod status;

pub use consent::{ConsentDecision, ConsentLog, ConsentRecord};
pub use lockdown::{ConsentToken, LockdownError, LockdownProposal};
pub use proton::{Proton, ProtonId, ProtonKind, ProtonReading};
pub use shield::{ShieldConfig, ShieldEvent, ShieldHandle, QuantumSecShield};
pub use status::{ProtonStatus, ShieldStatus, StatusLevel};
