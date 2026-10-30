//! Red / Blue / Purple team exercises (local simulation for Red).
//!
//! - **Red**: local simulation only — labeled, proposes `echo` / `true` dry-runs.
//! - **Blue**: hardening proposals — exact commands, blood-seal required.
//! - **Purple**: feedback notes linking Red findings to Blue fixes.
//!
//! Nothing here executes. Callers pass proposals through the blood seal.

use crate::lockdown::LockdownProposal;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Which team desk this exercise belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamColor {
    /// Offensive local simulation (never touches the host for real).
    Red,
    /// Defensive hardening proposals (consent-gated).
    Blue,
    /// Feedback that pairs Red findings with Blue responses.
    Purple,
}

impl TeamColor {
    /// All desks, in panel order.
    pub const ALL: [TeamColor; 3] = [TeamColor::Red, TeamColor::Blue, TeamColor::Purple];

    /// Short UI label.
    pub fn label(self) -> &'static str {
        match self {
            TeamColor::Red => "RED TEAM",
            TeamColor::Blue => "BLUE TEAM",
            TeamColor::Purple => "PURPLE TEAM",
        }
    }

    /// One-line tooltip.
    pub fn tooltip(self) -> &'static str {
        match self {
            TeamColor::Red => "Local simulation only. No host changes. Every step is an echo dry-run.",
            TeamColor::Blue => "Hardening proposals. Exact commands shown; blood seal required to run.",
            TeamColor::Purple => "Feedback loop: map Red findings to Blue fixes. Notes only until sealed.",
        }
    }

    /// WHISPER FROM THE CORE copy for the info popup.
    pub fn whisper(self) -> &'static str {
        match self {
            TeamColor::Red => {
                "WHISPER FROM THE CORE — Red is a mirror, not a blade. Simulations stay on this machine. \
                 The blood seal still opens so you see every echo before it runs."
            }
            TeamColor::Blue => {
                "WHISPER FROM THE CORE — Blue hardens. Commands are proposed, never forced. \
                 Sign in blood and the lattice remembers your mark under ~/.biotek/consent."
            }
            TeamColor::Purple => {
                "WHISPER FROM THE CORE — Purple listens to both sides. Feedback is text until you \
                 seal a linked Blue proposal. Nothing silent, nothing automatic."
            }
        }
    }

    fn dir_name(self) -> &'static str {
        match self {
            TeamColor::Red => "red",
            TeamColor::Blue => "blue",
            TeamColor::Purple => "purple",
        }
    }
}

/// One exercise / finding / hardening item on a team desk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamItem {
    /// Stable id (`red-sim-listeners`, `blue-harden-ssh`, …).
    pub id: String,
    /// Desk this item belongs to.
    pub team: TeamColor,
    /// Short title for the panel row.
    pub title: String,
    /// Tooltip on the row.
    pub tooltip: String,
    /// WHISPER FROM THE CORE body for the info popup.
    pub whisper: String,
    /// Exact command block shown in the blood seal (Red = local sim).
    pub commands: Vec<String>,
    /// True when Red (or any local-only) simulation.
    pub local_simulation: bool,
    /// Blue: show exact commands as "PROPOSAL, not run" (no spawn on seal).
    #[serde(default)]
    pub proposal_only: bool,
    /// Optional Purple link to a Red finding id.
    #[serde(default)]
    pub links_to: Option<String>,
}

impl TeamItem {
    /// Build a [`LockdownProposal`] for the blood seal (never executes here).
    pub fn as_proposal(&self) -> LockdownProposal {
        let mut cmds = self.commands.clone();
        if self.local_simulation {
            let banner = format!(
                "echo '[QuantumSec] {} LOCAL SIMULATION: {}'",
                self.team.label(),
                self.title
            );
            if cmds.first().map(|c| c.as_str()) != Some(banner.as_str()) {
                cmds.insert(0, banner);
            }
            if !cmds.iter().any(|c| c.trim() == "true") {
                cmds.push("true".into());
            }
        }
        let mut p = LockdownProposal::new(
            format!("{} — {}", self.team.label(), self.title),
            cmds,
        );
        p.id = format!("{}-{}", self.id, p.proposed_unix);
        p.local_simulation = self.local_simulation;
        p.proposal_only = self.proposal_only && !self.local_simulation;
        p
    }
}

/// Default catalog shipped with Phase 2 (additive; user items may be saved beside).
pub fn default_catalog() -> Vec<TeamItem> {
    vec![
        TeamItem {
            id: "red-sim-listeners".into(),
            team: TeamColor::Red,
            title: "Simulate listener sweep".into(),
            tooltip: "Dry-run: list what a listener sweep would report (echo only).".into(),
            whisper: "WHISPER FROM THE CORE — We name the ports we would watch. No sockets open. No packets leave.".into(),
            commands: vec![
                "echo '[QuantumSec] LOCAL SIMULATION: would scan listening sockets'".into(),
                "echo '[QuantumSec] LOCAL SIMULATION: report → ~/.biotek/teams/red/'".into(),
            ],
            local_simulation: true,
            proposal_only: false,
            links_to: None,
        },
        TeamItem {
            id: "red-sim-process".into(),
            team: TeamColor::Red,
            title: "Simulate process anomaly".into(),
            tooltip: "Dry-run: pretend a noisy process was spotted.".into(),
            whisper: "WHISPER FROM THE CORE — Red invents the shadow so Blue can practice the light.".into(),
            commands: vec![
                "echo '[QuantumSec] LOCAL SIMULATION: anomaly pid=0 name=phantom'".into(),
            ],
            local_simulation: true,
            proposal_only: false,
            links_to: None,
        },
        TeamItem {
            id: "blue-harden-ssh".into(),
            team: TeamColor::Blue,
            title: "Propose SSH hardening check".into(),
            tooltip: "Exact sshd grep shown as PROPOSAL, not run; blood seal acknowledges.".into(),
            whisper: "WHISPER FROM THE CORE — Blue does not rewrite sshd. It shows the read-only check first.".into(),
            commands: vec![
                "grep -E '^(PermitRootLogin|PasswordAuthentication)' /etc/ssh/sshd_config".into(),
            ],
            local_simulation: false,
            proposal_only: true,
            links_to: None,
        },
        TeamItem {
            id: "blue-harden-ufw".into(),
            team: TeamColor::Blue,
            title: "Propose firewall status check".into(),
            tooltip: "Exact ufw/firewall-cmd status as PROPOSAL, not run; blood seal acknowledges.".into(),
            whisper: "WHISPER FROM THE CORE — Status first. Enable never without a second seal.".into(),
            commands: vec![
                "ufw status verbose || firewall-cmd --state".into(),
            ],
            local_simulation: false,
            proposal_only: true,
            links_to: None,
        },
        TeamItem {
            id: "purple-link-listeners".into(),
            team: TeamColor::Purple,
            title: "Feedback: listeners → firewall".into(),
            tooltip: "Pairs the Red listener sweep with the Blue firewall check.".into(),
            whisper: "WHISPER FROM THE CORE — Purple writes the bridge. Seal the Blue check to close the loop.".into(),
            commands: vec![
                "echo '[QuantumSec] PURPLE: link red-sim-listeners → blue-harden-ufw'".into(),
                "true".into(),
            ],
            local_simulation: true,
            proposal_only: false,
            links_to: Some("red-sim-listeners".into()),
        },
    ]
}

/// Load catalog from `~/.biotek/teams/catalog.json`, or defaults.
pub fn load_catalog(biotek: &Path) -> Vec<TeamItem> {
    let path = biotek.join("teams").join("catalog.json");
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|_| default_catalog()),
        Err(_) => default_catalog(),
    }
}

/// Save catalog (additive user edits).
pub fn save_catalog(biotek: &Path, items: &[TeamItem]) -> std::io::Result<()> {
    let dir = biotek.join("teams");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("catalog.json");
    let s = serde_json::to_string_pretty(items)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, s)
}

/// Per-team notes directory under `~/.biotek/teams/<color>/`.
pub fn team_dir(biotek: &Path, color: TeamColor) -> PathBuf {
    biotek.join("teams").join(color.dir_name())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn red_proposals_are_local_simulation() {
        for item in default_catalog()
            .into_iter()
            .filter(|i| i.team == TeamColor::Red)
        {
            assert!(item.local_simulation, "{}", item.id);
            let p = item.as_proposal();
            assert!(p.local_simulation);
            assert!(p.detail_block().contains("LOCAL SIMULATION"));
        }
    }

    #[test]
    fn blue_proposals_are_proposal_not_run() {
        for item in default_catalog()
            .into_iter()
            .filter(|i| i.team == TeamColor::Blue)
        {
            assert!(!item.local_simulation, "{}", item.id);
            assert!(item.proposal_only, "{}", item.id);
            let p = item.as_proposal();
            assert!(!p.local_simulation);
            assert!(p.proposal_only);
            assert_eq!(p.seal_banner(), "PROPOSAL, not run");
            assert!(!p.commands.iter().any(|c| c.contains("echo ")), "{}", item.id);
            assert!(p.detail_block().contains("PROPOSAL, not run"));
        }
    }

    #[test]
    fn whispers_start_with_banner() {
        for c in TeamColor::ALL {
            assert!(c.whisper().starts_with("WHISPER FROM THE CORE"));
        }
    }

    #[test]
    fn catalog_roundtrip() {
        let dir = tempfile_dir();
        save_catalog(&dir, &default_catalog()).unwrap();
        let loaded = load_catalog(&dir);
        assert_eq!(loaded.len(), default_catalog().len());
    }

    fn tempfile_dir() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "qsec-teams-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
