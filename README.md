# Lillith Desktop Avatar

Interactive desktop avatar and system state and action monitor.

**Lillith** is a local desktop companion for **Garuda / Arch Linux on KDE Plasma 6 (Wayland)**. She's a transparent, always-on-top anime demon-girl avatar who lives on your desktop, moves with spring physics, reacts to what you and your system are doing (CPU load, battery, Wi-Fi, USB, notifications, windows opening and closing…), and chats with you through a fully local LLM, with her own memory, voice and tray controls.

This repository is the **pure-Rust rewrite** of the earlier Python prototypes (Tk/pygame/psutil). It is at **milestone M0: workspace scaffold**. The crates compile and the rule table is tested, but the overlay, sensors and chat are still stubs. See [`docs/PLAN.md`](docs/PLAN.md) for the full build plan and [`docs/FEATURES.md`](docs/FEATURES.md) for every feature and its status.

## Architecture

```
┌───────────────────────────┐   Commands    ┌──────────────────────────────┐
│ lillith-overlay           │◀──────────────│ lillith-core                 │
│ layer-shell (Wayland) or  │               │ rules · moods · springs ·    │
│ winit (X11), click-through│──input Events▶│ state machine (no I/O)       │
└───────────────────────────┘               └──────────────▲───────────────┘
        ▲ frames                                          │ Events (bus)
┌───────┴───────┐  ┌──────────────┐  ┌──────────────┐  ┌──┴───────────┐
│ lillith-anim  │  │ lillith-mind │  │ lillith-voice│  │ lillith-sense│◀── KWin script
│ atlas · clips │  │ memory · LLM │  │ TTS · STT ·  │  │ sysinfo ·    │    (kwin/lillith-bridge)
│ gestures      │  │ tools        │  │ SFX          │  │ D-Bus · inotify│
└───────────────┘  └──────────────┘  └──────────────┘  └──────────────┘
        lillith-panel (egui chat + settings window) · lillith-tray (ksni)
```

- **Overlay:** a `zwlr_layer_shell_v1` surface on the *Overlay* layer (KWin supports it), so she stays above normal windows. Clicks pass through everywhere except her silhouette (the input region follows each frame's alpha mask). The fallback is a `winit` transparent always-on-top window for X11/XWayland (`--backend x11`).
- **Panel:** chat, settings, pose browser and event injector run in a separate normal `egui` window.
- **KWin bridge:** Wayland hides other windows and the global cursor from clients, so a small KWin script forwards window events and the cursor position over D-Bus.

## Workspace layout

| Path | Crate | What it will hold |
|---|---|---|
| `crates/lillith-core` | lib | Pure logic: `Event`/`Command` types, moods + mischief, spring-damper physics, the `rules.toml` loader, config (README-SP keys), and the locomotion/reaction state machine. Unit-tested, no I/O |
| `crates/lillith-anim` | lib | Atlas + `manifest.toml` loader, clip player (~12 fps, loop/once, feet anchors), procedural gestures ported from `aether_animator`, particles. CPU compositing with `tiny-skia` |
| `crates/lillith-overlay` | lib | The avatar window: sctk layer-shell (`wayland` feature), winit + softbuffer fallback (`x11` feature), alpha-mask input region / hit-test, drag, speech bubbles (`text` feature, cosmic-text), portals (`portal` feature, ashpd) |
| `crates/lillith-sense` | lib | Monitors → event bus: `sysinfo` (CPU/RAM), `zbus` (UPower, NetworkManager, UDisks2, Notifications, ScreenSaver), `notify` (trash, screenshots, archives, backlight), KWin bridge receiver. Optional: `bluetooth` (bluer), `pulse` (libpulse), `journal` (systemd), `clipboard` (wl-clipboard-rs), `x11` (x11rb) |
| `crates/lillith-mind` | lib | Memory (`rusqlite`, same schema as `lillith_memory_sqlite.py`), LLM client for Ollama and OpenAI-compatible `llama-server` (`reqwest`), read-only agent tools with approval gating |
| `crates/lillith-voice` | lib | TTS chain piper → espeak-ng → spd-say, push-to-talk STT (pw-record/arecord + whisper.cpp), SFX via `rodio` (`audio` feature) |
| `crates/lillith-panel` | lib | `eframe`/`egui` window: Chat, Settings, Poses, Events (debug injector) |
| `crates/lillith-tray` | lib | `ksni` StatusNotifierItem tray menu |
| `apps/lillith` | bin | Main binary: CLI flags, tokio runtime, wiring |
| `tools/slice-sheets` | bin | Offline asset pipeline: sprite sheets + pose stills → atlases + `manifest.toml` |
| `kwin/lillith-bridge` | KWin script | JS that pushes window added/removed/activated/minimized, geometry and cursor position to `org.lillith.Bridge` |
| `assets/rules.toml` | data | Event → pose / gesture / line / mood / SFX table and key map (from FEATURES.md §4) |
| `assets/source/` | data | Curated original art (**not committed yet**, tracked with Git LFS once added) |
| `docs/` | docs | `PLAN.md` (build plan), `FEATURES.md` (feature list) |

## Building

Requirements: Rust **1.85+** (`rustup` or the distro `rust` package), a C compiler (for bundled SQLite), and `pkg-config`.

```fish
# Default features: pure-Rust parts only, no system libraries needed
cargo build
cargo test --workspace

# Try it (M0): resolve events through the rule table, list rules, open the panel
cargo run -p lillith -- event battery.low
cargo run -p lillith -- event notify.received "New mail from Mom"
cargo run -p lillith -- rules
cargo run -p lillith -- panel
```

Optional features need system libraries. On Garuda/Arch:

```fish
sudo pacman -S --needed libxkbcommon wayland dbus libpulse systemd-libs alsa-lib
cargo build -p lillith --features full      # wayland + x11 + bluetooth + pulse + journal + clipboard + audio + portal
```

| Feature (on `lillith`) | Enables |
|---|---|
| `wayland` | layer-shell overlay (smithay-client-toolkit) |
| `x11` | winit/softbuffer fallback window |
| `full` | all of the above plus Bluetooth, volume, journald, clipboard, X11 window list, audio, portals, text shaping |

### KWin bridge (M5)

```fish
kpackagetool6 --type=KWin/Script -i kwin/lillith-bridge
# then enable "Lillith Bridge" in System Settings → Window Management → KWin Scripts
```

### Assets

The heavy art (6 sprite sheets, 52 pose stills, reference sheets, 80 video frames; about 43 MB) is not committed yet. `.gitattributes` already routes `assets/**/*.png|jpg|…` through **Git LFS**, so run `git lfs install` before adding art under `assets/source/`. `tools/slice-sheets` will turn it into `assets/atlas/` (generated, git-ignored).

## CI

`.github/workflows/ci.yml` runs on every push to `main` and every PR (the workflow file lands in a follow-up commit, because pushing workflows needs a token with the `workflow` scope):
- `cargo fmt --check`, `cargo clippy -D warnings` and `cargo test` with default features;
- `cargo clippy --all-features` with the system libraries installed.

## Roadmap

| # | Milestone |
|---|---|
| M0 | Workspace + asset pipeline ← *this scaffold* |
| M1 | Overlay on KDE Wayland (transparent, on top, click-through, drag) + X11 fallback |
| M2 | Animation engine (clips, springs, breathing, blink, idle transitions, walk) |
| M3 | Interactions & state machine (clicks, hover/peek, keys, moods, mischief, sleep/wake) |
| M4 | System monitor v1 (CPU, RAM, battery, network, volume, brightness, USB, notifications…) |
| M5 | KWin bridge (window reactions, perching, Follow mode) |
| M6 | Tray, panels, config, autostart + PKGBUILD |
| M7 | Memory + local LLM chat |
| M8 | Voice (TTS + push-to-talk) |
| M9 | Proactive life & polish |

Details and acceptance criteria are in [`docs/PLAN.md` §5](docs/PLAN.md).
