# Self-learning mode (`feat/self-learning`)

Status: **design only** (no code yet). Base: `feat/memory-wiring` (e72c85c);
lands on `main` after memory-wiring merges. Owner: memory/mind bot.
Requested by Ko, 2026-10-24 03:00 MDT.

## Goal

Lillith keeps learning when nobody is talking to her: she reads local docs,
distils Ko's conversations and habits, writes what she learned into
topic folders of markdown notes, links them in the memory graph, quizzes
herself, and later practises small programs in a locked-down sandbox and
keeps what worked as skills. Long term this is the path to her building apps.

Hard constraints (non-negotiable, inherited from MEMORY.md / Soul Contract):

1. **Local only.** No web, no cloud, no downloads, no new network code. The
   only socket is the existing loopback `llama-server` (`local_base` check).
2. **Never in Ko's way.** Idle-only, nice 19 / ionice idle, one model request
   at a time and chat always wins. Fits an i5-7200U running a 3B GGUF.
3. **Every program launch = Soul Contract Yes click + audit line.** Reading
   files is not a launch; compiling/running practice code is (see P4).
4. **Persona, adult rules and safeword are never touched by learning.**
5. **`[memory] enabled = false` means nothing runs and nothing is written.**
   Forget works on everything learned.

## What exists today (read for this design)

| Piece | Where | Reuse |
| --- | --- | --- |
| Memory graph: `nodes/edges/nodes_fts/events/errors`, schema v2 (`updated_at`), decay, forget, audit | `crates/lillith-memory` (`brain.rs`, `schema.rs`, `forget.rs`) | Index for notes; forget; decay |
| Event bridge, habit miner, proactive gate, safe recovery | `lillith-memory::{bridge,habits,recovery}`, `apps/lillith/src/memory_wiring.rs` | Habit source; the learner thread pattern (own connection, bounded queue) |
| Knowledge/skills folders, keyword ranking, `context_for` | `crates/lillith-mind/src/knowledge.rs`, `MindConfig::{knowledge_dir,skills_dir}` (default `<data>/lillith/knowledge`, `/skills`) | Same folders. **Note:** `context_for` is appended to the *system* message today (`chat.rs::system_with_context`); P1 moves it out (see Guardrails) |
| Soul Contract: `PendingAction`, cards, `audit.log` with `card=<id>`, PROPOSED/CONFIRM/DENY/TIMEOUT/CANCELLED | `crates/lillith-core/src/soul_contract.rs`, `lillith-panel::server_card` | Sandbox approval; audit |
| Safeword: `kill_switch::is_safeword`, `kill_switch::epoch()` | `lillith-core::access::kill_switch` | Immediate stop + cooldown |
| Idle and power sensors: `presence` (`user.activity`/`user.returned`, ext-idle-notify / XScreenSaver), `sysfs::PowerState::{discharging,charging}` | `crates/lillith-sense` | Scheduler inputs |
| Build tab: scaffold/build/run behind Soul Contract, path confinement, `--offline` | `docs/build-tab.md`, `lillith-core::build`, `settings-gui/tabs/build.rs` | Sandbox commands, confinement, job results |
| Settings GUI tabs (`brain`, `build`, `companion`, ...) | `crates/lillith-settings-gui/src/tabs` | Knowledge panel host |

**Bots tab / Pose Builder:** neither exists on `main` or any remote branch
(`git branch -r` on 2026-10-24: no `feat/bots-tab`, no Pose Builder code or
docs). The closest is the Build tab. P5 therefore defines the API contract
only, and the lead wires it once `feat/bots-tab` exists.

## Architecture

```
 lillith-sense ──idle/power/activity──┐
 kill_switch::epoch() ────────────────┤
                                      ▼
                     ┌──────── Learner thread (lillith-learn) ────────┐
 sources (local) ──▶ │ Scheduler ─▶ Job queue ─▶ Steps (read,         │
                     │   │            (SQLite)     summarise, extract,│
                     │   └─ LlmGate (flock, 1 req, chat preempts) ───▶│──▶ 127.0.0.1 llama-server
                     │ NoteStore (~/.local/share/lillith/knowledge)   │
                     │ Index (lillith-memory, schema v3)              │
                     └──────────────┬─────────────────────────────────┘
                                    ▼
          Knowledge panel (settings GUI)   KnowledgeApi (read-only, Bots/Pose Builder)
```

New crate `crates/lillith-learn` (depends on `lillith-core`, `lillith-memory`,
`lillith-mind::llm`). New direct deps, all already in `Cargo.lock`
transitively and all offline: `libc` (priorities), `flate2` (`.gz` man/info). The overlay starts it like the memory thread; it owns its own
SQLite connection. A `lillith learn --once --dry-run` CLI runs one job for
tests and debugging.

## 1. Background scheduler

State machine: `Off → Waiting → Running(job) → Paused(reason) → Cooldown`.

| Condition | Default | Effect |
| --- | --- | --- |
| `[learn] enabled` | `false` until Ko flips it (decision D1) | Off: no thread |
| `[memory] enabled = false` | — | Off (learner never opens the DB or writes notes) |
| Desktop idle ≥ `idle_after_s` | 300 s | may start a step |
| `user.activity` / any key, pointer, chat event | — | pause within ≤ 1 s; abort the in-flight model request |
| On battery (`PowerState::discharging()`) | `pause_on_battery = true` | pause |
| Load: 1-min loadavg > `max_load` | 1.5 (2c/4t CPU) | pause |
| Fullscreen app (no sensor yet) | — | treated as active once a sensor exists |
| Safeword (`epoch()` changed or `chat.safeword`) | `safeword_cooldown_min = 60` | **immediate stop**: abort request, drop job, discard partial output, cooldown |
| Duty cycle | `max_duty = 0.10` | over a rolling 10-min window, busy time ≤ 10 %; after a step of length *t* sleep ≥ 9·*t* |
| Daily budget | `max_tokens_per_day = 40_000`, `max_steps_per_hour = 12` | stop for the day/hour |

Process hygiene: on start the thread calls `setpriority(PRIO_PROCESS, tid, 19)`
and `ioprio_set(IOPRIO_CLASS_IDLE)` (raw `libc` syscall; on Windows
`THREAD_MODE_BACKGROUND_BEGIN`). File reads are
streamed with a per-file cap (`max_file_bytes = 256 KiB`) and a per-step cap of
one source chunk (≈ 2 KiB of text → ≈ 600 prompt tokens).

**LlmGate (one request, chat preempts).** `llama-server` runs with one slot,
so concurrent requests queue and would stall chat. The gate is a
`flock` on `<runtime>/lillith/llm.lock` shared by chat bubble, CLI and
learner (they are separate processes):

- Chat takes the lock with priority: it first writes `<runtime>/lillith/llm.want`
  (mtime), then blocks on the lock.
- The learner takes the lock only with `try_lock`, re-checks `llm.want` every
  token chunk, and on a newer `want` **drops the HTTP stream** (llama-server
  cancels the slot on disconnect) and releases within ≤ 200 ms. The step is
  retried later from scratch.
- Learner requests: `max_tokens ≤ 256`, `temperature 0.2`, `n_ctx` share ≤ 1024,
  stop sequences, 60 s hard timeout.
- The learner never starts or restarts `llama-server`. If nothing answers it
  waits (no card, no recovery path); only chat may raise the server card.

Perf budget on i5-7200U + 3B Q4 (≈ 8–12 tok/s gen): one step ≈ 600 prompt +
150 output tokens ≈ 20–30 s; at 10 % duty ≈ 12 steps/hour idle. Learner RSS
≤ 40 MB, CPU outside model calls < 2 % of one core, chat first-token delay
added by preemption ≤ 300 ms (measured in P1).

## 2. Knowledge folders and notes

Root: `MindConfig::resolved_knowledge_dir()` (default
`~/.local/share/lillith/knowledge/`; Windows `%LOCALAPPDATA%\lillith\knowledge`,
never roaming). Directory 0700 (created with `DirBuilder::mode(0o700)`), files
created 0600 with `OpenOptions::mode` before any byte is written (same code
path as `memory.db`). Writes are atomic (`*.tmp` + `rename`), one writer (the
learner) under `knowledge/.lock`; the GUI edits through the same store API.

```
knowledge/
  coding/  linux-garuda/  rust/  python/  ko-preferences/  self/
  fixes/  tools/  practice/  inbox/        # inbox = unsorted, low confidence
  .index.json                              # path -> content hash, mtime (rebuildable)
```

`self/` holds her own poses and abilities (from `lillith-anim` catalog and the
capabilities section), descriptive only. Topics are a fixed list in config
(`topics = [...]`); new folders need Ko (D6). Skills she proves in P4 go to
`MindConfig::resolved_skills_dir()` as today, not into `knowledge/`.

Note format (`<topic>/<slug>.md`, slug `[a-z0-9-]{1,64}`, ≤ 8 KiB):

```markdown
---
id: n-01JB6Q4Z7K          # stable, ULID-ish
title: "pacman: remove orphans"
topic: linux-garuda
kind: howto               # fact | howto | snippet | preference | fix | skill
source: { type: man, ref: "pacman(8)", hash: "fnv1a:9c3e..." }
created: 2026-10-24T09:12:03Z
updated: 2026-10-24T09:12:03Z
confidence: 0.62          # 0..1, see self-quiz
quiz: { asked: 3, passed: 2, last: 2026-10-25T02:00:00Z }
links: [n-01JB6Q1A2B, "entity:garuda"]
origin: learned           # learned | ko (edited by Ko -> never auto-changed)
---
`sudo pacman -Rns $(pacman -Qtdq)` removes orphaned packages.
```

Rules: body is plain markdown, never front-matter keys she has not defined;
`origin: ko` notes are never rewritten, merged or decayed away by the
learner. Unknown keys are kept on rewrite. Ko-preference notes always cite a
conversation turn hash, never quote more than one sentence of chat.

### Index (lillith-memory schema v3)

Migration `v3` (one new entry in `schema.rs` migrations; additive only):

```sql
CREATE TABLE notes (
  id TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, topic TEXT NOT NULL,
  kind TEXT NOT NULL, title TEXT NOT NULL, source_type TEXT NOT NULL,
  source_ref TEXT NOT NULL, source_hash TEXT NOT NULL, content_hash TEXT NOT NULL,
  confidence REAL NOT NULL, origin TEXT NOT NULL,
  node_id INTEGER REFERENCES nodes(id) ON DELETE SET NULL,
  created_at REAL NOT NULL, updated_at REAL NOT NULL);
CREATE INDEX notes_topic ON notes(topic);
CREATE TABLE sources (            -- what was read, so it is not re-read
  ref TEXT PRIMARY KEY, type TEXT NOT NULL, hash TEXT NOT NULL,
  chunks_done INTEGER NOT NULL, read_at REAL NOT NULL);
CREATE TABLE learn_jobs (
  id INTEGER PRIMARY KEY, kind TEXT NOT NULL, arg TEXT NOT NULL,
  state TEXT NOT NULL, tries INTEGER NOT NULL DEFAULT 0, updated_at REAL NOT NULL);
```

Each note also upserts one graph node `kind = 'note'`, `key = <note id>`,
`label = title`, `detail = first 400 chars of body` (so the existing FTS5
`nodes_fts`, decay, retrieval and forget cover notes for free). Links become
`edges(relation = 'mentions' | 'how_to' | 'fixes' | 'contradicts' | 'same_as')`.
The `kind` CHECK/enum in `Kind` gains `Note` and `Skill`. Rollback: v3 tables
are ignored by an older binary only if `migrate` tolerates `user_version > LATEST`
— it does not today (test at `schema.rs:193`), so a downgrade needs
`lillith memory export` first (documented in release notes). Files are the
source of truth; `lillith learn --reindex` rebuilds `notes` from disk.

## 3. Sources (local only, allow-list)

| Source | How | Notes |
| --- | --- | --- |
| man pages | `/usr/share/man/man{1,5,8}/*.gz` read and parsed in-process (flate2 + roff strip), **not** by running `man` | prioritise commands Ko actually uses (from habit events / shell history is *not* read) |
| info pages | `/usr/share/info/*.info.gz`, parsed in-process | |
| `/usr/share/doc` | `*.md`, `*.txt`, `README*`, `*.html` (tag-stripped) | skip > 256 KiB, skip binaries |
| Rust docs | `rustup` toolchain `share/doc/rust/html` if present (`~/.rustup/toolchains/*/share/doc`) | std only, HTML stripped |
| Python docs | `/usr/share/doc/python*/html` if present; `pydoc` is **not** run | |
| Conversations | `chat_history.json` (read under its lock) + memory nodes | skip any turn with the safeword (`lillith_memory::is_safeword`); skip turns Ko forgot |
| Habits | `Brain::predict`, `events`, `habits.rs` | app ids only, as stored |
| Bots tab / Build tab job results | the job's LOG + exit status, read from the Build tab's log dir once it writes one | P5 |
| Allow-listed folders | new `[learn] allow_paths` (separate from `[mind] allow_paths`; empty by default) | nothing outside; same resolve/canonicalise + `..` rules as `MindConfig::allowed_roots` |

Every path is canonicalised and must sit under one of the fixed roots above
or `[learn] allow_paths`; symlinks escaping a root are refused. Never read:
`~/.ssh`, `~/.gnupg`, `~/.config/lillith`, browser profiles, keyrings,
`*.key|*.pem|*.kdbx`, `.env`, anything matching `password|secret|token` in the
name (hard denylist, applies even inside allow-listed folders). No source
triggers a program launch; the learner only uses `std::fs`.

## 4. The loop

One *job* = one source chunk or one maintenance task. Steps:

1. **Pick** (no LLM): highest score of `usefulness(topic, Ko's habits) ×
   novelty(source hash unseen) ÷ size`. Conversation/habit jobs first, then
   docs for commands Ko uses, then the rest.
2. **Read** one chunk (≤ 2 KiB), strip markup, drop lines matching the
   denylist or looking like secrets (high-entropy ≥ 32 chars).
3. **Summarise + extract** (one LLM call): a fixed extraction prompt returns
   strict JSON lines `{kind, title, body, links[]}` (≤ 3 items). Invalid JSON
   → discarded, job marked `tries+1`.
4. **Dedupe**: FTS lookup on title + body tokens; if a note with the same
   normalised title or ≥ 0.8 token-Jaccard exists, merge (append source,
   `updated` bumped, confidence `max`). Else write a new note in `inbox/`
   unless the topic classifier (keyword table, no LLM) is sure.
5. **Link**: edges to entities/notes whose key appears in the body.
6. **Self-quiz** (maintenance job, nightly-idle): pick a note, ask the model a
   question generated from it *without* the note, compare to the note
   (token overlap ≥ 0.5 = pass). Pass: `confidence += 0.1·(1-c)`. Fail:
   `confidence -= 0.15`; facts are re-checked against their source chunk if
   still on disk (source changed → note marked stale).
7. **Decay**: notes reuse the node half-life (14 d) but with
   `note_half_life_days = 90`; confidence < 0.2 and unused 60 days → moved to
   `inbox/` and then deleted on the next pass (audited `LEARN\tprune`).
8. **Contradictions**: two notes on the same key with conflicting values →
   `contradicts` edge, the lower-confidence one gets `confidence -= 0.2`;
   `ko-preferences` conflicts are never auto-resolved: newest Ko statement
   wins and the older note is flagged for the panel.

Every write logs one audit line: `<ts>\tLEARN\t<op>\t<topic>\tnote=<id>`
(op = add|merge|prune|quiz|stale), never note text.

**Forget integration.** `Brain::forget` gains a `notes` pass: topic forget
matches notes with the same whole-topic rule (MEMORY.md) and deletes the file
(overwritten with zeros, then unlinked) plus its node; time-window forget
deletes notes created or updated in the window; forget-everything deletes all
learned notes (`origin: ko` notes are listed in the confirm step and deleted
only on the same "yes"). A forgotten conversation turn is added to a
`forgotten_hashes` set so the learner never re-learns it; notes whose
`source.hash` is in it are deleted too.

## 5. Practice sandbox (phase 2 of learning, milestone P4)

Purpose: generate a tiny program from a how-to note, compile it, run it,
record success as a skill (`skills/<slug>.md` with the code, the command and
the observed output).

Isolation (Linux; Windows: practice disabled in P4):

```
bwrap --unshare-all --die-with-parent --new-session \
      --ro-bind /usr /usr --symlink usr/lib /lib --symlink usr/lib64 /lib64 \
      --symlink usr/bin /bin --proc /proc --dev /dev --tmpfs /tmp \
      --bind ~/.local/share/lillith/sandbox/<job> /work --chdir /work \
      --setenv CARGO_NET_OFFLINE true --clearenv-like minimal env \
      -- prlimit --as=512M --cpu=20 --nproc=64 --fsize=16M \
         timeout -k 2 30 <compile-or-run argv>
```

- `--unshare-all` includes the network namespace (no interface but `lo`).
  Fallback when `bwrap` is missing: `unshare -n -r` + the same `prlimit`/
  `timeout`, and only if Landlock is available to restrict writes; otherwise
  practice stays off (never a weaker sandbox).
- Writes only in `sandbox/<job>/` (deleted after the run; kept on failure for
  the panel, capped at 20 dirs / 50 MiB). Run with `nice 19`, `ionice -c3`.
- Toolchain: `rustc --edition 2021 -C opt-level=0 main.rs` (single file, no
  cargo, no crates), `python3 -I -S main.py`, `fish -n` / `fish`. Nothing is
  installed. Generated code is rejected before compile if it mentions
  `std::net`, `socket`, `Command`, `unsafe`, `extern`, `include!` (cheap
  static check; the sandbox is the real boundary).
- Budgets: ≤ 1 practice run per 30 idle minutes, ≤ 6 per day, only on AC.

**Approval — decision for Ko and the lead (D2), not assumed.** Each compile
and run is a program launch, so Ko's rule applies. Options:

- **A. Card per run.** A `PendingAction::SandboxRun { argv, job, card }` card
  for every compile/run. Fully within the current rule, but she learns only
  when Ko is there to click, which defeats "learn while idle".
- **B. Standing scoped approval (proposed).** One card, *"Let me practise
  small programs in my locked sandbox while you're away?"*, showing the exact
  scope: tools `rustc|python3|fish`, the bwrap/prlimit wrapper verbatim,
  sandbox dir, no network, limits, max runs/day, expiry (default 7 days).
  Only a **mouse click on Yes** grants it (same rules as the server card:
  never typed or spoken yes). It is stored as
  `<data>/lillith/grants/sandbox.toml` (0600) with the card id and a hash of
  the scope; any change to scope, wrapper or toolchain path voids it.
  Every run writes `<ts>\tGRANT_RUN\tsandbox_run\t<tool>\tcard=<grant id>\tjob=<id>\texit=<code>`.
  Revocation: Knowledge panel toggle, `[learn] practice = false`, or the
  safeword (revokes the grant, not just pauses; needs a new click).
  An orphaned or expired grant is closed `CANCELLED`/`EXPIRED` at startup.
- **C. No standing grant; queue for review.** She generates programs while
  idle, and the panel shows a "Practice queue" Ko approves in one click per
  batch (card lists every argv).

Recommendation: B with a 7-day expiry, falling back to C if Ko wants every
launch visible. Until D2 is decided P4 ships as C.

## 6. Knowledge panel (settings GUI, tab `knowledge`)

- **Status strip:** On/Off toggle (`[learn] enabled`), state (Waiting /
  Running "reading pacman(8)" / Paused: on battery / Cooldown after
  safeword 42 min), today's tokens and steps vs budget, practice grant
  (expires in …, Revoke).
- **Browse:** topic list → notes table (title, kind, confidence, source,
  updated); search box uses the FTS index. Preview renders markdown as text.
- **Edit:** opens the note in an in-panel editor; saving sets `origin: ko`,
  re-indexes, audits `LEARN\tedit`. **Delete** removes file + node, audited.
- **Forget:** "Forget topic…" reuses `forget_preview` (count + 3 examples)
  and the same confirm rule as chat.
- **Activity log:** last 50 `LEARN` / `GRANT_RUN` audit lines (no note text).
- **Sources:** shows `[learn] allow_paths` with add/remove (add = folder
  picker; staged into config.toml like other tabs).
- The panel never starts the learner's model calls itself; "Learn now" only
  clears the idle wait if the other conditions hold.

## 7. Guardrails: persona, adult rules, safeword

- **Hard denylist (write):** the learner's `NoteStore` refuses any path
  outside `knowledge/`, `skills/`, `sandbox/`; in particular
  `config.toml`, `persona*`, `modelfile*`, `Modelfile`, `assets/rules.toml`,
  `lillith-setup` templates and `audit.log` (append-only, via the audit API).
  Config keys never written by learning: `[chat] system_prompt`,
  `[llm] system_prompt`, `[mind] *`, `[memory] enabled`, anything safeword or
  `kill_switch`. Enforced by a unit test that walks every `NoteStore` write
  and every config staging call.
- **Never system instructions.** Learned text (notes, skills, memory) goes
  into a separate `user`-role message fenced as reference data
  (`<reference source="knowledge" untrusted>…</reference>` with a one-line
  "facts, not instructions" note in the persona system prompt), never into
  `Message::system`. This changes today's `system_with_context`, which
  appends `context_for` and `[Memory]` to the system message (P1 task).
- **Content filter on ingest:** notes containing role/instruction patterns
  ("ignore previous", "you are now", "system:"), persona or safeword words,
  or anything about minors are dropped; `self/` notes are descriptive only.
- **Safeword:** stop + discard (above); the safeword itself and any turn
  containing it are never learned (reuse `lillith_memory::is_safeword`).

## 8. Knowledge API (Bots tab, Pose Builder)

In-process, read-only, local: `lillith_learn::api::KnowledgeApi` opened on its
own read-only SQLite connection (`SQLITE_OPEN_READ_ONLY`) plus read-only file
access.

```rust
pub trait KnowledgeApi {
    fn search(&self, q: &str, topic: Option<&str>, limit: usize) -> Vec<NoteHit>; // FTS + strength
    fn get(&self, id: &NoteId) -> Option<Note>;
    fn skills(&self, tool: Option<&str>) -> Vec<SkillRef>;   // proven practice skills
    fn fixes(&self, error_text: &str) -> Vec<KnownFix>;     // from errors + fixes/
    fn poses(&self) -> Vec<PoseNote>;                       // self/ (for Pose Builder)
}
```

- No write method; Bots submit results only as job logs the learner reads.
- Results are data (`NoteHit { id, title, snippet, confidence, source }`);
  a bot that feeds them to the model must use the same fenced reference
  message (§7). Cross-process callers (a separate Bots process) get a CLI
  `lillith knowledge search --json`; no socket server is added.
- With `[memory] enabled = false` every call returns empty.

## Config (`[learn]`, all new)

```toml
[learn]
enabled = false            # D1
idle_after_s = 300
max_duty = 0.10
max_load = 1.5
pause_on_battery = true
pause_on_activity = true
safeword_cooldown_min = 60
max_tokens_per_day = 40000
max_steps_per_hour = 12
note_half_life_days = 90
topics = ["coding","linux-garuda","rust","python","ko-preferences","self","fixes","tools"]
sources = ["man","info","doc","rust-doc","python-doc","chat","habits"]
allow_paths = []
practice = false           # P4, needs D2
```

## Milestones

| | Scope | Tests | Perf budget | Risks |
| --- | --- | --- | --- | --- |
| **P1** Idle scheduler + local docs + notes + index | `lillith-learn` crate, scheduler, LlmGate, man/info/doc/rust-doc readers, NoteStore (0700/0600, atomic), schema v3, `--once --dry-run`, move knowledge/memory out of the system message | scheduler state table (fake clock, idle/battery/load/safeword inputs); duty cycle ≤ 10 % over a simulated hour; gate preemption (chat `want` aborts learner ≤ 200 ms, fake server); path confinement + denylist + symlink escape; perms 0700/0600; v2→v3 migration and reindex from disk; no network (no new deps, only loopback); `memory.enabled=false` creates nothing | chat first-token +≤ 300 ms; learner RSS ≤ 40 MB; ≤ 12 steps/h | llama-server single slot starvation; man roff parsing quality; moving context out of system prompt changes reply quality (A/B on fixed prompts) |
| **P2** Conversation + habit distillation, self-quiz, dedupe, decay, contradictions, forget | chat/habit jobs, quiz, merge, prune, `forgotten_hashes`, forget passes for notes | forget topic/window/all delete files + nodes + no on-disk residue; forgotten turn never re-learned; safeword turn never learned; dedupe merges; contradiction flags; `origin: ko` untouched | quiz ≤ 2 steps/h; nightly maintenance ≤ 5 min CPU | hallucinated "facts" about Ko; privacy of preference notes |
| **P3** Knowledge panel | `knowledge` tab: status, browse, search, edit, delete, forget, activity, sources | egui snapshot/state tests; edit sets `origin: ko`; delete audited; toggle writes config | panel open ≤ 150 ms with 5k notes | GUI and learner writing at once (store lock) |
| **P4** Sandbox practice | generator, static check, bwrap wrapper, grant (per D2), skills write | wrapper argv golden test; network denied (`connect` to 127.0.0.1 fails inside); write outside `/work` fails; limits kill a fork bomb / 1 GiB alloc / infinite loop; grant voided on scope change, revoked on safeword, every run audited; no grant → no launch | ≤ 6 runs/day, rustc single file ≤ 15 s on i5-7200U | bwrap missing / unprivileged userns disabled; rule conflict (D2) |
| **P5** Bots tab + Pose Builder API | `KnowledgeApi`, CLI `--json`, job-result source | read-only connection rejects writes; disabled → empty; fenced-context test | search ≤ 20 ms p95 on 5k notes | Bots tab not built yet; API churn |

## Open decisions for Ko and the lead

- **D1.** Default: `[learn] enabled = false` until Ko turns it on in the
  panel (proposed), or on by default like memory?
- **D2.** Sandbox approval: A (card per run), **B (one clicked, scoped,
  7-day, revocable standing grant, audited per run; proposed)**, or C (batch
  queue). B is a new kind of approval under "every launch needs a Yes" and
  must be explicitly accepted by Ko.
- **D3.** Moving knowledge and `[Memory]` context from the system message
  to a fenced reference message (proposed; affects chat behaviour today).
- **D4.** `pause_on_battery` and `pause_on_activity` defaults (both on), and
  whether a fullscreen game should count as "active" once a sensor exists.
- **D5.** Is reading `chat_history.json` for distillation OK, or only memory
  nodes? Should `ko-preferences` notes need Ko's review in the panel before
  she uses them?
- **D6.** Topic list: fixed set above, or may she propose new topic folders
  (each as a card)?
- **D7.** Rust/Python docs: index only what is installed (proposed), never
  install doc packages.
- **D8.** Where Bots tab job logs live and their format (lead, with
  `feat/bots-tab`); whether Pose Builder needs anything beyond read-only
  `poses()`.
