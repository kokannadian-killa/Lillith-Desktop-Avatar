# Self-learning mode (`feat/self-learning`)

Status: **design only** (no code yet). Base: `feat/memory-wiring` (e72c85c);
lands on `main` after memory-wiring merges. Owner: memory/mind bot.
Requested by Ko, 2026-10-24 03:00 MDT. Decisions below from Ko (pt16u,
pt19u) and New Bot defaults Ko can change.

## Goal

Lillith keeps learning when nobody is talking to her: she reads installed
docs, distils Ko's saved chat history and habits, reads pages Ko has opened
(locally), and — only when Ko clicks Yes — may fetch a short, announced
list of URLs over Tor. She writes what she learned into topic folders of
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
| Safeword: `kill_switch::is_safeword`, `kill_switch::epoch()` | `lillith-core::access::kill_switch` | Immediate stop + cooldown |
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

State machine: `Off → Waiting → Running(job) → Paused(reason) → Cooldown`.

| Condition | Default | Effect |
| --- | --- | --- |
| `[learn] enabled` | **`true`** (D1, Ko) | Off only when Ko toggles it in the Knowledge panel |
| `[memory] enabled = false` | — | Off (learner never opens the DB or writes notes) |
| Desktop idle ≥ `idle_after_s` | 300 s | may start a step |
| `user.activity` / any key, pointer, chat event | `pause_on_activity = true` (New Bot default; Ko can change) | pause within ≤ 1 s; abort the in-flight model request |
| On battery (`PowerState::discharging()`) | `pause_on_battery = true` (New Bot default; Ko can change) | pause |
| Load: 1-min loadavg > `max_load` | 1.5 (2c/4t CPU) | pause |
| Fullscreen app | counts as busy once a sensor exists (New Bot default; Ko can change) | pause |
| Safeword (`epoch()` changed or `chat.safeword`) | `safeword_cooldown_min = 60` | **immediate stop**: abort request, drop job, discard partial output, cooldown; cancels any open online/sandbox/install card |
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
| Pages Ko opens | local browser data only (§3.1); never fetches the URL for this source | Firefox + Chromium/Chrome; detect what's present |

**Not sources** (explicitly removed from the earlier draft):
`[learn] allow_paths` folders, Bots tab / Build tab job results. The Bots
tab still has a read-only query API (§8); its logs are for the tab, not for
learning.

Every path is canonicalised and must sit under one of the fixed roots above
(or the browser-profile roots approved in §3.1). Symlinks escaping a root
are refused. Never read: `~/.ssh`, `~/.gnupg`, `~/.config/lillith`, keyrings,
`*.key|*.pem|*.kdbx`, `.env`, anything matching `password|secret|token` in
the name (hard denylist). No source in this table triggers a program launch
or a network fetch; the learner only uses `std::fs` here.

### 3.1 Pages Ko opens (local, read-only)

**Pick: local browser history + cache/session files for Firefox and
Chromium/Chrome**, detected by which profile dirs exist. A browser
extension / native-messaging host remains a future option if the profile
read proves too brittle or too invasive; it is not the P1–P3 path.

Why this pick: no install for Ko, no extension to keep alive, works for
pages already opened (including when the browser is closed), and stays
fully local. The extension path would give cleaner "current page text" but
needs Ko to install and keep an extension, and still needs a Unix socket.

| Browser | Roots (read-only) | What is read |
| --- | --- | --- |
| Firefox | `~/.mozilla/firefox/<profile>/` (default + relative profiles from `profiles.ini`) | `places.sqlite` (url, title, last_visit_date; open with
  `SQLITE_OPEN_READONLY` + immutable/`?mode=ro` so Firefox's lock is not
  disturbed), `SessionStore` / `sessionstore-backups/*.jsonlz4` for open
  tabs; cache entry bodies under `cache2/` keyed by URL when present |
| Chromium / Chrome / ungoogled | `~/.config/chromium/`, `~/.config/google-chrome/`, `~/.config/brave-browser/` (Default + Profile *) | `History` SQLite (urls, visits), `Current Session` / `Current Tabs` /
  `Last Session` for open tabs; HTTP cache under `Cache/` / `Code Cache/`
  when a body is available |

Rules:

- Only URLs with at least one visit (or currently open in a normal window).
- **Excluded:** private / incognito windows (Firefox: no places writes for
  private; Chromium: separate encrypted profile — skipped), banking and
  login pages (path/host heuristics: `/login`, `/signin`, `/auth`,
  `accounts.`, `banking.`, `paypal.`, password managers), and a built-in
  domain denylist (banks, password managers, government ID, webmail
  compose). A **per-site block list** is editable in the Knowledge panel
  and stored in `[learn] block_hosts`.
- Text is stored as a note with `source.type = browser`, `source.ref = <url>`,
  content hash of the extracted text. **Never re-fetched** by this source;
  if Ko wants a fresh copy he approves an online research trip (§5).
- Cache bodies are used only when already on disk; missing body → title +
  URL only (low confidence note), never a network GET.
- Rate: at most one new browser URL per learner step; skip visits older
  than `browser_max_age_days` (default 30).

**Approval flag for Ko:** reading the browser profile (history DB + cache)
touches personal browsing data. First enable shows a one-time Soul Contract
card: *"Let me read the local history and cache of Firefox/Chromium that
are installed, so I can learn from pages you've opened? Private windows,
banking and login pages stay out. [Yes / No]"*. Only a mouse click on Yes
enables the source; the decision is audited (`CONFIRM` /
`browser_profile_read`) and stored in config. Denying leaves the other
sources on. Re-prompt if a new browser family appears.

## 4. The loop

One *job* = one source chunk or one maintenance task. The idle loop **never**
starts an online trip, a download/install, or a sandbox run by itself.

1. **Pick** (no LLM): highest score of `usefulness(topic, Ko's habits) ×
   novelty(source hash unseen) ÷ size`. Conversation/habit/browser jobs
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
  (Waiting / Running "reading pacman(8)" / Paused: on battery / Cooldown
  after safeword 42 min), today's tokens and steps vs budget.
- **Browse / Edit / Delete / Forget:** as before; search uses FTS; edit
  sets `origin: ko`.
- **Activity log:** last 50 `LEARN` / `online_research` / `sandbox_run` /
  `install` audit lines (no note bodies).
- **Sources:** installed-docs checklist, chat+memory toggle, browser-source
  status (and the one-time profile-read card state), **block_hosts** editor.
  No folder allow_paths picker.
- **Online trips:** "Research…" (purpose + URL list → card), history of
  trips with went-to URLs and saved notes.
- **Practice queue:** proposed sandbox argv list; each still opens its own
  card (D2 = A).
- **Installs:** list + gated uninstall (§9).
- The panel never starts the learner's model calls itself; "Learn now" only
  clears the idle wait if the other conditions hold.

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
safeword_cooldown_min = 60
max_tokens_per_day = 40000
max_steps_per_hour = 12
note_half_life_days = 90
topics = ["coding","linux-garuda","rust","python","ko-preferences","self","fixes","tools","web"]
allow_create_topics = true # D6, New Bot default; Ko can change
prefer_notes_need_review = false  # D5b, New Bot default; Ko can change
sources = ["man","info","doc","rust-doc","python-doc","chat","memory","habits","browser"]
browser_profile_read = false      # flipped by the one-time card
browser_max_age_days = 30
block_hosts = []                  # per-site block list (panel)
online = true                     # master switch; still needs Tor + a card per trip
online_max_pages = 5
online_max_bytes = 2097152
tor_socks = "127.0.0.1:9050"
practice = true                   # P4; each run still needs a card (D2 = A)
```

## Milestones

| | Scope | Tests | Perf budget | Risks |
| --- | --- | --- | --- | --- |
| **P1** Idle scheduler + installed docs + notes + index + move context out of system message | `lillith-learn`, scheduler, LlmGate, man/info/doc/rust-doc/python-doc readers, NoteStore (0700/0600, atomic), schema v3, `--once --dry-run`, fenced reference message (D3) | scheduler state table; duty ≤ 10 %; gate preemption ≤ 200 ms; path confinement + denylist + symlink escape; perms; v2→v3 + reindex; no network from idle path; `memory.enabled=false` creates nothing; `[learn] enabled=true` by default | chat first-token +≤ 300 ms; learner RSS ≤ 40 MB; ≤ 12 steps/h | llama-server single-slot starvation; man roff quality; reply-quality change from moving context |
| **P2** Chat history + memory/habit distillation, browser source (local), self-quiz, dedupe, decay, contradictions, forget | chat/habit/browser jobs, quiz, merge, prune, `forgotten_hashes`, forget for notes | forget deletes files + nodes + no residue; forgotten/safeword turns never re-learned; browser skips private/login/blocked; one-time profile-read card; `origin: ko` untouched | quiz ≤ 2 steps/h; browser scan ≤ 2 s/idle step | profile schema churn; privacy of preference notes; Firefox lock races |
| **P3** Knowledge panel | tab: status, browse, search, edit, delete, forget, activity, sources, block_hosts, trips, practice queue, installs | egui state tests; edit sets `origin: ko`; toggles write config; Off stops the thread | panel open ≤ 150 ms with 5k notes | GUI/learner lock contention |
| **P4** Sandbox practice (card per run) | generator, static check, bwrap wrapper, `SandboxRun` card, skills write, proposal queue | argv golden test; network denied; write outside `/work` fails; limits kill fork bomb / 1 GiB / infinite loop; no card → no launch; spoken yes rejected | ≤ 6 proposals/day; rustc single file ≤ 15 s | bwrap / userns missing |
| **P5** Online research (Tor) + gated install path + Bots/Pose API | `OnlineResearch` card, Tor SOCKS client, trip reporting, `Install`/`Download` cards, pkexec, installs ledger, `KnowledgeApi`, bots JSONL layout | Tor missing → online off; non-listed redirect refused; caps; no cookies/POST; circuit isolation; fetched text fenced untrusted; install refused without a Ko-asked task; learner never proposes install; pkexec cancel = DENY; uninstall card | trip ≤ 60 s wall; install card → agent ≤ 30 s to show | Tor/arti absent; exit-node HTTP risk; polkit UX |

## Decisions

### Decided by Ko

| ID | Decision |
| --- | --- |
| **D1** | Self-learning is **on by default**; Off toggle in the Knowledge panel. |
| **D2** | Sandbox: **user-gated, option A** — every compile/run needs a mouse-click Yes on a Soul Contract card + audit. Standing approval (B) removed. Batching (C) may queue proposals only; each still gets its own card. |
| **D5** | She learns from **saved chat history** (`chat_history.json`, skipping safeword and forgotten turns) **as well as** memory, habits, and installed docs. |
| **Sources** | Exactly: installed docs (man/info/`/usr/share/doc`/Rust/Python), saved chat + memory + habits, and **pages Ko opens** (local browser read). Nothing else. |
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
| **Browser pick** | Local Firefox + Chromium history/cache/session (detect present). One-time profile-read card. Extension/native-messaging left as a future alternative. |

### Still open

None that block P1. Lead confirms D8 log path when `feat/bots-tab` lands; Ko can flip any New Bot default in the panel or config.
