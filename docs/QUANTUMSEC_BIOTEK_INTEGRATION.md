# QuantumSec + Biotek Engine Integration

**Branch:** `quantumsec-biotek-integration`

This branch is the additive integration track for Lillith Desktop Avatar.

## Status
- Base: `main` (as of branch creation)
- Additive only: no existing visuals or functions removed
- Terminology target: Aether → Biotek Engine (`~/.aether` → `~/.biotek`)

## Planned / Designed Modules
- QuantumSecShield core (monitoring, lockdown, protons)
- Early-usage cyberpunk setup dialog
- Red / Blue / Purple teams panel with info popups
- Personalization Markdown report generator
- Consent-gated bot deployment (blood seal)
- Bot registry (main + anon swarm)
- systemd user service
- Cyberpunk / demon theme extensions

## Next Steps
1. Integrate drop-in modules under `crates/` or a new `python/` bridge as needed.
2. Preserve all existing overlay, animation, and installer behavior.
3. Build and test the hybrid DemonOS PhantomCore ISO.
4. Open a PR into `main` once the desktop system is verified.

## References
- Original repo: https://github.com/kokannadian-killa/Lillith-Desktop-Avatar
- Data home: `~/.biotek/`
- Consent required for all deploys and lockdowns.
