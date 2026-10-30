//! Lockdown proposals: exact command strings, never executed without a
//! [`ConsentToken`] issued after a blood-seal approval.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

/// Error from a lockdown execute attempt.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum LockdownError {
    /// No consent token, or token does not match this proposal.
    #[error("consent required: blood-seal token missing or mismatched")]
    ConsentRequired,
    /// Token was already consumed.
    #[error("consent token already consumed")]
    TokenConsumed,
    /// A proposed command refused to spawn (still never silently skipped).
    #[error("command failed to spawn: {0}")]
    Spawn(String),
}

/// A proposed lockdown: exact commands the shield wants to run.
///
/// Red team = **local simulation only**. The default proposal uses
/// clearly labeled dry-run / local-only commands (`echo`, `true`) so a
/// mistaken click cannot alter the host. Real hardening commands are
/// added only when the caller builds a custom proposal and the user
/// seals them in blood.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockdownProposal {
    /// Stable id (also the consent-log key).
    pub id: String,
    /// Human summary shown on the blood-seal card.
    pub summary: String,
    /// Exact command lines (each is one `sh -c` invocation if executed).
    pub commands: Vec<String>,
    /// When true, every command is labeled local-simulation / dry-run.
    pub local_simulation: bool,
    /// Unix epoch when proposed.
    pub proposed_unix: u64,
}

impl LockdownProposal {
    /// Default local-simulation proposal (never touches the host).
    pub fn local_simulation_default() -> Self {
        let id = format!("lockdown-sim-{}", now_unix());
        Self {
            id: id.clone(),
            summary: "LOCAL SIMULATION ONLY — QuantumSec dry-run lockdown (no host changes)"
                .into(),
            commands: vec![
                "echo '[QuantumSec] LOCAL SIMULATION: lockdown begin'".into(),
                "echo '[QuantumSec] LOCAL SIMULATION: would pause non-essential user units'"
                    .into(),
                "echo '[QuantumSec] LOCAL SIMULATION: lockdown end'".into(),
                "true".into(),
            ],
            local_simulation: true,
            proposed_unix: now_unix(),
        }
    }

    /// Exact text shown on the blood-seal terminal slab.
    pub fn detail_block(&self) -> String {
        let flag = if self.local_simulation {
            "# RED TEAM = LOCAL SIMULATION ONLY\n"
        } else {
            "# REAL COMMANDS — review carefully\n"
        };
        let body = self
            .commands
            .iter()
            .map(|c| format!("$ {c}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("{flag}{body}")
    }
}

/// Token proving the user sealed a specific proposal in blood (mouse click).
///
/// Opaque: created only by [`crate::consent::ConsentLog::approve`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentToken {
    /// Proposal id this token unlocks.
    pub proposal_id: String,
    /// Random nonce (hex).
    pub nonce: String,
    /// Unix epoch of approval.
    pub approved_unix: u64,
    /// Consumed after one successful execute.
    pub(crate) consumed: bool,
}

impl ConsentToken {
    /// Build a fresh token for `proposal_id` (crate-internal; prefer the consent log).
    pub(crate) fn issue(proposal_id: impl Into<String>) -> Self {
        Self {
            proposal_id: proposal_id.into(),
            nonce: format!("{:016x}", nonce64()),
            approved_unix: now_unix(),
            consumed: false,
        }
    }

    /// Whether this token unlocks `proposal`.
    pub fn matches(&self, proposal: &LockdownProposal) -> bool {
        !self.consumed && self.proposal_id == proposal.id
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn nonce64() -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    now_unix().hash(&mut h);
    std::thread::current().id().hash(&mut h);
    std::process::id().hash(&mut h);
    h.finish()
}

/// Run the proposal's commands **only** with a matching, unconsumed token.
///
/// Each command is `sh -c`. On success the token is marked consumed.
/// Without a matching token this returns [`LockdownError::ConsentRequired`]
/// and runs nothing.
pub fn execute_with_token(
    proposal: &LockdownProposal,
    token: &mut ConsentToken,
) -> Result<Vec<std::process::Output>, LockdownError> {
    if token.proposal_id != proposal.id {
        return Err(LockdownError::ConsentRequired);
    }
    if token.consumed {
        return Err(LockdownError::TokenConsumed);
    }
    let mut outs = Vec::with_capacity(proposal.commands.len());
    for cmd in &proposal.commands {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()
            .map_err(|e| LockdownError::Spawn(e.to_string()))?;
        outs.push(out);
    }
    token.consumed = true;
    Ok(outs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_without_token() {
        let p = LockdownProposal::local_simulation_default();
        let mut bad = ConsentToken::issue("other-id");
        assert_eq!(
            execute_with_token(&p, &mut bad).unwrap_err(),
            LockdownError::ConsentRequired
        );
    }

    #[test]
    fn runs_local_sim_with_matching_token() {
        let p = LockdownProposal::local_simulation_default();
        let mut tok = ConsentToken::issue(&p.id);
        let outs = execute_with_token(&p, &mut tok).unwrap();
        assert_eq!(outs.len(), p.commands.len());
        assert!(tok.consumed);
        assert_eq!(
            execute_with_token(&p, &mut tok).unwrap_err(),
            LockdownError::TokenConsumed
        );
    }

    #[test]
    fn detail_block_labels_simulation() {
        let p = LockdownProposal::local_simulation_default();
        let d = p.detail_block();
        assert!(d.contains("LOCAL SIMULATION"));
        assert!(d.contains("$ echo"));
    }
}
