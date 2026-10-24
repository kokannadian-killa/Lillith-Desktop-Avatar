"""lillith-bot: local bot deployment CLI."""
from __future__ import annotations

import argparse
import json
import sys
from datetime import datetime
from pathlib import Path

from .. import llm
from ..attachments.store import AttachmentError
from .cron import CronError
from .store import BotError
from .engine import Lillith, run_scheduler

PROFILE_OPTS = ["title", "description", "persona", "avatar", "endpoint", "model"]
INT_OPTS = ["context_tokens", "memory_budget", "attachment_budget", "history_budget"]
SAMPLING = {"temperature": float, "top_p": float, "top_k": int, "max_tokens": int,
            "repeat_penalty": float}


def _profile_args(sp, creating: bool):
    for o in PROFILE_OPTS:
        sp.add_argument(f"--{o.replace('_', '-')}")
    sp.add_argument("--persona-file", help="read persona/system prompt from a file")
    for o in INT_OPTS:
        sp.add_argument(f"--{o.replace('_', '-')}", type=int)
    for o, t in SAMPLING.items():
        sp.add_argument(f"--{o.replace('_', '-')}", type=t)
    sp.add_argument("--tools", help="comma list of allowed tools ('' for none)")


def _collect(args) -> dict:
    d = {o: getattr(args, o) for o in PROFILE_OPTS + INT_OPTS if getattr(args, o) is not None}
    if args.persona_file:
        d["persona"] = Path(args.persona_file).expanduser().read_text(encoding="utf-8")
    s = {o: getattr(args, o) for o in SAMPLING if getattr(args, o) is not None}
    if s:
        d["sampling"] = s
    if args.tools is not None:
        d["tools"] = [t.strip() for t in args.tools.split(",") if t.strip()]
    return d


def build_parser():
    p = argparse.ArgumentParser(prog="lillith-bot", description="Lillith local bots")
    p.add_argument("--home", help="data dir (default ~/.local/share/lillith)")
    p.add_argument("--json", action="store_true")
    sub = p.add_subparsers(dest="cmd", required=True)

    c = sub.add_parser("create")
    c.add_argument("name")
    _profile_args(c, True)
    sub.add_parser("list")
    s = sub.add_parser("show")
    s.add_argument("bot")
    e = sub.add_parser("edit")
    e.add_argument("bot")
    _profile_args(e, False)
    r = sub.add_parser("rm")
    r.add_argument("bot")
    r.add_argument("--keep-files", action="store_true")

    ch = sub.add_parser("chat", help="chat (interactive if no message)")
    ch.add_argument("bot")
    ch.add_argument("message", nargs="?")
    ch.add_argument("-a", "--attach", action="append", default=[])
    ch.add_argument("--dry-run", action="store_true")
    h = sub.add_parser("history")
    h.add_argument("bot")
    h.add_argument("-n", "--limit", type=int, default=20)
    h.add_argument("--clear", action="store_true")

    m = sub.add_parser("memory")
    msub = m.add_subparsers(dest="mcmd", required=True)
    for n in ("list",):
        x = msub.add_parser(n)
        x.add_argument("bot")
    x = msub.add_parser("add")
    x.add_argument("bot")
    x.add_argument("text")
    x = msub.add_parser("journal")
    x.add_argument("bot")
    x.add_argument("text")
    x.add_argument("--day")
    x = msub.add_parser("rm")
    x.add_argument("bot")
    x.add_argument("kind", choices=["fact", "journal"])
    x.add_argument("id", type=int)
    x = msub.add_parser("search")
    x.add_argument("bot")
    x.add_argument("query")

    a = sub.add_parser("attach")
    asub = a.add_subparsers(dest="acmd", required=True)
    x = asub.add_parser("add")
    x.add_argument("bot")
    x.add_argument("paths", nargs="+")
    x = asub.add_parser("list")
    x.add_argument("bot")
    x = asub.add_parser("rm")
    x.add_argument("bot")
    x.add_argument("ref")

    ex = sub.add_parser("export")
    ex.add_argument("bot")
    ex.add_argument("-o", "--output")
    ex.add_argument("--with-facts", action="store_true")
    ex.add_argument("--no-avatar", action="store_true")
    im = sub.add_parser("import")
    im.add_argument("file")
    im.add_argument("--name")

    ro = sub.add_parser("routine")
    rsub = ro.add_subparsers(dest="rcmd", required=True)
    x = rsub.add_parser("add")
    x.add_argument("bot")
    x.add_argument("name")
    x.add_argument("cron", help="e.g. '0 9 * * *' or @daily")
    x.add_argument("prompt")
    x.add_argument("--no-notify", action="store_true")
    x = rsub.add_parser("list")
    x.add_argument("bot", nargs="?")
    x = rsub.add_parser("rm")
    x.add_argument("id", type=int)
    x = rsub.add_parser("run")
    x.add_argument("id", type=int)
    x = rsub.add_parser("enable")
    x.add_argument("id", type=int)
    x = rsub.add_parser("disable")
    x.add_argument("id", type=int)

    sc = sub.add_parser("scheduler", help="run due routines")
    sc.add_argument("--once", action="store_true", help="single pass (for systemd timer)")
    sc.add_argument("--interval", type=float, default=30.0)
    sc.add_argument("--no-notify", action="store_true")

    sv = sub.add_parser("serve", help="local HTTP API")
    sv.add_argument("--host", default="127.0.0.1")
    sv.add_argument("--port", type=int, default=8765)
    return p


def _ts(t):
    return datetime.fromtimestamp(t).strftime("%Y-%m-%d %H:%M") if t else "-"


def main(argv=None) -> int:
    args = build_parser().parse_args(argv)
    home = Path(args.home).expanduser() if args.home else None
    try:
        if args.cmd == "serve":
            from .api import serve
            serve(home, args.host, args.port)
            return 0
        if args.cmd == "scheduler":
            run_scheduler(home, once=args.once, interval=args.interval, notify=not args.no_notify)
            return 0
        with Lillith(home) as app:
            return _run(app, args)
    except (BotError, AttachmentError, CronError, llm.LLMError, ValueError, OSError) as e:
        print(f"lillith-bot: {e}", file=sys.stderr)
        return 1


def _out(args, obj, text):
    print(json.dumps(obj, indent=2, default=str) if args.json else text)


def _run(app: Lillith, args) -> int:
    B = app.bots
    cmd = args.cmd
    if cmd == "create":
        p = B.create(args.name, **_collect(args))
        _out(args, p.to_dict(), f"created bot {p.name} ({p.title})")
    elif cmd == "list":
        bots = B.list()
        _out(args, [b.to_dict() for b in bots],
             "\n".join(f"{b.name:<20} {b.title:<20} {b.description}" for b in bots) or "(no bots)")
    elif cmd == "show":
        print(json.dumps(B.get(args.bot).to_dict(), indent=2))
    elif cmd == "edit":
        p = B.edit(args.bot, **_collect(args))
        _out(args, p.to_dict(), f"updated {p.name}")
    elif cmd == "rm":
        app.delete_bot(args.bot, keep_files=args.keep_files)
        print(f"deleted {args.bot}")
    elif cmd == "chat":
        if args.dry_run:
            for a in args.attach:
                app.attach(args.bot, a)
            msgs, rep = app.build_messages(B.get(args.bot), args.message or "")
            print(json.dumps({"messages": msgs, "attachments": rep}, indent=2))
            return 0
        if args.message:
            res = app.chat(args.bot, args.message, args.attach)
            _out(args, res, res["reply"])
            return 0
        p = B.get(args.bot)
        for a in args.attach:
            app.attach(p.name, a)
        print(f"Chatting with {p.title or p.name}. /attach PATH, /quit to exit.")
        while True:
            try:
                line = input("you> ").strip()
            except (EOFError, KeyboardInterrupt):
                print()
                break
            if not line:
                continue
            if line in ("/quit", "/exit"):
                break
            if line.startswith("/attach "):
                a = app.attach(p.name, line[8:].strip())
                print(f"attached {a.id} {a.name}")
                continue
            try:
                print(f"{p.title or p.name}> {app.chat(p.name, line)['reply']}")
            except llm.LLMError as e:
                print(f"[error] {e}")
    elif cmd == "history":
        if args.clear:
            B.clear_history(B.get(args.bot).name)
            print("cleared")
            return 0
        h = B.history(B.get(args.bot).name, args.limit)
        _out(args, h, "\n".join(f"[{_ts(m['created'])}] {m['role']}: {m['content']}" for m in h))
    elif cmd == "memory":
        bot = B.get(args.bot).name
        if args.mcmd == "list":
            f, j = B.facts(bot), B.journal(bot)
            _out(args, {"facts": f, "journal": j},
                 "Facts:\n" + "\n".join(f"  #{x['id']} {x['fact']}" for x in f) +
                 "\nJournal:\n" + "\n".join(f"  #{x['id']} {x['day']} {x['entry']}" for x in j))
        elif args.mcmd == "add":
            print(f"fact #{B.add_fact(bot, args.text)}")
        elif args.mcmd == "journal":
            print(f"journal #{B.add_journal(bot, args.text, args.day)}")
        elif args.mcmd == "rm":
            ok = (B.rm_journal if args.kind == "journal" else B.rm_fact)(bot, args.id)
            print("deleted" if ok else "not found")
            return 0 if ok else 1
        elif args.mcmd == "search":
            r = B.search_memory(bot, args.query)
            _out(args, r, "\n".join(f"[{x['type']}] {x['text']}" for x in r) or "(no matches)")
    elif cmd == "attach":
        bot = B.get(args.bot).name
        if args.acmd == "add":
            for pth in args.paths:
                a = app.attach(bot, pth)
                print(f"{a.id}  {a.name}")
        elif args.acmd == "list":
            atts = app.bot_attachments(bot)
            _out(args, [a.to_dict() for a in atts],
                 "\n".join(f"{a.id}  {a.kind:<6} {a.name}" for a in atts) or "(none)")
        elif args.acmd == "rm":
            print("detached" if app.detach(bot, args.ref) else "not attached")
    elif cmd == "export":
        t = app.export_template(args.bot, include_facts=args.with_facts, embed_avatar=not args.no_avatar)
        data = json.dumps(t, indent=2)
        if args.output:
            Path(args.output).expanduser().write_text(data, encoding="utf-8")
            print(f"wrote {args.output}")
        else:
            print(data)
    elif cmd == "import":
        t = json.loads(Path(args.file).expanduser().read_text(encoding="utf-8"))
        p = app.import_template(t, args.name)
        print(f"imported bot {p.name}")
    elif cmd == "routine":
        if args.rcmd == "add":
            r = B.add_routine(B.get(args.bot).name, args.name, args.cron, args.prompt,
                              notify=not args.no_notify)
            print(f"routine #{r['id']} next run {_ts(r['next_run'])}")
        elif args.rcmd == "list":
            rs = B.routines(B.get(args.bot).name if args.bot else None)
            _out(args, rs, "\n".join(
                f"#{r['id']} {r['bot']}/{r['name']} [{r['cron']}] "
                f"{'on' if r['enabled'] else 'off'} next={_ts(r['next_run'])} "
                f"last={_ts(r['last_run'])} {r['last_status'] or ''}" for r in rs) or "(none)")
        elif args.rcmd == "rm":
            print("deleted" if B.rm_routine(args.id) else "not found")
        elif args.rcmd == "run":
            r = app.run_routine(args.id)
            _out(args, r, r["reply"] or r["status"])
        elif args.rcmd in ("enable", "disable"):
            B.routine(args.id)
            B.set_routine_enabled(args.id, args.rcmd == "enable")
            print(f"{args.rcmd}d #{args.id}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
