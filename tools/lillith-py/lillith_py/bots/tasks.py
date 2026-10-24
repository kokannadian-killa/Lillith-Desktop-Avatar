"""Task queue + workers for bots: submit, progress, stop, retry, reassign,
stop-all, stall detection. State lives in registry.sqlite (so the CLI, the
scheduler and the API process all see it), plus an append-only JSONL job log
per task at <data>/lillith/bots/<bot>/jobs/<task-id>.jsonl and the result
text at .../results/<task-id>.md.

Statuses: queued -> running -> done | failed | cancelled | stalled.
A running task is "stalled" when no token/progress arrived for
``stall_minutes``; it is aborted and can be retried or reassigned.
"""
from __future__ import annotations

import json
import threading
import time
import uuid
from pathlib import Path

from .. import llm
from ..lillith_config import MAX_WORKERS
from .store import BotError, BotNotFound

ACTIVE = ("queued", "running")
TERMINAL = ("done", "failed", "cancelled", "stalled")


def new_task_id() -> str:
    return "t_" + uuid.uuid4().hex[:12]


class TaskManager:
    def __init__(self, app, workers: int = 1, stall_minutes: float = 5.0):
        self.app = app
        self.bots = app.bots
        self.workers = max(1, min(int(workers), MAX_WORKERS))
        self.stall_seconds = float(stall_minutes) * 60.0
        self._wake = threading.Condition()
        self._stop = threading.Event()
        self._threads: list = []
        self._aborts: dict = {}      # task id -> abort fn (this process)
        self._cancel: dict = {}      # task id -> Event
        self._claim_lock = threading.Lock()

    # ---------- job log ----------
    def _log(self, row: dict, event: str, **extra):
        bot = row.get("bot")
        if not bot:
            return
        d = self.bots.bot_dir(bot) / "jobs"
        d.mkdir(parents=True, exist_ok=True)
        rec = {"id": row["id"], "bot": bot, "task": row["task"], "event": event,
               "status": extra.pop("status", row.get("status")), "ts": time.time(),
               "created": row.get("created"), "started": row.get("started"),
               "finished": row.get("finished"), "result_path": row.get("result_path")}
        rec.update(extra)
        with open(d / f"{row['id']}.jsonl", "a", encoding="utf-8") as f:
            f.write(json.dumps(rec, default=str) + "\n")

    def job_log(self, tid: str) -> list:
        row = self.get(tid)
        p = self.bots.bot_dir(row["bot"]) / "jobs" / f"{tid}.jsonl"
        if not p.is_file():
            return []
        return [json.loads(l) for l in p.read_text(encoding="utf-8").splitlines() if l.strip()]

    # ---------- queries ----------
    def get(self, tid: str) -> dict:
        row = self.bots.task_get(tid)
        if not row:
            raise BotNotFound(f"no task {tid!r}")
        return self._view(row)

    def _view(self, row: dict) -> dict:
        row = dict(row)
        row["attachments"] = json.loads(row["attachments"] or "[]")
        row["cancel_requested"] = bool(row["cancel_requested"])
        row["result"] = None
        if row.get("result_path") and Path(row["result_path"]).is_file():
            row["result"] = Path(row["result_path"]).read_text(encoding="utf-8")
        return row

    def list(self, bot: str | None = None, status: str | None = None, limit: int = 100) -> list:
        return [self._view(r) for r in self.bots.task_list(bot, status, limit)]

    def bot_status(self, bot: str) -> dict:
        tasks = self.bots.task_list(bot, None, 20)
        running = [t for t in tasks if t["status"] == "running"]
        queued = [t for t in tasks if t["status"] == "queued"]
        now = time.time()
        status = "idle"
        current = None
        if running:
            current = running[0]["id"]
            lp = running[0]["last_progress"] or running[0]["started"] or now
            status = "stalled" if now - lp > self.stall_seconds else "working"
        elif tasks and tasks[0]["status"] == "stalled":
            status = "stalled"
            current = tasks[0]["id"]
        acts = [t.get("finished") or t.get("last_progress") or t.get("created") for t in tasks]
        acts.append(self.bots.last_message_time(bot))
        acts = [a for a in acts if a]
        return {"status": status, "current_task": current, "queued": len(queued),
                "last_activity": max(acts) if acts else None}

    # ---------- submit ----------
    def _pick_any(self) -> str:
        bots = self.bots.list()
        if not bots:
            raise BotError("no bots exist")

        def load(b):
            active = [t for t in self.bots.task_list(b.name, None, 50) if t["status"] in ACTIVE]
            return (len(active), self.bot_status(b.name)["last_activity"] or 0)
        return min(bots, key=load).name

    def submit(self, bot: str, task: str, attachments: list | None = None,
               source: str = "api", retry_of: str | None = None, run_inline: bool = False) -> dict:
        """Queue a task. ``run_inline`` inserts it already 'running' so no worker
        (in this or another process) claims it; the caller must run_task() it."""
        task = (task or "").strip()
        if not task:
            raise BotError("task text required")
        requested = bot or "any"
        target = self._pick_any() if requested == "any" else self.bots.get(requested).name
        for a in attachments or []:
            self.app.attach(target, a)
        row = {"id": new_task_id(), "bot": target, "requested_bot": requested, "task": task,
               "attachments": json.dumps(list(attachments or [])), "source": source,
               "status": "queued", "retry_of": retry_of, "created": time.time()}
        if run_inline:
            row.update(status="running", started=row["created"], last_progress=row["created"])
        self.bots.task_insert(row)
        self._log(row, "queued", status="queued")
        with self._wake:
            self._wake.notify_all()
        return self.get(row["id"])

    # ---------- control ----------
    def _request_cancel(self, row: dict) -> bool:
        if row["status"] == "queued":
            now = time.time()
            self.bots.task_update(row["id"], status="cancelled", cancel_requested=1, finished=now)
            self._log({**row, "finished": now}, "cancelled", status="cancelled")
            return True
        if row["status"] == "running":
            self.bots.task_update(row["id"], cancel_requested=1)
            ev = self._cancel.get(row["id"])
            if ev:
                ev.set()
            ab = self._aborts.get(row["id"])
            if ab:
                ab()
            return True
        return False

    def stop(self, tid: str) -> dict:
        row = self.bots.task_get(tid)
        if not row:
            raise BotNotFound(f"no task {tid!r}")
        self._request_cancel(row)
        return self.get(tid)

    def stop_all(self) -> dict:
        n = 0
        for st in ACTIVE:
            for row in self.bots.task_list(None, st, 100000):
                n += bool(self._request_cancel(row))
        return {"stopped": n}

    def retry(self, tid: str) -> dict:
        row = self.get(tid)
        if row["status"] in ACTIVE:
            raise BotError(f"task {tid} is still {row['status']}")
        return self.submit(row["bot"], row["task"], row["attachments"], row["source"], retry_of=tid)

    def reassign(self, tid: str, bot: str) -> dict:
        row = self.get(tid)
        target = self._pick_any() if bot == "any" else self.bots.get(bot).name
        if row["status"] == "queued":
            for a in row["attachments"]:
                self.app.attach(target, a)
            self.bots.task_update(tid, bot=target, requested_bot=bot)
            self._log({**row, "bot": target}, "reassigned", from_bot=row["bot"])
            return self.get(tid)
        if row["status"] == "running":
            self.stop(tid)
        return self.submit(target, row["task"], row["attachments"], row["source"], retry_of=tid)

    # ---------- execution ----------
    def _claim(self) -> dict | None:
        with self._claim_lock:
            busy = {t["bot"] for t in self.bots.task_list(None, "running", 1000)}
            for row in reversed(self.bots.task_list(None, "queued", 1000)):  # oldest first
                if row["bot"] in busy:
                    continue
                now = time.time()
                self.bots.task_update(row["id"], status="running", started=now, last_progress=now)
                row.update(status="running", started=now, last_progress=now)
                return row
        return None

    def run_task(self, tid: str) -> dict:
        """Run one task to completion in the calling thread (workers and the
        scheduler both use this)."""
        row = self.bots.task_get(tid)
        if row["status"] == "queued":
            now = time.time()
            self.bots.task_update(tid, status="running", started=now, last_progress=now)
            row = self.bots.task_get(tid)
        if row["status"] != "running":
            return self.get(tid)
        self._log(row, "started", status="running")
        ev = self._cancel.setdefault(tid, threading.Event())
        state = {"tokens": 0, "last_write": 0.0, "last_poll": 0.0}

        def on_token(text):
            now = time.time()
            state["tokens"] += 1
            if now - state["last_write"] >= 0.5:
                self.bots.task_update(tid, tokens=state["tokens"], last_progress=now)
                state["last_write"] = now
            if now - state["last_poll"] >= 0.5:  # cross-process stop (CLI / safeword)
                state["last_poll"] = now
                r = self.bots.task_get(tid)
                if r and (r["cancel_requested"] or r["status"] != "running"):
                    ev.set()
            if ev.is_set():
                raise llm.Cancelled()

        def register(fn):
            self._aborts[tid] = fn

        status, error, reply = "done", None, ""
        try:
            src = row["source"] if str(row["source"]).startswith("routine:") else f"task:{tid}"
            res = self.app.chat(row["bot"], row["task"], source=src,
                                on_token=on_token, register=register,
                                read_timeout=self.stall_seconds, check_safeword=False)
            reply = res["reply"]
        except llm.Cancelled:
            status = "cancelled"
        except llm.LLMError as e:
            cur = self.bots.task_get(tid)
            if ev.is_set() or (cur and cur["cancel_requested"]):
                status = "cancelled"
            elif cur and cur["status"] == "stalled" or "timed out" in str(e):
                status, error = "stalled", f"no progress for {self.stall_seconds / 60:g} min"
            else:
                status, error = "failed", str(e)
        except Exception as e:  # never kill the worker
            cur = self.bots.task_get(tid)
            if ev.is_set() or (cur and cur["cancel_requested"]):
                status = "cancelled"
            else:
                status, error = "failed", f"{type(e).__name__}: {e}"
        finally:
            self._aborts.pop(tid, None)
            self._cancel.pop(tid, None)
        cur = self.bots.task_get(tid)
        if cur and cur["status"] == "stalled":
            status = "stalled"
            error = error or f"no progress for {self.stall_seconds / 60:g} min"
        now = time.time()
        result_path = None
        if status == "done":
            d = self.bots.bot_dir(row["bot"]) / "results"
            d.mkdir(parents=True, exist_ok=True)
            result_path = str(d / f"{tid}.md")
            Path(result_path).write_text(reply, encoding="utf-8")
        self.bots.task_update(tid, status=status, error=error, finished=now, tokens=state["tokens"],
                              result_path=result_path)
        final = self.bots.task_get(tid)
        self._log(final, status, error=error, tokens=state["tokens"])
        return self.get(tid)

    def check_stalls(self) -> list:
        now = time.time()
        stalled = []
        for row in self.bots.task_list(None, "running", 1000):
            lp = row["last_progress"] or row["started"] or now
            if now - lp > self.stall_seconds:
                self.bots.task_update(row["id"], status="stalled")
                self._log(row, "stalled", status="stalled")
                ab = self._aborts.get(row["id"])
                if ab:
                    ab()
                stalled.append(row["id"])
        return stalled

    def _worker(self):
        while not self._stop.is_set():
            row = self._claim()
            if row is None:
                with self._wake:
                    self._wake.wait(timeout=1.0)
                continue
            self.run_task(row["id"])

    def _monitor(self):
        while not self._stop.wait(min(5.0, max(0.2, self.stall_seconds / 4))):
            self.check_stalls()

    def recover(self):
        """Tasks left 'running' by a crashed process become failed."""
        for row in self.bots.task_list(None, "running", 1000):
            self.bots.task_update(row["id"], status="failed", error="interrupted (process restart)",
                                  finished=time.time())
            self._log(row, "failed", status="failed", error="interrupted")

    def start(self):
        if self._threads:
            return
        self.recover()
        self._stop.clear()
        for i in range(self.workers):
            t = threading.Thread(target=self._worker, name=f"lillith-worker-{i}", daemon=True)
            t.start()
            self._threads.append(t)
        m = threading.Thread(target=self._monitor, name="lillith-stall-monitor", daemon=True)
        m.start()
        self._threads.append(m)

    def shutdown(self, timeout: float = 5.0):
        self._stop.set()
        self.stop_all()
        with self._wake:
            self._wake.notify_all()
        for t in self._threads:
            t.join(timeout)
        self._threads.clear()

    def wait(self, tid: str, timeout: float = 30.0) -> dict:
        end = time.time() + timeout
        while time.time() < end:
            row = self.get(tid)
            if row["status"] in TERMINAL:
                return row
            time.sleep(0.05)
        return self.get(tid)
