//! Shield status model (serialisable for `~/.biotek/quantumsec/status.json`).

use crate::proton::{ProtonId, ProtonKind, ProtonReading};
use serde::{Deserialize, Serialize};

/// Overall shield level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusLevel {
    /// All protons quiet.
    Nominal,
    /// At least one Notice.
    Elevated,
    /// At least one Alert, or lockdown proposed.
    Critical,
    /// Shield is paused / not polling.
    Paused,
}

impl StatusLevel {
    /// UI label.
    pub fn label(self) -> &'static str {
        match self {
            StatusLevel::Nominal => "NOMINAL",
            StatusLevel::Elevated => "ELEVATED",
            StatusLevel::Critical => "CRITICAL",
            StatusLevel::Paused => "PAUSED",
        }
    }
}

/// Per-proton status line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProtonStatus {
    /// Proton id.
    pub id: ProtonId,
    /// Last reading, if any.
    pub last: Option<ProtonReading>,
}

/// Whole-shield snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShieldStatus {
    /// Aggregate level.
    pub level: StatusLevel,
    /// One-line summary for menus / speech.
    pub summary: String,
    /// Per-proton lines.
    pub protons: Vec<ProtonStatus>,
    /// Lockdown proposal pending (id), if any.
    pub pending_lockdown: Option<String>,
    /// Unix epoch of this snapshot.
    pub taken_unix: u64,
}

impl ShieldStatus {
    /// Build from the latest readings.
    pub fn from_readings(
        readings: &[ProtonReading],
        pending_lockdown: Option<String>,
        paused: bool,
        taken_unix: u64,
    ) -> Self {
        let mut level = if paused {
            StatusLevel::Paused
        } else {
            StatusLevel::Nominal
        };
        if !paused {
            for r in readings {
                match r.kind {
                    ProtonKind::Alert => level = StatusLevel::Critical,
                    ProtonKind::Notice if level == StatusLevel::Nominal => {
                        level = StatusLevel::Elevated;
                    }
                    _ => {}
                }
            }
            if pending_lockdown.is_some() && level != StatusLevel::Critical {
                level = StatusLevel::Elevated;
            }
        }
        let protons: Vec<ProtonStatus> = ProtonId::ALL
            .iter()
            .map(|&id| ProtonStatus {
                id,
                last: readings.iter().rev().find(|r| r.id == id).cloned(),
            })
            .collect();
        let summary = if paused {
            "QuantumSec paused".into()
        } else {
            format!(
                "QuantumSec {} — {}",
                level.label(),
                readings
                    .iter()
                    .map(|r| format!("{}:{}", r.id.label(), r.summary))
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        };
        Self {
            level,
            summary,
            protons,
            pending_lockdown,
            taken_unix,
        }
    }

    /// Short line for the right-click menu / Say bubble.
    pub fn menu_line(&self) -> String {
        format!("QuantumSec: {}", self.level.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proton::ProtonId;

    #[test]
    fn alert_raises_critical() {
        let r = ProtonReading {
            id: ProtonId::ResourceLoad,
            kind: ProtonKind::Alert,
            summary: "cpu=99%".into(),
            detail: None,
            taken_unix: 1,
        };
        let s = ShieldStatus::from_readings(&[r], None, false, 1);
        assert_eq!(s.level, StatusLevel::Critical);
        assert!(s.menu_line().contains("CRITICAL"));
    }
}
