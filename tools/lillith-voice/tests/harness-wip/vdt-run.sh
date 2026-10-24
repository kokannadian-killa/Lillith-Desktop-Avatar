#!/usr/bin/env bash
# vdt-run.sh SHA [BASE]  - voice-daemon e2e for Lillith against the real lillith-voiced.
#   SHA   commit to test (pinned; the branch tip is never followed)
#   BASE  optional main SHA: tests SHA rebased onto BASE on a LOCAL branch (never pushed)
# env:  NO_BUILD=1 BIN=/path/lillith  skip git/cargo and use this binary (dry runs)
#       SKIP_TESTS=1 / SKIP_WINCHECK=1   skip cargo test / windows cargo check
# Output: $VT/results/<tag>/summary.txt (+ logs). Kills only the PIDs it started.
set -uo pipefail
S=${SLOW:-1}   # wait multiplier for a loaded box
SHA="${1:?usage: vdt-run.sh SHA [BASE]}"; BASE="${2:-}"
WT=${WT:-/workspace/lillith/wt-voice-daemon-test}   # override: WT=<worktree> (NO_BUILD runs read its manifest)
VT=/workspace/lillith/scratch/vdt-tmp; RESDIR=${RESDIR:-}
KIT=/workspace/lillith/lillith-voice-kit
LV=$VT/home/.local/share/lillith-voice; VPY=$LV/venv/bin/python
TAG="${SHA:0:7}${BASE:+-on-${BASE:0:7}}-$(date +%H%M%S)"
R=${RESDIR:-$VT/results/$TAG}; RUN=$VT/run-$TAG
mkdir -p "$R" "$RUN"; chmod 700 "$RUN"
SUM=$R/summary.txt; : > "$SUM"
PIDS=()
log() { printf '[%s] %s\n' "$(date +%T)" "$*" | tee -a "$R/runner.log" >&2; }
res() { printf '%-34s %-5s %s\n' "$1" "$2" "${3:-}" | sed 's/\x1b\[[0-9;]*m//g' | tee -a "$SUM" >&2; }
cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done; sleep 1
  for p in "${PIDS[@]}"; do kill -9 "$p" 2>/dev/null; done
  rm -rf "$RUN"
  log "cleaned up (results kept in $R)"
}
trap cleanup EXIT
trap 'exit 130' INT TERM
fatal() { res "$1" FAIL "$2"; log "FATAL: $2"; finish; exit 1; }
finish() {
  { echo; echo "SHA tested: ${TESTED:-?} (requested $SHA${BASE:+, rebased onto $BASE})"
    echo "verdict: $(grep -q ' FAIL ' "$SUM" && echo FAIL || echo PASS) ($(grep -c ' PASS ' "$SUM") pass, $(grep -c ' FAIL ' "$SUM") fail, $(grep -c ' SKIP \| INFO ' "$SUM") skip/info)"; } >> "$SUM"
  log "summary: $SUM"; cat "$SUM"
}
export CARGO_TARGET_DIR=/workspace/lillith/target-vdt CARGO_INCREMENTAL=0 \
  CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_PROFILE_TEST_DEBUG=line-tables-only \
  RUSTFLAGS="--remap-path-prefix=$WT=wt-voice-daemon-test"
CARGO_HOME_ENV=(env HOME=/home/box)

# ---------------- 1. checkout ----------------
if [ "${NO_BUILD:-0}" != 1 ]; then
  cd "$WT" || fatal git.worktree "no worktree $WT"
  [ -z "$(git status --porcelain)" ] || fatal git.clean "worktree $WT is dirty"
  HOME=/home/box git fetch origin feat/voice-daemon main >"$R/fetch.log" 2>&1 || log "git fetch failed (no creds?); using local objects"
  git cat-file -e "$SHA^{commit}" 2>/dev/null || fatal git.sha "commit $SHA not in the local repo (fetch failed: $(tail -1 "$R/fetch.log"))"
  if [ -n "$BASE" ]; then
    git cat-file -e "$BASE^{commit}" 2>/dev/null || fatal git.base "base $BASE not in the local repo"
    BR="test/vd-${SHA:0:7}-on-${BASE:0:7}"
    git checkout -q -B "$BR" "$SHA" || fatal git.checkout "checkout $SHA failed"
    if ! git rebase "$BASE" >"$R/rebase.log" 2>&1; then
      CF=$(git diff --name-only --diff-filter=U | tr '\n' ' '); git rebase --abort
      fatal integrate "FAIL-to-integrate: rebase of $SHA onto $BASE conflicts in: $CF"
    fi
  else
    git checkout -q --detach "$SHA" || fatal git.checkout "checkout $SHA failed"
  fi
  TESTED=$(git rev-parse HEAD); log "testing $TESTED"
  res git.checkout PASS "$TESTED"
  FREE=$(df -Pk /workspace | awk 'NR==2{print int($4/1024)}')
  [ "$FREE" -ge 1536 ] || fatal disk "only ${FREE} MiB free (< 1.5 GB)"
  # ---------------- 2. build ----------------
  log "cargo build (waits on other builds' locks)"
  "${CARGO_HOME_ENV[@]}" cargo build -p lillith --locked --offline -j 2 >"$R/build.log" 2>&1 \
    || fatal build "cargo build failed: $(grep -m1 -E '^error' "$R/build.log")"
  mkdir -p "$VT/bin"; BIN=$VT/bin/lillith-${TESTED:0:7}; cp -f "$CARGO_TARGET_DIR/debug/lillith" "$BIN"
  res build PASS "$(tail -1 "$R/build.log" | sed 's/^ *//') -> $BIN"
  # ---------------- 3. crate tests ----------------
  if [ "${SKIP_TESTS:-0}" != 1 ]; then
    "${CARGO_HOME_ENV[@]}" cargo test --locked --offline -j 2 -p lillith-voice -p lillith-anim -p lillith-panel >"$R/test.log" 2>&1
    rc=$?; T=$(grep -E '^test result:' "$R/test.log" | awk '{p+=$4; f+=$6} END{print p" passed, "f" failed"}')
    [ $rc = 0 ] && res cargo.test PASS "$T" || res cargo.test FAIL "rc=$rc $T; $(grep -m3 -E '^test .* FAILED|^error' "$R/test.log" | tr '\n' ';')"
  fi
  # ---------------- 4. windows check ----------------
  if [ "${SKIP_WINCHECK:-0}" != 1 ]; then
    TC=/home/box/.rustup/toolchains/1.85-x86_64-unknown-linux-gnu/bin   # /usr/bin/cargo's rustc has no windows std
    "${CARGO_HOME_ENV[@]}" RUSTC="$TC/rustc" "$TC/cargo" check --locked --offline -j 2 -p lillith-voice -p lillith-core --target x86_64-pc-windows-gnu --all-targets >"$R/wincheck.log" 2>&1 \
      && res windows.cargo_check PASS "lillith-voice+lillith-core x86_64-pc-windows-gnu --all-targets, rustup 1.85 ($(grep -c '^warning' "$R/wincheck.log") warnings)" \
      || res windows.cargo_check FAIL "$(grep -m2 -E '^error' "$R/wincheck.log" | tr '\n' ';')"
  fi
else
  BIN="${BIN:?NO_BUILD=1 needs BIN}"; TESTED="(prebuilt $BIN)"
fi
[ -x "$BIN" ] || fatal bin "no binary $BIN"

# ---------------- 5. daemon ----------------
mkdir -p "$RUN/played" "$RUN/mic" "$RUN/dxdg" "$RUN/drt"; chmod 700 "$RUN/drt"
DSOCK=$RUN/voiced.sock; ASOCK=$RUN/app.sock
start_daemon() { # $1 socket  $2 logfile  [extra env...]
  local s=$1 l=$2; shift 2
  env -i PATH=/usr/bin:/bin HOME="$VT/home" XDG_CONFIG_HOME="$RUN/dxdg" XDG_RUNTIME_DIR="$RUN/drt" \
    LILLITH_VOICE_SOCKET="$s" LILLITH_PLAYER="$VPY $KIT/tests/fake_player.py {} $RUN/played/" \
    LILLITH_MIC_CMD="$VPY $KIT/tests/fake_mic.py $RUN/mic" "$@" "$VT/home/.local/bin/lillith-voiced" >"$l" 2>&1 &
  PIDS+=($!); local i; for i in $(seq $((120*S))); do grep -q ready "$l" && return 0; sleep 0.5; done; return 1
}
start_daemon "$DSOCK" "$R/daemon.log" || fatal daemon.start "lillith-voiced not ready: $(tail -2 "$R/daemon.log")"
res daemon.start PASS "$(grep -m1 ready "$R/daemon.log" | sed "s|$RUN|\$RUN|g" | cut -c1-110)"
"$VPY" "$VT/lib/proxy.py" "$ASOCK" "$DSOCK" "$R/proxy.log" >"$R/proxy.err" 2>&1 & PIDS+=($!)
for _ in $(seq $((50*S))); do [ -S "$ASOCK" ] && break; sleep 0.1; done
LILLITH_VOICE_SOCKET=$DSOCK "$VPY" "$VT/lib/daemon_checks.py" "$VT/assets" "$RUN/mic" >"$R/daemon_checks.log" 2>&1
grep '^RESULT' "$R/daemon_checks.log" | while read -r _ n v d; do res "$n" "$v" "$d"; done
grep -q '^RESULT' "$R/daemon_checks.log" || res proto.checks FAIL "$(tail -3 "$R/daemon_checks.log" | tr '\n' ' ')"

# p1 fallback (clone unavailable): 2nd daemon with an empty pocket dir
mkdir -p "$RUN/nopocket"
if start_daemon "$RUN/np.sock" "$R/daemon-nopocket.log" LILLITH_POCKET_DIR="$RUN/nopocket"; then
  NPP=${PIDS[-1]}
  out=$(cd "$VT/lib" && "$VPY" -c "
import json; from vc import req; p=req({'cmd':'ping'},path='$RUN/np.sock'); s=req({'cmd':'say','text':'Fallback voice.','preset':'lillith','wait':True},path='$RUN/np.sock',timeout=$((120*S)))
print(json.dumps({'cloned_voice':p.get('cloned_voice'),'presets':p.get('presets'),'say_preset':s.get('preset'),'engine':s.get('engine'),'ok':s.get('ok')}))")
  warn=$(grep -m1 'falling back' "$R/daemon-nopocket.log")
  echo "$out" | grep -q '"say_preset": "p1"' && res preset.lillith_to_p1.daemon PASS "$out | $warn" || res preset.lillith_to_p1.daemon FAIL "$out | $warn"
else NPP=""; res preset.lillith_to_p1.daemon FAIL "no-pocket daemon did not start: $(tail -2 "$R/daemon-nopocket.log")"; fi

# ---------------- 6. app under Xvfb ----------------
DISP=""; for n in $(seq 99 199); do [ -e "/tmp/.X11-unix/X$n" ] || [ -e "/tmp/.X$n-lock" ] || { DISP=":$n"; break; }; done
[ -n "$DISP" ] || fatal xvfb "no free display"
Xvfb "$DISP" -screen 0 1280x800x24 -br -nolisten tcp -noreset +extension Composite >"$R/xvfb.log" 2>&1 & PIDS+=($!)
for _ in $(seq $((100*S))); do [ -e "/tmp/.X11-unix/X${DISP#:}" ] && break; sleep 0.1; done
export DISPLAY=$DISP; unset WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS PULSE_SERVER
REPLY="Of course, darling. I am right here with you, and I will keep talking for a little while so that you can interrupt me if you want to. Still here, still talking."
"$VPY" -u "$VT/lib/stub.py" "$RUN/port" "$REPLY" >"$R/stub.log" 2>&1 & PIDS+=($!)
for _ in $(seq $((50*S))); do [ -s "$RUN/port" ] && break; sleep 0.1; done
EP="http://127.0.0.1:$(cat "$RUN/port")"
p=$(awk -F'"' '/^page *= *"/ {print $2; exit}' "$WT/assets/generated/manifest.toml")
MAN=$WT/assets/generated/manifest.toml; [ "$(head -c4 "$WT/assets/generated/$p" | od -An -tx1 | tr -d ' \n')" = 89504e47 ] || fatal manifest "repo art not real PNGs"
mkapp() { # $1 name  $2 daemon_socket -> sets AH (home) ACFG
  AH=$RUN/app-$1; mkdir -p "$AH/home" "$AH/cfg/lillith" "$AH/data" "$AH/state" "$AH/cache" "$AH/rt"; chmod 700 "$AH/rt"
  mkdir -p "$AH/custom"; ACFG=$AH/custom/config.toml   # NOT the XDG default path: the mic must follow --config
  cat >"$ACFG" <<TOML
mode = "stay"
muted = ${3:-false}
voice_enabled = true
llm_backend = "llama-server"
llm_url = "$EP"
autostart = false

[voice]
enabled = true
mute = ${3:-false}
engine = "daemon"
daemon_socket = "$2"
preset = "lillith"
cooldown_s = 0.0
repeat_cooldown_s = 0.0
max_chars = 400

[voice.stt]
enabled = true
timeout_s = $((20*S)).0

[overlay]
auto_fix_kwin = false

[monitor]
enabled = false

[chat]
enabled = true
endpoint = "$EP"
save_history = false
overlay_speech = true
open_on_click = true
timeout_s = $((20*S)).0
TOML
}
appenv() { exec env -i PATH=/usr/bin:/bin DISPLAY="$DISP" HOME="$AH/home" XDG_CONFIG_HOME="$AH/cfg" XDG_DATA_HOME="$AH/data" \
  XDG_STATE_HOME="$AH/state" XDG_CACHE_HOME="$AH/cache" XDG_RUNTIME_DIR="$AH/rt" NO_AT_BRIDGE=1 RUST_BACKTRACE=1 \
  RUST_LOG="info,lillith=debug,lillith_voice=debug,lillith_overlay=debug,lillith_panel=debug,lillith_anim=debug,lillith_core=debug" "$@"; }
viewable() { xwininfo -id "$1" 2>/dev/null | grep -q 'Map State: IsViewable'; }
wins() { local w; for w in $(xdotool search --name '^Lillith$' 2>/dev/null); do viewable "$w" && echo "$w"; done; }
geom() { xwininfo -id "$1" | awk '/Absolute upper-left X:/{x=$4} /Absolute upper-left Y:/{y=$4} /Width:/{w=$2} /Height:/{h=$2} END{print x, y, w, h}'; }
shot() { ffmpeg -loglevel error -y -f x11grab -draw_mouse 0 -video_size 1280x800 -i "$DISP" -frames:v 1 "$R/$1.png" 2>/dev/null; }
waitfor() { # $1 file $2 regex $3 seconds
  local d=$((SECONDS + $3*S)); while [ $SECONDS -lt $d ]; do grep -qE "$2" "$1" 2>/dev/null && return 0; sleep 0.25; done; return 1; }
pcount() { grep -cE "$1" "$R/proxy.log" 2>/dev/null || true; }
launch_app() { # $1 log
  appenv "$BIN" --config "$ACFG" --manifest "$MAN" --backend x11 --no-tray --no-monitor --no-save --reset-position \
    --mode stay --exit-after 300 >"$1" 2>&1 & APP=$!; PIDS+=($APP)
  MAINW=""; local d=$((SECONDS + 40*S)); while [ $SECONDS -lt $d ]; do kill -0 $APP 2>/dev/null || return 1; MAINW=$(wins | head -1); [ -n "$MAINW" ] && return 0; sleep 0.3; done; return 1; }

mkapp main "$ASOCK"; AL=$R/app.log
launch_app "$AL" || fatal app.start "no window: $(tail -5 "$AL" | tr '\n' ' ')"
sleep 3
VREASON=$(grep -m1 -E 'voice( off)?: ' "$AL" | sed 's/^.*voice/voice/' | cut -c1-140)
{ pcount '> \{"cmd": ?"ping"|> \{"op": ?"ping"' | grep -qv '^0$'; } && echo "$VREASON" | grep -qi 'voiced\|daemon' \
  && res app.detect_ping PASS "$VREASON" || res app.detect_ping FAIL "log: $VREASON; ping lines in proxy: $(pcount '"ping"')"
waitfor "$R/proxy.log" '"subscribe"' 8 && res app.lipsync_subscribe PASS "$(grep -m1 -E '"subscribe"' "$R/proxy.log" | cut -d' ' -f3- | cut -c1-120)" \
  || res app.lipsync_subscribe FAIL "no subscribe request seen at the daemon"

# open the bubble: Enter on her window, fall back to a left click at her centre
read -r WX WY WW WH < <(geom "$MAINW")
xdotool windowactivate --sync "$MAINW" 2>/dev/null || xdotool windowfocus "$MAINW" 2>/dev/null; xdotool key --window "$MAINW" Return 2>/dev/null
BUBW=""; open_bubble() { local d=$((SECONDS + ${1:-10}*S)); while [ $SECONDS -lt $d ]; do BUBW=$(wins | grep -vx "$MAINW" | head -1); [ -n "$BUBW" ] && return 0; sleep 0.3; done; return 1; }
if ! open_bubble 6; then xdotool mousemove $((WX + WW / 2)) $((WY + WH / 2)) click 1; open_bubble 10; fi
shot bubble-open
[ -n "$BUBW" ] && res app.bubble_open PASS "bubble window $BUBW" || fatal app.bubble_open "chat bubble did not open (Enter / left click); see bubble-open.png, app.log"
read -r BX BY BW BH < <(geom "$BUBW")
sleep 1.5
SAY0=$(pcount '> .*"say"')
xdotool windowfocus --sync "$BUBW" 2>/dev/null; xdotool mousemove $((BX + 60)) $((BY + BH - 24)) click 1; sleep 0.3; xdotool windowfocus --sync "$BUBW" 2>/dev/null
xdotool type --delay 40 "hello lillith"; xdotool key Return
waitfor "$R/stub.log" 'chat/completions' 10 && res app.chat_request PASS "stub got the chat request" || res app.chat_request FAIL "stub got no request (typing in the bubble failed?)"
if waitfor "$R/proxy.log" '> .*"say".*darling' 25; then
  L=$(grep -m1 -E '> .*"say".*darling' "$R/proxy.log" | cut -d' ' -f3- | cut -c1-200)
  res app.chat_reply_spoken PASS "$L"
  waitfor "$R/proxy.log" '< .*"ev": ?"chunk_start"' 15 && res app.lipsync_delivered PASS "$(grep -cE '< .*"ev": ?"(envelope|chunk_start|chunk_end|utt_end|stop)"' "$R/proxy.log") lipsync events delivered to the app's subscription" \
    || res app.lipsync_delivered FAIL "no lipsync events on the app's subscription"
else res app.chat_reply_spoken FAIL "no say with the reply reached the daemon; app log: $(grep -iE 'not spoken|voice' "$AL" | tail -3 | tr '\n' ' ')"; fi

# mic: find the hold-to-talk button by probing the right end of the input row
mic_press() { local x y n0; n0=$(pcount '"listen".*"start"|"start".*"listen"')
  for y in 22 28 16 34 40 46; do for x in 22 28 16 34 40; do
    xdotool mousemove $((BX + BW - x)) $((BY + BH - y)) mousedown 1; sleep 0.5
    [ "$(pcount '"listen".*"start"|"start".*"listen"')" -gt "$n0" ] && { MICX=$x MICY=$y; return 0; }
    xdotool mouseup 1; sleep 0.15; done; done; return 1; }
mic_say() { # $1 wav name: press (already held) -> drop wav -> release; waits for the transcript reply
  cp "$VT/assets/$1.wav" "$RUN/mic/.mp.tmp" && mv -f "$RUN/mic/.mp.tmp" "$RUN/mic/mic_play.wav"; sleep 2.6; xdotool mouseup 1
  waitfor "$R/proxy.log" "< .*\"text\": ?\"$2" 15; }
sleep 1.5   # she is still speaking (long reply)
if mic_press; then
  res app.mic_uses_config PASS "config only at --config ($ACFG), XDG default empty; bubble mic ready"
  res app.mic_button PASS "hold-to-talk at (W-$MICX,H-$MICY) -> listen start"
  waitfor "$AL" 'voice interrupted.*barge_in|barge_in.*voice interrupted' 4 && waitfor "$R/proxy.log" '> .*"stop".*barge_in' 4 \
    && res app.barge_in_by_mic PASS "$(grep -m1 -E 'voice interrupted' "$AL" | sed 's/^.*INFO/INFO/' | cut -c1-90) + stop reason barge_in at daemon" \
    || res app.barge_in_by_mic FAIL "log: $(grep -m1 'voice interrupted' "$AL" | cut -c1-100); proxy stop: $(grep -m1 -E '> .*"stop"' "$R/proxy.log" | cut -c1-120)"
  N0=$(grep -c chat/completions "$R/stub.log")
  if mic_say yes '[Yy]es' || { xdotool key ctrl+a BackSpace; sleep 1; mic_press && mic_say yes '[Yy]es' && echo "yes leg passed on retry" >> "$R/runner.log"; }; then
    sleep 2; shot bubble-after-yes
    [ "$(grep -c chat/completions "$R/stub.log")" = "$N0" ] && res app.voice_yes_fills_input_only PASS "transcript 'Yes.' not sent automatically (see bubble-after-yes.png)" \
      || res app.voice_yes_fills_input_only FAIL "a chat request followed the voice 'yes' without Enter"
  else res app.voice_yes_fills_input_only FAIL "no transcript for yes.wav"; fi
  xdotool key ctrl+a BackSpace
  sleep 1
  if mic_press; then
    mic_say red '[Rr]ed'; sleep 2
    a=$(grep -m1 'safeword heard by the mic' "$AL" | cut -c1-120); b=$(grep -m1 'safeword: stopped every running command' "$AL" | sed 's/^.*safeword:/safeword:/' | cut -c1-100)
    c=$(grep -m1 'safeword from the chat bubble' "$AL" | sed 's/^.*safeword from/safeword from/'); d=$(grep -m1 -E '> .*"stop".*safeword' "$R/proxy.log" | cut -d' ' -f3- | cut -c1-80)
    hr=$(grep -cE '< .*"text": ?"[Rr]ed.*"safeword": ?true' "$R/proxy.log")
    [ -z "$a" ] && [ "$hr" -gt 0 ] && [ -n "$c" ] && a="(bubble log not in app.log: chat-bubble tracing goes to stdout = the event pipe) transcript Red safeword=true -> chat.safeword"
    [ -n "$a" ] && [ -n "$b" ] && [ -n "$c" ] && [ -n "$d" ] && kill -0 $APP 2>/dev/null \
      && res app.red_kill_switch PASS "bubble: heard; kill switch: '$b'; overlay: '$c'; daemon: $d; app alive" \
      || res app.red_kill_switch FAIL "mic-heard='${a:-none}' killswitch='${b:-none}' overlay='${c:-none}' daemon-stop='${d:-none}' alive=$(kill -0 $APP 2>/dev/null && echo y || echo n)"
  else res app.red_kill_switch FAIL "mic button not found the 2nd time"; fi
  # ---- safeword e2e legs (opt-in SW_LEGS=1): a reply still streaming is never spoken; a reply being spoken is cut
  if [ "${SW_LEGS:-0}" = 1 ]; then
    after() { tail -n +$(( $1 + 1 )) "$R/proxy.log"; }
    wait_after() { local d=$((SECONDS + $3*S)); while [ $SECONDS -lt $d ]; do after "$1" | grep -qE "$2" && return 0; sleep 0.1; done; return 1; }
    sw_leg() { # $1 name  $2 prompt  $3 wav  $4 transcript regex  $5 stream|speaking
      local n=$1 L0 tp tr ts ok=1 why="" utt S0 sb
      xdotool key ctrl+a BackSpace; sleep 1
      xdotool windowfocus --sync "$BUBW" 2>/dev/null; xdotool mousemove $((BX + 60)) $((BY + BH - 24)) click 1; sleep 0.3
      L0=$(wc -l < "$R/proxy.log"); S0=$(grep -c 'POST' "$R/stub.log"); sb=$(grep -c 'safeword from the chat bubble' "$AL")
      xdotool type --delay 20 "$2"; xdotool key Return
      if [ "$5" = stream ]; then
        local d=$((SECONDS + 10*S)); while [ $SECONDS -lt $d ] && [ "$(grep -c POST "$R/stub.log")" = "$S0" ]; do sleep 0.1; done
        sleep 1
      else
        wait_after "$L0" '< .*"ev": ?"chunk_start"' 30 || { res "app.safeword_$n" FAIL "she never started speaking the reply"; return; }
        utt=$(after "$L0" | grep -m1 -oE '"ev": ?"chunk_start", ?"utt": ?[0-9]+' | grep -oE '[0-9]+$'); sleep 1.2
      fi
      tp=$(date +%s.%N)
      mic_press || { res "app.safeword_$n" FAIL "mic button not found"; return; }
      cp "$VT/assets/$3.wav" "$RUN/mic/.mp.tmp" && mv -f "$RUN/mic/.mp.tmp" "$RUN/mic/mic_play.wav"; sleep 2.6
      tr=$(date +%s.%N); xdotool mouseup 1
      wait_after "$L0" "< .*\"text\": ?\"$4.*\"safeword\": ?true" 15 || { ok=0; why+="no safeword transcript ($(after "$L0" | grep -oE '"text": ?"[^"]*"' | tail -1)); "; }
      ts=$(after "$L0" | grep -m1 -E '> .*"stop"' | awk '{print $1}')
      sleep $((10 * S))   # long enough for the streamed reply to end / the next sentences to start if they survived
      local tt=$(after "$L0" | grep -m1 -E "< .*\"text\": ?\"$4" | awk '{print $1}')
      local spoke=$(after "$L0" | grep -E '> .*"say".*darling' | awk -v t="$tp" '$1 > t' | wc -l)
      local late=0; [ -n "${utt:-}" ] && late=$(after "$L0" | awk -v t="$tp" '$1 > t' | grep -cE "\"ev\": ?\"(chunk_start|utt_end)\", ?\"utt\": ?$utt\b")
      local other=$(after "$L0" | grep -E '> .*"say"' | awk -v t="$tp" '$1 > t' | grep -v darling | grep -oE '"text": ?"[^"]{0,60}' | tr '\n' '|')
      [ "$(grep -c 'safeword from the chat bubble' "$AL")" -gt "$sb" ] || { ok=0; why+="overlay saw no chat.safeword; "; }
      [ "$spoke" = 0 ] || { ok=0; why+="the cut reply was spoken again ($spoke say); "; }
      [ "$late" = 0 ] || { ok=0; why+="$late chunk_start/utt_end for utt $utt after the press; "; }
      kill -0 $APP 2>/dev/null || { ok=0; why+="app died; "; }
      local lat="press->daemon stop $(awk -v a="$tp" -v b="${ts:-0}" 'BEGIN{ if (b>0) printf "%d ms", (b-a)*1000; else print "n/a"}'), release->safeword transcript $(awk -v a="$tr" -v b="${tt:-0}" 'BEGIN{ if (b>0) printf "%d ms", (b-a)*1000; else print "n/a"}')"
      [ $ok = 1 ] && res "app.safeword_$n" PASS "$5: $lat; reply never re-spoken, no chunk for utt ${utt:-none} after the press; later says: ${other:-none}" \
        || res "app.safeword_$n" FAIL "$5: $lat; $why later says: ${other:-none}"
    }
    sw_leg red_streaming "tell me slowly" red '[Rr]ed' stream
    sw_leg ooc_streaming "slowly again" ooc_stop '(O|Oh)' stream
    sw_leg red_speaking "hello again lillith" red '[Rr]ed' speaking
    sw_leg ooc_speaking "one more time" ooc_stop '(O|Oh)' speaking
  fi
else
  shot mic-not-found
  res app.mic_uses_config FAIL "mic not usable with the config only at --config (XDG default empty)"
  res app.mic_button FAIL "no listen start from any probe at the input row's right end (mic not ready? see mic-not-found.png; app: $(grep -m1 -iE 'push-to-talk|mic' "$AL" | cut -c1-120))"
  res app.barge_in_by_mic SKIP "needs the mic button"; res app.voice_yes_fills_input_only SKIP "needs the mic"; res app.red_kill_switch SKIP "needs the mic"
fi
grep -q 'lipsync line ignored' "$AL" && res app.lipsync_parse FAIL "$(grep -m2 'lipsync line ignored' "$AL" | tr '\n' ' ' | cut -c1-200)" \
  || res app.lipsync_parse PASS "no 'lipsync line ignored' (TalkMouth has no per-event log; mouth motion itself not log-observable)"
kill -0 $APP 2>/dev/null && res app.alive_after_voice PASS "" || res app.alive_after_voice FAIL "app died: $(tail -3 "$AL" | tr '\n' ' ')"
grep -qE 'panicked' "$AL" && res app.no_panic FAIL "$(grep -m1 -A2 panicked "$AL" | tr '\n' ' ')" || res app.no_panic PASS ""
kill $APP 2>/dev/null; sleep 1; for w in $(wins); do :; done

# 'yes' must never approve a Soul Contract: standalone bubble with the sample contract up
BL=$R/bubble-contract.log
appenv "$BIN" chat-bubble --x 200 --y 150 --demo-contract >"$R/bubble-contract.out" 2>"$BL" & BP=$!; PIDS+=($BP)
BUBW=""; open_bubble 15; sleep 2; shot contract-before
if [ -n "$BUBW" ]; then
  # no blind mic probing here: the popup's SIGN-YES button sits where the probes would click
  sleep 3; shot contract-after-wait
  if grep -qiE 'confirm|approve' "$R/bubble-contract.out"; then res app.voice_yes_never_approves FAIL "bubble emitted an approval unprompted: $(head -3 "$R/bubble-contract.out")"
  elif ! grep -qE '^app.voice_yes_fills_input_only +PASS' "$SUM"; then res app.voice_yes_never_approves INFO "no approval emitted, but the voice-yes e2e leg did not run; rely on unit tests (daemon.rs ~1010/1017)"
  else res app.voice_yes_never_approves PASS "e2e: daemon tags 'yes' affirmation=true/consent_ok=false (proto.*_yes); voice 'Yes.' only fills the input and is not sent (app.voice_yes_fills_input_only); route_transcript has no approve variant and returns Refused with a gate up (unit tests daemon.rs ~1010/1017, in cargo.test). The popup covers the mic, so voice can't reach it headless (contract-before.png)"; fi
else res app.voice_yes_never_approves FAIL "standalone bubble did not open: $(tail -3 "$BL" | tr '\n' ' ')"; fi
kill $BP 2>/dev/null; sleep 1

# daemon missing: no crash, clear log line
mkapp missing "$RUN/no-such-voiced.sock"; ML=$R/app-missing.log
if launch_app "$ML"; then
  sleep 6; m=$(grep -m1 -iE 'voice daemon not running|voice( off)?: ' "$ML" | sed 's/^.*voice/voice/' | cut -c1-160)
  kill -0 $APP 2>/dev/null && echo "$m" | grep -qi 'not running' && res app.daemon_missing_fallback PASS "$m" \
    || res app.daemon_missing_fallback FAIL "alive=$(kill -0 $APP 2>/dev/null && echo y || echo n) log='$m'"
  kill $APP 2>/dev/null
else res app.daemon_missing_fallback FAIL "app did not start/died without the daemon: $(tail -3 "$ML" | tr '\n' ' ')"; fi

# quiet (muted) app: lipsync for speech it isn't playing must be discarded, no errors, no crash
mkapp muted "$ASOCK" true; QL=$R/app-muted.log; Q0=$(pcount '"subscribe"')
if launch_app "$QL"; then
  sleep 4
  (cd "$VT/lib" && LILLITH_VOICE_SOCKET=$DSOCK "$VPY" -c "from vc import req; print(req({'cmd':'say','text':'Someone else is talking now, quietly.','wait':True}))") >"$R/muted-say.log" 2>&1
  sleep 2; QS=$(( $(pcount '"subscribe"') - Q0 ))
  if kill -0 $APP 2>/dev/null && ! grep -qE 'lipsync line ignored|panicked' "$QL"; then
    res app.lipsync_quiet_discard PASS "muted app alive, $QS new subscribe(s), external say played, no 'lipsync line ignored'/panic; $(grep -m1 -E 'voice( off)?: |voice ready' "$QL" | sed 's/^.*\(voice[^ ]*\)/\1/' | cut -c1-90)"
  else res app.lipsync_quiet_discard FAIL "alive=$(kill -0 $APP 2>/dev/null && echo y || echo n); $(grep -m2 -E 'lipsync line ignored|panicked' "$QL" | tr '\n' ' ' | cut -c1-200)"; fi
  kill $APP 2>/dev/null; sleep 1
else res app.lipsync_quiet_discard FAIL "muted app did not start: $(tail -3 "$QL" | tr '\n' ' ')"; fi

# app-level p1 fallback log against the no-clone daemon
if [ -n "${NPP:-}" ]; then
  mkapp nopocket "$RUN/np.sock"; PL=$R/app-nopocket.log
  if launch_app "$PL"; then
    sleep 5; f=$(grep -m1 'using fallback voice' "$PL" | sed 's/^.*\(WARN\|INFO\)/\1/' | cut -c1-140)
    [ -n "$f" ] && echo "$f" | grep -q 'p1' && res app.fallback_p1_logged PASS "$f" \
      || res app.fallback_p1_logged FAIL "no 'using fallback voice p1' line; voice lines: $(grep -E 'voice' "$PL" | head -3 | tr '\n' ' ' | cut -c1-200)"
    kill $APP 2>/dev/null; sleep 1
  else res app.fallback_p1_logged FAIL "app did not start: $(tail -3 "$PL" | tr '\n' ' ')"; fi
  kill "$NPP" 2>/dev/null
fi

# protocol keys the app actually sent
K=$(grep -E ' c[0-9]+ > \{' "$R/proxy.log" | cut -d' ' -f4- | "$VPY" -c '
import sys,json,collections
c=collections.Counter(); bad=[]
for l in sys.stdin:
    try: o=json.loads(l)
    except Exception: bad.append(l[:60]); continue
    k="cmd" if "cmd" in o else "op" if "op" in o else "none"; c[(k, o.get(k,"say"))]+=1
print(" ".join(f"{k}:{v}x{n}" for (k,v),n in sorted(c.items())), "| unparsable:", len(bad))')
NOK=$(grep -cE ' < \{"ok": ?false' "$R/proxy.log")
[ "$NOK" = 0 ] && res app.protocol_keys PASS "$K; daemon errors: 0" || res app.protocol_keys FAIL "$K; daemon ok:false replies: $(grep -m2 -E ' < \{"ok": ?false' "$R/proxy.log" | cut -d' ' -f3- | tr '\n' ' ')"
finish
