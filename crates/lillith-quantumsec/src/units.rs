//! Systemd user-unit enable / disable (blood-seal gated; never auto-enabled).
//!
//! These are **real** host commands (not Red-team echo dry-runs). The blood
//! seal must show the exact lines below and run them only after a mouse-click
//! YES; the settings tab logs approve / result under `~/.biotek/consent/`.

use crate::lockdown::LockdownProposal;
use std::path::Path;

/// Blood-seal proposal to enable the QuantumSec user unit (exact real commands).
pub fn enable_quantumsec_unit_proposal(unit_path_hint: &Path) -> LockdownProposal {
    let unit = unit_path_hint.display().to_string();
    let cmds = vec![
        "mkdir -p ~/.config/systemd/user".into(),
        format!(
            "install -m 644 {} ~/.config/systemd/user/lillith-quantumsec.service",
            unit
        ),
        "systemctl --user daemon-reload".into(),
        "systemctl --user enable --now lillith-quantumsec.service".into(),
    ];
    let mut p = LockdownProposal::new(
        "Enable QuantumSec user unit (systemctl --user)",
        cmds,
    );
    p.local_simulation = false;
    p.proposal_only = false;
    p
}

/// Blood-seal proposal to disable/stop the unit (exact real commands).
pub fn disable_quantumsec_unit_proposal() -> LockdownProposal {
    let cmds = vec![
        "systemctl --user disable --now lillith-quantumsec.service".into(),
    ];
    let mut p = LockdownProposal::new(
        "Disable QuantumSec user unit (systemctl --user)",
        cmds,
    );
    p.local_simulation = false;
    p.proposal_only = false;
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lockdown::{execute_with_token, ConsentToken};
    use std::path::PathBuf;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn enable_shows_exact_real_commands_not_echo() {
        let p = enable_quantumsec_unit_proposal(&PathBuf::from(
            "/usr/lib/systemd/user/lillith-quantumsec.service",
        ));
        assert!(!p.local_simulation);
        assert!(!p.proposal_only);
        assert_eq!(p.seal_banner(), "REAL COMMANDS — review carefully");
        let d = p.detail_block();
        assert!(d.contains("mkdir -p ~/.config/systemd/user"));
        assert!(d.contains("install -m 644"));
        assert!(d.contains("systemctl --user daemon-reload"));
        assert!(d.contains("systemctl --user enable --now lillith-quantumsec.service"));
        assert!(!d.contains("echo "));
        assert!(!d.contains("LOCAL SIMULATION"));
    }

    #[test]
    fn enable_runs_with_fake_systemctl_on_path() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!(
            "lillith-qsec-unit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let bin = root.join("bin");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let unit_src = root.join("lillith-quantumsec.service");
        std::fs::write(&unit_src, "[Unit]\nDescription=test\n").unwrap();
        let log = root.join("systemctl.log");
        let fake = bin.join("systemctl");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
            log.display()
        );
        std::fs::write(&fake, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake, perms).unwrap();
        }

        let old_home = std::env::var_os("HOME");
        let old_path = std::env::var_os("PATH");
        std::env::set_var("HOME", &home);
        let path = match &old_path {
            Some(p) => format!("{}:{}", bin.display(), PathBuf::from(p).display()),
            None => bin.display().to_string(),
        };
        std::env::set_var("PATH", &path);

        let p = enable_quantumsec_unit_proposal(&unit_src);
        let mut tok = ConsentToken::issue(&p.id);
        let outs = execute_with_token(&p, &mut tok).expect("execute");
        assert_eq!(outs.len(), p.commands.len());
        assert!(tok.consumed);
        let installed = home.join(".config/systemd/user/lillith-quantumsec.service");
        assert!(
            installed.exists(),
            "unit should be installed under $HOME: {}",
            installed.display()
        );
        let log_text = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(
            log_text.contains("daemon-reload"),
            "systemctl log missing daemon-reload: {log_text:?}"
        );
        assert!(
            log_text.contains("enable") && log_text.contains("lillith-quantumsec.service"),
            "systemctl log missing enable: {log_text:?}"
        );

        match old_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match old_path {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
