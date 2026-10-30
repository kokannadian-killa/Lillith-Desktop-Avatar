//! The memory graph itself.

use crate::decay::{decayed, recency, reinforce};
use crate::errors::{signature, ErrorRecord, FixOutcome, KnownFix};
use crate::extract::{intent_key, keywords, Extractor, Fact, Polarity, RuleExtractor};
use crate::forget::{
    contains_phrase, content_words, matches_topic, parse_forget, same_word, ForgetCmd,
};
use crate::habits::{self, EventRow, Prediction};
use crate::safeword::{is_safeword, mentions_safeword};
use crate::{schema, Error, Kind};
use lillith_core::config::MemoryConfig;
use lillith_core::event::Event;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

type Result<T> = std::result::Result<T, Error>;

/// Retrieval weights (see docs/MEMORY.md).
const W_REL: f64 = 0.55;
const W_STRENGTH: f64 = 0.25;
const W_RECENCY: f64 = 0.15;
const APP_BOOST: f64 = 0.10;
const NEIGHBOR_DISCOUNT: f64 = 0.5;
/// id, kind, label, detail, strength, last_seen, edge strength, edge last_seen.
type NeighborRow = (i64, String, String, String, f64, f64, f64, f64);

const KIND_BOOST: f64 = 0.08;
const BARE_ENTITY: f64 = 0.6;
/// Reinforcement rates.
const RATE_OBSERVE: f64 = 0.35;
const RATE_RETRIEVE: f64 = 0.05;
const RATE_USED: f64 = 0.15;
const RATE_WEAK: f64 = 0.05;
/// New memories start here.
const INITIAL: f64 = 0.6;
/// Maintenance (prune + habit refresh) at most this often.
const MAINTAIN_EVERY_S: f64 = 3600.0;
/// Events older than this are ignored by the habit miner.
const HABIT_WINDOW_S: f64 = 60.0 * 86_400.0;
/// How long a write waits for another process holding the DB lock.
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(2000);
/// Most failed fixes remembered per error.
const MAX_FAILED_FIXES: usize = 8;

/// "When and where" for an observation or a query.
#[derive(Debug, Clone, PartialEq)]
pub struct Ctx {
    /// Unix seconds.
    pub now: f64,
    /// Focused app, if known (boosts memories about it).
    pub app: Option<String>,
    /// Local UTC offset in seconds (`None` = ask the OS via SQLite).
    pub utc_offset_s: Option<i32>,
}

impl Ctx {
    /// The current time, no app.
    pub fn now() -> Self {
        Self::at(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or_default(),
        )
    }

    /// A fixed time (tests, replays).
    pub fn at(now: f64) -> Self {
        Self {
            now,
            app: None,
            utc_offset_s: None,
        }
    }

    pub fn with_app(mut self, app: impl Into<String>) -> Self {
        self.app = Some(app.into());
        self
    }

    pub fn with_utc_offset(mut self, secs: i32) -> Self {
        self.utc_offset_s = Some(secs);
        self
    }
}

/// One retrieved memory.
#[derive(Debug, Clone, PartialEq)]
pub struct Memory {
    pub id: i64,
    pub kind: Kind,
    pub label: String,
    pub detail: String,
    /// Decayed strength now.
    pub strength: f64,
    /// Final ranking score.
    pub score: f64,
    /// Found through a graph edge rather than a direct match.
    pub via_neighbor: bool,
}

/// What an observation did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observed {
    /// Memory is off: nothing read or written.
    Disabled,
    /// Safeword: nothing stored, the loop stopped here.
    Stop,
    /// Not something memory keeps (private window, forget command, ...).
    Ignored,
    /// Stored `facts` new/updated facts; reinforced `reinforced` memories.
    Learned { facts: usize, reinforced: usize },
    /// The DB failed; logged, nothing else happens.
    Failed(String),
}

/// Row counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    pub nodes: u64,
    pub edges: u64,
    pub events: u64,
    pub errors: u64,
}

/// What a forget removed, and whether the file was scrubbed afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ForgetReport {
    pub nodes: usize,
    pub edges: usize,
    pub events: usize,
    pub errors: usize,
    /// `true` when VACUUM ran and the WAL was checkpointed + truncated, so
    /// the deleted text is gone from the file bytes too. `false` = the rows
    /// are deleted but another connection (a second Lillith window) kept
    /// old pages alive; see `note`.
    pub scrubbed: bool,
    /// Why scrubbing did not finish (no memory content in it).
    pub note: Option<String>,
    /// Up to [`FORGET_EXAMPLES`] short labels of what matched, for the
    /// confirmation question (shown to Ko only; never logged).
    pub examples: Vec<String>,
}

/// How many example rows a forget preview carries.
pub const FORGET_EXAMPLES: usize = 3;
/// Example labels are cut to this many characters.
const EXAMPLE_CHARS: usize = 60;

impl ForgetReport {
    /// Rows removed (nodes + events + errors; edges ride along).
    pub fn total(&self) -> usize {
        self.nodes + self.events + self.errors
    }
}

/// Lillith's memory. `Send` (move it to the chat worker), not `Sync`.
pub struct Brain {
    pub(crate) conn: Option<Connection>,
    cfg: MemoryConfig,
    path: Option<PathBuf>,
    extractor: Box<dyn Extractor>,
    pub(crate) last_retrieved: RefCell<Vec<i64>>,
    local_offset: Cell<Option<i32>>,
    last_maintain: Cell<f64>,
    /// The shared `<data>/lillith/audit.log` (also with a custom
    /// `db_path`): one line per forget, kind + counts only. Set by
    /// [`Brain::open`]; `None` for `open_path` / in-memory brains.
    audit_log: Option<PathBuf>,
}

impl std::fmt::Debug for Brain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Brain")
            .field("enabled", &self.is_enabled())
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Brain {
    /// Opens the brain for `cfg`: disabled (no file at all) when
    /// `cfg.enabled` is false, else the DB at `cfg.resolved_db_path()`.
    pub fn open(cfg: &MemoryConfig) -> Result<Self> {
        if !cfg.enabled {
            return Ok(Self::disabled(cfg));
        }
        let path = cfg.resolved_db_path().ok_or(Error::NoDataDir)?;
        Ok(Self::open_path(&path, cfg)?.with_audit_log(lillith_core::config::audit_log_path()))
    }

    /// Opens (creating) the DB at `path`. Still honours `cfg.enabled`. No
    /// audit file (tests / embedders opt in with
    /// [`with_audit_log`](Self::with_audit_log)); [`open`](Self::open) uses
    /// the shared `<data>/lillith/audit.log`.
    pub fn open_path(path: &Path, cfg: &MemoryConfig) -> Result<Self> {
        if !cfg.enabled {
            return Ok(Self::disabled(cfg));
        }
        prepare_private_file(path)?;
        let conn = Connection::open(path)?;
        let mut b = Self::init(conn, cfg)?;
        b.path = Some(path.to_path_buf());
        Ok(b)
    }

    /// In-memory DB (tests). Honours `cfg.enabled`.
    pub fn open_in_memory(cfg: &MemoryConfig) -> Result<Self> {
        if !cfg.enabled {
            return Ok(Self::disabled(cfg));
        }
        Self::init(Connection::open_in_memory()?, cfg)
    }

    /// A brain that never reads or writes anything.
    pub fn disabled(cfg: &MemoryConfig) -> Self {
        Self {
            conn: None,
            cfg: MemoryConfig {
                enabled: false,
                ..cfg.clone()
            },
            path: None,
            extractor: Box::new(RuleExtractor),
            last_retrieved: RefCell::new(Vec::new()),
            local_offset: Cell::new(None),
            last_maintain: Cell::new(0.0),
            audit_log: None,
        }
    }

    fn init(mut conn: Connection, cfg: &MemoryConfig) -> Result<Self> {
        conn.busy_timeout(BUSY_TIMEOUT)?;
        // WAL (in-memory DBs report "memory"), small cache, real deletes.
        let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        conn.execute_batch(
            "PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA secure_delete = ON;
             PRAGMA cache_size = -1024;
             PRAGMA temp_store = MEMORY;
             PRAGMA journal_size_limit = 1048576;",
        )?;
        schema::migrate(&mut conn)?;
        let last: f64 = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'last_maintain'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);
        Ok(Self {
            conn: Some(conn),
            cfg: cfg.clone(),
            path: None,
            extractor: Box::new(RuleExtractor),
            last_retrieved: RefCell::new(Vec::new()),
            local_offset: Cell::new(None),
            last_maintain: Cell::new(last),
            audit_log: None,
        })
    }

    /// Where forget audit lines go (kind + counts only, file 0600 on unix);
    /// `None` turns the file audit off (the tracing line stays).
    pub fn with_audit_log(mut self, path: Option<PathBuf>) -> Self {
        self.audit_log = path;
        self
    }

    /// The forget audit file, if any.
    pub fn audit_log(&self) -> Option<&Path> {
        self.audit_log.as_deref()
    }

    /// Swaps the fact extractor (e.g. an LLM-assisted one).
    pub fn with_extractor(mut self, e: Box<dyn Extractor>) -> Self {
        self.extractor = e;
        self
    }

    /// Whether memory is on (reads and writes happen).
    pub fn is_enabled(&self) -> bool {
        self.conn.is_some()
    }

    /// The DB file, if file-backed.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn config(&self) -> &MemoryConfig {
        &self.cfg
    }

    fn half_life_s(&self) -> f64 {
        (self.cfg.half_life_days as f64) * 86_400.0
    }

    fn offset(&self, ctx: &Ctx) -> i32 {
        if let Some(o) = ctx.utc_offset_s {
            return o;
        }
        if let Some(o) = self.local_offset.get() {
            return o;
        }
        let o = self
            .conn
            .as_ref()
            .and_then(|c| {
                c.query_row(
                    "SELECT CAST(strftime('%s','now','localtime') AS INTEGER) - CAST(strftime('%s','now') AS INTEGER)",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .ok()
            })
            .unwrap_or(0) as i32;
        self.local_offset.set(Some(o));
        o
    }

    // ----- graph primitives -------------------------------------------------

    /// Inserts or reinforces the node `(kind, key)`; updates its label/detail
    /// when they changed. Returns its id (`None` when disabled).
    pub fn upsert_node(
        &self,
        kind: Kind,
        key: &str,
        label: &str,
        detail: &str,
        ctx: &Ctx,
    ) -> Result<Option<i64>> {
        self.upsert_node_rate(kind, key, label, detail, ctx, RATE_OBSERVE, INITIAL)
    }

    /// Like [`upsert_node`](Self::upsert_node), but a brand-new node starts
    /// at `initial` (clamped to `0.0..=` the normal starting strength); an
    /// existing node is reinforced as usual. Dream learning stores its
    /// facts weak this way (docs/SELF_LEARNING.md §11: `dream_strength`).
    pub fn upsert_node_weak(
        &self,
        kind: Kind,
        key: &str,
        label: &str,
        detail: &str,
        ctx: &Ctx,
        initial: f64,
    ) -> Result<Option<i64>> {
        let initial = if initial.is_finite() {
            initial.clamp(0.0, INITIAL)
        } else {
            INITIAL
        };
        self.upsert_node_rate(kind, key, label, detail, ctx, RATE_OBSERVE, initial)
    }

    #[allow(clippy::too_many_arguments)]
    fn upsert_node_rate(
        &self,
        kind: Kind,
        key: &str,
        label: &str,
        detail: &str,
        ctx: &Ctx,
        rate: f64,
        initial: f64,
    ) -> Result<Option<i64>> {
        let Some(c) = &self.conn else { return Ok(None) };
        let key = key.trim().to_lowercase();
        let hl = self.half_life_s();
        let row: Option<(i64, f64, f64, String, String)> = c
            .query_row(
                "SELECT id, strength, last_seen, label, detail FROM nodes WHERE kind = ?1 AND key = ?2",
                params![kind.as_str(), key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        match row {
            Some((id, s, seen, old_label, old_detail)) => {
                let s = reinforce(decayed(s, ctx.now - seen, hl), rate);
                c.execute(
                    "UPDATE nodes SET strength = ?2, access_count = access_count + 1,
                     last_seen = max(last_seen, ?3) WHERE id = ?1",
                    params![id, s, ctx.now],
                )?;
                if old_label != label || old_detail != detail {
                    c.execute(
                        "UPDATE nodes SET label = ?2, detail = ?3, updated_at = ?4 WHERE id = ?1",
                        params![id, label, detail, ctx.now],
                    )?;
                }
                Ok(Some(id))
            }
            None => {
                c.execute(
                    "INSERT INTO nodes(kind, key, label, detail, strength, access_count, created_at, last_seen, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6, ?6)",
                    params![kind.as_str(), key, label, detail, initial, ctx.now],
                )?;
                Ok(Some(c.last_insert_rowid()))
            }
        }
    }

    /// Inserts or reinforces the edge `src -relation-> dst`.
    pub fn link(&self, src: i64, dst: i64, relation: &str, ctx: &Ctx) -> Result<()> {
        let Some(c) = &self.conn else { return Ok(()) };
        if src == dst {
            return Ok(());
        }
        let hl = self.half_life_s();
        let row: Option<(f64, f64)> = c
            .query_row(
                "SELECT strength, last_seen FROM edges WHERE src = ?1 AND dst = ?2 AND relation = ?3",
                params![src, dst, relation],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match row {
            Some((s, seen)) => {
                let s = reinforce(decayed(s, ctx.now - seen, hl), RATE_OBSERVE);
                c.execute(
                    "UPDATE edges SET strength = ?4, access_count = access_count + 1,
                     last_seen = max(last_seen, ?5) WHERE src = ?1 AND dst = ?2 AND relation = ?3",
                    params![src, dst, relation, s, ctx.now],
                )?;
            }
            None => {
                c.execute(
                    "INSERT INTO edges(src, dst, relation, strength, access_count, created_at, last_seen)
                     VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5)",
                    params![src, dst, relation, INITIAL, ctx.now],
                )?;
            }
        }
        Ok(())
    }

    /// Reinforces existing nodes by `rate`.
    fn reinforce_ids(&self, ids: &[i64], rate: f64, ctx: &Ctx) -> Result<usize> {
        let Some(c) = &self.conn else { return Ok(0) };
        let hl = self.half_life_s();
        let mut n = 0;
        for id in ids {
            let row: Option<(f64, f64)> = c
                .query_row(
                    "SELECT strength, last_seen FROM nodes WHERE id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((s, seen)) = row {
                let s = reinforce(decayed(s, ctx.now - seen, hl), rate);
                c.execute(
                    "UPDATE nodes SET strength = ?2, access_count = access_count + 1,
                     last_seen = max(last_seen, ?3) WHERE id = ?1",
                    params![id, s, ctx.now],
                )?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// Read-only: every remembered value that is a person's name, for the
    /// outgoing-request identity denylist (online research / dream learning
    /// must never put one in a URL). Ko's own `name` attribute plus the
    /// value of any relationship attribute ("my sister is Anna", "my
    /// boss is Sam Lee"), up to three words. Nothing is reinforced. Empty
    /// when disabled.
    pub fn person_names(&self) -> Vec<String> {
        const PEOPLE: &[&str] = &[
            "name",
            "nickname",
            "real name",
            "full name",
            "wife",
            "husband",
            "partner",
            "girlfriend",
            "boyfriend",
            "fiance",
            "fiancee",
            "spouse",
            "sister",
            "brother",
            "mom",
            "mother",
            "dad",
            "father",
            "son",
            "daughter",
            "child",
            "kid",
            "aunt",
            "uncle",
            "cousin",
            "niece",
            "nephew",
            "grandma",
            "grandmother",
            "grandpa",
            "grandfather",
            "friend",
            "best friend",
            "roommate",
            "boss",
            "manager",
            "coworker",
            "colleague",
            "neighbor",
            "neighbour",
            "teacher",
            "doctor",
            "therapist",
            "ex",
        ];
        let Some(c) = &self.conn else {
            return Vec::new();
        };
        let rows: Vec<(String, String)> = c
            .prepare("SELECT key, detail FROM nodes WHERE kind = ?1 AND key LIKE 'attr:%'")
            .and_then(|mut st| {
                st.query_map(params![Kind::Entity.as_str()], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?
                .collect()
            })
            .unwrap_or_default();
        let mut out: Vec<String> = Vec::new();
        for (key, value) in rows {
            let attr = key.trim_start_matches("attr:");
            let value = value.trim();
            let words = value.split_whitespace().count();
            if !PEOPLE.contains(&attr) || words == 0 || words > 3 {
                continue;
            }
            if !out.iter().any(|n| n.eq_ignore_ascii_case(value)) {
                out.push(value.to_string());
            }
        }
        out
    }

    /// Decayed strength of a node now (`None` if missing / disabled).
    pub fn strength_of(&self, kind: Kind, key: &str, ctx: &Ctx) -> Option<f64> {
        let c = self.conn.as_ref()?;
        let (s, seen): (f64, f64) = c
            .query_row(
                "SELECT strength, last_seen FROM nodes WHERE kind = ?1 AND key = ?2",
                params![kind.as_str(), key.trim().to_lowercase()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .ok()??;
        Some(decayed(s, ctx.now - seen, self.half_life_s()))
    }

    // ----- retrieval --------------------------------------------------------

    /// Top-`k` memories for `query` in `ctx`: FTS5 relevance + decayed
    /// strength + recency + app boost, then one hop of graph neighbours.
    /// Directly matched memories are lightly reinforced (they were reused).
    pub fn retrieve(&self, query: &str, ctx: &Ctx, k: usize) -> Result<Vec<Memory>> {
        let Some(c) = &self.conn else {
            return Ok(Vec::new());
        };
        if k == 0 || is_safeword(query) {
            return Ok(Vec::new());
        }
        let hl = self.half_life_s();
        let mut terms = keywords(query, 8);
        let app = ctx.app.as_deref().map(|a| a.trim().to_lowercase());
        if let Some(a) = &app {
            for w in keywords(a, 2) {
                if !terms.contains(&w) {
                    terms.push(w);
                }
            }
        }
        struct Cand {
            kind: Kind,
            label: String,
            detail: String,
            strength: f64,
            score: f64,
            rel: f64,
            neighbor: bool,
        }
        let mut cands: HashMap<i64, Cand> = HashMap::new();
        let score_of = |rel: f64, s: f64, seen: f64, label: &str, kind: Kind, detail: &str| {
            let eff = decayed(s, ctx.now - seen, hl);
            let mut score =
                W_REL * rel + W_STRENGTH * eff + W_RECENCY * recency(ctx.now - seen, hl);
            // Statements ("Ko likes X") say more than a bare topic node ("X").
            match kind {
                Kind::Preference | Kind::Habit | Kind::Error => score += KIND_BOOST,
                Kind::Entity if detail.is_empty() => score *= BARE_ENTITY,
                _ => {}
            }
            if let Some(a) = &app {
                if !a.is_empty() && label.to_lowercase().contains(a.as_str()) {
                    score += APP_BOOST;
                }
            }
            (eff, score)
        };
        if !terms.is_empty() {
            let expr = terms
                .iter()
                .map(|t| format!("\"{t}\"*"))
                .collect::<Vec<_>>()
                .join(" OR ");
            let mut st = c.prepare_cached(
                "SELECT n.id, n.kind, n.label, n.detail, n.strength, n.last_seen, bm25(nodes_fts)
                 FROM nodes_fts JOIN nodes n ON n.id = nodes_fts.rowid
                 WHERE nodes_fts MATCH ?1 ORDER BY bm25(nodes_fts) LIMIT 40",
            )?;
            let rows: Vec<(i64, String, String, String, f64, f64, f64)> = st
                .query_map([expr], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                })?
                .collect::<rusqlite::Result<_>>()?;
            let best = rows.iter().map(|r| -r.6).fold(0.0f64, f64::max).max(1e-9);
            for (id, kind, label, detail, s, seen, bm) in rows {
                let Some(kind) = Kind::parse(&kind) else {
                    continue;
                };
                let rel = ((-bm) / best).clamp(0.0, 1.0);
                let (eff, score) = score_of(rel, s, seen, &label, kind, &detail);
                if eff < self.cfg.prune_below as f64 {
                    continue;
                }
                cands.insert(
                    id,
                    Cand {
                        kind,
                        label,
                        detail,
                        strength: eff,
                        score,
                        rel,
                        neighbor: false,
                    },
                );
            }
        }
        // Core facts about Ko fill the remaining slots (ranked below matches).
        {
            let mut st = c.prepare_cached(
                "SELECT id, kind, label, detail, strength, last_seen FROM nodes
                 WHERE kind IN ('preference', 'habit') OR (kind = 'entity' AND key LIKE 'attr:%')
                 ORDER BY strength DESC LIMIT 32",
            )?;
            let mut rows: Vec<(i64, String, String, String, f64, f64)> = st
                .query_map([], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                })?
                .collect::<rusqlite::Result<_>>()?;
            // Rank by *decayed* strength (raw strength would let a stale but
            // once-strong fact crowd out fresh ones), then keep 4.
            let hl = self.half_life_s();
            rows.sort_by(|a, b| {
                decayed(b.4, ctx.now - b.5, hl).total_cmp(&decayed(a.4, ctx.now - a.5, hl))
            });
            rows.truncate(4);
            for (id, kind, label, detail, s, seen) in rows {
                let Some(kind) = Kind::parse(&kind) else {
                    continue;
                };
                let (eff, score) = score_of(0.0, s, seen, &label, kind, &detail);
                if eff < self.cfg.prune_below as f64 * 4.0 {
                    continue;
                }
                cands.entry(id).or_insert(Cand {
                    kind,
                    label,
                    detail,
                    strength: eff,
                    score: score * 0.5,
                    rel: 0.0,
                    neighbor: false,
                });
            }
        }
        // One hop of neighbours from the direct matches.
        let mut seeds: Vec<(i64, f64)> = cands
            .iter()
            .filter(|(_, v)| v.rel > 0.0)
            .map(|(id, v)| (*id, v.score))
            .collect();
        seeds.sort_by(|a, b| b.1.total_cmp(&a.1));
        seeds.truncate(k);
        {
            let mut st = c.prepare_cached(
                "SELECT n.id, n.kind, n.label, n.detail, n.strength, n.last_seen, e.strength, e.last_seen
                 FROM edges e JOIN nodes n ON n.id = CASE WHEN e.src = ?1 THEN e.dst ELSE e.src END
                 WHERE (e.src = ?1 OR e.dst = ?1) AND n.key != 'ko' LIMIT 16",
            )?;
            for (seed, seed_score) in seeds {
                let rows: Vec<NeighborRow> = st
                    .query_map([seed], |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                            r.get(6)?,
                            r.get(7)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<_>>()?;
                for (id, kind, label, detail, s, seen, es, eseen) in rows {
                    let Some(kind) = Kind::parse(&kind) else {
                        continue;
                    };
                    let eff = decayed(s, ctx.now - seen, hl);
                    let edge = decayed(es, ctx.now - eseen, hl);
                    let score = seed_score * NEIGHBOR_DISCOUNT * edge;
                    if eff < self.cfg.prune_below as f64 {
                        continue;
                    }
                    let e = cands.entry(id).or_insert(Cand {
                        kind,
                        label,
                        detail,
                        strength: eff,
                        score: 0.0,
                        rel: 0.0,
                        neighbor: true,
                    });
                    if score > e.score {
                        e.score = score;
                        e.neighbor = e.rel == 0.0;
                    }
                }
            }
        }
        let mut out: Vec<Memory> = cands
            .into_iter()
            .map(|(id, v)| Memory {
                id,
                kind: v.kind,
                label: v.label,
                detail: v.detail,
                strength: v.strength,
                score: v.score,
                via_neighbor: v.neighbor,
            })
            .filter(|m| !(m.kind == Kind::Entity && m.label == "Ko"))
            .collect();
        out.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.id.cmp(&b.id)));
        out.truncate(k);
        let direct: Vec<i64> = out
            .iter()
            .filter(|m| !m.via_neighbor)
            .map(|m| m.id)
            .collect();
        self.reinforce_ids(&direct, RATE_RETRIEVE, ctx)?;
        *self.last_retrieved.borrow_mut() = out.iter().map(|m| m.id).collect();
        Ok(out)
    }

    /// The compact block for the chat reference message for `query`, within
    /// `max_context_tokens` (~4 chars/token). `None` when there is nothing
    /// to say, memory is off, or `query` is the safeword.
    pub fn context_block(&self, query: &str, ctx: &Ctx) -> Option<String> {
        if !self.is_enabled() || mentions_safeword(query) {
            return None;
        }
        let mems = match self.retrieve(query, ctx, self.cfg.top_k) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("memory: retrieval failed: {e}");
                return None;
            }
        };
        let preds: Vec<Prediction> = self
            .predict(ctx, 2)
            .into_iter()
            .filter(|p| p.confidence >= 0.35)
            .collect();
        format_block(&mems, &preds, self.cfg.max_context_tokens.max(1) * 4)
    }

    // ----- learning loop ----------------------------------------------------

    /// Learns from one chat turn: extracts facts from Ko's message, updates
    /// the graph, logs the request for habit mining, reinforces memories the
    /// reply actually used, and runs maintenance at most hourly. The
    /// safeword stops everything before any write ([`Observed::Stop`]).
    pub fn observe_interaction(&self, user_msg: &str, reply: &str, ctx: &Ctx) -> Observed {
        if !self.is_enabled() {
            return Observed::Disabled;
        }
        if mentions_safeword(user_msg) {
            self.last_retrieved.borrow_mut().clear();
            return Observed::Stop;
        }
        if parse_forget(user_msg).is_some() {
            return Observed::Ignored;
        }
        match self.observe_inner(user_msg, reply, ctx) {
            Ok((facts, reinforced)) => {
                if let Err(e) = self.maybe_maintain(ctx) {
                    tracing::warn!("memory: maintenance failed: {e}");
                }
                Observed::Learned { facts, reinforced }
            }
            Err(e) => {
                tracing::warn!("memory: observe failed: {e}");
                Observed::Failed(e.to_string())
            }
        }
    }

    fn observe_inner(&self, user: &str, reply: &str, ctx: &Ctx) -> Result<(usize, usize)> {
        self.write_tx(|| self.observe_rows(user, reply, ctx))
    }

    fn observe_rows(&self, user: &str, reply: &str, ctx: &Ctx) -> Result<(usize, usize)> {
        let c = self.conn.as_ref().expect("enabled");
        let facts = self.extractor.extract(user, reply);
        let ko = self
            .upsert_node_rate(Kind::Entity, "ko", "Ko", "the user", ctx, RATE_WEAK, 0.9)?
            .expect("enabled");
        for f in &facts {
            self.learn_fact(ko, f, ctx)?;
        }
        if let Some(k) = intent_key(user) {
            self.log_event("request", &k, ctx)?;
        }
        // Memories injected last turn that the reply actually drew on.
        let used: Vec<i64> = {
            let ids = std::mem::take(&mut *self.last_retrieved.borrow_mut());
            let reply_l = reply.to_lowercase();
            let mut used = Vec::new();
            for id in ids {
                let label: Option<String> = c
                    .query_row("SELECT label FROM nodes WHERE id = ?1", [id], |r| r.get(0))
                    .optional()?;
                if let Some(l) = label {
                    if keywords(&l, 6)
                        .iter()
                        .any(|w| w.len() >= 4 && w != "likes" && reply_l.contains(w.as_str()))
                    {
                        used.push(id);
                    }
                }
            }
            used
        };
        let reinforced = self.reinforce_ids(&used, RATE_USED, ctx)?;
        Ok((facts.len(), reinforced))
    }

    fn learn_fact(&self, ko: i64, f: &Fact, ctx: &Ctx) -> Result<()> {
        let up = |kind, key: &str, label: &str, detail: &str| -> Result<i64> {
            Ok(self
                .upsert_node(kind, key, label, detail, ctx)?
                .expect("enabled"))
        };
        match f {
            Fact::Preference { polarity, object } => {
                let obj = up(Kind::Entity, object, object, "")?;
                let key = format!("{}:{object}", polarity.as_str());
                let label = format!("Ko {} {object}", polarity.as_str());
                let pref = up(Kind::Preference, &key, &label, "")?;
                self.link(pref, obj, "about", ctx)?;
                self.link(ko, pref, polarity.as_str(), ctx)?;
                // A change of heart weakens the opposite preference.
                let opposite: &[Polarity] = match polarity {
                    Polarity::Dislike => &[Polarity::Like, Polarity::Prefer],
                    _ => &[Polarity::Dislike],
                };
                for o in opposite {
                    self.conn.as_ref().expect("enabled").execute(
                        "UPDATE nodes SET strength = strength * 0.3 WHERE kind = 'preference' AND key = ?1",
                        [format!("{}:{object}", o.as_str())],
                    )?;
                }
            }
            Fact::Attribute { attr, value } => {
                let n = up(
                    Kind::Entity,
                    &format!("attr:{attr}"),
                    &format!("Ko's {attr}: {value}"),
                    value,
                )?;
                self.link(ko, n, "has", ctx)?;
            }
            Fact::Reminder(text) => {
                let n = up(
                    Kind::Event,
                    &format!("reminder:{}", text.to_lowercase()),
                    &format!("Ko asked to be reminded to {text}"),
                    "reminder",
                )?;
                self.link(ko, n, "asked", ctx)?;
                self.log_event("request", "reminder", ctx)?;
            }
            Fact::App(name) => {
                let n = up(
                    Kind::Entity,
                    &format!("app:{name}"),
                    &format!("{name} (app)"),
                    "",
                )?;
                self.link(ko, n, "uses", ctx)?;
                self.log_event("app", name, ctx)?;
            }
        }
        Ok(())
    }

    /// Learns from a desktop/sensor event: `window.opened.<app>` → app
    /// usage; `app.crash` / `*.error` → error memory. Private windows and
    /// anything carrying the safeword are never stored.
    pub fn observe_event(&self, ev: &Event, ctx: &Ctx) -> Observed {
        if !self.is_enabled() {
            return Observed::Disabled;
        }
        let detail = ev.detail.as_deref().unwrap_or("");
        if mentions_safeword(detail) {
            return Observed::Stop;
        }
        let name = ev.name.as_str();
        if name.starts_with("window.private") {
            return Observed::Ignored;
        }
        let r = (|| -> Result<Observed> {
            if let Some(app) = name.strip_prefix("window.opened.") {
                let subject = if app == "app" {
                    detail
                        .split_whitespace()
                        .next()
                        .map(|w| {
                            w.to_lowercase()
                                .chars()
                                .filter(|c| c.is_alphanumeric() || *c == '-')
                                .take(32)
                                .collect::<String>()
                        })
                        .unwrap_or_default()
                } else {
                    app.to_string()
                };
                if subject.is_empty() {
                    return Ok(Observed::Ignored);
                }
                self.log_event("app", &subject, ctx)?;
                self.upsert_node_rate(
                    Kind::Entity,
                    &format!("app:{subject}"),
                    &format!("{subject} (app)"),
                    "",
                    ctx,
                    RATE_WEAK,
                    0.3,
                )?;
                return Ok(Observed::Learned {
                    facts: 1,
                    reinforced: 0,
                });
            }
            if name == "app.crash" || name.ends_with(".crash") || name.ends_with(".error") {
                let text = if detail.is_empty() { name } else { detail };
                self.record_error(text, name, ctx)?;
                return Ok(Observed::Learned {
                    facts: 1,
                    reinforced: 0,
                });
            }
            if name.starts_with("quantumsec.") {
                let subject = if detail.is_empty() {
                    name.trim_start_matches("quantumsec.").to_string()
                } else {
                    detail.chars().take(64).collect()
                };
                self.log_event("quantumsec", &subject, ctx)?;
                return Ok(Observed::Learned {
                    facts: 1,
                    reinforced: 0,
                });
            }
            Ok(Observed::Ignored)
        })();
        let out = r.unwrap_or_else(|e| Observed::Failed(e.to_string()));
        if let Err(e) = self.maybe_maintain(ctx) {
            tracing::warn!("memory: maintenance failed: {e}");
        }
        out
    }

    /// Appends to the habit-mining event log (bounded to `max_events`).
    pub fn log_event(&self, kind: &str, subject: &str, ctx: &Ctx) -> Result<()> {
        let Some(c) = &self.conn else { return Ok(()) };
        let (hour, weekday) = habits::local_hour_weekday(ctx.now, self.offset(ctx));
        self.write_tx(|| {
            c.execute(
                "INSERT INTO events(ts, kind, subject, hour, weekday) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![ctx.now, kind, subject.to_lowercase(), hour, weekday],
            )?;
            c.execute(
                "DELETE FROM events WHERE id <= (SELECT max(id) FROM events) - ?1",
                [self.cfg.max_events.max(1) as i64],
            )?;
            Ok(())
        })
    }

    // ----- anticipation -----------------------------------------------------

    /// Ko's "usually around" profile: logged events of the last
    /// [`HABIT_WINDOW_S`] per 3-hour local bucket
    /// ([`habits::presence_profile`]). Read-only; zeros when memory is off
    /// or the read fails.
    pub fn presence_profile(&self, ctx: &Ctx) -> [f32; 8] {
        match self.recent_events(ctx) {
            Ok(rows) => habits::presence_profile(&rows),
            Err(e) => {
                tracing::warn!("memory: presence profile failed: {e}");
                [0.0; 8]
            }
        }
    }

    /// What Ko recently asked about (`request` event subjects of the last
    /// [`HABIT_WINDOW_S`], newest first, deduped, at most `limit`).
    /// Read-only; empty when memory is off or the read fails.
    pub fn recent_requests(&self, ctx: &Ctx, limit: usize) -> Vec<String> {
        let rows = match self.recent_events(ctx) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("memory: recent requests failed: {e}");
                return Vec::new();
            }
        };
        let mut out: Vec<String> = Vec::new();
        for r in rows {
            if out.len() >= limit {
                break;
            }
            if r.kind == "request" && !out.contains(&r.subject) {
                out.push(r.subject);
            }
        }
        out
    }

    /// Ranked guesses at what Ko will want around `ctx.now` (best first).
    pub fn predict(&self, ctx: &Ctx, limit: usize) -> Vec<Prediction> {
        let rows = match self.recent_events(ctx) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("memory: predict failed: {e}");
                return Vec::new();
            }
        };
        let (h, wd) = habits::local_hour_weekday(ctx.now, self.offset(ctx));
        habits::predict(&rows, h, wd, 2.0, limit)
    }

    fn recent_events(&self, ctx: &Ctx) -> Result<Vec<EventRow>> {
        let Some(c) = &self.conn else {
            return Ok(Vec::new());
        };
        let mut st = c.prepare_cached(
            "SELECT kind, subject, hour, weekday FROM events WHERE ts >= ?1 AND ts <= ?2
             ORDER BY id DESC LIMIT ?3",
        )?;
        let rows = st
            .query_map(
                params![
                    ctx.now - HABIT_WINDOW_S,
                    ctx.now + 1.0,
                    self.cfg.max_events.max(1) as i64
                ],
                |r| {
                    Ok(EventRow {
                        kind: r.get(0)?,
                        subject: r.get(1)?,
                        hour: r.get(2)?,
                        weekday: r.get(3)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Turns strong time-of-day patterns into habit nodes (so retrieval can
    /// mention them). Returns how many habits were upserted.
    pub fn refresh_habits(&self, ctx: &Ctx) -> Result<usize> {
        if !self.is_enabled() {
            return Ok(0);
        }
        let rows = self.recent_events(ctx)?;
        let mut n = 0;
        for (wd, class) in [(1u32, "weekdays"), (5u32, "weekends")] {
            for b in 0..8u32 {
                for p in habits::predict(&rows, b * 3 + 1, wd, 3.0, 3) {
                    if p.confidence < 0.5 {
                        continue;
                    }
                    let key = format!("habit:{}:{}:{b}:{class}", p.kind, p.subject);
                    let label = format!(
                        "Ko usually likes to {} in the {} on {class}",
                        p.describe(),
                        habits::bucket_name(b)
                    );
                    self.upsert_node_rate(
                        Kind::Habit,
                        &key,
                        &label,
                        "",
                        ctx,
                        RATE_WEAK,
                        p.confidence,
                    )?;
                    n += 1;
                }
            }
        }
        Ok(n)
    }

    // ----- error memory -----------------------------------------------------

    /// Records a failure (`text` = the error message, `context` = where it
    /// happened). Returns the signature, recurrence count and any fix that
    /// worked before (`None` when disabled).
    pub fn record_error(
        &self,
        text: &str,
        context: &str,
        ctx: &Ctx,
    ) -> Result<Option<ErrorRecord>> {
        if self.conn.is_none() || mentions_safeword(text) || mentions_safeword(context) {
            return Ok(None);
        }
        self.write_tx(|| self.record_error_rows(text, context, ctx))
    }

    fn record_error_rows(
        &self,
        text: &str,
        context: &str,
        ctx: &Ctx,
    ) -> Result<Option<ErrorRecord>> {
        let c = self.conn.as_ref().expect("enabled");
        let sig = signature(text);
        let sample: String = text.chars().take(300).collect();
        c.execute(
            "INSERT INTO errors(signature, sample, context, count, first_seen, last_seen, updated_at)
             VALUES (?1, ?2, ?3, 1, ?4, ?4, ?4)
             ON CONFLICT(signature) DO UPDATE SET count = count + 1, last_seen = ?4,
               updated_at = CASE WHEN sample IS NOT excluded.sample
                   OR context IS NOT excluded.context THEN ?4 ELSE updated_at END,
               context = excluded.context, sample = excluded.sample",
            params![sig, sample, context, ctx.now],
        )?;
        let rec = self.lookup_signature(&sig)?.expect("just inserted");
        let short: String = text.chars().take(80).collect();
        self.upsert_node(
            Kind::Error,
            &sig,
            &format!("Error ({context}): {short}"),
            &rec.fix
                .as_deref()
                .map(|f| format!("fix: {f}"))
                .unwrap_or_default(),
            ctx,
        )?;
        self.enforce_error_cap()?;
        Ok(Some(ErrorRecord {
            signature: sig,
            count: rec.recurrences,
            known_fix: rec.fix,
        }))
    }

    /// Keeps at most `max_errors` errors (least recently seen dropped, with
    /// their graph nodes). Returns how many were dropped.
    fn enforce_error_cap(&self) -> Result<usize> {
        let Some(c) = &self.conn else { return Ok(0) };
        let cap = self.cfg.max_errors.max(1) as i64;
        let over =
            "SELECT signature FROM errors ORDER BY last_seen DESC, signature LIMIT -1 OFFSET ?1";
        c.execute(
            &format!("DELETE FROM nodes WHERE kind = 'error' AND key IN ({over})"),
            [cap],
        )?;
        Ok(c.execute(
            &format!("DELETE FROM errors WHERE signature IN ({over})"),
            [cap],
        )?)
    }

    /// Records the result of applying `fix` to the error `text`.
    /// Text carrying the safeword is never stored.
    pub fn record_fix(&self, text: &str, fix: &str, outcome: FixOutcome, ctx: &Ctx) -> Result<()> {
        if self.conn.is_none() || mentions_safeword(text) || mentions_safeword(fix) {
            return Ok(());
        }
        self.write_tx(|| self.record_fix_rows(text, fix, outcome, ctx))
    }

    fn record_fix_rows(&self, text: &str, fix: &str, outcome: FixOutcome, ctx: &Ctx) -> Result<()> {
        let c = self.conn.as_ref().expect("enabled");
        let sig = signature(text);
        let cur = match self.lookup_signature(&sig)? {
            Some(cur) => cur,
            None => {
                // Fix for an error never recorded: record it first (once).
                if self.record_error_rows(text, "fix", ctx)?.is_none() {
                    return Ok(());
                }
                match self.lookup_signature(&sig)? {
                    Some(cur) => cur,
                    None => return Ok(()),
                }
            }
        };
        let stored: Option<String> =
            c.query_row("SELECT fix FROM errors WHERE signature = ?1", [&sig], |r| {
                r.get(0)
            })?;
        let same = stored.as_deref() == Some(fix);
        match outcome {
            FixOutcome::Worked => {
                let (ok, fail) = if same {
                    (cur.successes + 1, cur.failures)
                } else {
                    (1, 0)
                };
                let failed: Vec<&String> = cur.failed_fixes.iter().filter(|f| *f != fix).collect();
                c.execute(
                    "UPDATE errors SET fix = ?2, outcome = 'worked', fix_ok = ?3, fix_fail = ?4,
                     failed_fixes = ?5, updated_at = ?6 WHERE signature = ?1",
                    params![
                        sig,
                        fix,
                        ok,
                        fail,
                        failed
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join("\n"),
                        ctx.now
                    ],
                )?;
            }
            FixOutcome::Failed => {
                let mut failed = cur.failed_fixes.clone();
                if !failed.iter().any(|f| f == fix) {
                    failed.push(fix.to_string());
                }
                let start = failed.len().saturating_sub(MAX_FAILED_FIXES);
                let failed = failed[start..].join("\n");
                if same {
                    c.execute(
                        "UPDATE errors SET outcome = 'failed', fix_fail = fix_fail + 1,
                         failed_fixes = ?2, updated_at = ?3 WHERE signature = ?1",
                        params![sig, failed, ctx.now],
                    )?;
                } else {
                    c.execute(
                        "UPDATE errors SET outcome = 'failed', failed_fixes = ?2, updated_at = ?3
                         WHERE signature = ?1",
                        params![sig, failed, ctx.now],
                    )?;
                }
            }
        }
        // Keep the error node's detail in sync.
        if let Some(k) = self.lookup_signature(&sig)? {
            c.execute(
                "UPDATE nodes SET detail = ?2, updated_at = ?3
                 WHERE kind = 'error' AND key = ?1 AND detail IS NOT ?2",
                params![
                    sig,
                    k.fix.map(|f| format!("fix: {f}")).unwrap_or_default(),
                    ctx.now
                ],
            )?;
        }
        let _ = outcome.as_str();
        Ok(())
    }

    /// Everything known about the error `text` (its signature).
    pub fn lookup_error(&self, text: &str) -> Option<KnownFix> {
        self.lookup_signature(&signature(text)).ok().flatten()
    }

    /// The fix that worked for `text` before, if one is trusted
    /// (more successes than failures).
    pub fn known_fix(&self, text: &str) -> Option<String> {
        self.lookup_error(text)?.fix
    }

    fn lookup_signature(&self, sig: &str) -> Result<Option<KnownFix>> {
        let Some(c) = &self.conn else { return Ok(None) };
        Ok(c.query_row(
            "SELECT fix, fix_ok, fix_fail, count, failed_fixes FROM errors WHERE signature = ?1",
            [sig],
            |r| {
                let fix: Option<String> = r.get(0)?;
                let ok: u32 = r.get(1)?;
                let fail: u32 = r.get(2)?;
                let failed: String = r.get(4)?;
                Ok(KnownFix {
                    signature: sig.to_string(),
                    fix: fix.filter(|_| ok > fail),
                    successes: ok,
                    failures: fail,
                    recurrences: r.get(3)?,
                    failed_fixes: failed
                        .lines()
                        .filter(|l| !l.is_empty())
                        .map(str::to_string)
                        .collect(),
                })
            },
        )
        .optional()?)
    }

    // ----- maintenance ------------------------------------------------------

    fn maybe_maintain(&self, ctx: &Ctx) -> Result<()> {
        let last = self.last_maintain.get();
        // A clock that went backwards (or a DB from a skewed machine) must
        // not postpone maintenance forever.
        if last > ctx.now || ctx.now - last >= MAINTAIN_EVERY_S {
            self.maintain(ctx)?;
        }
        Ok(())
    }

    /// Prunes memories whose decayed strength fell below `prune_below`,
    /// enforces `max_nodes` (weakest first), and refreshes habit nodes.
    /// Returns how many nodes were pruned.
    pub fn maintain(&self, ctx: &Ctx) -> Result<usize> {
        let Some(c) = &self.conn else { return Ok(0) };
        let hl = self.half_life_s();
        let floor = self.cfg.prune_below as f64;
        let pruned = self.write_tx(|| self.maintain_rows(c, hl, floor, ctx))?;
        self.last_maintain.set(ctx.now);
        Ok(pruned)
    }

    fn maintain_rows(&self, c: &Connection, hl: f64, floor: f64, ctx: &Ctx) -> Result<usize> {
        let mut all: Vec<(i64, f64)> = {
            let mut st =
                c.prepare("SELECT id, strength, last_seen FROM nodes WHERE key != 'ko'")?;
            let rows = st
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, f64>(1)?,
                        r.get::<_, f64>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter()
                .map(|(id, s, seen)| (id, decayed(s, ctx.now - seen, hl)))
                .collect()
        };
        let mut doomed: Vec<i64> = all
            .iter()
            .filter(|(_, s)| *s < floor)
            .map(|(id, _)| *id)
            .collect();
        all.retain(|(_, s)| *s >= floor);
        let cap = self.cfg.max_nodes.max(1);
        if all.len() > cap {
            all.sort_by(|a, b| a.1.total_cmp(&b.1));
            let extra = all.len() - cap;
            doomed.extend(all.iter().take(extra).map(|(id, _)| *id));
        }
        {
            let mut del = c.prepare_cached("DELETE FROM nodes WHERE id = ?1")?;
            for id in &doomed {
                del.execute([id])?;
            }
        }
        // Weak edges go too.
        let weak: Vec<(i64, i64, String)> = {
            let mut st = c.prepare("SELECT src, dst, relation, strength, last_seen FROM edges")?;
            let rows = st
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, f64>(3)?,
                        r.get::<_, f64>(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter()
                .filter(|r| decayed(r.3, ctx.now - r.4, hl) < floor)
                .map(|r| (r.0, r.1, r.2))
                .collect()
        };
        for (s, d, rel) in weak {
            c.execute(
                "DELETE FROM edges WHERE src = ?1 AND dst = ?2 AND relation = ?3",
                params![s, d, rel],
            )?;
        }
        c.execute(
            "DELETE FROM events WHERE ts < ?1",
            [ctx.now - 2.0 * HABIT_WINDOW_S],
        )?;
        self.enforce_error_cap()?;
        self.refresh_habits(ctx)?;
        c.execute(
            "INSERT INTO meta(key, value) VALUES ('last_maintain', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [ctx.now.to_string()],
        )?;
        Ok(doomed.len())
    }

    /// Runs `f` inside one `BEGIN IMMEDIATE` transaction (the write lock is
    /// taken up front, waiting `busy_timeout` for another process instead of
    /// failing half-way). Already inside a transaction: just runs `f`.
    /// An error rolls everything back.
    pub(crate) fn write_tx<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let Some(c) = &self.conn else { return f() };
        if !c.is_autocommit() {
            return f();
        }
        let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
        let out = f()?;
        tx.commit()?;
        Ok(out)
    }

    // ----- forget -----------------------------------------------------------

    /// Deletes memories for `cmd`, then VACUUMs and checkpoints + truncates
    /// the WAL so nothing lingers in the file bytes. Topic matching is on
    /// whole content words only (stopwords stripped, see
    /// [`content_words`]); nothing is deleted on a substring. The report
    /// says what went and whether the file was fully scrubbed. Logs an
    /// audit line (kind + counts, never content).
    pub fn forget(&self, cmd: &ForgetCmd, ctx: &Ctx) -> Result<ForgetReport> {
        if self.conn.is_none() {
            return Ok(ForgetReport {
                scrubbed: true,
                ..ForgetReport::default()
            });
        }
        self.last_retrieved.borrow_mut().clear();
        let mut r = self.write_tx(|| self.forget_rows(cmd, ctx))?;
        match self.vacuum() {
            Ok(true) => r.scrubbed = true,
            Ok(false) => {
                r.note = Some("the memory file is still open elsewhere (another Lillith window?), so the WAL could not be truncated".into());
            }
            Err(e) => r.note = Some(format!("compacting the file failed: {e}")),
        }
        tracing::info!(
            target: "lillith_memory::audit",
            kind = forget_kind(cmd),
            nodes = r.nodes,
            edges = r.edges,
            events = r.events,
            errors = r.errors,
            scrubbed = r.scrubbed,
            "forget"
        );
        if let Some(log) = &self.audit_log {
            let line = format!(
                "{}\tFORGET\tmemory:{}\tnodes={} edges={} events={} errors={} scrubbed={}\n",
                format_utc(ctx.now),
                forget_kind(cmd),
                r.nodes,
                r.edges,
                r.events,
                r.errors,
                r.scrubbed
            );
            if let Err(e) = append_private(log, &line) {
                tracing::warn!("memory: could not write {}: {e}", log.display());
            }
        }
        Ok(r)
    }

    /// What [`forget`](Self::forget) would delete, without deleting anything
    /// (the deletes run in a transaction that is rolled back).
    pub fn forget_preview(&self, cmd: &ForgetCmd, ctx: &Ctx) -> Result<ForgetReport> {
        let Some(c) = &self.conn else {
            return Ok(ForgetReport::default());
        };
        let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
        let r = self.forget_rows(cmd, ctx);
        tx.rollback()?;
        r
    }

    fn forget_rows(&self, cmd: &ForgetCmd, ctx: &Ctx) -> Result<ForgetReport> {
        let c = self.conn.as_ref().expect("enabled");
        let edges_before = self.stats().edges as usize;
        let mut r = ForgetReport::default();
        let (nodes, sigs, events): (Vec<i64>, Vec<String>, Vec<i64>) = match cmd {
            ForgetCmd::All => {
                let s = self.stats();
                c.execute_batch(
                    "DELETE FROM edges; DELETE FROM nodes; DELETE FROM events;
                     DELETE FROM errors; DELETE FROM meta;",
                )?;
                r.nodes = s.nodes as usize;
                r.events = s.events as usize;
                r.errors = s.errors as usize;
                (Vec::new(), Vec::new(), Vec::new())
            }
            ForgetCmd::LastSecs(_) | ForgetCmd::LastTurn => {
                let secs = cmd.window_secs().unwrap_or(0);
                let cut = ctx.now - secs as f64;
                let ids = c
                    .prepare(
                        "SELECT id FROM nodes WHERE key != 'ko' AND (created_at >= ?1 OR updated_at >= ?1)",
                    )?
                    .query_map([cut], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<i64>>>()?;
                let sigs = c
                    .prepare(
                        "SELECT signature FROM errors WHERE first_seen >= ?1 OR updated_at >= ?1",
                    )?
                    .query_map([cut], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<String>>>()?;
                let ev = c
                    .prepare("SELECT id FROM events WHERE ts >= ?1")?
                    .query_map([cut], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<i64>>>()?;
                c.execute("DELETE FROM edges WHERE created_at >= ?1", [cut])?;
                (ids, sigs, ev)
            }
            ForgetCmd::Topic(topic) => {
                let words = content_words(topic);
                if words.is_empty() {
                    return Ok(r);
                }
                // A row matches when key, label and detail together hold the
                // whole topic: every content word for 1-2 words, else two
                // thirds of them (`matches_topic`). Whole words only (plus
                // the exact singular/plural), never FTS prefixes or LIKE:
                // "new job" never hits "new wave music", "news" never "new".
                let mut ids: Vec<i64> = Vec::new();
                // Values of matched attributes ("my ex" → "sarah"): the raw
                // event log and a bare entity of that name go too, matched
                // by the whole value as one phrase ("google engineer" never
                // hits "google chrome").
                let mut values: Vec<String> = Vec::new();
                {
                    let mut st =
                        c.prepare("SELECT id, key, label, detail FROM nodes WHERE key != 'ko'")?;
                    let rows = st.query_map([], |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                        ))
                    })?;
                    for row in rows {
                        let (id, key, label, detail) = row?;
                        // An attribute whose name IS one of the topic's
                        // words ("my new job" → `attr:job`) matches too:
                        // the whole key, never a part of it.
                        let attr_hit = key.strip_prefix("attr:").is_some_and(|a| {
                            let a = a.trim().to_lowercase();
                            words.iter().any(|w| same_word(&a, w))
                        });
                        if attr_hit || matches_topic(&[&key, &label, &detail], &words) {
                            ids.push(id);
                            if key.starts_with("attr:") && !detail.trim().is_empty() {
                                values.push(detail.trim().to_lowercase());
                            }
                        }
                    }
                }
                // Preferences/attributes hanging off a matched entity.
                let mut extra = Vec::new();
                for id in &ids {
                    let mut st = c.prepare_cached(
                        "SELECT src FROM edges WHERE dst = ?1 AND relation = 'about'",
                    )?;
                    let found = st
                        .query_map([id], |r| r.get::<_, i64>(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    extra.extend(found);
                }
                ids.extend(extra);
                for v in &values {
                    let mut st = c.prepare_cached(
                        "SELECT id FROM nodes WHERE kind = 'entity' AND key = ?1",
                    )?;
                    let found = st
                        .query_map([v], |r| r.get::<_, i64>(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    ids.extend(found);
                }
                let sigs = {
                    let mut st = c.prepare(
                        "SELECT signature, sample, context, coalesce(fix, ''), failed_fixes FROM errors",
                    )?;
                    let rows = st.query_map([], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, String>(4)?,
                        ))
                    })?;
                    let mut sigs = Vec::new();
                    for row in rows {
                        let (sig, a, b, f, ff) = row?;
                        if matches_topic(&[&a, &b, &f, &ff], &words) {
                            sigs.push(sig);
                        }
                    }
                    sigs
                };
                let ev = {
                    let mut st = c.prepare("SELECT id, subject FROM events")?;
                    let rows =
                        st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
                    let mut ev = Vec::new();
                    for row in rows {
                        let (id, subject) = row?;
                        if matches_topic(&[&subject], &words)
                            || values.iter().any(|v| contains_phrase(&subject, v))
                        {
                            ev.push(id);
                        }
                    }
                    ev
                };
                (ids, sigs, ev)
            }
        };
        if !matches!(cmd, ForgetCmd::All) {
            r.examples = self.forget_examples(&nodes, &sigs, &events)?;
            // An error goes with its graph node and vice versa.
            let mut sigs = sigs;
            for id in &nodes {
                let key: Option<String> = c
                    .query_row(
                        "SELECT key FROM nodes WHERE id = ?1 AND kind = 'error'",
                        [id],
                        |r| r.get(0),
                    )
                    .optional()?;
                sigs.extend(key);
            }
            sigs.sort_unstable();
            sigs.dedup();
            for sig in &sigs {
                r.nodes +=
                    c.execute("DELETE FROM nodes WHERE kind = 'error' AND key = ?1", [sig])?;
                r.errors += c.execute("DELETE FROM errors WHERE signature = ?1", [sig])?;
            }
            for id in &nodes {
                r.nodes += c.execute("DELETE FROM nodes WHERE id = ?1 AND key != 'ko'", [id])?;
            }
            for id in &events {
                r.events += c.execute("DELETE FROM events WHERE id = ?1", [id])?;
            }
        }
        // Rebuild the FTS index from what is left so no deleted token
        // survives in an index segment.
        c.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES ('rebuild');")?;
        r.edges = edges_before.saturating_sub(self.stats().edges as usize);
        Ok(r)
    }

    /// Short labels of up to [`FORGET_EXAMPLES`] rows about to be deleted
    /// (nodes first, then errors, then events), deduplicated.
    fn forget_examples(
        &self,
        nodes: &[i64],
        sigs: &[String],
        events: &[i64],
    ) -> Result<Vec<String>> {
        let c = self.conn.as_ref().expect("enabled");
        let mut out: Vec<String> = Vec::new();
        let mut push = |s: String| {
            let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
            let s = if s.chars().count() > EXAMPLE_CHARS {
                let cut: String = s.chars().take(EXAMPLE_CHARS - 1).collect();
                format!("{}…", cut.trim_end())
            } else {
                s
            };
            if !s.is_empty() && !out.contains(&s) && out.len() < FORGET_EXAMPLES {
                out.push(s);
            }
        };
        for id in nodes {
            let label: Option<String> = c
                .query_row(
                    "SELECT label FROM nodes WHERE id = ?1 AND key != 'ko'",
                    [id],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(l) = label {
                push(l);
            }
        }
        for sig in sigs {
            let sample: Option<String> = c
                .query_row(
                    "SELECT sample FROM errors WHERE signature = ?1",
                    [sig],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(sm) = sample {
                push(format!("error: {sm}"));
            }
        }
        for id in events {
            let ev: Option<(String, String)> = c
                .query_row(
                    "SELECT kind, subject FROM events WHERE id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((k, subj)) = ev {
                push(format!("{k}: {subj}"));
            }
        }
        Ok(out)
    }

    /// Compacts the file and checkpoints + truncates the WAL so deleted
    /// data is gone from disk. `Ok(false)` when another connection kept the
    /// WAL busy (rows are deleted, but old pages may linger until it closes).
    pub fn vacuum(&self) -> Result<bool> {
        let Some(c) = &self.conn else { return Ok(true) };
        // A reader elsewhere makes TRUNCATE wait on the busy handler: keep
        // that short here (the result is reported, not hidden).
        c.busy_timeout(std::time::Duration::from_millis(250))?;
        let r = (|| -> Result<bool> {
            let ok1 = checkpoint_truncate(c)?;
            c.execute_batch("VACUUM;")?;
            let ok2 = checkpoint_truncate(c)?;
            Ok(ok1 && ok2)
        })();
        c.busy_timeout(BUSY_TIMEOUT)?;
        r
    }

    /// Row counts (zeros when disabled).
    pub fn stats(&self) -> Stats {
        let Some(c) = &self.conn else {
            return Stats::default();
        };
        let n = |t: &str| -> u64 {
            c.query_row(&format!("SELECT count(*) FROM {t}"), [], |r| {
                r.get::<_, i64>(0)
            })
            .map(|v| v as u64)
            .unwrap_or(0)
        };
        Stats {
            nodes: n("nodes"),
            edges: n("edges"),
            events: n("events"),
            errors: n("errors"),
        }
    }
}

/// Formats memories (+ a short anticipation line) as a bullet block within
/// `max_chars`. `None` if nothing fits.
pub(crate) fn format_block(
    mems: &[Memory],
    preds: &[Prediction],
    max_chars: usize,
) -> Option<String> {
    const HEADER: &str =
        "[Memory: things you remember about Ko. Use only if relevant; never recite.]";
    let mut out = String::from(HEADER);
    let mut lines = 0;
    let add = |line: String, out: &mut String| -> bool {
        let line: String = line.chars().take(180).collect();
        if out.len() + 1 + line.len() > max_chars {
            return false;
        }
        out.push('\n');
        out.push_str(&line);
        true
    };
    for m in mems {
        let mut l = format!("- {}", m.label);
        if !m.detail.is_empty() && matches!(m.kind, Kind::Error | Kind::Note) {
            l.push_str(&format!(" ({})", m.detail));
        }
        if add(l, &mut out) {
            lines += 1;
        }
    }
    for p in preds {
        let l = format!(
            "- Around now Ko often wants to {} ({:.0}% sure)",
            p.describe(),
            p.confidence * 100.0
        );
        if add(l, &mut out) {
            lines += 1;
        }
    }
    (lines > 0).then_some(out)
}

/// `PRAGMA wal_checkpoint(TRUNCATE)`, retried once; `true` when every frame
/// was checkpointed and the WAL truncated (or the DB is not in WAL mode).
fn checkpoint_truncate(c: &Connection) -> Result<bool> {
    for attempt in 0..2 {
        let (busy, log, done): (i64, i64, i64) =
            c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
        if busy == 0 && (log <= 0 || done == log) {
            return Ok(true);
        }
        tracing::debug!(
            busy,
            log,
            done,
            attempt,
            "memory: WAL checkpoint incomplete"
        );
        if attempt == 0 {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    Ok(false)
}

/// The audit name of a forget command (never its topic text).
fn forget_kind(cmd: &ForgetCmd) -> &'static str {
    match cmd {
        ForgetCmd::All => "all",
        ForgetCmd::Topic(_) => "topic",
        ForgetCmd::LastSecs(_) => "last_secs",
        ForgetCmd::LastTurn => "last_turn",
    }
}

/// Appends `line` to `path`, creating the file 0600 on unix.
fn append_private(path: &Path, line: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut o = std::fs::OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)?.write_all(line.as_bytes())
}

/// Unix seconds → `YYYY-MM-DDTHH:MM:SSZ` (proleptic Gregorian).
fn format_utc(t: f64) -> String {
    let secs = t.max(0.0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Creates the DB's folder (0700 on unix) and the file itself (0600 on
/// unix, before SQLite opens it, so it is never world-readable even for a
/// moment). An existing file is tightened to 0600. Errors propagate.
fn prepare_private_file(path: &Path) -> Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
        #[cfg(not(unix))]
        std::fs::create_dir_all(dir)?;
    }
    let mut o = std::fs::OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    drop(o.open(path)?);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod internal_tests {
    use super::*;

    #[test]
    fn audit_timestamps_are_utc_rfc3339() {
        assert_eq!(format_utc(0.0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(951_782_400.0), "2000-02-29T00:00:00Z");
        assert_eq!(format_utc(1_792_368_000.5), "2026-10-19T00:00:00Z");
    }

    #[test]
    fn maintenance_recovers_from_a_future_last_maintain() {
        let b = Brain::open_in_memory(&MemoryConfig::default()).unwrap();
        let t = 1_792_368_000.0;
        b.last_maintain.set(t + 1.0e9); // clock was skewed far ahead
        b.observe_interaction("I love tea", "", &Ctx::at(t).with_utc_offset(0));
        assert_eq!(
            b.last_maintain.get(),
            t,
            "maintenance ran and reset the clock"
        );
    }

    #[test]
    fn write_tx_nests_and_rolls_back() {
        let b = Brain::open_in_memory(&MemoryConfig::default()).unwrap();
        let ctx = Ctx::at(10.0);
        let r: Result<()> = b.write_tx(|| {
            b.write_tx(|| b.upsert_node(Kind::Entity, "x", "x", "", &ctx).map(|_| ()))?;
            Err(Error::NoDataDir)
        });
        assert!(r.is_err());
        assert_eq!(
            b.stats().nodes,
            0,
            "inner write rolled back with the outer tx"
        );
        assert!(b.conn.as_ref().unwrap().is_autocommit());
    }
}
