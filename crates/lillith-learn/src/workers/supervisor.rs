//! The parent supervisor (§12): spawns one fresh worker per task (and per
//! restart), enforces its TTL, heartbeats, fetch policy and output checks,
//! does the worker's fetches itself, validates or quarantines what comes
//! back, restarts with backoff, keeps the breakers and the error memory,
//! and writes the D8 job log and the `WORKER` audit lines.

use super::breaker::{domain_key, source_key, Breakers, ErrorMemory, ErrorSink};
use super::dream::{DreamRefused, SessionApproval};
use super::external::Backend;
use super::joblog::{new_job_id, JobEntry, JobLog, JobStatus};
use super::policy::{FetchPolicy, Pacer};
use super::process::{find_bwrap, Out, WorkerCommand, WorkerProc};
use super::protocol::{self, Msg};
use super::quarantine::{QItem, Quarantine};
use super::scratch::Scratch;
use super::validate::{asks_for_action, validate, Candidate, Verdict};
use super::{backoff_delay, Budgets, StopReason, WorkerKind, WorkerLimits, EARLY_STOP_PAGES};
use crate::online::{self, FetchError, Fetcher, IdentityDenylist, Url};
use lillith_core::config::Config;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Most text handed back to a worker per page.
pub const MAX_PAGE_TEXT: usize = 16 * 1024;

/// One unit of work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub kind: WorkerKind,
    /// A few topic words (never chat text or memory contents).
    pub topic: String,
    /// The planned addresses (exact URLs or bare domains). The worker may
    /// fetch only on these hosts.
    pub urls: Vec<String>,
}

impl Task {
    pub fn research(topic: &str, urls: &[&str]) -> Self {
        Self {
            kind: WorkerKind::Research,
            topic: topic.to_string(),
            urls: urls.iter().map(|s| s.to_string()).collect(),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({"kind": self.kind.word(), "topic": self.topic, "urls": self.urls})
    }

    pub fn from_json(v: &Value) -> Option<Self> {
        let kind = match v.get("kind")?.as_str()? {
            "research" => WorkerKind::Research,
            "build" => WorkerKind::Build,
            _ => return None,
        };
        Some(Self {
            kind,
            topic: v.get("topic")?.as_str()?.to_string(),
            urls: v
                .get("urls")?
                .as_array()?
                .iter()
                .filter_map(|u| u.as_str().map(str::to_string))
                .collect(),
        })
    }
}

/// A dream / research session: every worker of it stops the moment it
/// does. Clones share the stop flag (the tray's End early, the battery /
/// thermal watcher and the shutdown path each hold one).
///
/// A session exists only from a clicked dream-session card
/// ([`Session::from_card`] consumes the [`SessionApproval`] that
/// [`super::dream::propose_session`] returns after a mouse click on Yes);
/// there is no other public constructor. Its id, deadline and the
/// "also use my TaskCompletion bots" tick all come from that card.
///
/// Every stop works on its own ([`StopReason`]): [`Session::end_early`],
/// [`Session::stop`], the process-wide kill switch (polled here, reported as
/// [`StopReason::KillSwitch`]) and the deadline. The safeword is not one
/// of them ([`Session::observe_chat`]): it only hushes her voice.
#[derive(Clone)]
pub struct Session {
    id: String,
    card: String,
    external: bool,
    deadline: Instant,
    stop: Arc<AtomicU8>,
    kill_switch: Arc<dyn Fn() -> bool + Send + Sync>,
    audit_log: Option<PathBuf>,
    ended: Arc<AtomicBool>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("id", &self.id)
            .field("card", &self.card)
            .field("stop", &self.stop_reason())
            .finish()
    }
}

impl Session {
    /// Starts the session one clicked card approved. Writes
    /// `DREAM\tstart\t…\tcard=<id>` first, fail-closed: no line, no
    /// session. A kill switch that fired since the card was shown stops it
    /// at once.
    pub fn from_card(approval: SessionApproval) -> Result<Self, DreamRefused> {
        let (card, id, minutes, deadline, external, audit_log, epoch) = approval.into_parts();
        crate::audit::record_dream(
            audit_log.as_deref(),
            "start",
            &format!("session={id} minutes={minutes} external={external}"),
            &card,
        )
        .map_err(|e| DreamRefused::Audit(e.to_string()))?;
        Ok(Self {
            id,
            card,
            external,
            deadline,
            stop: Arc::new(AtomicU8::new(0)),
            kill_switch: Arc::new(move || lillith_core::access::kill_switch::stopped_since(epoch)),
            audit_log,
            ended: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Tests only: a session without a card (unit tests of the
    /// supervisor itself; the card path has its own tests).
    #[cfg(test)]
    pub(crate) fn new(id: &str, length: Duration) -> Self {
        let epoch = lillith_core::access::kill_switch::epoch();
        Self {
            id: id.to_string(),
            card: format!("test-{id}"),
            external: false,
            deadline: Instant::now() + length,
            stop: Arc::new(AtomicU8::new(0)),
            kill_switch: Arc::new(move || lillith_core::access::kill_switch::stopped_since(epoch)),
            audit_log: None,
            ended: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Tests only: replaces the process-wide kill switch probe (other
    /// tests in the same binary fire the real one).
    #[cfg(test)]
    pub(crate) fn with_kill_switch_probe(mut self, p: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        self.kill_switch = p;
        self
    }

    /// Tests only: where `finish` writes its `DREAM\tend` line.
    #[cfg(test)]
    pub(crate) fn with_audit_log(mut self, log: Option<PathBuf>) -> Self {
        self.audit_log = log;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_external_tick(mut self, t: bool) -> Self {
        self.external = t;
        self
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// The dream-session card this session runs under.
    pub fn card(&self) -> &str {
        &self.card
    }

    /// Whether Ko ticked "also use my TaskCompletion bots" on this
    /// session's card and nothing has stopped the session since (any stop
    /// disables external for the rest of it; the safeword is not a stop).
    pub fn external_allowed(&self) -> bool {
        self.external && self.stop_reason().is_none()
    }

    /// Stops every worker of the session (the first reason wins).
    pub fn request_stop(&self, r: StopReason) {
        let _ = self
            .stop
            .compare_exchange(0, r.code(), Ordering::SeqCst, Ordering::SeqCst);
    }

    /// End early (tray / right-click menu). Works on its own: no kill
    /// switch involved.
    pub fn end_early(&self) {
        self.request_stop(StopReason::EndEarly);
    }

    /// A plain stop (panel, CLI, the dream subsystem shutting down). Works
    /// on its own: no kill switch involved.
    pub fn stop(&self) {
        self.request_stop(StopReason::Stop);
    }

    /// Shows the session one chat line. Returns whether it was the
    /// safeword ("red", "OOC: stop"), but the safeword **never** stops the
    /// session, its workers or a build: it only stops her talking and drops
    /// her queued lines, which the chat layer does (`kill_switch::hush`).
    /// Ko, 2026-10-24: "Just the voice". End early, the plain stop and the
    /// kill switch are the hard stops.
    pub fn observe_chat(&self, text: &str) -> bool {
        lillith_core::access::kill_switch::is_safeword(text)
    }

    pub fn stop_reason(&self) -> Option<StopReason> {
        if let Some(r) = StopReason::from_code(self.stop.load(Ordering::SeqCst)) {
            return Some(r);
        }
        if (self.kill_switch)() {
            self.request_stop(StopReason::KillSwitch);
        } else if Instant::now() >= self.deadline {
            self.request_stop(StopReason::SessionDeadline);
        }
        StopReason::from_code(self.stop.load(Ordering::SeqCst))
    }

    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    /// Ends the session: stops it (if nothing has) and writes
    /// `DREAM\tend\t<reason>\tcard=<id>` once.
    pub fn finish(&self) -> std::io::Result<()> {
        self.stop();
        if self.ended.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let why = self.stop_reason().map_or("stop", StopReason::word);
        crate::audit::record_dream(self.audit_log.as_deref(), "end", why, &self.card)
    }
}

/// Why no supervisor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The online gate is closed (the default).
    Off(online::Off),
    /// Helper workers can't run safely here.
    Unsupported(String),
}

/// How one job ended.
#[derive(Debug, Clone, PartialEq)]
pub struct JobReport {
    pub job: String,
    pub status: JobStatus,
    pub backend: Backend,
    /// Validated candidates for the caller to save (low strength,
    /// `source=dream`); the supervisor writes nothing itself.
    pub accepted: Vec<Candidate>,
    /// Read-only, never ingested.
    pub quarantined: Vec<QItem>,
    pub quarantine_path: Option<PathBuf>,
    /// The last kill reason.
    pub kill: Option<String>,
    pub stop: Option<StopReason>,
    pub reason: Option<String>,
    pub restarts: u32,
    pub backoffs: Vec<Duration>,
    /// Every page fetched for the job (after tracking stripping).
    pub fetched: Vec<String>,
    pub pages: u32,
    pub bytes: u64,
    pub tokens: u32,
    /// Each worker's scratch dir (all wiped by the time this returns).
    pub scratch: Vec<PathBuf>,
    /// Each worker's pid (all dead by the time this returns).
    pub pids: Vec<u32>,
}

impl JobReport {
    fn new(job: &str, backend: Backend) -> Self {
        Self {
            job: job.to_string(),
            status: JobStatus::Queued,
            backend,
            accepted: Vec::new(),
            quarantined: Vec::new(),
            quarantine_path: None,
            kill: None,
            stop: None,
            reason: None,
            restarts: 0,
            backoffs: Vec::new(),
            fetched: Vec::new(),
            pages: 0,
            bytes: 0,
            tokens: 0,
            scratch: Vec::new(),
            pids: Vec::new(),
        }
    }
}

/// How one worker attempt ended.
#[derive(Debug, Clone, PartialEq, Eq)]
enum End {
    Done(String),
    Ttl(&'static str),
    Anomaly(String),
    Stopped(StopReason),
    Heartbeat,
    Crashed(String),
}

#[derive(Default)]
struct Attempt {
    candidates: Vec<Candidate>,
    fetched: Vec<String>,
    fetches: u32,
    pages: u32,
    bytes: u64,
    tokens: u32,
    out_bytes: u64,
    since_new_fact: u32,
}

/// The supervisor.
pub struct Supervisor<'f> {
    fetcher: &'f dyn Fetcher,
    command: WorkerCommand,
    bwrap: Option<PathBuf>,
    limits: WorkerLimits,
    budgets: Budgets,
    pacer: Pacer,
    deny: IdentityDenylist,
    block_hosts: Vec<String>,
    allow_http: bool,
    joblog: JobLog,
    quarantine: Quarantine,
    audit_log: Option<PathBuf>,
    breakers: Breakers,
    errors: ErrorMemory,
    sink: Option<&'f dyn ErrorSink>,
    poll: Duration,
}

impl<'f> Supervisor<'f> {
    /// A supervisor, or why helper workers are off. `names` are the people
    /// a request must never mention (Ko's name and every person name in
    /// memory), on top of this machine's user / host / time-zone names.
    pub fn new(
        cfg: &Config,
        fetcher: &'f dyn Fetcher,
        names: &[String],
        command: WorkerCommand,
    ) -> Result<Self, Refused> {
        online::gate(cfg).map_err(Refused::Off)?;
        if !cfg!(target_os = "linux") {
            return Err(Refused::Unsupported(
                "helper workers need Linux (bwrap, seccomp, PR_SET_PDEATHSIG)".into(),
            ));
        }
        let bwrap = find_bwrap();
        if command.is_sandboxed() && bwrap.is_none() {
            return Err(Refused::Unsupported(
                "bwrap is not installed; helper workers stay off (never a weaker sandbox)".into(),
            ));
        }
        let limits = WorkerLimits::from_config(&cfg.learn.workers);
        // Scratch left by a parent that died (SIGKILL, crash) goes now.
        let swept = super::scratch::sweep_stale();
        if swept > 0 {
            tracing::info!("helper workers: removed {swept} stale scratch dir(s)");
        }
        Ok(Self {
            fetcher,
            command,
            bwrap,
            breakers: Breakers::new(limits.breaker_failures, limits.breaker_open),
            limits,
            budgets: Budgets::research(&cfg.learn),
            pacer: Pacer::from_config(&cfg.learn),
            deny: IdentityDenylist::from_local().with_names(names),
            block_hosts: cfg
                .learn
                .block_hosts
                .iter()
                .map(|h| h.trim().trim_end_matches('.').to_ascii_lowercase())
                .filter(|h| !h.is_empty())
                .collect(),
            allow_http: cfg.learn.allow_http,
            joblog: JobLog::new(JobLog::default_dir()),
            quarantine: Quarantine::new(Quarantine::default_dir()),
            audit_log: lillith_core::config::audit_log_path(),
            errors: ErrorMemory::default(),
            sink: None,
            poll: Duration::from_millis(20),
        })
    }

    /// Tightens the limits (never loosens them): shorter heartbeats, fewer
    /// missed beats and restarts, a longer backoff, fewer failures before
    /// a breaker opens and a longer open time.
    pub fn with_limits(self, l: WorkerLimits) -> Self {
        let tight = self.limits.tightened(&l);
        self.set_limits(tight)
    }

    /// Tests only: any limits (fast backoff in unit tests).
    #[cfg(test)]
    pub(crate) fn with_limits_unchecked(self, l: WorkerLimits) -> Self {
        self.set_limits(l)
    }

    fn set_limits(mut self, l: WorkerLimits) -> Self {
        self.breakers = Breakers::new(l.breaker_failures, l.breaker_open);
        self.limits = l;
        self
    }

    /// Tightens the budgets (never widens them).
    pub fn with_budgets(mut self, b: &Budgets) -> Self {
        self.budgets = self.budgets.lowered(b);
        self
    }

    /// Adds identifiers to the denylist (never removes any).
    pub fn with_denylist(mut self, d: IdentityDenylist) -> Self {
        self.deny = self.deny.merged(d);
        self
    }

    /// Tests only: production always writes the default D8 job log dir
    /// (review N3-2, like Practice's setters).
    #[cfg(test)]
    pub(crate) fn with_job_log(mut self, dir: Option<PathBuf>) -> Self {
        self.joblog = JobLog::new(dir);
        self
    }

    /// Tests only: production always quarantines into the default dir
    /// (review N3-2).
    #[cfg(test)]
    pub(crate) fn with_quarantine_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.quarantine = Quarantine::new(dir);
        self
    }

    /// Tests only: production always writes `lillith_core::config::
    /// audit_log_path()`, and every `WORKER` line is fail-closed.
    #[cfg(test)]
    pub(crate) fn with_audit_log(mut self, log: Option<PathBuf>) -> Self {
        self.audit_log = log;
        self
    }

    pub fn with_error_sink(mut self, s: &'f dyn ErrorSink) -> Self {
        self.sink = Some(s);
        self
    }

    /// Tests only: pacing can't be lowered in production (§12 timing).
    #[cfg(test)]
    pub(crate) fn with_pacer(mut self, p: Pacer) -> Self {
        self.pacer = p;
        self
    }

    pub fn budgets(&self) -> &Budgets {
        &self.budgets
    }

    pub fn limits(&self) -> &WorkerLimits {
        &self.limits
    }

    pub fn errors(&self) -> &ErrorMemory {
        &self.errors
    }

    pub fn breakers(&self) -> &Breakers {
        &self.breakers
    }

    pub fn job_log(&self) -> &JobLog {
        &self.joblog
    }

    /// Runs `task` as a new job of `session`.
    pub fn run(&mut self, task: &Task, session: &Session) -> JobReport {
        self.run_as(&new_job_id(), task, session)
    }

    /// Resumes an unfinished job from the D8 log ([`JobLog::recover`]).
    /// Only within the session that started it; otherwise it is closed as
    /// `CANCELLED`.
    pub fn resume(&mut self, e: &JobEntry, session: &Session) -> JobReport {
        let task = Task::from_json(&e.task);
        match task {
            Some(t) if e.session == session.id() && !e.status.is_final() => {
                self.run_as(&e.id, &t, session)
            }
            _ => {
                let mut rep = JobReport::new(&e.id, Backend::Local);
                rep.status = JobStatus::Cancelled;
                rep.reason = Some("not resumable in this session".into());
                self.log(
                    &e.id,
                    &e.task,
                    session,
                    JobStatus::Cancelled,
                    e.attempt,
                    None,
                );
                rep
            }
        }
    }

    fn log(
        &self,
        id: &str,
        task: &Value,
        session: &Session,
        status: JobStatus,
        attempt: u32,
        result: Option<&PathBuf>,
    ) -> bool {
        let e = JobEntry {
            id: id.to_string(),
            bot: WorkerKind::Research.bot(),
            task: task.clone(),
            status,
            ts: crate::audit::utc(crate::now_s()),
            result_path: result.map(|p| p.display().to_string()),
            backend: Backend::Local.word().into(),
            session: session.id().to_string(),
            attempt,
        };
        match self.joblog.append(&e) {
            Ok(()) => true,
            Err(err) => {
                tracing::warn!("worker job log ({}): {err}", status.word());
                false
            }
        }
    }

    /// Writes one `WORKER` line carrying the session's card. Fail-closed:
    /// a failed write stops the whole session ([`StopReason::AuditFailed`])
    /// and returns `false`, so no unaudited work goes on.
    fn audit(&self, session: &Session, op: &str, reason: &str, job: &str) -> bool {
        match crate::audit::record_worker(
            self.audit_log.as_deref(),
            op,
            reason,
            job,
            Some(session.card()),
        ) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("worker audit ({op}): {e}; stopping the session");
                session.request_stop(StopReason::AuditFailed);
                false
            }
        }
    }

    /// Records a fetch / worker error. The server's text is sanitised
    /// first ([`clean_error`]) before the error memory, the sink or the
    /// worker see it.
    fn record_error(&mut self, session: &Session, key: &str, kind: &str, text: &str) {
        let text = clean_error(text, &self.deny);
        self.errors.record(key, kind, &text);
        if let Some(s) = self.sink {
            s.record_error(&text, &format!("helper worker {key}"));
        }
        if self.breakers.failure(key, Instant::now()) {
            self.audit(session, "breaker_open", key, "-");
        }
    }

    fn run_as(&mut self, id: &str, task: &Task, session: &Session) -> JobReport {
        let mut rep = JobReport::new(id, Backend::Local);
        let tj = task.to_json();
        let finish = |me: &Self, mut rep: JobReport, status: JobStatus, why: String| {
            rep.status = status;
            rep.reason = Some(why.clone());
            me.log(
                &rep.job,
                &tj,
                session,
                status,
                rep.restarts,
                rep.quarantine_path.as_ref(),
            );
            me.audit(
                session,
                "end",
                &format!("{} {why}", status.word()),
                &rep.job,
            );
            rep
        };

        // 1. Gates: kind, stop, topic, addresses, breakers, Tor.
        if !task.kind.implemented() {
            return finish(
                self,
                rep,
                JobStatus::Refused,
                format!("{} workers are not implemented", task.kind.word()),
            );
        }
        if let Some(r) = session.stop_reason() {
            rep.stop = Some(r);
            return finish(
                self,
                rep,
                JobStatus::Cancelled,
                format!("stop: {}", r.word()),
            );
        }
        // The safeword is not a research topic; refusing this one task is
        // all it does (the session and its other workers go on).
        if lillith_memory::is_safeword(&task.topic) {
            return finish(
                self,
                rep,
                JobStatus::Refused,
                "the safeword is not a research topic".into(),
            );
        }
        if task.topic.trim().is_empty()
            || task.urls.is_empty()
            || task.urls.len() > online::MAX_URLS
        {
            return finish(
                self,
                rep,
                JobStatus::Refused,
                "a task needs a topic and 1–20 addresses".into(),
            );
        }
        let vetter = FetchPolicy {
            hosts: Vec::new(),
            deny: self.deny.clone(),
            block_hosts: self.block_hosts.clone(),
            allow_http: self.allow_http,
            canary: String::new(),
        };
        let mut urls: Vec<String> = Vec::new();
        let mut hosts: Vec<String> = Vec::new();
        let now = Instant::now();
        for raw in &task.urls {
            let u = match Url::parse_entry(raw)
                .map_err(|e| e.to_string())
                .and_then(|u| vetter.vet_planned(&u).map_err(|v| v.0))
            {
                Ok(u) => u,
                Err(why) => {
                    return finish(self, rep, JobStatus::Refused, format!("{raw:?}: {why}"))
                }
            };
            if !self.breakers.would_allow(&domain_key(u.host()), now) {
                continue;
            }
            if !hosts.iter().any(|h| h == u.host()) {
                hosts.push(u.host().to_string());
            }
            let s = u.to_string();
            if !urls.contains(&s) {
                urls.push(s);
            }
        }
        if urls.is_empty() {
            return finish(
                self,
                rep,
                JobStatus::Refused,
                "the circuit breaker is open for every planned site".into(),
            );
        }
        if !self
            .breakers
            .would_allow(&source_key(Backend::Local.word()), now)
        {
            return finish(
                self,
                rep,
                JobStatus::Refused,
                "the circuit breaker is open for local workers".into(),
            );
        }
        if let Err(why) = self.fetcher.available() {
            return finish(
                self,
                rep,
                JobStatus::Offline,
                format!("online research stays off: {why}; offline-only learning continues (Tor is never installed for you)"),
            );
        }
        // Fail-closed: no `WORKER start` line under the session's card, no
        // worker (written before the first spawn).
        if !self.audit(session, "start", task.kind.word(), id) {
            return finish(
                self,
                rep,
                JobStatus::Refused,
                "the audit log can't be written; nothing was started".into(),
            );
        }
        // Fail-closed: no job log line, no worker.
        if !self.log(id, &tj, session, JobStatus::Queued, 0, None) {
            return finish(
                self,
                rep,
                JobStatus::Refused,
                "the job log can't be written; nothing was started".into(),
            );
        }

        // 2. Attempts: a fresh worker each, backoff between them.
        let mut last = End::Crashed("never started".into());
        let mut kept: Vec<Candidate> = Vec::new();
        for attempt in 0..=self.limits.max_restarts {
            if attempt > 0 {
                let d = backoff_delay(self.limits.backoff_base, attempt);
                rep.backoffs.push(d);
                self.log(id, &tj, session, JobStatus::Restarting, attempt, None);
                if let Some(r) = sleep_unless_stopped(d, session) {
                    last = End::Stopped(r);
                    break;
                }
                rep.restarts += 1;
            }
            if !self.log(id, &tj, session, JobStatus::Running, attempt, None) {
                // Fail-closed: the D8 log is how a crash is recovered.
                session.request_stop(StopReason::AuditFailed);
                last = End::Stopped(StopReason::AuditFailed);
                break;
            }
            let mut st = Attempt::default();
            let end = self.attempt(id, task, &urls, &hosts, session, &mut rep, &mut st);
            rep.fetched.extend(st.fetched.iter().cloned());
            rep.pages += st.pages;
            rep.bytes += st.bytes;
            rep.tokens += st.tokens;
            match &end {
                End::Ttl(what) => {
                    let r = format!("ttl:{what}");
                    self.audit(session, "kill", &r, id);
                    rep.kill = Some(r);
                }
                End::Anomaly(why) => {
                    self.audit(session, "kill", why, id);
                    rep.kill = Some(why.clone());
                }
                End::Heartbeat => {
                    self.audit(session, "kill", "heartbeat", id);
                    rep.kill = Some("heartbeat".into());
                }
                End::Stopped(r) => {
                    self.audit(session, "kill", &format!("stop:{}", r.word()), id);
                    rep.kill = Some(format!("stop:{}", r.word()));
                }
                End::Crashed(why) => {
                    self.audit(session, "crash", why, id);
                }
                End::Done(_) => {}
            }
            let retry = matches!(end, End::Heartbeat | End::Crashed(_));
            if retry {
                let why = match &end {
                    End::Crashed(w) => w.clone(),
                    _ => "missed heartbeats".into(),
                };
                self.record_error(session, &source_key(Backend::Local.word()), "worker", &why);
            } else {
                self.breakers.success(&source_key(Backend::Local.word()));
            }
            last = end;
            if !retry {
                kept = st.candidates;
                break;
            }
        }

        // 3. Results.
        match last {
            End::Done(why) => {
                self.sort(&mut rep, kept);
                self.store_quarantine(&mut rep, None);
                finish(self, rep, JobStatus::Done, why)
            }
            End::Ttl(what) => {
                self.sort(&mut rep, kept);
                self.store_quarantine(&mut rep, None);
                finish(
                    self,
                    rep,
                    JobStatus::Partial,
                    format!("budget reached: {what}"),
                )
            }
            End::Anomaly(why) => {
                rep.quarantined = kept
                    .into_iter()
                    .map(|c| QItem {
                        title: c.title,
                        text: c.text,
                        url: c.url,
                        reason: format!("worker killed: {why}"),
                    })
                    .collect();
                self.store_quarantine(&mut rep, Some(&why));
                finish(self, rep, JobStatus::Killed, why)
            }
            End::Stopped(r) => {
                // Stop and discard: nothing kept, nothing quarantined.
                rep.stop = Some(r);
                finish(
                    self,
                    rep,
                    JobStatus::Cancelled,
                    format!("stop: {}; in-flight work discarded", r.word()),
                )
            }
            End::Heartbeat | End::Crashed(_) => {
                let why = format!(
                    "out of restarts ({} of {})",
                    rep.restarts, self.limits.max_restarts
                );
                finish(self, rep, JobStatus::Failed, why)
            }
        }
    }

    fn sort(&self, rep: &mut JobReport, kept: Vec<Candidate>) {
        for c in kept {
            match validate(c, &rep.fetched, self.budgets.fact_bytes) {
                Verdict::Accept(c) => rep.accepted.push(c),
                Verdict::Quarantine(c, why) => rep.quarantined.push(QItem {
                    title: c.title,
                    text: c.text,
                    url: c.url,
                    reason: why,
                }),
            }
        }
    }

    fn store_quarantine(&self, rep: &mut JobReport, kill: Option<&str>) {
        if rep.quarantined.is_empty() && kill.is_none() {
            return;
        }
        match self
            .quarantine
            .store(&rep.job, rep.backend.label(), kill, &rep.quarantined)
        {
            Ok(p) => rep.quarantine_path = Some(p),
            Err(e) => {
                tracing::warn!("worker quarantine: {e}; the items are dropped, never ingested")
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn attempt(
        &mut self,
        id: &str,
        task: &Task,
        urls: &[String],
        hosts: &[String],
        session: &Session,
        rep: &mut JobReport,
        st: &mut Attempt,
    ) -> End {
        let scratch = match Scratch::new(id) {
            Ok(s) => s,
            Err(e) => return End::Crashed(format!("scratch: {e}")),
        };
        rep.scratch.push(scratch.path().to_path_buf());
        // Fresh per worker: its own Tor circuit (SOCKS user = password =
        // random token, IsolateSOCKSAuth) and its own canary.
        let circuit = super::random_token();
        let canary = format!("cnry-{}", &super::random_token()[..16]);
        let policy = FetchPolicy {
            hosts: hosts.to_vec(),
            deny: self.deny.clone(),
            block_hosts: self.block_hosts.clone(),
            allow_http: self.allow_http,
            canary: canary.clone(),
        };
        // The spawn line goes first, fail-closed: no line, no process.
        if !self.audit(session, "spawn", task.kind.word(), id) {
            scratch.wipe();
            return End::Stopped(StopReason::AuditFailed);
        }
        let mut p = match WorkerProc::spawn(&self.command, scratch.path(), self.bwrap.as_deref()) {
            Ok(p) => p,
            Err(e) => return End::Crashed(format!("spawn: {e}")),
        };
        rep.pids.push(p.id());
        let b = &self.budgets;
        p.send(&json!({
            "t": "task",
            "job": id,
            "kind": task.kind.word(),
            "topic": task.topic,
            "urls": urls,
            "hosts": hosts,
            "canary": canary,
            "heartbeat_ms": self.limits.heartbeat.as_millis() as u64,
            "budgets": {
                "wall_s": b.wall.as_secs(), "fetches": b.fetches, "pages": b.pages,
                "bytes": b.bytes, "tokens": b.tokens, "facts": b.facts,
            },
        }));
        let ttl_end = Instant::now() + self.budgets.wall;
        let mut last_beat = Instant::now();
        let end = loop {
            if let Some(r) = session.stop_reason() {
                break End::Stopped(r);
            }
            let now = Instant::now();
            if now >= ttl_end {
                break End::Ttl("wall");
            }
            if now.saturating_duration_since(last_beat) >= self.limits.heartbeat_deadline() {
                break End::Heartbeat;
            }
            let line = match p.recv(self.poll) {
                None => continue,
                Some(Out::Eof) => {
                    let status = p
                        .exit_status()
                        .map_or_else(|| "?".to_string(), |s| s.to_string());
                    break End::Crashed(format!("worker exited before done ({status})"));
                }
                Some(Out::Oversized) => {
                    break End::Anomaly("oversized output: a line over 64 KiB".into())
                }
                Some(Out::Line(l)) => l,
            };
            st.out_bytes += line.len() as u64 + 1;
            if st.out_bytes > self.budgets.output_bytes {
                break End::Anomaly("oversized output: over the output budget".into());
            }
            if line.contains(&canary) {
                break End::Anomaly("canary: the planted canary showed up in its output".into());
            }
            if let Some(a) = asks_for_action(&line) {
                break End::Anomaly(format!("injection: output asks to act ({a})"));
            }
            let msg = match protocol::parse(&line) {
                Ok(m) => m,
                Err(e) => break End::Anomaly(e),
            };
            match msg {
                Msg::Heartbeat => last_beat = Instant::now(),
                Msg::Done => break End::Done("worker finished".into()),
                Msg::Tokens(n) => {
                    st.tokens = st.tokens.saturating_add(n);
                    if st.tokens > self.budgets.tokens {
                        break End::Ttl("tokens");
                    }
                }
                Msg::Fact { title, text, url } => {
                    if text.len() > self.budgets.fact_bytes || title.len() > 1024 {
                        break End::Anomaly("oversized output: a fact over the limit".into());
                    }
                    st.candidates.push(Candidate {
                        job: id.to_string(),
                        title,
                        text,
                        url,
                        backend: Backend::Local,
                    });
                    st.since_new_fact = 0;
                    if st.candidates.len() as u32 >= self.budgets.facts {
                        break End::Done("enough facts".into());
                    }
                }
                Msg::Get { url } => {
                    if st.since_new_fact >= EARLY_STOP_PAGES {
                        break End::Done(format!(
                            "no new facts in the last {EARLY_STOP_PAGES} pages"
                        ));
                    }
                    st.fetches += 1;
                    if st.fetches > self.budgets.fetches {
                        break End::Ttl("fetches");
                    }
                    let u = match policy.check(&url) {
                        Ok(u) => u,
                        Err(v) => break End::Anomaly(v.0),
                    };
                    if st.pages >= self.budgets.pages {
                        break End::Ttl("pages");
                    }
                    if st.bytes >= self.budgets.bytes {
                        break End::Ttl("bytes");
                    }
                    let key = domain_key(u.host());
                    if !self.breakers.allow(&key, Instant::now()) {
                        p.send(&json!({"t": "error", "url": u.to_string(), "why": "circuit breaker open"}));
                        continue;
                    }
                    let room = self.budgets.bytes - st.bytes;
                    let halt = || session.stop_reason().is_some() || Instant::now() >= ttl_end;
                    let fetcher = self.fetcher;
                    let got = self
                        .pacer
                        .run(&halt, || fetcher.get(&u, &circuit, room, &halt));
                    // The worker was waiting on the parent: not a missed beat.
                    last_beat = Instant::now();
                    let page = match got {
                        None | Some(Err(FetchError::Stopped)) => match session.stop_reason() {
                            Some(r) => break End::Stopped(r),
                            None => break End::Ttl("wall"),
                        },
                        Some(Err(e)) => {
                            let kind = match &e {
                                FetchError::TorUnavailable(_) => "tor",
                                FetchError::Tls(_) => "tls",
                                FetchError::Socks(_) => "socks",
                                FetchError::Io(_) => "io",
                                FetchError::Http(_) => "http",
                                FetchError::Stopped => "stopped",
                            };
                            let why = clean_error(&e.to_string(), &self.deny);
                            self.record_error(session, &key, kind, &why);
                            p.send(&json!({"t": "error", "url": u.to_string(), "why": why}));
                            continue;
                        }
                        Some(Ok(page)) => page,
                    };
                    self.breakers.success(&key);
                    let mut body = page.body;
                    body.truncate(room as usize);
                    st.bytes += body.len() as u64;
                    if (300..400).contains(&page.status) {
                        let next = page.location.as_deref().map(|l| u.join(l));
                        let msg = match next {
                            // `check_redirect`: an https → http downgrade is
                            // refused even with `allow_http` on.
                            Some(Ok(n)) => match policy.check_redirect(&u, &n.to_string()) {
                                Ok(n) => {
                                    json!({"t": "redirect", "url": u.to_string(), "to": n.to_string()})
                                }
                                Err(v) => {
                                    json!({"t": "error", "url": u.to_string(), "why": format!("redirect refused: {}", v.0)})
                                }
                            },
                            _ => json!({"t": "error", "url": u.to_string(), "why": "bad redirect"}),
                        };
                        p.send(&msg);
                        continue;
                    }
                    if !(200..300).contains(&page.status) {
                        p.send(&json!({"t": "error", "url": u.to_string(), "why": format!("HTTP {}", page.status)}));
                        continue;
                    }
                    if let Err(why) = online::text::content_allowed(&page.content_type, &body) {
                        p.send(&json!({"t": "error", "url": u.to_string(), "why": why}));
                        continue;
                    }
                    // Extracted text only; the raw body is dropped here.
                    let (title, text) = online::text::extract(&page.content_type, &body);
                    drop(body);
                    let text = crate::paths::drop_secret_lines(&text);
                    let text: String = cut(&text, MAX_PAGE_TEXT);
                    st.pages += 1;
                    st.since_new_fact += 1;
                    let us = u.to_string();
                    if !st.fetched.contains(&us) {
                        st.fetched.push(us.clone());
                    }
                    p.send(&json!({"t": "page", "url": us, "title": title.unwrap_or_default(), "text": text}));
                }
            }
        };
        // Whatever happened: the whole group dies, scratch is wiped.
        p.kill();
        scratch.wipe();
        end
    }
}

/// Longest error text kept from a server or the network stack.
pub const MAX_ERROR_TEXT: usize = 200;

/// A server's / the network stack's error text made safe to log, store and
/// hand to a worker: secret-looking lines dropped, the first line only,
/// control characters replaced, withheld whole if it names anything on
/// `deny` (an identifier or a local path), cut to
/// [`MAX_ERROR_TEXT`] bytes.
pub fn clean_error(text: &str, deny: &IdentityDenylist) -> String {
    let text = crate::paths::drop_secret_lines(text);
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let out: String = line
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if deny.hit(&out).is_some() {
        return "[error text withheld: it named an identifier or a local path]".into();
    }
    let out = cut(out.trim(), MAX_ERROR_TEXT);
    if out.is_empty() {
        "error".into()
    } else {
        out
    }
}

fn cut(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut i = max;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    s[..i].to_string()
}

/// Sleeps `d` unless the session stops first (then its reason).
fn sleep_unless_stopped(d: Duration, s: &Session) -> Option<StopReason> {
    let until = Instant::now() + d;
    loop {
        if let Some(r) = s.stop_reason() {
            return Some(r);
        }
        let now = Instant::now();
        if now >= until {
            return None;
        }
        std::thread::sleep((until - now).min(Duration::from_millis(20)));
    }
}
