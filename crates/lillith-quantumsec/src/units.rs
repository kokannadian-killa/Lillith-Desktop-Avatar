//! Systemd user-unit enable proposals (blood-seal gated; never auto-enabled).

use crate::lockdown::LockdownProposal;
use std::path::Path;

/// Blood-seal proposal to enable the QuantumSec user unit (exact commands).
///
/// Default is **local simulation** (`echo` of the systemctl lines). Callers
/// that want a real enable rebuild with `local_simulation = false` after the
/// user seals — this module never runs systemctl itself.
pub fn enable_quantumsec_unit_proposal(unit_path_hint: &Path) -> LockdownProposal {
    let unit = unit_path_hint.display().to_string();
    let cmds = vec![
        format!("echo '[QuantumSec] would: mkdir -p ~/.config/systemd/user'"),
        format!(
            "echo '[QuantumSec] would: install -m 644 {} ~/.config/systemd/user/lillith-quantumsec.service'",
            unit
        ),
        "echo '[QuantumSec] would: systemctl --user daemon-reload'".into(),
        "echo '[QuantumSec] would: systemctl --user enable --now lillith-quantumsec.service'".into(),
        "true".into(),
    ];
    let mut p = LockdownProposal::new(
        "Enable QuantumSec user unit (systemctl --user; shown as echo until seal)",
        cmds,
    );
    p.local_simulation = true;
    p
}

/// Blood-seal proposal to disable/stop the unit.
pub fn disable_quantumsec_unit_proposal() -> LockdownProposal {
    let cmds = vec![
        "echo '[QuantumSec] would: systemctl --user disable --now lillith-quantumsec.service'".into(),
        "true".into(),
    ];
    let mut p = LockdownProposal::new(
        "Disable QuantumSec user unit (systemctl --user; shown as echo until seal)",
        cmds,
    );
    p.local_simulation = true;
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn enable_is_local_simulation() {
        let p = enable_quantumsec_unit_proposal(&PathBuf::from(
            "/usr/lib/systemd/user/lillith-quantumsec.service",
        ));
        assert!(p.local_simulation);
        assert!(p.detail_block().contains("systemctl --user"));
    }
}
