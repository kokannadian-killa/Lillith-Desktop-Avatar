"""Bot registry, per-bot history, durable memory (facts + dated journal),
routines and tasks.

Layout under <data>/lillith/bots/ (<data> = $XDG_DATA_HOME or ~/.local/share):

    registry.sqlite          bot list, routines, task queue/state
    <bot-id>/profile.json    profile mirror (hand-editable backup)
    <bot-id>/bot.sqlite      this bot's history, facts and journal
    <bot-id>/jobs/<task>.jsonl   append-only job log (one JSON event per line)
    <bot-id>/results/<task>.md   task result text

Bot memory is separate from Lillith's own memory.db (owned by the Rust
lillith-memory crate), which this module never opens.
"""
from __future__ import annotations

import json
import os
import re
import shutil
import sqlite3
import threading
import time
from dataclasses import dataclass, field, asdict
from datetime import datetime, date
from pathlib import Path

from ..lillith_config import assert_loopback, data_dir
from .cron import Cron

NAME_RE = re.compile(r"^[a-z0-9][a-z0-9_-]{0,47}$")
DEFAULT_TOOLS = ["list_attachments", "read_attachment", "search_attachments",
                 "search_memory", "write_memory"]


def lillith_home() -> Path:
    return data_dir()


class BotError(Exception):
    pass


class BotNotFound(BotError):
    pass


def slugify(name: str) -> str:
    s = re.sub(r"[^a-z0-9_-]+", "-", name.strip().lower()).strip("-")
    return s[:48]


@dataclass
class Sampling:
    temperature: float = 0.7
    top_p: float = 0.9
    top_k: int = 40
    max_tokens: int = 512
    repeat_penalty: float = 1.1
    stop: list = field(default_factory=list)


@dataclass
class BotProfile:
    name: str                       # slug / id
    title: str = ""
    description: str = ""
    role: str = ""                  # short role shown in the Bots tab ("researcher")
    persona: str = ""               # empty = persona from config.toml [llm]/[chat]
    avatar: str = ""                # path to avatar image
    endpoint: str = ""              # empty = config.toml [llm]/[chat] endpoint (loopback only)
    model: str = ""                 # empty = config.toml model
    sampling: Sampling = field(default_factory=Sampling)
    tools: list = field(default_factory=lambda: list(DEFAULT_TOOLS))
    context_tokens: int = 4096      # model context window
    memory_budget: int = 512        # tokens for memory block
    attachment_budget: int = 1024   # tokens for attachments
    history_budget: int = 1536      # tokens for past turns
    created: float = 0.0
    updated: float = 0.0

    def validate(self):
        if not NAME_RE.match(self.name):
            raise BotError(f"invalid bot name {self.name!r} (a-z, 0-9, _ or -, max 48)")
        if self.endpoint:
            assert_loopback(self.endpoint)
        from .tools import TOOLS
        bad = [t for t in self.tools if t not in TOOLS]
        if bad:
            raise BotError(f"unknown tools: {bad}; allowed: {sorted(TOOLS)}")
        if self.avatar:
            self.avatar = str(Path(self.avatar).expanduser())
        return self

    def to_dict(self) -> dict:
        return asdict(self)

    @classmethod
    def from_dict(cls, d: dict) -> "BotProfile":
        d = dict(d)
        samp = d.pop("sampling", None) or {}
        known = set(cls.__dataclass_fields__) - {"sampling"}
        p = cls(**{k: v for k, v in d.items() if k in known})
        sk = Sampling.__dataclass_fields__
        p.sampling = Sampling(**{k: v for k, v in samp.items() if k in sk})
        return p


REGISTRY_SCHEMA = """
CREATE TABLE IF NOT EXISTS bots (
    name TEXT PRIMARY KEY, profile TEXT NOT NULL, created REAL, updated REAL);
CREATE TABLE IF NOT EXISTS routines (
    id INTEGER PRIMARY KEY AUTOINCREMENT, bot TEXT NOT NULL REFERENCES bots(name) ON DELETE CASCADE,
    name TEXT NOT NULL, cron TEXT NOT NULL, prompt TEXT NOT NULL, enabled INTEGER DEFAULT 1,
    notify INTEGER DEFAULT 1, last_run REAL, next_run REAL, last_status TEXT, created REAL NOT NULL);
CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY, bot TEXT, requested_bot TEXT NOT NULL, task TEXT NOT NULL,
    attachments TEXT, source TEXT DEFAULT 'api', status TEXT NOT NULL,
    cancel_requested INTEGER DEFAULT 0, tokens INTEGER DEFAULT 0, error TEXT,
    result_path TEXT, retry_of TEXT, created REAL NOT NULL, started REAL, finished REAL,
    last_progress REAL);
CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status, created);
"""

BOT_SCHEMA = """
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT, role TEXT NOT NULL, content TEXT NOT NULL,
    source TEXT DEFAULT 'chat', created REAL NOT NULL);
CREATE TABLE IF NOT EXISTS facts (
    id INTEGER PRIMARY KEY AUTOINCREMENT, fact TEXT NOT NULL UNIQUE, created REAL NOT NULL);
CREATE TABLE IF NOT EXISTS journal (
    id INTEGER PRIMARY KEY AUTOINCREMENT, day TEXT NOT NULL, entry TEXT NOT NULL, created REAL NOT NULL);
CREATE INDEX IF NOT EXISTS idx_journal_day ON journal(day);
"""


def _connect(path: Path, schema: str) -> sqlite3.Connection:
    path.parent.mkdir(parents=True, exist_ok=True)
    c = sqlite3.connect(str(path), timeout=15, check_same_thread=False)
    c.row_factory = sqlite3.Row
    c.execute("PRAGMA journal_mode=WAL")
    c.execute("PRAGMA foreign_keys=ON")
    c.executescript(schema)
    return c


class BotStore:
    """Thread-safe (one RLock) access to the registry and per-bot databases."""

    def __init__(self, home: Path | None = None):
        self.home = Path(home) if home else lillith_home()
        self.dir = self.home / "bots"
        self.dir.mkdir(parents=True, exist_ok=True)
        self.lock = threading.RLock()
        self.conn = _connect(self.dir / "registry.sqlite", REGISTRY_SCHEMA)
        self._bot_conns: dict = {}

    def close(self):
        with self.lock:
            for c in self._bot_conns.values():
                c.close()
            self._bot_conns.clear()
            self.conn.close()

    # ---------- per-bot paths ----------
    def bot_dir(self, bot: str) -> Path:
        if not NAME_RE.match(bot):
            raise BotError(f"invalid bot id {bot!r}")
        return self.dir / bot

    def _bdb(self, bot: str) -> sqlite3.Connection:
        with self.lock:
            c = self._bot_conns.get(bot)
            if c is None:
                if not self.exists(bot):
                    raise BotNotFound(f"no bot named {bot!r}")
                c = _connect(self.bot_dir(bot) / "bot.sqlite", BOT_SCHEMA)
                self._bot_conns[bot] = c
            return c

    # ---------- bots ----------
    def _mirror(self, p: BotProfile):
        d = self.bot_dir(p.name)
        d.mkdir(parents=True, exist_ok=True)
        (d / "profile.json").write_text(json.dumps(p.to_dict(), indent=2), encoding="utf-8")

    def create(self, name: str, **fields) -> BotProfile:
        slug = slugify(name)
        with self.lock:
            if self.exists(slug):
                raise BotError(f"bot {slug!r} already exists")
            if not fields.get("title"):
                fields["title"] = name.strip()
            now = time.time()
            p = BotProfile.from_dict({**fields, "name": slug, "created": now, "updated": now}).validate()
            with self.conn:
                self.conn.execute("INSERT INTO bots VALUES (?,?,?,?)",
                                  (slug, json.dumps(p.to_dict()), now, now))
            self._mirror(p)
            return p

    def exists(self, name: str) -> bool:
        with self.lock:
            return self.conn.execute("SELECT 1 FROM bots WHERE name=?", (name,)).fetchone() is not None

    def get(self, name: str) -> BotProfile:
        with self.lock:
            row = self.conn.execute("SELECT profile FROM bots WHERE name=?", (slugify(name),)).fetchone()
        if not row:
            raise BotNotFound(f"no bot named {name!r}")
        return BotProfile.from_dict(json.loads(row[0]))

    def list(self) -> list:
        with self.lock:
            rows = self.conn.execute("SELECT profile FROM bots ORDER BY created").fetchall()
        return [BotProfile.from_dict(json.loads(r[0])) for r in rows]

    def edit(self, name: str, **changes) -> BotProfile:
        with self.lock:
            p = self.get(name)
            d = p.to_dict()
            samp = changes.pop("sampling", None)
            for k, v in changes.items():
                if k in ("name", "created", "updated"):
                    continue
                if k not in d:
                    raise BotError(f"unknown field {k!r}")
                d[k] = v
            if samp:
                d["sampling"].update(samp)
            d["updated"] = time.time()
            p = BotProfile.from_dict(d).validate()
            with self.conn:
                self.conn.execute("UPDATE bots SET profile=?, updated=? WHERE name=?",
                                  (json.dumps(p.to_dict()), p.updated, p.name))
            self._mirror(p)
            return p

    def delete(self, name: str):
        with self.lock:
            p = self.get(name)
            c = self._bot_conns.pop(p.name, None)
            if c:
                c.close()
            with self.conn:
                self.conn.execute("DELETE FROM bots WHERE name=?", (p.name,))
            d = self.bot_dir(p.name)
            if d.is_dir() and d.parent == self.dir:
                shutil.rmtree(d)

    # ---------- history ----------
    def add_message(self, bot: str, role: str, content: str, source: str = "chat"):
        with self.lock:
            c = self._bdb(bot)
            with c:
                c.execute("INSERT INTO messages(role, content, source, created) VALUES (?,?,?,?)",
                          (role, content, source, time.time()))

    def history(self, bot: str, limit: int = 50) -> list:
        with self.lock:
            rows = self._bdb(bot).execute(
                "SELECT * FROM (SELECT id, role, content, source, created FROM messages "
                "ORDER BY id DESC LIMIT ?) ORDER BY id", (limit,)).fetchall()
        return [dict(r) for r in rows]

    def last_message_time(self, bot: str) -> float | None:
        with self.lock:
            r = self._bdb(bot).execute("SELECT MAX(created) FROM messages").fetchone()
        return r[0] if r else None

    def clear_history(self, bot: str):
        with self.lock:
            c = self._bdb(bot)
            with c:
                c.execute("DELETE FROM messages")

    # ---------- memory (this bot only) ----------
    def add_fact(self, bot: str, fact: str) -> int:
        fact = " ".join(fact.split())[:1000]
        if not fact:
            raise BotError("empty fact")
        with self.lock:
            c = self._bdb(bot)
            with c:
                c.execute("INSERT OR IGNORE INTO facts(fact, created) VALUES (?,?)", (fact, time.time()))
            return c.execute("SELECT id FROM facts WHERE fact=?", (fact,)).fetchone()[0]

    def facts(self, bot: str) -> list:
        with self.lock:
            return [dict(r) for r in self._bdb(bot).execute(
                "SELECT id, fact, created FROM facts ORDER BY id")]

    def rm_fact(self, bot: str, fact_id: int) -> bool:
        with self.lock:
            c = self._bdb(bot)
            with c:
                return c.execute("DELETE FROM facts WHERE id=?", (fact_id,)).rowcount > 0

    def add_journal(self, bot: str, entry: str, day: str | None = None) -> int:
        day = day or date.today().isoformat()
        datetime.strptime(day, "%Y-%m-%d")
        with self.lock:
            c = self._bdb(bot)
            with c:
                return c.execute("INSERT INTO journal(day, entry, created) VALUES (?,?,?)",
                                 (day, entry.strip()[:4000], time.time())).lastrowid

    def journal(self, bot: str, limit: int = 100) -> list:
        with self.lock:
            return [dict(r) for r in self._bdb(bot).execute(
                "SELECT id, day, entry, created FROM journal ORDER BY day DESC, id DESC LIMIT ?",
                (limit,))]

    def rm_journal(self, bot: str, entry_id: int) -> bool:
        with self.lock:
            c = self._bdb(bot)
            with c:
                return c.execute("DELETE FROM journal WHERE id=?", (entry_id,)).rowcount > 0

    def search_memory(self, bot: str, query: str, limit: int = 20) -> list:
        q = f"%{query.replace('%', '').replace('_', '')}%"
        with self.lock:
            c = self._bdb(bot)
            out = [{"type": "fact", "id": r["id"], "text": r["fact"]} for r in c.execute(
                "SELECT id, fact FROM facts WHERE fact LIKE ? ORDER BY id LIMIT ?", (q, limit))]
            out += [{"type": "journal", "id": r["id"], "day": r["day"], "text": r["entry"]}
                    for r in c.execute("SELECT id, day, entry FROM journal WHERE entry LIKE ? "
                                       "ORDER BY day DESC LIMIT ?", (q, limit))]
        return out[:limit]

    # ---------- routines ----------
    def add_routine(self, bot: str, name: str, cron: str, prompt: str, notify: bool = True,
                    now: datetime | None = None) -> dict:
        self.get(bot)
        nxt = Cron(cron).next_after(now or datetime.now()).timestamp()
        with self.lock, self.conn:
            c = self.conn.execute(
                "INSERT INTO routines(bot, name, cron, prompt, notify, next_run, created) "
                "VALUES (?,?,?,?,?,?,?)", (bot, name, cron, prompt, int(notify), nxt, time.time()))
        return self.routine(c.lastrowid)

    def routine(self, rid: int) -> dict:
        with self.lock:
            r = self.conn.execute("SELECT * FROM routines WHERE id=?", (rid,)).fetchone()
        if not r:
            raise BotError(f"no routine {rid}")
        return dict(r)

    def routines(self, bot: str | None = None) -> list:
        with self.lock:
            if bot:
                rows = self.conn.execute("SELECT * FROM routines WHERE bot=? ORDER BY id", (bot,))
            else:
                rows = self.conn.execute("SELECT * FROM routines ORDER BY id")
            return [dict(r) for r in rows]

    def rm_routine(self, rid: int) -> bool:
        with self.lock, self.conn:
            return self.conn.execute("DELETE FROM routines WHERE id=?", (rid,)).rowcount > 0

    def set_routine_enabled(self, rid: int, enabled: bool):
        with self.lock, self.conn:
            self.conn.execute("UPDATE routines SET enabled=? WHERE id=?", (int(enabled), rid))

    def due_routines(self, now: float) -> list:
        with self.lock:
            return [dict(r) for r in self.conn.execute(
                "SELECT * FROM routines WHERE enabled=1 AND next_run IS NOT NULL AND next_run<=? "
                "ORDER BY next_run", (now,))]

    def mark_routine_run(self, rid: int, ran_at: float, status: str, next_run: float):
        with self.lock, self.conn:
            self.conn.execute("UPDATE routines SET last_run=?, last_status=?, next_run=? WHERE id=?",
                              (ran_at, status[:500], next_run, rid))

    # ---------- tasks (rows; logic lives in tasks.py) ----------
    def task_insert(self, row: dict):
        cols = ",".join(row)
        with self.lock, self.conn:
            self.conn.execute(f"INSERT INTO tasks({cols}) VALUES ({','.join('?' * len(row))})",
                              tuple(row.values()))

    def task_update(self, tid: str, **fields):
        sets = ",".join(f"{k}=?" for k in fields)
        with self.lock, self.conn:
            self.conn.execute(f"UPDATE tasks SET {sets} WHERE id=?", (*fields.values(), tid))

    def task_get(self, tid: str) -> dict | None:
        with self.lock:
            r = self.conn.execute("SELECT * FROM tasks WHERE id=?", (tid,)).fetchone()
        return dict(r) if r else None

    def task_list(self, bot: str | None = None, status: str | None = None, limit: int = 100) -> list:
        q, a = "SELECT * FROM tasks WHERE 1=1", []
        if bot:
            q += " AND bot=?"
            a.append(bot)
        if status:
            q += " AND status=?"
            a.append(status)
        q += " ORDER BY created DESC LIMIT ?"
        a.append(limit)
        with self.lock:
            return [dict(r) for r in self.conn.execute(q, a)]
