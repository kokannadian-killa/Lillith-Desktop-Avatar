# lillith-py: local attachments and bots for Lillith

Python sidecar for the Lillith desktop AI (DemonOS - Phantomcore), living at
`tools/lillith-py/` next to `tools/lillith-voice/`. It is the **backend for the Rust GUI
"Bots" tab**: a loopback HTTP API, task workers, per-bot memory and attachments.

Everything runs on your machine with Python 3.11+ stdlib (`tomllib`), SQLite, and
optional poppler, tesseract and Pillow. The only HTTP traffic goes to the llama-server
named in Lillith's `config.toml` (loopback only) and to the sidecar API on `127.0.0.1`.
Non-loopback URLs are refused, proxy env vars are ignored, and nothing is downloaded or
sent anywhere at runtime. The test suite fails if anything opens a non-loopback socket.

Package: `lillith_py` (`lillith_py.attachments`, `lillith_py.bots`). CLIs: `lillith-attach`,
`lillith-bot`. This folder is excluded from the release zip (`.gitattributes` export-ignore).

## Install (Garuda / Arch, fish)

```fish
cd tools/lillith-py
./install.sh            # shows the pacman command and asks before running it
fish_add_path ~/.local/bin
```
`./install.sh --no-deps` skips pacman; `--yes` skips the prompt. The installer copies the
code to `~/.local/lib/lillith-py`, links the two CLIs into `~/.local/bin`, and copies the
systemd user units. **It never enables or starts them.** It prints these for you:
```fish
systemctl --user daemon-reload
systemctl --user enable --now lillith-py-api.service          # API for the Bots tab
systemctl --user enable --now lillith-py-scheduler.timer      # routines, once a minute
# or instead of the timer: lillith-py-scheduler-daemon.service
```
Uninstall: `rm -r ~/.local/lib/lillith-py ~/.local/bin/lillith-{attach,bot}
~/.config/systemd/user/lillith-py-*`. Your data stays in `~/.local/share/lillith`.

## Configuration: Lillith's `config.toml`

lillith-py has no config file of its own. It **reads** (never writes)
`$XDG_CONFIG_HOME/lillith/config.toml`, default `~/.config/lillith/config.toml` (the file
the Rust app uses; `LILLITH_CONFIG` overrides the path). Rust ignores the extra tables.

```toml
[llm]                       # preferred; if absent, [chat] endpoint/model/system_prompt/api_key are used
endpoint = "http://127.0.0.1:8080"   # loopback only, anything else is refused
model    = "local"
persona  = "You are Lillith ..."     # default persona when a bot has none

[sidecar]
port          = 8765        # API port (loopback only)
workers       = 1           # concurrent tasks, max 2
stall_minutes = 5           # no tokens/progress for this long -> task is "stalled"

[sidecar.attachments]       # optional knobs
token_budget = 2048
ocr = true
vision = false              # needs a multimodal model + llama-server --mmproj
```
Order: `[llm]` > `[chat]` > built-in fallback `http://127.0.0.1:8080` (only when no config
exists). Per-bot persona, sampling and (optionally) endpoint/model live in the bot
profile; empty values inherit from the config. Bundled persona templates are in
`lillith_py/templates/` (`assistant`, `researcher`, `writer`, `planner`).

## Data layout (`~/.local/share/lillith/`, or `$XDG_DATA_HOME/lillith`)

```
api.token                     API token, created on first start, mode 0600
attachments/                  content-addressed attachment store (below)
bots/registry.sqlite          bot list, routines, task queue and state
bots/<bot-id>/profile.json    profile mirror
bots/<bot-id>/bot.sqlite      this bot's chat history, facts and journal
bots/<bot-id>/jobs/<task>.jsonl   append-only job log, one JSON event per line
bots/<bot-id>/results/<task>.md   task result
```
Job log events: `{"id", "bot", "task", "event", "status", "ts", "created", "started",
"finished", "result_path", ...}` (`event` is queued/started/reassigned/stalled/done/failed/
cancelled; times are Unix seconds). Bot memory is separate from Lillith's own
`memory.db` (owned by the Rust `lillith-memory` crate), which lillith-py never opens.

## Attachments: `lillith-attach`

```fish
lillith-attach add ~/Documents/manual.pdf ~/Pictures/shot.png -c main
lillith-attach list                    # or: list -c main / --json list
lillith-attach show att_1a2b3c4d5e6f7a8b
lillith-attach read manual.pdf --offset 200 --limit 100    # negative offset = from the end
lillith-attach search "llama" -C 1     # -e regex, -s case-sensitive, -c conversation
lillith-attach chat "summarize this" -a ~/notes.md -c main  # --dry-run shows the messages
lillith-attach chat "what is in the picture?" -a shot.png --vision   # needs llama-server --mmproj
lillith-attach export manual.pdf ~/Desktop/ --with-text
lillith-attach detach main manual.pdf ; lillith-attach rm manual.pdf
lillith-attach verify
```

Storage (`~/.local/share/lillith/attachments/`):
`blobs/<aa>/<sha256>.<ext>` (read-only, deduped), `meta/<sha256>.json` (id, name, mime,
size, created, sha256, text_path, thumbnail_path, dimensions, pages, notes), `text/`,
`thumbs/`, and `lillith.sqlite` (conversation attachment lists and history). Every record
has a `uri` and `thumbnail_uri` (`file://`) that a GUI can display.

Extraction: text, code, JSON and CSV files are read directly, capped by `max_text_bytes`.
PDFs go through `pdftotext -layout`. For images, the store records dimensions (Pillow or a
stdlib header parser), makes a 256px PNG thumbnail (Pillow or ImageMagick), and runs
optional OCR (the `tesseract` CLI). pytesseract is no longer used, because it starts
tesseract itself outside the launch wrapper. Every external tool goes through the
launch wrapper and leaves one audit.log line (see "Program launches" below).

Chat injection: attachments go in the user turn as
`<attachment id=".." name=".." mime=".." size="..">…</attachment>`, and the system prompt
tells the model to treat their contents as untrusted data. If a file contains an
attachment tag, the tag is escaped so it can't close or fake a block. The `token_budget`
is split fairly: small files go in whole, large ones are cut at a line boundary with a
`[TRUNCATED … offset=N]` note.

Env overrides: `LILLITH_ATTACH_HOME` (store dir), `LILLITH_HOME` (whole data dir),
`LILLITH_CONFIG` (config.toml path).

Python API for the right-click GUI:
```python
from lillith_py.attachments import AttachmentStore, chat
st = AttachmentStore()
atts = st.add_uri_list(drop_event_text, conversation="main")   # KDE/Qt text/uri-list
st.add_bytes(clipboard_png, "paste.png", conversation="main")
st.read(atts[0].id, offset=1, limit=100); st.search("foo")
print(chat(st, "summarize", conversation="main")["reply"])
```

## Bots: `lillith-bot`

```fish
lillith-bot templates                                   # bundled persona templates
lillith-bot create Scout --template researcher          # or --persona-file / --persona
lillith-bot create Lillith --persona-file ~/lillith/persona.txt --avatar ~/lillith/avatar.png --role companion
lillith-bot list                                        # id, status (idle/working/stalled), role, title
lillith-bot edit scout --memory-budget 600 --tools search_memory,write_memory,read_attachment
lillith-bot chat scout "hey"                            # no message: interactive (/attach PATH, /quit)
lillith-bot task submit any "summarise ~/todo.md"       # queued for the API server's workers
lillith-bot task run scout "summarise" -a ~/todo.md     # run now in this process
lillith-bot task list ; lillith-bot task show t_abc ; lillith-bot task log t_abc
lillith-bot task stop t_abc ; lillith-bot task retry t_abc ; lillith-bot task reassign t_abc writer
lillith-bot stop-all                                    # cancel every running and queued task
lillith-bot memory add scout "Ko uses fish shell" ; lillith-bot memory journal scout "note"
lillith-bot memory list scout ; lillith-bot memory search scout fish ; lillith-bot memory rm scout fact 1
lillith-bot attach add scout ~/notes.pdf ; lillith-bot attach list scout ; lillith-bot attach rm scout notes.pdf
lillith-bot routine add scout morning "0 9 * * *" "Give Ko a short good-morning note"
lillith-bot routine list ; lillith-bot routine run 1 ; lillith-bot routine disable 1 ; lillith-bot routine rm 1
lillith-bot export scout -o scout.template.json ; lillith-bot import scout.template.json --name scout-2
lillith-bot scheduler --once                            # what the systemd timer runs
lillith-bot serve                                       # API on 127.0.0.1:<[sidecar] port>
lillith-bot rm scout-2                                  # deletes bots/scout-2/ only
```

- **Prompt:** persona (profile, else config), name, date, a `<memory>` block (facts, then
  newest journal entries, within `memory_budget`), tool instructions, the bot's
  attachments (within `attachment_budget`, fenced as untrusted), and recent history.
- **Tasks:** `queued -> running -> done | failed | cancelled | stalled`. Workers: 1 by
  default, 2 max; one running task per bot. `any` picks the least busy bot. A task with no
  streamed token/progress for `stall_minutes` is aborted and marked `stalled`. `retry`
  makes a new task (`retry_of` = old id). `reassign` moves a queued task in place; a
  finished/stalled task becomes a new task on the other bot; a running one is stopped and
  re-queued there. Routines run through the same task runner.
- **Safeword:** `red` (the whole message) or `OOC: stop` anywhere, in any bot chat (CLI,
  API, interactive), cancels every running and queued task, calls no model and stores
  nothing (same rule as `crates/lillith-memory/src/safeword.rs`). `POST /stop-all` and
  `lillith-bot stop-all` do the same.
- **Tools (closed set):** `list_attachments`, `read_attachment`, `search_attachments`,
  `search_memory`, `write_memory` (to the bot's own store). No file paths, shell, program
  launching or network. Anything privileged must go through Lillith's Soul Contract
  (`lillith contract-card`) and the shared `audit.log`; the hook for that is
  `lillith_py/bots/contract.py::request_privileged_action`, which **always refuses** until
  that bridge exists. Tool calls: `<tool_call>{"name":…,"arguments":{…}}</tool_call>` or
  OpenAI `tool_calls`; at most 3 rounds per turn.
- **Routines:** 5-field cron plus `@hourly/@daily/@weekly/@monthly/@yearly`; results go to
  the bot's history (`routine:<name>`) and the job log, plus a `notify-send` popup when
  it's installed (audited like every launch).

## Program launches (`lillith_py/launch.py`) and `audit.log`
Every external program lillith-py starts goes through one wrapper,
`lillith_py.launch.run`. The programs are pdftotext, pdfinfo, ImageMagick
(`magick`/`convert`), tesseract and notify-send. The project lead has exempted them
from a Soul Contract card (review X5), so they run exactly as before, but **each
launch writes exactly one append-only line** to Lillith's shared audit log.

- **Path:** the Rust `audit_log_path()` (`crates/lillith-core/src/config.rs`), i.e.
  `$XDG_DATA_HOME/lillith/audit.log`, else `~/.local/share/lillith/audit.log`.
  `$LILLITH_HOME` is ignored for this file so both sides share it.
- **Format:** the Rust `SoulContract::audit` line (`crates/lillith-core/src/soul_contract.rs`),
  `<rfc3339 UTC>\t<DECISION>\t<kind>\t<what>`, with kind `lillith-py:launch`:
  ```
  2026-10-24T10:14:03Z	EXEMPT	lillith-py:launch	pdftotext $ pdftotext -layout -enc UTF-8 ~/.local/share/lillith/attachments/blobs/ab/<sha>.pdf - -> exit=0 [attachments: PDF text]
  ```
  The outcome after `->` is `exit=N`, `timeout` or `error: …`. Masking: `$HOME` becomes
  `~`. Values of `--api-key`/`--token`/`--password`-style flags, `key=value` secrets,
  `Bearer …` and token-looking strings become `<redacted>`. Other arguments over 80 chars
  (a notification body, for example) become `<N chars>`.
- **Pillow** runs in-process and is not a program launch, so it writes no line. It is
  restricted to PNG/JPEG/GIF/WEBP/BMP/TIFF, so it can never call Ghostscript (EPS).
- **No bypass:** no other module may import `subprocess` or call `os.system`/`exec*`/
  `spawn*` (a source-scan test enforces this). Importing `lillith_py` installs a
  runtime guard: any `subprocess.Popen` that didn't come through the wrapper is refused
  and audited as `REFUSED … bypassed the lillith_py.launch wrapper`.

## HTTP API (for the Rust "Bots" tab)

Base URL `http://127.0.0.1:8765` (port = `[sidecar] port`). Loopback bind only; requests
with a non-loopback `Host` or `Origin` get **403**.

**Auth (mandatory):** every request needs
`Authorization: Bearer <contents of ~/.local/share/lillith/api.token>` (trim the newline).
The file is generated on first start with mode 0600. Missing/wrong token: **401**.
Bodies are JSON (`Content-Type: application/json`, max 2 MB). Errors are
`{"error": "message"}` with 400 (bad input), 401, 403, 404 (unknown bot/task/route) or
502 (model unreachable). Times are Unix seconds (float).

### Bots tab endpoints

| Method & path | Body | Response |
|---|---|---|
| `GET /health` | - | `{"ok":true,"workers":1,"stall_minutes":5.0,"llm":{"endpoint","model","source"}}` |
| `GET /bots` | - | `[BotSummary]` |
| `POST /bots` | `{"name","template"?,"role"?,"persona"?,"title"?,"description"?,...}` | 201 `Profile` |
| `GET /bots/{id}` | - | `Profile` + `BotStatus` fields |
| `POST /tasks` | `{"bot":"<id>"\|"any","task":"text","attachments"?:["/path"\|"att_id"]}` | 201 `Task` |
| `GET /tasks?bot=&status=&limit=100` | - | `[Task]`, newest first |
| `GET /tasks/{task_id}` | - | `Task` (progress + result) |
| `GET /tasks/{task_id}/log` | - | `[JobEvent]` (the JSONL job log) |
| `POST /tasks/{task_id}/stop` | - | `Task` (`cancel_requested:true`; status becomes `cancelled`) |
| `POST /tasks/{task_id}/retry` | - | 201 new `Task` (`retry_of` set) |
| `POST /tasks/{task_id}/reassign` | `{"bot":"<id>"\|"any"}` | `Task` (same id if it was queued, else a new one) |
| `POST /stop-all` | - | `{"stopped": n}` |
| `GET /templates` | - | `{"researcher": {"format":"lillith-bot-template","profile":{...}}, ...}` |

```jsonc
// BotSummary
{"id":"scout","name":"Scout","role":"researcher","description":"...","avatar":"",
 "status":"idle|working|stalled","current_task":"t_1a2b3c4d5e6f"|null,"queued":0,
 "last_activity":1792833903.05|null}
// Task
{"id":"t_1a2b3c4d5e6f","bot":"scout","requested_bot":"any","task":"find the moon",
 "attachments":[],"source":"api","status":"queued|running|done|failed|cancelled|stalled",
 "cancel_requested":false,"tokens":42,"error":null,
 "created":1792833903.0,"started":1792833903.1,"finished":1792833905.2,"last_progress":1792833905.1,
 "result":"text (done only)"|null,"result_path":"/home/ko/.local/share/lillith/bots/scout/results/t_1a2b3c4d5e6f.md"|null,
 "retry_of":null}
```
Poll `GET /tasks/{id}` (e.g. every 500 ms) for progress: `tokens` and `last_progress`
grow while the model streams. A bot shows `stalled` while its running task has had no
progress for `stall_minutes`, or when its latest task ended stalled.

### Other endpoints
`PATCH|DELETE /bots/{id}`; `POST /bots/{id}/chat {"message","attachments"?}` ->
`{"bot","reply","tools":[...],"attachments":[...],"safeword":bool,"stopped"?:n}`;
`GET|DELETE /bots/{id}/history?limit=50`; `GET|POST /bots/{id}/memory`
(`{"text","type":"fact"|"journal","day"?}`); `GET /bots/{id}/memory/search?q=`;
`DELETE /bots/{id}/memory/{fact|journal}/{n}`; `GET|POST /bots/{id}/attachments`
(`{"path"}` or `{"uri_list"}`); `GET /bots/{id}/attachments/{att}/read?offset=&limit=`;
`DELETE /bots/{id}/attachments/{att}`; `GET|POST /bots/{id}/routines`
(`{"name","cron","prompt","notify"?}`); `DELETE /routines/{n}`; `POST /routines/{n}/run`;
`GET /bots/{id}/export?facts=1`; `POST /templates/import {"template","name"?}`.

```fish
set T (string trim < ~/.local/share/lillith/api.token)
curl -s -H "Authorization: Bearer $T" localhost:8765/bots
curl -s -H "Authorization: Bearer $T" -H 'Content-Type: application/json' \
     -d '{"bot":"any","task":"summarise my notes"}' localhost:8765/tasks
curl -s -X POST -H "Authorization: Bearer $T" localhost:8765/stop-all
```

## systemd --user units (`systemd/`, never auto-enabled)
- `lillith-py-api.service`: the API (`lillith-bot serve`).
- `lillith-py-scheduler.timer` + `lillith-py-scheduler.service`: routines, one pass a minute.
- `lillith-py-scheduler-daemon.service`: a loop instead of the timer (enable one, not both).

All units set `IPAddressDeny=any` / `IPAddressAllow=localhost`.

## Tests
```fish
python -m pytest -q             # offline; tests/mock_llama.py stands in for llama-server
python tests/mock_llama.py --port 8080   # fake llama-server for manual GUI testing
```
`tests/netguard.py` patches `socket.connect`/`create_connection`/`getaddrinfo` for the whole
run; any non-loopback attempt fails the suite. `tests/procguard.py` patches
`subprocess.Popen` and `os.system`/`posix_spawn`/`exec*`/`spawn*`/`fork`; any launch that
skips `lillith_py.launch.run` fails the suite as well. CI: `.github/workflows/lillith-py.yml`
(runs on changes under `tools/lillith-py/**`).

## Limitations
- The 3B Dolphin model is text-only. Image understanding needs a multimodal GGUF plus
  `--mmproj` on llama-server and `vision = true` under `[sidecar.attachments]`. Without that, the model only gets image
  metadata and any OCR text.
- Token counts are estimated at about 4 characters per token, not measured with the
  model's tokenizer. Leave some headroom in the context size.
- Scanned PDFs have no text layer. OCR runs on images only, not on PDF pages.
- Small models follow the tool-call format inconsistently. Bad calls are ignored safely.
- Search is a linear substring or regex scan, which is fine for thousands of files. There
  is no semantic or vector search.
- Deleting an attachment removes it from every conversation, because identical files are
  stored once. Use `detach` to remove it from one conversation only.
