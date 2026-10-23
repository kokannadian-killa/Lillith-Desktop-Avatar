#!/usr/bin/env bash
# Keep KDE's force-blur effects (Better Blur DX `better_blur_dx`, Garuda's
# default, and its parent Better Blur `forceblur`) from drawing a dark,
# blurred, rounded panel behind Lillith (the "black box"), WITHOUT turning
# the effect off for other windows: her window class `lillith` is added to
# the effect's excluded window classes in kwinrc, then KWin reconfigures.
#
#   linux/kwin-blur-exclude.sh check    report only (exit 0 = she's fine, 1 = needs apply)
#   linux/kwin-blur-exclude.sh apply    exclude her (backs up what it changed)
#   linux/kwin-blur-exclude.sh disable  last resort (exclusion impossible, e.g. a
#                                       /regex/ matches her): turn the effect off
#   linux/kwin-blur-exclude.sh revert   undo exactly what `apply`/`disable` changed
#
# kwinrc keys (source: src/blur.kcfg + metadata.json of
# github.com/xarblu/kwin-effects-better-blur-dx and taj-ny/kwin-effects-forceblur):
#   [Plugins] better_blur_dxEnabled / forceblurEnabled   effect on (default off)
#   [Effect-better-blur-dx] (Better Blur DX; older releases: [Effect-blurplus])
#   [Effect-blurplus]       (Better Blur)
#     WindowClasses  one class per line, `/regex/` allowed, matched against
#                    the window's resourceClass and resourceName
#     BlurMatching   true (default) = blur only listed classes,
#                    false          = blur all EXCEPT listed classes
# Lillith's class is `lillith` on Wayland (executable name) and X11 (WM_CLASS).
#
# State (for revert): ${XDG_DATA_HOME:-~/.local/share}/lillith/kwin-blur-exclusion.state
# Test hooks: KREADCONFIG / KWRITECONFIG / KWINRC_FILE / KWIN_RECONFIGURE /
# KWIN_EFFECT_CTL (called as: $KWIN_EFFECT_CTL load|unload PLUGIN) override the tools.
set -euo pipefail

CLASS=lillith
KREAD="${KREADCONFIG:-kreadconfig6}"
KWRITE="${KWRITECONFIG:-kwriteconfig6}"
RC="${KWINRC_FILE:-kwinrc}"
STATE="${XDG_DATA_HOME:-$HOME/.local/share}/lillith/kwin-blur-exclusion.state"
RC_PATH="$RC"
[[ $RC == /* ]] || RC_PATH="${XDG_CONFIG_HOME:-$HOME/.config}/$RC"

# name|plugin key|group|legacy group
EFFECTS=(
  "Better Blur DX|better_blur_dxEnabled|Effect-better-blur-dx|Effect-blurplus"
  "Better Blur|forceblurEnabled|Effect-blurplus|"
)

have() { command -v "$1" >/dev/null 2>&1; }
rd() { "$KREAD" --file "$RC" --group "$1" --key "$2" 2>/dev/null || true; }
wr() { "$KWRITE" --file "$RC" --group "$1" --key "$2" -- "$3"; }
has_group() { [[ -f $RC_PATH ]] && grep -qxF "[$1]" "$RC_PATH"; }

reconfigure() {
  if [[ -n ${KWIN_RECONFIGURE:-} ]]; then "$KWIN_RECONFIGURE"; return 0; fi
  if have qdbus6; then qdbus6 org.kde.KWin /KWin reconfigure >/dev/null 2>&1 || true
  elif have dbus-send; then
    dbus-send --session --type=method_call --dest=org.kde.KWin /KWin org.kde.KWin.reconfigure >/dev/null 2>&1 || true
  fi
}

# Load/unload an effect in the running KWin (kwinrc alone only applies at next login).
effect_ctl() {
  if [[ -n ${KWIN_EFFECT_CTL:-} ]]; then "$KWIN_EFFECT_CTL" "$1" "$2"; return 0; fi
  local m=loadEffect; [[ $1 == unload ]] && m=unloadEffect
  if have qdbus6; then qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.$m "$2" >/dev/null 2>&1 || true
  elif have dbus-send; then
    dbus-send --session --type=method_call --dest=org.kde.KWin /Effects org.kde.kwin.Effects.$m string:"$2" >/dev/null 2>&1 || true
  fi
}

# Does one WindowClasses line match her? (exact, or a simple /a|lillith/ regex)
line_matches() {
  local l="${1#"${1%%[![:space:]]*}"}"; l="${l%"${l##*[![:space:]]}"}"
  if [[ $l == /*/ ]]; then
    local re="${l:1:${#l}-2}" alt
    IFS='|' read -r -a alts <<<"$re"
    for alt in "${alts[@]}"; do
      alt="${alt//[\^\$\(\)]/}"
      [[ ${alt,,} == "$CLASS" ]] && return 0
    done
    return 1
  fi
  [[ $l == "$CLASS" ]]
}
listed() { local l; while IFS= read -r l; do line_matches "$l" && return 0; done <<<"$1"; return 1; }
has_exact() { grep -qxF -- "$CLASS" <<<"$1"; }
without_exact() { grep -vxF -- "$CLASS" <<<"$1" || true; }
with_her() { local c="$1"; c="${c%$'\n'}"; if [[ -n $c ]]; then printf '%s\n%s' "$c" "$CLASS"; else printf '%s' "$CLASS"; fi; }

# For each enabled effect: "name<TAB>group<TAB>mode<TAB>targets(0/1)<TAB>plugin key<TAB>classes"
# (classes last: tabs are IFS whitespace, so an empty middle field would collapse)
scan() {
  local e name key group legacy g classes mode t
  for e in "${EFFECTS[@]}"; do
    IFS='|' read -r name key group legacy <<<"$e"
    [[ "$(rd Plugins "$key")" == true ]] || continue
    g="$group"
    if [[ -n $legacy ]] && ! has_group "$group" && has_group "$legacy"; then g="$legacy"; fi
    classes="$(rd "$g" WindowClasses)"
    if [[ "$(rd "$g" BlurMatching)" == false ]]; then mode=except; else mode=only; fi
    t=0
    if [[ $mode == except ]]; then listed "$classes" || t=1; else listed "$classes" && t=1; fi
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$name" "$g" "$mode" "$t" "$key" "${classes//$'\n'/\\n}"
  done
}

cmd_check() {
  local any=0 found=0 name g mode t classes
  while IFS=$'\t' read -r name g mode t _ classes; do
    found=1
    if [[ $t == 1 ]]; then
      any=1
      echo "$name is on and blurs Lillith's window (mode: blur $mode listed; [$g] WindowClasses=$classes)."
    else
      echo "$name is on; Lillith is already excluded."
    fi
  done < <(scan)
  [[ $found == 1 ]] || echo "No force-blur effect (Better Blur DX / Better Blur) is enabled."
  if [[ $any == 1 ]]; then
    echo "Fix: $0 apply   (or: lillith fix-kwin)"
    return 1
  fi
}

cmd_apply() {
  local changed=0 name g mode t classes cur new
  mkdir -p "$(dirname "$STATE")"
  while IFS=$'\t' read -r name g mode t _ classes; do
    [[ $t == 1 ]] || { echo "  $name: Lillith already excluded"; continue; }
    cur="$(rd "$g" WindowClasses)"
    if [[ $mode == except ]]; then
      new="$(with_her "$cur")"
      wr "$g" WindowClasses "$new"
      printf '%s\t%s\n' "$g" added >>"$STATE"
      echo "  $name: added '$CLASS' to [$g] WindowClasses (blur all except listed)"
      changed=1
    elif has_exact "$cur"; then
      new="$(without_exact "$cur")"
      wr "$g" WindowClasses "$new"
      printf '%s\t%s\n' "$g" removed >>"$STATE"
      echo "  $name: removed '$CLASS' from [$g] WindowClasses (blur only listed)"
      changed=1
    else
      echo "  $name: a /regex/ in [$g] WindowClasses matches '$CLASS'; edit it in System Settings > Desktop Effects > $name" >&2
    fi
  done < <(scan)
  [[ $changed == 0 ]] || reconfigure
}

# Last resort: turn off every enabled force-blur effect that still blurs her.
cmd_disable() {
  local name g mode t classes key changed=0
  mkdir -p "$(dirname "$STATE")"
  while IFS=$'\t' read -r name g mode t key classes; do
    [[ $t == 1 ]] || continue
    wr Plugins "$key" false
    effect_ctl unload "${key%Enabled}"
    printf '%s\t%s\n' Plugins "disabled:$key" >>"$STATE"
    echo "  $name: turned off ([Plugins] $key=false; revert turns it back on)"
    changed=1
  done < <(scan)
  [[ $changed == 0 ]] || reconfigure
}

cmd_revert() {
  [[ -f $STATE ]] || { echo "  nothing to revert"; return 0; }
  local g what cur
  while IFS=$'\t' read -r g what; do
    cur="$(rd "$g" WindowClasses)"
    case $what in
      added) has_exact "$cur" && wr "$g" WindowClasses "$(without_exact "$cur")" ;;
      removed) has_exact "$cur" || wr "$g" WindowClasses "$(with_her "$cur")" ;;
      disabled:*)
        wr Plugins "${what#disabled:}" true
        effect_ctl load "$(k="${what#disabled:}"; echo "${k%Enabled}")"
        echo "  turned [Plugins] ${what#disabled:} back on"
        continue ;;
    esac
    echo "  restored [$g] WindowClasses"
  done <"$STATE"
  rm -f -- "$STATE"
  reconfigure
}

if ! have "$KREAD" || ! have "$KWRITE"; then
  echo "kreadconfig6/kwriteconfig6 not found (not KDE Plasma 6): nothing to do."
  exit 0
fi
case "${1:-check}" in
  check) cmd_check ;;
  apply) cmd_apply ;;
  disable) cmd_disable ;;
  revert) cmd_revert ;;
  *) echo "usage: $0 check|apply|disable|revert" >&2; exit 2 ;;
esac
