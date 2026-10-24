#!/usr/bin/env bash
# Lillith core installer for Arch / Garuda. Network is used ONLY here (pacman),
# never at runtime.
set -euo pipefail
SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEST="${LILLITH_CORE_DIR:-$HOME/.local/share/lillith/core}"
BIN="$HOME/.local/bin"
UNITS="$HOME/.config/systemd/user"

if [[ "${1:-}" != "--no-deps" ]] && command -v pacman >/dev/null; then
    sudo pacman -S --needed --noconfirm python poppler tesseract tesseract-data-eng \
        python-pillow libnotify python-pytest
fi

mkdir -p "$DEST" "$BIN" "$UNITS"
rm -rf "$DEST/lillith_core"
cp -r "$SRC/lillith_core" "$SRC/bin" "$SRC/tests" "$SRC/README.md" "$SRC/pyproject.toml" "$DEST/"
find "$DEST" -name __pycache__ -prune -exec rm -rf {} +
ln -sf "$DEST/bin/lillith-attach" "$BIN/lillith-attach"
ln -sf "$DEST/bin/lillith-bot" "$BIN/lillith-bot"
cp "$SRC"/systemd/*.service "$SRC"/systemd/*.timer "$UNITS/"
systemctl --user daemon-reload 2>/dev/null || true

mkdir -p "$HOME/.local/share/lillith/attachments" "$HOME/.config/lillith"
[[ -f "$HOME/.config/lillith/attachments.json" ]] || cat > "$HOME/.config/lillith/attachments.json" <<JSON
{
  "llama_url": "http://127.0.0.1:8080",
  "token_budget": 2048,
  "ocr": true,
  "vision": false
}
JSON

echo "Installed to $DEST"
echo "Commands: $BIN/lillith-attach, $BIN/lillith-bot (make sure ~/.local/bin is on PATH)"
echo "Enable routines:  systemctl --user enable --now lillith-scheduler.timer"
echo "Enable API:       systemctl --user enable --now lillith-api.service"
