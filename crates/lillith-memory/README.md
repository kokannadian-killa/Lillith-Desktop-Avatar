# lillith-memory

Lillith's local learning brain: a small SQLite memory graph (entities,
events, preferences, habits, errors) with decay, reinforcement, lexical +
graph retrieval, habit prediction and error-fix recall. Sync API, one file,
no embedding model, **no network code, no telemetry**. Design: `docs/MEMORY.md`.

## Config (`[memory]`, on by default)

```toml
[memory]
enabled = true          # false = no reads, no writes, no file
db_path = ""            # default $XDG_DATA_HOME/lillith/memory.db (~/.local/share/...; Windows %LOCALAPPDATA%)
half_life_days = 14.0
top_k = 6
max_context_tokens = 300
prune_below = 0.05
max_nodes = 5000
max_events = 5000
max_errors = 1000
```

## API

```rust
use lillith_memory::{Brain, Ctx, Observed, MemoryConfig, FixOutcome, parse_forget};

let brain = Brain::open(&cfg.memory)?;            // disabled cfg => inert brain
let ctx = Ctx::now().with_app("firefox");

// Before the LLM call: compact block for the system prompt (<= ~300 tokens).
if let Some(block) = brain.context_block(user_msg, &ctx) { system.push_str(&block); }

// After the reply: learn (rule-based, no extra LLM call).
match brain.observe_interaction(user_msg, &reply, &ctx) {
    Observed::Stop => { /* safeword: nothing stored */ }
    _ => {}
}

// Desktop events from lillith-sense (window.opened.<app>, app.crash, ...).
brain.observe_event(&event, &ctx);

// Anticipation.
for p in brain.predict(&ctx, 3) { println!("{} ({:.0}%)", p.describe(), p.confidence * 100.0); }

// Error memory.
let rec = brain.record_error(&err_text, "chat.server", &ctx)?;      // count + known fix
brain.record_fix(&err_text, "set threads = 2", FixOutcome::Worked, &ctx)?;
let fix = brain.known_fix(&err_text);

// Forget (deletes rows, then VACUUM + checked WAL truncate). A topic must
// match as a whole within one row (`matches_topic`); `forget_preview` counts
// without deleting and carries a few example labels.
if let Some(cmd) = parse_forget("forget the last 30 minutes") {
    let report = brain.forget(&cmd, &ctx)?;   // counts + `scrubbed`
}
```

- `is_safeword(text)`: the canonical `lillith-core` kill-switch check plus
  "OOC: stop" anywhere. `mentions_safeword(text)` additionally catches a
  standalone "red" anywhere. Either one makes `observe_*` return
  `Observed::Stop` before any write, and `context_block` return `None`.
- `Extractor` trait: plug in an LLM-assisted extractor later
  (`Brain::with_extractor`); default is `RuleExtractor` ("I like/love/hate/
  prefer X", "my X is Y", "call me X", "remind me to ...", app names).

## Integration (done on `feat/memory-graph`)

- `lillith-mind::ChatSession::with_memory(&MemoryConfig)` attaches a brain:
  the system prompt gets the `[Memory]` block, replies feed
  `observe_interaction`, and "forget ..." messages are answered locally
  ("forget everything" / "forget about X" / windows over an hour need a
  "yes", see `ForgetCmd::needs_confirm`; chat history is
  pruned too, under a lock file). The bare safeword is still handled
  first by the kill switch in `ChatSession::send`.
- `lillith chat` and `lillith chat-bubble` pass `Config::memory` through.

## Desktop wiring

- `bridge::MemoryBridge` (overlay process): a non-blocking, rate-limited
  event bridge on its own thread and connection (debounced app focus, crash
  / error kinds, nothing else), plus proactive `memory.suggest` lines from
  `Brain::predict` behind a confidence / cooldown / hourly-cap / safeword /
  chat-busy gate. `apps/lillith/src/memory_wiring.rs` tees the overlay feed
  into it and runs the voice recovery ladder.
- `recovery`: record a failure with a stable text, order the built-in
  `SafeFix`es (`retry_backoff`, `restart_llama_server`, `text_only`) by
  what worked before, record each outcome. `ChatSession::send` uses it for
  an unreachable / loading llama-server; the restart is never run by
  recovery itself, only after a clicked Yes on the Soul Contract card.

See `docs/MEMORY.md` ("Desktop wiring").

## Tests

`cargo test -p lillith-memory` (in-memory and temp-file DBs): decay math,
retrieval ranking, neighbour expansion, token budget, forget (all / topic /
last N minutes incl. rewritten rows; rows, FTS and file bytes scrubbed; 2-char
topics; whole words only; busy WAL reported), two-connection locking, file
and folder permissions, error cap, opt-out, safeword short-circuit (incl.
`record_fix`), error-fix recall, habit prediction, migrations (v1 → v2), the event bridge (debounce, rate cap, privacy,
disabled = no thread / file, full queue never blocks), proactive gating
(threshold, cooldown, hourly cap, safeword, chat-busy, recent use) and the
error → known fix → outcome loop.
