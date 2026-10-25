#!/usr/bin/env bash
# lillith-shadows.sh: per-user helpers shared by lillith-firstrun and Lillith-Install.sh
# (lillith-bin package). Source it, or run it:
#   lillith-shadows.sh [--dry-run]     move stale lillith binaries aside (see below)
#   lillith-shadows.sh --desktop FILE  put FILE on the desktop as lillith.desktop (trusted)
#
# lillith_move_shadows: a `lillith`, `lillith-settings`, `lillith-setup-wizard` or
# `lillith-uninstall` in ~/.cargo/bin, ~/.local/bin or any PATH dir searched before
# /usr/bin (that isn't the package's own) would run instead of the packaged one
# (an old `cargo install`, an old zip install). Each is MOVED (never deleted) to
# <name>.old-<YYYYmmdd-HHMMSS> in the same folder, and every move is printed.
# Links that resolve to the package (/usr/bin/<name>, /opt/lillith/...) are left alone.
# Env (tests): LILLITH_SYSBIN (default /usr/bin), LILLITH_OPT (default /opt/lillith).

LILLITH_SHADOW_NAMES=(lillith lillith-settings lillith-setup-wizard lillith-uninstall)

_lls_say(){ printf '  %s\n' "$*"; }

# The dirs to check, one per line, without duplicates.
lillith_shadow_dirs(){
  local sysbin="${LILLITH_SYSBIN:-/usr/bin}" opt="${LILLITH_OPT:-/opt/lillith}" d seen=":" sysreal
  sysreal="$(readlink -f "$sysbin" 2>/dev/null || echo "$sysbin")"
  local -a dirs=("${CARGO_HOME:-$HOME/.cargo}/bin" "$HOME/.local/bin")
  local IFS=:
  for d in $PATH; do
    [[ -n $d ]] || continue
    [[ $d == "$sysbin" || $(readlink -f "$d" 2>/dev/null) == "$sysreal" ]] && break   # only dirs before /usr/bin
    dirs+=("$d")
  done
  unset IFS
  for d in "${dirs[@]}"; do
    d="${d%/}"; [[ -n $d && -d $d ]] || continue
    case "$d/" in "$opt"/*) continue ;; esac                  # the package's own dirs
    [[ $(readlink -f "$d") == "$sysreal" ]] && continue
    [[ $seen == *":$(readlink -f "$d"):"* ]] && continue
    seen+="$(readlink -f "$d"):"
    printf '%s\n' "$d"
  done
}

# Moves the shadows aside. --dry-run only prints. Prints "moved A -> B" per file;
# returns 0 (a file that can't be moved, e.g. root-owned in /usr/local/bin, is a warning).
lillith_move_shadows(){
  local dry=0; [[ ${1:-} == --dry-run ]] && dry=1
  local sysbin="${LILLITH_SYSBIN:-/usr/bin}" opt="${LILLITH_OPT:-/opt/lillith}"
  local stamp d n f real pkg dst moved=0
  stamp="$(date +%Y%m%d-%H%M%S)"
  while IFS= read -r d; do
    for n in "${LILLITH_SHADOW_NAMES[@]}"; do
      f="$d/$n"
      [[ -e $f || -L $f ]] || continue
      real="$(readlink -f "$f" 2>/dev/null || true)"
      pkg="$(readlink -f "$sysbin/$n" 2>/dev/null || true)"
      if [[ -n $real && ( $real == "$pkg" || $real == "$opt"/* ) ]]; then continue; fi
      dst="$f.old-$stamp"; local i=1; while [[ -e $dst || -L $dst ]]; do dst="$f.old-$stamp-$i"; i=$((i + 1)); done
      if [[ $dry == 1 ]]; then _lls_say "would move $f -> $dst"; moved=$((moved + 1)); continue; fi
      if mv -- "$f" "$dst" 2>/dev/null; then
        _lls_say "moved $f -> $dst (it was used instead of $sysbin/$n)"; moved=$((moved + 1))
      else
        _lls_say "! can't move $f (not yours?); it still shadows $sysbin/$n. Fix: sudo mv '$f' '$dst'"
      fi
    done
  done < <(lillith_shadow_dirs)
  hash -r 2>/dev/null || true
  [[ $moved -gt 0 ]] || _lls_say "no stale lillith programs ahead of $sysbin on your PATH"
  return 0
}

# The desktop folder (xdg-user-dir DESKTOP; ~/Desktop when unset or it says $HOME).
lillith_desktop_dir(){
  local d=""
  command -v xdg-user-dir >/dev/null 2>&1 && d="$(xdg-user-dir DESKTOP 2>/dev/null || true)"
  [[ -n $d && $d != "$HOME" && $d != "$HOME/" ]] || d="$HOME/Desktop"
  printf '%s\n' "${d%/}"
}

# Copies a .desktop file onto the desktop as lillith.desktop, chmod +x, and marks it
# trusted (gio metadata::trusted, when gio exists) so Plasma 6 / GNOME launch it on a
# double-click without the "untrusted launcher" prompt.
lillith_desktop_shortcut(){
  local src="${1:-/usr/share/applications/lillith.desktop}" dir dst
  [[ -f $src ]] || { _lls_say "! no $src; desktop shortcut skipped"; return 1; }
  dir="$(lillith_desktop_dir)"; mkdir -p -- "$dir"; dst="$dir/lillith.desktop"
  install -m 755 -- "$src" "$dst" || return 1
  if command -v gio >/dev/null 2>&1; then
    gio set -- "$dst" metadata::trusted true >/dev/null 2>&1 && _lls_say "desktop shortcut $dst (trusted)" ||
      _lls_say "desktop shortcut $dst (gio couldn't mark it trusted; right-click > Allow Launching once)"
  else
    _lls_say "desktop shortcut $dst"
  fi
}

if [[ ${BASH_SOURCE[0]} == "$0" ]]; then
  set -uo pipefail
  case "${1:-}" in
    --desktop) lillith_desktop_shortcut "${2:-}" ;;
    -h|--help) sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//' ;;
    *) lillith_move_shadows "$@" ;;
  esac
fi
