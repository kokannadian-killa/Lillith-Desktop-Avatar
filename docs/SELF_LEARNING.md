# Self-learning mode (`feat/self-learning`)

Status: **design only** (no code yet). Base: `feat/memory-wiring` (e72c85c);
lands on `main` after memory-wiring merges. Owner: memory/mind bot.
Requested by Ko, 2026-10-24 03:00 MDT. Decisions below from Ko (pt16u–
pt19u, plus dream learning 03:24, memory compaction 03:26, worker
lifespans/error handling 03:26 MDT) and New Bot defaults Ko can change.
Items Ko decided are marked **decided by Ko**.

## Goal

Lillith keeps learning when nobody is talking to her: she reads installed
docs, distils Ko's saved chat history and habits, reads pages Ko explicitly
shares with her, and — only when Ko clicks Yes — may fetch a short, announced
list of URLs over Tor, or "dream" online for a session length Ko picks (§11). She writes what she learned into topic folders of
markdown notes, links them in the memory graph, quizzes herself, and later
practises small programs in a locked-down sandbox (each compile/run needs
a click). Long term this is the path to her building apps.

Hard constraints (non-negotiable, inherited from MEMORY.md / Soul Contract,
with Ko's online/install exceptions below):

1. **Local-first.** The learner's always-on path never contacts the
   network. Online research and downloads/installs are separate, **user-
   gated** features (see §§5 and 9); they replace the original "no web"
   rule for those features only.
2. **Never in Ko's way.** Idle-only, nice 19 / ionice idle, one model
   request at a time and chat always wins. Fits an i5-7200U running a 3B
   GGUF.
3. **Every program launch = Soul Contract Yes click + audit line.** Reading
   files is not a launch; compiling/running practice code, online
   research trips, and downloads/installs are.
4. **Persona, adult rules and safeword are never touched by learning.**
5. **`[memory] enabled = false` means nothing runs and nothing is written.**
   Forget works on everything learned. Self-learning itself is **on by
   default** (D1) with an Off toggle in the Knowledge panel.

## What exists today (read for this design)

| Piece | Where | Reuse |
| --- | --- | --- |
| Memory graph: `nodes/edges/nodes_fts/events/errors`, schema v2 (`updated_at`), decay, forget, audit | `crates/lillith-memory` (`brain.rs`, `schema.rs`, `forget.rs`) | Index for notes; forget; decay |
| Event bridge, habit miner, proactive gate, safe recovery | `lillith-memory::{bridge,habits,recovery}`, `apps/lillith/src/memory_wiring.rs` | Habit source; the learner thread pattern (own connection, bounded queue) |
| Knowledge/skills folders, keyword ranking, `context_for` | `crates/lillith-mind/src/knowledge.rs`, `MindConfig::{knowledge_dir,skills_dir}` (default `<data>/lillith/knowledge`, `/skills`) | Same folders. **Note:** `context_for` is appended to the *system* message today (`chat.rs::system_with_context`); P1 moves it out (see Guardrails) |
| Soul Contract: `PendingAction`, cards, `audit.log` with `card=<id>`, PROPOSED/CONFIRM/DENY/TIMEOUT/CANCELLED | `crates/lillith-core/src/soul_contract.rs`, `lillith-panel::server_card` | Sandbox / online / install cards; audit |
| Safeword: `kill_switch::is_safeword`, `kill_switch::epoch()` | `lillith-core::access::kill_switch` | Immediate stop of the step (no rest period) |
| Idle and power sensors: `presence` (`user.activity`/`user.returned`, ext-idle-notify / XScreenSaver), `sysfs::PowerState::{discharging,charging}` | `crates/lillith-sense` | Scheduler inputs |
| Build tab: scaffold/build/run behind Soul Contract, path confinement, `--offline` | `docs/build-tab.md`, `lillith-core::build`, `settings-gui/tabs/build.rs` | Sandbox commands, confinement |
| Settings GUI tabs (`brain`, `build`, `companion`, ...) | `crates/lillith-settings-gui/src/tabs` | Knowledge panel host |
| HTTP client | `reqwest` (workspace, rustls) in `lillith-mind` | Online research (SOCKS5 via Tor only) |

**Bots tab / Pose Builder:** neither exists on `main` or any remote branch
(`git branch -r` on 2026-10-24: no `feat/bots-tab`, no Pose Builder code or
docs). The closest is the Build tab. P5 defines the API contract only; the
lead wires it once `feat/bots-tab` exists. Job logs are **not** a learning
source (Ko's source list); they live under `<data>/lillith/bots/` for the
Bots tab itself (D8).

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

  Online trips / installs / sandbox runs: only after a Soul Contract Yes click
  (never from the idle loop on its own). Online → Tor SOCKS5 127.0.0.1:9050.
```

New crate `crates/lillith-learn` (depends on `lillith-core`, `lillith-memory`,
`lillith-mind::llm`, workspace `reqwest` for gated online only). New direct
deps already in `Cargo.lock` transitively: `libc` (priorities), `flate2`
(`.gz` man/info). The overlay starts the learner like the memory thread; it
owns its own SQLite connection. A `lillith learn --once --dry-run` CLI runs
one job for tests and debugging.

## 1. Background scheduler

State machine: `Off → Waiting → Running(job) → Paused(reason)`. (The safeword cooldown was removed 2026-10-24: the safeword drops the running step, then she waits as usual.)

| Condition | Default | Effect |
| --- | --- | --- |
| `[learn] enabled` | **`true`** (D1, Ko) | Off only when Ko toggles it in the Knowledge panel |
| `[memory] enabled = false` | — | Off (learner never opens the DB or writes notes) |
| Desktop idle ≥ `idle_after_s` | 300 s | may start a step |
| `user.activity` / any key, pointer, chat event | `pause_on_activity = true` (New Bot default; Ko can change) | pause within ≤ 1 s; abort the in-flight model request |
| On battery (`PowerState::discharging()`) | `pause_on_battery = true` (New Bot default; Ko can change) | pause |
| Load: 1-min loadavg > `max_load` | 1.5 (2c/4t CPU) | pause |
| Fullscreen app | counts as busy once a sensor exists (New Bot default; Ko can change) | pause |
| Safeword (`epoch()` changed or `chat.safeword`) | — (`safeword_cooldown_min` is no longer read) | **immediate stop**: abort request, drop job, discard partial output (no rest period afterwards); cancels any open online/sandbox/install card |
| Duty cycle | `max_duty = 0.10` | over a rolling 10-min window, busy time ≤ 10 %; after a step of length *t* sleep ≥ 9·*t* |
| Daily budget | `max_tokens_per_day = 40_000`, `max_steps_per_hour = 12` | stop for the day/hour |

Process hygiene: on start the thread calls `setpriority(PRIO_PROCESS, tid, 19)`
and `ioprio_set(IOPRIO_CLASS_IDLE)` (raw `libc` syscall; on Windows
`THREAD_MODE_BACKGROUND_BEGIN`). File reads are streamed with a per-file
cap (`max_file_bytes = 256 KiB`) and a per-step cap of one source chunk
(≈ 2 KiB of text → ≈ 600 prompt tokens).

**LlmGate (one request, chat preempts).** `llama-server` runs with one slot,
so concurrent requests queue and would stall chat. The gate is a `flock` on
`<runtime>/lillith/llm.lock` shared by chat bubble, CLI and learner:

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
  fixes/  tools/  practice/  web/  inbox/   # inbox = unsorted; web/ = gated online
  .index.json                               # path -> content hash, mtime (rebuildable)
```

`self/` holds her own poses and abilities (from `lillith-anim` catalog and the
capabilities section), descriptive only. Seed topics are listed in config;
she **may create new topic folders herself** (New Bot default D6; Ko can
change) under `knowledge/<slug>/` with slug `[a-z0-9-]{1,32}`, never outside
`knowledge/`. Skills she proves in P4 go to
`MindConfig::resolved_skills_dir()` as today, not into `knowledge/`.

Note format (`<topic>/<slug>.md`, slug `[a-z0-9-]{1,64}`, ≤ 8 KiB):

```markdown
---
id: n-01JB6Q4Z7K          # stable, ULID-ish
title: "pacman: remove orphans"
topic: linux-garuda
kind: howto               # fact | howto | snippet | preference | fix | skill | web
source: { type: man, ref: "pacman(8)", hash: "fnv1a:9c3e..." }
# web notes also carry: url, fetched_via: tor, trip_id
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
**Preference notes do not need Ko's review before use** (New Bot default D5b;
Ko can change).

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
CREATE TABLE trips (              -- gated online research
  id TEXT PRIMARY KEY, purpose TEXT NOT NULL, proposed_urls TEXT NOT NULL,
  fetched_urls TEXT NOT NULL, saved_notes TEXT NOT NULL,
  state TEXT NOT NULL, card TEXT NOT NULL,
  started_at REAL, finished_at REAL);
```

Each note also upserts one graph node `kind = 'note'`, `key = <note id>`,
`label = title`, `detail = first 400 chars of body` (so the existing FTS5
`nodes_fts`, decay, retrieval and forget cover notes for free). Links become
`edges(relation = 'mentions' | 'how_to' | 'fixes' | 'contradicts' | 'same_as')`.
The `kind` CHECK/enum in `Kind` gains `Note` and `Skill`. Rollback: a
downgrade needs `lillith memory export` first (documented in release notes;
`migrate` does not tolerate `user_version > LATEST` today). Files are the
source of truth; `lillith learn --reindex` rebuilds `notes` from disk.

## 3. Sources (exactly Ko's list)

Ko (pt16u): she learns from **saved chat history as well as docs**, and she
can read **the internet pages he opens**. That is all. No other sources.

| Source | How | Notes |
| --- | --- | --- |
| Installed docs: man | `/usr/share/man/man{1,5,8}/*.gz` read and parsed in-process (flate2 + roff strip), **not** by running `man` | prioritise commands Ko actually uses (from habit events); shell history is *not* read |
| Installed docs: info | `/usr/share/info/*.info.gz`, parsed in-process | |
| Installed docs: `/usr/share/doc` | `*.md`, `*.txt`, `README*`, `*.html` (tag-stripped) | skip > 256 KiB, skip binaries |
| Installed docs: Rust | `rustup` toolchain `share/doc/rust/html` if present | std only, HTML stripped; **never install** doc packages (D7) |
| Installed docs: Python | `/usr/share/doc/python*/html` if present; `pydoc` is **not** run | never install doc packages |
| Saved chat history | `chat_history.json` under its lock (D5, Ko) | skip safeword turns (`lillith_memory::is_safeword`); skip forgotten turns |
| Memory + habits | memory nodes; `Brain::predict`, `events`, `habits.rs` | app ids only, as stored |
| Pages Ko shares | explicit "Share this page with Lillith" action Ko triggers (§3.1); never fetches the URL for this source | per-site block list; note stores the source URL |

**Not sources** (explicitly removed from the earlier draft):
`[learn] allow_paths` folders, Bots tab / Build tab job results. The Bots
tab still has a read-only query API (§8); its logs are for the tab, not for
learning.

Every path is canonicalised and must sit under one of the fixed roots above.
Symlinks escaping a root are refused. Never read: `~/.ssh`, `~/.gnupg`,
`~/.config/lillith`, **browser profiles, history, sessions or caches**, keyrings,
`*.key|*.pem|*.kdbx`, `.env`, anything matching `password|secret|token` in
the name (hard denylist). No source in this table triggers a program launch
or a network fetch; the learner only uses `std::fs` plus the local share
socket (§3.1) here.

### 3.1 Pages Ko shares (explicit, opt-in) — decided by Ko

Ko: *"she can read the internet pages i open"*. Designed as an **explicit
share**, which is what Ko was told: she reads a page only when Ko himself
sends it to her. There is **no background reading** of browser profiles,
history, sessions or caches.

- **Trigger:** Ko clicks "Share this page with Lillith" — a small local
  browser extension button (Firefox / Chromium, talking to a native-messaging
  host shipped with Lillith) or a KDE Purpose share action. It sends the
  current page's URL, title and visible text only (no cookies, form fields,
  scripts, or other tabs). Private/incognito windows: the action is not
  offered.
- **Transport:** local Unix socket `<runtime>/lillith/share.sock` (dir 0700,
  socket 0600, peer uid checked with `SO_PEERCRED`); no TCP port. Body cap
  512 KiB. The text is queued as a learn job and processed while idle.
- **Per-site block list** in the Knowledge panel (`[learn] block_hosts`);
  blocked hosts are refused at the socket with a short notice. A built-in
  denylist (banks, password managers, webmail compose, login/auth pages) is
  refused the same way.
- Stored notes carry `source.type = shared`, `source.url = <url>` and a
  content hash. **Never re-fetched** by this source; a fresh copy needs an
  online research trip (§5).
- Shared text is untrusted data: same ingest filter and fenced-reference
  rule as everything else (§7).
- Audit: `<ts>\tLEARN\tshare\t<host>\tnote=<id>` (no page text).

**Alternative Ko can opt into later (not built, off):** automatic reading of
pages he has opened from local Firefox/Chromium history, session and cache
files. It would need its own one-time Soul Contract card, would exclude
private windows, banking and login pages, and would honour the same block
list. Only built if Ko asks for it.

## 4. The loop

One *job* = one source chunk or one maintenance task. The idle loop **never**
starts an online trip, a download/install, or a sandbox run by itself.

1. **Pick** (no LLM): highest score of `usefulness(topic, Ko's habits) ×
   novelty(source hash unseen) ÷ size`. Conversation/habit/shared-page jobs
   first, then docs for commands Ko uses, then the rest.
2. **Read** one chunk (≤ 2 KiB), strip markup, drop lines matching the
   denylist or looking like secrets (high-entropy ≥ 32 chars).
3. **Summarise + extract** (one LLM call): a fixed extraction prompt returns
   strict JSON lines `{kind, title, body, links[]}` (≤ 3 items). Invalid JSON
   → discarded, job marked `tries+1`.
4. **Dedupe**: FTS lookup on title + body tokens; if a note with the same
   normalised title or ≥ 0.8 token-Jaccard exists, merge (append source,
   `updated` bumped, confidence `max`). Else write a new note in `inbox/`
   unless the topic classifier (keyword table, no LLM) is sure — or create
   a new topic folder when the classifier invents a stable slug (D6).
5. **Link**: edges to entities/notes whose key appears in the body.
6. **Self-quiz** (maintenance job, nightly-idle): pick a note, ask the model a
   question generated from it *without* the note, compare to the note
   (token overlap ≥ 0.5 = pass). Pass: `confidence += 0.1·(1-c)`. Fail:
   `confidence -= 0.15`; facts are re-checked against their source chunk if
   still on disk (source changed → note marked stale).
7. **Decay**: notes reuse the node half-life (14 d) but with
   `note_half_life_days = 90`; confidence < 0.2 and unused 60 days → moved to
   `inbox/` then deleted on the next pass (audited `LEARN\tprune`).
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
`source.hash` is in it are deleted too. Web notes for a forgotten URL go too.

## 5. Online research (user-gated) — decided by Ko

Ko: she may go online, but it is **user-gated**; she must say where she is
going, where she has gone, and what info she has saved; and she must be
**invisible online, completely anon**. This replaces the original
"local-only / never fetch" rule **for this feature only**. The idle learner
never starts a trip on its own.

### 5.1 The card (before)

A `PendingAction::OnlineResearch { trip_id, purpose, urls, max_pages,
max_bytes, card }` Soul Contract card. Only a **mouse click on Yes**
approves (never typed or spoken yes). The card shows:

- purpose (one short sentence she proposed, or that Ko typed in chat /
  the panel's "Research…" box);
- the **exact URLs or domains** she plans to visit (no globs that widen
  after the click);
- hard caps for this trip: `max_pages` (default 5) and `max_bytes`
  (default 2 MiB total).

Audit before any byte leaves: `<ts>\tPROPOSED\tonline_research\tcard=<id>\tpages=N\tbytes=B`
plus purpose and the URL list (URLs are intentional here; they are what Ko
approved).

### 5.2 Anonymity (Tor only)

- All traffic goes through a **local Tor SOCKS5** on `127.0.0.1:9050`
  (tor or arti must already be installed). **Never auto-installed by the
  learner.** If Tor is missing or the port does not answer, online research
  stays **off**. Installing Tor is allowed only under §9 (Ko asked for a
  task that needs it, card, etc.).
- DNS through the proxy (`socks5h://` so the hostname is resolved inside
  Tor). No cookies, cache, auth headers, `Referer`, or JavaScript.
- Fixed, generic User-Agent and headers matching Tor Browser's current
  release (pinned in code; updated only by a normal code change).
- Read-only **GET** (and redirects of GET). No forms, logins, or POSTs.
  Downloads limited to `text/*` and `application/xhtml+xml` /
  `application/json` with a sniffed-bytes check; executables /
  `application/octet-stream` / archives are refused.
- **New Tor circuit per trip** (`IsolateSOCKSAuth` with a fresh
  `trip_id` as SOCKS username, or arti's equivalent). Connection closes
  when the trip ends.
- Residual risks (stated on the card and in the panel): anonymity is never
  perfect; fetched content and timing can fingerprint; exit nodes see the
  cleartext of `http://` URLs (prefer `https://`).

### 5.3 During and after

- She only fetches URLs on the approved list (redirects to a different
  host are recorded and **stop** unless that host was also listed).
- Caps enforced: stop at `max_pages` or `max_bytes`; overflow is not
  fetched.
- After the trip (or on cancel / safeword / timeout): she reports in the
  Knowledge panel and the audit log:
  - **where she went** — every URL actually fetched, including redirects;
  - **what she saved** — note paths plus a ≤ 1-line summary each;
  - state: `CONFIRM` → `DONE` / `PARTIAL` / `CANCELLED` / `DENIED` /
    `TIMEOUT`.
- Audit after: `<ts>\tDONE\tonline_research\tcard=<id>\tfetched=...\tsaved=n-..,n-..`.
- Fetched text is **untrusted data**: stored as notes under `knowledge/web/`
  with `source.url` and `trip_id`, never as system instructions (§7).

## 6. Practice sandbox (milestone P4) — decided by Ko (D2 = A)

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

- `--unshare-all` includes the network namespace. Fallback when `bwrap` is
  missing: `unshare -n -r` + the same `prlimit`/`timeout`, and only if
  Landlock is available to restrict writes; otherwise practice stays off
  (never a weaker sandbox).
- Writes only in `sandbox/<job>/` (deleted after the run; kept on failure
  for the panel, capped at 20 dirs / 50 MiB). Run with `nice 19`, `ionice -c3`.
- Toolchain: `rustc --edition 2021 -C opt-level=0 main.rs` (single file, no
  cargo, no crates), `python3 -I -S main.py`, `fish -n` / `fish`. Nothing is
  installed by the sandbox itself. Generated code is rejected before compile
  if it mentions `std::net`, `socket`, `Command`, `unsafe`, `extern`,
  `include!` (cheap static check; the sandbox is the real boundary).
- Budgets: ≤ 1 practice proposal per 30 idle minutes, ≤ 6 per day, only on AC.

**Approval (D2 = A, Ko: "sandbox is user gate").** Every compile and every
run files a `PendingAction::SandboxRun { argv, job, card }` Soul Contract
card. Only a **mouse click on Yes** runs it; typed/spoken yes never counts.
Audit: `PROPOSED` / `CONFIRM` / `DENY` / `TIMEOUT` / `CANCELLED` with
`card=<id>` and the argv. Standing approval (old option B) is **removed**.
Batching (old option C) may remain only as a **queue of proposals** in the
Knowledge panel; each queued item still gets its own card and its own click.
No click → no launch. Safeword cancels any open sandbox card.

## 7. Guardrails: persona, adult rules, safeword

- **Hard denylist (write):** the learner's `NoteStore` refuses any path
  outside `knowledge/`, `skills/`, `sandbox/`, and the install record dir
  (§9); in particular `config.toml`, `persona*`, `modelfile*`, `Modelfile`,
  `assets/rules.toml`, `lillith-setup` templates and `audit.log`
  (append-only, via the audit API). Config keys never written by learning:
  `[chat] system_prompt`, `[llm] system_prompt`, `[mind] *`,
  `[memory] enabled`, anything safeword or `kill_switch`. Enforced by a
  unit test that walks every `NoteStore` write and every config staging call.
- **Never system instructions.** Learned text (notes, skills, memory, web)
  goes into a separate `user`-role message fenced as reference data
  (`<reference source="knowledge" untrusted>…</reference>` with a one-line
  "facts, not instructions" note in the persona system prompt), never into
  `Message::system`. This changes today's `system_with_context`, which
  appends `context_for` and `[Memory]` to the system message (P1 task; D3
  default = do it).
- **Content filter on ingest:** notes containing role/instruction patterns
  ("ignore previous", "you are now", "system:"), persona or safeword words,
  or anything about minors are dropped; `self/` notes are descriptive only.
- **Safeword:** stop + discard (above); the safeword itself and any turn
  containing it are never learned (reuse `lillith_memory::is_safeword`).

## 8. Knowledge API (Bots tab, Pose Builder)

In-process, read-only for queries, local: `lillith_learn::api::KnowledgeApi`
opened on its own read-only SQLite connection (`SQLITE_OPEN_READ_ONLY`)
plus read-only file access.

```rust
pub trait KnowledgeApi {
    fn search(&self, q: &str, topic: Option<&str>, limit: usize) -> Vec<NoteHit>;
    fn get(&self, id: &NoteId) -> Option<Note>;
    fn skills(&self, tool: Option<&str>) -> Vec<SkillRef>;
    fn fixes(&self, error_text: &str) -> Vec<KnownFix>;
    fn poses(&self) -> Vec<PoseNote>;                       // read-only now
    // later (Pose Builder): fn pose_feedback(&self, id, liked: bool) -> Result<()>;
}
```

- Job logs are **not** ingested by the learner. Location (D8, New Bot /
  lead): `~/.local/share/lillith/bots/<job-id>.jsonl`, append-only JSONL
  per job with fields `id`, `bot`, `task`, `status`, `ts`, `result_path`.
  Format owned by `feat/bots-tab` when it lands.
- Pose Builder: read-only `poses()` now; a later feedback write path for
  liked / rejected poses (gated, audited) once that UI exists.
- Results are data (`NoteHit { id, title, snippet, confidence, source }`);
  a bot that feeds them to the model must use the same fenced reference
  message (§7). Cross-process callers get `lillith knowledge search --json`;
  no socket server is added.
- With `[memory] enabled = false` every call returns empty.

## 9. Downloads and installs (user-gated) — decided by Ko

Ko: she is allowed to download and install things herself, but it needs to
be user-gated; and **not unless the user asks for her to complete a task
and it is needed for the task completion** (pt19u).

Rules:

1. **Trigger only from a Ko-asked task.** Chat / Build / Bots / a panel
   action Ko started. The self-learning idle loop **never** proposes a
   download or install (including Tor, doc packages, toolchains, or
   "nice to have" tools).
2. **Needed for completion.** The card must state which step of the
   current task requires the package, and what happens if Ko says No
   (task stops or continues without that step).
3. **Soul Contract click per install (and per download that is not a
   §5 research GET).** `PendingAction::Install { name, source_url_or_pkg,
   manager, argv, needs_root, card }` (and a sibling `Download` when she
   only fetches a file). Mouse-click Yes only.
4. **Root via pkexec / polkit.** She never prompts for or handles Ko's
   password; `pkexec` / a polkit agent does. If the agent is cancelled,
   the install is `DENIED`.
5. **Verify downloads.** HTTPS only (or Tor when the task is itself an
   online-research follow-on); checksum / signature when the upstream
   publishes one (pacman/apt signatures, `sha256sums`, minisign); refuse
   when verification fails.
6. **Audit every install:** `PROPOSED` / `CONFIRM` / `DENY` / `TIMEOUT` /
   `CANCELLED` with package, version, manager, argv, `card=<id>`. A
   durable record under `<data>/lillith/installs/<id>.toml` (0600) lists
   what was installed so the Knowledge panel can show it.
7. **Knowledge panel:** "Installs" list with name, version, when, which
   task asked; **Uninstall** files another Soul Contract card (same
   rules, root via pkexec).
8. **No auto-updates.** Nothing she installed updates itself through her;
   the distro's own updater is out of scope.
9. **Tor is not installed by the learner.** If Tor is missing, online
   research (§5) stays off. Installing Tor is allowed only when Ko asks
   for a task that needs online research and Tor is the blocker — then it
   goes through this same gated install path.

## 10. Knowledge panel (settings GUI, tab `knowledge`)

- **Status strip:** On/Off toggle (`[learn] enabled`, on by default), state
  (Waiting / Running "reading pacman(8)" / Paused: on battery), today's tokens and steps vs budget.
- **Browse / Edit / Delete / Forget:** as before; search uses FTS; edit
  sets `origin: ko`.
- **Activity log:** last 50 `LEARN` / `online_research` / `sandbox_run` /
  `install` audit lines (no note bodies).
- **Sources:** installed-docs checklist, chat+memory toggle, recent shared
  pages (URL, notes made), **block_hosts** editor. No folder allow_paths
  picker, no browser-profile reading.
- **Dreaming:** start (duration 30 min–8 h → card), live log, End early,
  past dream reports with per-item forget (§11).
- **Workers:** live helper workers (TTL left, budget used), killed /
  quarantined jobs, circuit-breaker state per domain (§12).
- **Storage:** DB size vs `max_db_mb`, last compaction run (rows merged,
  bytes saved), "Compact now" (still idle/AC only) (§13).
- **Online trips:** "Research…" (purpose + URL list → card), history of
  trips with went-to URLs and saved notes.
- **Practice queue:** proposed sandbox argv list; each still opens its own
  card (D2 = A).
- **Installs:** list + gated uninstall (§9).
- The panel never starts the learner's model calls itself; "Learn now" only
  clears the idle wait if the other conditions hold.

## 11. Dream learning (Ko-started, time-boxed) — decided by Ko

Ko (03:24 MDT): *"when the user asks say for 8 or 6 hours she can learn
using the internet and learn anything she wants during that 6 to 8 hour
peroid (user states how long from 30min to 8hours) it needs a end early
option and she can even use the task bots to do the reasearching for her"*.

1. **Start only when Ko asks.** Ko picks the duration; it must be between
   **30 min and 8 h** (validated; anything outside is rejected with a short
   message, no clamping). He confirms with **one Soul Contract mouse click**
   on a `PendingAction::DreamSession { id, minutes, deadline, budgets, card }`
   card that shows the duration, end time, budgets and the rules below. That
   click is **session-scoped consent**: it replaces the per-trip click (§5)
   for research trips *inside this session only*. It never carries over to
   another session, to the idle learner, or to sandbox runs or installs.
2. **She picks topics** from a curiosity queue: gaps in the memory graph
   (entities with few notes / low confidence), recent chats, past errors
   without a known fix. Same rules as §5: Tor only, anonymous, read-only
   GET of text/HTML, no logins, cookies or identity, **no uploads of Ko's
   data** (queries are built from topic words only, never from chat text or
   memory contents verbatim). **No installs or program launches** without
   their own per-item click; installs are still only for a task Ko asked for
   (§9), so effectively none during dreaming. No Tor → dreaming can't start
   (card says so).
3. **Visible and stoppable.** A visible "dreaming" indicator on the avatar
   and tray while it runs, plus an **End early** item in the right-click menu
   and the tray. The safeword (red / `OOC: stop`) ends it instantly. Hard
   stop at the deadline. Also stops on low battery (`stop_battery_pct`) or
   thermal limits (`stop_cpu_temp_c`). If Ko chats, chat takes priority on
   `llama-server` (LlmGate §1) and dreaming yields until he is idle again; the
   deadline does not move. Every stop is audited (`DREAM\tend\t<reason>`).
4. **Light on the i5-7200U.** One fetch at a time (`fetch_interval_s`),
   per-hour page and per-session byte caps, nice 19 / ionice idle, disk
   budget (`max_disk_mb`) and the session time limit.
5. **Research backends (D12, decided by Ko 03:30 MDT).** Ko: by "task
   bots" he means *"my own task completionbots"*, his TaskCompletion bots
   (his other Grok assistants, running in the cloud, off his laptop). The
   design therefore has two backends; see §11.1.
   - **Default: local helper workers.** She splits research into sub-tasks
     run by her own local helper task workers (bounded, `max_workers` 1–2),
     fully local and anonymous over Tor, under the sandbox, lifespan,
     capture and zero-fingerprint rules in §12. Each job is logged in the D8
     job log (`~/.local/share/lillith/bots/<job-id>.jsonl`).
   - **Optional, off by default: Ko's TaskCompletion bots** (§11.1). This
     path **leaves the machine** and is **not local or anonymous**.
6. **Live log + dream report.** While dreaming the panel shows a live log of
   where she is going (each planned URL before the GET, each visited URL
   after). When it ends she gives a **dream report** (chat + panel + audit):
   sites visited, what she learned, what was saved to memory (note paths,
   node ids), each item with its own **Forget** button.
7. **Low-strength memories.** Learned items are saved as memories/notes with
   `strength = dream_strength` (0.2) and tag `source=dream` (plus
   `dream_id`), so they decay unless reinforced by use or Ko, and are easy to
   review and forget (forget "dream <date>" removes the whole session).

### 11.1 Optional backend: Ko's TaskCompletion bots (external) — decided by Ko

> **Warning: this path leaves Ko's machine. It is NOT local and NOT
> anonymous.** It breaks the fully-local guarantee (constraint 1) and the
> zero-fingerprint / Tor rules (§12) for anything routed through it: the
> research is done by cloud assistants on Ko's own accounts, and the sites
> they visit see their traffic, not Tor.

- **Off by default.** `[learn.dream] external_bots = false`. Ko enables it
  himself in the Knowledge panel (a Soul Contract mouse click, audited
  `CONFIRM\tdream_external_enable`), per the user-gated rule. Even when
  enabled, each dream session's card has a separate, unticked
  **"Also use my TaskCompletion bots (leaves this laptop, not anonymous)"**
  option; only Ko's click on that session's card turns it on for that
  session. Typed or spoken yes never counts. The safeword disables it for
  the rest of the session.
- **What is sent (minimal):** a topic (short words from her curiosity
  queue), the budget (max time, max sources, max result size) and the
  session's deadline. **Never** chat text, memory contents, notes, file
  paths, hostnames or anything identifying beyond what the bot's own account
  already implies. Outgoing payloads go through the same identifier
  denylist as §12 queries and are shown in the live log before sending.
- **Coordination (high level):** Lillith hands topic + budget to a
  TaskCompletion bot through Ko's sanctioned agent-messaging channel; the
  bot does the research and returns summarized results (text + source URLs).
  End early, the deadline, or the safeword sends a cancel; anything arriving
  after the session ended is discarded unread.
- **Results are untrusted input.** They go through the same parent-side
  validation as local workers (ingest filter §7, schema, size, injection
  and canary checks); anything suspicious is quarantined, never saved.
  Accepted items are saved as low-strength `source=dream` memories with an
  extra tag `backend=external`.
- **Labelling:** the live log, the dream report and every audit line
  (`DREAM\texternal\t<job-id>\t…`) label items sourced this way as
  **"external (TaskCompletion bot)"**, so Ko can tell them apart and forget
  them separately (per-item, or "forget everything external from dream
  <date>").
- **Job log:** each delegated job is also recorded in the D8 job log with
  `bot = taskcompletion:<bot id>` and `backend = external`.

## 12. Helper workers: lifespans, capture handling, errors — decided by Ko

Ko (03:26 MDT): *"with advanced error handling and bot life spans incase of
capture ect"*. Applies to every helper worker (dream learning, online
research, compaction).

**Lifespans.** Every worker gets a hard TTL at spawn: wall clock plus fetch,
page, byte and token budgets. The parent **supervisor** enforces them by
killing the worker (SIGKILL of its process group; no asking). Heartbeat
every `heartbeat_s`; `missed_heartbeats_kill` missed → kill. Workers never
outlive the dream session, the safeword, End early, or the parent process
(`PR_SET_PDEATHSIG`, bwrap `--die-with-parent`).

**Capture / compromise handling.**

- All fetched content is untrusted data (prompt-injection defence): page
  text never becomes instructions, and no action is derived from page text
  beyond the planned read-only GET.
- Least privilege: a worker has **no direct memory write access** — it
  returns candidate facts to the parent, which validates them (ingest filter
  §7, schema, size) or quarantines them. No filesystem outside its scratch
  dir, no network except the Tor SOCKS port, no exec (bwrap + seccomp deny
  `execve` after start, no tools in the sandbox).
- Canary / anomaly checks: unexpected domains, output asking to run
  commands or change settings, oversized output, attempts to reach loopback
  or LAN addresses, canary strings planted in its prompt showing up in a
  request → **kill the worker, quarantine its results** (kept read-only in
  the panel for Ko, never ingested), audit line
  (`WORKER\tkill\t<reason>\tjob=<id>`), and mention it in the dream report.
- Fresh worker per task: no reused state or circuits; scratch dir wiped on
  exit (success, kill or crash).

**Anonymity for helper workers (hard rule, not configurable down) — decided
by Ko.** Ko (03:28 MDT): *"smart thinking and keep the zero finger printing
and footprinting anon for the bots esspecially"*. Applies to every worker
fetch and to §5 trips; config can only make it stricter.

- **Client:** a plain HTTP GET client only (no JS engine, no browser, so no
  WebRTC, fonts, canvas or other browser fingerprint surface), via Tor
  SOCKS5 with remote DNS (`socks5h`; no local DNS lookups, tested by a
  resolver that fails the test if called).
- **Headers:** a fixed generic set matching Tor Browser's common
  User-Agent / Accept / Accept-Language (`en-US`) set, pinned in code. No
  `Referer`. No cookies, storage or cache persisted; no `ETag` /
  `If-Modified-Since` revalidation across sessions.
- **Stream isolation:** a new Tor circuit per worker/task via SOCKS auth
  isolation (`IsolateSOCKSAuth`, random per-task credentials).
- **Timing:** randomized, jittered delays and rate limits on top of §11
  budgets, so request patterns don't fingerprint.
- **No identifiers in queries:** never Ko's name, usernames, local paths,
  hostnames, timezone, or locale beyond generic `en-US`; queries are checked
  against a denylist built from local identity data (user/host names, home
  path, known names in memory) before sending. Tracking params (`utm_*`,
  `fbclid`, `gclid`, …) are stripped from URLs.
- **Read-only:** no uploads, POSTs, forms or logins.
- **Storage:** fetched pages are kept only as extracted text, locally; raw
  HTML is discarded with the scratch dir.
- **Footprint on Ko's machine:** worker scratch lives in tmpfs and is wiped
  on exit; nothing persists outside `memory.db` / knowledge notes and the
  audit and job logs.
- **Smart thinking:** the planner picks a few focused queries, ranks
  candidate pages before fetching, and stops early once it has enough
  (confidence target met or no new facts in the last N pages), to keep the
  number of requests minimal.
- **Honest limit:** this minimises fingerprinting and footprint; no setup
  can guarantee zero fingerprinting or perfect anonymity.

**Advanced error handling.**

- Supervisor restart limits (`max_restarts`) with exponential backoff
  (`backoff_base_s`, capped at 300 s); then the job is marked failed.
- Circuit breakers per domain and per source: `breaker_failures` in a row →
  open for 30 min, then half-open with one probe.
- All failures are recorded in error memory (`record_error` /
  `known_fix`), so repeats are avoided and known fixes tried first.
- Crash-safe, resumable job log (D8 JSONL, append-only, fsync per line);
  on restart unfinished jobs resume or close as `CANCELLED`.
- Graceful degradation: Tor down → offline-only learning continues; model
  server down → wait (never restart it, §1).
- Never blocks chat: workers and supervisor sit behind the LlmGate.

## 13. Memory compaction (self-compression) — decided by Ko

Ko (03:26 MDT): *"the memory folders need a method of self compression for
storage and easy access for her"*. Extends the storage model in
`docs/MEMORY.md` (nodes/edges/events, strength decay).

1. **Consolidation.** Old, low-strength episodic events
   (`> digest_after_days`, strength `< digest_max_strength`) are merged into
   summary nodes (`kind = 'digest'`, daily then weekly), written by
   deterministic rollups (counts, habits, entities) and, when idle, a short
   3B summary. Digests keep edges to the entities/habits the originals
   linked, so retrieval still works. Originals are pruned after the digest is
   written and indexed. **Forget still reaches digests:** forgetting a topic
   also scrubs and rebuilds any digest mentioning it (rebuilt from remaining
   sources, or deleted if none). `origin: ko` items are never compacted away.
2. **Storage.** Long text (chat history archive, dream/research page
   extracts) is stored as zstd-compressed blobs with a small trained
   dictionary; hot recent data (`hot_days`) stays uncompressed. On idle:
   SQLite `incremental_vacuum` (VACUUM rarely), FTS5 `optimize`, and
   `wal_checkpoint(TRUNCATE)`. Size budget `memory.max_db_mb`; over budget →
   prune oldest-weakest first (never `origin: ko`, never preference notes
   without listing them in the panel).
3. **Easy access.** FTS index plus recency and strength indexes over digests,
   so lookup stays fast; blobs are decompressed on demand through an LRU
   cache (`lru_cache_mb`).
4. **Light on the i5-7200U.** Only when idle and on AC, chunked (≤ 2 s CPU
   per chunk), nice 19 / ionice idle, never during chat, resumable from a
   checkpoint. The safeword and forget always win (abort and roll back the
   current chunk).
5. **Audit** one line per run: `<ts>\tCOMPACT\trows_merged=N\tbytes_saved=B`.

Fully local. The only new dependency is the `zstd` crate via cargo (not in
`Cargo.lock` today; vendored with the release build), no other downloads.

## Config (`[learn]`, all new)

```toml
[learn]
enabled = true             # D1, Ko: on by default; Off toggle in panel
idle_after_s = 300
max_duty = 0.10
max_load = 1.5
pause_on_battery = true    # New Bot default; Ko can change
pause_on_activity = true   # New Bot default; Ko can change
pause_on_fullscreen = true # New Bot default; Ko can change (sensor pending)
safeword_cooldown_min = 60  # no longer read (no rest after the safeword)
max_tokens_per_day = 40000
max_steps_per_hour = 12
note_half_life_days = 90
topics = ["coding","linux-garuda","rust","python","ko-preferences","self","fixes","tools","web"]
allow_create_topics = true # D6, New Bot default; Ko can change
prefer_notes_need_review = false  # D5b, New Bot default; Ko can change
sources = ["man","info","doc","rust-doc","python-doc","chat","memory","habits","shared"]
share_socket = true               # "Share this page with Lillith" (explicit, Ko-triggered)
block_hosts = []                  # per-site block list (panel)
online = true                     # master switch; still needs Tor + a card per trip
online_max_pages = 5
online_max_bytes = 2097152
tor_socks = "127.0.0.1:9050"
practice = true                   # P4; each run still needs a card (D2 = A)

[learn.dream]                     # §11, decided by Ko; only when Ko starts it
min_minutes = 30
max_minutes = 480
max_workers = 2                   # local helper workers (§12), the default backend
external_bots = false             # §11.1: Ko's TaskCompletion bots; LEAVES the machine, not anonymous; Ko opt-in + per-session tick
fetch_interval_s = 20             # one fetch at a time, ≥ 20 s apart
max_pages_per_hour = 60
max_bytes_per_session = 52428800  # 50 MiB fetched
max_disk_mb = 100                 # notes + extracts written per session
dream_strength = 0.2              # low-strength memories, source=dream
stop_battery_pct = 20
stop_cpu_temp_c = 85

[learn.workers]                   # §12, decided by Ko
heartbeat_s = 10
missed_heartbeats_kill = 3
max_restarts = 3
backoff_base_s = 5                # 5, 10, 20 … capped at 300
breaker_failures = 3              # per domain/source → open 30 min

[memory.compaction]               # §13, decided by Ko
enabled = true
max_db_mb = 512
digest_after_days = 14            # episodic events older than this → digests
digest_max_strength = 0.3
zstd_level = 9
hot_days = 7                      # newer data stays uncompressed
lru_cache_mb = 8
```

## Milestones

| | Scope | Tests | Perf budget | Risks |
| --- | --- | --- | --- | --- |
| **P1** Idle scheduler + installed docs + notes + index + move context out of system message | `lillith-learn`, scheduler, LlmGate, man/info/doc/rust-doc/python-doc readers, NoteStore (0700/0600, atomic), schema v3, `--once --dry-run`, fenced reference message (D3) | scheduler state table; duty ≤ 10 %; gate preemption ≤ 200 ms; path confinement + denylist + symlink escape; perms; v2→v3 + reindex; no network from idle path; `memory.enabled=false` creates nothing; `[learn] enabled=true` by default | chat first-token +≤ 300 ms; learner RSS ≤ 40 MB; ≤ 12 steps/h | llama-server single-slot starvation; man roff quality; reply-quality change from moving context |
| **P2** Chat history + memory/habit distillation, self-quiz, dedupe, decay, contradictions, forget | chat/habit/shared-page jobs, quiz, merge, prune, `forgotten_hashes`, forget for notes | forget deletes files + nodes + no residue; forgotten/safeword turns never re-learned; `origin: ko` untouched | quiz ≤ 2 steps/h;  | privacy of preference notes |
| **P3** Knowledge panel | tab: status, browse, search, edit, delete, forget, activity, sources, block_hosts, trips, practice queue, installs | egui state tests; edit sets `origin: ko`; toggles write config; Off stops the thread | panel open ≤ 150 ms with 5k notes | GUI/learner lock contention |
| **P4** Sandbox practice (card per run) | generator, static check, bwrap wrapper, `SandboxRun` card, skills write, proposal queue | argv golden test; network denied; write outside `/work` fails; limits kill fork bomb / 1 GiB / infinite loop; no card → no launch; spoken yes rejected | ≤ 6 proposals/day; rustc single file ≤ 15 s | bwrap / userns missing |
| **P5** Online research (Tor) + gated install path + Bots/Pose API | `OnlineResearch` card, Tor SOCKS client, trip reporting, `Install`/`Download` cards, pkexec, installs ledger, `KnowledgeApi`, bots JSONL layout | Tor missing → online off; non-listed redirect refused; caps; no cookies/POST; circuit isolation; fetched text fenced untrusted; install refused without a Ko-asked task; learner never proposes install; pkexec cancel = DENY; uninstall card | trip ≤ 60 s wall; install card → agent ≤ 30 s to show | Tor/arti absent; exit-node HTTP risk; polkit UX |
| **P6** Shared pages | share socket, native-messaging host + extension, KDE share action, block list | peer-uid check; blocked/denylisted host refused; no browser-profile reads (denylist test); URL stored; injection text dropped | share → queued ≤ 50 ms | extension packaging per browser |
| **P7** Helper workers + dream learning | supervisor, worker sandbox, TTL/heartbeat, breakers, quarantine; `DreamSession` card, curiosity queue, live log, End early (menu/tray), dream report, `source=dream` memories | duration outside 30–480 min rejected; one click covers only that session's trips; deadline hard stop; safeword/End early/battery/thermal stop ≤ 1 s; chat preempts; worker killed on TTL/missed heartbeat/anomaly; quarantine never reaches memory; scratch wiped; breaker opens; Tor down → offline only; per-item forget | 1 fetch at a time; dream CPU outside model calls < 5 % one core | injection via pages; 3B summary quality; external backend (D12) leaks topics off-machine, so off by default, labelled and audited |
| **P8** Memory compaction | digests, prune-after-summarise, zstd blobs + dictionary, LRU, vacuum/optimize/checkpoint, size budget, forget rebuilds digests | digest keeps entity/habit links (retrieval golden test); forget topic scrubs + rebuilds digests; budget prunes oldest-weakest first, never `origin: ko`; resumable after kill; safeword aborts; audit line per run | chunk ≤ 2 s CPU; lookup p95 unchanged (≤ 20 ms on 5k notes) | dictionary drift; summary loses detail |

## Decisions

### Decided by Ko

| ID | Decision |
| --- | --- |
| **D1** | Self-learning is **on by default**; Off toggle in the Knowledge panel. |
| **D2** | Sandbox: **user-gated, option A** — every compile/run needs a mouse-click Yes on a Soul Contract card + audit. Standing approval (B) removed. Batching (C) may queue proposals only; each still gets its own card. |
| **D5** | She learns from **saved chat history** (`chat_history.json`, skipping safeword and forgotten turns) **as well as** memory, habits, and installed docs. |
| **Sources** | Exactly: installed docs (man/info/`/usr/share/doc`/Rust/Python), saved chat + memory + habits, and **pages Ko shares** via an explicit "Share this page with Lillith" action. Nothing else. No allow-listed folders, no Bots job results. |
| **D9 Dream learning** | Ko-started sessions of 30 min–8 h, one session-scoped click, her own topic choice, Tor/read-only rules, End early + safeword + deadline stops, light budgets, local helper workers, live log + dream report with per-item forget, `source=dream` low-strength memories (§11). |
| **D10 Workers** | Anonymity hard rule for workers (plain GET via Tor `socks5h`, generic headers, stream isolation, jitter, no identifiers, tmpfs scratch, smart early stopping); hard TTLs + heartbeats enforced by the supervisor, least privilege, quarantine on anomalies, fresh worker per task, restart limits/backoff, circuit breakers, error memory, resumable job log, graceful degradation (§12). |
| **D12 Research backends** | "Task bots" = Ko's own TaskCompletion bots (cloud). Default backend stays local helper workers over Tor; delegating to TaskCompletion bots is optional, off by default, Ko opt-in per session, leaves the machine and is not anonymous, results validated/quarantined and labelled "external (TaskCompletion bot)" (§11.1). |
| **D11 Compaction** | Self-compression of memory: digests, zstd blobs, vacuum/optimize, size budget, fast lookup, idle/AC only, audited (§13). |
| **Online** | User-gated online research: click per trip, announce where going / went / what saved, audited, through Tor, anonymous, read-only GET. Replaces local-only for this feature only. Idle loop never goes online alone. |
| **Installs** | User-gated downloads/installs, **only when Ko asked her to complete a task and it is needed for that task**. Learner never proposes installs. Click + audit; root via pkexec/polkit (she never handles the password); verified downloads; panel list + gated uninstall; no auto-updates. Tor not installed by the learner; if Tor is missing, online stays off unless Ko asks for a task that needs it. |

### Defaults set by New Bot / lead (Ko can change)

| ID | Default |
| --- | --- |
| **D3** | Move knowledge / `[Memory]` / web notes out of the system message into a fenced reference message. |
| **D4** | `pause_on_battery = true`, `pause_on_activity = true`, fullscreen counts as busy (configurable). |
| **D5b** | Preference notes do **not** need review before use. |
| **D6** | She **may** create new topic folders herself (slug rules above). |
| **D7** | Index only docs that are already installed; never install doc packages from the learner. |
| **D8** | Bots job logs at `~/.local/share/lillith/bots/<job-id>.jsonl` (append-only JSONL: id, bot, task, status, ts, result_path). Not a learning source. Pose Builder: read-only `poses()` now; later a feedback write path for liked/rejected poses. |
| **Share mechanism** | Explicit "Share this page with Lillith" (extension/native-messaging or KDE share) over a local Unix socket; no background browser reads. Automatic reading of opened pages is an opt-in alternative for later, only if Ko asks. |

### Still open

- Lead confirms the D8 log path when `feat/bots-tab` lands.

None block P1. Ko can flip any New Bot default in the panel or config.
