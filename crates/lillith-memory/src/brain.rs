//! The memory graph itself.

use crate::decay::{decayed, recency, reinforce};
use crate::errors::{signature, ErrorRecord, FixOutcome, KnownFix};
use crate::extract::{intent_key, keywords, Extractor, Fact, Polarity, RuleExtractor};
use crate::forget::{parse_forget, ForgetCmd};
use crate::habits::{self, EventRow, Prediction};
use crate::safeword::{is_safeword, mentions_safeword};
use crate::{schema, Error, Kind};
use lillith_core::config::MemoryConfig;
use lillith_core::event::Event;
use rusqlite::{params, Connection, OptionalExtension};
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

/// Lillith's memory. `Send` (move it to the chat worker), not `Sync`.
pub struct Brain {
    conn: Option<Connection>,
    cfg: MemoryConfig,
    path: Option<PathBuf>,
    extractor: Box<dyn Extractor>,
    last_retrieved: RefCell<Vec<i64>>,
    local_offset: Cell<Option<i32>>,
    last_maintain: Cell<f64>,
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
        Self::open_path(&path, cfg)
    }

    /// Opens (creating) the DB at `path`. Still honours `cfg.enabled`.
    pub fn open_path(path: &Path, cfg: &MemoryConfig) -> Result<Self> {
        if !cfg.enabled {
            return Ok(Self::disabled(cfg));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
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
        }
    }

    fn init(mut conn: Connection, cfg: &MemoryConfig) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_millis(2000))?;
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
        })
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
                        "UPDATE nodes SET label = ?2, detail = ?3 WHERE id = ?1",
                        params![id, label, detail],
                    )?;
                }
                Ok(Some(id))
            }
            None => {
                c.execute(
                    "INSERT INTO nodes(kind, key, label, detail, strength, access_count, created_at, last_seen)
                     VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6)",
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
                 ORDER BY strength DESC LIMIT 4",
            )?;
            let rows: Vec<(i64, String, String, String, f64, f64)> = st
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

    /// The compact block to prepend to the system prompt for `query`, within
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
        let c = self.conn.as_ref().expect("enabled");
        let tx = c.unchecked_transaction()?;
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
        tx.commit()?;
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
        c.execute(
            "INSERT INTO events(ts, kind, subject, hour, weekday) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![ctx.now, kind, subject.to_lowercase(), hour, weekday],
        )?;
        c.execute(
            "DELETE FROM events WHERE id <= (SELECT max(id) FROM events) - ?1",
            [self.cfg.max_events.max(1) as i64],
        )?;
        Ok(())
    }

    // ----- anticipation -----------------------------------------------------

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
        let Some(c) = &self.conn else { return Ok(None) };
        if mentions_safeword(text) {
            return Ok(None);
        }
        let sig = signature(text);
        let sample: String = text.chars().take(300).collect();
        c.execute(
            "INSERT INTO errors(signature, sample, context, count, first_seen, last_seen)
             VALUES (?1, ?2, ?3, 1, ?4, ?4)
             ON CONFLICT(signature) DO UPDATE SET count = count + 1, last_seen = ?4,
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
        Ok(Some(ErrorRecord {
            signature: sig,
            count: rec.recurrences,
            known_fix: rec.fix,
        }))
    }

    /// Records the result of applying `fix` to the error `text`.
    pub fn record_fix(&self, text: &str, fix: &str, outcome: FixOutcome, ctx: &Ctx) -> Result<()> {
        let Some(c) = &self.conn else { return Ok(()) };
        let sig = signature(text);
        let Some(cur) = self.lookup_signature(&sig)? else {
            // Fix for an error never recorded: record it first.
            self.record_error(text, "fix", ctx)?;
            return self.record_fix(text, fix, outcome, ctx);
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
                     failed_fixes = ?5 WHERE signature = ?1",
                    params![
                        sig,
                        fix,
                        ok,
                        fail,
                        failed
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
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
                         failed_fixes = ?2 WHERE signature = ?1",
                        params![sig, failed],
                    )?;
                } else {
                    c.execute(
                        "UPDATE errors SET outcome = 'failed', failed_fixes = ?2 WHERE signature = ?1",
                        params![sig, failed],
                    )?;
                }
            }
        }
        // Keep the error node's detail in sync.
        if let Some(k) = self.lookup_signature(&sig)? {
            c.execute(
                "UPDATE nodes SET detail = ?2 WHERE kind = 'error' AND key = ?1",
                params![sig, k.fix.map(|f| format!("fix: {f}")).unwrap_or_default()],
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
        if ctx.now - self.last_maintain.get() >= MAINTAIN_EVERY_S {
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
        let tx = c.unchecked_transaction()?;
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
        tx.commit()?;
        self.refresh_habits(ctx)?;
        c.execute(
            "INSERT INTO meta(key, value) VALUES ('last_maintain', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [ctx.now.to_string()],
        )?;
        self.last_maintain.set(ctx.now);
        Ok(doomed.len())
    }

    // ----- forget -----------------------------------------------------------

    /// Deletes memories for `cmd` and VACUUMs so nothing lingers on disk.
    /// Returns how many rows (nodes + events + errors) were removed.
    pub fn forget(&self, cmd: &ForgetCmd, ctx: &Ctx) -> Result<usize> {
        let Some(c) = &self.conn else { return Ok(0) };
        self.last_retrieved.borrow_mut().clear();
        let tx = c.unchecked_transaction()?;
        let n = match cmd {
            ForgetCmd::All => {
                let s = self.stats();
                c.execute_batch(
                    "DELETE FROM edges; DELETE FROM nodes; DELETE FROM events;
                     DELETE FROM errors; DELETE FROM meta;
                     INSERT INTO nodes_fts(nodes_fts) VALUES ('rebuild');",
                )?;
                (s.nodes + s.events + s.errors) as usize
            }
            ForgetCmd::LastSecs(secs) => {
                let cut = ctx.now - *secs as f64;
                c.execute("DELETE FROM edges WHERE created_at >= ?1", [cut])?;
                c.execute("DELETE FROM nodes WHERE created_at >= ?1", [cut])?
                    + c.execute("DELETE FROM events WHERE ts >= ?1", [cut])?
                    + c.execute("DELETE FROM errors WHERE first_seen >= ?1", [cut])?
            }
            ForgetCmd::Topic(topic) => {
                let mut words = keywords(topic, 6);
                if words.is_empty() {
                    words = topic
                        .to_lowercase()
                        .split(|ch: char| !ch.is_alphanumeric())
                        .filter(|w| w.len() >= 2)
                        .map(str::to_string)
                        .collect();
                }
                if words.is_empty() {
                    0
                } else {
                    let expr = words
                        .iter()
                        .map(|w| format!("\"{w}\"*"))
                        .collect::<Vec<_>>()
                        .join(" AND ");
                    let mut ids: Vec<i64> = {
                        let mut st =
                            c.prepare("SELECT rowid FROM nodes_fts WHERE nodes_fts MATCH ?1")?;
                        let r = st
                            .query_map([expr], |r| r.get(0))?
                            .collect::<rusqlite::Result<Vec<i64>>>()?;
                        r
                    };
                    // Preferences/attributes hanging off a matched entity.
                    let mut extra = Vec::new();
                    for id in &ids {
                        let mut st = c.prepare_cached(
                            "SELECT e.src FROM edges e JOIN nodes n ON n.id = e.src
                             WHERE e.dst = ?1 AND e.relation = 'about'",
                        )?;
                        let r = st
                            .query_map([id], |r| r.get::<_, i64>(0))?
                            .collect::<rusqlite::Result<Vec<_>>>()?;
                        extra.extend(r);
                    }
                    ids.extend(extra);
                    ids.sort_unstable();
                    ids.dedup();
                    let mut n = 0;
                    for id in ids {
                        n += c.execute("DELETE FROM nodes WHERE id = ?1 AND key != 'ko'", [id])?;
                    }
                    for w in &words {
                        let pat = format!("%{w}%");
                        n += c.execute("DELETE FROM events WHERE subject LIKE ?1", [&pat])?;
                        n += c.execute(
                            "DELETE FROM errors WHERE sample LIKE ?1 OR context LIKE ?1 OR fix LIKE ?1",
                            [&pat],
                        )?;
                    }
                    n
                }
            }
        };
        c.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES ('optimize');")?;
        tx.commit()?;
        self.vacuum()?;
        Ok(n)
    }

    /// Compacts the file and truncates the WAL so deleted data is gone.
    pub fn vacuum(&self) -> Result<()> {
        let Some(c) = &self.conn else { return Ok(()) };
        c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")?;
        let _ = c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
        Ok(())
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
        if !m.detail.is_empty() && matches!(m.kind, Kind::Error) {
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
