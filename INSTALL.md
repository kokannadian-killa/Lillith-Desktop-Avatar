# Installing Lillith

This guide covers building Lillith from source, running her, configuring her,
and fixing the usual overlay problems. For what she does, see the
[README](README.md).

- [Tested environment](#tested-environment)
- [1. Prerequisites](#1-prerequisites)
- [2. Clone and build](#2-clone-and-build)
- [3. Point her at an animation set](#3-point-her-at-an-animation-set)
- [4. Run](#4-run)
- [5. Configuration](#5-configuration)
- [6. Troubleshooting](#6-troubleshooting)

## Tested environment

| | |
|---|---|
| Distro | **Garuda Linux** (Arch-based) |
| Desktop | **KDE Plasma 6 on Wayland** (X11 / XWayland fallback) |
| Shell | **fish** (bash equivalents are given alongside) |
| Toolchain | Rust **1.85+** |

Other Linux distributions work as long as the libraries below are installed.
On Wayland you need a compositor that implements **`wlr-layer-shell`** (KWin,
Sway, Hyprland and other wlroots compositors do; GNOME/Mutter does not — there
she falls back to X11/XWayland automatically). On a plain X11 session the X11
backend is used directly; a compositing window manager is needed for
transparency.

> Status: the X11 path is verified; the Wayland layer-shell path is
> implemented but still awaiting its first on-machine Plasma 6 smoke test. If
> anything misbehaves on Wayland, try `--backend x11` (see
> [Troubleshooting](#6-troubleshooting)).

## 1. Prerequisites

### Rust 1.85 or newer

```fish
# via rustup (recommended)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup update stable
rustc --version   # must be ≥ 1.85
```

On Arch/Garuda you can instead use the distro package (`sudo pacman -S rust`)
or `sudo pacman -S rustup && rustup default stable`.

### System libraries

You also need a C compiler (SQLite is built from source via `rusqlite`'s
`bundled` feature) and `pkg-config`. The full list below is what CI installs
for the all-features build ([`.github/workflows/ci.yml`](.github/workflows/ci.yml));
the default build (both overlay backends + the egui panel) mostly needs
`libxkbcommon` and `wayland`, which every Plasma install already has.

**Arch / Garuda / Manjaro (pacman):**

```fish
sudo pacman -S --needed base-devel git pkgconf libxkbcommon wayland dbus libpulse systemd-libs alsa-lib
```

**Debian / Ubuntu (apt)** — exactly the CI list:

```sh
sudo apt-get install -y --no-install-recommends build-essential git \
  pkg-config libxkbcommon-dev libwayland-dev libdbus-1-dev \
  libpulse-dev libsystemd-dev libasound2-dev
```

**Fedora (dnf)** — untested equivalents:

```sh
sudo dnf install gcc git pkgconf-pkg-config libxkbcommon-devel wayland-devel \
  dbus-devel pulseaudio-libs-devel systemd-devel alsa-lib-devel
```

| CI (Debian) package | Arch package | Needed for |
|---|---|---|
| `pkg-config` | `pkgconf` | locating the libraries below |
| `libxkbcommon-dev` | `libxkbcommon` | Wayland keyboard input, egui panel |
| `libwayland-dev` | `wayland` | Wayland overlay + panel |
| `libdbus-1-dev` | `dbus` | D-Bus monitors, tray (feature `full`) |
| `libpulse-dev` | `libpulse` | volume monitor (`lillith-sense/pulse`) |
| `libsystemd-dev` | `systemd-libs` | journald monitor (`lillith-sense/journal`) |
| `libasound2-dev` | `alsa-lib` | audio / SFX (`lillith-voice/audio`) |

## 2. Clone and build

```fish
git clone https://github.com/kokannadian-killa/Lillith-Desktop-Avatar.git
cd Lillith-Desktop-Avatar

# Release build of the app (default features: Wayland + X11 backends)
cargo build --release -p lillith

# Optional: everything (Bluetooth, volume, journald, clipboard, X11 window
# list, audio, portals, text shaping) — needs all the libraries above
cargo build --release -p lillith --features full
```

The binary lands in `./target/release/lillith`. To put it on your `PATH`:

```fish
cargo install --path apps/lillith --locked     # → ~/.cargo/bin/lillith
```

| `lillith` feature | Enables |
|---|---|
| `wayland` *(default)* | layer-shell overlay (smithay-client-toolkit) |
| `x11` *(default)* | winit + softbuffer fallback window (X11/XWayland) |
| `full` | all of the above + Bluetooth, volume, journald, clipboard, X11 window list, audio, portals, text shaping |

## 3. Point her at an animation set

The character art is not committed to the repo yet (it will arrive via Git
LFS), so you need a local generated set with a `manifest.toml`. Lillith looks
for it in this order:

1. `--manifest <path>`
2. `$LILLITH_MANIFEST`
3. `./assets/generated/manifest.toml`
4. `~/.local/share/lillith/generated/manifest.toml`

If you only have a `manifest.json` from the image pipeline, convert it first:

```fish
cargo run -p import-manifest -- --input /path/to/generated/manifest.json --tag canonical --check
```

Set the variable for your session:

```fish
# fish
set -x LILLITH_MANIFEST /path/to/generated/manifest.toml
# make it permanent (universal variable):
set -Ux LILLITH_MANIFEST /path/to/generated/manifest.toml
```

```bash
# bash / zsh
export LILLITH_MANIFEST=/path/to/generated/manifest.toml
```

Or copy the set so no variable is needed:

```fish
mkdir -p ~/.local/share/lillith
cp -r /path/to/generated ~/.local/share/lillith/
```

## 4. Run

```fish
./target/release/lillith                    # Wayland if WAYLAND_DISPLAY is set, else X11
```

Stop her with `Q` / `Esc` (click her first so she has keyboard focus) or
`pkill lillith`.

### Flags

| Flag | Meaning |
|---|---|
| `--backend wayland\|x11` | Force a backend. Default: Wayland when `WAYLAND_DISPLAY` is set. Forcing `wayland` turns a missing layer-shell into an error instead of a fallback. |
| `--manifest <PATH>` | Animation set `manifest.toml` (see [§3](#3-point-her-at-an-animation-set)) |
| `--config <PATH>` | Config file (default `~/.config/lillith/config.toml`) |
| `--scale <F>` | Size multiplier on the 400 px cell (default `0.55` → 220 px) |
| `--opacity <0..1>` | Whole-sprite opacity |
| `--fps <HZ>` | Redraw cap (default 30) |
| `--mode wander\|stay\|follow` | Movement mode (default `wander`; `follow` currently behaves like `stay` — real Follow needs the M5 KWin bridge) |
| `--sleepy-after <SECONDS>` | Idle time until drowsy (default 300) |
| `--asleep-after <SECONDS>` | Idle time until asleep (default 900) |
| `--reset-position` | Ignore the saved position, start at the default spot |
| `--baked-jiggle` | Draw the baked `<clip>_jiggle` variants when the animation set has them (engine chest/pigtail/hip springs off while one plays); same as config `baked_jiggle = true` |
| `--no-save` | Don't write the position back to `config.toml` after a drag |
| `--exit-after <SECONDS>` | Quit after N seconds (smoke tests, CPU measurements) |
| `--no-tray`, `--no-voice`, `--no-llm` | Accepted, reserved for M6–M8 (tray, voice and LLM aren't wired yet) |

Run `lillith --help` for the authoritative list.

### Examples

```fish
# Smaller, slightly see-through, parked in place
./target/release/lillith --scale 0.6 --opacity 0.9 --mode stay

# Force the X11/XWayland fallback, lower redraw rate
./target/release/lillith --backend x11 --fps 15

# Demo the sleep cycle quickly without touching the saved position
./target/release/lillith --sleepy-after 10 --asleep-after 20 --no-save

# Debug logging (fish sets env vars per command with `env`)
env RUST_LOG=lillith=info,lillith_overlay=debug ./target/release/lillith
# bash: RUST_LOG=lillith=info,lillith_overlay=debug ./target/release/lillith
```

### Subcommands

```fish
lillith rules                              # list all event rules and their clips
lillith event battery.low                  # fire an event through the rule table
lillith event notify.received "New mail"   # …with a detail
lillith preview --list                     # list clips in the manifest
lillith preview knife_tap --physics        # render a clip through the animator → ./preview/
lillith preview --all --out preview        # contact sheet for every clip
lillith panel                              # egui window, Poses tab = clip browser
lillith settings                           # interim window: avatar import/anchors, jiggle/speed/chat (right-click ▸ Settings… opens lillith-settings)
lillith settings --section avatar --import ~/Pictures/me.png
```

Right-click her for the menu (poses, chat, avatar, resize, minimize,
settings, close). See [docs/MENU_SETTINGS_AVATAR.md](docs/MENU_SETTINGS_AVATAR.md).

## 5. Configuration

Config lives at **`~/.config/lillith/config.toml`** (or
`$XDG_CONFIG_HOME/lillith/config.toml`). Every key is optional; missing keys
use the defaults shown. CLI flags override the file. The `[overlay]` table is
rewritten when you drag her (other keys are kept).

```toml
# Top-level keys (must come before any [table])
mode = "wander"        # wander | stay | follow  (default wander; G toggles while she has focus)
scale = 1.0            # 1.0 here means "use the 0.55 default" (220 px); set e.g. 0.7 to override
opacity = 1.0
render_hz = 30.0
physics_hz = 120.0
baked_jiggle = false   # true: use the baked _jiggle clip variants when present
# jiggle_mode = "springs"  # baked | springs | off (overrides baked_jiggle)
always_on_top = true
click_through = false  # true: only her head takes clicks
autostart = false      # true: writes ~/.config/autostart/lillith.desktop
muted = false

[anim]
playback_speed = 0.7
talk_speed = 0.62
smooth_frames = true   # cross-fade consecutive sprite frames
crossfade = 0.5

[avatar]               # custom single-image avatars (Settings ▸ Avatar)
pace = 0.8
render_hz = 60.0

[overlay]              # where she sits (px from the bottom-left corner)
margin_left = 80
margin_bottom = 48
# output = "eDP-1"     # stored, not applied yet

[behavior]             # M3 behaviour timings
sleepy_after_s = 300.0
asleep_after_s = 900.0
pose_cycle_s = 45.0          # automatic pose change while idle
left_repeat_s = 4.0          # second left click within this → wave
wander_every_s = [20.0, 45.0]   # pause between calm walks in wander mode
wander_speed_px_s = 45.0        # glide speed for walk takes without root motion
wander_min_step_px = 120.0
event_wake_priority = 80     # events at/above this priority wake her
mood_tau_s = 45.0

[behavior.idle]              # idle life: rotating flirty poses, never the same twice in a row
breath_period_s = [3.5, 4.5]
blink_every_s = [4.0, 6.0]
transition_every_s = [8.0, 15.0]   # hip sway / hip lean / shimmy / tail curl …
transition_len_s = [1.2, 2.0]
reaction_every_s = [15.0, 30.0]    # unprompted gestures: idle_flirt_*, hip_pop_wink, blow_kiss …
variant_hold_s = 12.0              # keep one idle take at least this long (0 = every loop)
walk_variant_hold_s = 1.0          # plain walk this long before a sultry take (saunter / sassy / strut)

[behavior.gestures]
double_click_s = 0.35
secret_window_s = 3.0
hover_long_s = 1.2

[behavior.mischief]
poke = 0.15
decay_tau_s = 60.0
high = 0.7
boredom_s = 150.0
```

Spring tuning lives in a `[springs]` table, and the tuned jiggle physics in
[`assets/physics.toml`](assets/physics.toml). Event reactions and the key map
are data, in [`assets/rules.toml`](assets/rules.toml).

## 6. Troubleshooting

**She doesn't appear / "no manifest.toml found".** Pass `--manifest` or set
`LILLITH_MANIFEST` ([§3](#3-point-her-at-an-animation-set)). If only
`manifest.json` exists, run `import-manifest` as shown.

**Wayland layer-shell vs. X11 fallback.** On Wayland she uses
`zwlr_layer_shell_v1` (Overlay layer), supported by KWin and wlroots-based
compositors. If the compositor lacks it (e.g. GNOME), or `WAYLAND_DISPLAY`
isn't set, she falls back to an X11/XWayland window automatically; the log
says which backend was chosen. Use `--backend x11` to force the fallback.

**She sits below a fullscreen app (X11 fallback).** Add a KWin window rule
*Keep above* for window class `lillith` (System Settings → Window Management →
Window Rules). KWin rules don't apply to layer-shell surfaces.

**A black box around her.** Her window is transparent only while a
compositor blends it. On Wayland (layer-shell) KWin always does. On X11 the
startup log has a `transparency:` line: `alpha=Composited` means a 32-bit ARGB
visual and a running compositing manager (soft edges); `alpha=Shaped` means
compositing is off or suspended (KWin X11: Alt+Shift+F12, or System Settings →
Display & Monitor → Compositor), so she is clipped to her silhouette with the
X Shape extension instead — no box, but hard edges. She switches modes live
within ~2 s when compositing is toggled. Include that log line in bug reports.

**Build fails with `xkbcommon` / `wayland-client` / `Package … was not found`.**
Install `libxkbcommon` and `wayland` (Arch) or `libxkbcommon-dev` and
`libwayland-dev` (Debian/Ubuntu) and `pkg-config`/`pkgconf`. The egui panel
(`lillith panel`) also needs `libxkbcommon` at runtime on Wayland. For
`--features full`, install the whole list from [§1](#1-prerequisites).

**Keys do nothing.** Click her first — she only gets keyboard focus on demand.

**Clicks next to her hit her instead of the window below.** Only pixels with
alpha ≥ 40 are part of her input region; report it with
`RUST_LOG=lillith_overlay=debug` output.

**High CPU.** Lower the redraw cap (`--fps 15`); she should stay around a few
percent of one core at idle.
