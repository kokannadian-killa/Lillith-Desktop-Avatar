//! # lillith-memory
//!
//! Lillith's local learning brain: a small SQLite memory graph that decays,
//! gets reinforced on reuse, and feeds a compact context block to the local
//! 3B model. Sync, single file, no network code, no telemetry.
//!
//! - [`Brain`]: open (respecting `[memory] enabled`), observe chat turns and
//!   desktop events, retrieve, predict, remember errors, forget.
//! - [`safeword`]: messages carrying the safeword are never stored and stop
//!   the learning loop ([`Observed::Stop`]).
//! - [`decay`], [`habits`], [`errors`], [`extract`], [`forget`]: the pure
//!   pieces, each unit-tested on its own.
//! - [`bridge`]: the overlay's non-blocking event bridge and proactive
//!   suggestions; [`recovery`]: safe built-in fixes ordered by error memory.
//!
//! See `docs/MEMORY.md` for the design.

pub mod bridge;
pub mod decay;
pub mod errors;
pub mod extract;
pub mod forget;
pub mod habits;
pub mod notes;
pub mod recovery;
pub mod safeword;
pub mod schema;

mod brain;

pub use brain::{Brain, Ctx, ForgetReport, Memory, Observed, Stats, FORGET_EXAMPLES};
pub use bridge::{EventFilter, MemoryBridge, Proactive};
pub use errors::{ErrorRecord, FixOutcome, KnownFix};
pub use extract::{Extractor, Fact, Polarity, RuleExtractor};
pub use forget::{
    contains_phrase, content_words, matches_topic, mentions_any, parse_forget, ForgetCmd,
    CONFIRM_WINDOW_SECS, LAST_TURN_SECS,
};
pub use habits::Prediction;
pub use lillith_core::config::MemoryConfig;
pub use notes::{NoteRow, SourceState};
pub use recovery::SafeFix;
pub use safeword::{is_safeword, mentions_safeword};

/// Memory errors. The chat path never surfaces these; it logs and carries on.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("memory db schema v{found} is newer than this build (v{supported})")]
    FutureSchema { found: u32, supported: u32 },
    #[error("no data directory (HOME / XDG_DATA_HOME unset)")]
    NoDataDir,
}

/// Node kinds in the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Entity,
    Event,
    Preference,
    Habit,
    Error,
    /// A learned note (`knowledge/<topic>/<slug>.md`, key = note id).
    Note,
    /// A proven skill (P4; reserved now).
    Skill,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Entity => "entity",
            Kind::Event => "event",
            Kind::Preference => "preference",
            Kind::Habit => "habit",
            Kind::Error => "error",
            Kind::Note => "note",
            Kind::Skill => "skill",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "entity" => Kind::Entity,
            "event" => Kind::Event,
            "preference" => Kind::Preference,
            "habit" => Kind::Habit,
            "error" => Kind::Error,
            "note" => Kind::Note,
            "skill" => Kind::Skill,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod brain_tests;
