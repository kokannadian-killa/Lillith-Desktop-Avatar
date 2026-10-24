"""Local-only HTTP API (stdlib). Binds to loopback only; rejects foreign Host /
Origin headers (DNS-rebinding / browser CSRF protection). Optional bearer token
via LILLITH_API_TOKEN.

GET    /health
GET    /bots                         POST /bots {name, ...profile}
GET    /bots/{b}  PATCH /bots/{b}  DELETE /bots/{b}
POST   /bots/{b}/chat {message, attachments?: [path|id]}
GET    /bots/{b}/history?limit=50    DELETE /bots/{b}/history
GET    /bots/{b}/memory              POST /bots/{b}/memory {text, type: fact|journal, day?}
DELETE /bots/{b}/memory/{fact|journal}/{id}
GET    /bots/{b}/memory/search?q=
GET    /bots/{b}/attachments         POST /bots/{b}/attachments {path} or {uri_list}
GET    /bots/{b}/attachments/{id}/read?offset=1&limit=100
DELETE /bots/{b}/attachments/{id}
GET    /bots/{b}/routines            POST /bots/{b}/routines {name, cron, prompt, notify?}
DELETE /routines/{id}                POST /routines/{id}/run
GET    /bots/{b}/export              POST /templates/import {template, name?}
"""
from __future__ import annotations

import json
import os
import re
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse, parse_qs

from ..attachments.config import LOOPBACK_HOSTS
from ..attachments.store import AttachmentError, NotFound
from .. import llm
from .store import BotError, BotNotFound
from .engine import Lillith

MAX_BODY = 2 * 1024 * 1024


class HTTPError(Exception):
    def __init__(self, code, msg):
        super().__init__(msg)
        self.code = code


def _is_loopback_host(h: str) -> bool:
    h = (h or "").lower()
    if h.startswith("["):
        h = h[: h.find("]") + 1]
    elif h.count(":") == 1:
        h = h.split(":")[0]
    return h in LOOPBACK_HOSTS or h.startswith("127.")


class Handler(BaseHTTPRequestHandler):
    server_version = "LillithLocal/0.1"
    app: Lillith = None
    lock = threading.Lock()
    token: str | None = None

    def log_message(self, fmt, *args):  # quiet; no remote logging
        if os.environ.get("LILLITH_API_LOG"):
            super().log_message(fmt, *args)

    def _send(self, code, obj):
        data = json.dumps(obj, default=str).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _body(self) -> dict:
        n = int(self.headers.get("Content-Length") or 0)
        if n > MAX_BODY:
            raise HTTPError(413, "body too large")
        if not n:
            return {}
        try:
            obj = json.loads(self.rfile.read(n))
        except json.JSONDecodeError:
            raise HTTPError(400, "invalid JSON")
        if not isinstance(obj, dict):
            raise HTTPError(400, "JSON object expected")
        return obj

    def _guard(self):
        if not _is_loopback_host(self.headers.get("Host", "")):
            raise HTTPError(403, "non-loopback Host header")
        origin = self.headers.get("Origin")
        if origin and origin != "null":
            if not _is_loopback_host(urlparse(origin).netloc):
                raise HTTPError(403, "cross-origin request refused")
        if self.token and self.headers.get("Authorization") != f"Bearer {self.token}":
            raise HTTPError(401, "missing or bad token")

    def _handle(self, method):
        try:
            self._guard()
            u = urlparse(self.path)
            q = {k: v[-1] for k, v in parse_qs(u.query).items()}
            parts = [p for p in u.path.split("/") if p]
            body = self._body() if method in ("POST", "PATCH") else {}
            with self.lock:
                code, obj = route(self.app, method, parts, q, body)
            self._send(code, obj)
        except HTTPError as e:
            self._send(e.code, {"error": str(e)})
        except (BotNotFound, NotFound) as e:
            self._send(404, {"error": str(e)})
        except (BotError, AttachmentError, ValueError, KeyError, TypeError) as e:
            self._send(400, {"error": str(e)})
        except llm.LLMError as e:
            self._send(502, {"error": str(e)})

    def do_GET(self):
        self._handle("GET")

    def do_POST(self):
        self._handle("POST")

    def do_PATCH(self):
        self._handle("PATCH")

    def do_DELETE(self):
        self._handle("DELETE")


def route(app: Lillith, m: str, p: list, q: dict, body: dict):
    B = app.bots
    if p == ["health"]:
        return 200, {"ok": True}
    if p == ["bots"]:
        if m == "GET":
            return 200, [b.to_dict() for b in B.list()]
        if m == "POST":
            name = body.pop("name", None)
            if not name:
                raise HTTPError(400, "name required")
            return 201, B.create(name, **body).to_dict()
    if p == ["templates", "import"] and m == "POST":
        return 201, app.import_template(body["template"], body.get("name")).to_dict()
    if len(p) >= 2 and p[0] == "routines":
        rid = int(p[1])
        if len(p) == 2 and m == "DELETE":
            return (200, {"deleted": rid}) if B.rm_routine(rid) else (404, {"error": "no routine"})
        if len(p) == 3 and p[2] == "run" and m == "POST":
            return 200, app.run_routine(rid, notify=bool(body.get("notify", False)))
    if len(p) >= 2 and p[0] == "bots":
        bot = B.get(p[1]).name
        rest = p[2:]
        if not rest:
            if m == "GET":
                return 200, B.get(bot).to_dict()
            if m == "PATCH":
                return 200, B.edit(bot, **body).to_dict()
            if m == "DELETE":
                app.delete_bot(bot, keep_files=q.get("keep_files") == "1")
                return 200, {"deleted": bot}
        if rest == ["chat"] and m == "POST":
            if not body.get("message"):
                raise HTTPError(400, "message required")
            return 200, app.chat(bot, body["message"], body.get("attachments"))
        if rest == ["history"]:
            if m == "GET":
                return 200, B.history(bot, int(q.get("limit", 50)))
            if m == "DELETE":
                B.clear_history(bot)
                return 200, {"cleared": bot}
        if rest == ["memory"]:
            if m == "GET":
                return 200, {"facts": B.facts(bot), "journal": B.journal(bot)}
            if m == "POST":
                if body.get("type") == "journal":
                    return 201, {"id": B.add_journal(bot, body["text"], body.get("day"))}
                return 201, {"id": B.add_fact(bot, body["text"])}
        if rest == ["memory", "search"] and m == "GET":
            return 200, B.search_memory(bot, q.get("q", ""))
        if len(rest) == 3 and rest[0] == "memory" and m == "DELETE":
            ok = (B.rm_journal if rest[1] == "journal" else B.rm_fact)(bot, int(rest[2]))
            return (200, {"deleted": int(rest[2])}) if ok else (404, {"error": "not found"})
        if rest == ["attachments"]:
            if m == "GET":
                return 200, [a.to_dict() for a in app.bot_attachments(bot)]
            if m == "POST":
                if body.get("uri_list"):
                    atts = app.attachments.add_uri_list(body["uri_list"], app.conv(bot))
                else:
                    atts = [app.attach(bot, body["path"])]
                return 201, [a.to_dict() for a in atts]
        if len(rest) == 2 and rest[0] == "attachments" and m == "DELETE":
            return 200, {"detached": app.detach(bot, rest[1])}
        if len(rest) == 3 and rest[0] == "attachments" and rest[2] == "read" and m == "GET":
            att = app.attachments.resolve(rest[1])
            if att.id not in app.attachments.db.attachments_for(app.conv(bot)):
                raise HTTPError(404, "not attached to this bot")
            return 200, app.attachments.read(att, int(q.get("offset", 1)), int(q.get("limit", 100)))
        if rest == ["routines"]:
            if m == "GET":
                return 200, B.routines(bot)
            if m == "POST":
                return 201, B.add_routine(bot, body["name"], body["cron"], body["prompt"],
                                          bool(body.get("notify", True)))
        if rest == ["export"] and m == "GET":
            return 200, app.export_template(bot, include_facts=q.get("facts") == "1")
    raise HTTPError(404, f"no route {m} /{'/'.join(p)}")


def make_server(app: Lillith, host: str = "127.0.0.1", port: int = 8765,
                token: str | None = None) -> ThreadingHTTPServer:
    if not _is_loopback_host(host):
        raise ValueError("the Lillith API only binds to loopback (127.0.0.1 / ::1)")
    handler = type("LillithHandler", (Handler,), {"app": app, "lock": threading.Lock(),
                                                   "token": token or os.environ.get("LILLITH_API_TOKEN")})
    if ":" in host:
        class V6(ThreadingHTTPServer):
            import socket as _s
            address_family = _s.AF_INET6
        return V6((host, port), handler)
    return ThreadingHTTPServer((host, port), handler)


def serve(home: Path | None = None, host: str = "127.0.0.1", port: int = 8765):
    app = Lillith(home)
    srv = make_server(app, host, port)
    print(f"Lillith API on http://{host}:{srv.server_address[1]} (loopback only)", flush=True)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        srv.server_close()
        app.close()
