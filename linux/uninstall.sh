#!/usr/bin/env bash
# Removes Lillith's per-user install (binaries, launcher, desktop shortcut,
# autostart, icons, animation set, KWin bridge). Asks before deleting your
# settings and chat history (--purge: delete without asking). GGUF model
# files and llama.cpp are never touched. Same as ./linux/install.sh --uninstall.
set -euo pipefail
exec "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/install.sh" --uninstall "$@"
