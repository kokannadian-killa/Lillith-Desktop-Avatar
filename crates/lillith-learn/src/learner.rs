//! One learning step (§3, steps 1–5):
//!
//! 1. **pick** a source without the model: usefulness × novelty ÷ size;
//! 2. **read** one ≈2 KiB chunk of it (installed docs only);
//! 3. **extract**: one capped model call → ≤3 strict JSON-line items
//!    (invalid → discarded, `tries + 1`);
//! 4. **dedupe** through the FTS index (same normalized title or ≥0.8
//!    token-Jaccard → merge);
//! 5. **write** the note (to `inbox/` unless the keyword classifier is
//!    sure), index it as a `note` node and **link** it.
//!
//! Every write logs one `LEARN` audit line (never the note text). A step
//! stopped mid-way (safeword, activity, chat) keeps nothing of its output.

use crate::classify::classify;
use crate::dedupe::same_note;
use crate::extract::{self, Item};
use crate::readers::{chunk, read_text, Candidate, DocRoots};
use crate::store::{Note, NoteStore};
use crate::summarize::{SumError, Summarizer};
use crate::{audit, fnv1a, CHUNK_BYTES};
use lillith_core::config::{Config, LearnConfig};
use lillith_memory::{Brain, Ctx, NoteRow};
use std::collections::HashMap;
use std::path::PathBuf;

/// `chunks_done` value of a source read to the end.
pub const DONE: u32 = u32::MAX;
/// Failed tries before a chunk is skipped.
pub const MAX_TRIES: u32 = 3;
/// Sources are rescanned this often.
const RESCAN_S: f64 = 24.0 * 3600.0;
/// Picks tried per step before giving up (unreadable / finished sources).
const PICK_TRIES: usize = 8;

/// What a step ended with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Notes written or merged.
    Wrote,
    /// `--dry-run`: nothing written.
    DryRun,
    /// No source left to read.
    NothingToRead,
    /// The model wasn't asked or its reply was dropped (chat, safeword,
    /// activity, no server); nothing kept.
    Discarded(SumError),
    /// The reply wasn't valid JSON lines (`tries + 1`).
    Invalid,
    /// Valid reply, but nothing survived the content filter.
    Empty,
    /// Another writer holds `knowledge/.lock`.
    WriterBusy,
}

/// One note written (or that would be, in a dry run).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// `write` | `merge`.
    pub op: &'static str,
    pub topic: String,
    pub id: String,
    pub title: String,
}

/// What a step did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepReport {
    pub outcome: Outcome,
    pub source: Option<String>,
    pub chunk: usize,
    pub chunk_preview: String,
    pub items: Vec<Item>,
    pub written: Vec<Written>,
    pub tokens: u32,
}

impl StepReport {
    fn new(outcome: Outcome) -> Self {
        Self {
            outcome,
            source: None,
            chunk: 0,
            chunk_preview: String::new(),
            items: Vec::new(),
            written: Vec::new(),
            tokens: 0,
        }
    }
}

struct Picked {
    cand: Candidate,
    hash: String,
    idx: usize,
    text: String,
}

/// The learner (one per thread; owns its own memory connection).
pub struct Learner {
    cfg: LearnConfig,
    brain: Brain,
    knowledge_dir: Option<PathBuf>,
    store: Option<NoteStore>,
    docs: DocRoots,
    roots: crate::paths::Roots,
    candidates: Vec<Candidate>,
    scanned_at: f64,
    audit_log: Option<PathBuf>,
    dry_run: bool,
}

impl std::fmt::Debug for Learner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Learner")
            .field("knowledge_dir", &self.knowledge_dir)
            .field("candidates", &self.candidates.len())
            .field("dry_run", &self.dry_run)
            .finish_non_exhaustive()
    }
}

impl Learner {
    /// A learner for `cfg` over the system doc locations.
    pub fn new(cfg: &Config, brain: Brain) -> Self {
        let docs = DocRoots::system();
        Self {
            cfg: cfg.learn.clone(),
            brain,
            knowledge_dir: cfg.mind.resolved_knowledge_dir(),
            store: None,
            roots: docs.confinement(),
            docs,
            candidates: Vec::new(),
            scanned_at: f64::NEG_INFINITY,
            audit_log: lillith_core::config::audit_log_path(),
            dry_run: false,
        }
    }

    /// Reads from `docs` instead of the system locations.
    pub fn with_docs(mut self, docs: DocRoots) -> Self {
        self.roots = docs.confinement();
        self.docs = docs;
        self.candidates.clear();
        self.scanned_at = f64::NEG_INFINITY;
        self
    }

    pub fn with_knowledge_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.knowledge_dir = dir;
        self.store = None;
        self
    }

    pub fn with_audit_log(mut self, log: Option<PathBuf>) -> Self {
        self.audit_log = log;
        self
    }

    /// Dry run: pick, read and ask, but write nothing (no note, no index
    /// row, no audit line).
    pub fn dry_run(mut self, on: bool) -> Self {
        self.dry_run = on;
        self
    }

    pub fn brain(&self) -> &Brain {
        &self.brain
    }

    fn store(&mut self) -> std::io::Result<&NoteStore> {
        if self.store.is_none() {
            let dir = self.knowledge_dir.clone().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "no knowledge folder")
            })?;
            self.store = Some(NoteStore::open(
                &dir,
                &self.cfg.topics,
                self.cfg.allow_create_topics,
            )?);
        }
        Ok(self.store.as_ref().expect("opened above"))
    }

    fn rescan(&mut self, now: f64) {
        if self.candidates.is_empty() || now - self.scanned_at > RESCAN_S {
            self.candidates = self
                .docs
                .discover(&self.cfg.sources, self.cfg.max_file_bytes.min(256 * 1024));
            self.scanned_at = now;
        }
    }

    /// Candidates best first (step 1, no model).
    fn ranked(&self) -> Vec<usize> {
        let sources = self.brain.all_sources();
        let useful: HashMap<String, f64> = self.brain.useful_words(256).into_iter().collect();
        let mut scored: Vec<(f64, usize)> = self
            .candidates
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let novelty = match sources.get(&c.reference) {
                    None => 1.0,
                    Some(s) if s.chunks_done == DONE => return None,
                    Some(s) => 0.6 / (1.0 + 0.25 * f64::from(s.chunks_done)),
                };
                let usefulness = c.kind.weight()
                    * (1.0 + useful.get(&c.name.to_lowercase()).copied().unwrap_or(0.0));
                let size = (std::f64::consts::E + c.size as f64 / 1024.0).ln();
                Some((usefulness * novelty / size, i))
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    self.candidates[a.1]
                        .reference
                        .cmp(&self.candidates[b.1].reference)
                })
        });
        scored.into_iter().map(|(_, i)| i).collect()
    }

    fn mark(&self, c: &Candidate, hash: &str, done: u32) {
        if self.dry_run {
            return;
        }
        if let Err(e) =
            self.brain
                .mark_source(&c.reference, c.kind.as_str(), hash, done, &Ctx::now())
        {
            tracing::debug!("learn: mark source: {e}");
        }
    }

    /// Steps 1–2: the best source and its next chunk.
    fn pick(&mut self) -> Option<Picked> {
        self.rescan(crate::now_s());
        let order = self.ranked();
        let sources = self.brain.all_sources();
        for i in order.into_iter().take(PICK_TRIES) {
            let cand = self.candidates[i].clone();
            let text = match read_text(&cand, &self.roots, self.cfg.max_file_bytes.min(256 * 1024))
            {
                Ok(t) if !t.trim().is_empty() => t,
                _ => {
                    self.mark(&cand, "unreadable", DONE);
                    continue;
                }
            };
            let hash = fnv1a(text.as_bytes());
            let mut idx = match sources.get(&cand.reference) {
                Some(s) if s.hash == hash => s.chunks_done as usize,
                _ => 0,
            };
            // Skip chunks that failed too often.
            while self
                .brain
                .learn_job_tries("chunk", &format!("{}#{idx}", cand.reference))
                >= MAX_TRIES
            {
                idx += 1;
            }
            if chunk(&text, idx, CHUNK_BYTES).is_none() {
                self.mark(&cand, &hash, DONE);
                continue;
            }
            return Some(Picked {
                cand,
                hash,
                idx,
                text,
            });
        }
        None
    }

    /// Runs one step with `sum`; `stop` is polled during the model call and
    /// checked again before anything is written.
    pub fn step(&mut self, sum: &mut dyn Summarizer, stop: &dyn Fn() -> bool) -> StepReport {
        if !self.brain.is_enabled() {
            return StepReport::new(Outcome::NothingToRead);
        }
        let Some(mut p) = self.pick() else {
            return StepReport::new(Outcome::NothingToRead);
        };
        let piece = chunk(&p.text, p.idx, CHUNK_BYTES).unwrap_or_default();
        let job = format!("{}#{}", p.cand.reference, p.idx);
        let mut rep = StepReport::new(Outcome::Empty);
        rep.source = Some(p.cand.reference.clone());
        rep.chunk = p.idx;
        rep.chunk_preview = piece.chars().take(160).collect();
        p.text = String::new();

        // Step 3: one capped model call.
        let reply = match sum.summarize(&extract::messages(&p.cand.reference, &piece), stop) {
            Ok(r) => r,
            Err(e) => {
                if matches!(e, SumError::Failed(_)) && !self.dry_run {
                    let _ = self
                        .brain
                        .record_learn_job("chunk", &job, "failed", true, &Ctx::now());
                }
                rep.outcome = Outcome::Discarded(e);
                return rep;
            }
        };
        rep.tokens = reply.tokens;
        if stop() {
            // Safeword / activity after the reply: keep nothing.
            rep.outcome = Outcome::Discarded(SumError::Stopped);
            return rep;
        }
        let items = match extract::parse(&reply.text) {
            Ok(items) => items,
            Err(_) => {
                rep.outcome = Outcome::Invalid;
                if !self.dry_run {
                    let tries = self
                        .brain
                        .record_learn_job("chunk", &job, "invalid", true, &Ctx::now())
                        .unwrap_or(0);
                    if tries >= MAX_TRIES {
                        self.mark(&p.cand, &p.hash, (p.idx + 1) as u32);
                    }
                }
                return rep;
            }
        };
        rep.items = items.clone();
        if self.dry_run {
            rep.written = items
                .iter()
                .map(|it| Written {
                    op: "write",
                    topic: self.topic_for(it, &p.cand),
                    id: String::new(),
                    title: it.title.clone(),
                })
                .collect();
            rep.outcome = Outcome::DryRun;
            return rep;
        }
        if items.is_empty() {
            self.mark(&p.cand, &p.hash, (p.idx + 1) as u32);
            let _ = self
                .brain
                .record_learn_job("chunk", &job, "done", false, &Ctx::now());
            return rep;
        }

        // Steps 4–5 under the single-writer lock.
        let lock = match self.store().and_then(|s| s.lock()) {
            Ok(Some(l)) => l,
            Ok(None) => {
                rep.outcome = Outcome::WriterBusy;
                return rep;
            }
            Err(e) => {
                tracing::warn!("learn: knowledge folder: {e}");
                rep.outcome = Outcome::WriterBusy;
                return rep;
            }
        };
        for it in &items {
            if stop() {
                break;
            }
            match self.write_item(&lock, it, &p) {
                Ok(w) => rep.written.push(w),
                Err(e) => tracing::warn!("learn: note not written: {e}"),
            }
        }
        drop(lock);
        self.mark(&p.cand, &p.hash, (p.idx + 1) as u32);
        let _ = self
            .brain
            .record_learn_job("chunk", &job, "done", false, &Ctx::now());
        rep.outcome = Outcome::Wrote;
        rep
    }

    fn topic_for(&self, it: &Item, c: &Candidate) -> String {
        let text = format!("{} {} {}", it.title, it.body, it.links.join(" "));
        let t = classify(&text, c.kind);
        match &self.store {
            Some(s) => s.resolve_topic(t),
            None => {
                let topics = &self.cfg.topics;
                match t {
                    Some(t) if topics.iter().any(|x| x == t) => t.to_string(),
                    Some(t) if self.cfg.allow_create_topics && crate::store::slug_ok(t) => {
                        t.to_string()
                    }
                    _ => crate::store::INBOX.to_string(),
                }
            }
        }
    }

    fn write_item(
        &mut self,
        lock: &crate::store::WriteLock,
        it: &Item,
        p: &Picked,
    ) -> std::io::Result<Written> {
        let now = crate::now_s();
        let ctx = Ctx::at(now);
        let topic = self.topic_for(it, &p.cand);
        let dup = self
            .brain
            .similar_notes(&it.title, 5)
            .into_iter()
            .find(|r| same_note(&r.title, &it.title));
        let store = self.store()?.clone();
        let existing = dup.as_ref().and_then(|r| store.read(&r.path));
        let (op, note) = match existing {
            Some(mut n) => {
                if !n.body.contains(it.body.trim()) {
                    n.body = format!("{}\n\n{}", n.body.trim_end(), it.body.trim());
                }
                for l in &it.links {
                    if !n.links.contains(l) {
                        n.links.push(l.clone());
                    }
                }
                n.links.truncate(16);
                n.updated = now;
                n.confidence = (n.confidence + 0.05).min(0.95);
                ("merge", n)
            }
            None => (
                "write",
                Note {
                    id: new_id(&p.cand.reference, &it.title, now),
                    title: it.title.clone(),
                    topic,
                    kind: it.kind.clone(),
                    source_type: p.cand.kind.as_str().into(),
                    source_ref: p.cand.reference.clone(),
                    source_hash: p.hash.clone(),
                    created: now,
                    updated: now,
                    confidence: 0.5,
                    quiz_asked: 0,
                    quiz_passed: 0,
                    links: it.links.clone(),
                    origin: "learned".into(),
                    body: it.body.clone(),
                },
            ),
        };
        let path = store.write(lock, &note)?;
        let rel = store.rel(&path);
        let topic = rel.split('/').next().unwrap_or("inbox").to_string();
        let node = match self
            .brain
            .upsert_note(&row_of(&note, &rel, &topic), &note.body, &ctx)
        {
            Ok(node) => node,
            Err(e) => {
                // The DB step failed. Don't leave a note that isn't indexed or
                // audited: roll back a brand-new file (a merge updated an
                // existing note, so keep it), and always record a failure line.
                if op == "write" {
                    let _ = std::fs::remove_file(&path);
                }
                audit::record(self.audit_log.as_deref(), "fail", &topic, &note.id);
                return Err(std::io::Error::other(e.to_string()));
            }
        };
        if let Some(node) = node {
            let text = format!("{} {}", note.title, note.body);
            let _ = self.brain.link_note(node, &text, &note.links, &ctx);
        }
        audit::record(self.audit_log.as_deref(), op, &topic, &note.id);
        Ok(Written {
            op,
            topic,
            id: note.id,
            title: note.title,
        })
    }

    /// `lillith learn --reindex`: rebuilds the note index (rows, graph
    /// nodes, edges) from the files on disk. Returns how many notes.
    pub fn reindex(&mut self) -> std::io::Result<usize> {
        if !self.brain.is_enabled() {
            return Ok(0);
        }
        let store = self.store()?.clone();
        let Some(_lock) = store.lock()? else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "another writer holds knowledge/.lock",
            ));
        };
        let other = |e: lillith_memory::Error| std::io::Error::other(e.to_string());
        self.brain.clear_note_index().map_err(other)?;
        let notes = store.read_all();
        let ctx = Ctx::now();
        let mut nodes = Vec::new();
        for (rel, n) in &notes {
            let topic = rel.split('/').next().unwrap_or("inbox");
            if let Some(node) = self
                .brain
                .upsert_note(&row_of(n, rel, topic), &n.body, &ctx)
                .map_err(other)?
            {
                nodes.push((node, n));
            }
        }
        // Link after every node exists, so note → note links resolve.
        for (node, n) in nodes {
            let _ = self
                .brain
                .link_note(node, &format!("{} {}", n.title, n.body), &n.links, &ctx);
        }
        audit::record(
            self.audit_log.as_deref(),
            "reindex",
            "-",
            &notes.len().to_string(),
        );
        Ok(notes.len())
    }
}

fn row_of(n: &Note, rel: &str, topic: &str) -> NoteRow {
    NoteRow {
        id: n.id.clone(),
        path: rel.to_string(),
        topic: topic.to_string(),
        kind: n.kind.clone(),
        title: n.title.clone(),
        source_type: n.source_type.clone(),
        source_ref: n.source_ref.clone(),
        source_hash: n.source_hash.clone(),
        content_hash: n.content_hash(),
        confidence: n.confidence,
        origin: n.origin.clone(),
        created_at: n.created,
        updated_at: n.updated,
    }
}

/// `n-<12 hex>` from the source, title and time.
pub fn new_id(source_ref: &str, title: &str, now: f64) -> String {
    let h = fnv1a(
        format!(
            "{source_ref}|{}|{now}",
            crate::dedupe::normalize_title(title)
        )
        .as_bytes(),
    );
    format!("n-{}", &h[6..18])
}
