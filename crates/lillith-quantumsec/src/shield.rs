//! [`QuantumSecShield`]: owns the protons, status, callbacks, and lockdown
//! proposals. Never executes a lockdown without a [`ConsentToken`].

use crate::consent::ConsentLog;
use crate::lockdown::{
    execute_with_token, ConsentToken, LockdownError, LockdownProposal,
};
use crate::proton::{default_watch_paths, Proton, ProtonId, ProtonReading, DEFAULT_POLL};
use crate::status::{ShieldStatus, StatusLevel};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use sysinfo::System;

/// Events the shield emits to listeners.
#[derive(Debug, Clone, PartialEq)]
pub enum ShieldEvent {
    /// Fresh status after a poll tick.
    Status(ShieldStatus),
    /// A lockdown was proposed (awaiting blood seal).
    LockdownProposed(LockdownProposal),
    /// Lockdown was approved and (optionally) executed.
    LockdownExecuted {
        /// Proposal id.
        id: String,
        /// Number of commands run.
        commands_run: usize,
    },
    /// Lockdown was denied or refused without a token.
    LockdownRefused {
        /// Proposal id.
        id: String,
        /// Why.
        reason: String,
    },
}

/// Tunables.
#[derive(Debug, Clone)]
pub struct ShieldConfig {
    /// Poll interval (default 5 s).
    pub poll: Duration,
    /// Biotek home override (tests).
    pub biotek_home: Option<PathBuf>,
    /// Extra integrity watch paths.
    pub extra_watch: Vec<PathBuf>,
    /// When true, `tick` is a no-op until resumed.
    pub start_paused: bool,
}

impl Default for ShieldConfig {
    fn default() -> Self {
        Self {
            poll: DEFAULT_POLL,
            biotek_home: None,
            extra_watch: Vec::new(),
            start_paused: false,
        }
    }
}

type Callback = Arc<dyn Fn(ShieldEvent) + Send + Sync + 'static>;

/// The QuantumSec shield.
pub struct QuantumSecShield {
    protons: Vec<Proton>,
    sys: System,
    latest: Vec<ProtonReading>,
    pending: Option<LockdownProposal>,
    paused: bool,
    callbacks: Vec<Callback>,
    biotek: PathBuf,
    status_path: PathBuf,
    consent: ConsentLog,
}

impl QuantumSecShield {
    /// Build a shield rooted at the Biotek home (creating layout as needed).
    pub fn new(cfg: ShieldConfig) -> std::io::Result<Self> {
        let biotek = match cfg.biotek_home {
            Some(p) => p,
            None => lillith_setup::biotek::biotek_home().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "no Biotek home")
            })?,
        };
        lillith_setup::biotek::ensure_layout_in(&biotek)?;
        let qdir = biotek.join("quantumsec");
        std::fs::create_dir_all(&qdir)?;
        let consent = ConsentLog::open_in(biotek.join("consent"))?;
        let mut watch = default_watch_paths(&biotek);
        watch.extend(cfg.extra_watch);
        let protons = ProtonId::ALL
            .iter()
            .map(|&id| {
                let paths = if id == ProtonId::FileIntegrity {
                    watch.clone()
                } else {
                    vec![]
                };
                Proton::new(id, paths)
            })
            .collect();
        Ok(Self {
            protons,
            sys: System::new(),
            latest: Vec::new(),
            pending: None,
            paused: cfg.start_paused,
            callbacks: Vec::new(),
            biotek,
            status_path: qdir.join("status.json"),
            consent,
        })
    }

    /// Biotek home this shield uses.
    pub fn biotek_home(&self) -> &Path {
        &self.biotek
    }

    /// Path of the status snapshot file.
    pub fn status_path(&self) -> &Path {
        &self.status_path
    }

    /// Consent log handle.
    pub fn consent(&self) -> &ConsentLog {
        &self.consent
    }

    /// Register an event callback.
    pub fn on_event<F>(&mut self, f: F)
    where
        F: Fn(ShieldEvent) + Send + Sync + 'static,
    {
        self.callbacks.push(Arc::new(f));
    }

    fn emit(&self, ev: ShieldEvent) {
        for cb in &self.callbacks {
            cb(ev.clone());
        }
    }

    /// Pause / resume polling.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        let _ = self.persist_status();
    }

    /// Is polling paused?
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// One poll of every proton; updates status and notifies callbacks.
    pub fn tick(&mut self) -> ShieldStatus {
        if !self.paused {
            self.latest.clear();
            // Split borrows: poll each proton with the shared System.
            let n = self.protons.len();
            for i in 0..n {
                // Safe: only one proton borrowed mutably at a time; sys is separate.
                let reading = {
                    let p = &mut self.protons[i];
                    p.poll(&mut self.sys)
                };
                self.latest.push(reading);
            }
        }
        let status = self.status();
        let _ = self.write_status(&status);
        self.emit(ShieldEvent::Status(status.clone()));
        status
    }

    /// Current status (from last tick / pending).
    pub fn status(&self) -> ShieldStatus {
        ShieldStatus::from_readings(
            &self.latest,
            self.pending.as_ref().map(|p| p.id.clone()),
            self.paused,
            now_unix(),
        )
    }

    /// Read the on-disk status snapshot, if any.
    pub fn load_status_file(path: &Path) -> Option<ShieldStatus> {
        let s = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&s).ok()
    }

    fn write_status(&self, status: &ShieldStatus) -> std::io::Result<()> {
        if let Some(parent) = self.status_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let s = serde_json::to_string_pretty(status)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(&self.status_path, s)
    }

    fn persist_status(&mut self) -> std::io::Result<()> {
        let s = self.status();
        self.write_status(&s)
    }

    /// Propose a local-simulation lockdown (does **not** execute).
    pub fn propose_lockdown(&mut self) -> LockdownProposal {
        let p = LockdownProposal::local_simulation_default();
        self.pending = Some(p.clone());
        let _ = self.persist_status();
        self.emit(ShieldEvent::LockdownProposed(p.clone()));
        p
    }

    /// Propose a custom lockdown (caller supplies exact commands).
    pub fn propose_custom(&mut self, proposal: LockdownProposal) -> LockdownProposal {
        self.pending = Some(proposal.clone());
        let _ = self.persist_status();
        self.emit(ShieldEvent::LockdownProposed(proposal.clone()));
        proposal
    }

    /// Pending proposal, if any.
    pub fn pending_lockdown(&self) -> Option<&LockdownProposal> {
        self.pending.as_ref()
    }

    /// Blood-seal approve: log consent, issue token, optionally execute.
    ///
    /// When `execute` is false, only the token is returned (caller runs later).
    pub fn approve_lockdown(
        &mut self,
        execute: bool,
    ) -> Result<ConsentToken, LockdownError> {
        let proposal = self
            .pending
            .clone()
            .ok_or(LockdownError::ConsentRequired)?;
        let mut token = self
            .consent
            .approve(&proposal)
            .map_err(|e| LockdownError::Spawn(e.to_string()))?;
        if execute {
            let outs = execute_with_token(&proposal, &mut token)?;
            self.pending = None;
            let _ = self.persist_status();
            self.emit(ShieldEvent::LockdownExecuted {
                id: proposal.id.clone(),
                commands_run: outs.len(),
            });
        }
        Ok(token)
    }

    /// Blood-seal deny: log and clear pending.
    pub fn deny_lockdown(&mut self) -> Result<(), std::io::Error> {
        if let Some(p) = self.pending.take() {
            self.consent.deny(&p)?;
            let _ = self.persist_status();
            self.emit(ShieldEvent::LockdownRefused {
                id: p.id,
                reason: "denied by blood seal".into(),
            });
        }
        Ok(())
    }

    /// Execute a previously approved token against the pending (or given) proposal.
    pub fn execute_approved(
        &mut self,
        proposal: &LockdownProposal,
        token: &mut ConsentToken,
    ) -> Result<usize, LockdownError> {
        let outs = execute_with_token(proposal, token)?;
        if self.pending.as_ref().map(|p| p.id.as_str()) == Some(proposal.id.as_str()) {
            self.pending = None;
        }
        let _ = self.persist_status();
        self.emit(ShieldEvent::LockdownExecuted {
            id: proposal.id.clone(),
            commands_run: outs.len(),
        });
        Ok(outs.len())
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Background handle: a thread that ticks the shield on `poll` interval.
pub struct ShieldHandle {
    stop: Sender<()>,
    join: Option<JoinHandle<()>>,
    inner: Arc<Mutex<QuantumSecShield>>,
}

impl ShieldHandle {
    /// Spawn a background poller. Dropping / [`ShieldHandle::stop`] ends it.
    pub fn spawn(shield: QuantumSecShield, poll: Duration) -> Self {
        let inner = Arc::new(Mutex::new(shield));
        let (tx, rx): (Sender<()>, Receiver<()>) = mpsc::channel();
        let worker = Arc::clone(&inner);
        let join = thread::Builder::new()
            .name("lillith-quantumsec".into())
            .spawn(move || loop {
                match rx.recv_timeout(poll) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if let Ok(mut s) = worker.lock() {
                            let _ = s.tick();
                        }
                    }
                }
            })
            .ok();
        Self {
            stop: tx,
            join,
            inner,
        }
    }

    /// Shared shield (for propose / approve from the UI thread).
    pub fn shield(&self) -> Arc<Mutex<QuantumSecShield>> {
        Arc::clone(&self.inner)
    }

    /// Latest status (lock + clone).
    pub fn status(&self) -> Option<ShieldStatus> {
        self.inner.lock().ok().map(|s| s.status())
    }

    /// Stop the background thread.
    pub fn stop(mut self) {
        let _ = self.stop.send(());
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for ShieldHandle {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Level helper for menus when only the file is available.
pub fn status_level_label(path: &Path) -> String {
    match QuantumSecShield::load_status_file(path) {
        Some(s) => s.menu_line(),
        None => format!("QuantumSec: {}", StatusLevel::Paused.label()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "lillith-qsec-shield-{tag}-{}-{}",
            std::process::id(),
            now_unix()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn tick_writes_status_and_propose_needs_consent() {
        let home = tmp_home("tick");
        let mut shield = QuantumSecShield::new(ShieldConfig {
            biotek_home: Some(home.clone()),
            ..ShieldConfig::default()
        })
        .unwrap();
        shield.on_event(|ev| {
            let _ = ev;
        });
        let st = shield.tick();
        assert!(!st.summary.is_empty());
        assert!(shield.status_path().exists());
        let loaded = QuantumSecShield::load_status_file(shield.status_path()).unwrap();
        assert_eq!(loaded.level, st.level);

        let p = shield.propose_lockdown();
        assert!(p.local_simulation);
        // Without approve, execute path refuses.
        let mut bad = ConsentToken::issue("nope");
        assert!(shield.execute_approved(&p, &mut bad).is_err());

        let tok = shield.approve_lockdown(true).unwrap();
        // approve_lockdown(true) already executed; token returned is consumed.
        assert!(tok.consumed);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn deny_clears_pending() {
        let home = tmp_home("deny");
        let mut shield = QuantumSecShield::new(ShieldConfig {
            biotek_home: Some(home.clone()),
            ..ShieldConfig::default()
        })
        .unwrap();
        let p = shield.propose_lockdown();
        assert!(shield.pending_lockdown().is_some());
        shield.deny_lockdown().unwrap();
        assert!(shield.pending_lockdown().is_none());
        let text = std::fs::read_to_string(shield.consent().today_path()).unwrap();
        assert!(text.contains(&p.id));
        assert!(text.contains("deny"));
        let _ = std::fs::remove_dir_all(&home);
    }
}
