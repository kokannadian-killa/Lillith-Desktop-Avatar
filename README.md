# Lillith Desktop Avatar

Interactive desktop avatar and system state and action monitor.

**Lillith** is a local desktop companion for **Garuda / Arch Linux on KDE Plasma 6 (Wayland)**. She's a transparent, always-on-top anime demon-girl avatar who lives on your desktop, moves with spring physics, reacts to what you and your system are doing (CPU load, battery, Wi-Fi, USB, notifications, windows opening and closing…), and chats with you through a fully local LLM, with her own memory, voice and tray controls.

This repository is the **pure-Rust rewrite** of the earlier Python prototypes (Tk/pygame/psutil). It has reached **milestone M1 + M2 + M3**: she runs on the desktop as a transparent, always-on-top, click-through overlay (Wayland layer-shell, X11/XWayland fallback) playing the M2 animation engine (clip player, spring physics, breathing, blink, idle transitions, walk/run with flip, reaction queue). Drag her anywhere; the spot is remembered. M3 adds the interaction state machine: click/double/right/middle reactions, hover and peek, README-SP keys, the secret sequence, moods, the mischief meter, sleep/wake and Wander/Stay. Sensors and chat are still stubs. See [`docs/PLAN.md`](docs/PLAN.md) for the full build plan and [`docs/FEATURES.md`](docs/FEATURES.md) for every feature and its status.

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
| `crates/lillith-anim` | lib | Atlas + `manifest.toml` loader, clip player (fps or per-frame ms, loop/once, feet anchors), **animator** (base + idle life + reaction queue + springs), renderer and headless preview (`tiny-skia`), gestures, particles |
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
# Default features: both overlay backends (Wayland + X11) and the egui panel
cargo build
cargo test --workspace

# Run her (M1): see "Running on Garuda" below
cargo run --release -p lillith -- --manifest /path/to/generated/manifest.toml

# Resolve events through the rule table, list rules, open the panel
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
| `wayland` (default) | layer-shell overlay (smithay-client-toolkit) |
| `x11` (default) | winit/softbuffer fallback window (X11/XWayland) |
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
| M0 | Workspace + asset pipeline ✔ (CI workflow still pending) |
| M1 | Overlay on KDE Wayland (transparent, on top, click-through, drag) + X11 fallback ✔ X11 verified, Wayland needs an on-machine smoke test |
| M2 | Animation engine (clips, springs, breathing, blink, idle transitions, walk) ✔ |
| M3 | Interactions & state machine (clicks, hover/peek, keys, moods, mischief, sleep/wake, Wander/Stay) ✔ X11 verified ← *current* |
| M4 | System monitor v1 (CPU, RAM, battery, network, volume, brightness, USB, notifications…) |
| M5 | KWin bridge (window reactions, perching, Follow mode) |
| M6 | Tray, panels, config, autostart + PKGBUILD |
| M7 | Memory + local LLM chat |
| M8 | Voice (TTS + push-to-talk) |
| M9 | Proactive life & polish |

Details and acceptance criteria are in [`docs/PLAN.md` §5](docs/PLAN.md).

## Running on Garuda (M1 overlay + M3 behaviour)

```fish
sudo pacman -S --needed rust libxkbcommon wayland        # usually already there on Plasma
cargo build --release -p lillith
set -x LILLITH_MANIFEST /path/to/generated/manifest.toml  # or copy the set to ~/.local/share/lillith/generated/
./target/release/lillith                                  # Plasma Wayland → layer-shell overlay
./target/release/lillith --backend x11                    # force the XWayland/X11 fallback
```

- **Move her:** hold the left button on her body and drag. On release the spot is written to `~/.config/lillith/config.toml` (`[overlay] margin_left / margin_bottom`, px from the bottom-left corner) and restored next start. `--reset-position` ignores it, `--no-save` never writes it.
- **Mouse (M3):** left click = knife tap (a second click within 4 s waves, then it alternates; when you've been teasing her a lot it's always the knife, in Mischievous mood); double click = hacking; right click = cleaning + pose-menu stub (right-click **no longer quits**); middle click = help; hovering her = sit-swing, keep hovering ~1.2 s = peek; drag = `drag_held`.
- **Secret:** click once, then double-click, then right-click, all within 3 s → tail wag + "You found me out~" (30 s cooldown).
- **Keys** (click her first so she has keyboard focus): the README-SP map (`T` tail, `K` hack, `H` help, `R` random, `C` cycle pose, `M` mute, `S` sleep, `W`/`V`/`B`/`A`/`E`/`F`/`P`/`0`–`9`/`D`/`N`/`U` system poses), plus `G` = toggle Wander/Stay, `Y`/`Enter` = chat (M7 stub), **`Q`/`Esc` = quit**. Or `pkill lillith`.
- **Life:** pose cycle every 16 s when idle; drowsy after 5 min without interaction, asleep after 15 min (hover or click wakes her with the wake clip); ignored for ~2.5 min she plays a prank. `--mode wander|stay` (default config `mode`, wander): in Wander she walks to a new spot along the bottom every 20–45 s. Demo the sleep cycle quickly with `--sleepy-after 10 --asleep-after 20`. Timings live in `[behavior]` in config.toml.
- **Click-through:** only her opaque pixels take the mouse; everything around her goes to the window or desktop underneath.
- **Size / look:** `--scale 0.6` (default 0.75 → 300 px window from the 400 px cells; or `scale = 0.6` in config.toml), `--opacity 0.9`, `--fps 15` (redraw cap; default 30).
- **Backend choice:** Wayland if `WAYLAND_DISPLAY` is set, else X11. If the compositor has no `zwlr_layer_shell_v1` (e.g. GNOME) she falls back to X11/XWayland automatically; `--backend wayland` makes that an error instead.
- **Smoke test checklist on Plasma 6 Wayland** (not verifiable on the build box): she shows above normal and maximised windows; clicking right next to her clicks the desktop/window below; drag is smooth and the position survives a restart; `top -p (pgrep lillith)` stays under ~3 % idle. If she ever sits below a fullscreen app, run `--backend x11` and add a KWin window rule "Keep above" for window class `lillith` (KWin rules don't apply to layer-shell surfaces).
- **Click-through probe (X11):** `cargo run -p lillith-overlay --features x11 --example x11_click_probe -- 0 500 900 600` opens a striped window that prints every click it gets; put it behind her and click around.
- Logs: `RUST_LOG=lillith=info,lillith_overlay=debug ./target/release/lillith`. On exit she logs `renders` and `avg_render_us`.

## Previewing the animation engine (M2)

The generated art set is not in git yet; point the tools at its `manifest.toml` (`--manifest`, or `LILLITH_MANIFEST`, or `assets/generated/manifest.toml`, or `~/.local/share/lillith/generated/manifest.toml`):

```sh
export LILLITH_MANIFEST=/path/to/generated/manifest.toml
cargo run -p lillith -- preview --list                       # all clips: frames, fps, mode, length, metadata
cargo run -p lillith -- preview knife_tap                     # raw frames → preview/knife_tap.png
cargo run -p lillith -- preview knife_tap --physics --frames  # through the animator with springs, + per-frame PNGs
cargo run -p lillith -- preview --all --out preview           # overview.png + one contact sheet per clip
cargo run -p lillith -- preview --sim 60 --seed 3             # idle-life timeline (blinks, transitions, idle reactions)
cargo run -p lillith -- panel                                 # egui window, Poses tab: list/play/step/react/walk/jiggle
```

Physics: one spring-damper per channel (root x/y, angle, scale, chest, pigtails; README-SP stiffness/damping and gains, 120 Hz semi-implicit Euler, velocity and offset clamps). Breathing drives the scale/chest targets; reactions start with an impulse sized by energy tier (`knife_tap`, `dance`, `event_cpu_busy`… hit hardest); walk/run/dance add footfall bounces; starting/stopping a walk tips the angle spring. With single-image frames the chest and pigtail channels render as a subtle whole-sprite squash and skew until layered art exists.
