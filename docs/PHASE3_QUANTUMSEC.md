# Phase 3 — QuantumSec polish (wizard, rotation, tray, units, branding)

Additive on `quantumsec/integration` after Phase 2 (PR #11).

## Delivered

| Area | Where |
|------|-------|
| Setup wizard QuantumSec step | `settings-gui` Brain tab `Step::QuantumSec` (DemonOS / PhantomCore) |
| Right-click menu | Existing QuantumSec ▸ Status / Open panel (unchanged; nothing removed) |
| Report rotation | `REPORT_KEEP = 8` dated personalization-*.md |
| Consent log viewer + rotation | QuantumSec tab; `ConsentLog::list_recent` / `rotate_keep(30)` |
| Desktop entry | `linux/lillith-quantumsec.desktop` → `lillith-settings --tab quantumsec` |
| Fish helpers | `linux/fish/biotek_qsec.fish` (status / reports / consent_tail) |
| Live proton status | Tab refreshes status.json every ~2s; tray tooltip via `QuantumSecTooltip` |
| Info-rune pass | About (+ other tabs keep chrome tooltips); QuantumSec already covered |
| Systemd enable UX | Blood-seal **real** commands (`units::enable_*`); run only after YES; result → consent log |
| DemonOS / PhantomCore branding | About section; tray title; wizard copy |
| Teams labels | Red = LOCAL SIMULATION ONLY (echo); Blue = PROPOSAL, not run (exact cmds, no spawn) |

## Rules kept

`~/.biotek` via `lillith_setup::biotek`; additive only; units never auto-enabled; Red = local sim only.

## Out of scope (later)

Live Tor circuits, non-local lockdown, auto-enable systemd by default.
