"""Bot registry, per-bot history, durable memory (facts + dated journal) and routines.

Everything lives in one SQLite file: $LILLITH_HOME/bots/bots.sqlite
(default ~/.local/share/lillith/bots/bots.sqlite). Profiles are also mirrored
to $LILLITH_HOME/bots/profiles/<name>.json so they are easy to hand-edit/back up.
"""
from __future__ import annotations

import json
import os
import re
import sqlite3
import time
from dataclasses import dataclass, field, asdict
from datetime import datetime, date
from pathlib import Path

from ..attachments.config import assert_loopback
from .cron import Cron

NAME_RE = re.compile(r"^[a-z0-9][a-z0-9_-]{0,47}$")
DEFAULT_TOOLS = ["list_attachments", "read_attachment", "search_attachments",
                 "search_memory", "write_memory"]


def lillith_home() -> Path:
    env = os.environ.get("LILLITH_HOME")
    if env:
        return Path(env).expanduser()
    base = os.environ.get("XDG_DATA_HOME") or str(Path.home() / ".local/share")
    return Path(base) / "lillith"


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
    persona: str = "You are a helpful local AI companion."
    avatar: str = ""                # path to avatar image
    endpoint: str = "http://127.0.0.1:8080"
    model: str = "dolphin3.0-llama3.2-3b-q4_k_m"
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


SCHEMA = """
CREATE TABLE IF NOT EXISTS bots (
    name TEXT PRIMARY KEY, profile TEXT NOT NULL, created REAL, updated REAL);
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT, bot TEXT NOT NULL REFERENCES bots(name) ON DELETE CASCADE,
    role TEXT NOT NULL, content TEXT NOT NULL, source TEXT DEFAULT 'chat', created REAL NOT NULL);
CREATE TABLE IF NOT EXISTS facts (
    id INTEGER PRIMARY KEY AUTOINCREMENT, bot TEXT NOT NULL REFERENCES bots(name) ON DELETE CASCADE,
    fact TEXT NOT NULL, created REAL NOT NULL, UNIQUE(bot, fact));
CREATE TABLE IF NOT EXISTS journal (
    id INTEGER PRIMARY KEY AUTOINCREMENT, bot TEXT NOT NULL REFERENCES bots(name) ON DELETE CASCADE,
    day TEXT NOT NULL, entry TEXT NOT NULL, created REAL NOT NULL);
CREATE TABLE IF NOT EXISTS routines (
    id INTEGER PRIMARY KEY AUTOINCREMENT, bot TEXT NOT NULL REFERENCES bots(name) ON DELETE CASCADE,
    name TEXT NOT NULL, cron TEXT NOT NULL, prompt TEXT NOT NULL, enabled INTEGER DEFAULT 1,
    notify INTEGER DEFAULT 1, last_run REAL, next_run REAL, last_status TEXT, created REAL NOT NULL);
CREATE INDEX IF NOT EXISTS idx_msg_bot ON messages(bot, id);
CREATE INDEX IF NOT EXISTS idx_journal_bot ON journal(bot, day);
"""


class BotStore:
    def __init__(self, home: Path | None = None):
        self.home = Path(home) if home else lillith_home()
        self.dir = self.home / "bots"
        (self.dir / "profiles").mkdir(parents=True, exist_ok=True)
        self.conn = sqlite3.connect(str(self.dir / "bots.sqlite"), timeout=10,
                                    check_same_thread=False)
        self.conn.row_factory = sqlite3.Row
        self.conn.execute("PRAGMA journal_mode=WAL")
        self.conn.execute("PRAGMA foreign_keys=ON")
        self.conn.executescript(SCHEMA)

    def close(self):
        self.conn.close()

    # ---------- bots ----------
    def _mirror(self, p: BotProfile):
        f = self.dir / "profiles" / f"{p.name}.json"
        f.write_text(json.dumps(p.to_dict(), indent=2), encoding="utf-8")

    def create(self, name: str, **fields) -> BotProfile:
        slug = slugify(name)
        if self.exists(slug):
            raise BotError(f"bot {slug!r} already exists")
        if "title" not in fields or not fields.get("title"):
            fields["title"] = name.strip()
        now = time.time()
        p = BotProfile.from_dict({**fields, "name": slug, "created": now, "updated": now}).validate()
        with self.conn:
            self.conn.execute("INSERT INTO bots VALUES (?,?,?,?)",
                              (slug, json.dumps(p.to_dict()), now, now))
        self._mirror(p)
        return p

    def exists(self, name: str) -> bool:
        return self.conn.execute("SELECT 1 FROM bots WHERE name=?", (name,)).fetchone() is not None

    def get(self, name: str) -> BotProfile:
        row = self.conn.execute("SELECT profile FROM bots WHERE name=?", (slugify(name),)).fetchone()
        if not row:
            raise BotNotFound(f"no bot named {name!r}")
        return BotProfile.from_dict(json.loads(row[0]))

    def list(self) -> list:
        rows = self.conn.execute("SELECT profile FROM bots ORDER BY created").fetchall()
        return [BotProfile.from_dict(json.loads(r[0])) for r in rows]

    def edit(self, name: str, **changes) -> BotProfile:
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
        p = self.get(name)
        with self.conn:
            self.conn.execute("DELETE FROM bots WHERE name=?", (p.name,))
        (self.dir / "profiles" / f"{p.name}.json").unlink(missing_ok=True)

    # ---------- history ----------
    def add_message(self, bot: str, role: str, content: str, source: str = "chat"):
        with self.conn:
            self.conn.execute("INSERT INTO messages(bot, role, content, source, created) "
                              "VALUES (?,?,?,?,?)", (bot, role, content, source, time.time()))

    def history(self, bot: str, limit: int = 50) -> list:
        rows = self.conn.execute(
            "SELECT * FROM (SELECT id, role, content, source, created FROM messages WHERE bot=? "
            "ORDER BY id DESC LIMIT ?) ORDER BY id", (bot, limit)).fetchall()
        return [dict(r) for r in rows]

    def clear_history(self, bot: str):
        with self.conn:
            self.conn.execute("DELETE FROM messages WHERE bot=?", (bot,))

    # ---------- memory ----------
    def add_fact(self, bot: str, fact: str) -> int:
        fact = " ".join(fact.split())[:1000]
        if not fact:
            raise BotError("empty fact")
        with self.conn:
            self.conn.execute("INSERT OR IGNORE INTO facts(bot, fact, created) VALUES (?,?,?)",
                              (bot, fact, time.time()))
        return self.conn.execute("SELECT id FROM facts WHERE bot=? AND fact=?", (bot, fact)).fetchone()[0]

    def facts(self, bot: str) -> list:
        return [dict(r) for r in self.conn.execute(
            "SELECT id, fact, created FROM facts WHERE bot=? ORDER BY id", (bot,))]

    def rm_fact(self, bot: str, fact_id: int) -> bool:
        with self.conn:
            c = self.conn.execute("DELETE FROM facts WHERE bot=? AND id=?", (bot, fact_id))
        return c.rowcount > 0

    def add_journal(self, bot: str, entry: str, day: str | None = None) -> int:
        day = day or date.today().isoformat()
        datetime.strptime(day, "%Y-%m-%d")
        with self.conn:
            c = self.conn.execute("INSERT INTO journal(bot, day, entry, created) VALUES (?,?,?,?)",
                                  (bot, day, entry.strip()[:4000], time.time()))
        return c.lastrowid

    def journal(self, bot: str, limit: int = 100) -> list:
        return [dict(r) for r in self.conn.execute(
            "SELECT id, day, entry, created FROM journal WHERE bot=? ORDER BY day DESC, id DESC LIMIT ?",
            (bot, limit))]

    def rm_journal(self, bot: str, entry_id: int) -> bool:
        with self.conn:
            c = self.conn.execute("DELETE FROM journal WHERE bot=? AND id=?", (bot, entry_id))
        return c.rowcount > 0

    def search_memory(self, bot: str, query: str, limit: int = 20) -> list:
        q = f"%{query.replace('%', '').replace('_', '')}%"
        out = [{"type": "fact", "id": r["id"], "text": r["fact"]} for r in self.conn.execute(
            "SELECT id, fact FROM facts WHERE bot=? AND fact LIKE ? ORDER BY id LIMIT ?", (bot, q, limit))]
        out += [{"type": "journal", "id": r["id"], "day": r["day"], "text": r["entry"]}
                for r in self.conn.execute(
                    "SELECT id, day, entry FROM journal WHERE bot=? AND entry LIKE ? "
                    "ORDER BY day DESC LIMIT ?", (bot, q, limit))]
        return out[:limit]

    # ---------- routines ----------
    def add_routine(self, bot: str, name: str, cron: str, prompt: str, notify: bool = True,
                    now: datetime | None = None) -> dict:
        self.get(bot)
        nxt = Cron(cron).next_after(now or datetime.now()).timestamp()
        with self.conn:
            c = self.conn.execute(
                "INSERT INTO routines(bot, name, cron, prompt, notify, next_run, created) "
                "VALUES (?,?,?,?,?,?,?)", (bot, name, cron, prompt, int(notify), nxt, time.time()))
        return self.routine(c.lastrowid)

    def routine(self, rid: int) -> dict:
        r = self.conn.execute("SELECT * FROM routines WHERE id=?", (rid,)).fetchone()
        if not r:
            raise BotError(f"no routine {rid}")
        return dict(r)

    def routines(self, bot: str | None = None) -> list:
        if bot:
            rows = self.conn.execute("SELECT * FROM routines WHERE bot=? ORDER BY id", (bot,))
        else:
            rows = self.conn.execute("SELECT * FROM routines ORDER BY id")
        return [dict(r) for r in rows]

    def rm_routine(self, rid: int) -> bool:
        with self.conn:
            c = self.conn.execute("DELETE FROM routines WHERE id=?", (rid,))
        return c.rowcount > 0

    def set_routine_enabled(self, rid: int, enabled: bool):
        with self.conn:
            self.conn.execute("UPDATE routines SET enabled=? WHERE id=?", (int(enabled), rid))

    def due_routines(self, now: float) -> list:
        return [dict(r) for r in self.conn.execute(
            "SELECT * FROM routines WHERE enabled=1 AND next_run IS NOT NULL AND next_run<=? "
            "ORDER BY next_run", (now,))]

    def mark_routine_run(self, rid: int, ran_at: float, status: str, next_run: float):
        with self.conn:
            self.conn.execute("UPDATE routines SET last_run=?, last_status=?, next_run=? WHERE id=?",
                              (ran_at, status[:500], next_run, rid))
