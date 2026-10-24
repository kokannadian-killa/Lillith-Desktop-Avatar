#!/usr/bin/env bash
# ┌──────────────────────────────────────────────────────────────────┐
# │  Lillith one-step installer for Linux (Garuda / Arch / Manjaro)  │
# │                                                                  │
# │  In the folder you unzipped (fish, bash or zsh all work):        │
# │      bash install.sh                                             │
# │  From anywhere:                                                  │
# │      bash ~/Downloads/Lillith-Desktop-Avatar-main/install.sh     │
# │  Remove her again:                                               │
# │      bash install.sh --uninstall                                 │
# └──────────────────────────────────────────────────────────────────┘
#
# What it does (asks before anything big):
#   1. The all-in-one Lillith-main-linux.zip from the release already has
#      her art, the programs and the offline voice kit: nothing to download.
#      (GitHub's "Download ZIP" has only Git LFS placeholders: then it
#      downloads the art from your GitHub repo, about 530 MB, `gh auth login`.)
#   2. Installs the libraries with pacman (sudo); the list is shown first.
#   3. Uses the bundled programs (bin/), else builds Lillith (10-20 min).
#   4. Installs her for your user: ~/.local/bin/lillith, her animations,
#      an app-menu entry, a desktop icon, the KWin bridge, the blur fix.
#   5. Points her at your local model (default
#      ~/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf) and llama-server,
#      loopback 127.0.0.1:8080 only. No model is ever downloaded.
#
# Every option of linux/install.sh works here too: bash install.sh --help
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

if [[ ! -f $ROOT/linux/install.sh || ! -f $ROOT/Cargo.toml ]]; then
  echo "error: this install.sh must stay in the unzipped Lillith folder (next to linux/ and Cargo.toml)" >&2
  exit 1
fi
if [[ ${1:-} == --help || ${1:-} == -h ]]; then
  sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'
  echo
  exec bash "$ROOT/linux/install.sh" --help
fi
if [[ $(uname -s) != Linux ]]; then
  echo "This is the Linux installer. On Windows run install.bat; on Android see INSTALL.md." >&2
  exit 1
fi

args=("$@")
has() { local a; for a in "${args[@]}"; do [[ $a == "$1" || $a == "$1="* ]] && return 0; done; return 1; }

# Uninstall: hand straight over.
if has --uninstall; then exec bash "$ROOT/linux/install.sh" "${args[@]}"; fi

# python3 runs the art check/download and the asset staging (stock on Garuda).
if ! command -v python3 >/dev/null 2>&1; then
  if command -v pacman >/dev/null 2>&1 && ! has --skip-deps; then
    echo "==> python3 is needed first: sudo pacman -S --needed python"
    sudo pacman -S --needed python
  else
    echo "error: python3 is needed (sudo pacman -S python)" >&2; exit 1
  fi
fi

# ffmpeg + ffprobe: her MP4 poses and Pose Builder's "Import Video" need
# them. A bundled bin/ffmpeg is used first if the zip has one; otherwise
# they come from your system. Never installed silently: the command is
# shown and the default answer is No (also with --yes / no terminal).
if [[ -x $ROOT/bin/ffmpeg && -x $ROOT/bin/ffprobe ]]; then
  echo "Using the bundled ffmpeg/ffprobe in $ROOT/bin"
elif ! command -v ffmpeg >/dev/null 2>&1 || ! command -v ffprobe >/dev/null 2>&1; then
  if command -v pacman >/dev/null 2>&1; then
    echo "==> ffmpeg/ffprobe are missing (needed to play poses and import your own videos)."
    echo "    To install them:  sudo pacman -S --needed ffmpeg"
    reply=""
    if [[ -t 0 ]]; then
      read -r -p "    Run that now? [y/N] " reply || reply=""
    fi
    if [[ ${reply,,} == y || ${reply,,} == yes ]]; then
      sudo pacman -S --needed ffmpeg
    else
      echo "    Skipped. Install it later with: sudo pacman -S --needed ffmpeg"
    fi
  else
    echo "==> ffmpeg/ffprobe are missing (needed to play poses and import your own videos)."
    echo "    Install the 'ffmpeg' package with your distro's package manager"
    echo "    (Debian/Ubuntu: sudo apt install ffmpeg   Fedora: sudo dnf install ffmpeg"
    echo "     openSUSE: sudo zypper install ffmpeg), or put ffmpeg and ffprobe in $ROOT/bin/."
  fi
fi

# Default model: the one Ko already uses, then any .gguf in ~/models.
# (Otherwise linux/install.sh searches your home folder and lets you pick.)
# Never downloaded: the zip has no model (bundle/models/README.txt).
if ! has --gguf; then
  found=""
  for m in "${LILLITH_GGUF:-}" \
           "$HOME/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf" \
           "$HOME"/models/*.gguf; do
    [[ -n $m && -f $m && ${m,,} != *mmproj* ]] || continue
    found="$m"; break
  done
  if [[ -n $found ]]; then
    echo "Using your model: $found   (other one: bash install.sh --gguf /path/to/model.gguf)"
    args+=(--gguf "$found")
  else
    echo "No model at ~/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf or ~/models/*.gguf (nothing is downloaded); the installer will search your home folder."
  fi
fi

# The all-in-one zip: ready-made programs in bin/ (skip the Rust build).
if ! has --prebuilt && ! has --skip-build && [[ -x $ROOT/bin/lillith ]] && "$ROOT/bin/lillith" --help >/dev/null 2>&1; then
  echo "Using the ready-made programs in $ROOT/bin (no build needed)"
  args+=(--prebuilt "$ROOT/bin")
fi

# Build in this folder's target/ unless told otherwise.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

exec bash "$ROOT/linux/install.sh" "${args[@]}"
