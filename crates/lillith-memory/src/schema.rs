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
///
/// Safe to call from several processes at once: the version is re-read
/// under the write lock, so a second opener never re-runs a migration
/// another process already applied ("table nodes already exists").
///
/// An *unversioned* DB that already has the tables (`user_version = 0`
/// but `nodes` exists: an earlier build, an interrupted first run, or a
/// copy restored without its pragma) is adopted in place, never wiped:
/// the file is first copied to `<db>.bak-<YYYYmmdd-HHMMSS>`, the schema
/// version is worked out from what is there, missing pieces are created
/// (`IF NOT EXISTS`), and the usual migrations continue from there.
pub fn migrate(conn: &mut Connection) -> Result<u32, Error> {
    migrate_with(conn, MIGRATIONS)
}

pub(crate) fn migrate_with(conn: &mut Connection, migrations: &[&str]) -> Result<u32, Error> {
    let latest = migrations.len() as u32;
    let v = version(conn)?;
    if v > latest {
        return Err(Error::FutureSchema {
            found: v,
            supported: latest,
        });
    }
    if v == latest {
        return Ok(v);
    }
    if v == 0 && has_table(conn, "nodes")? {
        adopt_unversioned(conn, migrations)?;
    }
    loop {
        // IMMEDIATE: take the write lock up front, then re-read the version
        // under it (another process may have migrated meanwhile).
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let v = version(&tx)?;
        if v > latest {
            return Err(Error::FutureSchema {
                found: v,
                supported: latest,
            });
        }
        if v == latest {
            tx.commit()?;
            return Ok(v);
        }
        tx.execute_batch(migrations[v as usize])?;
        tx.pragma_update(None, "user_version", v + 1)?;
        tx.commit()?;
    }
}

fn has_table(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
}

fn columns(conn: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    let mut st = conn.prepare(&format!("PRAGMA table_info(\"{table}\")"))?;
    let cols = st
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(cols)
}

/// `CREATE TABLE x` -> `CREATE TABLE IF NOT EXISTS x` (and the same for
/// indexes, virtual tables and triggers). A fresh DB gets exactly the same
/// schema from either form.
fn idempotent(sql: &str) -> String {
    sql.replace("CREATE TABLE ", "CREATE TABLE IF NOT EXISTS ")
        .replace("CREATE INDEX ", "CREATE INDEX IF NOT EXISTS ")
        .replace(
            "CREATE VIRTUAL TABLE ",
            "CREATE VIRTUAL TABLE IF NOT EXISTS ",
        )
        .replace("CREATE TRIGGER ", "CREATE TRIGGER IF NOT EXISTS ")
}

/// The schema version an unversioned DB already has, from its tables.
fn detect_version(conn: &Connection) -> rusqlite::Result<u32> {
    Ok(if has_table(conn, "notes")? {
        3
    } else if columns(conn, "nodes")?.iter().any(|c| c == "updated_at") {
        2
    } else {
        1
    })
}

/// Copies the DB file to `<db>.bak-<local date-time>` (a consistent copy
/// via `VACUUM INTO`, WAL included). In-memory DBs have nothing to copy.
fn backup(conn: &Connection) -> Result<Option<std::path::PathBuf>, Error> {
    let Some(path) = conn.path().filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    let stamp: String = conn.query_row(
        "SELECT strftime('%Y%m%d-%H%M%S', 'now', 'localtime')",
        [],
        |r| r.get(0),
    )?;
    let mut bak = std::path::PathBuf::from(format!("{path}.bak-{stamp}"));
    let mut n = 1;
    while bak.exists() {
        bak = std::path::PathBuf::from(format!("{path}.bak-{stamp}-{n}"));
        n += 1;
    }
    conn.execute("VACUUM INTO ?1", [bak.to_string_lossy()])?;
    Ok(Some(bak))
}

/// Adopts an unversioned DB that already has tables (see [`migrate`]).
fn adopt_unversioned(conn: &mut Connection, migrations: &[&str]) -> Result<(), Error> {
    const V1_NODES: &[&str] = &[
        "id",
        "kind",
        "key",
        "label",
        "detail",
        "strength",
        "access_count",
        "created_at",
        "last_seen",
    ];
    let have = columns(conn, "nodes")?;
    let missing: Vec<&str> = V1_NODES
        .iter()
        .copied()
        .filter(|c| !have.iter().any(|h| h == c))
        .collect();
    let bak = backup(conn)?;
    if let Some(b) = &bak {
        tracing::warn!(
            backup = %b.display(),
            "memory db: unversioned (user_version 0) but its tables exist; backed up, adopting in place"
        );
    }
    if !missing.is_empty() {
        return Err(Error::LegacySchema(format!(
            "nodes lacks {}",
            missing.join(", ")
        )));
    }
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if version(&tx)? != 0 {
        // Another process adopted it meanwhile.
        return Ok(tx.commit()?);
    }
    let found = detect_version(&tx)?.min(migrations.len() as u32);
    let had_fts = has_table(&tx, "nodes_fts")?;
    for (i, m) in migrations.iter().enumerate().take(found as usize) {
        if i == 0 {
            // v1 has triggers (`;` inside BEGIN..END): run it as one batch.
            tx.execute_batch(&idempotent(m))?;
            continue;
        }
        // Later migrations: plain statements. ADD COLUMN only when the
        // column is missing; a backfill UPDATE only runs right after an
        // ADD COLUMN it belongs to (never rewrites kept data).
        let mut added = false;
        for stmt in m.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            let up = stmt.to_ascii_uppercase();
            if up.starts_with("ALTER TABLE") {
                let w: Vec<&str> = stmt.split_whitespace().collect();
                // ALTER TABLE <t> ADD COLUMN <c> ...
                if w.len() >= 6 && w[3].eq_ignore_ascii_case("ADD") {
                    let (t, c) = (w[2], w[5]);
                    if columns(&tx, t)?.iter().any(|h| h == c) {
                        added = false;
                        continue;
                    }
                }
                tx.execute_batch(stmt)?;
                added = true;
            } else if up.starts_with("UPDATE") {
                if added {
                    tx.execute_batch(stmt)?;
                }
            } else {
                tx.execute_batch(&idempotent(stmt))?;
            }
        }
    }
    if !had_fts && found >= 1 {
        // The search index was missing: rebuild it from the kept rows.
        tx.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES ('rebuild');")?;
    }
    tx.pragma_update(None, "user_version", found)?;
    tx.commit()?;
    tracing::info!(version = found, "memory db: adopted (rows kept)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "lillith-schema-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d.join("memory.db")
    }

    fn baks(db: &std::path::Path) -> Vec<std::path::PathBuf> {
        let name = db.file_name().unwrap().to_string_lossy().to_string();
        std::fs::read_dir(db.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&format!("{name}.bak-"))
            })
            .collect()
    }

    /// Ko's case: an older DB file with the tables but `user_version = 0`
    /// ("table nodes already exists; learning off"). It is backed up, then
    /// adopted in place with every row kept.
    #[test]
    fn unversioned_existing_db_is_backed_up_and_adopted_keeping_rows() {
        for keep in 1..=3usize {
            let db = temp_db(&format!("adopt{keep}"));
            {
                let mut c = Connection::open(&db).unwrap();
                migrate_with(&mut c, &MIGRATIONS[..keep]).unwrap();
                c.execute_batch(
                    "INSERT INTO nodes(kind,key,label,strength,created_at,last_seen)
                         VALUES ('fact','k','Ko likes tea',1.0,100.0,500.0);
                     INSERT INTO meta(key,value) VALUES ('last_maintain','42');",
                )
                .unwrap();
                if keep >= 2 {
                    c.execute_batch("UPDATE nodes SET updated_at = 321.0")
                        .unwrap();
                }
                // The broken state: tables there, version pragma lost.
                c.pragma_update(None, "user_version", 0).unwrap();
            }
            let mut c = Connection::open(&db).unwrap();
            assert_eq!(migrate(&mut c).unwrap(), LATEST, "keep={keep}");
            let label: String = c
                .query_row("SELECT label FROM nodes WHERE key='k'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(label, "Ko likes tea");
            let upd: f64 = c
                .query_row("SELECT updated_at FROM nodes WHERE key='k'", [], |r| {
                    r.get(0)
                })
                .unwrap();
            // v1 rows get the backfill; existing v2 values are never rewritten.
            assert_eq!(upd, if keep >= 2 { 321.0 } else { 100.0 }, "keep={keep}");
            let hits: i64 = c
                .query_row(
                    "SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH 'tea'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(hits, 1, "search index still finds the kept row");
            let m: String = c
                .query_row(
                    "SELECT value FROM meta WHERE key='last_maintain'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(m, "42");
            let b = baks(&db);
            assert_eq!(b.len(), 1, "one backup before touching it: {b:?}");
            let bc = Connection::open(&b[0]).unwrap();
            let n: i64 = bc
                .query_row("SELECT count(*) FROM nodes", [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 1, "the backup holds the old rows");
            assert_eq!(migrate(&mut c).unwrap(), LATEST, "second open is a no-op");
            assert_eq!(baks(&db).len(), 1, "no second backup");
            let _ = std::fs::remove_dir_all(db.parent().unwrap());
        }
    }

    #[test]
    fn unknown_legacy_layout_is_left_untouched() {
        let db = temp_db("legacy");
        {
            let c = Connection::open(&db).unwrap();
            c.execute_batch(
                "CREATE TABLE nodes (id INTEGER PRIMARY KEY, name TEXT);
                 INSERT INTO nodes(name) VALUES ('keep me');",
            )
            .unwrap();
        }
        let mut c = Connection::open(&db).unwrap();
        assert!(matches!(migrate(&mut c), Err(Error::LegacySchema(_))));
        assert_eq!(version(&c).unwrap(), 0);
        let name: String = c
            .query_row("SELECT name FROM nodes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "keep me");
        assert_eq!(baks(&db).len(), 1);
        let _ = std::fs::remove_dir_all(db.parent().unwrap());
    }

    /// Two openers of a fresh file: the second must not re-run v1.
    #[test]
    fn concurrent_first_open_does_not_rerun_v1() {
        let db = temp_db("race");
        let mut a = Connection::open(&db).unwrap();
        let mut b = Connection::open(&db).unwrap();
        // b read version 0 before a migrated (the old race window).
        assert_eq!(version(&b).unwrap(), 0);
        assert_eq!(migrate(&mut a).unwrap(), LATEST);
        assert_eq!(migrate(&mut b).unwrap(), LATEST);
        let _ = std::fs::remove_dir_all(db.parent().unwrap());
    }

    #[test]
    fn idempotent_rewrite_gives_the_same_fresh_schema() {
        let mut a = Connection::open_in_memory().unwrap();
        migrate(&mut a).unwrap();
        let b = Connection::open_in_memory().unwrap();
        for m in MIGRATIONS {
            b.execute_batch(&idempotent(m)).unwrap();
        }
        let dump = |c: &Connection| -> Vec<(String, String)> {
            let mut st = c
                .prepare("SELECT type, name FROM sqlite_master ORDER BY type, name")
                .unwrap();
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(dump(&a), dump(&b));
    }

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
