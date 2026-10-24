//! Schema and migrations (`PRAGMA user_version`).
//!
//! Each entry in [`MIGRATIONS`] upgrades the schema from version `i` to
//! `i + 1` inside one transaction. Never edit a released migration; append a
//! new one.

use crate::Error;
use rusqlite::Connection;

/// Migration `i` takes the DB from `user_version = i` to `i + 1`.
pub const MIGRATIONS: &[&str] = &[
    // v1: memory graph, FTS index, event log, error memory, meta.
    r#"
    CREATE TABLE nodes (
        id INTEGER PRIMARY KEY,
        kind TEXT NOT NULL,
        key TEXT NOT NULL,
        label TEXT NOT NULL,
        detail TEXT NOT NULL DEFAULT '',
        strength REAL NOT NULL,
        access_count INTEGER NOT NULL DEFAULT 0,
        created_at REAL NOT NULL,
        last_seen REAL NOT NULL,
        UNIQUE(kind, key)
    );
    CREATE INDEX nodes_created ON nodes(created_at);
    CREATE TABLE edges (
        src INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
        dst INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
        relation TEXT NOT NULL,
        strength REAL NOT NULL,
        access_count INTEGER NOT NULL DEFAULT 0,
        created_at REAL NOT NULL,
        last_seen REAL NOT NULL,
        PRIMARY KEY (src, dst, relation)
    ) WITHOUT ROWID;
    CREATE INDEX edges_dst ON edges(dst);
    CREATE VIRTUAL TABLE nodes_fts USING fts5(
        label, detail, content='nodes', content_rowid='id',
        tokenize='unicode61 remove_diacritics 2'
    );
    CREATE TRIGGER nodes_ai AFTER INSERT ON nodes BEGIN
        INSERT INTO nodes_fts(rowid, label, detail) VALUES (new.id, new.label, new.detail);
    END;
    CREATE TRIGGER nodes_ad AFTER DELETE ON nodes BEGIN
        INSERT INTO nodes_fts(nodes_fts, rowid, label, detail)
            VALUES ('delete', old.id, old.label, old.detail);
    END;
    CREATE TRIGGER nodes_au AFTER UPDATE OF label, detail ON nodes BEGIN
        INSERT INTO nodes_fts(nodes_fts, rowid, label, detail)
            VALUES ('delete', old.id, old.label, old.detail);
        INSERT INTO nodes_fts(rowid, label, detail) VALUES (new.id, new.label, new.detail);
    END;
    CREATE TABLE events (
        id INTEGER PRIMARY KEY,
        ts REAL NOT NULL,
        kind TEXT NOT NULL,
        subject TEXT NOT NULL,
        hour INTEGER NOT NULL,
        weekday INTEGER NOT NULL
    );
    CREATE INDEX events_ts ON events(ts);
    CREATE TABLE errors (
        signature TEXT PRIMARY KEY,
        sample TEXT NOT NULL,
        context TEXT NOT NULL DEFAULT '',
        fix TEXT,
        outcome TEXT,
        fix_ok INTEGER NOT NULL DEFAULT 0,
        fix_fail INTEGER NOT NULL DEFAULT 0,
        failed_fixes TEXT NOT NULL DEFAULT '',
        count INTEGER NOT NULL DEFAULT 0,
        first_seen REAL NOT NULL,
        last_seen REAL NOT NULL
    );
    CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    "#,
    // v2: `updated_at` = when a row's remembered *content* last changed (a
    // new label/detail value, a new error sample/fix), so "forget the last
    // N minutes" also removes old rows that were rewritten recently.
    r#"
    ALTER TABLE nodes ADD COLUMN updated_at REAL NOT NULL DEFAULT 0;
    UPDATE nodes SET updated_at = created_at;
    CREATE INDEX nodes_updated ON nodes(updated_at);
    ALTER TABLE errors ADD COLUMN updated_at REAL NOT NULL DEFAULT 0;
    UPDATE errors SET updated_at = last_seen;
    CREATE INDEX errors_updated ON errors(updated_at);
    CREATE INDEX edges_created ON edges(created_at);
    "#,
    // v3: self-learning index (docs/SELF_LEARNING.md §2). Additive only:
    // learned notes (files are the source of truth, `lillith learn
    // --reindex` rebuilds this table), what was read, learn jobs and the
    // (gated, later) online trips.
    r#"
    CREATE TABLE notes (
        id TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, topic TEXT NOT NULL,
        kind TEXT NOT NULL, title TEXT NOT NULL, source_type TEXT NOT NULL,
        source_ref TEXT NOT NULL, source_hash TEXT NOT NULL, content_hash TEXT NOT NULL,
        confidence REAL NOT NULL, origin TEXT NOT NULL,
        node_id INTEGER REFERENCES nodes(id) ON DELETE SET NULL,
        created_at REAL NOT NULL, updated_at REAL NOT NULL);
    CREATE INDEX notes_topic ON notes(topic);
    CREATE TABLE sources (
        ref TEXT PRIMARY KEY, type TEXT NOT NULL, hash TEXT NOT NULL,
        chunks_done INTEGER NOT NULL, read_at REAL NOT NULL);
    CREATE TABLE learn_jobs (
        id INTEGER PRIMARY KEY, kind TEXT NOT NULL, arg TEXT NOT NULL,
        state TEXT NOT NULL, tries INTEGER NOT NULL DEFAULT 0, updated_at REAL NOT NULL);
    CREATE TABLE trips (
        id TEXT PRIMARY KEY, purpose TEXT NOT NULL, proposed_urls TEXT NOT NULL,
        fetched_urls TEXT NOT NULL, saved_notes TEXT NOT NULL,
        state TEXT NOT NULL, card TEXT NOT NULL,
        started_at REAL, finished_at REAL);
    "#,
];

/// The schema version this build writes.
pub const LATEST: u32 = MIGRATIONS.len() as u32;

/// Current `user_version`.
pub fn version(conn: &Connection) -> rusqlite::Result<u32> {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
}

/// Applies every pending migration. A DB from a newer build is refused
/// (never downgraded or clobbered).
pub fn migrate(conn: &mut Connection) -> Result<u32, Error> {
    migrate_with(conn, MIGRATIONS)
}

pub(crate) fn migrate_with(conn: &mut Connection, migrations: &[&str]) -> Result<u32, Error> {
    let mut v = version(conn)?;
    let latest = migrations.len() as u32;
    if v > latest {
        return Err(Error::FutureSchema {
            found: v,
            supported: latest,
        });
    }
    while v < latest {
        // IMMEDIATE: take the write lock up front (two processes opening an
        // old DB at once serialize instead of failing mid-upgrade).
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute_batch(migrations[v as usize])?;
        tx.pragma_update(None, "user_version", v + 1)?;
        tx.commit()?;
        v += 1;
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_db_reaches_latest_and_is_idempotent() {
        let mut c = Connection::open_in_memory().unwrap();
        assert_eq!(version(&c).unwrap(), 0);
        assert_eq!(migrate(&mut c).unwrap(), LATEST);
        assert_eq!(migrate(&mut c).unwrap(), LATEST, "second run is a no-op");
        let n: i64 = c
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name IN ('nodes','edges','nodes_fts','events','errors','meta')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 6);
    }

    #[test]
    fn v1_db_upgrades_to_v2_with_updated_at_backfilled() {
        let mut c = Connection::open_in_memory().unwrap();
        assert_eq!(migrate_with(&mut c, &MIGRATIONS[..1]).unwrap(), 1);
        c.execute_batch(
            "INSERT INTO nodes(kind,key,label,strength,created_at,last_seen)
                 VALUES ('fact','k','Ko likes tea',1.0,100.0,500.0);
             INSERT INTO errors(signature,sample,first_seen,last_seen)
                 VALUES ('sig','boom',10.0,20.0);",
        )
        .unwrap();
        assert_eq!(migrate(&mut c).unwrap(), LATEST);
        let n: f64 = c
            .query_row("SELECT updated_at FROM nodes WHERE key='k'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let e: f64 = c
            .query_row(
                "SELECT updated_at FROM errors WHERE signature='sig'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!((n, e), (100.0, 20.0));
    }

    #[test]
    fn v2_db_upgrades_to_v3_with_learning_tables_and_keeps_data() {
        let mut c = Connection::open_in_memory().unwrap();
        assert_eq!(migrate_with(&mut c, &MIGRATIONS[..2]).unwrap(), 2);
        c.execute_batch(
            "INSERT INTO nodes(kind,key,label,strength,created_at,last_seen,updated_at)
                 VALUES ('fact','k','Ko likes tea',1.0,100.0,500.0,100.0);",
        )
        .unwrap();
        assert_eq!(migrate(&mut c).unwrap(), 3);
        assert_eq!(LATEST, 3);
        let n: i64 = c
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name IN
                 ('notes','sources','learn_jobs','trips','notes_topic')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 5);
        let label: String = c
            .query_row("SELECT label FROM nodes WHERE key='k'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(label, "Ko likes tea");
        // node_id is cleared (not cascaded) when the graph node goes.
        c.execute_batch(
            "PRAGMA foreign_keys = ON;
             INSERT INTO notes(id,path,topic,kind,title,source_type,source_ref,source_hash,
                 content_hash,confidence,origin,node_id,created_at,updated_at)
                 VALUES ('n-1','inbox/a.md','inbox','fact','A','man','ls(1)','h','c',0.5,
                 'learned',(SELECT id FROM nodes WHERE key='k'),1.0,1.0);
             DELETE FROM nodes WHERE key='k';",
        )
        .unwrap();
        let nid: Option<i64> = c
            .query_row("SELECT node_id FROM notes WHERE id='n-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(nid, None);
    }

    #[test]
    fn upgrades_step_by_step_and_keeps_data() {
        let mut c = Connection::open_in_memory().unwrap();
        let v2 = [MIGRATIONS[0], "ALTER TABLE meta ADD COLUMN note TEXT;"];
        assert_eq!(migrate_with(&mut c, &v2[..1]).unwrap(), 1);
        c.execute("INSERT INTO meta(key, value) VALUES ('k', 'v')", [])
            .unwrap();
        assert_eq!(migrate_with(&mut c, &v2).unwrap(), 2);
        let (v, note): (String, Option<String>) = c
            .query_row("SELECT value, note FROM meta WHERE key='k'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((v.as_str(), note), ("v", None));
    }

    #[test]
    fn refuses_a_newer_schema() {
        let mut c = Connection::open_in_memory().unwrap();
        c.pragma_update(None, "user_version", LATEST + 5).unwrap();
        assert!(matches!(
            migrate(&mut c),
            Err(Error::FutureSchema { found, .. }) if found == LATEST + 5
        ));
    }

    #[test]
    fn failed_migration_rolls_back() {
        let mut c = Connection::open_in_memory().unwrap();
        let bad = [MIGRATIONS[0], "CREATE TABLE x(a); THIS IS NOT SQL;"];
        assert!(migrate_with(&mut c, &bad).is_err());
        assert_eq!(version(&c).unwrap(), 1, "v1 committed, v2 rolled back");
        let x: i64 = c
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'x'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(x, 0);
    }
}
