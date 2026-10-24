import json
import threading
import urllib.error
import urllib.request
from datetime import datetime
from pathlib import Path

import pytest

from helpers import make_png
from lillith_py.attachments import Config as AttachConfig
from lillith_py.attachments.config import NonLocalURLError
from lillith_py.bots import Lillith, Cron, CronError, BotError, BotNotFound
from lillith_py.bots import engine as engine_mod
from lillith_py.bots.api import make_server
from lillith_py.bots.cli import main as bot_cli
from lillith_py.bots.tools import parse_tool_calls


@pytest.fixture
def app(tmp_path):
    a = Lillith(tmp_path / "lh", AttachConfig(home=tmp_path / "lh" / "attachments", ocr=False))
    yield a
    a.close()


@pytest.fixture
def bot(app, mock_llama):
    return app.bots.create("Lily", persona="You are Lily, Ko's companion.", endpoint=mock_llama.url,
                           description="desktop avatar")


# ---------- profiles ----------
def test_create_list_edit_delete(app, mock_llama):
    p = app.bots.create("My Bot!", endpoint=mock_llama.url, sampling={"temperature": 0.2})
    assert p.name == "my-bot" and p.title == "My Bot!" and p.sampling.temperature == 0.2
    assert (app.home / "bots" / "my-bot" / "profile.json").is_file()
    with pytest.raises(BotError):
        app.bots.create("my bot")
    p2 = app.bots.edit("my-bot", description="d", sampling={"max_tokens": 64})
    assert p2.description == "d" and p2.sampling.max_tokens == 64 and p2.sampling.temperature == 0.2
    assert [b.name for b in app.bots.list()] == ["my-bot"]
    with pytest.raises(BotError):
        app.bots.edit("my-bot", nonsense=1)
    app.bots.add_fact("my-bot", "x")
    assert (app.home / "bots" / "my-bot" / "bot.sqlite").is_file()
    app.delete_bot("my-bot")
    assert app.bots.list() == []
    assert not (app.home / "bots" / "my-bot").exists()
    with pytest.raises(BotNotFound):
        app.bots.get("my-bot")


def test_profile_rejects_remote_endpoint_and_unknown_tools(app):
    with pytest.raises(NonLocalURLError):
        app.bots.create("x", endpoint="https://api.example.com")
    with pytest.raises(BotError):
        app.bots.create("y", tools=["shell"])


# ---------- memory ----------
def test_memory_and_prompt_injection(app, bot):
    app.bots.add_fact(bot.name, "Ko's favourite colour is crimson")
    app.bots.add_fact(bot.name, "Ko's favourite colour is crimson")  # dedupe
    app.bots.add_journal(bot.name, "Helped Ko set up Phantomcore", day="2026-10-20")
    app.bots.add_journal(bot.name, "Talked about Garuda", day="2026-10-22")
    assert len(app.bots.facts(bot.name)) == 1
    msgs, _ = app.build_messages(app.bots.get(bot.name), "hi")
    sysmsg = msgs[0]["content"]
    assert "crimson" in sysmsg and sysmsg.index("2026-10-22") < sysmsg.index("2026-10-20")
    assert [r["type"] for r in app.bots.search_memory(bot.name, "garuda")] == ["journal"]


def test_memory_budget_truncates(app, bot):
    for i in range(200):
        app.bots.add_fact(bot.name, f"fact number {i} " + "x" * 40)
    app.bots.edit(bot.name, memory_budget=100)
    block = app.memory_block(app.bots.get(bot.name))
    assert "memory truncated" in block and len(block) < 100 * 4 + 200


def test_history_budget(app, bot):
    for i in range(50):
        app.bots.add_message(bot.name, "user", f"question {i} " + "y" * 200)
        app.bots.add_message(bot.name, "assistant", f"answer {i} " + "z" * 200)
    app.bots.edit(bot.name, history_budget=300)
    msgs, _ = app.build_messages(app.bots.get(bot.name), "latest")
    mid = msgs[1:-1]
    assert mid and mid[0]["role"] == "user" and "answer 49" in mid[-1]["content"]
    assert sum(len(m["content"]) for m in mid) <= 300 * 4 + 100


# ---------- chat + attachments ----------
def test_chat_saves_history_and_sends_sampling(app, bot, mock_llama, tmp_path):
    f = tmp_path / "plan.txt"
    f.write_text("Phantomcore launch is on Friday")
    res = app.chat(bot.name, "when is launch?", attachments=[str(f)])
    assert res["reply"].startswith("MOCK REPLY")
    req = mock_llama.requests[-1]
    assert req["temperature"] == 0.7 and req["max_tokens"] == 512
    assert "Phantomcore launch" in req["messages"][-1]["content"]
    assert [h["role"] for h in app.bots.history(bot.name)] == ["user", "assistant"]
    assert [a.name for a in app.bot_attachments(bot.name)] == ["plan.txt"]


def test_per_bot_attachment_isolation(app, bot, mock_llama, tmp_path):
    other = app.bots.create("Other", endpoint=mock_llama.url)
    f = tmp_path / "secret.txt"
    f.write_text("only for lily")
    app.attach(bot.name, str(f))
    assert app.bot_attachments(other.name) == []
    mock_llama.script = [
        '<tool_call>{"name": "read_attachment", "arguments": {"id": "secret.txt"}}</tool_call>',
        "done"]
    res = app.chat(other.name, "read it")
    assert "not attached to this bot" in res["tools"][0]["result"]


def test_tool_loop_write_and_search_memory(app, bot, mock_llama):
    mock_llama.script = [
        '<tool_call>{"name": "write_memory", "arguments": {"text": "Ko uses fish shell"}}</tool_call>',
        "Noted! I'll remember that."]
    res = app.chat(bot.name, "remember I use fish")
    assert res["reply"] == "Noted! I'll remember that."
    assert res["tools"][0]["name"] == "write_memory"
    assert app.bots.facts(bot.name)[0]["fact"] == "Ko uses fish shell"
    last = mock_llama.requests[-1]["messages"][-1]["content"]
    assert '<tool_result name="write_memory">' in last


def test_tool_allowlist_enforced(app, bot, mock_llama):
    app.bots.edit(bot.name, tools=["search_memory"])
    mock_llama.script = [
        '<tool_call>{"name": "write_memory", "arguments": {"text": "x"}}</tool_call>', "ok"]
    res = app.chat(bot.name, "hi")
    assert "not allowed" in res["tools"][0]["result"] and app.bots.facts(bot.name) == []
    mock_llama.script = ['<tool_call>{"name": "run_shell", "arguments": {"cmd": "rm -rf /"}}</tool_call>',
                         "ok"]
    assert "not allowed" in app.chat(bot.name, "hi")["tools"][0]["result"]


def test_tool_rounds_capped(app, bot, mock_llama):
    loop = '<tool_call>{"name": "list_attachments", "arguments": {}}</tool_call>'
    mock_llama.script = [loop] * 10
    res = app.chat(bot.name, "loop")
    assert len(res["tools"]) <= engine_mod.MAX_TOOL_ROUNDS
    assert "<tool_call>" not in res["reply"]


def test_openai_style_tool_calls_parsed():
    calls = parse_tool_calls({"content": None, "tool_calls": [
        {"function": {"name": "search_memory", "arguments": '{"query": "x"}'}}]})
    assert calls == [("search_memory", {"query": "x"})]


# ---------- templates ----------
def test_export_import_template(app, bot, tmp_path):
    av = tmp_path / "lily.png"
    av.write_bytes(make_png(16, 16))
    app.bots.edit(bot.name, avatar=str(av))
    app.bots.add_fact(bot.name, "private fact")
    app.bots.add_message(bot.name, "user", "private chat")
    app.bots.add_routine(bot.name, "morning", "0 9 * * *", "Good morning summary")
    t = app.export_template(bot.name)
    s = json.dumps(t)
    assert "private fact" not in s and "private chat" not in s and "created" not in t["profile"]
    assert t["routines"][0]["cron"] == "0 9 * * *" and t["avatar"]["filename"] == "lily.png"
    p = app.import_template(json.loads(s), name="Lily Copy")
    assert p.name == "lily-copy" and p.persona == bot.persona
    assert Path(p.avatar).read_bytes() == av.read_bytes()
    assert app.bots.routines(p.name)[0]["name"] == "morning"
    assert app.bots.facts(p.name) == [] and app.bots.history(p.name) == []
    with pytest.raises(BotError):
        app.import_template({"format": "something-else"})


# ---------- cron + routines ----------
@pytest.mark.parametrize("expr,after,expected", [
    ("*/15 * * * *", datetime(2026, 1, 1, 10, 7), datetime(2026, 1, 1, 10, 15)),
    ("0 9 * * mon-fri", datetime(2026, 10, 23, 9, 0), datetime(2026, 10, 26, 9, 0)),
    ("@daily", datetime(2026, 10, 23, 21, 0), datetime(2026, 10, 24, 0, 0)),
    ("30 6 1 * *", datetime(2026, 10, 23), datetime(2026, 11, 1, 6, 30)),
    ("0 0 29 2 *", datetime(2026, 3, 1), datetime(2028, 2, 29)),
    ("0 12 13 * 5", datetime(2026, 10, 23, 13), datetime(2026, 10, 30, 12)),  # dom OR dow
])
def test_cron(expr, after, expected):
    assert Cron(expr).next_after(after) == expected


@pytest.mark.parametrize("bad", ["* * * *", "61 * * * *", "* * * * 9", "*/0 * * * *", "a b c d e"])
def test_cron_invalid(bad):
    with pytest.raises(CronError):
        Cron(bad)


def test_routine_runs_when_due(app, bot, mock_llama, monkeypatch):
    sent = []
    monkeypatch.setattr(engine_mod, "send_notification", lambda *a: sent.append(a) or True)
    r = app.bots.add_routine(bot.name, "check-in", "*/5 * * * *", "How is Ko doing?",
                             now=datetime(2026, 10, 23, 8, 2))
    assert app.run_due(now=datetime(2026, 10, 23, 8, 4)) == []
    res = app.run_due(now=datetime(2026, 10, 23, 8, 5))
    assert len(res) == 1 and res[0]["status"] == "ok"
    rr = app.bots.routine(r["id"])
    assert rr["next_run"] == datetime(2026, 10, 23, 8, 10).timestamp() and rr["last_status"] == "ok"
    h = app.bots.history(bot.name)
    assert h[-1]["source"] == "routine:check-in" and h[-1]["role"] == "assistant"
    assert sent and "check-in" in sent[0][0]


def test_routine_error_recorded(app, mock_llama, monkeypatch):
    monkeypatch.setattr(engine_mod, "send_notification", lambda *a: True)
    app.bots.create("dead", endpoint="http://127.0.0.1:9")  # nothing listens
    r = app.bots.add_routine("dead", "x", "* * * * *", "hi", now=datetime(2026, 1, 1))
    res = app.run_due(now=datetime(2026, 1, 1, 0, 1))
    assert res[0]["status"].startswith("failed")
    assert app.bots.routine(r["id"])["last_status"].startswith("failed")


# ---------- HTTP API ----------
@pytest.fixture
def api(app):
    srv = make_server(app, "127.0.0.1", 0, token="test-token")
    t = threading.Thread(target=srv.serve_forever, daemon=True)
    t.start()
    base = f"http://127.0.0.1:{srv.server_address[1]}"
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def call(method, path, body=None, headers=None):
        req = urllib.request.Request(base + path, method=method,
                                     data=json.dumps(body).encode() if body is not None else None,
                                     headers={"Content-Type": "application/json",
                                              "Authorization": "Bearer test-token", **(headers or {})})
        try:
            with opener.open(req, timeout=10) as r:
                return r.status, json.loads(r.read())
        except urllib.error.HTTPError as e:
            return e.code, json.loads(e.read())
    yield call
    srv.shutdown()
    srv.server_close()


def test_api_end_to_end(api, mock_llama, tmp_path):
    code, h = api("GET", "/health")
    assert code == 200 and h["ok"] is True and h["workers"] == 1
    code, b = api("POST", "/bots", {"name": "Nyx", "endpoint": mock_llama.url, "persona": "You are Nyx."})
    assert code == 201 and b["name"] == "nyx"
    assert api("PATCH", "/bots/nyx", {"description": "night"})[1]["description"] == "night"
    f = tmp_path / "n.txt"
    f.write_text("moon phases\nfull moon friday")
    code, atts = api("POST", "/bots/nyx/attachments", {"path": str(f)})
    assert code == 201
    code, r = api("GET", f"/bots/nyx/attachments/{atts[0]['id']}/read?offset=2&limit=1")
    assert r["lines"] == [[2, "full moon friday"]]
    assert api("POST", "/bots/nyx/memory", {"text": "Ko loves the moon"})[0] == 201
    assert api("GET", "/bots/nyx/memory/search?q=moon")[1][0]["type"] == "fact"
    code, c = api("POST", "/bots/nyx/chat", {"message": "hi"})
    assert code == 200 and c["reply"].startswith("MOCK")
    assert len(api("GET", "/bots/nyx/history")[1]) == 2
    code, ro = api("POST", "/bots/nyx/routines", {"name": "r", "cron": "@hourly", "prompt": "p"})
    assert code == 201
    assert api("POST", f"/routines/{ro['id']}/run", {})[1]["status"] == "ok"
    code, t = api("GET", "/bots/nyx/export")
    assert t["format"] == "lillith-bot-template"
    assert api("POST", "/templates/import", {"template": t, "name": "nyx2"})[0] == 201
    assert api("DELETE", f"/routines/{ro['id']}")[0] == 200
    assert api("DELETE", "/bots/nyx")[0] == 200
    assert api("GET", "/bots/nyx")[0] == 404
    assert api("GET", "/nope")[0] == 404
    assert api("POST", "/bots", {"name": "bad", "endpoint": "http://8.8.8.8"})[0] == 400
    assert api("GET", "/health", headers={"Authorization": ""})[0] == 401
    assert api("GET", "/health", headers={"Authorization": "Bearer wrong"})[0] == 401


def test_api_rejects_foreign_host_and_origin(api):
    assert api("GET", "/health", headers={"Host": "evil.example"})[0] == 403
    assert api("GET", "/health", headers={"Origin": "https://evil.example"})[0] == 403
    assert api("GET", "/health", headers={"Origin": "http://127.0.0.1:3000"})[0] == 200


def test_api_refuses_non_loopback_bind(app):
    with pytest.raises(ValueError):
        make_server(app, "0.0.0.0", 0)


def test_api_token_autogenerated_0600(app):
    import stat
    from lillith_py.bots.api import token_path
    srv = make_server(app, "127.0.0.1", 0)  # no token given -> <data>/api.token
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    p = token_path(app.home)
    assert p == app.home / "api.token" and p.is_file()
    assert stat.S_IMODE(p.stat().st_mode) == 0o600
    tok = p.read_text().strip()
    assert len(tok) >= 32
    url = f"http://127.0.0.1:{srv.server_address[1]}/health"
    op = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with pytest.raises(urllib.error.HTTPError) as e:
        op.open(url, timeout=5)
    assert e.value.code == 401
    assert op.open(urllib.request.Request(url, headers={"Authorization": f"Bearer {tok}"}),
                   timeout=5).status == 200
    srv.shutdown()
    srv.server_close()
    # second start reuses the same token
    srv2 = make_server(app, "127.0.0.1", 0, start_workers=False)
    assert srv2.RequestHandlerClass.token == tok
    srv2.server_close()


def test_api_token(app, monkeypatch):
    srv = make_server(app, "127.0.0.1", 0, token="s3cret")
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    url = f"http://127.0.0.1:{srv.server_address[1]}/bots"
    op = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with pytest.raises(urllib.error.HTTPError) as e:
        op.open(url, timeout=5)
    assert e.value.code == 401
    req = urllib.request.Request(url, headers={"Authorization": "Bearer s3cret"})
    assert op.open(req, timeout=5).status == 200
    srv.shutdown()
    srv.server_close()


# ---------- CLI ----------
def test_bot_cli(tmp_path, mock_llama, capsys, monkeypatch):
    monkeypatch.setattr(engine_mod, "send_notification", lambda *a: True)
    h = ["--home", str(tmp_path / "clihome")]
    assert bot_cli(h + ["create", "Lillith", "--persona", "You are Lillith.",
                        "--endpoint", mock_llama.url, "--temperature", "0.5"]) == 0
    assert bot_cli(h + ["list"]) == 0
    assert "lillith" in capsys.readouterr().out
    assert bot_cli(h + ["edit", "lillith", "--description", "avatar", "--tools", "search_memory"]) == 0
    assert bot_cli(h + ["memory", "add", "lillith", "Ko runs Garuda"]) == 0
    assert bot_cli(h + ["memory", "journal", "lillith", "first day", "--day", "2026-10-23"]) == 0
    f = tmp_path / "a.md"
    f.write_text("# doc")
    assert bot_cli(h + ["attach", "add", "lillith", str(f)]) == 0
    capsys.readouterr()
    assert bot_cli(h + ["chat", "lillith", "hello"]) == 0
    assert "MOCK REPLY" in capsys.readouterr().out
    assert bot_cli(h + ["routine", "add", "lillith", "daily", "@daily", "Daily summary"]) == 0
    assert bot_cli(h + ["routine", "list"]) == 0
    assert "lillith/daily" in capsys.readouterr().out
    assert bot_cli(h + ["routine", "run", "1"]) == 0
    out = tmp_path / "t.json"
    assert bot_cli(h + ["export", "lillith", "-o", str(out)]) == 0
    assert bot_cli(h + ["import", str(out), "--name", "lillith-2"]) == 0
    assert bot_cli(h + ["routine", "rm", "1"]) == 0
    assert bot_cli(h + ["rm", "lillith-2"]) == 0
    assert bot_cli(h + ["routine", "add", "lillith", "bad", "99 * * * *", "x"]) == 1
    assert bot_cli(h + ["show", "ghost"]) == 1
    assert bot_cli(h + ["scheduler", "--once", "--no-notify"]) == 0
