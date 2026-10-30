# Preservation Manifest — Lillith OS + QuantumSec (Phase 0)

Baseline: `main` @ `ec0036613e3272fb021249e488f757279dc2bea4` (2026-10-25 07:30 MDT,
"Merge fix/firstrun-voice-status … into ci/batch37"). Audited 2026-10-29.

**Rule: additive only.** Everything listed here must still exist, build, and behave the
same after every QuantumSec phase. New work goes beside it (new crates/files/menu items),
never in place of it. A regression check against this list is part of every phase report.

## 1. Rust workspace (18 members, all in `members` and `default-members`)

| Member | Purpose (from Cargo.toml) |
|---|---|
| `crates/lillith-core` | Pure behaviour logic: events, commands, moods, springs, rules, config, right-click menu tree (`menu.rs`), Soul Contract consent gate (`soul_contract.rs`), access/command policy. No I/O. |
| `crates/lillith-anim` | Atlas loader, clip player, procedural gestures (ported from `aether_animator.py`), particles. |
| `crates/lillith-overlay` | Transparent, always-on-top, click-through avatar overlay: wlr-layer-shell (KDE Plasma 6 Wayland) + winit/X11 fallback; tiny-skia right-click popup. |
| `crates/lillith-sense` | System-monitor event sources: procfs/sysfs, desktop idle, notifications, KWin bridge (D-Bus), inotify, journald. |
| `crates/lillith-mind` | SQLite memory, local LLM client (Ollama / llama-server), context builder, read-only tools. |
| `crates/lillith-voice` | TTS chain (piper → espeak-ng → spd-say), push-to-talk STT (whisper.cpp), SFX. |
| `crates/lillith-panel` | egui window: chat bar/bubble, settings, pose browser, event injector, contract card, server card, voice mic. |
| `crates/lillith-tray` | KDE/freedesktop StatusNotifierItem tray icon (ksni). |
| `crates/lillith-setup` | First-run LLM setup wizard core: runtime detection (llama-server/Ollama), GGUF scanner + picker, Ollama Modelfile customizer, `[llm]` config. |
| `crates/lillith-theme` | "Ancient demon relic × DemonOS" look: palette, bundled fonts (JetBrains Mono, Chakra Petch, Cinzel Decorative, Michroma), runes/skulls/horns/blades/blood-drip/claw geometry, egui Visuals. |
| `crates/lillith-settings-gui` | `lillith-settings` binary: settings/options window (tabs: about, avatar, bounce, brain, build, companion, icon, import, knobs, sealed I/II/III) incl. Chat/Brain first-run setup wizard, video import. |
| `crates/lillith-admin` | Elevated Windows helper (one UAC prompt, named pipe). |
| `crates/lillith-memory` | Decaying SQLite memory graph, lexical+graph retrieval, habit mining, error-fix recall, forget + safeword. Fully local. |
| `crates/lillith-learn` | Idle self-learning from installed docs; user-approved Tor-only online research (P5, off by default). |
| `apps/lillith` | Main `lillith` binary: overlay + brain + chat (`chat.rs`), contract/server consent cards, memory & learn wiring, preview. |
| `tools/slice-sheets` | Offline asset pipeline: slices sheets/stills into atlases + manifest.toml. |
| `tools/import-manifest` | Converts generated manifest.json strips into engine manifest.toml. |
| `android/rust` | Android JNI glue: desktop brain + animator + clips for the overlay service. |

Binaries: `lillith`, `lillith-settings`, `lillith-admin` (Windows).
Baseline build: `cargo check --workspace --all-targets` passes with 0 warnings (rustc 1.85.1).

## 2. Linux / Arch / Garuda installer & desktop integration

- `install.sh` (root), `linux/install.sh`, `linux/uninstall.sh` — per-user installer/upgrader (pacman only with sudo, never AUR).
- `linux/PKGBUILD`, `linux/lillith.install`, `linux/lillith.desktop` (Exec `/usr/bin/lillith`, action "Lillith Settings").
- `linux/aio/` — all-in-one package: `lillith-bin.install`, `lillith-firstrun` (voice kit, model pick, config, menu entry, KWin bridge, enables `lillith-voice.service`), `lillith-shadows.sh`, `lillith-uninstall`, `lillith-wrapper`, `test-aio.sh`.
- `linux/portable/` — `config.toml`, `run.sh`, `run.fish`.
- `linux/stage-assets.py`, `linux/fetch-art.py`, `linux/kwin-blur-exclude.sh`, `linux/tests/` (fake-kconfig, blur-exclude test).
- `kwin/lillith-bridge/` — KWin script (`metadata.json`, `contents/code/main.js`).
- `tools/bundle/make-bundle.sh` + `bundle/llama/llama.lock` (pinned llama.cpp b11829) + `bundle/models/README.txt`.
- Windows: `install.ps1`, `install.bat`, `windows/` (NSIS `installer.nsi`, `build.sh`, `lillith.ico`, …) — preserved even though the ISO is Linux-only.

## 3. Icons & branding

- `branding/icons.toml`, `branding/make-icons.py`, `branding/README.md`.
- 15 icon sets × 10 files (`<id>.ico`, `icon-1024.png`, `lillith-{16,24,32,48,64,128,256}.png`) = **150 files**:
  chibi-perched, chibi-terminal, classic, demonos-emblem, demonos-green-chibi, glitch,
  green-hologram, hooded-phantom, kiss-skullframe, neon-portrait, neon-sign,
  phantomcore-red, pixel-chibi, winged-sigil, wink-chibi.
- Android: `android/app/res/mipmap-anydpi-v26/` 15 `ic_icon_*.xml` + `ic_launcher{,_round}.xml`;
  `mipmap-xxxhdpi/` 15 `ic_icon_*_fg.png` + `ic_launcher_foreground.png`.
- `windows/lillith.ico`.

## 4. GUI surfaces (Rust, egui / tiny-skia — no Qt in repo)

- Right-click menu (`crates/lillith-core/src/menu.rs`, drawn by overlay / tray):
  Poses ▸, Chat, Avatar ▸, Resize ▸, Minimize to peek, Mode ▸ (Stay/Wander/Follow),
  Mood ▸ (Playful/Happy/Mischievous/Alert/Sleepy), Outfit ▸, Speak out loud, Mute voice,
  Pause system monitor, Settings…, Icon…, Reset position, About Lillith, Close.
- Settings window + setup wizard (`lillith-settings-gui`, `lillith-setup`), sealed add-in slots I/II/III.
- Chat bubble, Soul Contract card (`contract-card`), model-server card (`server-card`), poses page.
- Tray icon (`lillith-tray`).

## 5. Animation / video pipeline & assets (Git LFS)

- `.gitattributes`: LFS for `assets/**/*.{png,jpg,jpeg,webp,gif,mp4,webm,wav,ogg,flac,gguf}` + `master.zip`.
- LFS: **573 files, ~2.23 GB** (462 in `assets/generated`, 110 in `assets/source`, 1 `master.zip`). LFS fetch verified working.
- `assets/generated/` — 232 clip folders (`<clip>.json`, `.mp4`, `_sheet.png`): idle/blend/event_*/dom_*/dance/etc.
- `assets/source/` — `video/`, `frames_640x960/`, `poses/`, `reference/`, `sheets/`.
- `assets/{config.example.toml,physics.toml,rules.toml,seasons.toml,README.md}`.
- `tools/video-pipeline/` (~35 Python scripts + tests), `tools/matte-clean/`, `tools/gen-kit/` (prompts, recipes, `make_clip.py`).

## 6. Python sidecars

- `tools/lillith-py/` — attachments + bots backend for the Bots tab (loopback HTTP API, cron, contract, safeword, templates assistant/planner/researcher/writer); systemd user units `lillith-py-api.service`, `lillith-py-scheduler{,-daemon}.service`, `lillith-py-scheduler.timer` (installed, never auto-enabled).
- `tools/lillith-voice/` — voice daemon (`lillith-voiced`, `-say`, `-listen`, `-chat`, `-piper`), Fish helper `fish/lillith-voice.fish`, `systemd/lillith-voice.service`, tests.

## 7. Android APK

`android/` — `app/` (OverlayService, ChatActivity, MenuActivity, SetupActivity, LocalLlm, IconSet, voice …), `llm/` (llama.cpp JNI C shim), `rust/`, `build-apk.sh`, `clip-pack.py`, `voice-tests.sh`, `INSTALL-android.md`, `VOICE-RUNTIME.md`.

## 8. CI

`.github/workflows/ci.yml` (check, full-features, windows), `lillith-py.yml` (pytest), `video-pipeline.yml` (pytest); composite action `.github/actions/setup-lillith/action.yml`.

## 9. Persona / banter / consent / safety systems (existing)

- Persona + banter (PR #8 "dominant, explicit banter + persona"), rules in `assets/rules.toml`, seasons/outfits.
- **Soul Contract** consent gate (`lillith-core/src/soul_contract.rs`, `apps/lillith/src/{contract_consent,server_consent}.rs`): mutations need a mouse-clicked Yes; typed/spoken yes never approves; every action audited (PROPOSED/CONFIRM/DENY/TIMEOUT/CANCELLED/NO_UI). Safeword closes cards.
- Local-only audit, loopback-only LLM, netguard in voice.

## 10. Docs

`README.md`, `INSTALL.md`, `docs/` (CHAT, CHEST_BOUNCE, EVENT_MAPPING, FEATURES, LIPSYNC, MEMORY, MENU_SETTINGS_AVATAR, OWNERS, PLAN, RELEASE-3, RELEASE-4, SEASONS, SELF_LEARNING, TASKBOARD, VIDEO_IMPORT, VIDEO_PIPELINE, build-tab, mind-tools).

## 11. Mandate gap analysis (present vs. absent at baseline)

| Mandate item | Status at baseline |
|---|---|
| Rust crates listed in §1 | **Present** (plus 9 more: setup, theme, settings-gui, admin, memory, learn, 2 tools, android). |
| install.sh / PKGBUILD / .desktop / KWin bridge / icon system | **Present**. |
| Setup wizard + right-click GUI | **Present** (egui + tiny-skia, not Qt). |
| Animation + video pipeline, LFS | **Present**. |
| Android APK + 15 icon sets | **Present**. |
| CI, banter/persona | **Present**. |
| Cyberpunk palette #0a0a0f / #ff3e6c / #00f0ff / #b829f7 | **Absent** (existing palette: VOID #050003, CRIMSON #c4001a, BLOOD_NEON #ff1a2e, NEON_BLUE #2fb4ff, VIOLET #a24dff). Add as a new named palette beside it. |
| JetBrains Mono | **Present** (bundled in lillith-theme). Demon lettering: Cinzel Decorative + rune geometry present. |
| QuantumSecSetupDialog, QuantumSecShield, protons, lockdown | **Absent** (no "quantumsec" anywhere). |
| Red/Blue/Purple teams panel + "WHISPER FROM THE CORE" popups | **Absent**. |
| Personalization report generator | **Absent**. |
| Consent-gated deployment ("blood seal") | **Partial**: Soul Contract card is the existing gate; blood-seal bot-deploy dialog absent — build on top of Soul Contract. |
| Bot registry / anon swarm | **Partial**: lillith-py bots (templates, store, API) exist; no swarm, no registry manifest dir. Tor: only lillith-learn online research. |
| `~/.aether/` data home | **Absent** in repo (only name references to `aether_animator`). Created on dev box Phase 0. |
| systemd user service | **Partial**: voice + lillith-py units exist; no unit for `lillith` itself / QuantumSec daemon. |
| ISO / archiso / build-demonos / GRUB / Plymouth / SDDM theming | **Absent** (no archiso profile or ISO script in repo). |
| PyQt6 code | **Absent** (mentioned only in docs/PLAN.md, FEATURES.md). |

## 12. Planned `~/.aether/` layout (spec only — the app creates dirs at runtime)

```
~/.aether/
├── README.md
├── config/          QuantumSec / teams / swarm settings (never overrides ~/.config/lillith/config.toml)
├── logs/            shield + exercise logs (rotated)
├── memory/          agent-loop scratch for new modules (lillith-memory stays in ~/.local/share/lillith)
├── reports/         personalization Markdown reports
├── bots/registry/   main bot manifests
├── bots/swarm/      anon (Tor-aware) swarm manifests
├── consent/         append-only consent log (exact command, decision, timestamp)
├── quantumsec/      proton status, lockdown state, callbacks
└── teams/{red,blue,purple}/   red = LOCAL SIMULATION ONLY; blue = consent-gated hardening; purple = feedback
```
