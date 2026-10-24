//! Helper workers (docs/SELF_LEARNING.md §12, D10), milestone P7, first
//! slice: the parent **supervisor** and the worker lifecycle. No dream
//! orchestration, cards, chat, CLI or panel here.
//!
//! A helper worker is a fresh child process per task that does one bounded
//! piece of research and hands **candidate facts** back to the parent. The
//! parent ([`supervisor::Supervisor`]) owns everything that matters:
//!
//! - **Lifespans.** Every worker gets a hard TTL at spawn ([`Budgets`]:
//!   wall clock plus fetch, page, byte and token budgets). On breach the
//!   supervisor kills it (SIGKILL of its process group, no asking).
//!   Heartbeats every `heartbeat_s`; `missed_heartbeats_kill` missed → kill.
//!   Workers never outlive the session, the safeword, End early or the
//!   parent ([`StopReason`], `PR_SET_PDEATHSIG`, bwrap `--die-with-parent`).
//! - **Least privilege.** A worker has no memory write, no file system
//!   outside its tmpfs scratch dir, and — stricter than the doc's "Tor
//!   SOCKS port only" — **no network at all**: it asks the parent for each
//!   read-only GET ([`protocol`]); the parent vets the address
//!   ([`policy`]), paces it (one fetch at a time, jitter), fetches it over
//!   Tor on the job's own circuit, keeps only extracted text and hands that
//!   back. No exec (bwrap + seccomp, [`process`]).
//! - **Capture handling.** Unexpected domains, loopback / LAN addresses,
//!   write attempts (POST, forms, cookies, uploads, logins), output asking to
//!   run commands or change settings, oversized output, or the planted
//!   canary showing up → kill, quarantine every result read-only
//!   ([`quarantine`]), audit `WORKER\tkill\t<reason>\tjob=<id>`.
//! - **Errors.** Restart limits with exponential backoff, per-domain and
//!   per-source circuit breakers fed by an error memory ([`breaker`]), a
//!   crash-safe resumable D8 job log ([`joblog`]), graceful degradation
//!   (Tor down → nothing spawned, offline-only learning continues).
//!
//! Off by default: the supervisor only exists when the online gate
//! ([`crate::online::gate`]: `[learn] online`, `[learn] enabled`,
//! `[memory] enabled`) is open, and the idle learner has no path here.
//!
//! **Kinds.** [`WorkerKind::Research`] runs under the supervisor.
//! [`WorkerKind::Build`] (Ko: helpers build applications) never does: it has
//! its own budgets and its own pipeline, the §6 practice sandbox
//! ([`build::Practice`]), with one Soul Contract card per compile and per
//! run, behind the same online gate plus `[learn] practice`.

pub mod breaker;
pub mod build;
pub mod external;
pub mod joblog;
pub mod policy;
pub mod process;
pub mod protocol;
pub mod quarantine;
pub mod scratch;
pub mod static_check;
pub mod supervisor;
pub mod validate;

#[cfg(test)]
mod tests;

pub use breaker::{Breakers, ErrorMemory};
pub use build::{
    BuildBudgets, BuildError, BuildJob, BuildStep, CardApproval, NetworkPolicy, Practice,
    PracticeOff, SandboxAnswer, SandboxCardRequest, SandboxConsent,
};
pub use external::{Backend, ExternalBudget, ExternalRequest};
pub use joblog::{JobEntry, JobLog, JobStatus};
pub use static_check::Lang;
pub use supervisor::{JobReport, Refused, Session, Supervisor, Task};
pub use validate::Candidate;

use lillith_core::config::{LearnConfig, LearnWorkersConfig};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// What a helper worker is for. New kinds get their own budgets and their
/// own pipeline; the supervisor refuses kinds it does not implement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkerKind {
    /// Read-only research over Tor (dream learning, online research).
    Research,
    /// Build / run a small application in the practice sandbox (docs §6):
    /// [`build::Practice`], never the research supervisor.
    Build,
}

impl WorkerKind {
    pub fn word(self) -> &'static str {
        match self {
            WorkerKind::Research => "research",
            WorkerKind::Build => "build",
        }
    }

    /// Whether the research supervisor runs this kind (build jobs go
    /// through [`build::Practice`] instead).
    pub fn implemented(self) -> bool {
        matches!(self, WorkerKind::Research)
    }

    /// The D8 `bot` field of a local worker of this kind.
    pub fn bot(self) -> String {
        format!("local:{}-worker", self.word())
    }
}

/// Why every worker of a session stops at once. All of them kill the
/// workers and discard in-flight work. Battery / thermal detection belongs
/// to the dream orchestration; it only reports the reason here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// "red" / "OOC: stop", or the process-wide kill switch.
    Safeword,
    /// End early (right-click menu / tray).
    EndEarly,
    /// The session's hard deadline.
    SessionDeadline,
    /// The parent is shutting down.
    ParentExit,
    /// `[learn.dream] stop_battery_pct`.
    LowBattery,
    /// `[learn.dream] stop_cpu_temp_c`.
    Thermal,
}

impl StopReason {
    pub const ALL: [StopReason; 6] = [
        StopReason::Safeword,
        StopReason::EndEarly,
        StopReason::SessionDeadline,
        StopReason::ParentExit,
        StopReason::LowBattery,
        StopReason::Thermal,
    ];

    pub fn word(self) -> &'static str {
        match self {
            StopReason::Safeword => "safeword",
            StopReason::EndEarly => "end_early",
            StopReason::SessionDeadline => "session_deadline",
            StopReason::ParentExit => "parent_exit",
            StopReason::LowBattery => "low_battery",
            StopReason::Thermal => "thermal",
        }
    }

    fn code(self) -> u8 {
        StopReason::ALL.iter().position(|r| *r == self).unwrap_or(0) as u8 + 1
    }

    fn from_code(c: u8) -> Option<StopReason> {
        (c as usize)
            .checked_sub(1)
            .and_then(|i| StopReason::ALL.get(i).copied())
    }
}

/// The circuit breaker's open time (§12: "open for 30 min").
pub const BREAKER_OPEN: Duration = Duration::from_secs(30 * 60);
/// Backoff cap (§12: "capped at 300 s").
pub const BACKOFF_CAP: Duration = Duration::from_secs(300);
/// Pages in a row without a new fact before the job stops early (§12
/// "smart thinking": no new facts in the last N pages).
pub const EARLY_STOP_PAGES: u32 = 3;

/// `[learn.workers]` as durations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerLimits {
    /// `heartbeat_s`.
    pub heartbeat: Duration,
    /// `missed_heartbeats_kill`.
    pub missed_heartbeats_kill: u32,
    /// `max_restarts`.
    pub max_restarts: u32,
    /// `backoff_base_s`.
    pub backoff_base: Duration,
    /// `breaker_failures`.
    pub breaker_failures: u32,
    /// Fixed by the doc: 30 min.
    pub breaker_open: Duration,
}

impl WorkerLimits {
    pub fn from_config(w: &LearnWorkersConfig) -> Self {
        Self {
            heartbeat: Duration::from_secs(u64::from(w.heartbeat_s.max(1))),
            missed_heartbeats_kill: w.missed_heartbeats_kill.max(1),
            max_restarts: w.max_restarts,
            backoff_base: Duration::from_secs(u64::from(w.backoff_base_s)),
            breaker_failures: w.breaker_failures.max(1),
            breaker_open: BREAKER_OPEN,
        }
    }

    /// How long without a heartbeat before the kill.
    pub fn heartbeat_deadline(&self) -> Duration {
        self.heartbeat * self.missed_heartbeats_kill
    }
}

/// Delay before restart number `n` (1-based): `base · 2^(n-1)`, capped at
/// [`BACKOFF_CAP`] (5, 10, 20 … 300 s with the default `backoff_base_s`).
pub fn backoff_delay(base: Duration, n: u32) -> Duration {
    let shift = n.saturating_sub(1).min(31);
    base.checked_mul(1u32 << shift)
        .unwrap_or(BACKOFF_CAP)
        .min(BACKOFF_CAP)
}

/// A worker's hard TTL, set at spawn. Per kind; callers may only lower it
/// ([`Budgets::lowered`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Budgets {
    pub wall: Duration,
    /// GET requests (each redirect hop is one).
    pub fetches: u32,
    /// Pages delivered.
    pub pages: u32,
    /// Body bytes fetched.
    pub bytes: u64,
    /// Model tokens the worker reports.
    pub tokens: u32,
    /// Everything the worker writes to the parent.
    pub output_bytes: u64,
    /// One candidate fact.
    pub fact_bytes: usize,
    /// Facts accepted before the job stops ("enough").
    pub facts: u32,
}

/// Default research worker wall clock (New Bot default; the session
/// deadline always caps it).
pub const RESEARCH_WALL: Duration = Duration::from_secs(10 * 60);

impl Budgets {
    /// A research worker's budgets from `[learn]`: pages and bytes are the
    /// per-trip caps (`online_max_pages`, `online_max_bytes`).
    pub fn research(cfg: &LearnConfig) -> Self {
        let pages = cfg.online_max_pages.max(1);
        Self {
            wall: RESEARCH_WALL,
            fetches: pages.saturating_mul(1 + crate::online::MAX_REDIRECTS as u32),
            pages,
            bytes: cfg.online_max_bytes.max(1),
            tokens: crate::MAX_TOKENS * 8,
            output_bytes: 256 * 1024,
            fact_bytes: 2048,
            facts: 12,
        }
    }

    /// Budgets for `kind` (only research has a worker today).
    pub fn for_kind(kind: WorkerKind, cfg: &LearnConfig) -> Option<Self> {
        match kind {
            WorkerKind::Research => Some(Self::research(cfg)),
            WorkerKind::Build => None,
        }
    }

    /// The element-wise minimum: a caller can tighten, never widen.
    pub fn lowered(&self, other: &Budgets) -> Budgets {
        Budgets {
            wall: self.wall.min(other.wall),
            fetches: self.fetches.min(other.fetches),
            pages: self.pages.min(other.pages),
            bytes: self.bytes.min(other.bytes),
            tokens: self.tokens.min(other.tokens),
            output_bytes: self.output_bytes.min(other.output_bytes),
            fact_bytes: self.fact_bytes.min(other.fact_bytes),
            facts: self.facts.min(other.facts),
        }
    }
}

static WORKERS_SPAWNED: AtomicUsize = AtomicUsize::new(0);

/// How many worker processes this process has spawned (tests use it to
/// prove the idle path starts none).
pub fn workers_spawned() -> usize {
    WORKERS_SPAWNED.load(Ordering::SeqCst)
}

pub(crate) fn count_spawn() {
    WORKERS_SPAWNED.fetch_add(1, Ordering::SeqCst);
}

static SANDBOX_LAUNCHES: AtomicUsize = AtomicUsize::new(0);

/// How many practice-sandbox steps (compile or run) this process has
/// launched (tests use it: no card → no launch; the idle path launches
/// none).
pub fn sandbox_launches() -> usize {
    SANDBOX_LAUNCHES.load(Ordering::SeqCst)
}

pub(crate) fn count_sandbox_launch() {
    SANDBOX_LAUNCHES.fetch_add(1, Ordering::SeqCst);
}

/// 64 random bits: `/dev/urandom` where there is one, else a time/pid/seq
/// mix (circuit credentials, canaries, jitter).
pub fn random_u64() -> u64 {
    #[cfg(unix)]
    {
        use std::io::Read;
        let mut b = [0u8; 8];
        if std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut b))
            .is_ok()
        {
            return u64::from_le_bytes(b);
        }
    }
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let s = SEQ.fetch_add(1, Ordering::SeqCst);
    let h = crate::fnv1a(format!("{nanos}|{}|{s}", std::process::id()).as_bytes());
    u64::from_str_radix(&h[6..], 16).unwrap_or(nanos as u64)
}

/// A fresh random token (`[0-9a-f]{32}`).
pub fn random_token() -> String {
    format!("{:016x}{:016x}", random_u64(), random_u64())
}

/// `<ts>\tWORKER\t<op>\t<reason>\tjob=<id>` (§12).
pub fn audit_line(t: f64, op: &str, reason: &str, job: &str) -> String {
    let col = |s: &str| {
        lillith_core::launch_audit::one_column(s)
            .chars()
            .take(256)
            .collect::<String>()
    };
    format!(
        "{}\tWORKER\t{}\t{}\tjob={}\n",
        crate::audit::utc(t),
        col(op),
        col(reason),
        col(job)
    )
}
