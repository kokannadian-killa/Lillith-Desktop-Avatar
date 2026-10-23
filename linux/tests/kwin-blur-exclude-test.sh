#!/usr/bin/env bash
# Tests linux/kwin-blur-exclude.sh against fake kwinrc files, with a fake
# kreadconfig6/kwriteconfig6 (tests/fake-kconfig.py). No KDE needed:
#   bash linux/tests/kwin-blur-exclude-test.sh
# shellcheck disable=SC2016,SC2155,SC1007,SC2034
set -u
T="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
S="$T/../kwin-blur-exclude.sh"
BIN="$(mktemp -d)"
ln -s "$T/fake-kconfig.py" "$BIN/kreadconfig6"; ln -s "$T/fake-kconfig.py" "$BIN/kwriteconfig6"
export PATH="$BIN:$PATH" KWIN_RECONFIGURE=true
fail=0; t() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi; }
setup() { export HOME=$(mktemp -d); unset XDG_CONFIG_HOME XDG_DATA_HOME; mkdir -p "$HOME/.config"; printf '%s' "$1" > "$HOME/.config/kwinrc"; }
rc() { cat "$HOME/.config/kwinrc"; }
# 1. Ko's case: BBDX on, blur all except listed
setup $'[Plugins]\nbetter_blur_dxEnabled=true\nlillith-bridgeEnabled=true\n\n[Effect-better-blur-dx]\nBlurMatching=false\nBlurNonMatching=true\nWindowClasses=vlc\\nzen\n'
$S check >/dev/null; t "check flags Ko's case" '[[ $? == 1 ]]'
$S apply
t "apply appends lillith" 'grep -qx "WindowClasses=vlc\\\\nzen\\\\nlillith" <(rc)'
$S check >/dev/null; t "check clean after apply" '[[ $? == 0 ]]'
$S apply | grep -q "already excluded"; t "apply idempotent" '[[ $(grep -o "nlillith" <(rc) | wc -l) == 1 ]]'
$S revert >/dev/null
t "revert restores" 'grep -qx "WindowClasses=vlc\\\\nzen" <(rc)'
t "state removed" '[[ ! -f $HOME/.local/share/lillith/kwin-blur-exclusion.state ]]'
# 2. whitelist with lillith listed
setup $'[Plugins]\nforceblurEnabled=true\n[Effect-blurplus]\nWindowClasses=kate\\nlillith\n'
$S apply >/dev/null; t "whitelist removes her" 'grep -qx "WindowClasses=kate" <(rc)'
$S revert >/dev/null; t "whitelist revert re-adds" 'grep -qx "WindowClasses=kate\\\\nlillith" <(rc)'
# 3. disabled effect untouched
setup $'[Plugins]\nbetter_blur_dxEnabled=false\n[Effect-better-blur-dx]\nBlurMatching=false\n'
before=$(rc); $S apply >/dev/null; t "disabled effect untouched" '[[ "$(rc)" == "$before" ]]'
# 4. legacy group, empty list
setup $'[Plugins]\nbetter_blur_dxEnabled=true\n[Effect-blurplus]\nBlurMatching=false\n'
$S apply >/dev/null; t "legacy group, empty list" 'grep -qx "WindowClasses=lillith" <(rc)'
# 5. regex in whitelist
setup $'[Plugins]\nforceblurEnabled=true\n[Effect-blurplus]\nWindowClasses=/kate|lillith/\n'
$S apply 2>&1 | grep -q regex; t "regex reported" '[[ $? == 0 ]]'
# 6. disable fallback + revert (regex case, empty-list legacy too)
export KWIN_EFFECT_CTL="$BIN/effctl" LOG=$(mktemp)
printf '#!/bin/sh\necho "$@" >>"$LOG"\n' > "$KWIN_EFFECT_CTL"; chmod +x "$KWIN_EFFECT_CTL"
setup $'[Plugins]\nbetter_blur_dxEnabled=true\n[Effect-better-blur-dx]\nWindowClasses=/kate|lillith/\n'
$S disable >/dev/null
t "disable sets false" 'grep -qx "better_blur_dxEnabled=false" <(rc)'
t "disable unloads" 'grep -qx "unload better_blur_dx" $LOG'
$S check >/dev/null; t "check clean after disable" '[[ $? == 0 ]]'
$S revert >/dev/null
t "revert re-enables" 'grep -qx "better_blur_dxEnabled=true" <(rc)'
t "revert loads" 'grep -qx "load better_blur_dx" $LOG'
setup $'[Plugins]\nbetter_blur_dxEnabled=true\n[Effect-blurplus]\nBlurMatching=false\n'
$S disable >/dev/null; t "disable with empty class list" 'grep -qx "better_blur_dxEnabled=false" <(rc)'
echo; [[ $fail == 0 ]] && echo "all passed" || echo "FAILURES"
exit $fail
