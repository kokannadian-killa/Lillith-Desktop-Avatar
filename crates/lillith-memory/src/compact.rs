//! Memory compaction (self-compression, docs/SELF_LEARNING.md §13, P8,
//! first slice): old habit events become one digest per day, then the
//! database is tidied.
//!
//! - **Digests.** Events older than [`DIGEST_AFTER_S`] (past the 60-day
//!   habit window, so predictions don't change) are rolled up per UTC day
//!   into one `event` node (`key = digest:day:<YYYY-MM-DD>`, the counts per
//!   app / subject in `detail`, linked `digest_of` to the app entities the
//!   originals were about). The originals are deleted in the same
//!   transaction, after the digest is written. Without compaction those
//!   events were simply deleted at 120 days; now their gist stays
//!   searchable.
//! - **Never touched:** notes and their files (`origin: ko` or not),
//!   preferences, entities, errors, anything newer than the cut-off. No
//!   table is dropped and no schema changes: digests are ordinary rows.
//! - **Forget wins.** A digest is a node like any other: forgetting a topic
//!   that a digest mentions deletes the digest (§13 "deleted if none").
//! - **Stops.** `stop` is checked before every day (one transaction each);
//!   a stop or the safeword mid-run leaves every finished day done and the
//!   rest for next time (resumable: nothing half-written).
//! - **Tidy.** FTS `optimize`, `PRAGMA optimize`, `incremental_vacuum` and
//!   a WAL checkpoint (TRUNCATE). No full VACUUM (that stays with forget).
//! - **Audit:** one line per run,
//!   `<ts>\tCOMPACT\trows_merged=N\tbytes_saved=B` (counts only).

use crate::{Brain, Error};
use rusqlite::{params, Connection};
use std::collections::BTreeMap;
use std::path::Path;

type Result<T> = std::result::Result<T, Error>;

/// Events older than this are digested (61 days: just past the habit
/// window, which reads the last 60).
pub const DIGEST_AFTER_S: f64 = 61.0 * 86_400.0;
/// Days digested per run at most (the rest wait for the next run).
pub const MAX_DAYS_PER_RUN: usize = 90;
/// Subjects listed per digest at most (the rest are counted as "others").
pub const MAX_SUBJECTS: usize = 24;
/// A digest's starting strength (it decays like every memory).
pub const DIGEST_STRENGTH: f64 = 0.5;
/// The digest → entity relation.
pub const DIGEST_REL: &str = "digest_of";

/// What one run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompactReport {
    /// Days digested.
    pub days: usize,
    /// Original event rows merged into digests (and deleted).
    pub rows_merged: usize,
    /// Database bytes freed (page count × page size, before − after; ≥ 0).
    pub bytes_saved: u64,
    /// `true` when `stop` cut the run short.
    pub stopped: bool,
}

fn ymd(day: i64) -> String {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn utc(t: f64) -> String {
    let secs = t.max(0.0) as i64;
    let rem = secs.rem_euclid(86_400);
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        ymd(secs.div_euclid(86_400)),
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// The digest's text for `(kind, subject) → count`, biggest first.
pub fn digest_detail(day: &str, counts: &BTreeMap<(String, String), usize>) -> String {
    let mut v: Vec<(&(String, String), &usize)> = counts.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let total: usize = counts.values().sum();
    let mut parts: Vec<String> = v
        .iter()
        .take(MAX_SUBJECTS)
        .map(|((k, s), n)| format!("{s} ({k}) ×{n}"))
        .collect();
    let rest: usize = v.iter().skip(MAX_SUBJECTS).map(|(_, n)| **n).sum();
    if rest > 0 {
        parts.push(format!("others ×{rest}"));
    }
    format!("{day}: {total} events. {}", parts.join(", "))
}

fn db_bytes(c: &Connection) -> u64 {
    let q = |p: &str| c.query_row(p, [], |r| r.get::<_, i64>(0)).unwrap_or(0);
    (q("PRAGMA page_count") - q("PRAGMA freelist_count")).max(0) as u64
        * q("PRAGMA page_size").max(0) as u64
}

impl Brain {
    /// One compaction run at `now` (see the module docs). Memory off: an
    /// empty report, nothing opened.
    pub fn compact(&self, now: f64, stop: &dyn Fn() -> bool) -> Result<CompactReport> {
        let mut rep = CompactReport::default();
        let Some(c) = &self.conn else {
            return Ok(rep);
        };
        let before = db_bytes(c);
        let cutoff = now - DIGEST_AFTER_S;
        let mut st = c.prepare(
            "SELECT DISTINCT CAST(ts / 86400 AS INTEGER) AS d FROM events
             WHERE ts < ?1 AND ts >= 0 ORDER BY d LIMIT ?2",
        )?;
        let days: Vec<i64> = st
            .query_map(params![cutoff, MAX_DAYS_PER_RUN as i64], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        drop(st);
        for day in days {
            if stop() {
                rep.stopped = true;
                break;
            }
            let lo = day as f64 * 86_400.0;
            // Only whole days before the cut-off (the cut-off's own day
            // waits until it is entirely past it).
            let hi = (lo + 86_400.0).min(cutoff);
            let n = digest_day(c, day, lo, hi, now)?;
            if n > 0 {
                rep.days += 1;
                rep.rows_merged += n;
            }
        }
        if !rep.stopped {
            tidy(c);
        }
        rep.bytes_saved = before.saturating_sub(db_bytes(c));
        tracing::info!(
            target: "lillith_memory::audit",
            days = rep.days,
            rows_merged = rep.rows_merged,
            bytes_saved = rep.bytes_saved,
            stopped = rep.stopped,
            "compact"
        );
        Ok(rep)
    }
}

/// Digests the events of `day` in `[lo, hi)` and deletes them, in one
/// transaction. Returns the rows merged.
fn digest_day(c: &Connection, day: i64, lo: f64, hi: f64, now: f64) -> Result<usize> {
    c.execute_batch("BEGIN IMMEDIATE")?;
    let r = (|| -> Result<usize> {
        let mut st =
            c.prepare_cached("SELECT kind, subject FROM events WHERE ts >= ?1 AND ts < ?2")?;
        let rows: Vec<(String, String)> = st
            .query_map(params![lo, hi], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<(String, String)>>>()?;
        drop(st);
        if rows.is_empty() {
            return Ok(0);
        }
        let mut counts: BTreeMap<(String, String), usize> = BTreeMap::new();
        for k in &rows {
            *counts.entry(k.clone()).or_default() += 1;
        }
        let date = ymd(day);
        let detail = digest_detail(&date, &counts);
        let key = format!("digest:day:{date}");
        c.execute(
            "INSERT INTO nodes(kind, key, label, detail, strength, access_count,
                               created_at, last_seen, updated_at)
             VALUES ('event', ?1, ?2, ?3, ?4, 0, ?5, ?6, ?6)
             ON CONFLICT(kind, key) DO UPDATE SET
                 detail = nodes.detail || ' | ' || excluded.detail,
                 strength = max(nodes.strength, excluded.strength),
                 last_seen = excluded.last_seen,
                 updated_at = excluded.updated_at",
            params![
                key,
                format!("Digest of {date}"),
                detail,
                DIGEST_STRENGTH,
                lo,
                now
            ],
        )?;
        let id: i64 = c.query_row(
            "SELECT id FROM nodes WHERE kind = 'event' AND key = ?1",
            [&key],
            |r| r.get(0),
        )?;
        for (kind, subject) in counts.keys() {
            if kind != "app" {
                continue;
            }
            c.execute(
                "INSERT OR IGNORE INTO edges(src, dst, relation, strength, access_count,
                                             created_at, last_seen)
                 SELECT ?1, id, ?2, ?3, 0, ?4, ?4 FROM nodes
                 WHERE kind = 'entity' AND key = ?5",
                params![
                    id,
                    DIGEST_REL,
                    DIGEST_STRENGTH,
                    now,
                    format!("app:{subject}")
                ],
            )?;
        }
        let n = c.execute(
            "DELETE FROM events WHERE ts >= ?1 AND ts < ?2",
            params![lo, hi],
        )?;
        Ok(n)
    })();
    match r {
        Ok(n) => {
            c.execute_batch("COMMIT")?;
            Ok(n)
        }
        Err(e) => {
            let _ = c.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// Best-effort upkeep (each step logged, none fatal).
fn tidy(c: &Connection) {
    let steps = [
        "INSERT INTO nodes_fts(nodes_fts) VALUES('optimize')",
        "PRAGMA incremental_vacuum",
        "PRAGMA optimize",
    ];
    for s in steps {
        if let Err(e) = c.execute_batch(s) {
            tracing::debug!("compact: {s}: {e}");
        }
    }
    if let Err(e) = c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())) {
        tracing::debug!("compact: checkpoint: {e}");
    }
}

/// The audit line for a run (with the trailing newline).
pub fn audit_line(t: f64, r: &CompactReport) -> String {
    format!(
        "{}\tCOMPACT\trows_merged={}\tbytes_saved={}\n",
        utc(t),
        r.rows_merged,
        r.bytes_saved
    )
}

/// Appends [`audit_line`] to `log` (created 0600).
pub fn record(log: &Path, t: f64, r: &CompactReport) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = log.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut o = std::fs::OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(log)?.write_all(audit_line(t, r).as_bytes())
}

#[cfg(test)]
#[path = "compact_tests.rs"]
mod tests;
