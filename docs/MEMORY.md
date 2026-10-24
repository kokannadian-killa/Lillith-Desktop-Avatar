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
- "forget about <topic>": deletes memories whose key/label/detail contain one
  of the topic's *content words* as a whole word (stopwords like
  my/your/the/about/it are stripped; "forget about my ex" matches `ex`, never
  "next" or "text"), the preferences hanging off a matched entity, the value
  of a matched attribute ("my ex is Sarah" → also events/entities `sarah`),
  matching events/errors, and chat-history messages containing those words.
  If nothing matches she says so ("I don't have anything about X").
- "forget the last 30 minutes" / "the last hour" / "the last week" / "the
  last thing I said" (= 5 minutes): deletes rows created **or rewritten**
  (`updated_at`, schema v2) in that window (and the most recent chat turns).

"forget everything" and "forget about X" only *ask* first; Ko's next message
must be "yes" (y / yes please / confirm / do it) to run it. Anything else
cancels (the safeword too); "no" is answered locally. "Forget the last N
minutes" runs at once. Casual sentences are not commands: "forget it",
"forget about it, let's go", anything with a comma, more than six words, or a
topic without content words. Every forget logs one audit line
(`target: lillith_memory::audit`, kind + counts, never content). The chat
history is only rewritten after the DB forget succeeded, under
`chat_history.json.lock`, merged with what other windows wrote (a stale window
only appends its new turns, so it cannot bring forgotten messages back);
a failure to rewrite it is said in the reply.

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
  `%LOCALAPPDATA%\lillith\memory.db` (then `%APPDATA%`), default ACLs.
- Caps: `max_nodes`, `max_events`, `max_errors` (least recently seen first).
- No network code, no telemetry. The crate has no HTTP dependency.

## Not done / for the lead

- Feeding `lillith-sense` events into `Brain::observe_event` from the overlay
  main loop is documented in the crate README but not wired (it lives in the
  app's event loop, which other branches are touching).
- LLM-assisted extraction: `Extractor` trait hook exists; the default is the
  rule-based extractor.
