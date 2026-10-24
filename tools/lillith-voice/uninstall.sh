#!/usr/bin/env bash
# Removes everything install.sh put in your home (keeps nothing, touches no system packages).
set -euo pipefail
LV_HOME="${LILLITH_VOICE_HOME:-${XDG_DATA_HOME:-$HOME/.local/share}/lillith-voice}"
CFG="${XDG_CONFIG_HOME:-$HOME/.config}"
systemctl --user disable --now lillith-voice.service 2>/dev/null || true
rm -f "$CFG/systemd/user/lillith-voice.service" "$CFG/fish/conf.d/lillith-voice.fish"
for b in lillith-say lillith-voiced lillith-chat lillith-piper lillith-listen; do rm -f "$HOME/.local/bin/$b"; done
grep -qs lillith "$HOME/.local/bin/piper" && rm -f "$HOME/.local/bin/piper"
rm -rf "$LV_HOME"
systemctl --user daemon-reload 2>/dev/null || true
echo "lillith-voice removed (pacman packages left alone)."
