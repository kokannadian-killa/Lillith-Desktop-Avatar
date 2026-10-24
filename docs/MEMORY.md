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
- "forget about <topic>": deletes memories mentioning the topic, the
  preferences hanging off it, and matching events/errors (and chat-history
  messages containing it).
- "forget the last 30 minutes" / "forget the last hour": deletes what was
  created in that window (and the most recent chat turns).

## Privacy and safety

- `enabled = false` ⇒ `Brain::open` returns a disabled brain: no file is
  created, every read returns nothing, every write is a no-op.
- Safeword ⇒ `observe_interaction` returns `Observed::Stop` before touching
  the DB; the chat path already short-circuits with the kill switch first.
- `forget` (from chat: "forget everything", "forget about <topic>",
  "forget the last 30 minutes") actually `DELETE`s the rows (nodes, edges,
  FTS rows, events, errors) and runs `VACUUM` so nothing lingers in free pages.
- No network code, no telemetry. The crate has no HTTP dependency.

## Not done / for the lead

- Feeding `lillith-sense` events into `Brain::observe_event` from the overlay
  main loop is documented in the crate README but not wired (it lives in the
  app's event loop, which other branches are touching).
- LLM-assisted extraction: `Extractor` trait hook exists; the default is the
  rule-based extractor.
