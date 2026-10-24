"""Safe, allowlisted tool layer for bots.

Tools only touch the bot's OWN attachments and memory. No filesystem paths,
no shell, no network. A bot can use a tool only if it is in BOTH the global
registry below and the bot profile's ``tools`` list.

Protocol (works with small text-only models): the model writes one line
    <tool_call>{"name": "search_memory", "arguments": {"query": "birthday"}}</tool_call>
and the engine replies with a fenced <tool_result> block. OpenAI-style
``tool_calls`` in the response are accepted too.

POLICY: this set is closed. Do not add tools that write outside the bot's own
data dir, run commands, launch programs or use the network. Such actions must
go through Lillith's Soul Contract (`lillith contract-card`) and the shared
audit.log; the only entry point for that is ``contract.request_privileged_action``,
which currently always refuses (see contract.py). A model that asks for any
privileged tool name gets that refusal back.
"""
from __future__ import annotations

import json
import re
from dataclasses import dataclass

from ..attachments.store import AttachmentStore, NotFound

MAX_RESULT_CHARS = 3000
TOOL_CALL_RE = re.compile(r"<tool_call>\s*(\{.*?\})\s*</tool_call>", re.S)


@dataclass
class ToolContext:
    bot: str
    bots: "object"              # BotStore
    attachments: AttachmentStore

    @property
    def conversation(self) -> str:
        return f"bot:{self.bot}"


def _own_attachment(ctx: ToolContext, ref: str):
    att = ctx.attachments.resolve(ref)
    if att.id not in ctx.attachments.db.attachments_for(ctx.conversation):
        raise NotFound(f"{ref!r} is not attached to this bot")
    return att


def t_list_attachments(ctx, args):
    atts = ctx.attachments.list(ctx.conversation)
    return json.dumps([{"id": a.id, "name": a.name, "kind": a.kind, "lines": a.text_lines}
                       for a in atts]) or "[]"


def t_read_attachment(ctx, args):
    att = _own_attachment(ctx, str(args.get("id") or args.get("ref") or args.get("name") or ""))
    off = int(args.get("offset", 1))
    lim = max(1, min(int(args.get("limit", 60)), 200))
    r = ctx.attachments.read(att, off, lim)
    body = "\n".join(f"{n}: {l}" for n, l in r["lines"])
    tail = f"\n[more: offset={r['next_offset']}]" if r["has_more"] else ""
    return f"{att.name} lines {r['start']}-{r['end']} of {r['total_lines']}\n{body}{tail}"


def t_search_attachments(ctx, args):
    q = str(args.get("query", "")).strip()
    if not q:
        return "error: query required"
    res = ctx.attachments.search(q, ctx.conversation, max_results=20)
    return "\n".join(f"{r['name']}:{r['line']}: {r['text']}" for r in res) or "no matches"


def t_search_memory(ctx, args):
    q = str(args.get("query", "")).strip()
    res = ctx.bots.search_memory(ctx.bot, q) if q else []
    return "\n".join(f"[{r['type']}{' ' + r['day'] if r.get('day') else ''}] {r['text']}"
                     for r in res) or "no matches"


def t_write_memory(ctx, args):
    kind = args.get("type", "fact")
    text = str(args.get("text") or args.get("fact") or args.get("entry") or "").strip()
    if not text:
        return "error: text required"
    if kind == "journal":
        i = ctx.bots.add_journal(ctx.bot, text)
        return f"journal entry {i} saved"
    i = ctx.bots.add_fact(ctx.bot, text)
    return f"fact {i} saved"


TOOLS = {
    "list_attachments": (t_list_attachments, "list files attached to you", {}),
    "read_attachment": (t_read_attachment, "read lines of an attached file",
                        {"id": "attachment id or name", "offset": "1-based line", "limit": "lines (<=200)"}),
    "search_attachments": (t_search_attachments, "substring search in attached files",
                           {"query": "text to find"}),
    "search_memory": (t_search_memory, "search your saved facts and journal", {"query": "text"}),
    "write_memory": (t_write_memory, "save a durable fact or a journal entry",
                     {"text": "what to remember", "type": "fact|journal"}),
}


def tool_instructions(allowed: list) -> str:
    allowed = [t for t in allowed if t in TOOLS]
    if not allowed:
        return ""
    lines = ["You can use tools. To call one, reply with ONLY a single line:",
             '<tool_call>{"name": "TOOL", "arguments": {...}}</tool_call>',
             "then wait for the <tool_result>. Available tools:"]
    for t in allowed:
        _, desc, params = TOOLS[t]
        ps = ", ".join(f"{k}: {v}" for k, v in params.items())
        lines.append(f"- {t}({ps}): {desc}")
    return "\n".join(lines)


def parse_tool_calls(resp_message: dict) -> list:
    """Return [(name, args_dict)] from an assistant message (text or OpenAI tool_calls)."""
    calls = []
    for tc in resp_message.get("tool_calls") or []:
        fn = tc.get("function", {})
        try:
            args = json.loads(fn.get("arguments") or "{}")
        except json.JSONDecodeError:
            args = {}
        calls.append((fn.get("name", ""), args if isinstance(args, dict) else {}))
    text = resp_message.get("content") or ""
    for m in TOOL_CALL_RE.finditer(text):
        try:
            obj = json.loads(m.group(1))
        except json.JSONDecodeError:
            continue
        args = obj.get("arguments") or obj.get("args") or {}
        calls.append((str(obj.get("name", "")), args if isinstance(args, dict) else {}))
    return calls


PRIVILEGED_TOOL_NAMES = {"write_file", "run_command", "shell", "exec", "launch_program",
                         "open_url", "download", "http_get", "fetch"}


def run_tool(ctx: ToolContext, allowed: list, name: str, args: dict) -> str:
    if name in PRIVILEGED_TOOL_NAMES:
        from .contract import request_privileged_action, ContractRequired
        try:
            request_privileged_action(name, json.dumps(args)[:200], ctx.bot)
        except ContractRequired as e:
            return f"error: refused: {e}"
    if name not in TOOLS or name not in allowed:
        return f"error: tool {name!r} is not allowed"
    try:
        out = TOOLS[name][0](ctx, args)
    except Exception as e:  # tool errors go back to the model, never crash the chat
        out = f"error: {e}"
    if len(out) > MAX_RESULT_CHARS:
        out = out[:MAX_RESULT_CHARS] + "\n[truncated]"
    return out
