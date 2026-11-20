# Biotek Engine terminology map (Aether → Biotek Engine)

Branch: `biotek/terminology` (off `quantumsec/integration` @ `e0248a6`).
Scope: Lillith OS + QuantumSec (this repo). Rules: additive, non-breaking,
nothing deleted, legacy names kept as read fallbacks / deprecated aliases.

## 1. Reference material found

| Source | What it shows |
|---|---|
| GitHub `kokannadian-killa/BoiTekEngine` (public, Rust, last push 2026-07-07; note the repo name is spelled **BoiTek**) | The existing Biotek Engine project. Crate/binary `biotekengine`, brand `BIOTEKENGINE`, config `config/biotekengine.toml`, systemd `biotekengine.service` + `biotekengine-backup.{service,timer}`, env prefix `BIOTEKENGINE_` (see §4), user dot-dir `~/.biotekengine/` / CWD `.biotekengine/`, metrics prefix `biotekengine_*`, GUI `gui/biotekengine.html` (+ legacy `gui/aethermind.html`), ports 8080 (static GUI) / 8765 (WebSocket) / 9090 (Prometheus). Its message bus module is still called `aether_core` / `AetherCore` ("Nervous System"). |
| `docs/PRESERVATION_MANIFEST.md` §12 (this repo) | Phase 0 data-home spec, renamed by Ko from `~/.aether/` to `~/.biotek/`. |
| `~/.biotek/README.md` on the dev box (Phase 0) | Same layout as §12. `~/.aether/` also exists there (Phase 0 original, same layout, left in place). |
| `lillith/grok_history*.md` (dev box notes) | "BIOTEKENGINE, a mycelial organism desktop icon (Rust core, quantum branches, self-edits KEEP/REVERT logged)". |

No `~/.aether` path, `AETHER_*` env var, `aether.*` config key, `aether*` unit or
`aether*` file name exists in any branch of this repo (checked with `git grep -i aether`
across all 170 `origin/*` refs; `quantumsec/integration` is a superset of every hit).

## 2. Casing rules

| Context | Form | Example |
|---|---|---|
| Prose / UI copy (this project) | **Biotek Engine** | "Biotek Engine data home" |
| Upstream engine brand (BoiTekEngine repo) | `BIOTEKENGINE` | banner, Docker labels |
| Lillith-side identifiers, paths, modules | `biotek` (snake/kebab) | `lillith_setup::biotek`, `~/.biotek` |
| Lillith-side env vars | `BIOTEK_` prefix | `BIOTEK_HOME` |
| Talking to the upstream engine process | its own names, unchanged | `BIOTEKENGINE_CONFIG`, `biotekengine.service`, `biotekengine_*` metrics |
| Rust types (new) | `Biotek…` | `MigrationReport` lives in `biotek` |
| Decorative UI labels (terminal style) | `BIOTEK_…` | `BIOTEK_FLUX` |

Never write "BioTek", "Bio-Tek" or "BoiTek" in new code (the GitHub repo name typo is
left alone; it is not ours to rename here).

## 3. Old → new

| Old | New | Status / compat |
|---|---|---|
| `~/.aether/` (Phase 0 mandate) | `~/.biotek/` | primary = `~/.biotek`; reads fall back to `~/.aether/<rel>`; one-time copy-only migration (§5) |
| `$AETHER_HOME` (implied by the mandate; never used in code) | `$BIOTEK_HOME` | `AETHER_HOME` still honoured as a deprecated alias |
| `AETHER_*` env prefix (none existed) | `BIOTEK_*` | reserve `BIOTEK_` for Lillith/QuantumSec; do not reuse `BIOTEKENGINE_` (upstream engine's) |
| `aether_home()` (mandate wording) | `biotek::biotek_home()` | `biotek::aether_home()` kept, `#[deprecated]`, returns the Biotek home |
| GUI label `AETHER_FLUX` (settings GUI, decorative flux graph) | `BIOTEK_FLUX` | label text only; graph unchanged |
| "old Aether key `num_ctx`" (`[llm]`) | `ctx_size` | unchanged behaviour: `num_ctx` still read as alias, `ctx_size` wins (wording updated only) |
| systemd unit names | none to rename | Lillith units are `lillith-*`; no `aether*` unit exists. Future QuantumSec unit: `lillith-quantumsec.service` (user unit), not `biotek*` (that namespace belongs to the upstream engine) |
| ISO / archiso scripts | none exist yet | new ISO code must use `~/.biotek` via `lillith_setup::biotek` semantics (§6) |
| Config keys | none `aether.*` | new QuantumSec settings go in `~/.biotek/config/*.toml`, never in `~/.config/lillith/config.toml` |
| File / dir names | none contain `aether` | no `git mv` needed |

## 4. Data home layout (`~/.biotek/`, matches Phase 0 exactly)

```
~/.biotek/                 ($BIOTEK_HOME, else $AETHER_HOME [deprecated], else ~/.biotek)
├── README.md
├── config/                QuantumSec / teams / swarm settings
├── logs/                  shield + exercise logs (rotated)
├── memory/                agent-loop scratch (lillith-memory stays in ~/.local/share/lillith)
├── reports/               personalization Markdown reports
├── bots/registry/         main bot manifests
├── bots/swarm/            anon (Tor-aware) swarm manifests
├── consent/               append-only consent log
├── quantumsec/            proton status, lockdown state, callbacks
└── teams/{red,blue,purple}/
```

`lillith_setup::biotek::LAYOUT` is this list; `ensure_layout_in()` creates it.

### Differences vs the upstream Biotek Engine (BoiTekEngine)

The upstream engine has **no** `~/.biotek` concept. It uses:

| Upstream | Where |
|---|---|
| `./data`, `./keys/vault_master.key` (relative to CWD) | `config/biotekengine.toml` `[cortex] data_dir`, `[vault] key_path` |
| `/var/lib/biotekengine/{data,logs,backups}` | systemd units (system service, user `biotekengine`) |
| `/etc/biotekengine/biotekengine.toml`, `/etc/biotekengine/environment` | systemd units |
| `/data/{vault,cortex,search,backups,snapshots,logs}` | Docker image |
| `~/.biotekengine/models/` (`$HOME`-relative) | `src/model_registry.rs` (GGUF model registry) |
| `.biotekengine/{reports,probability,probability/registry,lab/registry}` (CWD-relative) | `report_generator.rs`, `probability_store.rs`, `lab_forge.rs`, `swarm/scribe.rs` |

So the upstream user-level dot-dir is **`.biotekengine`**, not `.biotek`.
Lillith keeps `~/.biotek` (Ko's Phase 0 decision); the two must not be merged
silently. Options for the Architect: (a) keep both and point Lillith's model
scanner at `~/.biotekengine/models` as an extra scan path, or (b) run the engine
with CWD `~/.biotek/engine` so its relative `.biotekengine/…` lands under
`~/.biotek/engine/.biotekengine/…`.

`src/main.rs` hard-codes `Config::load("config/biotekengine.toml")`.
`BIOTEKENGINE_DATA_DIR` / `BIOTEKENGINE_CONFIG` are set by its units, Dockerfile,
compose and k8s but no source file reads them. Env vars the code does read:
`BIOTEKENGINE_HARDWARE_EMULATOR` (`hardware_interface.rs`),
`BIOTEKENGINE_WS_RATE_LIMIT_PER_SEC` / `BIOTEKENGINE_WS_RATE_BURST`
(`rate_limiter.rs`), plus `RUST_LOG`, `HOME` / `USERPROFILE`. If the engine is bundled into the ISO, run it
with `WorkingDirectory` set so `./config` and `./data` resolve, e.g. a user unit
with `WorkingDirectory=%h/.biotek/engine` (proposal, not implemented here).

## 4b. Split: Lillith data home vs upstream engine paths

Two resolvers in `lillith_setup::biotek`, deliberately separate:

| | Lillith data home | Upstream Biotek Engine (`biotekengine`) |
|---|---|---|
| Resolver | `biotek_home()` | `engine_paths()` → `EnginePaths` |
| Root | `$BIOTEK_HOME` → `$AETHER_HOME` (deprecated) → `~/.biotek` | work dir `<biotek home>/engine` (= `%h/.biotek/engine` in a user unit) |
| Config | `~/.biotek/config/*.toml` | `$BIOTEKENGINE_CONFIG` → `<work_dir>/config/biotekengine.toml` |
| Data | layout in §4 | `$BIOTEKENGINE_DATA_DIR` → `<work_dir>/data` |
| Models | Lillith's own GGUF scan paths | `~/.biotekengine/models` (`$HOME`-relative upstream) |
| Engine state | n/a | `<work_dir>/.biotekengine/{reports,probability,lab/registry}` (`state_dir`, `reports_dir()`) |
| Names | `biotek`, `BIOTEK_*` | unchanged upstream: `BIOTEKENGINE_*`, `biotekengine.service`, `biotekengine_*` metrics |

`BIOTEKENGINE_DATA_DIR` / `BIOTEKENGINE_CONFIG` are set by the upstream units,
Dockerfile, compose and k8s files but **upstream code does not read them**. Lillith
honours them (absolute values only) so its view matches the unit's intent. Since the
engine always loads `config/biotekengine.toml` from its CWD,
`EnginePaths::config_needs_placing()` is true when the override points elsewhere; the
launcher must then copy or symlink the file into `<work_dir>/config/`. Nothing in
Lillith creates or writes engine paths.

## 5. Fallback / migration behaviour (`crates/lillith-setup/src/biotek.rs`)

* `biotek_home()`: `$BIOTEK_HOME` → `$AETHER_HOME` (deprecated) → `~/.biotek`. Only absolute, non-empty env values count.
* `legacy_aether_home()`: `~/.aether` (read fallback / migration source only; never written).
* `resolve_read(rel)`: `~/.biotek/<rel>` if it exists, else `~/.aether/<rel>` if it exists, else `~/.biotek/<rel>`. All writes go to `~/.biotek`.
* `migrate_from_aether()`: one-time, **copy-only**. Copies every regular file from `~/.aether` that is missing in `~/.biotek`; never overwrites, never deletes, skips symlinks; writes `~/.biotek/.migrated-from-aether` and is a no-op afterwards; then creates the layout. `~/.aether` is left untouched.
* Nothing calls these yet (no QuantumSec code exists on `quantumsec/integration`). New modules should call `migrate_from_aether()` once at startup, then `biotek_home()` / `resolve_read()`.

## 6. Intentionally left as "aether"

| Occurrence | Why |
|---|---|
| `aether_animator.py`, `aether_gesture_map.py`, `aether_avatar_window.py`, `aether_tts.py`, `aether_stt.py`, … in `docs/PLAN.md`, `docs/FEATURES.md`, `crates/lillith-anim/src/{lib,gestures}.rs`, `crates/lillith-sense/src/kwin.rs`, `assets/rules.toml` (comment) | Provenance: real file names of the original Python prototype (`source/aether_*.py`, outside this repo). Renaming would break the trail. |
| `status = "PY (aether)"` in `assets/rules.toml` | Same provenance tag (where the behaviour existed). Data, not branding. |
| "Aether Companion" brand pass, name option "Aether" in `docs/FEATURES.md` / `docs/PLAN.md` | Historical design record. |
| `docs/PRESERVATION_MANIFEST.md` lines 121/128 | Already describe the rename. |
| Upstream `aether_core` / `AetherCore` / `aethermind.html` in BoiTekEngine | Upstream module names inside the Biotek Engine itself; not in this repo. |

## 7. Biotek Engine integration points (for the final Lillith + QuantumSec + ISO build)

1. **Data home**: `lillith_setup::biotek` (this branch) is the single resolver for `~/.biotek`.
2. **Engine binary** `biotekengine` (Rust, clap CLI): `init <scope>`, `health`, `report`, `pending` / `approve <id>` / `deny <id>` (approval gate → map onto Lillith's consent / blood-seal dialog and `~/.biotek/consent/`), `gui`, `backup`, `search`, `model list`.
3. **WebSocket** `:8765`: JSON frames `{"type": "metrics"|"state"|"holo_update"|"event", "data": …}`; a QuantumSec panel can subscribe read-only. **Must be localhost-only** (see §8).
4. **Prometheus** `:9090/metrics`, prefix `biotekengine_` (e.g. `biotekengine_pending_approvals`, `biotekengine_swarm_agents_active`). **Must be localhost-only.** Note: in the upstream `main.rs` we read, `MetricsExporter` is defined but never started, so `:9090` may not be served until upstream wires it.
5. **Static GUI** `:8080` serves `gui/biotekengine.html`. **Clashes with Lillith's llama-server default** `127.0.0.1:8080` (see §8).
6. **LLM**: engine uses Ollama `http://localhost:11434`, model `dolphin3.0-llama3.2-3b`; Lillith's Ollama host defaults to the same port, so one Ollama can serve both.
7. **systemd**: upstream ships a hardened *system* unit (`User=biotekengine`, `/var/lib/biotekengine`). For the ISO prefer a user unit or keep the system unit but do not mix data with `~/.biotek`.
8. **Legal/scope**: upstream is "PROPRIETARY" and pentest-oriented (`swarm <target>`, `predict-*`, `pos-audit`). Lillith's red team is LOCAL SIMULATION ONLY; only defensive/read-only engine commands should be wired without explicit consent.

## 8. Network integration notes (no engine code changed)

Upstream hard-codes all three listeners on `0.0.0.0` (`gui_server.rs`, `websocket_server.rs`)
with fixed ports (`with_port(8080)`, `8765`) and has no CLI flag or env var for host/port.

| Port | Upstream | Required in Lillith OS |
|---|---|---|
| 8765 WebSocket | `0.0.0.0:8765` | localhost only |
| 9090 Prometheus | documented, `0.0.0.0` | localhost only |
| 8080 static GUI | `0.0.0.0:8080` | move off 8080; **leave Lillith's llama-server default `127.0.0.1:8080` alone** |

Recommended fix (unit / launch level only):

* **Container launch (preferred, no code change):** publish only to loopback and remap the GUI,
  e.g. `-p 127.0.0.1:18080:8080 -p 127.0.0.1:8765:8765 -p 127.0.0.1:9090:9090`
  (compose: `"127.0.0.1:18080:8080"` etc.). The engine keeps listening on 8080 inside its
  own network namespace, so there is no clash with llama-server, and nothing is reachable off-box.
  `18080` is a suggestion; pick any free loopback port and record it in `~/.biotek/config/`.
* **Bare systemd unit (`biotekengine.service` or a user unit with `WorkingDirectory=%h/.biotek/engine`):**
  add `IPAddressAllow=localhost` + `IPAddressDeny=any` so 8765 / 9090 / 8080 only answer loopback
  peers. This does **not** free port 8080: a bare process cannot be moved off 8080 without an
  upstream change. So under a bare unit either run the engine in its own netns (e.g.
  `PrivateNetwork=yes` plus a `systemd-socket-proxyd` unit on `127.0.0.1:18080`, `:8765`, `:9090`,
  which also cuts its Ollama access unless proxied), or do not run `biotekengine gui` while
  llama-server is up. Commands other than `gui` open no listeners.
* Upstream follow-up (not done here, BoiTekEngine untouched): a host/port flag or env for the three
  listeners would make the bare-unit path trivial.

Constants for these live in `lillith_setup::biotek` (`ENGINE_GUI_PORT`, `ENGINE_WS_PORT`,
`ENGINE_METRICS_PORT`, `ENGINE_UNIT`, `ENGINE_BIN`, `ENGINE_METRICS_PREFIX`).
