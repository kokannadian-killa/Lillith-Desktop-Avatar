//! Bot registry (main bots + anon / Tor-aware swarm).
//!
//! Deploy actions only **propose** exact commands and must go through the
//! blood seal. Reuses the layout concepts from `tools/lillith-py` bots and
//! the Tor-aware learn worker policy (SOCKS5h) — additive, never removes
//! the Python bot store.

use crate::lockdown::LockdownProposal;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Where a bot lives in the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotLane {
    /// Main local bots (`~/.biotek/bots/registry`).
    Main,
    /// Anon swarm — Tor-aware (`~/.biotek/bots/swarm`).
    AnonSwarm,
}

impl BotLane {
    /// Short UI label.
    pub fn label(self) -> &'static str {
        match self {
            BotLane::Main => "MAIN",
            BotLane::AnonSwarm => "ANON SWARM",
        }
    }

    /// One-line tooltip.
    pub fn tooltip(self) -> &'static str {
        match self {
            BotLane::Main => "Local bot profiles (mirrors tools/lillith-py BotStore concepts).",
            BotLane::AnonSwarm => {
                "Tor-aware swarm lane. Deploy proposes socks5h routes; blood seal required."
            }
        }
    }

    /// WHISPER FROM THE CORE body.
    pub fn whisper(self) -> &'static str {
        match self {
            BotLane::Main => {
                "WHISPER FROM THE CORE — Main bots stay on loopback minds. Deploy shows the exact \
                 python -m lillith_py command before anything starts."
            }
            BotLane::AnonSwarm => {
                "WHISPER FROM THE CORE — Anon swarm speaks only through Tor (socks5h). \
                 No local DNS. Seal the deploy and the lattice logs your mark."
            }
        }
    }

    fn dir_name(self) -> &'static str {
        match self {
            BotLane::Main => "registry",
            BotLane::AnonSwarm => "swarm",
        }
    }
}

/// One registered bot (or swarm member).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotEntry {
    /// Stable bot id.
    pub id: String,
    /// Display title.
    pub title: String,
    /// Main vs anon swarm lane.
    pub lane: BotLane,
    /// Role blurb for the panel.
    pub role: String,
    /// When true, deploy proposal includes Tor SOCKS5h env.
    pub tor_aware: bool,
    /// Exact deploy command template (shell).
    pub deploy_command: String,
    /// Tooltip on the row.
    pub tooltip: String,
    /// WHISPER FROM THE CORE body.
    pub whisper: String,
}

impl BotEntry {
    /// Blood-seal proposal for deploying this bot (never executes here).
    pub fn deploy_proposal(&self, biotek: &Path) -> LockdownProposal {
        let mut cmds = Vec::new();
        if self.tor_aware {
            cmds.push(
                "echo '[QuantumSec] ANON: ALL_PROXY=socks5h://127.0.0.1:9050 (Tor must already be up)'"
                    .into(),
            );
            cmds.push(
                "echo '[QuantumSec] ANON: no local DNS — socks5h only (see lillith-learn worker policy)'"
                    .into(),
            );
        }
        cmds.push(format!(
            "echo '[QuantumSec] DEPLOY PROPOSAL ({lane}): {cmd}'",
            lane = self.lane.label(),
            cmd = self.deploy_command
        ));
        cmds.push(format!(
            "echo '[QuantumSec] registry path: {}'",
            biotek
                .join("bots")
                .join(self.lane.dir_name())
                .join(&self.id)
                .display()
        ));
        cmds.push("true".into());
        let mut p = LockdownProposal::new(
            format!(
                "Deploy {} bot `{}` — review exact command",
                self.lane.label(),
                self.id
            ),
            cmds,
        );
        p.id = format!("deploy-{}-{}", self.id, p.proposed_unix);
        p.local_simulation = true; // proposals stay simulation until a future Phase elevates
        p
    }
}

/// Default main + anon entries (additive starters).
pub fn default_registry() -> Vec<BotEntry> {
    vec![
        BotEntry {
            id: "lillith-main".into(),
            title: "Lillith main mind".into(),
            lane: BotLane::Main,
            role: "desktop companion".into(),
            tor_aware: false,
            deploy_command: "python3 -m lillith_py bots ensure lillith-main".into(),
            tooltip: "Primary local bot; uses loopback llama/Ollama only.".into(),
            whisper: "WHISPER FROM THE CORE — She already walks beside you. Ensure only mirrors the Python BotStore."
                .into(),
        },
        BotEntry {
            id: "researcher".into(),
            title: "Local researcher".into(),
            lane: BotLane::Main,
            role: "notes + memory".into(),
            tor_aware: false,
            deploy_command: "python3 -m lillith_py bots ensure researcher".into(),
            tooltip: "Allowlisted tools only (attachments + memory).".into(),
            whisper: "WHISPER FROM THE CORE — Researcher never leaves the allowlist. Seal to acknowledge the ensure."
                .into(),
        },
        BotEntry {
            id: "anon-scout".into(),
            title: "Anon scout".into(),
            lane: BotLane::AnonSwarm,
            role: "Tor-aware fetch (policy-bound)".into(),
            tor_aware: true,
            deploy_command:
                "ALL_PROXY=socks5h://127.0.0.1:9050 python3 -m lillith_py bots ensure anon-scout"
                    .into(),
            tooltip: "Swarm lane; Tor socks5h. Mirrors lillith-learn anonymity hard rule.".into(),
            whisper: "WHISPER FROM THE CORE — Scout rides Tor circuits. No cookies, no local DNS, fresh credentials per worker."
                .into(),
        },
    ]
}

/// Load `~/.biotek/bots/registry.json` or defaults.
pub fn load_registry(biotek: &Path) -> Vec<BotEntry> {
    let path = biotek.join("bots").join("registry.json");
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|_| default_registry()),
        Err(_) => default_registry(),
    }
}

/// Persist registry.
pub fn save_registry(biotek: &Path, bots: &[BotEntry]) -> std::io::Result<()> {
    let dir = biotek.join("bots");
    std::fs::create_dir_all(dir.join("registry"))?;
    std::fs::create_dir_all(dir.join("swarm"))?;
    let path = dir.join("registry.json");
    let s = serde_json::to_string_pretty(bots)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, s)
}

/// Lane directory for an entry.
pub fn bot_lane_dir(biotek: &Path, lane: BotLane) -> PathBuf {
    biotek.join("bots").join(lane.dir_name())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anon_deploy_mentions_tor() {
        let b = default_registry()
            .into_iter()
            .find(|b| b.lane == BotLane::AnonSwarm)
            .unwrap();
        assert!(b.tor_aware);
        let dir = std::env::temp_dir().join("qsec-reg");
        let p = b.deploy_proposal(&dir);
        assert!(p.detail_block().contains("socks5h"));
        assert!(p.local_simulation);
    }

    #[test]
    fn whispers_banner() {
        for b in default_registry() {
            assert!(b.whisper.starts_with("WHISPER FROM THE CORE"));
            assert!(b.lane.whisper().starts_with("WHISPER FROM THE CORE"));
        }
    }
}
