#!/usr/bin/env bash
# Tests for the lillith-bin per-user helpers (lillith-shadows.sh) and the package's
# desktop entry. No root, no network: everything runs in a temp HOME.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; ROOT="$(cd "$HERE/../.." && pwd)"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
fail(){ echo "FAIL: $*" >&2; exit 1; }
pass=0; ok(){ pass=$((pass + 1)); echo "ok - $*"; }

# A fake system: /usr/bin -> /opt/lillith/launch links, like the package.
SYS="$T/usr/bin"; OPT="$T/opt/lillith"; mkdir -p "$SYS" "$OPT/launch" "$OPT/bin"
for n in lillith lillith-settings lillith-setup-wizard lillith-uninstall; do
  printf '#!/bin/sh\necho packaged-%s\n' "$n" > "$OPT/launch/$n"; chmod +x "$OPT/launch/$n"; ln -s "$OPT/launch/$n" "$SYS/$n"
done
export HOME="$T/home" LILLITH_SYSBIN="$SYS" LILLITH_OPT="$OPT"; unset CARGO_HOME XDG_DATA_HOME
mkdir -p "$HOME/.cargo/bin" "$HOME/.local/bin" "$T/earlier" "$T/later"
mk(){ printf '#!/bin/sh\necho stale\n' > "$1"; chmod +x "$1"; }
mk "$HOME/.cargo/bin/lillith"; mk "$HOME/.cargo/bin/lillith-settings"
mk "$HOME/.local/bin/lillith-setup-wizard"; mk "$T/earlier/lillith-uninstall"
mk "$T/later/lillith"                                   # after /usr/bin: not a shadow
ln -s "$SYS/lillith-settings" "$HOME/.local/bin/lillith-settings"   # points at the package: keep
mk "$HOME/.local/bin/lillith-say"                       # not one of ours: keep
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$T/earlier:$SYS:$T/later:/usr/bin:/bin"

. "$HERE/lillith-shadows.sh"
[[ $(type -P lillith) == "$HOME/.cargo/bin/lillith" ]] || fail "setup: cargo lillith should shadow"
dirs="$(lillith_shadow_dirs)"
grep -qx "$T/earlier" <<<"$dirs" || fail "PATH dir before /usr/bin not checked: $dirs"
! grep -qx "$T/later" <<<"$dirs" || fail "PATH dir after /usr/bin checked"
ok "shadow dirs: ~/.cargo/bin, ~/.local/bin and PATH dirs before /usr/bin only"

dry="$(lillith_move_shadows --dry-run)"
[[ -e $HOME/.cargo/bin/lillith ]] || fail "--dry-run moved something"
grep -q "would move $HOME/.cargo/bin/lillith ->" <<<"$dry" || fail "dry run: $dry"
ok "--dry-run only prints"

out="$(lillith_move_shadows)"; echo "$out"
for f in "$HOME/.cargo/bin/lillith" "$HOME/.cargo/bin/lillith-settings" "$HOME/.local/bin/lillith-setup-wizard" "$T/earlier/lillith-uninstall"; do
  [[ ! -e $f ]] || fail "$f not moved"
  m=( "$f".old-* ); [[ -f ${m[0]} ]] || fail "$f: no $f.old-<date>"
  [[ $(cat "${m[0]}") == *stale* ]] || fail "moved copy changed"
  grep -q "moved $f -> $f.old-" <<<"$out" || fail "move of $f not printed"
done
[[ -L $HOME/.local/bin/lillith-settings ]] || fail "link to the package moved"
[[ -f $HOME/.local/bin/lillith-say && -f $T/later/lillith ]] || fail "unrelated files moved"
hash -r; [[ $(type -P lillith) == "$SYS/lillith" ]] || fail "lillith still resolves to $(type -P lillith)"
ok "stale programs moved to <name>.old-<date> (none deleted); /usr/bin/lillith wins"

out2="$(lillith_move_shadows)"; grep -q "no stale lillith programs" <<<"$out2" || fail "second run: $out2"
mk "$HOME/.cargo/bin/lillith"; lillith_move_shadows >/dev/null
[[ $(ls "$HOME/.cargo/bin" | grep -c '^lillith\.old-') -ge 2 ]] || fail "a second move overwrote the first backup"
ok "idempotent; a later shadow gets its own backup name"

# desktop entry: absolute Exec, Icon=lillith
D="$ROOT/linux/lillith.desktop"
grep -qx 'Exec=/usr/bin/lillith' "$D" || fail "$D: Exec must be /usr/bin/lillith"
grep -qx 'Exec=/usr/bin/lillith-settings' "$D" || fail "$D: settings Exec must be absolute"
grep -qx 'Icon=lillith' "$D" || fail "$D: Icon=lillith"
! grep -E '^Exec=[^/]' "$D" || fail "$D: relative Exec"
ok "lillith.desktop: absolute Exec, Icon=lillith"

# desktop shortcut: xdg-user-dir DESKTOP, chmod +x, gio metadata::trusted
mkdir -p "$T/stub" "$HOME/Schreibtisch"
printf '#!/bin/sh\n[ "$1" = DESKTOP ] && echo "%s/Schreibtisch"\n' "$HOME" > "$T/stub/xdg-user-dir"
printf '#!/bin/sh\necho "$*" >> "%s/gio.log"\n' "$T" > "$T/stub/gio"; chmod +x "$T/stub/"*
PATH="$T/stub:$PATH" lillith_desktop_shortcut "$D" >/dev/null
f="$HOME/Schreibtisch/lillith.desktop"
[[ -x $f ]] || fail "desktop shortcut not executable"; cmp -s "$f" "$D" || fail "shortcut differs"
grep -q -- "set -- $f metadata::trusted true" "$T/gio.log" || fail "gio not called: $(cat "$T/gio.log" 2>/dev/null)"
rm "$T/stub/xdg-user-dir" "$T/stub/gio"
PATH="$T/stub:/usr/bin:/bin" HOME="$T/h2" lillith_desktop_shortcut "$D" >/dev/null
[[ -x $T/h2/Desktop/lillith.desktop ]] || fail "no xdg-user-dir: ~/Desktop fallback"
ok "desktop shortcut: xdg desktop dir, +x, gio trusted; ~/Desktop fallback"

# firstrun / wrapper parse and use absolute paths
bash -n "$HERE/lillith-firstrun" "$HERE/lillith-uninstall" && sh -n "$HERE/lillith-wrapper"
grep -q '"$SYSBIN/lillith" setup --auto-gguf' "$HERE/lillith-firstrun" || fail "firstrun: auto-gguf"
grep -q -- '--systemd' "$HERE/lillith-firstrun" || fail "firstrun: voice --systemd"
grep -q 'systemctl --user enable --now lillith-voice.service' "$HERE/lillith-firstrun" || fail "firstrun: enable voice"
ok "firstrun: shadows, --systemd + enable --now lillith-voice, setup --auto-gguf"
# install.sh's model list: same order as `lillith setup --auto-gguf`
eval "$(sed -n '/^ram_bytes() {/,/^find_llama() {/p' "$ROOT/linux/install.sh" | sed '$d')"
M="$T/models"; mkdir -p "$M" "$HOME/AI/models"; G=$((1024 ** 3))
mkg(){ truncate -s "$2" "$1"; }
mkg "$M/mistral-7b-instruct-q4_k_m.gguf" $((4 * G + G / 14)); mkg "$M/llama-2-13b-q4_k_m.gguf" $((7 * G + 4 * G / 10)); mkg "$M/phi-3-mini-q4.gguf" $((2 * G + G / 4))
mkg "$M/dolphin-2.9-llama3-8b-q4_k_m.gguf" $((4 * G + 9 * G / 10)); mkg "$M/dolphin-3b-q8.gguf" $((3 * G + G / 2))
mkg "$M/qwen2-0.5b.gguf" $((G / 2))
ko=$(( 76 * G / 10 ))   # Ko's laptop: 7.6 GiB (firstrun used to take mistral-7b, the first glob match)
first=$(printf '%s\n' "$M"/*.gguf | LILLITH_RAM_BYTES=$ko gguf_rank | head -1)
[[ $first == "$M/dolphin-3b-q8.gguf" ]] || fail "7.6 GB: want the Dolphin that fits, got $first"
order=$(printf '%s\n' "$M"/*.gguf | LILLITH_RAM_BYTES=$ko gguf_rank | xargs -n1 basename | tr '\n' ' ')
[[ $order == "dolphin-3b-q8.gguf qwen2-0.5b.gguf phi-3-mini-q4.gguf mistral-7b-instruct-q4_k_m.gguf dolphin-2.9-llama3-8b-q4_k_m.gguf llama-2-13b-q4_k_m.gguf " ]] || fail "order: $order"
LILLITH_RAM_BYTES=$ko gguf_too_big "$M/llama-2-13b-q4_k_m.gguf" || fail "a 13B should be too big for 7.6 GB"
! LILLITH_RAM_BYTES=$ko gguf_too_big "$M/phi-3-mini-q4.gguf" || fail "phi-3 fits"
P="$HOME/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf"; mkg "$P" $((2 * G))
first=$(printf '%s\n' "$M"/*.gguf "$P" | LILLITH_RAM_BYTES=$ko gguf_rank | head -1)
[[ $first == "$P" ]] || fail "the preferred file must win, got $first"
ok "install.sh model list: preferred, Dolphin that fits, smallest that fits; too big is flagged"
echo "# $pass passed"
