//! The dream-session card (docs/SELF_LEARNING.md §11 item 1, §12): the only
//! way a helper-worker [`Session`](super::Session) comes to exist.
//!
//! 1. [`propose_session`] checks the online gate and the duration (30 min
//!    to 8 h and inside `[learn.dream] min_minutes..=max_minutes`; outside
//!    is rejected, never clamped), writes the `PROPOSED` line
//!    (`dream_session` kind, fail-closed: no line, no card) and shows a
//!    [`PendingAction::DreamSession`] card.
//! 2. Only [`SessionAnswer::Click`] with [`CardAnswer::Yes`] approves; a
//!    typed or spoken yes is refused on the record. The kill switch firing
//!    while the card is open cancels it. The `CONFIRM` line is fail-closed
//!    too: no `CONFIRM` line, no approval.
//! 3. The [`SessionApproval`] (private fields, not `Clone`) is consumed by
//!    [`Session::from_card`](super::Session::from_card), which writes `DREAM\tstart` (fail-closed)
//!    before the session exists. Every `WORKER` line of the session then
//!    carries `card=<id>`, and the "also use my TaskCompletion bots" tick
//!    (a separate option, unticked by default, only offered with
//!    `[learn.dream] external_bots = true`) comes from this same card.

use super::{Budgets, StopReason};
use crate::online;
use lillith_core::card_process::{CardAnswer, CardText};
use lillith_core::config::Config;
use lillith_core::soul_contract::PendingAction;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Shortest session the card may offer (§11.1).
pub const MIN_MINUTES: u64 = 30;
/// Longest session the card may offer (§11.1: 8 h).
pub const MAX_MINUTES: u64 = 480;

/// What the dream-session card shows and the audit records.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionCardRequest {
    pub card: String,
    pub session: String,
    pub minutes: u64,
    /// The end time (UTC; audit and binding use this). The card shows it in
    /// the system local time zone with a zone label, the stamp after it.
    /// The session ends at exactly this time: its deadline is fixed when
    /// the card appears, not at the click (review N3-1), so a slow click
    /// shortens the session instead of moving its end past what the card
    /// said.
    pub deadline_utc: String,
    /// The budgets shown on the card.
    pub budgets: String,
    /// Whether the separate "also use my TaskCompletion bots" option is
    /// on the card (unticked).
    pub external_offered: bool,
    /// `PendingAction::DreamSession { .. }`.
    pub action: PendingAction,
    pub text: CardText,
}

/// How the UI answered. Only a click counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionAnswer {
    /// The card's own answer, and whether Ko ticked the external option
    /// (ignored unless it was offered).
    Click {
        answer: CardAnswer,
        external_ticked: bool,
    },
    /// A typed chat reply: never approves.
    Typed(String),
    /// A spoken reply: never approves.
    Spoken(String),
}

/// The card UI. `stop()` turns true when the kill switch fires: the card
/// must close.
pub trait SessionConsent {
    fn ask(&self, req: &SessionCardRequest, stop: &dyn Fn() -> bool) -> SessionAnswer;
}

/// The existing click card ([`online::Consent`], e.g.
/// [`online::CardProcessConsent`]) as a [`SessionConsent`]. That card has
/// no tick box, so the external option always stays unticked through it.
pub struct ClickSessionCard<'a>(pub &'a dyn online::Consent);

impl SessionConsent for ClickSessionCard<'_> {
    fn ask(&self, req: &SessionCardRequest, stop: &dyn Fn() -> bool) -> SessionAnswer {
        SessionAnswer::Click {
            answer: self.0.ask(&req.text, stop),
            external_ticked: false,
        }
    }
}

/// Proof of one mouse click on Yes on one dream-session card. Made only by
/// [`propose_session`]; not `Clone`; consumed by [`Session::from_card`](super::Session::from_card).
#[derive(Debug, PartialEq, Eq)]
pub struct SessionApproval {
    card: String,
    session: String,
    minutes: u64,
    deadline: Instant,
    deadline_utc: String,
    external: bool,
    audit_log: Option<PathBuf>,
    epoch: u64,
}

impl SessionApproval {
    pub fn card(&self) -> &str {
        &self.card
    }
    pub fn session(&self) -> &str {
        &self.session
    }
    pub fn minutes(&self) -> u64 {
        self.minutes
    }
    /// The end time the card showed (UTC); the session's deadline is that
    /// same moment.
    pub fn deadline_utc(&self) -> &str {
        &self.deadline_utc
    }
    /// Whether the external option was offered and ticked.
    pub fn external(&self) -> bool {
        self.external
    }

    #[allow(clippy::type_complexity)]
    pub(crate) fn into_parts(self) -> (String, String, u64, Instant, bool, Option<PathBuf>, u64) {
        (
            self.card,
            self.session,
            self.minutes,
            self.deadline,
            self.external,
            self.audit_log,
            self.epoch,
        )
    }
}

/// Why no session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DreamRefused {
    /// The online gate is closed.
    Off(online::Off),
    /// Outside 30 min–8 h or `[learn.dream]`'s range (never clamped).
    Duration { minutes: u64, min: u64, max: u64 },
    /// The audit log can't be written: fail-closed.
    Audit(String),
    /// The card said No / timed out / closed / had no display, or a typed
    /// or spoken yes was offered.
    NotApproved(String),
    /// The kill switch fired while the card was open.
    Stopped(StopReason),
}

impl std::fmt::Display for DreamRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DreamRefused::Off(o) => write!(f, "online learning is off: {o}"),
            DreamRefused::Duration { minutes, min, max } => {
                write!(
                    f,
                    "{minutes} min is outside {min}–{max} min; nothing started"
                )
            }
            DreamRefused::Audit(e) => {
                write!(f, "the audit log can't be written ({e}); nothing started")
            }
            DreamRefused::NotApproved(w) => write!(f, "not approved: {w}"),
            DreamRefused::Stopped(r) => write!(f, "stopped ({}) before it started", r.word()),
        }
    }
}

/// The allowed duration range: 30–480 min, narrowed by `[learn.dream]`.
pub fn duration_range(cfg: &Config) -> (u64, u64) {
    let d = &cfg.learn.dream;
    (
        u64::from(d.min_minutes).max(MIN_MINUTES),
        u64::from(d.max_minutes).min(MAX_MINUTES),
    )
}

/// Shows the dream-session card for `minutes` and returns the approval on
/// a click on Yes (see the module docs). `audit_log` is where the card,
/// `DREAM` and `WORKER` lines go (`None` refuses: fail-closed).
pub fn propose_session(
    cfg: &Config,
    minutes: u64,
    consent: &dyn SessionConsent,
    audit_log: Option<PathBuf>,
) -> Result<SessionApproval, DreamRefused> {
    online::gate(cfg).map_err(DreamRefused::Off)?;
    let (min, max) = duration_range(cfg);
    if minutes < min || minutes > max {
        return Err(DreamRefused::Duration { minutes, min, max });
    }
    let epoch = lillith_core::access::kill_switch::epoch();
    let stopped = || lillith_core::access::kill_switch::stopped_since(epoch);
    let tok = super::random_token();
    let card = format!("dream-{}", &tok[..16]);
    let session = format!("s-{}", &tok[16..32]);
    // One reference for both the shown end time and the real deadline
    // (review N3-1): taken once, when the card is made.
    let length = Duration::from_secs(minutes * 60);
    let shown_at = Instant::now();
    let deadline = shown_at + length;
    let deadline_utc = crate::audit::utc(crate::now_s() + length.as_secs_f64());
    let b = Budgets::research(&cfg.learn);
    let budgets = format!(
        "{} page{} and {} KiB per trip, {} min per worker, at most {} worker{}, one fetch every {} s or more",
        b.pages,
        if b.pages == 1 { "" } else { "s" },
        b.bytes.div_ceil(1024),
        b.wall.as_secs() / 60,
        cfg.learn.dream.max_workers.max(1),
        if cfg.learn.dream.max_workers == 1 { "" } else { "s" },
        cfg.learn.dream.fetch_interval_s.max(1),
    );
    let external_offered = cfg.learn.dream.external_bots;
    let fields = |extra: &[String]| -> Vec<String> {
        let mut f = vec![
            format!("session={session}"),
            format!("minutes={minutes}"),
            format!("deadline={deadline_utc}"),
            format!("external_offered={external_offered}"),
        ];
        f.extend(extra.iter().cloned());
        f
    };
    let log = audit_log.as_deref();
    // PROPOSED first; fail-closed: no line, no card.
    crate::audit::record_dream_card(log, "PROPOSED", &card, &fields(&[]))
        .map_err(|e| DreamRefused::Audit(e.to_string()))?;
    let action = PendingAction::DreamSession {
        id: session.clone(),
        minutes,
        deadline: deadline_utc.clone(),
        budgets: budgets.clone(),
        card: card.clone(),
        external_offered,
    };
    let pr = action.prompt();
    let req = SessionCardRequest {
        card: card.clone(),
        session: session.clone(),
        minutes,
        deadline_utc: deadline_utc.clone(),
        budgets,
        external_offered,
        text: CardText {
            title: pr.title.to_string(),
            summary: pr.summary,
            target: String::new(),
            detail: pr.detail.unwrap_or_default(),
        },
        action,
    };
    let answer = consent.ask(&req, &stopped);
    let best_effort = |state: &str, why: &str| {
        if let Err(e) =
            crate::audit::record_dream_card(log, state, &card, &fields(&[format!("why={why}")]))
        {
            tracing::warn!("dream card audit ({state}): {e}");
        }
    };
    if stopped() {
        best_effort("CANCELLED", StopReason::KillSwitch.word());
        return Err(DreamRefused::Stopped(StopReason::KillSwitch));
    }
    let why = match answer {
        SessionAnswer::Click {
            answer: CardAnswer::Yes,
            external_ticked,
        } => {
            let external = external_offered && external_ticked;
            // CONFIRM is fail-closed too: no line, no approval.
            crate::audit::record_dream_card(
                log,
                "CONFIRM",
                &card,
                &fields(&[format!("external={external}")]),
            )
            .map_err(|e| DreamRefused::Audit(e.to_string()))?;
            return Ok(SessionApproval {
                card,
                session,
                minutes,
                deadline,
                deadline_utc,
                external,
                audit_log,
                epoch,
            });
        }
        SessionAnswer::Click {
            answer: CardAnswer::No,
            ..
        } => ("DENY", "declined on the card"),
        SessionAnswer::Click {
            answer: CardAnswer::TimedOut,
            ..
        } => ("TIMEOUT", "no answer on the card"),
        SessionAnswer::Click {
            answer: CardAnswer::Cancelled,
            ..
        } => ("CANCELLED", "the card was closed"),
        SessionAnswer::Click {
            answer: CardAnswer::NoUi,
            ..
        } => (
            "NO_UI",
            "no display for the card; a typed or spoken yes never counts",
        ),
        SessionAnswer::Typed(_) => ("DENY", "a typed yes never counts; only a click"),
        SessionAnswer::Spoken(_) => ("DENY", "a spoken yes never counts; only a click"),
    };
    best_effort(why.0, why.1);
    Err(DreamRefused::NotApproved(why.1.to_string()))
}
