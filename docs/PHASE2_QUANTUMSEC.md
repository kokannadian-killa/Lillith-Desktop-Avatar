# Phase 2 — QuantumSec teams, bots, reports, memory, whispers

Additive on `quantumsec/integration` after Phase 1 (PR #10).

## Delivered

| Area | Where | Notes |
|------|-------|-------|
| Red / Blue / Purple teams | `lillith-quantumsec::teams` + QuantumSec settings tab | Red = LOCAL SIMULATION ONLY; Blue = hardening via blood seal; Purple = feedback. Per-item tooltips + WHISPER popups. |
| Personalization report | `lillith-quantumsec::report` | Device profile → Markdown under `~/.biotek/reports`. Fish helpers shown; blood seal to acknowledge (not auto-run). |
| Bot registry | `lillith-quantumsec::registry` | Main + anon Tor swarm lanes. Deploy proposes exact commands (Python BotStore / socks5h); blood seal required. |
| Memory wiring | `memory_hook` + `lillith-memory` EventFilter / `Brain::observe_event` + `apps/lillith` shield `on_event` | `quantumsec.*` events habit-logged; Nominal ticks skipped. |
| WHISPER FROM THE CORE | `lillith-theme::egui_whisper` | Info rune + modal; tooltips kept. |
| Info standard on chrome | `blade_button` / `stone_button` hover text | Additive tooltips on existing blades/stones; QuantumSec rows get info runes. |

## Data home

Always `lillith_setup::biotek` → `~/.biotek` (`teams/`, `bots/`, `reports/`, `consent/`, `quantumsec/`).

## Screenshots

Real egui (xvfb + glow): `/workspace/qsec/shots/real-*.png` (Phase 1 dialogs + updated tab / whisper).

## Out of scope (Phase 3+)

- Live Tor circuit management / onion services
- Automated Red exercise runners beyond echo dry-runs
- Cross-host botnet orchestration
- Replacing Python `tools/lillith-py` BotStore (reuse only)
- Enabling systemd user units by default
- Non-local (remote) lockdown execution
