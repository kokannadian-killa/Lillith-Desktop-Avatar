//! Consent log under `~/.biotek/consent/`: approve / deny for blood-seal
//! decisions (lockdown, bot deploy, hardening). Append-only JSONL.

use crate::lockdown::{ConsentToken, LockdownProposal};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Approve or deny.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsentDecision {
    /// Mouse-click Yes on the blood seal.
    Approve,
    /// Refuse / Esc.
    Deny,
}

/// One consent-log line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentRecord {
    /// Unix epoch.
    pub unix: u64,
    /// UTC timestamp string.
    pub ts: String,
    /// What was gated (`lockdown`, `bot_deploy`, …).
    pub kind: String,
    /// Proposal / action id.
    pub id: String,
    /// Decision.
    pub decision: ConsentDecision,
    /// Exact command block shown on the card.
    pub commands: Vec<String>,
    /// Token nonce when approved (empty on deny).
    pub token_nonce: String,
    /// Free-form summary.
    pub summary: String,
}

/// Append-only consent log rooted at `~/.biotek/consent/`.
pub struct ConsentLog {
    dir: PathBuf,
}

impl ConsentLog {
    /// Open (creating) the consent directory under the Biotek home.
    pub fn open_default() -> io::Result<Self> {
        let home = lillith_setup::biotek::biotek_home().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no Biotek home")
        })?;
        Self::open_in(home.join("consent"))
    }

    /// Open a specific consent directory (tests).
    pub fn open_in(dir: PathBuf) -> io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// Path of today's JSONL file.
    pub fn today_path(&self) -> PathBuf {
        let ts = format_utc(SystemTime::now());
        let day = &ts[..10];
        self.dir.join(format!("consent-{day}.jsonl"))
    }

    /// Directory this log writes into.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Record an approval for `proposal` and issue a [`ConsentToken`].
    pub fn approve(&self, proposal: &LockdownProposal) -> io::Result<ConsentToken> {
        let token = ConsentToken::issue(&proposal.id);
        let rec = ConsentRecord {
            unix: now_unix(),
            ts: format_utc(SystemTime::now()),
            kind: "lockdown".into(),
            id: proposal.id.clone(),
            decision: ConsentDecision::Approve,
            commands: proposal.commands.clone(),
            token_nonce: token.nonce.clone(),
            summary: proposal.summary.clone(),
        };
        self.append(&rec)?;
        Ok(token)
    }

    /// Record a denial (no token).
    pub fn deny(&self, proposal: &LockdownProposal) -> io::Result<()> {
        let rec = ConsentRecord {
            unix: now_unix(),
            ts: format_utc(SystemTime::now()),
            kind: "lockdown".into(),
            id: proposal.id.clone(),
            decision: ConsentDecision::Deny,
            commands: proposal.commands.clone(),
            token_nonce: String::new(),
            summary: proposal.summary.clone(),
        };
        self.append(&rec)
    }

    /// Record a generic gated action (bot deploy, hardening, …).
    pub fn record(
        &self,
        kind: &str,
        id: &str,
        decision: ConsentDecision,
        summary: &str,
        commands: &[String],
        token_nonce: &str,
    ) -> io::Result<()> {
        let rec = ConsentRecord {
            unix: now_unix(),
            ts: format_utc(SystemTime::now()),
            kind: kind.into(),
            id: id.into(),
            decision,
            commands: commands.to_vec(),
            token_nonce: token_nonce.into(),
            summary: summary.into(),
        };
        self.append(&rec)
    }

    fn append(&self, rec: &ConsentRecord) -> io::Result<()> {
        let path = self.today_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = OpenOptions::new().create(true).append(true).open(path)?;
        serde_json::to_writer(&mut f, rec)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        f.write_all(b"\n")?;
        Ok(())
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn format_utc(t: SystemTime) -> String {
    // Same civil_from_days helper as lillith-setup::audit.
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Awakening / early-usage flag under `~/.biotek/config/awakening.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AwakeningState {
    /// True once the early-usage dialog has been shown and dismissed.
    pub shown: bool,
    /// Unix epoch of first show.
    pub shown_unix: Option<u64>,
}

impl AwakeningState {
    /// Path of the awakening flag file.
    pub fn path_default() -> Option<PathBuf> {
        lillith_setup::biotek::biotek_home()
            .map(|h| h.join("config").join("awakening.json"))
    }

    /// Load from disk (missing → default).
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Persist.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let s = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        std::fs::write(path, s)
    }

    /// Mark shown now.
    pub fn mark_shown(&mut self) {
        self.shown = true;
        self.shown_unix = Some(now_unix());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approve_and_deny_append_jsonl() {
        let dir = std::env::temp_dir().join(format!(
            "lillith-qsec-consent-{}-{}",
            std::process::id(),
            now_unix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let log = ConsentLog::open_in(dir.clone()).unwrap();
        let p = LockdownProposal::local_simulation_default();
        let tok = log.approve(&p).unwrap();
        assert_eq!(tok.proposal_id, p.id);
        log.deny(&p).unwrap();
        let text = std::fs::read_to_string(log.today_path()).unwrap();
        let lines: Vec<_> = text.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"approve\""));
        assert!(lines[1].contains("\"deny\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn awakening_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "lillith-qsec-awaken-{}-{}",
            std::process::id(),
            now_unix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("awakening.json");
        let mut s = AwakeningState::load(&path);
        assert!(!s.shown);
        s.mark_shown();
        s.save(&path).unwrap();
        let s2 = AwakeningState::load(&path);
        assert!(s2.shown);
        assert!(s2.shown_unix.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
