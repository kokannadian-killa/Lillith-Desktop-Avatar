# Biotek Engine terminology map (Aether → Biotek Engine)

Branch: `biotek/terminology` (off `quantumsec/integration` @ `e0248a6`).
Scope: Lillith OS + QuantumSec (this repo). Rules: additive, non-breaking,
nothing deleted, legacy names kept as read fallbacks / deprecated aliases.

## 1. Reference material found

| Source | What it shows |
|---|---|
| GitHub `kokannadian-killa/BoiTekEngine` (public, Rust, last push 2026-07-07; note the repo name is spelled **BoiTek**) | The existing Biotek Engine project. Crate/binary `biotekengine`, brand `BIOTEKENGINE`, config `config/biotekengine.toml`, systemd `biotekengine.service` + `biotekengine-backup.{service,timer}`, env `BIOTEKENGINE_DATA_DIR` / `BIOTEKENGINE_CONFIG` / `BIOTEKENGINE_LOG_LEVEL` / `BIOTEKENGINE_GUI_DIR`, metrics prefix `biotekengine_*`, GUI `gui/biotekengine.html` (+ legacy `gui/aethermind.html`), ports 8080 (static GUI) / 8765 (WebSocket) / 9090 (Prometheus). Its message bus module is still called `aether_core` / `AetherCore` ("Nervous System"). |
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

`src/main.rs` hard-codes `Config::load("config/biotekengine.toml")`; the
`BIOTEKENGINE_*` env vars set by its units/Dockerfile are not read by the code
we saw (only `RUST_LOG` matters). If the engine is bundled into the ISO, run it
with `WorkingDirectory` set so `./config` and `./data` resolve, e.g. a user unit
with `WorkingDirectory=%h/.biotek/engine` (proposal, not implemented here).

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
3. **WebSocket** `ws://<host>:8765`: JSON frames `{"type": "metrics"|"state"|"holo_update"|"event", "data": …}`; a QuantumSec panel can subscribe read-only. Bind to loopback (upstream binds `0.0.0.0`).
4. **Prometheus** `http://localhost:9090/metrics`, prefix `biotekengine_` (e.g. `biotekengine_pending_approvals`, `biotekengine_swarm_agents_active`).
5. **Static GUI** `:8080` serves `gui/biotekengine.html`. Conflict: Lillith's llama-server default is also `127.0.0.1:8080` (`[llm] server_url`); one must move before bundling both.
6. **LLM**: engine uses Ollama `http://localhost:11434`, model `dolphin3.0-llama3.2-3b`; Lillith's Ollama host defaults to the same port, so one Ollama can serve both.
7. **systemd**: upstream ships a hardened *system* unit (`User=biotekengine`, `/var/lib/biotekengine`). For the ISO prefer a user unit or keep the system unit but do not mix data with `~/.biotek`.
8. **Legal/scope**: upstream is "PROPRIETARY" and pentest-oriented (`swarm <target>`, `predict-*`, `pos-audit`). Lillith's red team is LOCAL SIMULATION ONLY; only defensive/read-only engine commands should be wired without explicit consent.
