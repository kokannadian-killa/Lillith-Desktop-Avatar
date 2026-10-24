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
#   1. If this is GitHub's "Download ZIP" (main.zip), her art is only Git LFS
#      placeholders: it downloads the real files from your GitHub repo
#      (about 530 MB, needs `gh auth login` once; the repo is private).
#   2. Installs the build tools and libraries with pacman (sudo).
#   3. Builds Lillith (first time: about 10-20 minutes on a laptop).
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

# Default model: the one Ko already uses, if it's on disk. (Otherwise
# linux/install.sh searches your home folder and lets you pick.)
if ! has --gguf; then
  for m in "${LILLITH_GGUF:-}" \
           "$HOME/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf" \
           "$HOME/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf"; do
    if [[ -n $m && -f $m ]]; then
      echo "Using your model: $m   (other one: bash install.sh --gguf /path/to/model.gguf)"
      args+=(--gguf "$m"); break
    fi
  done
fi

# Build in this folder's target/ unless told otherwise.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

exec bash "$ROOT/linux/install.sh" "${args[@]}"
