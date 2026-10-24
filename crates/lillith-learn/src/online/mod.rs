//! User-gated online research (docs/SELF_LEARNING.md §5, §7, §12),
//! milestone P5, first slice: the trip engine and its gates.
//!
//! A trip only ever starts because Ko asked for one (chat, the panel); the
//! idle learner never starts one (it has no path to this module). Every
//! trip:
//!
//! 1. is off unless `[learn] online` (default **false**), `[learn]
//!    enabled` and `[memory] enabled` are all on;
//! 2. is refused (after its `PROPOSED` line, see 3) when the local Tor
//!    SOCKS port doesn't answer (Tor is never installed or started here),
//!    and before anything else when an address is not `https://`
//!    (plain `http://` only with `[learn] allow_http = true`), points at Ko's own network, is on `[learn] block_hosts`, or carries a
//!    local identifier (§12 denylist); tracking parameters are stripped
//!    before anything is shown or sent;
//! 3. audits `PROPOSED` (purpose, every address, the caps) before the Tor
//!    probe and before any byte leaves (fail-closed: no line, no trip),
//!    then asks on the Soul Contract card
//!    ([`PendingAction::OnlineResearch`]): only a mouse click on Yes
//!    approves (`CONFIRM`); No is `DENIED`, no answer `TIMEOUT`, a closed
//!    card or the safeword `CANCELLED`, no display `NO_UI` (a typed or
//!    spoken yes never counts);
//! 4. fetches only the approved addresses, one read-only GET per hop on
//!    the trip's own Tor circuit; a redirect to a host that is not on the
//!    list is recorded and stops the trip; `max_pages` / `max_bytes` are
//!    hard caps (overflow is not fetched); the safeword stops it within
//!    about a second, even mid-request, and discards everything;
//! 5. keeps fetched text only as untrusted `web` notes under
//!    `knowledge/web/` (with `url` and `trip_id`), after the §7 ingest
//!    filter; never as instructions;
//! 6. reports where she went, what she saved and how it ended (`DONE` /
//!    `PARTIAL` / `CANCELLED` / `DENIED` / `TIMEOUT`), and audits it.

pub mod fetch;
pub mod identity;
pub mod text;
pub mod url;

pub use fetch::{FetchError, FetchedPage, Fetcher, TorFetcher};
pub use identity::IdentityDenylist;
pub use url::{Url, UrlError};

use crate::audit;
use crate::store::{Note, NoteStore, WebSource};
use lillith_core::card_process::{CardAnswer, CardText};
use lillith_core::config::Config;
use lillith_core::soul_contract::PendingAction;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Most addresses on one card.
pub const MAX_URLS: usize = 20;
/// Most redirect hops followed from one address.
pub const MAX_REDIRECTS: usize = 5;
/// Whole-trip wall clock (P5 budget: a trip ≤ 60 s).
pub const MAX_WALL: Duration = Duration::from_secs(60);
/// How long the card waits for a click.
pub const CARD_TIMEOUT: Duration = Duration::from_secs(120);
/// The topic folder web notes go to.
pub const WEB_TOPIC: &str = "web";
/// Confidence of a fresh web note (untrusted, unquizzed).
pub const WEB_CONFIDENCE: f64 = 0.3;

static TRIPS_STARTED: AtomicUsize = AtomicUsize::new(0);

/// How many trips this process has started (passed the config gate);
/// tests use it to prove the idle path never starts one.
pub fn trips_started() -> usize {
    TRIPS_STARTED.load(Ordering::SeqCst)
}

/// Why online research is off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Off {
    /// `[learn] online = false` (the shipped default).
    OnlineOff,
    /// `[learn] enabled = false`.
    LearnOff,
    /// `[memory] enabled = false`.
    MemoryOff,
}

impl std::fmt::Display for Off {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Off::OnlineOff => "online research is off ([learn] online = false)",
            Off::LearnOff => "learning is off ([learn] enabled = false)",
            Off::MemoryOff => "memory is off ([memory] enabled = false)",
        })
    }
}

/// `Ok` when the config allows online research at all.
pub fn gate(cfg: &Config) -> Result<(), Off> {
    if !cfg.memory.enabled {
        return Err(Off::MemoryOff);
    }
    if !cfg.learn.enabled {
        return Err(Off::LearnOff);
    }
    if !cfg.learn.online {
        return Err(Off::OnlineOff);
    }
    Ok(())
}

/// What Ko asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TripRequest {
    /// One short sentence.
    pub purpose: String,
    /// Exact URLs, or bare domains (`example.org` = `https://example.org/`).
    pub urls: Vec<String>,
    /// Lower than the config cap only (never higher).
    pub max_pages: Option<u32>,
    pub max_bytes: Option<u64>,
}

/// Shows the Soul Contract card and returns its answer. Only
/// [`CardAnswer::Yes`] (a mouse click on Yes) approves.
pub trait Consent {
    fn ask(&self, text: &CardText, stop: &dyn Fn() -> bool) -> CardAnswer;
}

/// The real card: `lillith contract-card` in its own process through
/// [`lillith_core::card_process::ask_until`], the same path the console
/// chat's `contract_consent::show_card` uses. The safeword ("red" /
/// "OOC: stop") or `stop()` kills the card process: `Cancelled`, even if
/// Yes was clicked at that moment. No display: `NoUi`.
#[derive(Debug, Clone)]
pub struct CardProcessConsent {
    pub exe: PathBuf,
    pub timeout: Duration,
}

impl CardProcessConsent {
    /// The running binary's card.
    pub fn current_exe() -> Option<Self> {
        Some(Self {
            exe: std::env::current_exe().ok()?,
            timeout: CARD_TIMEOUT,
        })
    }
}

impl Consent for CardProcessConsent {
    fn ask(&self, text: &CardText, stop: &dyn Fn() -> bool) -> CardAnswer {
        use lillith_core::access::kill_switch::{hush_epoch, hushed_since};
        let start = hush_epoch();
        let halted = || stop() || hushed_since(start);
        let a = lillith_core::card_process::ask_until(&self.exe, text, self.timeout, halted);
        if halted() {
            CardAnswer::Cancelled
        } else {
            a
        }
    }
}

/// A `stop()` for [`OnlineResearch::run`] that turns true once the
/// safeword (the process-wide hush) or a stop fires.
pub fn safeword_stop() -> impl Fn() -> bool {
    let start = lillith_core::access::kill_switch::hush_epoch();
    move || lillith_core::access::kill_switch::hushed_since(start)
}

/// How a trip ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TripState {
    /// Never proposed (a gate said no; nothing shown, nothing sent).
    Refused,
    /// No on the card.
    Denied,
    /// No card could be shown; nothing was sent.
    NoUi,
    /// The card or the trip ran out of time.
    Timeout,
    /// Card closed, or the safeword: stopped and discarded.
    Cancelled,
    /// Stopped early (a cap, an unlisted redirect, an error, a refused
    /// body): what was fetched before is kept.
    Partial,
    Done,
}

impl TripState {
    /// The audit word.
    pub fn word(self) -> &'static str {
        match self {
            TripState::Refused => "REFUSED",
            TripState::Denied => "DENIED",
            TripState::NoUi => "NO_UI",
            TripState::Timeout => "TIMEOUT",
            TripState::Cancelled => "CANCELLED",
            TripState::Partial => "PARTIAL",
            TripState::Done => "DONE",
        }
    }
}

/// One request she made (or refused to make).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Visit {
    pub url: String,
    /// HTTP status; `None` when it was not requested or failed to connect.
    pub status: Option<u16>,
    /// `redirect to …`, `not fetched: …`, `refused: …`, an error, or empty.
    pub note: String,
}

/// One saved note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    pub id: String,
    /// Relative to the knowledge folder (`web/n-….md`).
    pub path: String,
    pub url: String,
    /// At most one line.
    pub summary: String,
}

/// The after-trip report (§5.3): where she went, what she saved, the end
/// state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TripReport {
    pub trip_id: String,
    pub card: String,
    pub state: TripState,
    pub purpose: String,
    /// The approved addresses (tracking stripped), as shown on the card.
    pub approved: Vec<String>,
    pub visits: Vec<Visit>,
    pub saved: Vec<Saved>,
    /// `(url, why)` for fetched pages that were not kept.
    pub dropped: Vec<(String, String)>,
    /// Why it was refused or stopped early.
    pub reason: Option<String>,
    pub pages: u32,
    pub bytes: u64,
}

impl TripReport {
    /// Every URL actually requested, redirects included.
    pub fn fetched(&self) -> Vec<&str> {
        self.visits
            .iter()
            .filter(|v| v.status.is_some())
            .map(|v| v.url.as_str())
            .collect()
    }

    /// For the chat / Knowledge panel.
    pub fn text(&self) -> String {
        let mut o = format!(
            "Online research {} (trip {}, card {})\npurpose: {}\n",
            self.state.word(),
            self.trip_id,
            self.card,
            self.purpose
        );
        if let Some(r) = &self.reason {
            o.push_str(&format!("why: {r}\n"));
        }
        o.push_str("where I went:\n");
        if self.visits.is_empty() {
            o.push_str("  nowhere (nothing was sent)\n");
        }
        for v in &self.visits {
            let st = v.status.map_or("---".to_string(), |s| s.to_string());
            o.push_str(&format!("  {st} {}", v.url));
            if !v.note.is_empty() {
                o.push_str(&format!(" ({})", v.note));
            }
            o.push('\n');
        }
        o.push_str("what I saved:\n");
        if self.saved.is_empty() {
            o.push_str("  nothing\n");
        }
        for s in &self.saved {
            o.push_str(&format!("  knowledge/{} — {}\n", s.path, s.summary));
        }
        for (u, why) in &self.dropped {
            o.push_str(&format!("  not kept: {u} ({why})\n"));
        }
        o.push_str(&format!(
            "{} request(s), {} bytes\n",
            self.pages, self.bytes
        ));
        o
    }
}

/// A fetched body waiting for the ingest filter.
struct Page {
    url: Url,
    content_type: String,
    body: Vec<u8>,
}

/// `t-<16 hex>`: unique per trip (it names the Tor circuit).
fn fresh_ids() -> (String, String) {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    let h = crate::fnv1a(format!("{nanos}|{pid}|{seq}").as_bytes());
    (
        format!("t-{}", &h[6..22]),
        format!("online-{nanos:x}-{pid:x}-{seq}"),
    )
}

/// The trip engine.
pub struct OnlineResearch<'f> {
    fetcher: &'f dyn Fetcher,
    knowledge_dir: Option<PathBuf>,
    topics: Vec<String>,
    audit_log: Option<PathBuf>,
    deny: IdentityDenylist,
    block_hosts: Vec<String>,
    max_pages: u32,
    max_bytes: u64,
    max_wall: Duration,
    allow_http: bool,
}

impl std::fmt::Debug for OnlineResearch<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnlineResearch")
            .field("knowledge_dir", &self.knowledge_dir)
            .field("max_pages", &self.max_pages)
            .field("max_bytes", &self.max_bytes)
            .finish_non_exhaustive()
    }
}

impl<'f> OnlineResearch<'f> {
    /// The engine, or why online research is off ([`gate`]).
    ///
    /// `names` are the people a URL must never mention, on top of this
    /// machine's user / host / time-zone names
    /// ([`IdentityDenylist::from_local`]). The Brain has no read-only
    /// person accessor and the config no user-name key, so the caller
    /// supplies them: **production wiring must pass Ko's name and every
    /// person name known to memory** (e.g. `name` attribute facts). Each
    /// name is blocked whole and by its parts ([`IdentityDenylist::with_names`]).
    pub fn new(cfg: &Config, fetcher: &'f dyn Fetcher, names: &[String]) -> Result<Self, Off> {
        // TODO(production wiring, review S3): no production caller exists
        // yet. Whoever wires `new()` into the app MUST pass the configured
        // user name AND every person-name fact from memory (Brain `name`
        // attribute facts) in `names`; `&[]` there would leave Ko's and
        // other people's names out of the URL denylist.
        gate(cfg)?;
        Ok(Self {
            fetcher,
            knowledge_dir: cfg.mind.resolved_knowledge_dir(),
            topics: cfg.learn.topics.clone(),
            audit_log: lillith_core::config::audit_log_path(),
            deny: IdentityDenylist::from_local().with_names(names),
            block_hosts: cfg
                .learn
                .block_hosts
                .iter()
                .map(|h| h.trim().trim_end_matches('.').to_ascii_lowercase())
                .filter(|h| !h.is_empty())
                .collect(),
            max_pages: cfg.learn.online_max_pages.max(1),
            max_bytes: cfg.learn.online_max_bytes.max(1),
            max_wall: MAX_WALL,
            allow_http: cfg.learn.allow_http,
        })
    }

    pub fn with_knowledge_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.knowledge_dir = dir;
        self
    }

    pub fn with_audit_log(mut self, log: Option<PathBuf>) -> Self {
        self.audit_log = log;
        self
    }

    /// Adds `deny`'s identifiers to the denylist. It never replaces it:
    /// the local identity words and the `names` given to [`new`](Self::new)
    /// always stay blocked.
    pub fn with_denylist(mut self, deny: IdentityDenylist) -> Self {
        self.deny = self.deny.merged(deny);
        self
    }

    pub fn with_max_wall(mut self, d: Duration) -> Self {
        self.max_wall = d;
        self
    }

    fn blocked_host(&self, host: &str) -> bool {
        self.block_hosts
            .iter()
            .any(|b| host == b || host.ends_with(&format!(".{b}")))
    }

    /// Checks one address: parse, tracking stripped, not blocked, no
    /// identifier. Returns the cleaned URL.
    fn vet(&self, u: &Url) -> Result<Url, String> {
        let u = u.strip_tracking();
        if !u.is_https() && !self.allow_http {
            return Err(format!(
                "{u} is plain http:// and [learn] allow_http = false (a Tor exit node \
                 could read or change it); use https://"
            ));
        }
        if self.blocked_host(u.host()) {
            return Err(format!("{} is on [learn] block_hosts", u.host()));
        }
        if let Some(w) = self.deny.hit(&u.to_string()) {
            return Err(format!("an address would send {w}"));
        }
        Ok(u)
    }

    /// A best-effort trip line (the outcome is already fixed; the only
    /// line that gates anything is `PROPOSED`, see [`run`](Self::run)).
    fn audit(&self, state: &str, card: &str, fields: &[String]) {
        if let Err(e) = audit::record_online(self.audit_log.as_deref(), state, card, fields) {
            tracing::warn!("online research audit ({state}): {e}");
        }
    }

    /// Runs one trip: gates, card, fetch, ingest, report. `stop()` is the
    /// safeword ([`safeword_stop`]); it is polled on the card and between
    /// requests.
    pub fn run(
        &self,
        req: &TripRequest,
        consent: &dyn Consent,
        stop: &dyn Fn() -> bool,
    ) -> TripReport {
        TRIPS_STARTED.fetch_add(1, Ordering::SeqCst);
        let (trip_id, card) = fresh_ids();
        let purpose = req.purpose.split_whitespace().collect::<Vec<_>>().join(" ");
        let max_pages = req
            .max_pages
            .unwrap_or(self.max_pages)
            .clamp(1, self.max_pages);
        let max_bytes = req
            .max_bytes
            .unwrap_or(self.max_bytes)
            .clamp(1, self.max_bytes);
        let mut rep = TripReport {
            trip_id: trip_id.clone(),
            card: card.clone(),
            state: TripState::Refused,
            purpose: purpose.clone(),
            approved: Vec::new(),
            visits: Vec::new(),
            saved: Vec::new(),
            dropped: Vec::new(),
            reason: None,
            pages: 0,
            bytes: 0,
        };
        let refuse = |mut rep: TripReport, why: String| {
            self.audit("REFUSED", &rep.card, &[format!("why={why}")]);
            rep.reason = Some(why);
            rep
        };

        // 1. What she would send.
        if purpose.is_empty() {
            return refuse(rep, "no purpose given".into());
        }
        if lillith_memory::is_safeword(&purpose) || stop() {
            rep.state = TripState::Cancelled;
            rep.reason = Some("safeword".into());
            self.audit("CANCELLED", &card, &["why=safeword".into()]);
            return rep;
        }
        if req.urls.is_empty() {
            return refuse(rep, "no addresses given".into());
        }
        if req.urls.len() > MAX_URLS {
            return refuse(rep, format!("more than {MAX_URLS} addresses"));
        }
        let mut list: Vec<Url> = Vec::new();
        for raw in &req.urls {
            let u = match Url::parse_entry(raw) {
                Ok(u) => u,
                Err(e) => return refuse(rep, format!("{raw:?}: {e}")),
            };
            match self.vet(&u) {
                Ok(u) if !list.contains(&u) => list.push(u),
                Ok(_) => {}
                Err(why) => return refuse(rep, why),
            }
        }
        rep.approved = list.iter().map(Url::to_string).collect();
        let hosts: Vec<String> = list.iter().map(|u| u.host().to_string()).collect();

        // 2. PROPOSED first (before the Tor probe, the card or any request),
        //    then Tor, then the mouse-only card.
        let action = PendingAction::OnlineResearch {
            trip_id: trip_id.clone(),
            purpose: purpose.clone(),
            urls: rep.approved.clone(),
            max_pages,
            max_bytes,
            card: card.clone(),
        };
        let proposed = audit::record_online(
            self.audit_log.as_deref(),
            "PROPOSED",
            &card,
            &[
                format!("pages={max_pages}"),
                format!("bytes={max_bytes}"),
                format!("trip={trip_id}"),
                format!("purpose={purpose}"),
                format!("urls={}", rep.approved.join(",")),
            ],
        );
        // Fail-closed: no PROPOSED line on disk, no card and no request.
        if let Err(e) = proposed {
            return refuse(
                rep,
                format!("the audit log can't be written ({e}); nothing is shown or sent"),
            );
        }
        // 3. Tor must already be running; it is never installed here. The
        //    probe is a loopback connect + close: nothing leaves the machine.
        if let Err(why) = self.fetcher.available() {
            return refuse(
                rep,
                format!("online research stays off: {why} (Tor is never installed for you)"),
            );
        }
        let p = action.prompt();
        let text = CardText {
            title: p.title.to_string(),
            summary: p.summary,
            target: String::new(),
            detail: p.detail.unwrap_or_default(),
        };
        let answer = consent.ask(&text, stop);
        let answer = if stop() {
            CardAnswer::Cancelled
        } else {
            answer
        };
        let early = match answer {
            CardAnswer::Yes => None,
            CardAnswer::No => Some((TripState::Denied, "declined on the card".to_string())),
            CardAnswer::TimedOut => Some((TripState::Timeout, "no answer on the card".to_string())),
            CardAnswer::Cancelled => Some((
                TripState::Cancelled,
                "the card was closed (or the safeword)".to_string(),
            )),
            CardAnswer::NoUi => Some((
                TripState::NoUi,
                format!(
                    "no display for the Soul Contract card, and a typed or spoken yes never \
                     counts; nothing was sent. To look it up yourself, open these in Tor \
                     Browser: {}",
                    rep.approved.join(" ")
                ),
            )),
        };
        if let Some((state, why)) = early {
            self.audit(state.word(), &card, &[format!("why={why}")]);
            rep.state = state;
            rep.reason = Some(why);
            return rep;
        }
        self.audit("CONFIRM", &card, &[format!("trip={trip_id}")]);

        // 4. Fetch: approved list only, caps, redirects hop by hop.
        let deadline = Instant::now() + self.max_wall;
        let (mut cancelled, mut timed_out, mut partial) = (false, false, false);
        let mut pages: Vec<Page> = Vec::new();
        'trip: for start in &list {
            let mut url = start.clone();
            let mut hops = 0;
            loop {
                if stop() {
                    cancelled = true;
                    break 'trip;
                }
                if Instant::now() >= deadline {
                    timed_out = true;
                    rep.reason = Some(format!("the trip ran over {:?}", self.max_wall));
                    break 'trip;
                }
                if rep.pages >= max_pages {
                    partial = true;
                    rep.reason = Some(format!(
                        "page cap reached ({max_pages}); the rest was not fetched"
                    ));
                    break 'trip;
                }
                if rep.bytes >= max_bytes {
                    partial = true;
                    rep.reason = Some(format!(
                        "byte cap reached ({max_bytes}); the rest was not fetched"
                    ));
                    break 'trip;
                }
                if !hosts.iter().any(|h| h == url.host()) {
                    rep.visits.push(Visit {
                        url: url.to_string(),
                        status: None,
                        note: "not fetched: host not on the approved list".into(),
                    });
                    partial = true;
                    rep.reason = Some(format!(
                        "redirected to {}, which is not on the approved list; the trip stopped",
                        url.host()
                    ));
                    break 'trip;
                }
                if let Err(why) = self.vet(&url) {
                    rep.visits.push(Visit {
                        url: url.to_string(),
                        status: None,
                        note: format!("not fetched: {why}"),
                    });
                    partial = true;
                    rep.reason = Some(format!("{why}; the trip stopped"));
                    break 'trip;
                }
                rep.pages += 1;
                let halt = || stop() || Instant::now() >= deadline;
                let got = self
                    .fetcher
                    .get(&url, &trip_id, max_bytes - rep.bytes, &halt);
                let page = match got {
                    Ok(p) => p,
                    Err(FetchError::Stopped) => {
                        rep.visits.push(Visit {
                            url: url.to_string(),
                            status: None,
                            note: "stopped mid-request".into(),
                        });
                        if stop() {
                            cancelled = true;
                        } else {
                            timed_out = true;
                            rep.reason = Some(format!("the trip ran over {:?}", self.max_wall));
                        }
                        break 'trip;
                    }
                    Err(e) => {
                        rep.visits.push(Visit {
                            url: url.to_string(),
                            status: None,
                            note: e.to_string(),
                        });
                        partial = true;
                        break;
                    }
                };
                // Never trust the transport with the cap.
                let mut body = page.body;
                let room = (max_bytes - rep.bytes) as usize;
                let over = body.len() > room;
                body.truncate(room);
                rep.bytes += body.len() as u64;
                if (300..400).contains(&page.status) {
                    let next = page.location.as_deref().map(|l| url.join(l));
                    match next {
                        Some(Ok(n)) if hops < MAX_REDIRECTS => {
                            let n = n.strip_tracking();
                            // An https → http downgrade always stops the
                            // trip, whatever `allow_http` says.
                            if url.is_https() && !n.is_https() {
                                rep.visits.push(Visit {
                                    url: url.to_string(),
                                    status: Some(page.status),
                                    note: format!("redirect to {n}"),
                                });
                                rep.visits.push(Visit {
                                    url: n.to_string(),
                                    status: None,
                                    note: "not fetched: https to http downgrade refused".into(),
                                });
                                partial = true;
                                rep.reason = Some(format!(
                                    "{url} redirected down to plain http ({n}); the trip stopped"
                                ));
                                break 'trip;
                            }
                            rep.visits.push(Visit {
                                url: url.to_string(),
                                status: Some(page.status),
                                note: format!("redirect to {n}"),
                            });
                            url = n;
                            hops += 1;
                            continue;
                        }
                        other => {
                            let why = match other {
                                Some(Ok(_)) => "too many redirects".to_string(),
                                Some(Err(e)) => format!("bad redirect: {e}"),
                                None => "redirect without a Location".to_string(),
                            };
                            rep.visits.push(Visit {
                                url: url.to_string(),
                                status: Some(page.status),
                                note: why,
                            });
                            partial = true;
                            break;
                        }
                    }
                }
                if !(200..300).contains(&page.status) {
                    rep.visits.push(Visit {
                        url: url.to_string(),
                        status: Some(page.status),
                        note: "not kept: HTTP error".into(),
                    });
                    partial = true;
                    break;
                }
                if let Err(why) = text::content_allowed(&page.content_type, &body) {
                    rep.visits.push(Visit {
                        url: url.to_string(),
                        status: Some(page.status),
                        note: format!("refused: {why}"),
                    });
                    partial = true;
                    break;
                }
                rep.visits.push(Visit {
                    url: url.to_string(),
                    status: Some(page.status),
                    note: String::new(),
                });
                pages.push(Page {
                    url: url.clone(),
                    content_type: page.content_type,
                    body,
                });
                if page.truncated || over {
                    partial = true;
                    rep.reason = Some(format!(
                        "byte cap reached ({max_bytes}); the rest was not fetched"
                    ));
                    break 'trip;
                }
                break;
            }
        }

        // The safeword: stop and discard (nothing is saved).
        if cancelled || stop() {
            rep.state = TripState::Cancelled;
            rep.reason = Some("safeword: stopped and discarded".into());
            self.audit_after(&rep);
            return rep;
        }

        // 5. Ingest: untrusted web notes only.
        if !pages.is_empty() {
            self.save(&mut rep, pages, &mut partial);
        }
        rep.state = if timed_out {
            TripState::Timeout
        } else if partial {
            TripState::Partial
        } else {
            TripState::Done
        };
        self.audit_after(&rep);
        rep
    }

    fn audit_after(&self, rep: &TripReport) {
        let saved: Vec<&str> = rep.saved.iter().map(|s| s.id.as_str()).collect();
        let mut f = vec![
            format!("fetched={}", rep.fetched().join(",")),
            format!("saved={}", saved.join(",")),
            format!("pages={}", rep.pages),
            format!("bytes={}", rep.bytes),
        ];
        if let Some(r) = &rep.reason {
            f.push(format!("why={r}"));
        }
        self.audit(rep.state.word(), &rep.card, &f);
    }

    fn save(&self, rep: &mut TripReport, pages: Vec<Page>, partial: &mut bool) {
        let fail = |rep: &mut TripReport, pages: &[Page], why: &str| {
            for p in pages {
                rep.dropped.push((p.url.to_string(), why.to_string()));
            }
        };
        let Some(dir) = &self.knowledge_dir else {
            fail(rep, &pages, "no knowledge folder");
            *partial = true;
            return;
        };
        let mut topics = self.topics.clone();
        if !topics.iter().any(|t| t == WEB_TOPIC) {
            topics.push(WEB_TOPIC.into());
        }
        let store = match NoteStore::open(dir, &topics, false) {
            Ok(s) => s,
            Err(e) => {
                fail(rep, &pages, &format!("knowledge folder: {e}"));
                *partial = true;
                return;
            }
        };
        let lock = match store.lock() {
            Ok(Some(l)) => l,
            Ok(None) => {
                fail(rep, &pages, "another writer holds knowledge/.lock");
                *partial = true;
                return;
            }
            Err(e) => {
                fail(rep, &pages, &format!("lock: {e}"));
                *partial = true;
                return;
            }
        };
        let now = crate::now_s();
        for p in pages {
            let url = p.url.to_string();
            let (title, body) = text::extract(&p.content_type, &p.body);
            let body = crate::paths::drop_secret_lines(&body);
            let body = body.trim();
            if body.is_empty() {
                rep.dropped.push((url, "no text".into()));
                continue;
            }
            let title = title
                .unwrap_or_else(|| format!("{}{}", p.url.host(), p.url.path_and_query()))
                .chars()
                .take(120)
                .collect::<String>();
            // §7 ingest filter: role / instruction patterns, persona or
            // safeword words, anything about minors → dropped whole.
            let item = crate::extract::Item {
                kind: "web".into(),
                title: title.clone(),
                body: body.to_string(),
                links: Vec::new(),
            };
            if !crate::extract::allowed(&item) || lillith_memory::is_safeword(body) {
                rep.dropped.push((
                    url,
                    "dropped by the ingest filter (instructions, persona / safeword words or \
                     minors)"
                        .into(),
                ));
                continue;
            }
            let note = Note {
                id: crate::learner::new_id(&url, &title, now),
                title,
                topic: WEB_TOPIC.into(),
                kind: "web".into(),
                source_type: "web".into(),
                source_ref: url.clone(),
                source_hash: crate::fnv1a(&p.body),
                created: now,
                updated: now,
                confidence: WEB_CONFIDENCE,
                quiz_asked: 0,
                quiz_passed: 0,
                links: Vec::new(),
                origin: "learned".into(),
                body: body.to_string(),
                web: Some(WebSource {
                    url: url.clone(),
                    trip_id: rep.trip_id.clone(),
                }),
            };
            match store.write(&lock, &note) {
                Ok(path) => {
                    audit::record(self.audit_log.as_deref(), "web", WEB_TOPIC, &note.id);
                    rep.saved.push(Saved {
                        id: note.id.clone(),
                        path: store.rel(&path),
                        url,
                        summary: text::one_line_summary(&note.body, 120),
                    });
                }
                Err(e) => {
                    rep.dropped.push((url, format!("write failed: {e}")));
                    *partial = true;
                }
            }
        }
    }
}
