//! Persistent memory, ported from `lillith_memory_sqlite.py`. TODO(M7):
//! fact extraction, mood history, warmth drift, context summary.

use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

/// Same schema as the Python module, so existing DBs keep working.
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS profile (key TEXT PRIMARY KEY, value TEXT, updated_at REAL);
CREATE TABLE IF NOT EXISTS mood_history (id INTEGER PRIMARY KEY AUTOINCREMENT, mood TEXT, strength REAL, reason TEXT, ts REAL);
CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT, content TEXT, meta TEXT, ts REAL);
CREATE TABLE IF NOT EXISTS conversation (id INTEGER PRIMARY KEY AUTOINCREMENT, role TEXT, content TEXT, ts REAL);
CREATE TABLE IF NOT EXISTS preferences (key TEXT PRIMARY KEY, value TEXT, updated_at REAL);
CREATE TABLE IF NOT EXISTS facts (id INTEGER PRIMARY KEY AUTOINCREMENT, fact TEXT UNIQUE, source TEXT, ts REAL);
"#;

/// Handle to the memory database.
pub struct Memory {
    conn: Connection,
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or_default()
}

impl Memory {
    /// Opens (or creates) the DB at `path`.
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        Self::init(Connection::open(path)?)
    }

    /// In-memory DB for tests.
    pub fn open_in_memory() -> rusqlite::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> rusqlite::Result<Self> {
        conn.execute_batch(SCHEMA)?;
        let m = Self { conn };
        if m.get_profile("user_name")?.is_none() {
            m.set_profile("user_name", "Master")?;
        }
        if m.get_profile("assistant_name")?.is_none() {
            m.set_profile("assistant_name", "Lillith")?;
        }
        Ok(m)
    }

    pub fn get_profile(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row("SELECT value FROM profile WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()
    }

    pub fn set_profile(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO profile(key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, value, now()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_update() {
        let m = Memory::open_in_memory().unwrap();
        assert_eq!(
            m.get_profile("user_name").unwrap().as_deref(),
            Some("Master")
        );
        m.set_profile("user_name", "Ko").unwrap();
        assert_eq!(m.get_profile("user_name").unwrap().as_deref(), Some("Ko"));
    }
}
