#!/usr/bin/env bash
# lillith-py installer for Arch / Garuda.
#  * The network is used ONLY by pacman here (after you confirm), never at runtime.
#  * Never enables or starts systemd units/timers: it copies them and prints the
#    `systemctl --user` commands for you to run.
#  * Never writes Lillith's config.toml (the Rust app owns it); it only reads it.
#
# Usage: ./install.sh [--yes] [--no-deps]
set -euo pipefail
SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEST="${LILLITH_PY_DIR:-$HOME/.local/lib/lillith-py}"
BIN="$HOME/.local/bin"
UNITS="$HOME/.config/systemd/user"
YES=0 DEPS=1
for a in "$@"; do
    case "$a" in
        --yes|-y) YES=1 ;;
        --no-deps) DEPS=0 ;;
        -h|--help) sed -n '2,9p' "$0"; exit 0 ;;
        *) echo "unknown option: $a" >&2; exit 2 ;;
    esac
done

PKGS=(python poppler tesseract tesseract-data-eng python-pillow libnotify python-pytest)
if (( DEPS )) && command -v pacman >/dev/null; then
    echo "Step 1/3: system packages. This will run:"
    echo "    sudo pacman -S --needed ${PKGS[*]}"
    if (( ! YES )); then
        read -r -p "Run it now? [y/N] " ans
        [[ "$ans" =~ ^[Yy]$ ]] || { echo "Skipped pacman (re-run with --no-deps to skip this prompt)."; DEPS=0; }
    fi
    (( DEPS )) && sudo pacman -S --needed "${PKGS[@]}"
fi

echo "Step 2/3: copying lillith-py to $DEST"
mkdir -p "$DEST" "$BIN" "$UNITS"
rm -rf "$DEST/lillith_py"
cp -r "$SRC/lillith_py" "$SRC/bin" "$SRC/tests" "$SRC/README.md" "$SRC/pyproject.toml" "$DEST/"
find "$DEST" -name __pycache__ -prune -exec rm -rf {} +
ln -sf "$DEST/bin/lillith-attach" "$BIN/lillith-attach"
ln -sf "$DEST/bin/lillith-bot" "$BIN/lillith-bot"

echo "Step 3/3: copying systemd user units to $UNITS (not enabled)"
cp "$SRC"/systemd/lillith-py-*.service "$SRC"/systemd/lillith-py-*.timer "$UNITS/"
mkdir -p "${XDG_DATA_HOME:-$HOME/.local/share}/lillith/attachments" \
         "${XDG_DATA_HOME:-$HOME/.local/share}/lillith/bots"

CFG="${XDG_CONFIG_HOME:-$HOME/.config}/lillith/config.toml"
cat <<MSG

Installed. Commands: $BIN/lillith-attach, $BIN/lillith-bot (~/.local/bin must be on PATH)
Model settings come from $CFG ([llm] endpoint/model, else [chat];
fallback http://127.0.0.1:8080). Optional [sidecar] port/workers/stall_minutes.

Nothing was enabled. When you want them, run:
    systemctl --user daemon-reload
    systemctl --user enable --now lillith-py-api.service           # API for the Bots tab
    systemctl --user enable --now lillith-py-scheduler.timer       # routines, once a minute
  (or, instead of the timer:)
    systemctl --user enable --now lillith-py-scheduler-daemon.service
The API token is created on first start at ${XDG_DATA_HOME:-$HOME/.local/share}/lillith/api.token (0600).
MSG
