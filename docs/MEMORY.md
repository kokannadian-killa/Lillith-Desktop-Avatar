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
| `lillith-core::event::Event` | Desktop/sensor events (`window.opened.<app>`, `app.crash`, ...). `Brain::observe_event` turns them into app-usage / error memories. These come from `lillith-sense` (monitor, KWin bridge). |
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

- Feeding `lillith-sense` events into `Brain::observe_event` from the overlay
  main loop is documented in the crate README but not wired (it lives in the
  app's event loop, which other branches are touching).
- LLM-assisted extraction: `Extractor` trait hook exists; the default is the
  rule-based extractor.
