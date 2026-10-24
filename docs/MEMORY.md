# Lillith's learning memory (`lillith-memory`)

Status: design + first implementation on `feat/memory-graph`.

## Goals

- Remember what Ko tells her (entities, preferences, habits), what happens on
  the desktop (app usage, crashes) and what went wrong and how it was fixed.
- Inject a small, relevant slice of that into each chat turn for the local
  3B GGUF model on `llama-server` (127.0.0.1:8080), within ~300 tokens.
- Anticipate Ko's next need from cheap pattern mining (time of day, weekday,
  app usage, repeated requests).
- Stay tiny on an i5-7200U that is also running `llama-server`: sync API, one
  SQLite file, no embedding model, no async runtime, no network code.

## Where it fits

| Existing piece | Role for memory |
| --- | --- |
| `lillith-core::config` | New `[memory]` table (`MemoryConfig`), **`enabled = true` by default** (Ko's decision, 2026-10-23). The default is one line in `MemoryConfig::default()`; set it to `false` to make memory opt-in. |
| `lillith-core::access::kill_switch::is_safeword` | Canonical safeword ("red" / "OOC: stop"). `lillith-memory::is_safeword` wraps it and is *stricter* for storage: any message containing the standalone word "red" or "OOC stop" anywhere is never stored. |
| `lillith-core::event::Event` | Desktop/sensor events (`window.opened.<app>`, `app.crash`, ...). `Brain::observe_event` turns them into app-usage / error memories. These come from `lillith-sense` (monitor, KWin bridge) through the bridge below. |
| `lillith-mind::chat::ChatSession` | Chat path. `with_memory(&MemoryConfig)` attaches a `Brain`; the system prompt gets a `[Memory]` block; replies feed `observe_interaction`; "forget ..." commands are answered locally. |
| `lillith-mind::memory` | Legacy profile store (`lillith_memory.db`, Python schema). Left untouched; the graph lives in a separate `memory.db`. |

## Data model (SQLite, WAL, `PRAGMA user_version` migrations)

- `nodes(id, kind, key, label, detail, strength, access_count, created_at,
  last_seen)`; `kind` in `entity | event | preference | habit | error`,
  unique on `(kind, key)` so re-observing reinforces instead of duplicating.
- `edges(src, dst, relation, strength, access_count, created_at, last_seen)`,
  unique on `(src, dst, relation)`, `ON DELETE CASCADE`.
- `nodes_fts` (FTS5, external content) over `label` + `detail`.
- `events(id, ts, kind, subject, hour, weekday)` – a bounded ring (oldest
  rows pruned) used by the habit miner.
- `errors(signature, context, fix, outcome, count, first_seen, last_seen)` –
  signature = stable FNV-1a hash of the normalised error text (digits,
  hex, paths squashed), so the same crash with a different PID matches.

## Scoring

- Strength decays exponentially: `s(t) = s0 * 0.5^(dt / half_life)`
  (default half-life 14 days). Decay is applied lazily at read time and
  persisted by `Brain::decay()` (run from `observe_interaction` at most
  once an hour). Nodes below `prune_below` (default 0.05) are deleted, and the
  graph is capped at `max_nodes`.
- Reinforcement on reuse: `s = min(1, s + rate * (1 - s))`, `access_count += 1`,
  `last_seen = now`.
- Retrieval score = `bm25 relevance (FTS5, normalised) * 0.55
  + decayed strength * 0.25 + recency (half-life) * 0.15
  + kind/context boost`, then one hop of graph neighbours at a discount
  (0.5 * edge strength). Top-k (default 6) formatted as bullet lines and cut to
  the token budget (~4 chars/token, default 300 tokens).

## Anticipation

Counts over the `events` ring: requests / app launches bucketed by
(hour-of-day bucket of 3h, weekday class: weekday/weekend). The prediction for
"now" ranks subjects by `count(bucket match) / total(bucket)` with Laplace
smoothing, multiplied by a small-sample damping `n / (n + 3)`. O(events), no ML.

## Desktop wiring (`lillith_memory::bridge`, `apps/lillith/src/memory_wiring.rs`)

**Event bridge.** The overlay's one event feed (system monitor, KWin
bridge, chat bubble) goes through a tee: every event is forwarded unchanged
and also offered to `MemoryBridge::observe`, which never blocks the sender.
A pure `EventFilter` runs in the caller, then a `try_send` on a bounded
queue (64; a full queue drops the event). One dedicated thread
(`lillith-memory`) owns its own SQLite connection (`Brain` is not `Sync`)
and calls `Brain::observe_event`.

| Incoming | Stored as | Debounce |
| --- | --- | --- |
| `window.focused` (KWin `WindowActivated`, detail = window class) | `window.opened.app` + app id only (`org.kde.konsole` → `konsole`, ≤32 chars `[a-z0-9-]`) | same app again within 10 min dropped |
| `app.crash`, `*.crash`, `*.error` (not `chat.*`) | the kind plus the first word of the detail as an app id (`app.crash: steam`) | same text within 60 s dropped |
| `window.private`, private windows, `window.opened` (carries the caption), notifications, anything else | nothing | — |
| anything carrying the safeword | nothing | — |

At most `events_per_minute` (default 20) admitted events per 60 s window.
`chat.*` events are never stored; they only mark "Ko is chatting" (and
`chat.safeword` marks the safeword). With `enabled = false` there is no
thread, no queue and no file; the tee is the plain feed.

**Proactive lines.** Every `proactive_interval_min` (default 12) the thread
calls `Brain::predict` and a `Proactive` gate picks the first prediction
with `confidence ≥ proactive_min_confidence` (default 0.5). It stays quiet:

- for 30 min after the safeword (the kill-switch epoch changed, or
  `chat.safeword` came through the feed);
- for 5 min after any chat activity, and while `set_busy_for` is active
  (no fullscreen sensor exists yet; the overlay also drops the line while
  the monitor is paused, and an asleep Lillith ignores low-priority rules);
- for an app used in the last 30 min (she does not suggest what Ko is
  already doing);
- per subject for `proactive_cooldown_min` (default 180) and beyond
  `proactive_max_per_hour` (default 2).

The line ("Mm, about now you usually open steam. Want it, darling?") goes
out as `memory.suggest` with the line as detail; the bundled rule shows it
in the normal speech bubble (`{summary}`, pose/clip `calendar`, priority 30,
cooldown 10 min). `proactive = false` turns only this off; learning stays.

**Error recovery (`lillith_memory::recovery`).** A failure is recorded with
a *stable* text, then the built-in `SafeFix` list is ordered by error
memory (the fix that worked first, untried next, failed ones last), tried,
and each outcome is recorded with `record_fix`. Only these three fixes
exist; a stored fix string that is not one of them is never applied:

| Failure (signature text) | Fixes | Where |
| --- | --- | --- |
| `llm: server unreachable` (nothing streamed yet) | `retry_backoff` (0.4 s, 1.2 s; same loopback server, no card), then `restart_llama_server` **only through the Soul Contract card** (see below) | `ChatSession::send` |
| `llm: server loading` (503) | `retry_backoff` | `ChatSession::send` |
| `voice: engine error (<engine>)` | `retry_backoff` (spoken lines rest 30 s, then worked when a line is spoken without a new error), `text_only` (speakers off for this run, never saved) | overlay voice sink |
| `llama-server: spawn failed`, `setup: ...` | none (recorded, recurrences counted) | `ServerHandle::spawn_approved`, `lillith setup` |
| sensor crashes (`app.crash`, `*.error`) | none (recorded through the bridge) | feed |

When nothing worked the chat answers with the usual in-character line for
the *first* error. Recovery never contacts anything new: retries and the
restart use the same loopback endpoint. With memory off the same fixes run
in their default order and nothing is stored.

**Restarting llama-server is a program launch: Soul Contract card only.**
Recovery never spawns anything. When `[chat.server]` is configured, nothing
answers and the plain retries failed, the session files one
`start_model_server` request (`PendingAction::StartModelServer`, the exact
command line with `--api-key` masked) and returns the text-only fallback at
once; the chat stays usable while the card waits.

| What happens | Result | Audit log (`audit.log`, also `commands.log`) |
| --- | --- | --- |
| Card filed | "Restart local model server?" card in the chat bubble ("Start …" before any reply this session) | `PROPOSED` |
| **Click on Yes** (the only approval) | server started, then her last message is sent again (plain retry) | `CONFIRM` |
| Click on No, or Esc on the card | text-only, 10 min cooldown | `DENY` |
| No answer for 120 s | text-only, cooldown | `TIMEOUT` |
| No card surface (`lillith chat` CLI, headless) | text-only straight away, cooldown | `NO_UI` |
| Safeword (`red`) while it waits | dropped, cooldown | `CANCELLED` |
| Bubble closed (Esc / focus loss / window manager) while it waits | dropped by the bubble's close handler (`on_exit`, close-requested, `Drop`) | `CANCELLED` + note `chat bubble closed before an answer` |
| Bubble killed (SIGKILL, crash) while it waits | the overlay sees the bubble's stdout end with the card still open | `CANCELLED` + note `chat bubble gone before an answer (killed)` |
| Card still open from an earlier run (app killed too) | closed when the overlay starts (or `lillith chat --spawn-server`), unless the card's process (the pid in its id) is still alive | `CANCELLED` + note `orphaned PROPOSED closed at startup` |
| Overlay starts the server for a `CONFIRM` | once per card: the approval event carries the card id and argv; the argv must equal what `[chat.server]` runs now and the card must have an unused `CONFIRM` | `USED` |
| Overlay won't start it (argv differs, no `CONFIRM`, card already used) | nothing starts; the bubble stops waiting | `SPAWN_REFUSED` + reason |
| Approved start failed (spawn error, never came up) | cooldown, no resend | `SPAWN_FAILED` + reason |

Every line of one card carries the same `card=<id>` column after the path
(`<ts>\t<DECISION>\tstart_model_server\t<binary>\tcard=<id>[\t<note>]`),
so a `PROPOSED` is matched to its decision by id. Lines are only ever
appended: closing a card writes a new `CANCELLED` line, it never rewrites
the `PROPOSED`. A `CANCELLED` is written only while the log still shows the
card open, so the bubble's own close handler, the overlay's check and the
startup scan never write it twice. (Older id-less `PROPOSED` lines pair with
the next id-less decision; an orphaned one is closed with
`proposed=<its timestamp>` instead of an id.) The bubble has no SIGTERM
handler of its own; the overlay check and the startup scan cover it.

- The card shows the whole command, one argument per line (`--api-key`
  masked), scrolling when long, never cut off. The binary is resolved to
  an absolute path for the card and the spawn alike, and the spawn drops
  inherited `LLAMA_ARG_*` / `HF_*` env vars, so what runs is what was shown.
- `USED`, `SPAWN_REFUSED` and `SPAWN_FAILED` come after the decision and
  never open or close a card. Columns are sanitized (no tab or newline in a
  path or note) and a torn last line is never glued to the next one.
- Saying the safeword while the bubble waits for the approved server ends
  the wait at once and nothing is resent.
- Typed or spoken "yes" never approves: chat text (`yes`, `/contract yes`)
  is just a message, the voice path is refused while a card is up, and the
  start call (`start_approved_server` / `wait_for_approved_server`) does
  nothing without the recorded click.
- At most one card at a time; while it is pending or cooling down recovery
  only retries. The restart is never retried in a loop: a failed approved
  start also cools down. Its outcome is remembered as the
  `restart_llama_server` fix.
- Audited to `[mind]`'s `audit.log`, or the same default path in the data
  dir when `[mind]` is off.
- **Startup is gated the same way.** The overlay no longer starts
  `[chat.server]` by itself: the config only says *which* server may be
  asked for, it is not an approval. The first chat finds nothing answering
  and raises the same card; a clicked Yes makes the bubble emit
  `chat.server_approved`, and the overlay spawns the server (it owns it and
  stops it on exit, Ctrl-C or SIGTERM). A standalone `lillith chat-bubble`
  (no `--app-server`) starts it in its own session instead.
- **`lillith chat --spawn-server` asks too** (the flag is a request, not an
  approval; there is no `CLI_FLAG` consent any more). A server already
  answering needs nothing. Otherwise, after `PROPOSED`:

  | Surface | When | Approves | Audit |
  | --- | --- | --- | --- |
  | The same card, in its own window (`lillith server-card`) | a desktop is available (`WAYLAND_DISPLAY` / `DISPLAY`; always on Windows) | click on Yes only | `CONFIRM`, `DENY` (No / Esc), `TIMEOUT` (120 s), `CANCELLED` (window closed) |
  | `Start local model server? [y/N]` on the controlling terminal | no card could be shown | exactly `y` / `yes` (any case); anything else, an empty line or end of input is No | `CONFIRM_TTY` / `DENY_TTY` |
  | none | no card and no terminal (`/dev/tty` / `CONIN$` can't be opened) | never | `NO_UI` |

  The terminal answer is read only from the controlling terminal
  (`/dev/tty` on unix, `CONIN$` / `CONOUT$` on Windows), never from stdin,
  so piped input can't answer it. Not approved: chat goes on text-only.

## On by default; turning it off; forgetting

Memory is **on by default**. To turn it off, put this in
`~/.config/lillith/config.toml` (see `assets/config.example.toml`):

```toml
[memory]
enabled = false
```

Off means off: no database file is opened or created, nothing is read into
prompts and nothing is written. Existing memories stay on disk untouched until
you delete `~/.local/share/lillith/memory.db` (or say "forget everything"
while it is on).

To forget from chat (handled locally, no model call):

- "forget everything" / "wipe your memory": deletes every memory, event and
  error record (and clears the chat history).
- "forget about <topic>": deletes memories where **one row** (key + label +
  detail together) holds the whole topic: every *content word* for a 1-2
  word topic, at least two thirds of them for longer ones (stopwords like
  my/your/the/about/it are stripped; "forget about my ex" matches `ex`).
  Words match whole, plus only the exact singular/plural of the same word
  ("job"/"jobs", "box"/"boxes", "hobby"/"hobbies"; never "new"/"news", never
  "next"/"text" for `ex`). So "forget my new job" or "forget the news" never
  touches "likes new wave music". An attribute also matches when a topic
  word *is* its whole key: "forget about my new job" deletes `job = barista`
  (key `job`), never `jobsite`. Also deleted: the preferences hanging off a
  matched entity, and the value of a matched attribute **as one whole
  phrase** ("my ex is Sarah" → events/entity `sarah`; job = "google
  engineer" → events containing "google engineer", never "google chrome"),
  matching events/errors (same rule), and chat-history messages that match
  the same rule. If nothing matches she says so ("I don't have anything
  about X"); if only chat lines matched she says that too ("Nothing in my
  memory matched X. I cleared 2 chat messages that mentioned it."), never
  "Gone".
- "forget the last 30 minutes" / "the last hour": deletes
  rows created **or rewritten** (`updated_at`, schema v2) in that window
  (and the most recent chat turns, ~1 turn a minute).
- "forget the last thing I said" / "what I just said": removes only the last
  user message and her reply from the chat history, and what memory learned
  in the last 5 minutes. Runs at once.

"forget everything", "forget about X" and any time window **longer than one
hour** ("forget the last 2 hours", "the last week", "the last 1000 months",
which is everything) only *ask* first. The question says how much matched
and shows up to three short examples, e.g. *"Forget everything about my job? That's 4
memories (like "Ko's job: barista", "request: job") and 2 chat messages."*
Ko's next message must be "yes" (y / yes please / confirm / do it) to run
it. Anything else cancels (the safeword too); "no" is answered locally.
Windows up to an hour and "the last thing I said" run at once. Casual
sentences are not commands: "forget it", "forget about it, let's go",
anything with a comma, more than six words, or a topic without content
words.

Every forget logs one audit line (`target: lillith_memory::audit`) **and**
appends it to the shared `<data dir>/lillith/audit.log` (the same file the
Soul Contract uses, even with a custom `db_path`; created 0600 on unix): `<UTC time>\tFORGET\tmemory:<all|topic|last_secs|last_turn>\t
nodes=N edges=N events=N errors=N scrubbed=bool`. Kind and counts only,
never the topic or any memory text. The chat history is only rewritten after
the DB forget succeeded, under `chat_history.json.lock`, merged with what
other windows wrote (a stale window only appends its new turns, so it cannot
bring forgotten messages back); a failure to rewrite it is said in the reply.
A history file that does not parse is moved aside to
`chat_history.json.corrupt` (then `.corrupt.1`, ...) before a fresh log is
started; one that cannot be read is never overwritten (the save fails and
says so).

## Privacy and safety

- `enabled = false` ⇒ `Brain::open` returns a disabled brain: no file is
  created, every read returns nothing, every write is a no-op.
- Safeword ⇒ `observe_interaction` returns `Observed::Stop` before touching
  the DB; the chat path already short-circuits with the kill switch first.
- `forget` actually `DELETE`s the rows (nodes, edges, events, errors),
  rebuilds the FTS index, then `VACUUM`s and checkpoints + truncates the WAL
  (`secure_delete = ON`), checking the checkpoint result. If another
  connection (a second window) keeps the WAL busy, the reply says the rows
  are gone but the file is not scrubbed yet (`ForgetReport::scrubbed`).
- Every write path runs in one `BEGIN IMMEDIATE` transaction with a 2 s
  `busy_timeout`, so two processes serialize instead of failing half-way.
- The DB folder is created 0700 and the file 0600 *before* SQLite opens it
  (unix); errors propagate. Windows: default path is
  `%LOCALAPPDATA%\lillith\memory.db` (order: `$XDG_DATA_HOME` if set,
  then `%LOCALAPPDATA%`, then `HOME`), default ACLs. Roaming `%APPDATA%` is
  never used for data, since it can sync to a server.
- Caps: `max_nodes`, `max_events`, `max_errors` (least recently seen first).
- No network code, no telemetry. The crate has no HTTP dependency.

## Not done / for the lead

- No fullscreen / "busy" sensor: `MemoryBridge::set_busy_for` is the hook
  for one. Windows has no `window.focused` source yet (KWin only), so app
  habits are learned on KDE only.
- LLM-assisted extraction: `Extractor` trait hook exists; the default is the
  rule-based extractor.
- Model-server card (lead's decisions, done): startup waits for the card on
  the first chat; `--spawn-server` asks (card, else tty, else `NO_UI`);
  a card that loses its window is closed `CANCELLED` (bubble close handler,
  overlay check for a killed bubble, startup scan for orphans).
- The orphan scan runs when the overlay starts, not for `lillith chat`
  (an overlay bubble may have a live card up then).
