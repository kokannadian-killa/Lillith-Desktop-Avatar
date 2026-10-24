#!/usr/bin/env bash
# Launch Lillith from this portable package.
#   ./run.sh                      -> Wayland (KDE Plasma layer-shell overlay)
#   ./run.sh --backend x11        -> force the X11/XWayland fallback instead
#   ./run.sh --fix-kwin           -> first exclude her from KDE force blur
#                                    (Better Blur DX: the dark box), then launch
#   ./run.sh status | rules | preview --list | panel   (subcommands work too)
# Any extra arguments are passed straight to the binary.
# LILLITH_FIX_KWIN=1 does the same as --fix-kwin.
set -euo pipefail
PKG="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" && pwd)"
cd "$PKG"
export LILLITH_MANIFEST="$PKG/assets/generated/manifest.toml"

# --fix-kwin is ours, not the binary's: take it out of the arguments.
fix_kwin="${LILLITH_FIX_KWIN:-0}"
rest=()
for a in "$@"; do
  if [[ "$a" == --fix-kwin ]]; then fix_kwin=1; else rest+=("$a"); fi
done
set -- ${rest[@]+"${rest[@]}"}

# Launching her = no arguments, or options only (`--backend x11`);
# `status`, `rules`, `preview --list`, `setup` … are subcommands.
launch=0
if [[ $# -eq 0 || "$1" == -* ]]; then launch=1; fi

# KDE force blur (Better Blur DX / Better Blur) paints a dark rounded panel
# behind her. With consent, exclude only her window class; otherwise check
# once (only when launching her) and print the fix.
if [[ "$fix_kwin" == 1 ]]; then
  "$PKG/bin/lillith" fix-kwin || true
elif [[ "$launch" == 1 ]] && command -v kreadconfig6 >/dev/null 2>&1; then
  marker="${XDG_STATE_HOME:-$HOME/.local/state}/lillith/kwin-blur-checked"
  if [[ ! -f "$marker" ]]; then
    report="$("$PKG/bin/lillith" fix-kwin --check 2>/dev/null || true)"
    if grep -q -e "would exclude" -e "regex" <<<"$report"; then
      printf '%s\n' "$report" \
        "  -> This draws a dark box around Lillith. Fix (only her window is excluded):" \
        "       ./run.sh --fix-kwin      (or: ./kwin-blur-exclude.sh apply; undo: ./kwin-blur-exclude.sh revert)" \
        "  (shown once)" >&2
    fi
    mkdir -p "$(dirname "$marker")" && : >"$marker"
  fi
fi

args=()
# Prefer a personal config if one exists, else the bundled one. `setup`
# always writes the personal config (seeded from the bundled one).
user_cfg="${XDG_CONFIG_HOME:-$HOME/.config}/lillith/config.toml"
if [[ ! -f "$user_cfg" && " $* " != *" --config "* && " $* " != *" setup "* ]]; then
  args+=(--config "$PKG/config.toml")
fi
# Default to Wayland unless the caller picked a backend or runs a subcommand.
if [[ "$launch" == 1 ]] && [[ " $* " != *" --backend "* ]] && [[ -n "${WAYLAND_DISPLAY:-}" ]]; then
  args+=(--backend wayland)   # use: ./run.sh --backend x11  for the X11 fallback
fi
exec "$PKG/bin/lillith" ${args[@]+"${args[@]}"} "$@"
