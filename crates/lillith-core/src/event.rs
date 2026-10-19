//! Events flowing into the behaviour core.
//!
//! Event names are dotted strings (`cpu.high`, `window.opened.browser`,
//! `input.click.left`) so new sensors and the debug injector
//! (`lillith event battery.low`) need no code change, only a rule in
//! `assets/rules.toml`.

use serde::{Deserialize, Serialize};

/// One thing that happened: a system change, a user input, or an internal signal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Dotted event name, matched against `[[rule]].event`.
    pub name: String,
    /// Optional human-readable detail (notification summary, window title, ...).
    /// Substituted for `{summary}` in rule lines.
    #[serde(default)]
    pub detail: Option<String>,
}

impl Event {
    /// Creates an event with no detail.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            detail: None,
        }
    }

    /// Creates an event carrying a detail string.
    pub fn with_detail(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            detail: Some(detail.into()),
        }
    }
}
