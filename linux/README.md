# Lillith on Arch / Garuda Linux

One command installs Lillith for your user (no root except for `pacman`):

```fish
./linux/install.sh
```

It asks a few short questions and is safe to run again; re-running upgrades
in place. `./linux/install.sh --yes` takes every default without asking.

## Layout

| File | What it is |
|---|---|
| `install.sh` | Per-user installer, upgrader and uninstaller (`--uninstall`) |
| `uninstall.sh` | Same as `install.sh --uninstall` |
| `stage-assets.py` | Checks and copies the animation set (manifest + atlas pages); spots Git LFS placeholders |
| `PKGBUILD`, `lillith.install`, `lillith.desktop` | Optional system-wide package: `cd linux && makepkg -si` |

## What `install.sh` does

1. **Checks the animation set** (`assets/generated`, or `--assets PATH`).
2. **Installs packages** with `sudo pacman -S --needed`: base-devel git pkgconf
   libxkbcommon wayland dbus libpulse systemd-libs alsa-lib python, plus `rust`
   if you have no cargo/rustup. It never installs anything from the AUR.
3. **Builds** `cargo build --release --locked -p lillith -p lillith-settings-gui`.
4. **Installs** the binaries to `~/.local/bin`, the art to
   `~/.local/share/lillith/generated` (found automatically, so you don't need
   `LILLITH_MANIFEST`), the icon set to `~/.local/share/lillith/icons`, the
   chosen icon (`--icon ID`, else `icon` in config.toml, else Phantomcore Red)
   to the hicolor theme, and a menu entry.
5. **Desktop shortcut** (default yes) in your desktop folder (`xdg-user-dir
   DESKTOP`), marked trusted. **Autostart** (default no).
6. **KWin bridge** (window perching) via `kpackagetool6` + `kwriteconfig6`.
   Skipped quietly when you're not on Plasma 6. Then, if a KDE force-blur
   effect (Better Blur DX `better_blur_dx`, Better Blur `forceblur`) is on and
   would blur her, asks (default yes) to **exclude her window class `lillith`**
   from it via `kwin-blur-exclude.sh apply`; this is what draws the dark box
   around her. Only if that is impossible (a `/regex/` in its class list) does
   it offer to turn the effect off. Both are undone by `--uninstall`.
7. **Brain**: finds a `.gguf` under `~` (Dolphin 3.0 Llama 3.2 3B Q4_K_M first)
   and `llama-server`, then runs `lillith setup`. The endpoint is fixed to
   `http://127.0.0.1:8080`. Nothing is downloaded, and no model is ever fetched.
   If `llama-server` is missing, install `llama.cpp` (pacman, or an AUR helper
   yourself) and run `lillith setup --gguf … --llama-server …` later.
8. Checks `lillith --help` and the clip list, then asks **Start Lillith now?**

## Options

```
-y, --yes          take every default            --uninstall [--purge]
--skip-deps        don't run pacman              --skip-build
--assets PATH      art folder or release .zip    --gguf FILE  --llama-server BIN
--ctx-size N       --autostart/--no-autostart    --desktop-shortcut/--no-desktop-shortcut
--no-kwin          --start/--no-start
```

`./linux/install.sh --help` lists them all.

## From the GitHub "Download ZIP" (main.zip)

GitHub's ZIP has **Git LFS placeholders instead of the character art**, and
the installer stops with an explanation. Give it the art from the release's
Windows portable zip (it carries the same `assets/generated`):

```fish
cd ~/Downloads
unzip Lillith-Desktop-Avatar-main.zip
cd Lillith-Desktop-Avatar-main
bash linux/install.sh --assets ~/Downloads/Lillith-0.0.1-windows-x64.zip
```

Or use a git clone with LFS (`sudo pacman -S git-lfs`):

```fish
git clone https://github.com/kokannadian-killa/Lillith-Desktop-Avatar.git
cd Lillith-Desktop-Avatar
git lfs install; git lfs pull
./linux/install.sh
```

(`bash linux/install.sh` works if unzip dropped the executable bit.)

## Uninstall

```fish
./linux/uninstall.sh            # asks before deleting settings + chat history
./linux/uninstall.sh --purge    # also deletes them, no questions
```

This removes the binaries, art, icons, menu entry, desktop shortcut, autostart
entry and KWin script, and reverts the Better Blur DX exclusion it added
(state in `~/.local/share/lillith/kwin-blur-exclusion.state`).

## Dark box around her (KDE Better Blur DX)

```fish
lillith fix-kwin --check               # does a force-blur effect target her?
lillith fix-kwin                       # exclude `lillith` from it (kwinrc)
./linux/kwin-blur-exclude.sh apply     # same; `revert` undoes it, `disable` turns the effect off
# last resort, turns it off for every window:
qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.unloadEffect better_blur_dx
kwriteconfig6 --file kwinrc --group Plugins --key better_blur_dxEnabled false
```

kwinrc keys (from `src/blur.kcfg` of xarblu/kwin-effects-better-blur-dx and
taj-ny/kwin-effects-forceblur): `[Plugins] better_blur_dxEnabled` /
`forceblurEnabled`; class list `WindowClasses` (one per line) in
`[Effect-better-blur-dx]` (older Better Blur DX and Better Blur:
`[Effect-blurplus]`); `BlurMatching=false` means "blur all except listed".
Tests: `bash linux/tests/kwin-blur-exclude-test.sh`. It never touches your `.gguf` models or `llama-server`.

## App icon

Lillith ships 15 icons (`branding/icons.toml`; default `phantomcore-red`,
plus `classic`, the old idle-frame icon). Pick one in **Settings ▸ Icon**
(right-click her ▸ **Icon…**), or:

```fish
lillith icon --list             # ids, * = current
lillith icon glitch             # saves icon = "glitch" and updates the menu entry + desktop shortcut
./linux/install.sh --icon glitch   # same, at install time
```

Switching rewrites `~/.local/share/icons/hicolor/<N>x<N>/apps/lillith.png`,
touches the `.desktop` files and runs `gtk-update-icon-cache` /
`kbuildsycoca6` when present. Some docks cache icons until you log out.

## Privacy

Chat runs on your own `llama-server` over loopback (`127.0.0.1`). No cloud, no
telemetry, no model downloads. The installer only talks to the network through
`pacman` (and `cargo` fetching the locked crate sources for the build).
