"""SQLite persistence: conversations, their attachment lists, and chat history."""
from __future__ import annotations

import sqlite3
import threading
import functools
import time
from pathlib import Path

SCHEMA = """
CREATE TABLE IF NOT EXISTS conversations (
    id TEXT PRIMARY KEY,
    title TEXT,
    created REAL NOT NULL
);
CREATE TABLE IF NOT EXISTS conversation_attachments (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    attachment_id TEXT NOT NULL,
    added REAL NOT NULL,
    PRIMARY KEY (conversation_id, attachment_id)
);
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    attachment_ids TEXT,
    created REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_ca_att ON conversation_attachments(attachment_id);
CREATE INDEX IF NOT EXISTS idx_msg_conv ON messages(conversation_id, id);
"""


class DB:
    def __init__(self, path: Path):
        path.parent.mkdir(parents=True, exist_ok=True)
        self.path = path
        self.lock = threading.RLock()  # one connection shared by API/worker threads
        self.conn = sqlite3.connect(str(path), timeout=10, check_same_thread=False)
        self.conn.row_factory = sqlite3.Row
        self.conn.execute("PRAGMA journal_mode=WAL")
        self.conn.execute("PRAGMA foreign_keys=ON")
        self.conn.executescript(SCHEMA)

    def close(self):
        self.conn.close()

    def ensure_conversation(self, conv_id: str, title: str | None = None):
        with self.conn:
            self.conn.execute(
                "INSERT OR IGNORE INTO conversations(id, title, created) VALUES (?,?,?)",
                (conv_id, title or conv_id, time.time()))

    def list_conversations(self) -> list[dict]:
        rows = self.conn.execute(
            "SELECT c.id, c.title, c.created, COUNT(ca.attachment_id) AS n "
            "FROM conversations c LEFT JOIN conversation_attachments ca ON ca.conversation_id=c.id "
            "GROUP BY c.id ORDER BY c.created").fetchall()
        return [dict(r) for r in rows]

    def attach(self, conv_id: str, att_id: str):
        self.ensure_conversation(conv_id)
        with self.conn:
            self.conn.execute(
                "INSERT OR IGNORE INTO conversation_attachments VALUES (?,?,?)",
                (conv_id, att_id, time.time()))

    def detach(self, conv_id: str, att_id: str) -> bool:
        with self.conn:
            cur = self.conn.execute(
                "DELETE FROM conversation_attachments WHERE conversation_id=? AND attachment_id=?",
                (conv_id, att_id))
        return cur.rowcount > 0

    def attachments_for(self, conv_id: str) -> list[str]:
        rows = self.conn.execute(
            "SELECT attachment_id FROM conversation_attachments WHERE conversation_id=? ORDER BY added",
            (conv_id,)).fetchall()
        return [r[0] for r in rows]

    def conversations_for(self, att_id: str) -> list[str]:
        rows = self.conn.execute(
            "SELECT conversation_id FROM conversation_attachments WHERE attachment_id=?",
            (att_id,)).fetchall()
        return [r[0] for r in rows]

    def forget_attachment(self, att_id: str):
        with self.conn:
            self.conn.execute("DELETE FROM conversation_attachments WHERE attachment_id=?", (att_id,))

    def add_message(self, conv_id: str, role: str, content: str, attachment_ids=None):
        self.ensure_conversation(conv_id)
        with self.conn:
            self.conn.execute(
                "INSERT INTO messages(conversation_id, role, content, attachment_ids, created) "
                "VALUES (?,?,?,?,?)",
                (conv_id, role, content, ",".join(attachment_ids or []), time.time()))

    def history(self, conv_id: str, limit: int = 20) -> list[dict]:
        rows = self.conn.execute(
            "SELECT role, content FROM (SELECT id, role, content FROM messages "
            "WHERE conversation_id=? ORDER BY id DESC LIMIT ?) ORDER BY id",
            (conv_id, limit)).fetchall()
        return [dict(r) for r in rows]

    def delete_conversation(self, conv_id: str):
        with self.conn:
            self.conn.execute("DELETE FROM conversations WHERE id=?", (conv_id,))


def _locked(fn):
    @functools.wraps(fn)
    def wrapper(self, *a, **kw):
        with self.lock:
            return fn(self, *a, **kw)
    return wrapper


for _n, _f in list(vars(DB).items()):
    if callable(_f) and not _n.startswith("__"):
        setattr(DB, _n, _locked(_f))
