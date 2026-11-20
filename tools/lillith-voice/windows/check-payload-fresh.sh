#!/usr/bin/env bash
# Refuse a stale Windows voice payload.
#
#     tools/lillith-voice/windows/check-payload-fresh.sh <payload.zip | dir holding voice/>
#
# The payload's lib\lillith_voice\ must match tools/lillith-voice/lillith_voice/ in
# this tree, file for file (*.py; __pycache__ ignored). A payload built from an older
# tree ships an old daemon: e.g. one without micgate.py/launchguard.py has no mic
# consent card and no safeword capture-drop. Rebuild it with build-voice-win.sh.
# Exit 0 = fresh, 1 = stale (lists the files), 2 = usage/not a payload.
# VOICE_ALLOW_STALE=1 turns a stale result into a warning (exit 0).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KIT_PKG="${LILLITH_VOICE_PKG:-$HERE/../lillith_voice}"
src="${1:-}"
[[ -n "$src" && -e "$src" ]] || { echo "usage: $0 <lillith-voice-win-*.zip | dir with voice/>" >&2; exit 2; }
tmp=""
trap '[[ -n "$tmp" ]] && rm -rf "$tmp"' EXIT
if [[ -f "$src" ]]; then
  tmp="$(mktemp -d)"
  python3 - "$src" "$tmp" <<'PY'
import sys, zipfile
z = zipfile.ZipFile(sys.argv[1])
z.extractall(sys.argv[2], [n for n in z.namelist() if (n.startswith("voice/lib/lillith_voice/") and n.endswith(".py")) or n == "voice/PAYLOAD_ID"])
PY
  src="$tmp"
fi
lib="$src/voice/lib/lillith_voice"
[[ -d "$lib" ]] || lib="$src/lib/lillith_voice"
[[ -d "$lib" ]] || { echo "error: no voice/lib/lillith_voice in $1" >&2; exit 2; }
list() { (cd "$1" && find . -name __pycache__ -prune -o -type f -name '*.py' -print | sed 's|^\./||' | LC_ALL=C sort); }
bad=()
while IFS= read -r f; do
  if [[ ! -f "$lib/$f" ]]; then bad+=("missing: $f")
  elif ! cmp -s "$KIT_PKG/$f" "$lib/$f"; then bad+=("differs: $f"); fi
done < <(list "$KIT_PKG")
while IFS= read -r f; do [[ -f "$KIT_PKG/$f" ]] || bad+=("extra:   $f"); done < <(list "$lib")
id="$(cat "$src/voice/PAYLOAD_ID" 2>/dev/null || true)"
if ((${#bad[@]})); then
  echo "error: voice payload ${id:+$id }is stale vs this tree's tools/lillith-voice/lillith_voice:" >&2
  printf '  %s\n' "${bad[@]}" >&2
  echo "  rebuild it: tools/lillith-voice/windows/build-voice-win.sh" >&2
  if [[ "${VOICE_ALLOW_STALE:-0}" == 1 ]]; then echo "warning: VOICE_ALLOW_STALE=1, continuing" >&2; exit 0; fi
  exit 1
fi
echo "voice payload ${id:+$id }matches this tree's lillith_voice"
