//! App-building helper jobs ([`super::WorkerKind::Build`]): the type and its
//! gate only. **Nothing here executes anything.**
//!
//! Ko wants helper workers to build applications later. The future pipeline
//! follows docs §6 (P4 practice sandbox): `bwrap --unshare-all
//! --die-with-parent --new-session` (no network namespace), read-only
//! `/usr`, a tmpfs `/tmp`, only `sandbox/<job>/` writable, then
//! `prlimit --as --cpu --nproc --fsize` and `timeout -k 2 <s>` around the
//! compile / run argv, nice 19 / ionice idle; generated code is rejected
//! before compile if it mentions `std::net`, `socket`, `Command`, `unsafe`,
//! `extern` or `include!`; no installs (§9). Every compile and every run
//! needs its own Soul Contract card (`PendingAction::SandboxRun`) and a
//! mouse click on Yes: no standing approval, a typed or spoken yes never
//! counts, and the safeword cancels any open card.

use lillith_core::card_process::CardAnswer;
use std::time::Duration;

/// Network access of a build job. Only `Denied` exists: there is no way to
/// ask for more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkPolicy {
    Denied,
}

/// A build job's own budgets (defaults from docs §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildBudgets {
    /// `timeout` around the compile.
    pub compile_wall: Duration,
    /// `timeout` around the run.
    pub run_wall: Duration,
    /// `prlimit --as`.
    pub memory_bytes: u64,
    /// `prlimit --cpu` (seconds).
    pub cpu_s: u64,
    /// `prlimit --nproc`.
    pub nproc: u32,
    /// `prlimit --fsize`.
    pub fsize_bytes: u64,
    /// The whole `sandbox/<job>/` dir.
    pub scratch_bytes: u64,
    pub network: NetworkPolicy,
}

impl Default for BuildBudgets {
    fn default() -> Self {
        Self {
            compile_wall: Duration::from_secs(30),
            run_wall: Duration::from_secs(30),
            memory_bytes: 512 * 1024 * 1024,
            cpu_s: 20,
            nproc: 64,
            fsize_bytes: 16 * 1024 * 1024,
            scratch_bytes: 50 * 1024 * 1024,
            network: NetworkPolicy::Denied,
        }
    }
}

/// What a build job would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStep {
    Compile,
    Run,
}

/// Proof of one mouse click on Yes on this step's card. Only
/// [`CardApproval::from_answer`] makes one, and only from
/// [`CardAnswer::Yes`]; it is consumed by the step it approves.
#[derive(Debug, PartialEq, Eq)]
pub struct CardApproval {
    _private: (),
}

impl CardApproval {
    /// `Yes` (a mouse click) approves; No, timeout, closed card, no
    /// display — and the safeword (`stopped`) — never do.
    pub fn from_answer(a: CardAnswer, stopped: bool) -> Option<Self> {
        (a == CardAnswer::Yes && !stopped).then_some(Self { _private: () })
    }
}

/// Why a build step did not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// No card click for this step.
    RequiresCard,
    /// The build pipeline is not wired in this slice.
    NotImplemented,
}

/// A sandboxed build job (type only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildJob {
    pub id: String,
    /// What Ko asked for.
    pub purpose: String,
    pub budgets: BuildBudgets,
    needs_card_per_run: bool,
}

impl BuildJob {
    pub fn new(id: &str, purpose: &str) -> Self {
        Self {
            id: id.to_string(),
            purpose: purpose.to_string(),
            budgets: BuildBudgets::default(),
            needs_card_per_run: true,
        }
    }

    /// Always `true` (no standing approval, D2 = A).
    pub fn needs_card_per_run(&self) -> bool {
        self.needs_card_per_run
    }

    /// Runs one step: without its own card approval → `RequiresCard`; with
    /// one → `NotImplemented` (nothing is executed in this slice).
    pub fn execute(&self, _step: BuildStep, card: Option<CardApproval>) -> Result<(), BuildError> {
        if self.needs_card_per_run && card.is_none() {
            return Err(BuildError::RequiresCard);
        }
        Err(BuildError::NotImplemented)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_job_cannot_run_without_a_card_and_never_runs_in_this_slice() {
        let j = BuildJob::new("w-b1", "a tiny todo app");
        assert!(j.needs_card_per_run());
        assert_eq!(
            j.execute(BuildStep::Compile, None),
            Err(BuildError::RequiresCard)
        );
        assert_eq!(
            j.execute(BuildStep::Run, None),
            Err(BuildError::RequiresCard)
        );
        for a in [
            CardAnswer::No,
            CardAnswer::TimedOut,
            CardAnswer::Cancelled,
            CardAnswer::NoUi,
        ] {
            assert_eq!(CardApproval::from_answer(a, false), None, "{a:?}");
        }
        assert_eq!(
            CardApproval::from_answer(CardAnswer::Yes, true),
            None,
            "safeword cancels the card"
        );
        let yes = CardApproval::from_answer(CardAnswer::Yes, false);
        assert!(yes.is_some());
        assert_eq!(
            j.execute(BuildStep::Run, yes),
            Err(BuildError::NotImplemented),
            "no exec wired"
        );
    }

    #[test]
    fn build_defaults_deny_network_and_are_resource_limited() {
        let b = BuildBudgets::default();
        assert_eq!(b.network, NetworkPolicy::Denied);
        assert_eq!(b.compile_wall, Duration::from_secs(30));
        assert_eq!(b.memory_bytes, 512 * 1024 * 1024);
        assert_eq!(b.fsize_bytes, 16 * 1024 * 1024);
        assert_eq!(b.cpu_s, 20);
        assert_eq!(b.nproc, 64);
        assert!(b.scratch_bytes <= 50 * 1024 * 1024);
        assert!(!super::super::WorkerKind::Build.implemented());
        assert!(super::super::Budgets::for_kind(
            super::super::WorkerKind::Build,
            &Default::default()
        )
        .is_none());
    }
}
