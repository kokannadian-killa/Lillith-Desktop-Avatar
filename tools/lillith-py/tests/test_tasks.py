"""Tasks, workers, stall detection, safeword/stop-all, per-bot layout, config
merge, the Bots-tab API, tool policy and the network guard."""
import json
import socket
import stat
import threading
import time
import urllib.error
import urllib.request

import pytest

import netguard
from conftest import write_config
from lillith_py.attachments import Config as AttachConfig
from lillith_py.bots import Lillith, BotError
from lillith_py.bots import tools as toolmod
from lillith_py.bots.api import make_server
from lillith_py.bots.cli import main as bot_cli
from lillith_py.bots.contract import ContractRequired, request_privileged_action
from lillith_py.bots.safeword import is_safeword
from lillith_py.lillith_config import (llm_settings, sidecar_settings, load_toml, config_path,
                                       DEFAULT_ENDPOINT, NonLocalURLError)

LONG = " ".join(f"w{i}" for i in range(40))


def make_app(tmp_path, **sidecar):
    cfg = load_toml()
    cfg = {**cfg, "sidecar": {**cfg.get("sidecar", {}), **sidecar}}
    return Lillith(tmp_path / "lh", AttachConfig(home=tmp_path / "lh" / "attachments", ocr=False),
                   config=cfg)


@pytest.fixture
def app(tmp_path, mock_llama):
    a = make_app(tmp_path)
    yield a
    a.close()


# ---------- config merge ----------
def test_settings_from_config_toml(tmp_path, mock_llama):
    s = llm_settings()
    assert s.endpoint == mock_llama.url and s.model == "mock-model" and s.source == "[chat]"
    assert s.persona == "You are Lillith (test persona)."
    s2 = llm_settings({"chat": {"endpoint": "http://127.0.0.1:1", "model": "a"},
                       "llm": {"endpoint": "http://127.0.0.1:2", "model": "b", "persona": "P"}})
    assert (s2.endpoint, s2.model, s2.persona, s2.source) == ("http://127.0.0.1:2", "b", "P", "[llm]")
    with pytest.raises(NonLocalURLError):
        llm_settings({"llm": {"endpoint": "http://10.0.0.2:8080"}})
    sc = sidecar_settings({"sidecar": {"port": 9000, "workers": 7, "stall_minutes": 2}})
    assert (sc.port, sc.workers, sc.stall_minutes) == (9000, 2, 2.0)
    assert sidecar_settings({}).port == 8765 and sidecar_settings({}).workers == 1
    with pytest.raises(ValueError):
        sidecar_settings({"sidecar": {"port": 70000}})


def test_missing_config_falls_back_to_8080(tmp_path):
    assert not config_path().exists()
    s = llm_settings()
    assert s.endpoint == DEFAULT_ENDPOINT == "http://127.0.0.1:8080" and s.source == "default"


def test_bot_inherits_config_endpoint_model_persona(app, mock_llama):
    app.bots.create("plain")
    r = app.chat("plain", "hello")
    sent = mock_llama.requests[-1]
    assert r["reply"].startswith("MOCK") and sent["model"] == "mock-model" and sent["stream"] is True
    assert sent["messages"][0]["content"].startswith("You are Lillith (test persona).")


def test_templates(app, mock_llama):
    p = app.create_bot("Res", template="researcher")
    assert p.role == "researcher" and "research bot" in p.persona and p.endpoint == ""
    with pytest.raises(BotError):
        app.create_bot("x", template="nope")
    r = app.chat("res", "hi")
    assert "research bot" in mock_llama.requests[-1]["messages"][0]["content"] and r["reply"]


# ---------- per-bot layout ----------
def test_per_bot_layout_and_job_log(app, mock_llama):
    app.bots.create("alpha", role="helper")
    app.bots.add_fact("alpha", "Ko likes tea")
    t = app.tasks.submit("alpha", "summarise", run_inline=True)
    t = app.tasks.run_task(t["id"])
    assert t["status"] == "done" and t["result"].startswith("MOCK") and t["tokens"] > 0
    d = app.home / "bots" / "alpha"
    assert (d / "profile.json").is_file() and (d / "bot.sqlite").is_file()
    assert (d / "results" / f"{t['id']}.md").read_text() == t["result"]
    log = [json.loads(l) for l in (d / "jobs" / f"{t['id']}.jsonl").read_text().splitlines()]
    assert [e["event"] for e in log] == ["queued", "started", "done"]
    for e in log:
        assert {"id", "bot", "task", "status", "ts", "created", "started", "finished",
                "result_path"} <= set(e)
    assert log[-1]["result_path"] == str(d / "results" / f"{t['id']}.md")
    assert app.tasks.job_log(t["id"]) == log
    # never touches Lillith's main memory DB
    assert not (app.home / "memory.db").exists()
    assert not list(app.home.rglob("memory.db"))


# ---------- workers ----------
def test_workers_run_queue_and_any(app, mock_llama):
    for n in ("a", "b"):
        app.bots.create(n)
    app.tasks.start()
    busy = app.tasks.submit("a", "one")
    t2 = app.tasks.submit("any", "two")
    assert t2["bot"] == "b" and t2["requested_bot"] == "any"  # 'a' already has work
    for t in (busy, t2):
        assert app.tasks.wait(t["id"], 15)["status"] == "done"
    assert app.tasks.bot_status("a")["status"] == "idle"
    assert app.tasks.bot_status("a")["last_activity"]


def test_worker_concurrency_clamped(tmp_path, mock_llama):
    a = make_app(tmp_path, workers=9)
    assert a.tasks.workers == 2
    a.close()


def test_stop_running_task(app, mock_llama):
    app.bots.create("slow")
    mock_llama.script.append({"content": LONG, "_chunk_delay": 0.2})
    app.tasks.start()
    t = app.tasks.submit("slow", "go")
    for _ in range(100):
        if app.tasks.get(t["id"])["status"] == "running" and app.tasks.get(t["id"])["tokens"] >= 0:
            break
        time.sleep(0.05)
    time.sleep(0.5)
    assert app.tasks.bot_status("slow")["status"] == "working"
    app.tasks.stop(t["id"])
    r = app.tasks.wait(t["id"], 10)
    assert r["status"] == "cancelled" and r["result"] is None


def test_stop_all_cancels_running_and_queued(app, mock_llama):
    app.bots.create("x")
    mock_llama.script.append({"content": LONG, "_chunk_delay": 0.2})
    app.tasks.start()
    t1 = app.tasks.submit("x", "first")
    t2 = app.tasks.submit("x", "second")  # same bot -> waits in queue
    time.sleep(0.8)
    assert app.tasks.get(t2["id"])["status"] == "queued"
    assert app.stop_all()["stopped"] == 2
    assert app.tasks.wait(t1["id"], 10)["status"] == "cancelled"
    assert app.tasks.get(t2["id"])["status"] == "cancelled"


@pytest.mark.parametrize("word", ["red", "RED!", " Red ", "OOC: stop", "ok (ooc stop) sorry"])
def test_safeword_in_chat_stops_the_reply_but_cancels_no_task(app, mock_llama, word):
    app.bots.create("y")
    app.bots.create("z")
    mock_llama.script.append({"content": LONG, "_chunk_delay": 0.2})
    app.tasks.start()
    t1 = app.tasks.submit("y", "long job")
    t2 = app.tasks.submit("y", "queued job")
    time.sleep(0.6)
    n_before = len(mock_llama.requests)
    r = app.chat("z", word)
    assert r["safeword"] is True and r["stopped"] == 0
    assert len(mock_llama.requests) == n_before  # no model call
    assert app.bots.history("z") == []          # safeword is never stored
    # Nothing is shut down: the running task goes on, the queued one waits.
    assert app.tasks.get(t1["id"])["status"] == "running"
    assert app.tasks.get(t2["id"])["status"] == "queued"
    app.stop_all()  # tidy up for the next test
    assert app.tasks.wait(t1["id"], 10)["status"] == "cancelled"


def test_is_safeword_matches_rust_rules():
    for s in ["red", "RED", " Red! ", "OOC: stop", "(ooc stop)", "ok... ooc: stop",
              "sorry OOC:STOP please"]:
        assert is_safeword(s), s
    for s in ["", "redo", "reddit", "stop", "ooc: go on", "I read a red book"]:
        assert not is_safeword(s), s


def test_stall_detection_then_retry(tmp_path, mock_llama):
    a = make_app(tmp_path, stall_minutes=0.01)  # 0.6 s
    a.bots.create("st")
    mock_llama.script.append({"content": LONG, "_hang": 30})
    a.tasks.start()
    t = a.tasks.submit("st", "hang")
    r = a.tasks.wait(t["id"], 15)
    assert r["status"] == "stalled" and "no progress" in r["error"]
    assert a.tasks.bot_status("st")["status"] == "stalled"
    r2 = a.tasks.retry(t["id"])
    assert r2["retry_of"] == t["id"]
    assert a.tasks.wait(r2["id"], 15)["status"] == "done"
    assert a.tasks.bot_status("st")["status"] == "idle"
    a.close()


def test_reassign_and_retry_rules(app, mock_llama):
    app.bots.create("p")
    app.bots.create("q")
    t = app.tasks.submit("p", "move me")          # no workers: stays queued
    moved = app.tasks.reassign(t["id"], "q")
    assert moved["id"] == t["id"] and moved["bot"] == "q" and moved["status"] == "queued"
    with pytest.raises(BotError):
        app.tasks.retry(t["id"])                   # still active
    app.tasks.stop(t["id"])
    new = app.tasks.reassign(t["id"], "p")         # terminal -> new task elsewhere
    assert new["id"] != t["id"] and new["bot"] == "p" and new["retry_of"] == t["id"]


def test_recover_marks_orphaned_running_failed(app, mock_llama):
    app.bots.create("o")
    t = app.tasks.submit("o", "x", run_inline=True)  # 'running' but nobody runs it
    app.tasks.recover()
    assert app.tasks.get(t["id"])["status"] == "failed"


def test_cli_task_commands(tmp_path, mock_llama, capsys):
    h = ["--home", str(tmp_path / "c")]
    assert bot_cli(h + ["create", "Helper", "--template", "assistant"]) == 0
    assert bot_cli(h + ["templates"]) == 0
    assert "researcher" in capsys.readouterr().out
    assert bot_cli(h + ["task", "run", "any", "hello"]) == 0
    assert "MOCK" in capsys.readouterr().out
    assert bot_cli(h + ["--json", "task", "submit", "helper", "later"]) == 0
    tid = json.loads(capsys.readouterr().out)["id"]
    assert bot_cli(h + ["task", "reassign", tid, "helper"]) == 0
    assert bot_cli(h + ["stop-all"]) == 0
    assert "stopped 1" in capsys.readouterr().out
    assert bot_cli(h + ["task", "retry", tid]) == 0
    assert bot_cli(h + ["task", "list"]) == 0
    assert bot_cli(h + ["task", "log", tid]) == 0
    assert bot_cli(h + ["chat", "helper", "red"]) == 0
    assert "Okay, stopping" in capsys.readouterr().out  # stops the reply, cancels no task


# ---------- Bots-tab API ----------
@pytest.fixture
def api(app):
    srv = make_server(app, "127.0.0.1", 0)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    tok = (app.home / "api.token").read_text().strip()
    base = f"http://127.0.0.1:{srv.server_address[1]}"
    op = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def call(method, path, body=None, auth=True):
        h = {"Content-Type": "application/json"}
        if auth:
            h["Authorization"] = f"Bearer {tok}"
        req = urllib.request.Request(base + path, method=method, headers=h,
                                     data=json.dumps(body).encode() if body is not None else None)
        try:
            with op.open(req, timeout=10) as r:
                return r.status, json.loads(r.read())
        except urllib.error.HTTPError as e:
            return e.code, json.loads(e.read())
    yield call
    srv.shutdown()
    srv.server_close()


def test_api_bots_tab_flow(api, app, mock_llama):
    assert api("GET", "/bots", auth=False)[0] == 401
    assert stat.S_IMODE((app.home / "api.token").stat().st_mode) == 0o600
    assert api("POST", "/bots", {"name": "Scout", "template": "researcher"})[0] == 201
    assert api("POST", "/bots", {"name": "Quill", "role": "writer"})[0] == 201
    code, bots = api("GET", "/bots")
    assert code == 200 and [b["id"] for b in bots] == ["scout", "quill"]
    assert set(bots[0]) >= {"id", "name", "role", "status", "last_activity", "current_task", "queued"}
    assert bots[0]["role"] == "researcher" and bots[0]["status"] == "idle"

    code, t = api("POST", "/tasks", {"bot": "any", "task": "find the moon"})
    assert code == 201 and t["status"] in ("queued", "running", "done")
    for _ in range(200):
        code, t = api("GET", f"/tasks/{t['id']}")
        if t["status"] == "done":
            break
        time.sleep(0.05)
    assert t["status"] == "done" and t["result"].startswith("MOCK") and t["result_path"]
    assert [e["event"] for e in api("GET", f"/tasks/{t['id']}/log")[1]][-1] == "done"
    assert api("POST", f"/tasks/{t['id']}/retry")[0] == 201
    assert api("POST", "/tasks", {"bot": "scout"})[0] == 400

    mock_llama.script.append({"content": LONG, "_chunk_delay": 0.2})
    code, slow = api("POST", "/tasks", {"bot": "quill", "task": "slow"})
    time.sleep(0.5)
    code, q2 = api("POST", "/tasks", {"bot": "quill", "task": "waiting"})
    code, moved = api("POST", f"/tasks/{q2['id']}/reassign", {"bot": "scout"})
    assert moved["bot"] == "scout"
    code, st = api("POST", f"/tasks/{slow['id']}/stop")
    assert code == 200 and st["cancel_requested"] is True
    assert app.tasks.wait(slow["id"], 10)["status"] == "cancelled"
    code, r = api("POST", "/stop-all")
    assert code == 200 and "stopped" in r
    assert api("GET", "/tasks?bot=quill")[1][0]["id"] == slow["id"]
    assert api("GET", "/tasks/t_nope")[0] == 404
    code, c = api("POST", "/bots/quill/chat", {"message": "OOC: stop"})
    assert c["safeword"] is True


# ---------- tool policy ----------
def test_tool_set_is_closed():
    assert set(toolmod.TOOLS) == {"list_attachments", "read_attachment", "search_attachments",
                                  "search_memory", "write_memory"}
    with pytest.raises(ContractRequired) as e:
        request_privileged_action("run_command", "ls")
    assert "contract-card" in str(e.value)


def test_privileged_tool_call_refused(app, mock_llama):
    app.bots.create("t", tools=["search_memory"])
    mock_llama.script += ['<tool_call>{"name": "run_command", "arguments": {"cmd": "rm -rf ~"}}</tool_call>',
                          "ok"]
    r = app.chat("t", "do it")
    assert r["tools"][0]["result"].startswith("error: refused") and "contract-card" in r["tools"][0]["result"]


# ---------- network guard ----------
def test_network_guard_blocks_non_loopback():
    before = list(netguard.VIOLATIONS)
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        with pytest.raises(netguard.NetworkGuardError):
            s.connect(("8.8.8.8", 53))
        with pytest.raises(netguard.NetworkGuardError):
            socket.create_connection(("1.1.1.1", 443), timeout=1)
        with pytest.raises(netguard.NetworkGuardError):
            socket.getaddrinfo("example.com", 80)
    finally:
        s.close()
        netguard.VIOLATIONS[:] = before  # these three were deliberate
    assert netguard._is_local("127.0.0.1") and netguard._is_local("::1")
    assert netguard._is_local("localhost") and not netguard._is_local("10.0.0.1")
