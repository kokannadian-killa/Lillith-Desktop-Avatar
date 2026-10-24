"""lillith-attach: command-line interface for the attachment store."""
from __future__ import annotations

import argparse
import json
import sys
import time

from .config import Config
from ..lillith_config import config_path, llm_settings, load_toml
from .store import AttachmentStore, AttachmentError
from .chat import build_messages, chat as send_chat
from .. import llm


def _fmt_size(n: int) -> str:
    for unit in ("B", "KB", "MB", "GB"):
        if n < 1024 or unit == "GB":
            return f"{n:.0f}{unit}" if unit == "B" else f"{n:.1f}{unit}"
        n /= 1024
    return str(n)


def _line(a) -> str:
    extra = f" {a.width}x{a.height}" if a.width else ""
    extra += f" {a.pages}p" if a.pages else ""
    when = time.strftime("%Y-%m-%d %H:%M", time.localtime(a.created))
    return f"{a.id}  {_fmt_size(a.size):>8}  {a.kind:<6}{extra:<12} {when}  {a.name}"


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="lillith-attach", description="Lillith local attachments")
    p.add_argument("--home", help="store directory (default ~/.local/share/lillith/attachments)")
    p.add_argument("--config", help="Lillith config.toml (default ~/.config/lillith/config.toml)")
    p.add_argument("--json", action="store_true", help="machine-readable output for the GUI")
    sub = p.add_subparsers(dest="cmd", required=True)

    a = sub.add_parser("add", help="add file(s)")
    a.add_argument("paths", nargs="+")
    a.add_argument("-c", "--conversation")

    l = sub.add_parser("list", help="list attachments")
    l.add_argument("-c", "--conversation")

    s = sub.add_parser("show", help="show metadata")
    s.add_argument("ref")

    r = sub.add_parser("read", help="read extracted text (paged)")
    r.add_argument("ref")
    r.add_argument("-o", "--offset", type=int, default=1, help="1-based line; negative = from end")
    r.add_argument("-n", "--limit", type=int)

    se = sub.add_parser("search", help="search extracted text")
    se.add_argument("query")
    se.add_argument("-c", "--conversation")
    se.add_argument("-e", "--regex", action="store_true")
    se.add_argument("-s", "--case-sensitive", action="store_true")
    se.add_argument("-C", "--context", type=int, default=0)
    se.add_argument("-m", "--max", type=int, default=200)

    rm = sub.add_parser("rm", help="delete attachment(s) everywhere")
    rm.add_argument("refs", nargs="+")

    at = sub.add_parser("attach", help="attach existing attachment to a conversation")
    at.add_argument("conversation")
    at.add_argument("refs", nargs="+")
    dt = sub.add_parser("detach", help="remove from a conversation (keeps the file)")
    dt.add_argument("conversation")
    dt.add_argument("refs", nargs="+")

    ex = sub.add_parser("export", help="copy the original file out")
    ex.add_argument("ref")
    ex.add_argument("dest")
    ex.add_argument("--with-text", action="store_true")

    sub.add_parser("convs", help="list conversations")
    sub.add_parser("verify", help="check store integrity")
    sub.add_parser("config", help="print effective config")

    ch = sub.add_parser("chat", help="ask Lillith with attachments")
    ch.add_argument("message")
    ch.add_argument("-a", "--attach", action="append", default=[],
                    help="file path or attachment id (repeatable)")
    ch.add_argument("-c", "--conversation")
    ch.add_argument("-b", "--budget", type=int, help="attachment token budget")
    ch.add_argument("--vision", action="store_true", help="send images (llama-server --mmproj)")
    ch.add_argument("--dry-run", action="store_true", help="print the messages, don't call the model")
    return p


def main(argv=None) -> int:
    args = build_parser().parse_args(argv)
    cfg = Config.load(args.config, home=args.home)
    out = sys.stdout
    try:
        with AttachmentStore(cfg) as st:
            return _run(st, args, out)
    except (AttachmentError, llm.LLMError, ValueError) as e:
        print(f"lillith-attach: {e}", file=sys.stderr)
        return 1


def _emit(args, obj, text):
    if args.json:
        print(json.dumps(obj, indent=2, default=str))
    else:
        print(text)


def _run(st: AttachmentStore, args, out) -> int:
    from pathlib import Path
    cmd = args.cmd
    if cmd == "add":
        atts = [st.add(p, conversation=args.conversation) for p in args.paths]
        _emit(args, [a.to_dict() for a in atts], "\n".join(_line(a) for a in atts))
    elif cmd == "list":
        atts = st.list(args.conversation)
        _emit(args, [a.to_dict() for a in atts],
              "\n".join(_line(a) for a in atts) or "(no attachments)")
    elif cmd == "show":
        a = st.resolve(args.ref)
        d = a.to_dict()
        d["conversations"] = st.db.conversations_for(a.id)
        print(json.dumps(d, indent=2))
    elif cmd == "read":
        r = st.read(args.ref, args.offset, args.limit)
        if args.json:
            print(json.dumps(r, indent=2))
        else:
            if not r["text_available"]:
                print(f"({r['name']}: no extracted text)")
            for n, line in r["lines"]:
                print(f"{n:>6}\t{line}")
            if r["has_more"]:
                print(f"… {r['total_lines'] - r['end']} more lines; next: --offset {r['next_offset']}",
                      file=sys.stderr)
    elif cmd == "search":
        res = st.search(args.query, args.conversation, regex=args.regex,
                        ignore_case=not args.case_sensitive, max_results=args.max,
                        context=args.context)
        _emit(args, res, "\n".join(f"{r['name']}:{r['line']}: {r['text']}" for r in res)
              or "(no matches)")
        return 0 if res else 1
    elif cmd == "rm":
        for ref in args.refs:
            a = st.delete(ref)
            print(f"deleted {a.id} {a.name}")
    elif cmd == "attach":
        for ref in args.refs:
            a = st.attach(args.conversation, ref)
            print(f"attached {a.id} to {args.conversation}")
    elif cmd == "detach":
        for ref in args.refs:
            ok = st.detach(args.conversation, ref)
            print(("detached " if ok else "not attached: ") + ref)
    elif cmd == "export":
        print(st.export(args.ref, args.dest, args.with_text))
    elif cmd == "convs":
        cs = st.db.list_conversations()
        _emit(args, cs, "\n".join(f"{c['id']}  ({c['n']} attachments)" for c in cs) or "(none)")
    elif cmd == "verify":
        probs = st.verify()
        print("\n".join(probs) or "ok")
        return 1 if probs else 0
    elif cmd == "config":
        ls = llm_settings(load_toml(args.config))
        print(json.dumps({**st.config.to_dict(), "config_toml": str(args.config or config_path()),
                          "llm": {"endpoint": ls.endpoint, "model": ls.model, "source": ls.source}},
                         indent=2))
    elif cmd == "chat":
        refs = []
        for a in args.attach:
            refs.append(st.add(a).id if Path(a).expanduser().is_file() else st.resolve(a).id)
        ls = llm_settings(load_toml(args.config))
        kw = {"token_budget": args.budget, "vision": True if args.vision else None,
              "system_prompt": ls.persona}
        if args.dry_run:
            hist = st.db.history(args.conversation) if args.conversation else []
            msgs, rep = build_messages(st, args.message, refs or None, args.conversation,
                                               hist, **kw)
            print(json.dumps({"messages": msgs, "report": rep}, indent=2))
            return 0
        res = send_chat(st, args.message, refs or None, args.conversation, settings=ls, **kw)
        if args.json:
            print(json.dumps({"reply": res["reply"], "report": res["report"]}, indent=2))
        else:
            for r in res["report"]:
                if r["truncated"]:
                    print(f"[note] {r['name']} truncated to {r['included_chars']}/{r['chars']} chars",
                          file=sys.stderr)
            print(res["reply"])
    return 0


if __name__ == "__main__":
    sys.exit(main())
