# Phase 1 — QuantumSec critical running system (additive)

Branch: `quantumsec/phase1` → PR into `quantumsec/integration` (never `main`).

## Added

| Piece | Location |
|---|---|
| QuantumSec shield + protons | `crates/lillith-quantumsec` |
| Cyberpunk palette | `lillith-theme` `CYBER_*` (beside existing DemonOS colours) |
| Awakening dialog | `lillith_theme::egui_awakening` |
| Blood seal (Soul Contract) | `lillith_theme::egui_blood_seal` |
| Settings tab | Settings → **QuantumSec** |
| Right-click menu | **QuantumSec ▸ Status / Open panel…** |
| CLI | `lillith quantumsec [--interval N] [--once]` |
| Overlay start | background `ShieldHandle` → `~/.biotek/quantumsec/status.json` |
| User units (not enabled) | `linux/systemd/lillith.service`, `lillith-quantumsec.service` |

## Consent

Every lockdown / deploy goes through the blood-seal card (mouse Yes only).
Decisions append to `~/.biotek/consent/consent-YYYY-MM-DD.jsonl`.
Lockdown **proposes** exact commands; execution requires a `ConsentToken`.
Default proposal is **LOCAL SIMULATION ONLY** (`echo` / `true`).

## Data home

All QuantumSec state uses `lillith_setup::biotek` → `~/.biotek/`.

## Verify

```bash
export CARGO_TARGET_DIR=/workspace/qsec/target-shared
cargo check --workspace --all-targets
cargo test -p lillith-quantumsec -p lillith-core -p lillith-theme --features lillith-theme/egui -p lillith-settings-gui --lib
lillith quantumsec --once
# enable yourself (installer never enables):
# systemctl --user enable --now lillith-quantumsec.service
```

## Left for Phase 2

- Teams panel (Red/Blue/Purple) with WHISPER info popups on every control
- Personalization Markdown reports
- Full bot registry + anon/Tor swarm (consent-gated)
- Memory wiring for QuantumSec events
- Info-popup standard across remaining interactive chrome
