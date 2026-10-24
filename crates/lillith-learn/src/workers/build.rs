//! App-building helper jobs ([`super::WorkerKind::Build`]): the practice
//! sandbox of docs/SELF_LEARNING.md §6 (milestone P4, D2 = A).
//!
//! A [`BuildJob`] is one tiny single-file program ([`Lang`]: Rust, Python
//! or fish) for one purpose. [`Practice`] runs it in two steps, compile
//! then run, and **each step needs its own Soul Contract card**
//! (`PendingAction::SandboxRun { argv, job, card }`):
//!
//! 1. **Static reject first** ([`super::static_check`]): `std::net`,
//!    `socket`, `Command`, `unsafe`, `extern`, `include!` and the other
//!    bypasses never reach a card.
//! 2. **Card** ([`Practice::propose`]): `PROPOSED` is audited first
//!    (fail-closed), then the card is shown through [`SandboxConsent`] (the
//!    UI implements it; [`ClickCard`] adapts the existing card process).
//!    Only [`SandboxAnswer::Click`] with [`CardAnswer::Yes`] makes a
//!    [`CardApproval`]; a typed or spoken yes never counts. The approval is
//!    single-use (consumed by value, never `Clone`, its card id recorded on
//!    the job) and bound to the job id, the step and the exact source hash:
//!    no standing approval.
//! 3. **Sandbox** ([`Practice::execute`], [`sandbox_argv`]):
//!    `bwrap --unshare-all --die-with-parent --new-session` (no network
//!    namespace), read-only `/usr`, `/proc`, `/dev`, a tmpfs `/tmp`, only
//!    `sandbox/<job>/` writable (as `/work`, root remounted read-only),
//!    `--clearenv` + a minimal env with `CARGO_NET_OFFLINE=true`, then
//!    `prlimit --as --cpu --nproc --fsize --nofile --core=0` and
//!    `timeout -k 2 <s>` from [`BuildBudgets`] around the compile or run
//!    argv; nice 19 and the idle I/O class on the whole tree; own process
//!    group, killed (SIGKILL) on any [`StopReason`], output discarded. The
//!    tmpfs `/tmp` has a hard size (`bwrap --size`), and a watchdog kills
//!    the group the moment `sandbox/<job>/` goes over its cap. A rustup
//!    toolchain is bound read-only at `/opt/rust`. No `bwrap` / `prlimit` /
//!    `timeout` → practice stays off (never a weaker sandbox, nothing
//!    installed for you). The approval is also bound to the SHA-256 of the
//!    exact argv on the card; anything else refuses to run.
//! 4. **Skill** after a successful run ([`Practice::offer_skill`]): a
//!    separate `PendingAction::WriteSkill` card (`write_skill` lines);
//!    only its click writes `<[mind] skills_dir>/<slug>.md`, through the
//!    Soul Contract's own confirm path. `[mind] enabled = false` → no card,
//!    nothing written.
//!
//! Audit: `sandbox_run` card lines (`PROPOSED` / `CONFIRM` / `DENY` /
//! `TIMEOUT` / `CANCELLED` / `NO_UI`, `card=<id>` and the argv) and
//! `WORKER` lines (`reject`, `compile`, `run`, `kill`, `skill`); the D8 job
//! log gets a `local:build-worker` entry per state. `PROPOSED`, `CONFIRM`,
//! the step's start line and the `QUEUED` / `RUNNING` job-log entries are
//! fail-closed: no line, no run. Any other `WORKER` line that can't be
//! written stops the session ([`StopReason::AuditFailed`]).
//!
//! Off by default: [`gate`] needs the online gate (`[learn] online`,
//! `enabled`, `[memory] enabled`), `[learn] practice` and Linux. The idle
//! learner has no path here.

pub use super::static_check::{Lang, Rejected};

use super::joblog::{job_id_ok, JobEntry, JobLog, JobStatus};
use super::process::SandboxEnd;
use super::{Session, StopReason, WorkerKind};
use crate::online::{self, IdentityDenylist};
use lillith_core::card_process::{CardAnswer, CardText};
use lillith_core::config::Config;
use lillith_core::soul_contract::PendingAction;
use serde_json::json;
use std::path::{Path, PathBuf};
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
    /// The whole `sandbox/<job>/` dir (watchdog) and the tmpfs `/tmp`
    /// (`bwrap --size`), each.
    pub scratch_bytes: u64,
    pub network: NetworkPolicy,
}

impl BuildBudgets {
    /// The element-wise stricter of the two (a caller can only tighten).
    pub fn lowered(&self, o: &BuildBudgets) -> BuildBudgets {
        BuildBudgets {
            compile_wall: self.compile_wall.min(o.compile_wall),
            run_wall: self.run_wall.min(o.run_wall),
            memory_bytes: self.memory_bytes.min(o.memory_bytes),
            cpu_s: self.cpu_s.min(o.cpu_s),
            nproc: self.nproc.min(o.nproc),
            fsize_bytes: self.fsize_bytes.min(o.fsize_bytes),
            scratch_bytes: self.scratch_bytes.min(o.scratch_bytes),
            network: NetworkPolicy::Denied,
        }
    }
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

/// `timeout -k <KILL_AFTER_S>` (docs §6).
pub const KILL_AFTER_S: u64 = 2;
/// Output kept per stream and step.
pub const OUTPUT_CAP: usize = 64 * 1024;
/// Largest skill file.
pub const SKILL_MAX_BYTES: usize = 8 * 1024;
/// Failed sandbox dirs kept for the panel (docs §6).
pub const KEEP_FAILED_DIRS: usize = 20;
/// … and their total size.
pub const KEEP_FAILED_BYTES: u64 = 50 * 1024 * 1024;
/// Where `sandbox/<job>/` is mounted.
pub const WORK: &str = "/work";
/// Where a rustup toolchain (outside `/usr`) is bound read-only, so no home
/// path shows inside the sandbox or in rustc's own paths.
pub const RUST_MOUNT: &str = "/opt/rust";
/// `prlimit --nofile`.
pub const NOFILE: u32 = 64;
/// Most files and folders `sandbox/<job>/` may hold (watchdog).
pub const MAX_SCRATCH_ENTRIES: u64 = 4096;

/// One of the two steps; each needs its own card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuildStep {
    Compile,
    Run,
}

impl BuildStep {
    pub fn word(self) -> &'static str {
        match self {
            BuildStep::Compile => "compile",
            BuildStep::Run => "run",
        }
    }
}

/// What the card shows and the audit records.
#[derive(Debug, Clone, PartialEq)]
pub struct SandboxCardRequest {
    pub card: String,
    pub job: String,
    pub step: BuildStep,
    pub lang: Lang,
    pub source_hash: String,
    /// The whole sandboxed argv (bwrap … prlimit … timeout … argv).
    pub argv: Vec<String>,
    /// [`argv_hash`] of `argv`: the approval is bound to it.
    pub argv_hash: String,
    /// `PendingAction::SandboxRun { argv, job, card }`.
    pub action: PendingAction,
    /// The card text (from the action's prompt).
    pub text: CardText,
}

/// How the UI answered. Only a click counts; the other variants exist so
/// a chat or voice yes can be handed in and refused on the record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxAnswer {
    /// The Soul Contract card's own answer (a mouse click on Yes / No,
    /// timeout, closed, no display).
    Click(CardAnswer),
    /// A typed chat reply: never approves.
    Typed(String),
    /// A spoken reply: never approves.
    Spoken(String),
}

/// What the skill card (`PendingAction::WriteSkill`) shows.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillCardRequest {
    pub card: String,
    pub job: String,
    /// The file it would create under `[mind] skills_dir`.
    pub path: PathBuf,
    /// The whole skill text.
    pub body: String,
    /// `PendingAction::WriteSkill { path, body }`.
    pub action: PendingAction,
    pub text: CardText,
}

/// The card UI (implemented by the app later). `stop()` turns true on any
/// [`StopReason`] (End early, the plain stop, the kill switch, …): the card
/// must close. The safeword is not one: it only hushes her voice.
pub trait SandboxConsent {
    fn ask(&self, req: &SandboxCardRequest, stop: &dyn Fn() -> bool) -> SandboxAnswer;

    /// The skill card after a successful run. A UI without one never
    /// approves (no card, no skill).
    fn ask_skill(&self, _req: &SkillCardRequest, _stop: &dyn Fn() -> bool) -> SandboxAnswer {
        SandboxAnswer::Click(CardAnswer::NoUi)
    }
}

/// The existing click card ([`online::Consent`], e.g.
/// [`online::CardProcessConsent`]) as a [`SandboxConsent`].
pub struct ClickCard<'a>(pub &'a dyn online::Consent);

impl SandboxConsent for ClickCard<'_> {
    fn ask(&self, req: &SandboxCardRequest, stop: &dyn Fn() -> bool) -> SandboxAnswer {
        SandboxAnswer::Click(self.0.ask(&req.text, stop))
    }

    fn ask_skill(&self, req: &SkillCardRequest, stop: &dyn Fn() -> bool) -> SandboxAnswer {
        SandboxAnswer::Click(self.0.ask(&req.text, stop))
    }
}

/// `sha256:<hex>` of the exact argv (NUL-separated) shown on a card.
pub fn argv_hash(argv: &[String]) -> String {
    crate::sha256::sha256_tagged(argv.join("\0").as_bytes())
}

/// Proof of one mouse click on Yes on one card, for one step of one job,
/// one exact source and the exact argv shown. Made only by
/// [`Practice::propose`]; not `Clone`, consumed by [`Practice::execute`],
/// and its card id is spent on the job.
#[derive(Debug, PartialEq, Eq)]
pub struct CardApproval {
    card: String,
    job: String,
    step: BuildStep,
    source_hash: String,
    argv_hash: String,
}

impl CardApproval {
    pub fn card(&self) -> &str {
        &self.card
    }
    pub fn job(&self) -> &str {
        &self.job
    }
    pub fn step(&self) -> BuildStep {
        self.step
    }
    pub fn source_hash(&self) -> &str {
        &self.source_hash
    }
    pub fn argv_hash(&self) -> &str {
        &self.argv_hash
    }
}

/// Why a build step did not run (or did not finish).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// No card click for this step.
    RequiresCard,
    /// The card said No / timed out / closed / had no display, or a typed
    /// or spoken yes was offered.
    NotApproved(String),
    /// The approval is for another job, another step, another source, or
    /// its card was already used.
    ApprovalMismatch(String),
    /// The static check refused the source.
    Rejected(Rejected),
    /// Run before a successful compile of this exact source.
    NotCompiled,
    /// The language's toolchain is not on this machine (never installed).
    ToolMissing(String),
    /// A stop reason: card cancelled or process group killed, output
    /// discarded.
    Stopped(StopReason),
    /// The audit log can't be written: fail-closed.
    Audit(String),
    /// The sandbox could not be set up or started.
    Io(String),
    /// `[mind] enabled = false` (or no skills folder): no skill card,
    /// nothing written.
    SkillsOff,
    /// A skill needs a successful run of this exact source first.
    NoSuccessfulRun,
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::RequiresCard => f.write_str("this step needs its own card click"),
            BuildError::NotApproved(w) => write!(f, "not approved: {w}"),
            BuildError::ApprovalMismatch(w) => write!(f, "approval refused: {w}"),
            BuildError::Rejected(r) => write!(f, "rejected before compile: {r}"),
            BuildError::NotCompiled => f.write_str("run needs a successful compile first"),
            BuildError::ToolMissing(t) => {
                write!(f, "{t} is not installed (never installed for you)")
            }
            BuildError::Stopped(r) => write!(f, "stopped ({}); output discarded", r.word()),
            BuildError::Audit(e) => write!(f, "the audit log can't be written ({e}); nothing runs"),
            BuildError::Io(e) => write!(f, "sandbox error: {e}"),
            BuildError::SkillsOff => f.write_str("skills are off ([mind] enabled = false)"),
            BuildError::NoSuccessfulRun => {
                f.write_str("a skill needs a successful run of this exact source")
            }
        }
    }
}

/// A sandboxed build job: one source file for one purpose.
#[derive(Debug)]
pub struct BuildJob {
    pub id: String,
    /// What Ko asked for.
    pub purpose: String,
    pub lang: Lang,
    source: String,
    /// Private: [`tighten_budgets`](Self::tighten_budgets) only lowers them,
    /// and the card's argv hash pins what runs.
    budgets: BuildBudgets,
    needs_card_per_run: bool,
    /// Card ids already spent on this job (no replay).
    used_cards: Vec<String>,
    /// The source hash of the last successful compile.
    compiled: Option<String>,
    /// The source hash and stdout of the last successful run (what a skill
    /// card offers).
    ran: Option<(String, String)>,
}

impl BuildJob {
    pub fn new(id: &str, purpose: &str, lang: Lang, source: &str) -> Self {
        Self {
            id: id.to_string(),
            purpose: purpose.split_whitespace().collect::<Vec<_>>().join(" "),
            lang,
            source: source.to_string(),
            budgets: BuildBudgets::default(),
            needs_card_per_run: true,
            used_cards: Vec::new(),
            compiled: None,
            ran: None,
        }
    }

    pub fn budgets(&self) -> &BuildBudgets {
        &self.budgets
    }

    /// Lowers the budgets (never raises them).
    pub fn tighten_budgets(&mut self, b: &BuildBudgets) {
        self.budgets = self.budgets.lowered(b);
    }

    /// Always `true` (no standing approval, D2 = A).
    pub fn needs_card_per_run(&self) -> bool {
        self.needs_card_per_run
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// `sha256:<hex>` of the language and the exact source.
    pub fn source_hash(&self) -> String {
        let mut b = self.lang.word().as_bytes().to_vec();
        b.push(0);
        b.extend_from_slice(self.source.as_bytes());
        crate::sha256::sha256_tagged(&b)
    }

    /// New source: every earlier approval, the compile and the run are
    /// void.
    pub fn revise(&mut self, source: &str) {
        self.source = source.to_string();
        self.compiled = None;
        self.ran = None;
    }

    /// Whether this exact source compiled.
    pub fn compiled(&self) -> bool {
        self.compiled.as_deref() == Some(self.source_hash().as_str())
    }
}

/// Why practice is off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PracticeOff {
    /// The online gate (`[learn] online` is off by default).
    Gate(online::Off),
    /// `[learn] practice = false`.
    PracticeOff,
    /// Windows / other: practice is disabled (docs §6).
    NotLinux,
    /// `bwrap`, `prlimit` or `timeout` is missing.
    Missing(String),
    /// No data dir for `sandbox/`.
    NoDataDir,
    /// `sandbox/` is a symlink (it must be a real folder).
    SymlinkRoot,
}

impl std::fmt::Display for PracticeOff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PracticeOff::Gate(o) => write!(f, "practice stays off: {o}"),
            PracticeOff::PracticeOff => {
                f.write_str("practice stays off ([learn] practice = false)")
            }
            PracticeOff::NotLinux => f.write_str("practice is Linux-only (docs §6)"),
            PracticeOff::Missing(t) => write!(
                f,
                "practice stays off: {t} is not installed. It is never installed for you \
                 and nothing runs without it (never a weaker sandbox)"
            ),
            PracticeOff::NoDataDir => f.write_str("practice stays off: no data folder"),
            PracticeOff::SymlinkRoot => {
                f.write_str("practice stays off: the sandbox folder is a symlink")
            }
        }
    }
}

/// `Ok` when the config allows practice at all: the online gate (default
/// off), `[learn] practice`, Linux.
pub fn gate(cfg: &Config) -> Result<(), PracticeOff> {
    online::gate(cfg).map_err(PracticeOff::Gate)?;
    if !cfg.learn.practice {
        return Err(PracticeOff::PracticeOff);
    }
    if !cfg!(target_os = "linux") {
        return Err(PracticeOff::NotLinux);
    }
    Ok(())
}

/// A rustc toolchain usable inside the sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustToolchain {
    /// `<sysroot>/bin/rustc` (not a rustup shim).
    pub rustc: PathBuf,
    /// Bound read-only when it is outside `/usr`.
    pub sysroot: PathBuf,
    /// The C linker, resolved to its real path under `/usr`.
    pub linker: PathBuf,
}

/// The programs practice needs, found at fixed system paths (never on
/// `$PATH`, never installed).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FoundTools {
    pub bwrap: Option<PathBuf>,
    pub prlimit: Option<PathBuf>,
    pub timeout: Option<PathBuf>,
    pub rust: Option<RustToolchain>,
    pub python3: Option<PathBuf>,
    pub fish: Option<PathBuf>,
}

fn in_usr(names: &[&str]) -> Option<PathBuf> {
    names
        .iter()
        .flat_map(|n| [format!("/usr/bin/{n}"), format!("/usr/local/bin/{n}")])
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

impl FoundTools {
    /// This machine's tools. `prlimit`, `timeout` and the interpreters
    /// must live under `/usr` (the only system tree inside the sandbox).
    pub fn detect() -> Self {
        Self {
            bwrap: super::process::find_bwrap(),
            prlimit: in_usr(&["prlimit"]),
            timeout: in_usr(&["timeout"]),
            rust: find_rust(),
            python3: in_usr(&["python3"]),
            fish: in_usr(&["fish"]),
        }
    }
}

/// A rustc whose sysroot can be bound: a real `/usr/bin/rustc`, or a
/// rustup toolchain (`stable-*` first). The linker is `cc` resolved to its
/// real file under `/usr` (`/etc/alternatives` is not in the sandbox).
fn find_rust() -> Option<RustToolchain> {
    let linker = std::fs::canonicalize("/usr/bin/cc")
        .ok()
        .filter(|p| p.starts_with("/usr/"))?;
    let usr = Path::new("/usr/bin/rustc");
    if let Ok(real) = std::fs::canonicalize(usr) {
        if real.starts_with("/usr/") && real.file_name().is_some_and(|n| n == "rustc") {
            return Some(RustToolchain {
                rustc: real,
                sysroot: PathBuf::from("/usr"),
                linker,
            });
        }
    }
    let home = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".rustup")))?;
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(home.join("toolchains"))
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("bin/rustc").is_file())
        .collect();
    dirs.sort();
    let pick = dirs
        .iter()
        .find(|d| {
            d.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("stable-"))
        })
        .or_else(|| dirs.last())?
        .clone();
    Some(RustToolchain {
        rustc: pick.join("bin/rustc"),
        sysroot: pick,
        linker,
    })
}

/// The tools a ready sandbox has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxTools {
    pub bwrap: PathBuf,
    pub prlimit: PathBuf,
    pub timeout: PathBuf,
    pub rust: Option<RustToolchain>,
    pub python3: Option<PathBuf>,
    pub fish: Option<PathBuf>,
}

impl SandboxTools {
    /// Refuses (never degrades the sandbox) without `bwrap`, `prlimit` or
    /// `timeout`.
    pub fn from_found(f: FoundTools) -> Result<Self, PracticeOff> {
        let bwrap = f.bwrap.ok_or(PracticeOff::Missing("bwrap".into()))?;
        let prlimit = f
            .prlimit
            .filter(|p| p.starts_with("/usr/"))
            .ok_or(PracticeOff::Missing("prlimit".into()))?;
        let timeout = f
            .timeout
            .filter(|p| p.starts_with("/usr/"))
            .ok_or(PracticeOff::Missing("timeout".into()))?;
        Ok(Self {
            bwrap,
            prlimit,
            timeout,
            rust: f.rust,
            python3: f.python3,
            fish: f.fish,
        })
    }
}

fn s(x: impl AsRef<str>) -> String {
    x.as_ref().to_string()
}

fn p(x: &Path) -> String {
    x.to_string_lossy().into_owned()
}

/// A `prlimit` size in bytes (`prlimit` takes no `512M` suffix, so the
/// doc's `--as=512M` is written `--as=536870912`).
fn size_arg(b: u64) -> String {
    b.to_string()
}

/// The `prlimit --fsize` actually used: the budget's, but never more than a
/// quarter of the scratch cap (review N3-3), so one file written between
/// two watchdog polls can't push `/work` far past its cap.
pub(crate) fn effective_fsize(b: &BuildBudgets) -> u64 {
    b.fsize_bytes.min((b.scratch_bytes / 4).max(1))
}

/// The compile-or-run argv inside `/work` (docs §6 toolchain), or which
/// tool is missing.
pub fn step_argv(tools: &SandboxTools, lang: Lang, step: BuildStep) -> Result<Vec<String>, String> {
    Ok(match (lang, step) {
        (Lang::Rust, BuildStep::Compile) => {
            let r = tools.rust.as_ref().ok_or("rustc (and cc)")?;
            vec![
                p(&rustc_inside(r)),
                s("--edition"),
                s("2021"),
                s("-C"),
                s("opt-level=0"),
                s("-C"),
                format!("linker={}", p(&r.linker)),
                s("main.rs"),
            ]
        }
        (Lang::Rust, BuildStep::Run) => vec![s("./main")],
        (Lang::Python, BuildStep::Compile) => {
            let py = tools.python3.as_ref().ok_or("python3")?;
            vec![
                p(py),
                s("-I"),
                s("-S"),
                s("-m"),
                s("py_compile"),
                s("main.py"),
            ]
        }
        (Lang::Python, BuildStep::Run) => {
            let py = tools.python3.as_ref().ok_or("python3")?;
            vec![p(py), s("-I"), s("-S"), s("main.py")]
        }
        (Lang::Fish, BuildStep::Compile) => {
            let f = tools.fish.as_ref().ok_or("fish")?;
            vec![p(f), s("--no-config"), s("-n"), s("main.fish")]
        }
        (Lang::Fish, BuildStep::Run) => {
            let f = tools.fish.as_ref().ok_or("fish")?;
            vec![p(f), s("--no-config"), s("main.fish")]
        }
    })
}

/// rustc as the sandbox sees it: a `/usr` one as is, a rustup one under
/// [`RUST_MOUNT`].
fn rustc_inside(r: &RustToolchain) -> PathBuf {
    if r.sysroot.starts_with("/usr") {
        return r.rustc.clone();
    }
    match r.rustc.strip_prefix(&r.sysroot) {
        Ok(rel) => Path::new(RUST_MOUNT).join(rel),
        Err(_) => Path::new(RUST_MOUNT).join("bin/rustc"),
    }
}

/// The command a skill records: the doc's toolchain line, no local paths.
pub fn skill_command(lang: Lang) -> &'static str {
    match lang {
        Lang::Rust => "rustc --edition 2021 -C opt-level=0 main.rs && ./main",
        Lang::Python => "python3 -I -S main.py",
        Lang::Fish => "fish -n main.fish && fish main.fish",
    }
}

/// The whole sandboxed argv (docs §6), with `work` bound as `/work`.
pub fn sandbox_argv(
    tools: &SandboxTools,
    work: &Path,
    budgets: &BuildBudgets,
    step: BuildStep,
    inner: &[String],
    extra_ro: Option<&Path>,
) -> Vec<String> {
    let mut a: Vec<String> = vec![
        p(&tools.bwrap),
        s("--unshare-all"),
        s("--die-with-parent"),
        s("--new-session"),
    ];
    for x in [
        ["--ro-bind", "/usr", "/usr"],
        ["--symlink", "usr/lib", "/lib"],
        ["--symlink", "usr/lib64", "/lib64"],
        ["--symlink", "usr/bin", "/bin"],
    ] {
        a.extend(x.iter().map(s));
    }
    a.extend(["--proc", "/proc", "--dev", "/dev"].map(s));
    // `--size` applies to the next tmpfs: `/tmp` gets the scratch cap.
    a.extend([
        s("--size"),
        budgets.scratch_bytes.max(1).to_string(),
        s("--tmpfs"),
        s("/tmp"),
    ]);
    a.extend([s("--bind"), p(work), s(WORK), s("--chdir"), s(WORK)]);
    // A rustup sysroot goes to a neutral path (no home dir inside).
    if let Some(ro) = extra_ro.filter(|r| !r.starts_with("/usr")) {
        a.extend([s("--ro-bind"), p(ro), s(RUST_MOUNT)]);
    }
    // Nothing but /work (and the tmpfs /tmp) is writable.
    a.extend(["--remount-ro", "/"].map(s));
    // `--clearenv` must come first: bwrap applies env options in order.
    a.push(s("--clearenv"));
    for (k, v) in [
        ("CARGO_NET_OFFLINE", "true"),
        ("PATH", "/usr/bin:/bin"),
        ("HOME", WORK),
        ("TMPDIR", "/tmp"),
        ("LANG", "C"),
        ("TZ", "UTC"),
        // One malloc arena: rustc's per-thread 64 MiB arenas would not fit
        // the doc's 512 MiB `--as` cap.
        ("MALLOC_ARENA_MAX", "1"),
    ] {
        a.extend([s("--setenv"), s(k), s(v)]);
    }
    let wall = match step {
        BuildStep::Compile => budgets.compile_wall,
        BuildStep::Run => budgets.run_wall,
    };
    a.extend([
        s("--"),
        p(&tools.prlimit),
        format!("--as={}", size_arg(budgets.memory_bytes)),
        format!("--cpu={}", budgets.cpu_s),
        format!("--nproc={}", budgets.nproc),
        format!("--fsize={}", size_arg(effective_fsize(budgets))),
        format!("--nofile={NOFILE}"),
        s("--core=0"),
        p(&tools.timeout),
        s("-k"),
        KILL_AFTER_S.to_string(),
        wall.as_secs().max(1).to_string(),
    ]);
    a.extend(inner.iter().cloned());
    a
}

/// What one step produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepOutput {
    pub step: BuildStep,
    pub ok: bool,
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
    /// Why it failed (timeout, limit, exit code), if it did.
    pub failure: Option<String>,
    /// The skill written after a successful run.
    pub skill: Option<PathBuf>,
}

/// Compile + run + skill, one card each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PracticeReport {
    pub compile: Option<StepOutput>,
    pub run: Option<StepOutput>,
    pub skill: Option<PathBuf>,
    pub error: Option<BuildError>,
}

/// The practice pipeline. Exists only when [`gate`] is open and the
/// sandbox tools are present.
#[derive(Debug, Clone)]
pub struct Practice {
    tools: SandboxTools,
    sandbox_root: PathBuf,
    /// `[mind] skills_dir` (resolved); `None` with `[mind] enabled = false`.
    skills_dir: Option<PathBuf>,
    audit_log: Option<PathBuf>,
    job_log: JobLog,
    deny: IdentityDenylist,
}

impl Practice {
    /// The real pipeline: config gate, this machine's tools, the data dir.
    pub fn new(cfg: &Config) -> Result<Self, PracticeOff> {
        Self::with_tools(cfg, FoundTools::detect())
    }

    /// As [`new`](Self::new) with given tools (tests: a missing `bwrap`).
    pub fn with_tools(cfg: &Config, found: FoundTools) -> Result<Self, PracticeOff> {
        gate(cfg)?;
        let tools = SandboxTools::from_found(found)?;
        let data = lillith_core::config::data_dir().ok_or(PracticeOff::NoDataDir)?;
        let sandbox_root = data.join("sandbox");
        refuse_symlink(&sandbox_root).map_err(|_| PracticeOff::SymlinkRoot)?;
        Ok(Self {
            tools,
            sandbox_root,
            skills_dir: skills_dir_of(cfg),
            audit_log: lillith_core::config::audit_log_path(),
            job_log: JobLog::new(JobLog::default_dir()),
            deny: IdentityDenylist::from_local(),
        })
    }

    /// Tests only: `sandbox/` (each job gets `sandbox/<job>/`).
    #[cfg(test)]
    pub(crate) fn with_sandbox_root(mut self, d: PathBuf) -> Self {
        self.sandbox_root = d;
        self
    }
    /// Tests only: production uses `[mind] skills_dir`.
    #[cfg(test)]
    pub(crate) fn with_skills_dir(mut self, d: Option<PathBuf>) -> Self {
        self.skills_dir = d;
        self
    }
    /// Tests only: production always audits to the real log.
    #[cfg(test)]
    pub(crate) fn with_audit_log(mut self, l: Option<PathBuf>) -> Self {
        self.audit_log = l;
        self
    }
    /// Tests only: production always keeps the D8 job log.
    #[cfg(test)]
    pub(crate) fn with_job_log(mut self, d: Option<PathBuf>) -> Self {
        self.job_log = JobLog::new(d);
        self
    }

    /// Where an approved skill would go (`None`: `[mind]` is off).
    pub fn skills_dir(&self) -> Option<&Path> {
        self.skills_dir.as_deref()
    }
    /// More identifiers to keep out of skills (names from memory).
    pub fn with_denylist(mut self, d: IdentityDenylist) -> Self {
        self.deny = self.deny.merged(d);
        self
    }

    pub fn tools(&self) -> &SandboxTools {
        &self.tools
    }

    /// `sandbox/<job>/`.
    pub fn work_dir(&self, job: &str) -> PathBuf {
        self.sandbox_root.join(job)
    }

    /// The argv a step of `job` would run.
    pub fn argv(&self, job: &BuildJob, step: BuildStep) -> Result<Vec<String>, BuildError> {
        let inner = step_argv(&self.tools, job.lang, step).map_err(BuildError::ToolMissing)?;
        let extra = match job.lang {
            Lang::Rust => self.tools.rust.as_ref().map(|r| r.sysroot.as_path()),
            _ => None,
        };
        Ok(sandbox_argv(
            &self.tools,
            &self.work_dir(&job.id),
            &job.budgets,
            step,
            &inner,
            extra,
        ))
    }

    /// One `WORKER` line under the session's card. Fail-closed: a failed
    /// write stops the session ([`StopReason::AuditFailed`]) and returns
    /// `false`.
    fn worker_audit(&self, session: &Session, op: &str, reason: &str, job: &str) -> bool {
        match crate::audit::record_worker(
            self.audit_log.as_deref(),
            op,
            reason,
            job,
            Some(session.card()),
        ) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("build audit ({op}): {e}; stopping the session");
                session.request_stop(StopReason::AuditFailed);
                false
            }
        }
    }

    fn card_audit(&self, state: &str, card: &str, fields: &[String]) {
        if let Err(e) = crate::audit::record_sandbox(self.audit_log.as_deref(), state, card, fields)
        {
            tracing::warn!("sandbox card audit ({state}): {e}");
        }
    }

    fn d8(
        &self,
        job: &BuildJob,
        step: BuildStep,
        status: JobStatus,
        session: &Session,
        result: Option<&Path>,
    ) -> bool {
        let e = JobEntry {
            id: job.id.clone(),
            bot: WorkerKind::Build.bot(),
            task: json!({
                "kind": "build",
                "lang": job.lang.word(),
                "step": step.word(),
                "purpose": job.purpose,
                "hash": job.source_hash(),
            }),
            status,
            ts: crate::audit::utc(crate::now_s()),
            result_path: result.map(|p| p.display().to_string()),
            backend: "local".into(),
            session: session.id().to_string(),
            attempt: 0,
        };
        match self.job_log.append(&e) {
            Ok(()) => true,
            Err(err) => {
                tracing::warn!("build job log: {err}");
                false
            }
        }
    }

    /// Static check → `PROPOSED` → the step's card. Returns the single-use
    /// approval only for a mouse click on Yes with no stop reason.
    pub fn propose(
        &self,
        job: &BuildJob,
        step: BuildStep,
        consent: &dyn SandboxConsent,
        session: &Session,
    ) -> Result<CardApproval, BuildError> {
        if !job_id_ok(&job.id) {
            return Err(BuildError::Io(format!("bad job id {:?}", job.id)));
        }
        if let Some(r) = session.stop_reason() {
            return Err(BuildError::Stopped(r));
        }
        // 1. Static reject before anything else (docs §6).
        if let Err(r) = super::static_check::check(job.lang, &job.source) {
            self.worker_audit(session, "reject", &r.to_string(), &job.id);
            self.d8(job, step, JobStatus::Refused, session, None);
            return Err(BuildError::Rejected(r));
        }
        if step == BuildStep::Run && !job.compiled() {
            return Err(BuildError::NotCompiled);
        }
        let argv = self.argv(job, step)?;
        let card = format!("sbx-{}", &super::random_token()[..16]);
        let hash = job.source_hash();
        let fields = |extra: &[String]| -> Vec<String> {
            let mut f = vec![
                format!("job={}", job.id),
                format!("step={}", step.word()),
                format!("hash={hash}"),
            ];
            f.extend(extra.iter().cloned());
            f
        };
        // 2. PROPOSED first; fail-closed.
        crate::audit::record_sandbox(
            self.audit_log.as_deref(),
            "PROPOSED",
            &card,
            &fields(&[
                format!("lang={}", job.lang.word()),
                format!("argv={}", argv.join(" ")),
            ]),
        )
        .map_err(|e| BuildError::Audit(e.to_string()))?;
        // The D8 job log is fail-closed too.
        if !self.d8(job, step, JobStatus::Queued, session, None) {
            self.card_audit("CANCELLED", &card, &fields(&["why=job_log".into()]));
            return Err(BuildError::Audit(
                "the D8 job log could not be written".into(),
            ));
        }
        let ahash = argv_hash(&argv);
        let action = PendingAction::SandboxRun {
            argv: argv.clone(),
            job: job.id.clone(),
            card: card.clone(),
        };
        let pr = action.prompt();
        let req = SandboxCardRequest {
            card: card.clone(),
            job: job.id.clone(),
            step,
            lang: job.lang,
            source_hash: hash.clone(),
            argv,
            argv_hash: ahash.clone(),
            text: CardText {
                title: pr.title.to_string(),
                summary: pr.summary,
                target: String::new(),
                detail: pr.detail.unwrap_or_default(),
            },
            action,
        };
        // 3. The card; any stop reason closes it.
        let stop = || session.stop_reason().is_some();
        let answer = consent.ask(&req, &stop);
        let stopped = session.stop_reason();
        let (state, why) = match (&answer, stopped) {
            (_, Some(r)) => {
                self.card_audit("CANCELLED", &card, &fields(&[format!("why={}", r.word())]));
                self.d8(job, step, JobStatus::Cancelled, session, None);
                return Err(BuildError::Stopped(r));
            }
            (SandboxAnswer::Click(CardAnswer::Yes), None) => {
                // CONFIRM is fail-closed: no line, no approval.
                crate::audit::record_sandbox(
                    self.audit_log.as_deref(),
                    "CONFIRM",
                    &card,
                    &fields(&[format!("argv_hash={ahash}")]),
                )
                .map_err(|e| BuildError::Audit(e.to_string()))?;
                return Ok(CardApproval {
                    card,
                    job: job.id.clone(),
                    step,
                    source_hash: hash,
                    argv_hash: ahash,
                });
            }
            (SandboxAnswer::Click(CardAnswer::No), _) => ("DENY", "declined on the card"),
            (SandboxAnswer::Click(CardAnswer::TimedOut), _) => ("TIMEOUT", "no answer on the card"),
            (SandboxAnswer::Click(CardAnswer::Cancelled), _) => {
                ("CANCELLED", "the card was closed")
            }
            (SandboxAnswer::Click(CardAnswer::NoUi), _) => (
                "NO_UI",
                "no display for the card; a typed or spoken yes never counts",
            ),
            (SandboxAnswer::Typed(_), _) => ("DENY", "a typed yes never counts; only a click"),
            (SandboxAnswer::Spoken(_), _) => ("DENY", "a spoken yes never counts; only a click"),
        };
        self.card_audit(state, &card, &fields(&[format!("why={why}")]));
        self.d8(job, step, JobStatus::Refused, session, None);
        Err(BuildError::NotApproved(why.to_string()))
    }

    /// Runs the one step `approval` was clicked for. The approval is spent
    /// whatever happens next.
    pub fn execute(
        &self,
        job: &mut BuildJob,
        approval: CardApproval,
        session: &Session,
    ) -> Result<StepOutput, BuildError> {
        let step = approval.step;
        let jid = job.id.clone();
        let refuse = |why: String| {
            self.worker_audit(session, "refused", &why, &jid);
            Err(BuildError::ApprovalMismatch(why))
        };
        if approval.job != job.id {
            return refuse(format!(
                "card {} is for job {}",
                approval.card, approval.job
            ));
        }
        if job.used_cards.contains(&approval.card) {
            return refuse(format!("card {} was already used", approval.card));
        }
        job.used_cards.push(approval.card.clone());
        if approval.source_hash != job.source_hash() {
            return refuse(format!(
                "card {} approved another source ({})",
                approval.card, approval.source_hash
            ));
        }
        if let Err(r) = super::static_check::check(job.lang, &job.source) {
            self.worker_audit(session, "reject", &r.to_string(), &job.id);
            return Err(BuildError::Rejected(r));
        }
        if step == BuildStep::Run && !job.compiled() {
            return Err(BuildError::NotCompiled);
        }
        if let Some(r) = session.stop_reason() {
            self.card_audit(
                "CANCELLED",
                &approval.card,
                &[format!("job={}", job.id), format!("why={}", r.word())],
            );
            return Err(BuildError::Stopped(r));
        }
        // The exact argv on the card, or nothing: budgets, the sandbox root
        // or the tools can't change after the click.
        let argv = self.argv(job, step)?;
        if argv_hash(&argv) != approval.argv_hash {
            return refuse(format!(
                "card {} approved another command line",
                approval.card
            ));
        }
        let work = self.work_dir(&job.id);
        self.prepare(job, step, &work)
            .map_err(|e| BuildError::Io(e.to_string()))?;
        // The start line and the RUNNING entry are fail-closed.
        if !self.worker_audit(
            session,
            step.word(),
            &format!("card={} start", approval.card),
            &job.id,
        ) || !self.d8(job, step, JobStatus::Running, session, None)
        {
            session.request_stop(StopReason::AuditFailed);
            self.keep_failed(&work);
            return Err(BuildError::Audit(
                "the start line or the D8 job log could not be written".into(),
            ));
        }
        let wall = match step {
            BuildStep::Compile => job.budgets.compile_wall,
            BuildStep::Run => job.budgets.run_wall,
        } + Duration::from_secs(KILL_AFTER_S + 3);
        let stop = || session.stop_reason();
        let cap = job.budgets.scratch_bytes;
        let over_cap = || {
            let (bytes, entries) = dir_usage(&work);
            bytes > cap || entries > MAX_SCRATCH_ENTRIES
        };
        let end =
            super::process::run_sandboxed_watched(&argv, &work, OUTPUT_CAP, wall, &stop, &over_cap);
        let end = match end {
            Ok(e) => e,
            Err(e) => {
                self.worker_audit(session, step.word(), &format!("spawn failed: {e}"), &job.id);
                self.d8(job, step, JobStatus::Failed, session, None);
                self.keep_failed(&work);
                return Err(BuildError::Io(e.to_string()));
            }
        };
        match end {
            SandboxEnd::Stopped(r) => {
                // Killed; nothing it wrote is kept.
                let _ = std::fs::remove_dir_all(&work);
                self.worker_audit(session, "kill", r.word(), &job.id);
                self.card_audit(
                    "CANCELLED",
                    &approval.card,
                    &[format!("job={}", job.id), format!("why={}", r.word())],
                );
                self.d8(job, step, JobStatus::Cancelled, session, None);
                job.compiled = None;
                Err(BuildError::Stopped(r))
            }
            SandboxEnd::WallKilled | SandboxEnd::OverCap => {
                let why = if matches!(end, SandboxEnd::OverCap) {
                    "the sandbox folder went over its size cap"
                } else {
                    "killed at the wall clock"
                };
                self.worker_audit(session, "kill", why, &job.id);
                self.d8(job, step, JobStatus::Killed, session, None);
                self.keep_failed(&work);
                job.compiled = None;
                Ok(StepOutput {
                    step,
                    ok: false,
                    code: None,
                    signal: None,
                    stdout: String::new(),
                    stderr: String::new(),
                    truncated: false,
                    failure: Some(why.into()),
                    skill: None,
                })
            }
            SandboxEnd::Exited {
                code,
                signal,
                stdout,
                stderr,
                truncated,
            } => {
                let mut failure = failure_of(code, signal);
                if failure.is_none() && over_cap() {
                    failure = Some("the sandbox folder went over its size cap".into());
                }
                let mut out = StepOutput {
                    step,
                    ok: failure.is_none(),
                    code,
                    signal,
                    stdout,
                    stderr,
                    truncated,
                    failure,
                    skill: None,
                };
                let reason = match &out.failure {
                    None => "ok".to_string(),
                    Some(f) => f.clone(),
                };
                self.worker_audit(session, step.word(), &reason, &job.id);
                if !out.ok {
                    if out
                        .failure
                        .as_deref()
                        .is_some_and(|f| f.contains("limit") || f.contains("timeout"))
                    {
                        self.worker_audit(session, "kill", &reason, &job.id);
                    }
                    self.d8(job, step, JobStatus::Failed, session, None);
                    self.keep_failed(&work);
                    return Ok(out);
                }
                match step {
                    BuildStep::Compile => {
                        job.compiled = Some(job.source_hash());
                        self.d8(job, step, JobStatus::Running, session, None);
                    }
                    BuildStep::Run => {
                        // No skill here: it needs its own card
                        // ([`Practice::offer_skill`]).
                        job.ran = Some((job.source_hash(), out.stdout.clone()));
                        self.d8(job, step, JobStatus::Done, session, None);
                        let _ = std::fs::remove_dir_all(&work);
                        let _ = std::fs::remove_file(self.failed_marker(&job.id));
                    }
                }
                out.skill = None;
                Ok(out)
            }
        }
    }

    /// The skill card after a successful run of this exact source: a
    /// `PendingAction::WriteSkill` (`write_skill` lines, `PROPOSED` and
    /// `CONFIRM` fail-closed); only a mouse click on Yes writes it, through
    /// the Soul Contract's confirm. `[mind]` off → [`BuildError::SkillsOff`],
    /// no card, nothing written.
    pub fn offer_skill(
        &self,
        job: &BuildJob,
        consent: &dyn SandboxConsent,
        session: &Session,
    ) -> Result<PathBuf, BuildError> {
        let Some(dir) = self.skills_dir.clone() else {
            return Err(BuildError::SkillsOff);
        };
        if let Some(r) = session.stop_reason() {
            return Err(BuildError::Stopped(r));
        }
        let hash = job.source_hash();
        let stdout = match &job.ran {
            Some((h, out)) if *h == hash => out.clone(),
            _ => return Err(BuildError::NoSuccessfulRun),
        };
        let body = skill_body(job, &stdout, &self.deny, &self.local_paths(job)).map_err(|w| {
            self.worker_audit(session, "skill", &format!("not offered: {w}"), &job.id);
            BuildError::Io(w)
        })?;
        let path = free_skill_path(&dir, &slug(&job.purpose))
            .ok_or_else(|| BuildError::Io("too many skills with this name".into()))?;
        let action = PendingAction::WriteSkill {
            path: path.clone(),
            body: body.clone(),
        };
        let kind = action.kind();
        let card = format!("skill-{}", &super::random_token()[..16]);
        let fields = |extra: &[String]| -> Vec<String> {
            let mut f = vec![format!("job={}", job.id), format!("hash={hash}")];
            f.extend(extra.iter().cloned());
            f
        };
        let audit = |state: &str, extra: &[String]| {
            crate::audit::record_card(
                self.audit_log.as_deref(),
                state,
                kind,
                &card,
                &fields(extra),
            )
        };
        audit(
            "PROPOSED",
            &[
                format!("path={}", path.display()),
                format!("bytes={}", body.len()),
            ],
        )
        .map_err(|e| BuildError::Audit(e.to_string()))?;
        let pr = action.prompt();
        let req = SkillCardRequest {
            card: card.clone(),
            job: job.id.clone(),
            path: path.clone(),
            body,
            text: CardText {
                title: pr.title.to_string(),
                summary: pr.summary,
                target: path.display().to_string(),
                detail: pr.detail.unwrap_or_default(),
            },
            action: action.clone(),
        };
        let stop = || session.stop_reason().is_some();
        let answer = consent.ask_skill(&req, &stop);
        let why = match (&answer, session.stop_reason()) {
            (_, Some(r)) => {
                let _ = audit("CANCELLED", &[format!("why={}", r.word())]);
                return Err(BuildError::Stopped(r));
            }
            (SandboxAnswer::Click(CardAnswer::Yes), None) => None,
            (SandboxAnswer::Click(CardAnswer::No), _) => Some(("DENY", "declined on the card")),
            (SandboxAnswer::Click(CardAnswer::TimedOut), _) => {
                Some(("TIMEOUT", "no answer on the card"))
            }
            (SandboxAnswer::Click(CardAnswer::Cancelled), _) => {
                Some(("CANCELLED", "the card was closed"))
            }
            (SandboxAnswer::Click(CardAnswer::NoUi), _) => Some((
                "NO_UI",
                "no display for the card; a typed or spoken yes never counts",
            )),
            (SandboxAnswer::Typed(_), _) => {
                Some(("DENY", "a typed yes never counts; only a click"))
            }
            (SandboxAnswer::Spoken(_), _) => {
                Some(("DENY", "a spoken yes never counts; only a click"))
            }
        };
        if let Some((state, why)) = why {
            let _ = audit(state, &[format!("why={why}")]);
            return Err(BuildError::NotApproved(why.to_string()));
        }
        audit("CONFIRM", &[]).map_err(|e| BuildError::Audit(e.to_string()))?;
        mkdir_private(&dir).map_err(|e| BuildError::Io(e.to_string()))?;
        let mut contract = lillith_core::soul_contract::SoulContract::new(self.audit_log.clone());
        let id = contract.request(action);
        // Never overwrite: the write itself refuses an existing file (no
        // exists()-then-write race; review N3-4).
        match contract.confirm_new_file(id) {
            Ok(_) => {
                self.worker_audit(session, "skill", &format!("written card={card}"), &job.id);
                Ok(path)
            }
            Err(lillith_core::soul_contract::ContractError::Io(e))
                if e.kind() == std::io::ErrorKind::AlreadyExists =>
            {
                let why = format!("{} appeared meanwhile; not replaced", path.display());
                self.worker_audit(session, "skill", &format!("not written: {why}"), &job.id);
                Err(BuildError::Io(why))
            }
            Err(e) => {
                self.worker_audit(session, "skill", &format!("not written: {e}"), &job.id);
                Err(BuildError::Io(e.to_string()))
            }
        }
    }

    /// Compile + run, each with its own card, then the skill card.
    pub fn practice(
        &self,
        job: &mut BuildJob,
        consent: &dyn SandboxConsent,
        session: &Session,
    ) -> PracticeReport {
        let mut rep = PracticeReport {
            compile: None,
            run: None,
            skill: None,
            error: None,
        };
        for step in [BuildStep::Compile, BuildStep::Run] {
            let out = self
                .propose(job, step, consent, session)
                .and_then(|a| self.execute(job, a, session));
            match out {
                Ok(o) => {
                    let ok = o.ok;
                    rep.skill = o.skill.clone().or(rep.skill.take());
                    match step {
                        BuildStep::Compile => rep.compile = Some(o),
                        BuildStep::Run => rep.run = Some(o),
                    }
                    if !ok {
                        break;
                    }
                }
                Err(e) => {
                    rep.error = Some(e);
                    return rep;
                }
            }
        }
        if rep.run.as_ref().is_some_and(|r| r.ok) && self.skills_dir.is_some() {
            match self.offer_skill(job, consent, session) {
                Ok(p) => {
                    if let Some(r) = rep.run.as_mut() {
                        r.skill = Some(p.clone());
                    }
                    rep.skill = Some(p);
                }
                Err(e) => rep.error = Some(e),
            }
        }
        rep
    }

    /// Compile: a fresh `sandbox/<job>/` (0700) with the source. Run: the
    /// compiled dir, its source unchanged.
    fn prepare(&self, job: &BuildJob, step: BuildStep, work: &Path) -> std::io::Result<()> {
        let file = work.join(job.lang.file());
        match step {
            BuildStep::Compile => {
                refuse_symlink(&self.sandbox_root)?;
                let _ = std::fs::remove_file(self.failed_marker(&job.id));
                let _ = std::fs::remove_dir_all(work);
                mkdir_private(&self.sandbox_root)?;
                refuse_symlink(&self.sandbox_root)?;
                mkdir_private(work)?;
                refuse_symlink(work)?;
                std::fs::write(&file, job.source.as_bytes())
            }
            BuildStep::Run => {
                refuse_symlink(&self.sandbox_root)?;
                refuse_symlink(work)?;
                let on_disk = std::fs::read(&file)?;
                if on_disk != job.source.as_bytes() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "the source in the sandbox changed since the compile",
                    ));
                }
                Ok(())
            }
        }
    }

    /// `sandbox/<job>.failed`: the mark that lets [`prune_sandbox`] delete
    /// a dir (outside the dir, so the sandboxed code can't forge it).
    fn failed_marker(&self, job: &str) -> PathBuf {
        self.sandbox_root.join(format!("{job}{FAILED_SUFFIX}"))
    }

    /// Marks a failed `sandbox/<job>/` (kept for the panel), then prunes
    /// the oldest failed dirs so at most 20 / 50 MiB stay (docs §6). A
    /// compiled dir waiting for its run card is never marked, so never
    /// pruned.
    fn keep_failed(&self, work: &Path) {
        if let Some(name) = work.file_name().and_then(|n| n.to_str()) {
            if work.parent() == Some(self.sandbox_root.as_path()) && job_id_ok(name) {
                let _ = std::fs::write(self.failed_marker(name), b"failed\n");
            }
        }
        prune_sandbox(&self.sandbox_root, KEEP_FAILED_DIRS, KEEP_FAILED_BYTES);
    }

    /// Host paths that must never reach a skill.
    fn local_paths(&self, job: &BuildJob) -> Vec<String> {
        let mut v = vec![p(&self.work_dir(&job.id)), p(&self.sandbox_root)];
        if let Some(d) = &self.skills_dir {
            v.push(p(d));
        }
        if let Some(r) = &self.tools.rust {
            v.push(p(&r.sysroot));
        }
        if let Some(d) = lillith_core::config::data_dir() {
            v.push(p(&d));
        }
        if let Some(h) = std::env::var_os("HOME") {
            v.push(h.to_string_lossy().into_owned());
        }
        v.retain(|x| x.len() > 1);
        v.sort_by_key(|x| std::cmp::Reverse(x.len()));
        v
    }
}

/// Why an exit is a failure (`timeout` → 124 / 137, a limit → a signal).
fn failure_of(code: Option<i32>, signal: Option<i32>) -> Option<String> {
    match (code, signal) {
        (Some(0), _) => None,
        (Some(124), _) => Some("timeout (wall clock)".into()),
        (Some(137), _) => Some("timeout (killed after the grace period)".into()),
        (Some(c), _) if c > 128 => Some(format!("killed by signal {} (a limit)", c - 128)),
        (Some(c), _) => Some(format!("exit code {c}")),
        (None, Some(s)) => Some(format!("killed by signal {s} (a limit)")),
        (None, None) => Some("ended without a status".into()),
    }
}

fn mkdir_private(d: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(d)
}

/// The suffix of a failed dir's mark (`sandbox/<job>.failed`).
pub const FAILED_SUFFIX: &str = ".failed";

/// `[mind] skills_dir` when `[mind]` is on.
fn skills_dir_of(cfg: &Config) -> Option<PathBuf> {
    if cfg.mind.enabled {
        cfg.mind.resolved_skills_dir()
    } else {
        None
    }
}

/// Refuses a symlink (the sandbox root or a job dir must be a real dir).
fn refuse_symlink(d: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(d) {
        Ok(m) if m.file_type().is_symlink() => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} is a symlink; refusing it", d.display()),
        )),
        _ => Ok(()),
    }
}

/// The first free `<dir>/<base>.md`, `<base>-2.md`, … (never overwrites).
fn free_skill_path(dir: &Path, base: &str) -> Option<PathBuf> {
    (1..100)
        .map(|n| {
            if n == 1 {
                dir.join(format!("{base}.md"))
            } else {
                dir.join(format!("{base}-{n}.md"))
            }
        })
        .find(|p| std::fs::symlink_metadata(p).is_err())
}

/// Bytes and entries under `d` (symlinks not followed; a file counts its
/// apparent size or its blocks, whichever is larger).
pub fn dir_usage(d: &Path) -> (u64, u64) {
    let Ok(rd) = std::fs::read_dir(d) else {
        return (0, 0);
    };
    let mut bytes = 0u64;
    let mut entries = 0u64;
    for e in rd.flatten() {
        entries += 1;
        match e.file_type() {
            Ok(t) if t.is_dir() => {
                let (b, n) = dir_usage(&e.path());
                bytes = bytes.saturating_add(b);
                entries = entries.saturating_add(n);
            }
            Ok(t) if t.is_file() => {
                if let Ok(m) = e.metadata() {
                    #[cfg(unix)]
                    let real = {
                        use std::os::unix::fs::MetadataExt;
                        m.blocks().saturating_mul(512)
                    };
                    #[cfg(not(unix))]
                    let real = 0;
                    bytes = bytes.saturating_add(m.len().max(real));
                }
            }
            _ => {}
        }
    }
    (bytes, entries)
}

/// Bytes under `d` (symlinks not followed).
pub fn dir_bytes(d: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(d) else {
        return 0;
    };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_bytes(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// Deletes the oldest *failed* job dirs under `root` (job-id names with a
/// `<job>.failed` mark) until at most `max_dirs` and `max_bytes` of them
/// remain. Unmarked dirs (a compiled job waiting for its run card) are
/// never touched.
pub fn prune_sandbox(root: &Path, max_dirs: usize, max_bytes: u64) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    let mut dirs: Vec<(std::time::SystemTime, PathBuf, u64)> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| job_id_ok(&e.file_name().to_string_lossy()))
        .filter(|e| {
            root.join(format!(
                "{}{FAILED_SUFFIX}",
                e.file_name().to_string_lossy()
            ))
            .is_file()
        })
        .map(|e| {
            let t = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (t, e.path(), dir_bytes(&e.path()))
        })
        .collect();
    // Newest first; keep while under both caps.
    dirs.sort_by(|a, b| b.0.cmp(&a.0));
    let mut total = 0u64;
    for (i, (_, path, bytes)) in dirs.iter().enumerate() {
        total += bytes;
        if i >= max_dirs || total > max_bytes {
            let _ = std::fs::remove_dir_all(path);
            let mut m = path.clone().into_os_string();
            m.push(FAILED_SUFFIX);
            let _ = std::fs::remove_file(PathBuf::from(m));
        }
    }
}

/// `skills/<slug>.md` name: `[a-z0-9-]`, at most 48, else `practice`.
pub fn slug(purpose: &str) -> String {
    let mut out = String::new();
    for c in purpose.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 48 {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "practice".into()
    } else {
        out
    }
}

fn cap(s: &str, max: usize) -> (String, bool) {
    if s.len() <= max {
        return (s.to_string(), false);
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].to_string(), true)
}

/// The skill text, or why none may be written. The purpose and the code
/// must be free of local identifiers and paths (a skill is never edited
/// into broken code); output lines that carry one, or look like a secret,
/// are dropped, and host paths are rewritten to `/work`.
pub fn skill_body(
    job: &BuildJob,
    stdout: &str,
    deny: &IdentityDenylist,
    local_paths: &[String],
) -> Result<String, String> {
    if let Some(h) = deny.hit(&job.purpose) {
        return Err(format!("the purpose carries {h}"));
    }
    if let Some(h) = deny.hit(&job.source) {
        return Err(format!("the code carries {h}"));
    }
    let mut out = stdout.to_string();
    for lp in local_paths {
        out = out.replace(lp.as_str(), WORK);
    }
    let out: String = crate::paths::drop_secret_lines(&out)
        .lines()
        .filter(|l| deny.hit(l).is_none())
        .collect::<Vec<_>>()
        .join("\n");
    let (code, code_cut) = cap(&job.source, SKILL_MAX_BYTES / 2);
    let (out, out_cut) = cap(&out, SKILL_MAX_BYTES / 4);
    let fence = match job.lang {
        Lang::Rust => "rust",
        Lang::Python => "python",
        Lang::Fish => "fish",
    };
    let mut b = format!(
        "# {}\n\nPractised in the sandbox (no network, single file, nothing installed).\n\n\
         ## Code ({})\n\n```{fence}\n{}{}\n```\n\n## Command\n\n```\n{}\n```\n\n\
         ## Observed output\n\n```\n{}{}\n```\n",
        job.purpose.chars().take(200).collect::<String>(),
        job.lang.file(),
        code.trim_end(),
        if code_cut { "\n… (cut)" } else { "" },
        skill_command(job.lang),
        out.trim_end(),
        if out_cut { "\n… (cut)" } else { "" },
    );
    if b.len() > SKILL_MAX_BYTES {
        b = cap(&b, SKILL_MAX_BYTES).0;
    }
    Ok(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "lillith-build-{name}-{}-{}",
            std::process::id(),
            &super::super::random_token()[..8]
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn on_cfg() -> Config {
        let mut c = Config::default();
        c.learn.online = true;
        // Opt-in (review N2-2).
        c.learn.practice = true;
        c
    }

    fn session() -> Session {
        Session::new("s-build", Duration::from_secs(120)).with_kill_switch_probe(Arc::new(|| false))
    }

    fn fake_tools() -> SandboxTools {
        SandboxTools {
            bwrap: "/usr/bin/bwrap".into(),
            prlimit: "/usr/bin/prlimit".into(),
            timeout: "/usr/bin/timeout".into(),
            rust: Some(RustToolchain {
                rustc: "/opt/rust/bin/rustc".into(),
                sysroot: "/opt/rust".into(),
                linker: "/usr/bin/gcc".into(),
            }),
            python3: Some("/usr/bin/python3".into()),
            fish: Some("/usr/bin/fish".into()),
        }
    }

    fn practice_in(dir: &Path, tools: SandboxTools) -> Practice {
        Practice {
            tools,
            sandbox_root: dir.join("sandbox"),
            skills_dir: Some(dir.join("skills")),
            audit_log: Some(dir.join("audit.log")),
            job_log: JobLog::new(Some(dir.join("bots"))),
            deny: IdentityDenylist::from_local(),
        }
    }

    fn audit(dir: &Path) -> String {
        std::fs::read_to_string(dir.join("audit.log")).unwrap_or_default()
    }

    /// The real sandbox, if this machine can run it (bwrap, prlimit,
    /// timeout and user namespaces). `None` → the caller skips.
    fn real_sandbox(test: &str) -> Option<SandboxTools> {
        let Ok(t) = SandboxTools::from_found(FoundTools::detect()) else {
            eprintln!("SKIP {test}: bwrap / prlimit / timeout not installed");
            return None;
        };
        let d = tmp("probe");
        let argv = sandbox_argv(
            &t,
            &d,
            &BuildBudgets::default(),
            BuildStep::Run,
            &["/usr/bin/true".to_string()],
            None,
        );
        let ok = matches!(
            super::super::process::run_sandboxed(&argv, &d, 1024, Duration::from_secs(20), &|| {
                None
            }),
            Ok(SandboxEnd::Exited { code: Some(0), .. })
        );
        let _ = std::fs::remove_dir_all(&d);
        if !ok {
            eprintln!("SKIP {test}: bwrap can't create a sandbox here");
            return None;
        }
        eprintln!("RAN-BWRAP {test}");
        Some(t)
    }

    /// Runs `inner` in the real sandbox, bypassing the static check (the
    /// sandbox is the boundary under test).
    fn raw(t: &SandboxTools, work: &Path, b: &BuildBudgets, inner: &[&str]) -> SandboxEnd {
        let inner: Vec<String> = inner.iter().map(|s| s.to_string()).collect();
        let argv = sandbox_argv(t, work, b, BuildStep::Run, &inner, None);
        super::super::process::run_sandboxed(
            &argv,
            work,
            OUTPUT_CAP,
            b.run_wall + Duration::from_secs(10),
            &|| None,
        )
        .unwrap()
    }

    fn stdout_of(e: &SandboxEnd) -> String {
        match e {
            SandboxEnd::Exited { stdout, .. } => stdout.clone(),
            other => panic!("{other:?}"),
        }
    }

    struct Fixed {
        answer: SandboxAnswer,
        asked: Cell<usize>,
        seen: std::cell::RefCell<Vec<SandboxCardRequest>>,
        skill_cards: std::cell::RefCell<Vec<SkillCardRequest>>,
    }

    impl Fixed {
        fn new(answer: SandboxAnswer) -> Self {
            Self {
                answer,
                asked: Cell::new(0),
                seen: Default::default(),
                skill_cards: Default::default(),
            }
        }
    }

    impl SandboxConsent for Fixed {
        fn ask(&self, req: &SandboxCardRequest, _stop: &dyn Fn() -> bool) -> SandboxAnswer {
            self.asked.set(self.asked.get() + 1);
            self.seen.borrow_mut().push(req.clone());
            self.answer.clone()
        }

        fn ask_skill(&self, req: &SkillCardRequest, _stop: &dyn Fn() -> bool) -> SandboxAnswer {
            self.skill_cards.borrow_mut().push(req.clone());
            self.answer.clone()
        }
    }

    fn yes() -> Fixed {
        Fixed::new(SandboxAnswer::Click(CardAnswer::Yes))
    }

    const PY_OK: &str = "total = 0\nfor i in range(1, 11):\n    total += i\nprint('sum', total)\n";

    #[test]
    fn build_defaults_deny_network_and_are_resource_limited() {
        let b = BuildBudgets::default();
        assert_eq!(b.network, NetworkPolicy::Denied);
        assert_eq!(b.compile_wall, Duration::from_secs(30));
        assert_eq!(b.run_wall, Duration::from_secs(30));
        assert_eq!(b.memory_bytes, 512 * 1024 * 1024);
        assert_eq!(b.fsize_bytes, 16 * 1024 * 1024);
        assert_eq!(b.cpu_s, 20);
        assert_eq!(b.nproc, 64);
        assert!(b.scratch_bytes <= 50 * 1024 * 1024);
        // Build jobs never go through the research supervisor.
        assert!(!super::super::WorkerKind::Build.implemented());
        assert!(super::super::Budgets::for_kind(
            super::super::WorkerKind::Build,
            &Default::default()
        )
        .is_none());
        let j = BuildJob::new("w-b1", "a tiny todo app", Lang::Python, PY_OK);
        assert!(j.needs_card_per_run());
        assert!(!j.compiled());
    }

    #[test]
    fn practice_is_off_by_default_and_behind_the_online_gate() {
        let d = Config::default();
        assert!(!d.learn.practice, "practice is opt-in (N2-2)");
        let mut only_online = d.clone();
        only_online.learn.online = true;
        assert_eq!(gate(&only_online), Err(PracticeOff::PracticeOff));
        assert_eq!(gate(&d), Err(PracticeOff::Gate(online::Off::OnlineOff)));
        assert!(Practice::new(&d).is_err());
        let mut c = on_cfg();
        c.learn.practice = false;
        assert_eq!(gate(&c), Err(PracticeOff::PracticeOff));
        let mut c = on_cfg();
        c.memory.enabled = false;
        assert_eq!(gate(&c), Err(PracticeOff::Gate(online::Off::MemoryOff)));
        let mut c = on_cfg();
        c.learn.enabled = false;
        assert_eq!(gate(&c), Err(PracticeOff::Gate(online::Off::LearnOff)));
        if cfg!(target_os = "linux") {
            assert_eq!(gate(&on_cfg()), Ok(()));
        }
        // The shipped defaults: online off, practice off (opt-in, N2-2).
        assert!(!d.learn.online);
        assert!(!d.learn.practice);
    }

    #[test]
    fn missing_bwrap_or_prlimit_refuses_and_never_falls_back() {
        let full = FoundTools {
            bwrap: Some("/usr/bin/bwrap".into()),
            prlimit: Some("/usr/bin/prlimit".into()),
            timeout: Some("/usr/bin/timeout".into()),
            ..FoundTools::default()
        };
        let cfg = on_cfg();
        for (found, tool) in [
            (
                FoundTools {
                    bwrap: None,
                    ..full.clone()
                },
                "bwrap",
            ),
            (
                FoundTools {
                    prlimit: None,
                    ..full.clone()
                },
                "prlimit",
            ),
            (
                FoundTools {
                    prlimit: Some("/opt/prlimit".into()),
                    ..full.clone()
                },
                "prlimit",
            ),
            (
                FoundTools {
                    timeout: None,
                    ..full.clone()
                },
                "timeout",
            ),
        ] {
            let e = Practice::with_tools(&cfg, found).unwrap_err();
            assert_eq!(e, PracticeOff::Missing(tool.into()));
            let msg = e.to_string();
            assert!(
                msg.contains("never installed") && msg.contains("never a weaker sandbox"),
                "{msg}"
            );
        }
        // The runner itself execs nothing but bwrap: no unsandboxed path.
        let d = tmp("nofallback");
        for argv in [
            vec![
                "/usr/bin/python3".to_string(),
                "-c".into(),
                "print(1)".into(),
            ],
            vec![
                "/usr/bin/unshare".to_string(),
                "-n".into(),
                "-r".into(),
                "true".into(),
            ],
            vec![],
        ] {
            assert!(super::super::process::run_sandboxed(
                &argv,
                &d,
                64,
                Duration::from_secs(1),
                &|| None
            )
            .is_err());
        }
        // A language without its toolchain is refused, not installed.
        let t = SandboxTools {
            rust: None,
            python3: None,
            fish: None,
            ..fake_tools()
        };
        for lang in [Lang::Rust, Lang::Python, Lang::Fish] {
            assert!(step_argv(&t, lang, BuildStep::Compile).is_err(), "{lang:?}");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn sandbox_argv_matches_the_doc() {
        let t = fake_tools();
        let b = BuildBudgets::default();
        let work = Path::new("/home/u/.local/share/lillith/sandbox/w-1");
        let inner = step_argv(&t, Lang::Python, BuildStep::Run).unwrap();
        let got = sandbox_argv(&t, work, &b, BuildStep::Run, &inner, None).join(" ");
        assert_eq!(
            got,
            "/usr/bin/bwrap --unshare-all --die-with-parent --new-session \
             --ro-bind /usr /usr --symlink usr/lib /lib --symlink usr/lib64 /lib64 \
             --symlink usr/bin /bin --proc /proc --dev /dev --size 52428800 --tmpfs /tmp \
             --bind /home/u/.local/share/lillith/sandbox/w-1 /work --chdir /work \
             --remount-ro / --clearenv --setenv CARGO_NET_OFFLINE true \
             --setenv PATH /usr/bin:/bin --setenv HOME /work --setenv TMPDIR /tmp \
             --setenv LANG C --setenv TZ UTC --setenv MALLOC_ARENA_MAX 1 \
             -- /usr/bin/prlimit --as=536870912 --cpu=20 --nproc=64 --fsize=13107200 \
             --nofile=64 --core=0 /usr/bin/timeout -k 2 30 /usr/bin/python3 -I -S main.py"
        );
        // Rust: the doc's rustc line (+ the resolved linker), its sysroot
        // read-only; budgets drive the limits and the timeout.
        let mut b2 = b.clone();
        b2.compile_wall = Duration::from_secs(7);
        b2.memory_bytes = 1 << 20;
        let inner = step_argv(&t, Lang::Rust, BuildStep::Compile).unwrap();
        let a = sandbox_argv(
            &t,
            work,
            &b2,
            BuildStep::Compile,
            &inner,
            Some(Path::new("/opt/rust")),
        );
        let j = a.join(" ");
        assert!(
            j.contains("--ro-bind /opt/rust /opt/rust --remount-ro /"),
            "{j}"
        );
        assert!(j.contains("--as=1048576 "), "{j}");
        assert!(
            j.ends_with(
                "/usr/bin/timeout -k 2 7 /opt/rust/bin/rustc --edition 2021 -C opt-level=0 \
             -C linker=/usr/bin/gcc main.rs"
            ),
            "{j}"
        );
        assert_eq!(
            step_argv(&t, Lang::Fish, BuildStep::Compile)
                .unwrap()
                .join(" "),
            "/usr/bin/fish --no-config -n main.fish"
        );
        assert!(!j.contains("--share-net") && !j.contains("--bind / "));
        // N2-5: a rustup sysroot under $HOME is bound at /opt/rust; nothing
        // inside the sandbox (after `--`) names the home path.
        let mut t2 = fake_tools();
        let home_sr = PathBuf::from("/home/u/.rustup/toolchains/1.85-x86_64");
        t2.rust = Some(RustToolchain {
            rustc: home_sr.join("bin/rustc"),
            sysroot: home_sr.clone(),
            linker: "/usr/bin/gcc".into(),
        });
        let inner = step_argv(&t2, Lang::Rust, BuildStep::Compile).unwrap();
        assert_eq!(inner[0], "/opt/rust/bin/rustc");
        let a = sandbox_argv(&t2, work, &b, BuildStep::Compile, &inner, Some(&home_sr));
        let j = a.join(" ");
        assert!(
            j.contains("--ro-bind /home/u/.rustup/toolchains/1.85-x86_64 /opt/rust "),
            "{j}"
        );
        let after = j.split(" -- ").nth(1).unwrap();
        assert!(
            !after.contains(".rustup") && !after.contains("/home/u"),
            "{after}"
        );
    }

    #[test]
    fn static_reject_happens_before_any_card_or_compile() {
        let dir = tmp("reject");
        let p = practice_in(&dir, fake_tools());
        let c = yes();
        for (lang, src) in [
            (Lang::Rust, "use ::std::net::TcpStream; fn main(){}"),
            (Lang::Python, "import socket\n"),
            (Lang::Rust, "fn main(){ unsafe {} }"),
        ] {
            let j = BuildJob::new("w-rej", "net test", lang, src);
            let e = p
                .propose(&j, BuildStep::Compile, &c, &session())
                .unwrap_err();
            assert!(matches!(e, BuildError::Rejected(_)), "{e:?}");
        }
        assert_eq!(c.asked.get(), 0, "no card for rejected code");
        let a = audit(&dir);
        assert!(a.contains("\tWORKER\treject\t"), "{a}");
        assert!(!a.contains("PROPOSED"), "{a}");
        assert!(!dir.join("sandbox").exists(), "nothing compiled");
        let d8 = JobLog::new(Some(dir.join("bots"))).read("w-rej");
        assert!(d8
            .iter()
            .all(|e| e.status == JobStatus::Refused && e.bot == "local:build-worker"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_click_no_launch_and_every_answer_is_audited() {
        let dir = tmp("noclick");
        let p = practice_in(&dir, fake_tools());
        let j = BuildJob::new("w-noclick", "sum numbers", Lang::Python, PY_OK);
        for (ans, word) in [
            (CardAnswer::No, "DENY"),
            (CardAnswer::TimedOut, "TIMEOUT"),
            (CardAnswer::Cancelled, "CANCELLED"),
            (CardAnswer::NoUi, "NO_UI"),
        ] {
            let c = Fixed::new(SandboxAnswer::Click(ans));
            let e = p
                .propose(&j, BuildStep::Compile, &c, &session())
                .unwrap_err();
            assert!(matches!(e, BuildError::NotApproved(_)), "{ans:?} {e:?}");
            let a = audit(&dir);
            assert!(
                a.contains(&format!("\t{word}\tsandbox_run\tcard=sbx-")),
                "{word}: {a}"
            );
            // The card showed the SandboxRun action with the whole argv.
            let req = c.seen.borrow()[0].clone();
            assert_eq!(req.text.title, "RUN IN SANDBOX");
            assert!(matches!(
                &req.action,
                PendingAction::SandboxRun { job, card, argv }
                    if job == "w-noclick" && card == &req.card && argv == &req.argv
            ));
        }
        let a = audit(&dir);
        assert_eq!(a.matches("\tPROPOSED\tsandbox_run\t").count(), 4, "{a}");
        assert!(a.contains("argv=/usr/bin/bwrap --unshare-all"), "{a}");
        assert!(!a.contains("CONFIRM"), "{a}");
        assert!(!a.contains("\tWORKER\tcompile"), "nothing launched: {a}");
        assert!(!p.work_dir("w-noclick").exists());
        // Fail-closed: no audit log, no card.
        let c = yes();
        let p2 = practice_in(&dir, fake_tools()).with_audit_log(None);
        assert!(matches!(
            p2.propose(&j, BuildStep::Compile, &c, &session()),
            Err(BuildError::Audit(_))
        ));
        assert_eq!(c.asked.get(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn typed_or_spoken_yes_never_counts() {
        let dir = tmp("typed");
        let p = practice_in(&dir, fake_tools());
        let j = BuildJob::new("w-typed", "sum numbers", Lang::Python, PY_OK);
        for ans in [
            SandboxAnswer::Typed("yes".into()),
            SandboxAnswer::Typed("Yes, run it".into()),
            SandboxAnswer::Spoken("yes".into()),
        ] {
            let c = Fixed::new(ans.clone());
            let e = p
                .propose(&j, BuildStep::Compile, &c, &session())
                .unwrap_err();
            assert!(
                matches!(&e, BuildError::NotApproved(w) if w.contains("never counts")),
                "{ans:?}: {e:?}"
            );
        }
        let a = audit(&dir);
        assert_eq!(a.matches("\tDENY\tsandbox_run\t").count(), 3, "{a}");
        assert!(!a.contains("CONFIRM"));
        assert!(!p.work_dir("w-typed").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Static proof that an approval can't be copied: this only compiles
    // while `CardApproval` is neither `Clone` nor `Copy`.
    #[allow(dead_code)]
    trait AmbiguousIfClone<A> {
        fn some_item() {}
    }
    impl<T: ?Sized> AmbiguousIfClone<()> for T {}
    impl<T: Clone> AmbiguousIfClone<u8> for T {}
    const _: fn() = <CardApproval as AmbiguousIfClone<_>>::some_item;

    #[test]
    fn approval_is_single_use_and_bound_to_job_step_and_source() {
        let dir = tmp("bound");
        let p = practice_in(&dir, fake_tools());
        let s = session();
        let mut j = BuildJob::new("w-bound", "sum numbers", Lang::Python, PY_OK);
        let forge = |card: &str, job: &str, step, hash: &str| CardApproval {
            card: card.into(),
            job: job.into(),
            step,
            source_hash: hash.into(),
            argv_hash: "sha256:0".into(),
        };
        let h = j.source_hash();
        // Another job's approval.
        let e = p.execute(
            &mut j,
            forge("sbx-a", "w-other", BuildStep::Compile, &h),
            &s,
        );
        assert!(matches!(e, Err(BuildError::ApprovalMismatch(_))), "{e:?}");
        // Another source's approval (the code changed after the click).
        let e = p.execute(
            &mut j,
            forge("sbx-b", "w-bound", BuildStep::Compile, "sha256:0"),
            &s,
        );
        assert!(matches!(e, Err(BuildError::ApprovalMismatch(_))), "{e:?}");
        // A replayed card id: spent by the first attempt above.
        let e = p.execute(
            &mut j,
            forge("sbx-b", "w-bound", BuildStep::Compile, &h),
            &s,
        );
        assert!(
            matches!(&e, Err(BuildError::ApprovalMismatch(w)) if w.contains("already used")),
            "{e:?}"
        );
        // A run approval before any compile.
        let e = p.execute(&mut j, forge("sbx-c", "w-bound", BuildStep::Run, &h), &s);
        assert_eq!(e, Err(BuildError::NotCompiled));
        // Run proposals need a compile of this exact source first.
        assert_eq!(
            p.propose(&j, BuildStep::Run, &yes(), &s).unwrap_err(),
            BuildError::NotCompiled
        );
        // The real approval names its job, step and hash; revising the
        // source voids it.
        let a = p.propose(&j, BuildStep::Compile, &yes(), &s).unwrap();
        assert_eq!(
            (a.job(), a.step(), a.source_hash()),
            ("w-bound", BuildStep::Compile, h.as_str())
        );
        j.revise("print('other')\n");
        let e = p.execute(&mut j, a, &s);
        assert!(matches!(e, Err(BuildError::ApprovalMismatch(_))), "{e:?}");
        let a = audit(&dir);
        assert!(a.contains("\tWORKER\trefused\t"), "{a}");
        assert!(!a.contains("\tWORKER\tcompile"), "nothing launched: {a}");
        assert!(!p.work_dir("w-bound").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A card that waits for the stop probe, then "answers" Yes anyway.
    struct WaitsForStop<'s> {
        trigger: Box<dyn Fn() + 's>,
    }

    impl SandboxConsent for WaitsForStop<'_> {
        fn ask(&self, _req: &SandboxCardRequest, stop: &dyn Fn() -> bool) -> SandboxAnswer {
            (self.trigger)();
            let t0 = std::time::Instant::now();
            while !stop() && t0.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(stop(), "the card must see the stop");
            SandboxAnswer::Click(CardAnswer::Yes)
        }
    }

    #[test]
    fn every_stop_reason_cancels_an_open_card() {
        let dir = tmp("cancelcard");
        let p = practice_in(&dir, fake_tools());
        let j = BuildJob::new("w-cancel", "sum numbers", Lang::Python, PY_OK);
        for r in StopReason::ALL {
            let s = session();
            let c = WaitsForStop {
                trigger: Box::new(|| s.request_stop(r)),
            };
            let e = p.propose(&j, BuildStep::Compile, &c, &s).unwrap_err();
            assert_eq!(e, BuildError::Stopped(r));
        }
        // The process-wide kill switch, reported as such (N4).
        let fired = Arc::new(AtomicBool::new(false));
        let f2 = fired.clone();
        let s = Session::new("s-k", Duration::from_secs(60))
            .with_kill_switch_probe(Arc::new(move || f2.load(Ordering::SeqCst)));
        let c = WaitsForStop {
            trigger: Box::new(|| fired.store(true, Ordering::SeqCst)),
        };
        assert_eq!(
            p.propose(&j, BuildStep::Compile, &c, &s).unwrap_err(),
            BuildError::Stopped(StopReason::KillSwitch)
        );
        let a = audit(&dir);
        assert_eq!(
            a.matches("\tCANCELLED\tsandbox_run\t").count(),
            StopReason::ALL.len() + 1,
            "{a}"
        );
        assert!(!a.contains("CONFIRM"), "{a}");
        assert!(!p.work_dir("w-cancel").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A card that hears the safeword while open, then gets a click on Yes.
    struct SaysRedThenYes<'s> {
        session: &'s Session,
        word: &'static str,
        saw_stop: std::cell::Cell<bool>,
    }

    impl SandboxConsent for SaysRedThenYes<'_> {
        fn ask(&self, _req: &SandboxCardRequest, stop: &dyn Fn() -> bool) -> SandboxAnswer {
            assert!(self.session.observe_chat(self.word), "{}", self.word);
            std::thread::sleep(Duration::from_millis(50));
            self.saw_stop.set(stop());
            SandboxAnswer::Click(CardAnswer::Yes)
        }
    }

    #[test]
    fn the_safeword_never_cancels_an_open_card_it_only_hushes_her_voice() {
        // Ko, 2026-10-24: "Just the voice". Red stops her talking (the chat
        // layer's kill_switch::hush); the sandbox card and session go on.
        let dir = tmp("redcard");
        let p = practice_in(&dir, fake_tools());
        let j = BuildJob::new("w-red", "sum numbers", Lang::Python, PY_OK);
        for word in ["red", "OOC: stop"] {
            assert!(lillith_core::access::kill_switch::is_safeword(word));
            let s = session();
            let c = SaysRedThenYes {
                session: &s,
                word,
                saw_stop: std::cell::Cell::new(true),
            };
            let a = p.propose(&j, BuildStep::Compile, &c, &s);
            assert!(a.is_ok(), "{word}: {a:?}");
            assert!(!c.saw_stop.get(), "{word}: the card never saw a stop");
            assert_eq!(s.stop_reason(), None, "{word}: the session runs on");
        }
        let a = audit(&dir);
        assert!(!a.contains("	CANCELLED	sandbox_run	"), "{a}");
        assert_eq!(a.matches("	CONFIRM	sandbox_run	").count(), 2, "{a}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skill_body_is_capped_and_free_of_identifiers_and_paths() {
        let deny = IdentityDenylist::empty().with_names(["Ko Kannadian"]);
        let j = BuildJob::new("w-s", "Sum the numbers 1 to 10", Lang::Python, PY_OK);
        let out = "sum 55\nwrote /home/u/.local/share/lillith/sandbox/w-s/x.txt\nhi kannadian\n";
        let b = skill_body(
            &j,
            out,
            &deny,
            &["/home/u/.local/share/lillith/sandbox/w-s".into()],
        )
        .unwrap();
        assert!(
            b.contains("## Code (main.py)") && b.contains(PY_OK.trim_end()),
            "{b}"
        );
        assert!(b.contains("python3 -I -S main.py"), "{b}");
        assert!(
            b.contains("sum 55") && b.contains("wrote /work/x.txt"),
            "{b}"
        );
        assert!(!b.to_lowercase().contains("kannadian"), "{b}");
        assert!(!b.contains("/home/"), "{b}");
        // Identifiers in the code or the purpose: no skill at all.
        let j2 = BuildJob::new("w-s", "greet", Lang::Python, "print('hi Kannadian')\n");
        assert!(skill_body(&j2, "", &deny, &[]).is_err());
        let j3 = BuildJob::new("w-s", "greet Ko Kannadian", Lang::Python, PY_OK);
        assert!(skill_body(&j3, "", &deny, &[]).is_err());
        // Size cap.
        let big = format!("# {}\nprint(1)\n", "x".repeat(20_000));
        let j4 = BuildJob::new("w-s", "big", Lang::Python, &big);
        let b = skill_body(&j4, &"y\n".repeat(20_000), &deny, &[]).unwrap();
        assert!(b.len() <= SKILL_MAX_BYTES, "{}", b.len());
        assert!(b.contains("(cut)"));
        assert_eq!(slug("Sum the numbers 1 to 10!"), "sum-the-numbers-1-to-10");
        assert_eq!(slug("../../etc"), "etc");
        assert_eq!(slug("ü"), "practice");
    }

    #[test]
    fn failed_dirs_are_pruned_to_twenty_and_fifty_mib() {
        let dir = tmp("prune");
        for i in 0..25 {
            let d = dir.join(format!("w-old{i:02}"));
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("f"), b"x").unwrap();
            std::fs::write(dir.join(format!("w-old{i:02}{FAILED_SUFFIX}")), b"failed\n").unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
        std::fs::create_dir_all(dir.join("not-a-job")).unwrap();
        prune_sandbox(&dir, KEEP_FAILED_DIRS, KEEP_FAILED_BYTES);
        let left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("w-"))
            .collect();
        assert_eq!(left.len(), 20);
        assert!(
            !dir.join(format!("w-old00{FAILED_SUFFIX}")).exists(),
            "mark goes too"
        );
        assert!(left.contains(&"w-old24".to_string()) && !left.contains(&"w-old00".to_string()));
        assert!(dir.join("not-a-job").exists(), "only job dirs are touched");
        prune_sandbox(&dir, 20, 3);
        let n = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter(|e| e.file_name().to_string_lossy().starts_with("w-"))
            .count();
        assert_eq!(n, 3, "byte cap");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- The real sandbox (bwrap). Each test skips cleanly without it. ----

    #[test]
    fn bwrap_sandbox_has_no_network() {
        let Some(t) = real_sandbox("bwrap_sandbox_has_no_network") else {
            return;
        };
        let Some(py) = t.python3.clone() else {
            eprintln!("SKIP bwrap_sandbox_has_no_network: no python3");
            return;
        };
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let port = l.local_addr().unwrap().port();
        let work = tmp("nonet");
        let script = format!(
            "import socket\n\
             s = socket.socket()\n\
             s.settimeout(3)\n\
             try:\n    s.connect(('127.0.0.1', {port}))\n    print('CONNECTED')\n\
             except Exception as e:\n    print('FAILED', type(e).__name__)\n\
             print('IFACES', sorted(n for _, n in socket.if_nameindex()))\n"
        );
        std::fs::write(work.join("main.py"), script).unwrap();
        let end = raw(
            &t,
            &work,
            &BuildBudgets::default(),
            &[&py.to_string_lossy(), "-I", "-S", "main.py"],
        );
        let out = stdout_of(&end);
        assert!(out.contains("FAILED"), "{out}");
        assert!(!out.contains("CONNECTED"), "{out}");
        assert!(
            out.contains("IFACES ['lo']"),
            "only a private loopback: {out}"
        );
        assert!(l.accept().is_err(), "nothing reached the host's loopback");
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn bwrap_sandbox_writes_only_in_its_job_dir() {
        let Some(t) = real_sandbox("bwrap_sandbox_writes_only_in_its_job_dir") else {
            return;
        };
        let root = tmp("writes");
        let work = root.join("w-writes");
        std::fs::create_dir_all(&work).unwrap();
        let escape = root.join("escape");
        let host_tmp =
            std::env::temp_dir().join(format!("lillith-sbx-{}", super::super::random_token()));
        let script = format!(
            "for f in /x /usr/x /etc/x /lib/x /proc/x {esc} {ht}; do \
               if (echo hi > $f) 2>/dev/null; then echo WROTE $f; else echo DENIED $f; fi; done; \
             echo hi > /work/ok && echo WROTE /work/ok; \
             test -e {home} && echo SEES-HOME || echo NO-HOME",
            esc = escape.display(),
            ht = host_tmp.display(),
            home = std::env::var("HOME").unwrap_or_else(|_| "/home".into()),
        );
        let end = raw(
            &t,
            &work,
            &BuildBudgets::default(),
            &["/usr/bin/sh", "-c", &script],
        );
        let out = stdout_of(&end);
        for f in ["/x", "/usr/x", "/etc/x", "/lib/x", "/proc/x"] {
            assert!(out.contains(&format!("DENIED {f}\n")), "{f}: {out}");
        }
        assert!(
            out.contains("DENIED") && out.contains("WROTE /work/ok"),
            "{out}"
        );
        assert!(out.contains("NO-HOME"), "{out}");
        assert!(work.join("ok").exists());
        assert!(!escape.exists(), "{out}");
        assert!(!host_tmp.exists(), "/tmp is the sandbox's own tmpfs");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn bwrap_limits_stop_fork_bomb_memory_hog_and_infinite_loop() {
        let Some(t) = real_sandbox("bwrap_limits_stop_fork_bomb_memory_hog_and_infinite_loop")
        else {
            return;
        };
        let work = tmp("limits");
        let b = BuildBudgets {
            run_wall: Duration::from_secs(3),
            ..BuildBudgets::default()
        };
        let t0 = std::time::Instant::now();
        // Infinite loop: timeout.
        let end = raw(&t, &work, &b, &["/usr/bin/sh", "-c", "while :; do :; done"]);
        let SandboxEnd::Exited { code, signal, .. } = end else {
            panic!("{end:?}")
        };
        assert!(
            failure_of(code, signal).unwrap().contains("timeout"),
            "{code:?} {signal:?}"
        );
        // Fork bomb: nproc + timeout, the pid namespace dies with it.
        let end = raw(
            &t,
            &work,
            &b,
            &["/usr/bin/sh", "-c", "f() { f | f & }; f; sleep 60"],
        );
        let SandboxEnd::Exited { code, signal, .. } = end else {
            panic!("{end:?}")
        };
        assert!(failure_of(code, signal).is_some());
        if let Some(py) = t.python3.as_ref() {
            let py = py.to_string_lossy();
            // 1 GiB: --as.
            let end = raw(
                &t,
                &work,
                &b,
                &[
                    &py,
                    "-I",
                    "-S",
                    "-c",
                    "b = bytearray(1 << 30)\nprint('ALLOCATED')",
                ],
            );
            let SandboxEnd::Exited {
                code,
                stdout,
                stderr,
                ..
            } = end
            else {
                panic!()
            };
            assert_ne!(code, Some(0));
            assert!(
                !stdout.contains("ALLOCATED") && stderr.contains("MemoryError"),
                "{stderr}"
            );
            // A 32 MiB file: --fsize stops it at a quarter of the scratch
            // cap (12.5 MiB, below the 16 MiB budget; review N3-3).
            let end = raw(
                &t,
                &work,
                &b,
                &[
                    &py,
                    "-I",
                    "-S",
                    "-c",
                    "open('big', 'wb').write(b'x' * (32 << 20))",
                ],
            );
            let SandboxEnd::Exited { code, .. } = end else {
                panic!()
            };
            assert_ne!(code, Some(0));
            assert!(
                std::fs::metadata(work.join("big"))
                    .map(|m| m.len())
                    .unwrap_or(0)
                    <= effective_fsize(&b)
            );
        }
        assert!(t0.elapsed() < Duration::from_secs(40), "{:?}", t0.elapsed());
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn bwrap_runs_with_nice_19_and_idle_io() {
        let Some(t) = real_sandbox("bwrap_runs_with_nice_19_and_idle_io") else {
            return;
        };
        let work = tmp("nice");
        let end = raw(
            &t,
            &work,
            &BuildBudgets::default(),
            &["/usr/bin/sh", "-c", "cut -d' ' -f19 /proc/self/stat; ionice -p $$; grep NoNewPrivs /proc/self/status; echo env=$(env | sort | tr '\\n' ' ')"],
        );
        let out = stdout_of(&end);
        assert!(out.starts_with("19\n"), "{out}");
        assert!(out.contains("idle"), "{out}");
        assert!(out.contains("NoNewPrivs:\t1"), "{out}");
        assert!(out.contains("CARGO_NET_OFFLINE=true"), "{out}");
        assert!(!out.contains("USER=") && !out.contains("DISPLAY="), "{out}");
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn bwrap_stop_kills_the_running_group_and_discards_output() {
        let Some(t) = real_sandbox("bwrap_stop_kills_the_running_group_and_discards_output") else {
            return;
        };
        if t.python3.is_none() {
            eprintln!("SKIP bwrap_stop_kills…: no python3");
            return;
        }
        for r in [
            StopReason::EndEarly,
            StopReason::Stop,
            StopReason::KillSwitch,
            StopReason::ParentExit,
        ] {
            let dir = tmp("stoprun");
            let p = practice_in(&dir, t.clone());
            let s = session();
            let mut j = BuildJob::new(
                "w-stoprun",
                "count forever",
                Lang::Python,
                "i = 0\nwhile True:\n    i += 1\n    if i % 100000 == 0:\n        print(i, flush=True)\n",
            );
            let a = p.propose(&j, BuildStep::Compile, &yes(), &s).unwrap();
            assert!(p.execute(&mut j, a, &s).unwrap().ok);
            let a = p.propose(&j, BuildStep::Run, &yes(), &s).unwrap();
            let t0 = std::time::Instant::now();
            let e = std::thread::scope(|sc| {
                sc.spawn(|| {
                    std::thread::sleep(Duration::from_millis(400));
                    s.request_stop(r);
                });
                p.execute(&mut j, a, &s)
            });
            assert_eq!(e, Err(BuildError::Stopped(r)));
            assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
            assert!(!p.work_dir("w-stoprun").exists(), "output discarded");
            let au = audit(&dir);
            assert!(
                au.contains(&format!("\tWORKER\tkill\t{}\t", r.word())),
                "{au}"
            );
            assert!(au.contains("\tCANCELLED\tsandbox_run\t"), "{au}");
            let d8 = JobLog::new(Some(dir.join("bots"))).read("w-stoprun");
            assert_eq!(d8.last().map(|e| e.status), Some(JobStatus::Cancelled));
            assert!(!dir.join("skills").exists());
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn bwrap_the_safeword_during_a_run_kills_nothing() {
        let Some(t) = real_sandbox("bwrap_the_safeword_during_a_run_kills_nothing") else {
            return;
        };
        if t.python3.is_none() {
            eprintln!("SKIP bwrap_the_safeword…: no python3");
            return;
        }
        let dir = tmp("redrun");
        let p = practice_in(&dir, t);
        let s = session();
        let mut j = BuildJob::new(
            "w-redrun",
            "count a little",
            Lang::Python,
            "import time\nfor i in range(8):\n    time.sleep(0.1)\nprint('finished', 8)\n",
        );
        let a = p.propose(&j, BuildStep::Compile, &yes(), &s).unwrap();
        assert!(p.execute(&mut j, a, &s).unwrap().ok);
        let a = p.propose(&j, BuildStep::Run, &yes(), &s).unwrap();
        let out = std::thread::scope(|sc| {
            sc.spawn(|| {
                std::thread::sleep(Duration::from_millis(200));
                assert!(s.observe_chat("red"));
                assert!(s.observe_chat("OOC: stop"));
            });
            p.execute(&mut j, a, &s)
        })
        .unwrap();
        assert!(out.ok && out.stdout.contains("finished 8"), "{out:?}");
        assert_eq!(s.stop_reason(), None, "the safeword stopped nothing");
        let au = audit(&dir);
        assert!(!au.contains("\tWORKER\tkill\t"), "{au}");
        assert!(!au.contains("\tCANCELLED\tsandbox_run\t"), "{au}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn end_to_end(t: SandboxTools, lang: Lang, src: &str, expect: &str) {
        let dir = tmp("e2e");
        let p = practice_in(&dir, t);
        let id = format!("w-e2e-{}", lang.word());
        let mut j = BuildJob::new(&id, "Sum the numbers 1 to 10", lang, src);
        let c = yes();
        let rep = p.practice(&mut j, &c, &session());
        assert_eq!(rep.error, None, "{rep:?}");
        assert!(rep.compile.as_ref().is_some_and(|o| o.ok), "{rep:?}");
        assert_eq!(c.asked.get(), 2, "one card per compile and one per run");
        let steps: Vec<BuildStep> = c.seen.borrow().iter().map(|r| r.step).collect();
        assert_eq!(steps, [BuildStep::Compile, BuildStep::Run]);
        let cards: Vec<String> = c.seen.borrow().iter().map(|r| r.card.clone()).collect();
        assert_ne!(cards[0], cards[1]);
        let run = rep.run.clone().unwrap();
        assert!(run.ok && run.stdout.contains(expect), "{run:?}");
        let skill = rep.skill.clone().unwrap();
        assert_eq!(skill, dir.join("skills/sum-the-numbers-1-to-10.md"));
        let body = std::fs::read_to_string(&skill).unwrap();
        assert!(
            body.contains(src.trim_end()) && body.contains(skill_command(lang)),
            "{body}"
        );
        assert!(body.contains(expect), "{body}");
        assert!(
            !body.contains(&*dir.to_string_lossy()) && !body.contains("/home/"),
            "{body}"
        );
        assert!(!p.work_dir(&id).exists(), "deleted after a successful run");
        let a = audit(&dir);
        assert_eq!(a.matches("\tCONFIRM\tsandbox_run\t").count(), 2, "{a}");
        assert!(
            a.contains("\tWORKER\tcompile\tok\t") && a.contains("\tWORKER\trun\tok\t"),
            "{a}"
        );
        assert!(a.contains("\tWORKER\tskill\twritten card=skill-"), "{a}");
        assert!(a.contains("\tCONFIRM\twrite_skill\tcard=skill-"), "{a}");
        let d8 = JobLog::new(Some(dir.join("bots"))).read(&id);
        assert!(d8.iter().all(|e| e.bot == "local:build-worker"));
        assert_eq!(d8.last().map(|e| e.status), Some(JobStatus::Done));
        // The spent cards can't run anything again; a new run needs a new card.
        assert!(p
            .propose(
                &j,
                BuildStep::Run,
                &Fixed::new(SandboxAnswer::Typed("yes".into())),
                &session()
            )
            .is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bwrap_python_practice_compiles_runs_and_records_a_skill() {
        let Some(t) = real_sandbox("bwrap_python_practice_compiles_runs_and_records_a_skill")
        else {
            return;
        };
        if t.python3.is_none() {
            eprintln!("SKIP python practice: no python3");
            return;
        }
        end_to_end(t, Lang::Python, PY_OK, "sum 55");
    }

    #[test]
    fn bwrap_rust_practice_compiles_runs_and_records_a_skill() {
        let Some(t) = real_sandbox("bwrap_rust_practice_compiles_runs_and_records_a_skill") else {
            return;
        };
        if t.rust.is_none() {
            eprintln!("SKIP rust practice: no rustc / cc");
            return;
        }
        end_to_end(
            t,
            Lang::Rust,
            "fn main() {\n    let s: u32 = (1..=10).sum();\n    println!(\"sum {s}\");\n}\n",
            "sum 55",
        );
    }

    #[test]
    fn bwrap_fish_practice_compiles_runs_and_records_a_skill() {
        let Some(t) = real_sandbox("bwrap_fish_practice_compiles_runs_and_records_a_skill") else {
            return;
        };
        if t.fish.is_none() {
            eprintln!("SKIP fish practice: no fish");
            return;
        }
        end_to_end(
            t,
            Lang::Fish,
            "set -l total 0\nfor i in 1 2 3 4 5 6 7 8 9 10\n    set total (math $total + $i)\nend\necho sum $total\n",
            "sum 55",
        );
    }

    #[test]
    fn bwrap_failed_compile_keeps_the_dir_and_runs_nothing() {
        let Some(t) = real_sandbox("bwrap_failed_compile_keeps_the_dir_and_runs_nothing") else {
            return;
        };
        if t.python3.is_none() {
            return;
        }
        let dir = tmp("failc");
        let p = practice_in(&dir, t);
        let mut j = BuildJob::new("w-failc", "broken", Lang::Python, "def f(:\n    pass\n");
        let c = yes();
        let rep = p.practice(&mut j, &c, &session());
        assert_eq!(c.asked.get(), 1, "no run card after a failed compile");
        let comp = rep.compile.unwrap();
        assert!(!comp.ok && comp.stderr.contains("SyntaxError"), "{comp:?}");
        assert!(rep.run.is_none() && rep.skill.is_none());
        assert!(p.work_dir("w-failc").exists(), "kept for the panel");
        let d8 = JobLog::new(Some(dir.join("bots"))).read("w-failc");
        assert_eq!(d8.last().map(|e| e.status), Some(JobStatus::Failed));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- review fixes (slice 2) ----

    /// A card that breaks the audit log (a directory in its place) while it
    /// is open, then clicks Yes: the CONFIRM line can't be written.
    struct BreaksLog(PathBuf);

    impl SandboxConsent for BreaksLog {
        fn ask(&self, _req: &SandboxCardRequest, _stop: &dyn Fn() -> bool) -> SandboxAnswer {
            break_log(&self.0);
            SandboxAnswer::Click(CardAnswer::Yes)
        }
        fn ask_skill(&self, _req: &SkillCardRequest, _stop: &dyn Fn() -> bool) -> SandboxAnswer {
            break_log(&self.0);
            SandboxAnswer::Click(CardAnswer::Yes)
        }
    }

    fn break_log(p: &Path) {
        let _ = std::fs::remove_file(p);
        std::fs::create_dir_all(p).unwrap();
    }

    #[test]
    fn source_hash_is_sha256() {
        let j = BuildJob::new("w-h", "x", Lang::Python, PY_OK);
        let h = j.source_hash();
        assert!(h.starts_with("sha256:") && h.len() == 7 + 64, "{h}");
        let mut b = b"python\0".to_vec();
        b.extend_from_slice(PY_OK.as_bytes());
        assert_eq!(h, crate::sha256::sha256_tagged(&b));
        assert_ne!(
            h,
            BuildJob::new("w-h", "x", Lang::Python, "print(2)\n").source_hash()
        );
    }

    #[test]
    fn approval_is_bound_to_the_exact_argv_nothing_loosens_after_the_click() {
        let dir = tmp("argvbind");
        let p = practice_in(&dir, fake_tools());
        let s = session();
        let mut j = BuildJob::new("w-argv", "sum numbers", Lang::Python, PY_OK);
        // The card carries the hash of the argv it shows.
        let c = yes();
        let a = p.propose(&j, BuildStep::Compile, &c, &s).unwrap();
        let req = c.seen.borrow()[0].clone();
        assert_eq!(req.argv_hash, argv_hash(&req.argv));
        assert_eq!(a.argv_hash(), req.argv_hash);
        assert!(audit(&dir).contains(&format!("argv_hash={}", req.argv_hash)));
        // Budgets can only go down, and even that changes the argv: the
        // clicked approval no longer runs.
        let before = j.budgets().clone();
        let mut loose = before.clone();
        loose.compile_wall = Duration::from_secs(3600);
        loose.memory_bytes = u64::MAX;
        loose.scratch_bytes = u64::MAX;
        j.tighten_budgets(&loose);
        assert_eq!(j.budgets(), &before, "nothing loosens");
        let mut tight = before.clone();
        tight.compile_wall = Duration::from_secs(5);
        j.tighten_budgets(&tight);
        assert_eq!(j.budgets().compile_wall, Duration::from_secs(5));
        let e = p.execute(&mut j, a, &s);
        assert!(
            matches!(&e, Err(BuildError::ApprovalMismatch(w)) if w.contains("command line")),
            "{e:?}"
        );
        // Another sandbox root after the click: refused the same way.
        let a = p.propose(&j, BuildStep::Compile, &yes(), &s).unwrap();
        let p2 = practice_in(&dir, fake_tools()).with_sandbox_root(dir.join("elsewhere"));
        let e = p2.execute(&mut j, a, &s);
        assert!(matches!(e, Err(BuildError::ApprovalMismatch(_))), "{e:?}");
        let a = audit(&dir);
        assert!(!a.contains("\tWORKER\tcompile\t"), "nothing launched: {a}");
        assert!(!p.work_dir("w-argv").exists() && !dir.join("elsewhere").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_and_the_d8_queued_entry_are_fail_closed() {
        let dir = tmp("confirmfc");
        let log = dir.join("audit.log");
        let p = practice_in(&dir, fake_tools());
        let j = BuildJob::new("w-cfc", "sum numbers", Lang::Python, PY_OK);
        // CONFIRM can't be written → no approval.
        let e = p.propose(&j, BuildStep::Compile, &BreaksLog(log.clone()), &session());
        assert!(matches!(e, Err(BuildError::Audit(_))), "{e:?}");
        let _ = std::fs::remove_dir_all(&log);
        // The D8 job log can't be written → no card at all.
        let not_a_dir = dir.join("bots-file");
        std::fs::write(&not_a_dir, b"x").unwrap();
        let p = practice_in(&dir, fake_tools()).with_job_log(Some(not_a_dir));
        let c = yes();
        let e = p.propose(&j, BuildStep::Compile, &c, &session());
        assert!(matches!(e, Err(BuildError::Audit(_))), "{e:?}");
        assert_eq!(c.asked.get(), 0, "no card without the QUEUED entry");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_start_line_is_fail_closed_and_stops_the_session() {
        let dir = tmp("startfc");
        let log = dir.join("audit.log");
        let p = practice_in(&dir, fake_tools());
        let s = session();
        let mut j = BuildJob::new("w-sfc", "sum numbers", Lang::Python, PY_OK);
        let a = p.propose(&j, BuildStep::Compile, &yes(), &s).unwrap();
        break_log(&log);
        let e = p.execute(&mut j, a, &s);
        assert!(matches!(e, Err(BuildError::Audit(_))), "{e:?}");
        assert_eq!(s.stop_reason(), Some(StopReason::AuditFailed));
        assert!(!j.compiled());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_sandbox_root_is_refused() {
        let dir = tmp("symroot");
        let real = dir.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = dir.join("sandbox-link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let p = practice_in(&dir, fake_tools()).with_sandbox_root(link);
        let s = session();
        let mut j = BuildJob::new("w-sym", "sum numbers", Lang::Python, PY_OK);
        let a = p.propose(&j, BuildStep::Compile, &yes(), &s).unwrap();
        let e = p.execute(&mut j, a, &s);
        assert!(
            matches!(&e, Err(BuildError::Io(w)) if w.contains("symlink")),
            "{e:?}"
        );
        assert!(
            std::fs::read_dir(&real).unwrap().next().is_none(),
            "nothing written"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_dirs_marked_failed_are_pruned_never_a_compiled_one() {
        let dir = tmp("prunemark");
        let p = practice_in(&dir, fake_tools());
        let root = dir.join("sandbox");
        // A compiled job waiting for its run card: unmarked.
        std::fs::create_dir_all(root.join("w-compiled")).unwrap();
        std::fs::write(root.join("w-compiled/main.py"), PY_OK).unwrap();
        for i in 0..3 {
            let w = root.join(format!("w-f{i}"));
            std::fs::create_dir_all(&w).unwrap();
            p.keep_failed(&w);
            assert!(root.join(format!("w-f{i}{FAILED_SUFFIX}")).is_file());
        }
        prune_sandbox(&root, 0, 0);
        assert!(root.join("w-compiled/main.py").exists(), "never pruned");
        for i in 0..3 {
            assert!(!root.join(format!("w-f{i}")).exists());
        }
        // A mark outside the sandbox root (or a non-job name) isn't made.
        p.keep_failed(&dir.join("w-outside"));
        assert!(!dir.join(format!("w-outside{FAILED_SUFFIX}")).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn ran_job(id: &str) -> BuildJob {
        let mut j = BuildJob::new(id, "Sum the numbers 1 to 10", Lang::Python, PY_OK);
        j.compiled = Some(j.source_hash());
        j.ran = Some((j.source_hash(), "sum 55\n".into()));
        j
    }

    #[test]
    fn a_skill_needs_its_own_click_and_is_written_through_the_contract() {
        let dir = tmp("skillcard");
        let p = practice_in(&dir, fake_tools());
        let skills = dir.join("skills");
        let j = ran_job("w-skill");
        // Typed, spoken, No, TimedOut, NoUi: nothing written.
        for ans in [
            SandboxAnswer::Typed("yes".into()),
            SandboxAnswer::Spoken("yes".into()),
            SandboxAnswer::Click(CardAnswer::No),
            SandboxAnswer::Click(CardAnswer::TimedOut),
            SandboxAnswer::Click(CardAnswer::NoUi),
        ] {
            let c = Fixed::new(ans.clone());
            let e = p.offer_skill(&j, &c, &session());
            assert!(
                matches!(e, Err(BuildError::NotApproved(_))),
                "{ans:?} {e:?}"
            );
            assert_eq!(c.skill_cards.borrow().len(), 1);
            assert!(!skills.exists(), "{ans:?}: nothing written");
        }
        // The card shows a WriteSkill action with the whole body.
        let c = yes();
        let path = p.offer_skill(&j, &c, &session()).unwrap();
        assert_eq!(path, skills.join("sum-the-numbers-1-to-10.md"));
        let req = c.skill_cards.borrow()[0].clone();
        assert!(matches!(
            &req.action,
            PendingAction::WriteSkill { path: pp, body } if *pp == path && *body == req.body
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), req.body);
        let a = audit(&dir);
        assert!(a.contains("\tPROPOSED\twrite_skill\tcard=skill-"), "{a}");
        assert!(a.contains("\tCONFIRM\twrite_skill\tcard=skill-"), "{a}");
        assert!(a.contains("\tDENY\twrite_skill\tcard=skill-"), "{a}");
        assert!(a.contains("\tWORKER\tskill\twritten card=skill-"), "{a}");
        // Never overwrites: the next one gets -2.
        let again = p.offer_skill(&j, &yes(), &session()).unwrap();
        assert_eq!(again, skills.join("sum-the-numbers-1-to-10-2.md"));
        // No successful run of this exact source → no card.
        let mut j2 = ran_job("w-skill");
        j2.revise("print('other')\n");
        let c = yes();
        assert_eq!(
            p.offer_skill(&j2, &c, &session()),
            Err(BuildError::NoSuccessfulRun)
        );
        assert!(c.skill_cards.borrow().is_empty());
        // Fail-closed: CONFIRM can't be written → nothing written.
        let n = std::fs::read_dir(&skills).unwrap().count();
        let e = p.offer_skill(&j, &BreaksLog(dir.join("audit.log")), &session());
        assert!(matches!(e, Err(BuildError::Audit(_))), "{e:?}");
        assert_eq!(std::fs::read_dir(&skills).unwrap().count(), n);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Says Yes, but first puts other content at the card's target path,
    /// like a second offer (or Ko) racing for the same name.
    struct TargetAppearsThenYes(&'static str);

    impl SandboxConsent for TargetAppearsThenYes {
        fn ask(&self, _req: &SandboxCardRequest, _stop: &dyn Fn() -> bool) -> SandboxAnswer {
            SandboxAnswer::Click(CardAnswer::Yes)
        }

        fn ask_skill(&self, req: &SkillCardRequest, _stop: &dyn Fn() -> bool) -> SandboxAnswer {
            std::fs::create_dir_all(req.path.parent().unwrap()).unwrap();
            std::fs::write(&req.path, self.0).unwrap();
            SandboxAnswer::Click(CardAnswer::Yes)
        }
    }

    #[test]
    fn a_second_offer_for_the_same_target_never_overwrites_it() {
        // Review N3-4: no exists()-then-write; the write is create-new.
        let dir = tmp("skillrace");
        let p = practice_in(&dir, fake_tools());
        let j = ran_job("w-race");
        let target = dir.join("skills").join("sum-the-numbers-1-to-10.md");
        let e = p.offer_skill(&j, &TargetAppearsThenYes("ko's own notes\n"), &session());
        assert!(
            matches!(&e, Err(BuildError::Io(w)) if w.contains("appeared meanwhile")),
            "{e:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "ko's own notes\n",
            "the existing file is untouched"
        );
        let a = audit(&dir);
        assert!(a.contains("\tWORKER\tskill\tnot written: "), "{a}");
        assert!(!a.contains("written card=skill-"), "{a}");
        // No temp sibling is left behind.
        let names: Vec<String> = std::fs::read_dir(dir.join("skills"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["sum-the-numbers-1-to-10.md".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn with_mind_off_no_skill_card_and_nothing_written() {
        let dir = tmp("mindoff");
        let mut c = on_cfg();
        c.mind.enabled = false;
        assert_eq!(skills_dir_of(&c), None);
        c.mind.enabled = true;
        assert_eq!(skills_dir_of(&c), c.mind.resolved_skills_dir());
        let p = practice_in(&dir, fake_tools()).with_skills_dir(None);
        let j = ran_job("w-mindoff");
        let k = yes();
        assert_eq!(
            p.offer_skill(&j, &k, &session()),
            Err(BuildError::SkillsOff)
        );
        assert!(k.skill_cards.borrow().is_empty(), "no card");
        assert!(!dir.join("skills").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bwrap_work_dir_over_its_cap_is_killed() {
        let Some(t) = real_sandbox("bwrap_work_dir_over_its_cap_is_killed") else {
            return;
        };
        let root = tmp("workcap");
        let work = root.join("w-workcap");
        std::fs::create_dir_all(&work).unwrap();
        let b = BuildBudgets {
            scratch_bytes: 1 << 20,
            ..Default::default()
        };
        // Keeps writing (slowly) well past the 1 MiB cap.
        let argv = sandbox_argv(
            &t,
            &work,
            &b,
            BuildStep::Run,
            &[
                "/usr/bin/sh".to_string(),
                "-c".to_string(),
                "i=0; while [ $i -lt 40 ]; do head -c 262144 /dev/zero > /work/f$i; i=$((i+1)); sleep 0.05; done; echo DONE".to_string(),
            ],
            None,
        );
        let w = work.clone();
        let over = move || dir_usage(&w).0 > (1 << 20);
        let end = super::super::process::run_sandboxed_watched(
            &argv,
            &work,
            OUTPUT_CAP,
            Duration::from_secs(30),
            &|| None,
            &over,
        )
        .unwrap();
        assert_eq!(end, SandboxEnd::OverCap);
        assert!(dir_bytes(&work) < 4 << 20, "stopped soon after the cap");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fsize_is_never_more_than_a_quarter_of_the_scratch_cap() {
        // Review N3-3: one file can't push /work far past its cap.
        let d = BuildBudgets::default();
        assert_eq!(effective_fsize(&d), (50 * 1024 * 1024) / 4);
        assert!(effective_fsize(&d) < d.fsize_bytes);
        let small = BuildBudgets {
            scratch_bytes: 4 << 20,
            ..Default::default()
        };
        assert_eq!(effective_fsize(&small), 1 << 20);
        let tight = BuildBudgets {
            fsize_bytes: 1 << 10,
            ..Default::default()
        };
        assert_eq!(effective_fsize(&tight), 1 << 10, "a smaller budget wins");
        let argv = sandbox_argv(
            &fake_tools(),
            Path::new("/tmp/w-fsize"),
            &small,
            BuildStep::Run,
            &["/usr/bin/true".to_string()],
            None,
        );
        assert!(argv.contains(&format!("--fsize={}", 1 << 20)), "{argv:?}");
        assert_eq!(
            super::super::process::SANDBOX_POLL,
            Duration::from_millis(10)
        );
    }

    #[test]
    fn bwrap_work_dir_overshoot_stays_well_under_one_old_fsize() {
        let Some(t) = real_sandbox("bwrap_work_dir_overshoot_stays_well_under_one_old_fsize")
        else {
            return;
        };
        let root = tmp("workfast");
        let work = root.join("w-workfast");
        std::fs::create_dir_all(&work).unwrap();
        let cap: u64 = 4 << 20;
        let b = BuildBudgets {
            scratch_bytes: cap,
            ..Default::default()
        };
        let run = |script: &str| {
            let argv = sandbox_argv(
                &t,
                &work,
                &b,
                BuildStep::Run,
                &[
                    "/usr/bin/sh".to_string(),
                    "-c".to_string(),
                    script.to_string(),
                ],
                None,
            );
            let w = work.clone();
            let over = move || dir_usage(&w).0 > cap;
            super::super::process::run_sandboxed_watched(
                &argv,
                &work,
                OUTPUT_CAP,
                Duration::from_secs(30),
                &|| None,
                &over,
            )
            .unwrap()
        };
        // One file asking for the old --fsize (16 MiB) in a single go: it
        // stops at cap/4, so on its own it can't take /work over the cap.
        let end = run("head -c 16777216 /dev/zero > /work/one");
        assert_ne!(end, SandboxEnd::OverCap, "{end:?}");
        assert!(
            dir_bytes(&work) <= effective_fsize(&b),
            "{}",
            dir_bytes(&work)
        );
        std::fs::remove_file(work.join("one")).unwrap();
        // Many files as fast as the box can write them: the watchdog
        // (every SANDBOX_POLL) kills the group, and the overshoot stays
        // under one old 16 MiB --fsize (before N3-3 a single file could
        // add that much between two polls).
        let end = run(
            "i=0; while [ $i -lt 400 ]; do head -c 16777216 /dev/zero > /work/f$i; i=$((i+1)); done; echo DONE",
        );
        assert_eq!(end, SandboxEnd::OverCap);
        let used = dir_bytes(&work);
        assert!(
            used < cap + (16 << 20),
            "overshoot {} bytes past a {cap}-byte cap",
            used.saturating_sub(cap)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn bwrap_practice_over_the_scratch_cap_fails() {
        let Some(t) = real_sandbox("bwrap_practice_over_the_scratch_cap_fails") else {
            return;
        };
        if t.python3.is_none() {
            return;
        }
        let dir = tmp("capfail");
        let p = practice_in(&dir, t);
        // Many files, each under --fsize (cap/4), together over the cap.
        let src = "for i in range(12):\n    f = open('big%d' % i, 'w')\n    f.write('x' * (200 * 1024))\n    f.close()\nprint('wrote')\n";
        let mut j = BuildJob::new("w-capfail", "write a big file", Lang::Python, src);
        let mut b = j.budgets().clone();
        b.scratch_bytes = 1 << 20;
        j.tighten_budgets(&b);
        let rep = p.practice(&mut j, &yes(), &session());
        let run = rep.run.expect("ran");
        assert!(!run.ok, "{run:?}");
        assert!(
            run.failure
                .as_deref()
                .is_some_and(|f| f.contains("size cap")),
            "{run:?}"
        );
        assert!(rep.skill.is_none() && !dir.join("skills").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bwrap_tmp_has_a_hard_size_cap() {
        let Some(t) = real_sandbox("bwrap_tmp_has_a_hard_size_cap") else {
            return;
        };
        let root = tmp("tmpcap");
        let work = root.join("w-tmpcap");
        std::fs::create_dir_all(&work).unwrap();
        let b = BuildBudgets {
            scratch_bytes: 1 << 20,
            ..Default::default()
        };
        let end = raw(
            &t,
            &work,
            &b,
            &[
                "/usr/bin/sh",
                "-c",
                "head -c 4194304 /dev/zero > /tmp/big; echo rc=$?; wc -c < /tmp/big",
            ],
        );
        let out = stdout_of(&end);
        assert!(out.contains("rc=1"), "{out}");
        let n: u64 = out.lines().last().unwrap().trim().parse().unwrap();
        assert!(n <= 1 << 20, "tmpfs capped at 1 MiB: {n}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn bwrap_nofile_and_core_limits_apply() {
        let Some(t) = real_sandbox("bwrap_nofile_and_core_limits_apply") else {
            return;
        };
        let root = tmp("nofile");
        let work = root.join("w-nofile");
        std::fs::create_dir_all(&work).unwrap();
        let end = raw(
            &t,
            &work,
            &BuildBudgets::default(),
            &["/usr/bin/cat", "/proc/self/limits"],
        );
        let out = stdout_of(&end);
        let row = |name: &str| -> Vec<String> {
            out.lines()
                .find(|l| l.starts_with(name))
                .unwrap_or_else(|| panic!("{name}: {out}"))[name.len()..]
                .split_whitespace()
                .map(str::to_string)
                .collect()
        };
        assert_eq!(
            row("Max open files")[..2],
            [NOFILE.to_string(), NOFILE.to_string()]
        );
        assert_eq!(row("Max core file size")[..2], ["0", "0"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
