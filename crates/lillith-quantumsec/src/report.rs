//! Personalization report: device profile → Markdown under `~/.biotek/reports`.
//!
//! Fish helper commands are **shown** in the report and require the blood seal
//! before anything runs — this module never executes them.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Snapshot of the local machine used for a personalization report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceProfile {
    /// Host name.
    pub hostname: String,
    /// Local username.
    pub username: String,
    /// OS description.
    pub os: String,
    /// CPU architecture.
    pub arch: String,
    /// Logical CPU count.
    pub cpu_cores: usize,
    /// Total RAM in MiB.
    pub total_memory_mb: u64,
    /// Resolved biotek home path.
    pub biotek_home: String,
    /// Unix time the profile was taken.
    pub generated_unix: u64,
}

impl DeviceProfile {
    /// Collect a light local profile (sysinfo + env). Idle-friendly.
    pub fn collect(biotek: &Path) -> Self {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        sys.refresh_cpu_usage();
        let hostname = sysinfo::System::host_name().unwrap_or_else(|| "unknown".into());
        let username = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "unknown".into());
        let os = sysinfo::System::long_os_version()
            .or_else(sysinfo::System::name)
            .unwrap_or_else(|| std::env::consts::OS.to_string());
        Self {
            hostname,
            username,
            os,
            arch: std::env::consts::ARCH.into(),
            cpu_cores: sys.cpus().len().max(1),
            total_memory_mb: sys.total_memory() / 1024 / 1024,
            biotek_home: biotek.display().to_string(),
            generated_unix: now_unix(),
        }
    }

    /// Consent-gated Fish helper lines (shown, never run here).
    pub fn fish_helpers(&self) -> Vec<String> {
        vec![
            format!(
                "set -gx BIOTEK_HOME {}",
                shell_single_quote(&self.biotek_home)
            ),
            "mkdir -p $BIOTEK_HOME/{config,logs,memory,reports,bots/registry,bots/swarm,consent,quantumsec,teams/{red,blue,purple}}".into(),
            "functions -e biotek_qsec_status; function biotek_qsec_status; cat $BIOTEK_HOME/quantumsec/status.json 2>/dev/null; end".into(),
        ]
    }

    /// Render Markdown report body.
    pub fn to_markdown(&self) -> String {
        let fish = self
            .fish_helpers()
            .iter()
            .map(|l| format!("    {l}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            r#"# Lillith · QuantumSec personalization

Generated: {ts} (unix)
Host: **{host}** · User: **{user}**
OS: {os} · Arch: `{arch}`
CPU cores: {cores} · Memory: {mem} MiB
Biotek home: `{biotek}`

## Consent

Fish helpers below are **shown only**. Running them requires the blood seal
(mouse Yes) and an append to `~/.biotek/consent/`.

## Fish helpers (do not run until sealed)

```fish
{fish}
```

## Notes

- Red team exercises stay **LOCAL SIMULATION ONLY**.
- QuantumSec protons write `quantumsec/status.json` under the Biotek home.
- Reports rotate: keep the latest few under `reports/`.
"#,
            ts = self.generated_unix,
            host = self.hostname,
            user = self.username,
            os = self.os,
            arch = self.arch,
            cores = self.cpu_cores,
            mem = self.total_memory_mb,
            biotek = self.biotek_home,
            fish = fish,
        )
    }

    /// Write Markdown to `~/.biotek/reports/personalization-<unix>.md` and
    /// refresh `personalization-latest.md`. Returns the dated path.
    pub fn save_markdown(&self, biotek: &Path) -> std::io::Result<PathBuf> {
        let dir = biotek.join("reports");
        std::fs::create_dir_all(&dir)?;
        let dated = dir.join(format!("personalization-{}.md", self.generated_unix));
        let latest = dir.join("personalization-latest.md");
        let body = self.to_markdown();
        std::fs::write(&dated, &body)?;
        std::fs::write(&latest, &body)?;
        rotate_reports(&dir, REPORT_KEEP)?;
        Ok(dated)
    }

    /// Blood-seal proposal that would apply the Fish helpers (still local echo
    /// by default — real `fish -c` only if caller rebuilds with local_simulation=false).
    pub fn fish_apply_proposal(&self) -> crate::lockdown::LockdownProposal {
        let mut cmds: Vec<String> = self
            .fish_helpers()
            .into_iter()
            .map(|l| format!("echo '[QuantumSec] FISH PROPOSAL: {l}'"))
            .collect();
        cmds.push("true".into());
        let mut p = crate::lockdown::LockdownProposal::new(
            "Apply personalization Fish helpers (shown as echo; seal to acknowledge)",
            cmds,
        );
        p.local_simulation = true;
        p
    }
}

/// How many dated personalization-*.md files to retain (latest kept).
pub const REPORT_KEEP: usize = 8;

fn rotate_reports(dir: &Path, keep: usize) -> std::io::Result<()> {
    let mut files: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("personalization-") && n.ends_with(".md") && n != "personalization-latest.md")
                .unwrap_or(false)
        })
        .collect();
    files.sort();
    let excess = files.len().saturating_sub(keep);
    for p in files.into_iter().take(excess) {
        let _ = std::fs::remove_file(p);
    }
    Ok(())
}

fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r#"'\''"#))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_contains_fish_and_consent() {
        let dir = std::env::temp_dir().join(format!("qsec-rep-{}", now_unix()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = DeviceProfile::collect(&dir);
        let md = p.to_markdown();
        assert!(md.contains("Fish helpers"));
        assert!(md.contains("BIOTEK_HOME"));
        assert!(md.contains("blood seal"));
        let path = p.save_markdown(&dir).unwrap();
        assert!(path.exists());
        assert!(dir.join("reports/personalization-latest.md").exists()
            || dir.join("personalization-latest.md").exists()
            || std::fs::read_to_string(dir.join("reports").join("personalization-latest.md")).is_ok()
            || {
                // save writes under biotek/reports
                true
            });
        let latest = dir.join("reports").join("personalization-latest.md");
        assert!(latest.exists(), "{}", latest.display());
    }

    #[test]
    fn fish_proposal_is_local_sim() {
        let dir = std::env::temp_dir().join("qsec-rep-fish");
        let _ = std::fs::create_dir_all(&dir);
        let p = DeviceProfile::collect(&dir).fish_apply_proposal();
        assert!(p.local_simulation);
        assert!(p.detail_block().contains("FISH PROPOSAL"));
    }
}
