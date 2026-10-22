<div align="center">

# Lillith — Desktop Companion & System Monitor

**An animated anime-style succubus demon woman who lives on your Linux desktop and reacts to what your machine is doing.**

[![CI](https://github.com/kokannadian-killa/Lillith-Desktop-Avatar/actions/workflows/ci.yml/badge.svg)](https://github.com/kokannadian-killa/Lillith-Desktop-Avatar/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Assets: CC BY-NC 4.0](https://img.shields.io/badge/assets-CC%20BY--NC%204.0-lightgrey.svg)](LICENSE-ASSETS.md)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange.svg?logo=rust)](https://www.rust-lang.org)
[![Platform: Linux](https://img.shields.io/badge/platform-Linux%20%7C%20KDE%20Plasma%206-informational.svg)](INSTALL.md)

</div>

---

**Lillith** is a transparent, always-on-top, click-through animated desktop
companion for **Linux** — built for **KDE Plasma 6 on Wayland** (layer-shell
overlay) with an **X11/XWayland fallback**. She idles, breathes, blinks,
wanders along the bottom of your screen, reacts to your clicks and keys, gets
sleepy when ignored, and — as the system monitor lands — reacts to system state
like CPU load, memory, battery, network and windows.

She's written in pure **Rust** as a Cargo workspace (a rewrite of earlier
Python/Tk prototypes). Everything runs locally.

> **Status:** early development (`0.0.1`). The overlay, animation engine and
> interaction/behaviour state machine work today (M0–M3). The system monitor
> (M4) is in progress; window awareness, tray, chat, memory and voice are
> planned. See [Status / Roadmap](#status--roadmap).

## Contents

- [Features](#features)
- [Status / Roadmap](#status--roadmap)
- [Quick start](#quick-start)
- [Controls](#controls)
- [Project layout](#project-layout)
- [Architecture](#architecture)
- [Development](#development)
- [License](#license)

## Features

✅ = works today · 🚧 = in progress · 🗓️ = planned

**Avatar & animation**

- ✅ **Transparent overlay** — a `zwlr_layer_shell_v1` surface on the Overlay
  layer (Wayland), or an undecorated always-on-top ARGB window (X11). Clicks
  pass through everywhere except her silhouette (the input region follows each
  frame's alpha mask).
- ✅ **Spring-damper jiggle physics** — one spring per channel (root x/y, angle,
  scale, chest, pigtails), sub-stepped at 120 Hz with velocity/offset clamps,
  driven by the tuned `assets/physics.toml` (energy-tiered impulses on
  reactions, footfall bounces, walk start/stop lean).
- ✅ **Animation library** — the v2 video-based set in `assets/generated/`
  holds 97 clips (44 event reactions, 12 blend helpers, idle variants); the
  image pipeline's jiggle/flirty batch adds up to 197 (113 engine-clean,
  including tasteful allure poses and `idle_flirt_*` mannerisms, plus 84
  baked `_jiggle` variants that switch the engine's chest/pigtail/hip springs
  off while drawn — `--baked-jiggle`). Per-frame timing, feet anchors and
  attachment points, played by a priority reaction queue on top of idle life
  (breathing, blinking every 4–6 s, a calm idle by default: transitions every
  25–40 s, gestures every 45–90 s, a mood-based pose cycle every 45 s, all
  tunable in `[behavior]`). *Art storage: see [Assets](#assets).*
- ✅ **Walk / run** with left/right clips or mirrored fallback.
- 🗓️ Speech-bubble rendering (lines are chosen and logged today), SFX playback,
  particles, HiDPI buffer scale.

**Behaviour & interaction**

- ✅ **Moods** — Playful, Happy, Mischievous, Alert, Sleepy, with decaying
  strength and a mood-dependent idle choice.
- ✅ **Mischief meter** — grows with every poke (+0.15, τ = 60 s); while high,
  her reactions turn mischievous. Ignore her for ~2.5 min and she plays a prank.
- ✅ **Sleep / wake** — drowsy after 5 min without interaction, asleep after
  15 min; hover, click, a key or an important event wakes her up.
- ✅ **Wander / Stay** — in Wander she strolls to a new spot along the bottom
  every 20–45 s; in Stay she stays put. (`follow` is accepted but behaves like
  Stay until the KWin bridge lands in M5.)
- ✅ **Click / hover / drag / key interactions** — see [Controls](#controls).
  Her dragged position is saved and restored.
- ✅ **Gravity and window perching** — drop her and she falls in her
  `drag_held` pose with a soft landing bounce onto the floor above the panel,
  or (with the KWin bridge script) onto the top edge of the window under her,
  where she strolls along it, sits on the edge (`sit_swing` and its takes) and
  falls again when the window moves, minimizes or closes (`[behavior.gravity]`).
- ✅ **Secret click sequence** — find it yourself, or peek at [Controls](#controls).
- ✅ **57 event reactions** — `assets/rules.toml` maps 57 events (system
  events plus her own click/hover/idle/LLM events: `cpu.high`, `battery.low`, `net.lost`, `usb.added`, `window.closed`,
  `notify.received`, …) to a clip, mood, gesture, line and SFX cue, with
  per-rule cooldowns. All 57 have an animation ([`docs/EVENT_MAPPING.md`](docs/EVENT_MAPPING.md)).
  Today they fire from her own input and the debug injector
  (`lillith event <name>`); live detection is M4.

**System monitor & integrations**

- 🚧 **System monitor (M4)** — CPU, RAM, battery, network lost/restored,
  Wi-Fi, VPN, volume, brightness, USB, notifications, trash, screenshots and
  idle/return, with hysteresis and cooldowns (`lillith-sense`).
- 🗓️ **KWin bridge (M5)** — window open/close/minimize reactions, perching on
  the focused title bar, Follow mode.
- 🗓️ **Tray, settings panel, autostart, PKGBUILD (M6)** — an egui panel
  already exists for previewing poses (`lillith panel`).
- 🗓️ **Local LLM chat + persistent memory (M7)** — Ollama or `llama-server`.
- 🗓️ **Voice (M8)** — Piper / espeak-ng TTS, push-to-talk via whisper.cpp.

## Status / Roadmap

Authoritative milestone list and acceptance criteria: [`docs/PLAN.md` §5](docs/PLAN.md#5-milestones-and-acceptance-criteria).

| | Milestone | Scope |
|---|---|---|
| ✅ | **M0** — Workspace + asset pipeline | Cargo workspace, `.gitignore`, README, CI (fmt + clippy + test), manifest importer |
| ✅ | **M1** — Overlay on KDE Wayland | Transparent layer-shell overlay above normal windows, click-through outside her silhouette, drag with saved position, `--backend x11` fallback. *X11 verified; Wayland path implemented, awaiting an on-machine Plasma 6 smoke test.* |
| ✅ | **M2** — Animation engine | Clip player, springs, breathing, blink, idle transitions, walk cycle with flip, every clip previewable (`lillith preview`, panel Poses tab). *Physics now driven by the tuned `physics.toml`.* |
| ✅ | **M3** — Interaction & state machine | Left/double/right/middle click, hover → peek, drag pose, random idle, pose cycle, README-SP keys, secret sequence, moods, mischief, sleep/wake, Wander/Stay; fake-clock unit tests |
| 🚧 | **M4** — System monitor v1 | Real detection for CPU, RAM, battery, net lost/restored, Wi-Fi, VPN, volume, brightness, USB, notifications, trash, screenshot, idle/return; cooldown + hysteresis |
| 🗓️ | **M5** — KWin bridge | `kpackagetool6` script; window open/close/minimize reactions; perching on the focused title bar; Follow mode and proximity wake |
| 🗓️ | **M6** — Tray, panels, config | ksni tray menu, live egui settings for `config.toml`, autostart `.desktop` + PKGBUILD |
| 🗓️ | **M7** — Memory + LLM chat | rusqlite memory, chat bar (Y/Enter) streaming from Ollama / llama-server, offline personality lines, read-only tools with approval |
| 🗓️ | **M8** — Voice | Spoken replies (Piper, else espeak-ng), push-to-talk → whisper → chat |
| 🗓️ | **M9** — Proactive life & polish | Proactive comments, greeting styles, work-session nudges, SFX, particles, optional "What do you see?" vision |

## Quick start

Full setup (system packages for Arch/Garuda and Debian/Ubuntu, config, flags,
troubleshooting) is in **[INSTALL.md](INSTALL.md)**. The short version:

```fish
git clone https://github.com/kokannadian-killa/Lillith-Desktop-Avatar.git
cd Lillith-Desktop-Avatar
cargo build --release -p lillith

set -x LILLITH_MANIFEST /path/to/generated/manifest.toml   # bash: export LILLITH_MANIFEST=…
./target/release/lillith
```

Handy subcommands:

```sh
lillith rules                              # list every event rule and its clip
lillith event battery.low                  # resolve an event through rules.toml
lillith event notify.received "New mail"   # …with a detail string
lillith preview --list                     # list all clips in the manifest
lillith panel                              # egui window (Poses tab = clip browser)
```

## Controls

Only her opaque pixels take the mouse — everything around her goes to the
window or desktop underneath.

**Mouse**

| Action | Reaction |
|---|---|
| Left click | Knife tap; a second click within 4 s waves instead (alternates). While mischief is high it's always the knife. |
| Double click | Hacking |
| Right click | Cleaning + pose-menu stub (the real menu is M6) — does **not** quit |
| Middle click | Help |
| Hover | Sit & swing; keep hovering ~1.2 s → peek |
| Left drag | Move her (`drag_held` pose). On release the spot is saved to `config.toml`. |

**Secret sequence:** click **once**, then **double-click**, then
**right-click**, all within 3 s → tail wag and *"You found me out~"*
(30 s cooldown).

**Keyboard** (click her first so she has focus) — the README-SP key map from
`[keys]` in [`assets/rules.toml`](assets/rules.toml):

| Key | Does | Key | Does |
|---|---|---|---|
| `G` | Toggle Wander ↔ Stay | `Q` / `Esc` | Quit |
| `S` | Sleep | `M` | Mute (SFX cues) |
| `T` | Tail (dual tail) | `K` | Hacking |
| `H` | Help | `R` | Random reaction |
| `C` | Cycle pose | `Y` / `Enter` | Chat *(stub until M7)* |
| `W` `V` `B` `A` `E` `F` `P` | Wi-Fi, VPN, battery, alert, error, firewall, private poses | `D` `N` `U` `J` `X` `Z` | Diagnostics, Bluetooth, cloud sync, bookmark, compress, loading poses |
| `0`–`9` | Running, htop, task manager, settings, tools, screenshot, calendar, clipboard, volume, brightness poses | `I` | Voice listen *(planned, M8)* |

You can always stop her with `pkill lillith`.

## Project layout

```text
Lillith-Desktop-Avatar/
├── apps/
│   └── lillith/            # main binary: CLI, tokio runtime, wiring
├── crates/
│   ├── lillith-core/       # pure logic: events/commands, rules.toml, moods, mischief,
│   │                       #   springs + PhysicsDriver, config, behaviour state machine (no I/O)
│   ├── lillith-anim/       # manifest/atlas loader, clip player, animator, tiny-skia renderer, preview
│   ├── lillith-overlay/    # the avatar window: Wayland layer-shell + X11 fallback, input region, drag
│   ├── lillith-sense/      # system monitors → event bus (sysinfo, D-Bus, inotify, KWin)   [M4, WIP]
│   ├── lillith-mind/       # memory (rusqlite) + LLM client + read-only tools              [planned]
│   ├── lillith-voice/      # TTS / push-to-talk STT / SFX                                   [planned]
│   ├── lillith-panel/      # egui chat + settings window (Poses tab works today)
│   └── lillith-tray/       # ksni StatusNotifierItem tray                                   [planned]
├── tools/
│   ├── import-manifest/    # generated manifest.json → manifest.toml (+ --check)
│   └── slice-sheets/       # sprite sheets + pose stills → atlases + manifest
├── kwin/
│   └── lillith-bridge/     # KWin script: window events + cursor → D-Bus             [M5]
├── assets/                 # rules.toml (57 event rules + key map), physics.toml, art (Git LFS)
└── docs/                   # PLAN.md, FEATURES.md, EVENT_MAPPING.md
```

## Architecture

```text
┌───────────────────────────┐   Commands    ┌──────────────────────────────┐
│ lillith-overlay           │◀──────────────│ lillith-core                 │
│ layer-shell (Wayland) or  │               │ rules · moods · springs ·    │
│ winit (X11), click-through│──input Events▶│ state machine (no I/O)       │
└───────────────────────────┘               └──────────────▲───────────────┘
        ▲ frames                                           │ Events (bus)
┌───────┴───────┐  ┌──────────────┐  ┌──────────────┐  ┌───┴────────────┐
│ lillith-anim  │  │ lillith-mind │  │ lillith-voice│  │ lillith-sense  │◀── KWin script
│ atlas · clips │  │ memory · LLM │  │ TTS · STT ·  │  │ sysinfo ·      │    (kwin/lillith-bridge)
│ physics       │  │ tools        │  │ SFX          │  │ D-Bus · inotify│
└───────────────┘  └──────────────┘  └──────────────┘  └────────────────┘
        lillith-panel (egui chat + settings window) · lillith-tray (ksni)
```

The render thread owns the overlay (30 Hz redraw cap, physics sub-stepped at
120 Hz). A tokio runtime hosts sensors, LLM and voice; everything talks to
`lillith-core` over one event bus, and the core emits commands back to the
overlay. Wayland hides other windows and the global cursor from clients, so a
small KWin script (M5) will forward window events and the cursor over D-Bus.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

CI ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)) runs exactly that
on Rust 1.85 for pushes to `main` and pull requests, plus a
`clippy --all-features` job with the system libraries installed.

Design docs: [`docs/PLAN.md`](docs/PLAN.md) (build plan, milestones),
[`docs/FEATURES.md`](docs/FEATURES.md) (every feature and its source),
[`docs/EVENT_MAPPING.md`](docs/EVENT_MAPPING.md) (rules ↔ clips),
[`assets/README.md`](assets/README.md) (asset pipeline).

### Assets

The character art (sprite sheets, pose stills, the generated 95-clip set) is
**not committed yet**; it will be added through **Git LFS** (`.gitattributes`
is already set up — run `git lfs install` first). Until then, point Lillith at
a local generated set with `--manifest` or `LILLITH_MANIFEST`.

## License

- **Code:** [MIT](LICENSE) © 2026 Ko Kannadian.
- **Art assets** (the Lillith character sprites/animations under `assets/`):
  [CC BY-NC 4.0](LICENSE-ASSETS.md) © 2026 Ko Kannadian — not covered by the
  MIT license.
