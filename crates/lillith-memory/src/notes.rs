//! The learned-notes index (schema v3, `docs/SELF_LEARNING.md` §2).
//!
//! Note *files* under `knowledge/` are the source of truth; this is the
//! rebuildable index the learner keeps next to the memory graph. Every
//! note also upserts one graph node (`kind = 'note'`, `key = <note id>`,
//! `label = title`, `detail = first 400 chars of the body`), so the
//! existing FTS, decay, retrieval and forget cover notes too.
//!
//! All methods are no-ops (empty results) when memory is off.

use crate::extract::keywords;
use crate::{Brain, Ctx, Error, Kind};
use rusqlite::{params, OptionalExtension};

type Result<T> = std::result::Result<T, Error>;

/// Characters of a note body copied into its graph node's `detail`.
pub const NODE_DETAIL_CHARS: usize = 400;

/// One row of the `notes` table.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteRow {
    pub id: String,
    /// Relative to the knowledge root (`<topic>/<slug>.md`).
    pub path: String,
    pub topic: String,
    pub kind: String,
    pub title: String,
    pub source_type: String,
    pub source_ref: String,
    pub source_hash: String,
    pub content_hash: String,
    pub confidence: f64,
    /// `learned` | `ko`.
    pub origin: String,
    pub created_at: f64,
    pub updated_at: f64,
}

/// What the `sources` table remembers about a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceState {
    pub hash: String,
    pub chunks_done: u32,
}

fn row_of(r: &rusqlite::Row<'_>) -> rusqlite::Result<NoteRow> {
    Ok(NoteRow {
        id: r.get(0)?,
        path: r.get(1)?,
        topic: r.get(2)?,
        kind: r.get(3)?,
        title: r.get(4)?,
        source_type: r.get(5)?,
        source_ref: r.get(6)?,
        source_hash: r.get(7)?,
        content_hash: r.get(8)?,
        confidence: r.get(9)?,
        origin: r.get(10)?,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
    })
}

const COLS: &str = "id, path, topic, kind, title, source_type, source_ref, source_hash, \
                    content_hash, confidence, origin, created_at, updated_at";

impl Brain {
    /// Inserts or replaces the index row for `note` and upserts its graph
    /// node. Returns the node id (`None` when memory is off).
    pub fn upsert_note(&self, note: &NoteRow, body: &str, ctx: &Ctx) -> Result<Option<i64>> {
        if self.conn.is_none() {
            return Ok(None);
        }
        self.write_tx(|| {
            let detail: String = body.chars().take(NODE_DETAIL_CHARS).collect();
            let node = self.upsert_node(Kind::Note, &note.id, &note.title, &detail, ctx)?;
            let Some(c) = &self.conn else { return Ok(None) };
            c.execute(
                "INSERT INTO notes(id, path, topic, kind, title, source_type, source_ref,
                     source_hash, content_hash, confidence, origin, node_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 ON CONFLICT(id) DO UPDATE SET path = excluded.path, topic = excluded.topic,
                     kind = excluded.kind, title = excluded.title,
                     source_type = excluded.source_type, source_ref = excluded.source_ref,
                     source_hash = excluded.source_hash, content_hash = excluded.content_hash,
                     confidence = excluded.confidence, origin = excluded.origin,
                     node_id = excluded.node_id, updated_at = excluded.updated_at",
                params![
                    note.id,
                    note.path,
                    note.topic,
                    note.kind,
                    note.title,
                    note.source_type,
                    note.source_ref,
                    note.source_hash,
                    note.content_hash,
                    note.confidence,
                    note.origin,
                    node,
                    note.created_at,
                    note.updated_at
                ],
            )?;
            Ok(node)
        })
    }

    /// One note row by id.
    pub fn note(&self, id: &str) -> Option<NoteRow> {
        let c = self.conn.as_ref()?;
        c.query_row(
            &format!("SELECT {COLS} FROM notes WHERE id = ?1"),
            [id],
            row_of,
        )
        .optional()
        .ok()?
    }

    /// Every indexed note, by path.
    pub fn all_notes(&self) -> Vec<NoteRow> {
        let Some(c) = &self.conn else {
            return Vec::new();
        };
        let Ok(mut st) = c.prepare(&format!("SELECT {COLS} FROM notes ORDER BY path")) else {
            return Vec::new();
        };
        st.query_map([], row_of)
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    /// Notes whose title or first 400 body chars match words of `text`
    /// (FTS over the note nodes), best first, at most `limit`.
    pub fn similar_notes(&self, text: &str, limit: usize) -> Vec<NoteRow> {
        let Some(c) = &self.conn else {
            return Vec::new();
        };
        let terms = keywords(text, 12);
        if terms.is_empty() || limit == 0 {
            return Vec::new();
        }
        let expr = terms
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect::<Vec<_>>()
            .join(" OR ");
        let sql = format!(
            "SELECT {} FROM nodes_fts JOIN nodes n ON n.id = nodes_fts.rowid
             JOIN notes ON notes.id = n.key
             WHERE nodes_fts MATCH ?1 AND n.kind = 'note'
             ORDER BY bm25(nodes_fts) LIMIT ?2",
            COLS.split(", ")
                .map(|c| format!("notes.{}", c.trim()))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let Ok(mut st) = c.prepare(&sql) else {
            return Vec::new();
        };
        st.query_map(params![expr, limit as i64], row_of)
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    /// Drops every note row and note node (before a `--reindex`). Returns
    /// how many note rows went. Other memories are untouched.
    pub fn clear_note_index(&self) -> Result<usize> {
        let Some(c) = &self.conn else { return Ok(0) };
        self.write_tx(|| {
            let n = c.execute("DELETE FROM notes", [])?;
            c.execute("DELETE FROM nodes WHERE kind = 'note'", [])?;
            Ok(n)
        })
    }

    /// Links the note node `node` to existing entity/note nodes whose key
    /// appears (as a whole word) in `text`, plus the note ids in `links`.
    /// Returns how many edges were written.
    pub fn link_note(&self, node: i64, text: &str, links: &[String], ctx: &Ctx) -> Result<usize> {
        let Some(c) = &self.conn else { return Ok(0) };
        let lower = format!(" {} ", text.to_lowercase());
        let words: std::collections::HashSet<&str> = lower
            .split(|ch: char| !ch.is_alphanumeric() && ch != '-' && ch != '_')
            .filter(|w| w.len() >= 3)
            .collect();
        let mut targets: Vec<i64> = Vec::new();
        {
            let mut st = c.prepare_cached("SELECT id, key FROM nodes WHERE kind = 'entity'")?;
            let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            for (id, key) in rows.flatten() {
                let hit = if key.contains(' ') {
                    lower.contains(&format!(" {key} "))
                } else {
                    words.contains(key.as_str())
                };
                if hit {
                    targets.push(id);
                }
            }
        }
        for l in links {
            let key = l.trim().to_lowercase();
            let key = key.strip_prefix("entity:").unwrap_or(&key).to_string();
            let id: Option<i64> = c
                .query_row(
                    "SELECT id FROM nodes WHERE (kind = 'note' OR kind = 'entity') AND key = ?1",
                    [&key],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(id) = id {
                targets.push(id);
            }
        }
        targets.sort_unstable();
        targets.dedup();
        targets.truncate(16);
        let mut n = 0;
        for t in targets {
            if t != node {
                self.link(node, t, "mentions", ctx)?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// What was read of `source_ref` before.
    pub fn source_state(&self, source_ref: &str) -> Option<SourceState> {
        let c = self.conn.as_ref()?;
        c.query_row(
            "SELECT hash, chunks_done FROM sources WHERE ref = ?1",
            [source_ref],
            |r| {
                Ok(SourceState {
                    hash: r.get(0)?,
                    chunks_done: r.get::<_, i64>(1)?.max(0) as u32,
                })
            },
        )
        .optional()
        .ok()?
    }

    /// Records that `chunks_done` chunks of `source_ref` (content `hash`)
    /// were read.
    pub fn mark_source(
        &self,
        source_ref: &str,
        source_type: &str,
        hash: &str,
        chunks_done: u32,
        ctx: &Ctx,
    ) -> Result<()> {
        let Some(c) = &self.conn else { return Ok(()) };
        c.execute(
            "INSERT INTO sources(ref, type, hash, chunks_done, read_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(ref) DO UPDATE SET type = excluded.type, hash = excluded.hash,
                 chunks_done = excluded.chunks_done, read_at = excluded.read_at",
            params![source_ref, source_type, hash, chunks_done, ctx.now],
        )?;
        Ok(())
    }

    /// Records a learn job outcome (`state`), bumping `tries` when
    /// `failed`. Returns the job's tries so far.
    pub fn record_learn_job(
        &self,
        kind: &str,
        arg: &str,
        state: &str,
        failed: bool,
        ctx: &Ctx,
    ) -> Result<u32> {
        let Some(c) = &self.conn else { return Ok(0) };
        let bump = i64::from(failed);
        let id: Option<i64> = c
            .query_row(
                "SELECT id FROM learn_jobs WHERE kind = ?1 AND arg = ?2",
                params![kind, arg],
                |r| r.get(0),
            )
            .optional()?;
        match id {
            Some(id) => {
                c.execute(
                    "UPDATE learn_jobs SET state = ?2, tries = tries + ?3, updated_at = ?4 WHERE id = ?1",
                    params![id, state, bump, ctx.now],
                )?;
            }
            None => {
                c.execute(
                    "INSERT INTO learn_jobs(kind, arg, state, tries, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![kind, arg, state, bump, ctx.now],
                )?;
            }
        }
        Ok(self.learn_job_tries(kind, arg))
    }

    /// Failed tries of the job `(kind, arg)`.
    pub fn learn_job_tries(&self, kind: &str, arg: &str) -> u32 {
        let Some(c) = &self.conn else { return 0 };
        c.query_row(
            "SELECT tries FROM learn_jobs WHERE kind = ?1 AND arg = ?2",
            params![kind, arg],
            |r| r.get::<_, i64>(0),
        )
        .map(|t| t.max(0) as u32)
        .unwrap_or(0)
    }

    /// Every source read so far, by `ref` (one query for the picker).
    pub fn all_sources(&self) -> std::collections::HashMap<String, SourceState> {
        let mut out = std::collections::HashMap::new();
        let Some(c) = &self.conn else { return out };
        let Ok(mut st) = c.prepare("SELECT ref, hash, chunks_done FROM sources") else {
            return out;
        };
        if let Ok(rows) = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                SourceState {
                    hash: r.get(1)?,
                    chunks_done: r.get::<_, i64>(2)?.max(0) as u32,
                },
            ))
        }) {
            out.extend(rows.flatten());
        }
        out
    }

    /// The `limit` strongest habit/entity/event keys with their strength
    /// (usefulness signal for the learner's picker, one query).
    pub fn useful_words(&self, limit: usize) -> Vec<(String, f64)> {
        let Some(c) = &self.conn else {
            return Vec::new();
        };
        let Ok(mut st) = c.prepare(
            "SELECT key, strength FROM nodes WHERE kind IN ('habit','entity','event')
             ORDER BY strength DESC LIMIT ?1",
        ) else {
            return Vec::new();
        };
        st.query_map([limit as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?))
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    /// Decayed strength of the habit/entity nodes mentioning `word`
    /// (usefulness signal for the learner's picker; 0 when unknown).
    pub fn usefulness_of(&self, word: &str) -> f64 {
        let Some(c) = &self.conn else { return 0.0 };
        let w = word.trim().to_lowercase();
        if w.is_empty() {
            return 0.0;
        }
        c.query_row(
            "SELECT coalesce(max(strength), 0) FROM nodes
             WHERE kind IN ('habit','entity','event') AND (key = ?1 OR instr(key, ?1) > 0)",
            [w],
            |r| r.get::<_, f64>(0),
        )
        .unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryConfig;

    fn row(id: &str, title: &str) -> NoteRow {
        NoteRow {
            id: id.into(),
            path: format!("inbox/{id}.md"),
            topic: "inbox".into(),
            kind: "fact".into(),
            title: title.into(),
            source_type: "man".into(),
            source_ref: "pacman(8)".into(),
            source_hash: "fnv1a:1".into(),
            content_hash: "fnv1a:2".into(),
            confidence: 0.5,
            origin: "learned".into(),
            created_at: 10.0,
            updated_at: 10.0,
        }
    }

    #[test]
    fn upsert_note_indexes_row_and_graph_node() {
        let b = Brain::open_in_memory(&MemoryConfig::default()).unwrap();
        let ctx = Ctx::at(100.0);
        let node = b
            .upsert_note(
                &row("n-1", "pacman remove orphans"),
                "pacman -Rns body",
                &ctx,
            )
            .unwrap()
            .unwrap();
        assert_eq!(b.note("n-1").unwrap().title, "pacman remove orphans");
        assert!(b.strength_of(Kind::Note, "n-1", &ctx).is_some());
        let hits = b.similar_notes("how do I remove orphans with pacman", 5);
        assert_eq!(hits.len(), 1);
        // Re-upsert updates in place.
        let mut r = row("n-1", "pacman: remove orphans");
        r.confidence = 0.9;
        assert_eq!(b.upsert_note(&r, "x", &ctx).unwrap(), Some(node));
        assert_eq!(b.all_notes().len(), 1);
        assert_eq!(b.note("n-1").unwrap().confidence, 0.9);
        assert_eq!(b.clear_note_index().unwrap(), 1);
        assert!(b.all_notes().is_empty());
        assert!(b.strength_of(Kind::Note, "n-1", &ctx).is_none());
    }

    #[test]
    fn link_note_finds_entities_and_listed_notes() {
        let b = Brain::open_in_memory(&MemoryConfig::default()).unwrap();
        let ctx = Ctx::at(100.0);
        b.upsert_node(Kind::Entity, "garuda", "garuda", "", &ctx)
            .unwrap();
        let other = b.upsert_note(&row("n-2", "other"), "", &ctx).unwrap();
        let n = b.upsert_note(&row("n-1", "t"), "", &ctx).unwrap().unwrap();
        let made = b
            .link_note(n, "On Garuda use pacman", &["n-2".into()], &ctx)
            .unwrap();
        assert_eq!(made, 2);
        assert!(other.is_some());
    }

    #[test]
    fn sources_and_jobs_are_tracked() {
        let b = Brain::open_in_memory(&MemoryConfig::default()).unwrap();
        let ctx = Ctx::at(1.0);
        assert!(b.source_state("ls(1)").is_none());
        b.mark_source("ls(1)", "man", "h1", 2, &ctx).unwrap();
        assert_eq!(b.all_sources().len(), 1);
        assert_eq!(
            b.source_state("ls(1)"),
            Some(SourceState {
                hash: "h1".into(),
                chunks_done: 2
            })
        );
        assert_eq!(
            b.record_learn_job("chunk", "ls(1)#2", "failed", true, &ctx)
                .unwrap(),
            1
        );
        assert_eq!(
            b.record_learn_job("chunk", "ls(1)#2", "failed", true, &ctx)
                .unwrap(),
            2
        );
        assert_eq!(
            b.record_learn_job("chunk", "ls(1)#2", "done", false, &ctx)
                .unwrap(),
            2
        );
    }

    #[test]
    fn disabled_brain_does_nothing() {
        let b = Brain::disabled(&MemoryConfig::disabled());
        let ctx = Ctx::at(1.0);
        assert_eq!(b.upsert_note(&row("n-1", "t"), "", &ctx).unwrap(), None);
        assert!(b.all_notes().is_empty());
        assert!(b.similar_notes("t", 3).is_empty());
        assert!(b.source_state("x").is_none());
        assert_eq!(b.record_learn_job("a", "b", "c", true, &ctx).unwrap(), 0);
    }
}
