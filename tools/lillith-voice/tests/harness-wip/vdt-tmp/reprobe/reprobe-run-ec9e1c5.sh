#!/usr/bin/env bash
# reprobe-run.sh: app-side e2e for fix/voice-client-reprobe (late daemon re-probe, timeouts).
# env: BIN=<lillith binary> WT=<worktree for manifest> RESDIR=<results dir>. Kills only PIDs it started.
set -uo pipefail
VT=/workspace/lillith/scratch/vdt-tmp; RP=$VT/reprobe
KIT=/workspace/lillith/lillith-voice-kit; LV=$VT/home/.local/share/lillith-voice; VPY=$LV/venv/bin/python
BIN=${BIN:?}; WT=${WT:?}; R=${RESDIR:?}; RUN=$VT/run-reprobe-$(date +%H%M%S)
mkdir -p "$R" "$RUN"; chmod 700 "$RUN"; SUM=$R/summary.txt; : > "$SUM"; PIDS=()
log() { printf '[%s] %s\n' "$(date +%T.%3N)" "$*" | tee -a "$R/runner.log" >&2; }
res() { printf '%-34s %-5s %s\n' "$1" "$2" "${3:-}" | tee -a "$SUM" >&2; }
now() { date +%s.%N; }
ms() { awk -v a="$1" -v b="$2" 'BEGIN{ if (a=="" || b=="") print "n/a"; else printf "%d", (b-a)*1000 }'; }
ts() { "$VPY" "$RP/tsof.py" "$@" 2>/dev/null; }
cleanup() { for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done; sleep 1; for p in "${PIDS[@]}"; do kill -9 "$p" 2>/dev/null; done; rm -rf "$RUN"; log "cleaned up"; }
trap cleanup EXIT; trap 'exit 130' INT TERM
waitfor() { local d=$((SECONDS + $3)); while [ $SECONDS -lt $d ]; do grep -qE "$2" "$1" 2>/dev/null && return 0; sleep 0.1; done; return 1; }
mkdir -p "$RUN/played" "$RUN/mic" "$RUN/dxdg" "$RUN/drt" "$RUN/shim"; chmod 700 "$RUN/drt"
cp "$RP/espeak-ng" "$RUN/shim/espeak-ng"; ESL=$R/espeak-shim.log; : > "$ESL"
DSOCK=$RUN/voiced.sock; ASOCK=$RUN/app.sock
start_daemon() { # $1 log
  env -i PATH=/usr/bin:/bin HOME="$VT/home" XDG_CONFIG_HOME="$RUN/dxdg" XDG_RUNTIME_DIR="$RUN/drt" \
    LILLITH_VOICE_SOCKET="$DSOCK" LILLITH_PLAYER="$VPY $KIT/tests/fake_player.py {} $RUN/played/" \
    LILLITH_MIC_CMD="$VPY $KIT/tests/fake_mic.py $RUN/mic" "$VT/home/.local/bin/lillith-voiced" >>"$1" 2>&1 &
  DPID=$!; PIDS+=($DPID); local i; for i in $(seq 240); do [ -S "$DSOCK" ] && grep -q ready "$1" && return 0; sleep 0.25; done; return 1; }
start_proxy() { "$VPY" "$VT/lib/proxy.py" "$ASOCK" "$DSOCK" "$R/proxy.log" >>"$R/proxy.err" 2>&1 & XPID=$!; PIDS+=($XPID)
  for _ in $(seq 50); do [ -S "$ASOCK" ] && return 0; sleep 0.05; done; return 1; }
# ---- X + stub LLM
DISP=""; for n in $(seq 140 199); do [ -e "/tmp/.X11-unix/X$n" ] || [ -e "/tmp/.X$n-lock" ] || { DISP=":$n"; break; }; done
Xvfb "$DISP" -screen 0 1280x800x24 -br -nolisten tcp -noreset +extension Composite >"$R/xvfb.log" 2>&1 & PIDS+=($!)
for _ in $(seq 100); do [ -e "/tmp/.X11-unix/X${DISP#:}" ] && break; sleep 0.1; done
export DISPLAY=$DISP; unset WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS PULSE_SERVER
REPLY="Of course, darling. I am right here with you, and I will keep talking for a little while so that you can hear me. Still here, still talking."
"$VPY" -u "$VT/lib/stub.py" "$RUN/port" "$REPLY" >"$R/stub.log" 2>&1 & PIDS+=($!)
for _ in $(seq 50); do [ -s "$RUN/port" ] && break; sleep 0.1; done; EP="http://127.0.0.1:$(cat "$RUN/port")"
MAN=$WT/assets/generated/manifest.toml
mkapp() { AH=$RUN/app-$1; mkdir -p "$AH/home" "$AH/cfg/lillith" "$AH/data" "$AH/state" "$AH/cache" "$AH/rt" "$AH/custom"; chmod 700 "$AH/rt"; ACFG=$AH/custom/config.toml
  cat >"$ACFG" <<TOML
mode = "stay"
muted = false
voice_enabled = true
llm_backend = "llama-server"
llm_url = "$EP"
autostart = false
[voice]
enabled = true
mute = false
engine = "daemon"
daemon_socket = "$2"
preset = "lillith"
cooldown_s = 0.0
repeat_cooldown_s = 0.0
max_chars = 400
[voice.stt]
enabled = false
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
timeout_s = 20.0
TOML
}
viewable() { xwininfo -id "$1" 2>/dev/null | grep -q 'Map State: IsViewable'; }
wins() { local w; for w in $(xdotool search --name '^Lillith$' 2>/dev/null); do viewable "$w" && echo "$w"; done; }
geom() { xwininfo -id "$1" | awk '/Absolute upper-left X:/{x=$4} /Absolute upper-left Y:/{y=$4} /Width:/{w=$2} /Height:/{h=$2} END{print x, y, w, h}'; }
launch_app() { # $1 log
  env -i PATH="$RUN/shim:/usr/bin:/bin" ESPEAK_SHIM_LOG="$ESL" DISPLAY="$DISP" HOME="$AH/home" XDG_CONFIG_HOME="$AH/cfg" XDG_DATA_HOME="$AH/data" \
    XDG_STATE_HOME="$AH/state" XDG_CACHE_HOME="$AH/cache" XDG_RUNTIME_DIR="$AH/rt" NO_AT_BRIDGE=1 RUST_BACKTRACE=1 \
    RUST_LOG="info,lillith=debug,lillith_voice=debug,lillith_overlay=info" \
    "$BIN" --config "$ACFG" --manifest "$MAN" --backend x11 --no-tray --no-monitor --no-save --reset-position --mode stay --exit-after 900 >"$1" 2>&1 &
  APP=$!; PIDS+=($APP); T_LAUNCH=$(now)
  MAINW=""; local d=$((SECONDS + 40)); while [ $SECONDS -lt $d ]; do kill -0 $APP 2>/dev/null || return 1; MAINW=$(wins | head -1); [ -n "$MAINW" ] && return 0; sleep 0.3; done; return 1; }
open_bubble_ui() { read -r WX WY WW WH < <(geom "$MAINW")
  xdotool windowactivate --sync "$MAINW" 2>/dev/null || xdotool windowfocus "$MAINW" 2>/dev/null; xdotool key --window "$MAINW" Return 2>/dev/null
  BUBW=""; ob() { local d=$((SECONDS + $1)); while [ $SECONDS -lt $d ]; do BUBW=$(wins | grep -vx "$MAINW" | head -1); [ -n "$BUBW" ] && return 0; sleep 0.3; done; return 1; }
  ob 6 || { xdotool mousemove $((WX + WW / 2)) $((WY + WH / 2)) click 1; ob 10; } || return 1
  read -r BX BY BW BH < <(geom "$BUBW"); sleep 1.5; }
chat() { chat1 "$1" && return 0; log "chat retry: $1"; sleep 2; chat1 "$1"; }
chat1() { # $1 text -> sets T_CHAT (epoch when Return pressed); waits for the stub POST
  local n0; n0=$(grep -c POST "$R/stub.log")
  xdotool key ctrl+a BackSpace; xdotool windowfocus --sync "$BUBW" 2>/dev/null; xdotool mousemove $((BX + 60)) $((BY + BH - 24)) click 1; sleep 0.3
  xdotool windowfocus --sync "$BUBW" 2>/dev/null; xdotool type --delay 20 "$1"; T_CHAT=$(now); xdotool key Return
  local d=$((SECONDS + 15)); while [ $SECONDS -lt $d ]; do [ "$(grep -c POST "$R/stub.log")" -gt "$n0" ] && return 0; sleep 0.1; done; return 1; }
stop_app() { local k; k=$(pgrep -P $APP | tr '\n' ' '); kill $k $APP 2>/dev/null; sleep 1; kill -9 $k $APP 2>/dev/null
  local d=$((SECONDS + 10)); while [ $SECONDS -lt $d ] && [ -n "$(wins)" ]; do sleep 0.3; done; [ -z "$(wins)" ] || log "WARN: windows left: $(wins | tr '\n' ' ')"; }
DL=$R/daemon.log
start_daemon "$DL" || { res daemon.start FAIL "$(tail -2 "$DL")"; exit 1; }
res daemon.start PASS "real lillith-voiced ready on hidden socket (app sees nothing until the proxy starts)"

if [ "${ONLY_DE:-0}" != 1 ]; then
######## LEG A: points 1-5 (timer trigger, between lines, lipsync)
log "LEG A"; mkapp a "$ASOCK"; AL=$R/app-a.log
launch_app "$AL" || { res A.app_start FAIL "$(tail -3 "$AL" | tr '\n' ' ')"; exit 1; }
sleep 2
eng=$(grep -m1 'voice ready' "$AL" | sed 's/\x1b\[[0-9;]*m//g' | grep -oE 'engine="[^"]*"')
vl=$(grep -m1 -E 'lillith_voice::speech.*voice( off)?:' "$AL" | sed 's/\x1b\[[0-9;]*m//g' | sed 's/^.*voice/voice/' | cut -c1-160)
[ "$eng" = 'engine="espeak-ng"' ] && [ ! -e "$ASOCK" ] && res 1.start_daemon_down PASS "config engine=daemon, socket absent -> $eng; log: '$vl'" || res 1.start_daemon_down FAIL "$eng; $vl"
wa=$(sed 's/\x1b\[[0-9;]*m//g' "$AL" | grep -m1 -E 'WARN.*voice: voice daemon not running \(.*\); using the fallback espeak-ng voice until it starts' | sed 's/^.*WARN/WARN/' | cut -c1-220)
[ -n "$wa" ] && res a.startup_warn PASS "$wa" || res a.startup_warn FAIL "no startup WARN; voice lines: $(sed 's/\x1b\[[0-9;]*m//g' "$AL" | grep -E 'voice' | head -3 | cut -c1-150 | tr '\n' '|')"
wb=$(sed 's/\x1b\[[0-9;]*m//g' "$AL" | grep -m1 -E 'WARN.*voice ready \(fallback voice\)' | sed 's/^.*WARN/WARN/' | cut -c1-260)
echo "$wb" | grep -q 'engine="espeak-ng"' && res b.voice_ready_warn PASS "$wb" || res b.voice_ready_warn FAIL "got: '${wb:-none}'"
open_bubble_ui || { res A.bubble FAIL "bubble did not open"; exit 1; }
sleep $(awk -v t="$T_LAUNCH" -v n="$(now)" 'BEGIN{s=12-(n-t); if (s<0) s=0; print s}')   # >= REPROBE_MIN_GAP after the start-up probe
chat "hello lillith" || res A.chat1 FAIL "stub got no request"
waitfor "$ESL" 'START.*darling' 20 && res 2.fallback_speaks PASS "espeak-ng shim started the reply ($(ms "$T_CHAT" "$(ts "$ESL" 'START.*darling' "$T_CHAT")") ms after Return)" || res 2.fallback_speaks FAIL "fallback never spoke"
T_P1=$(ts "$AL" 'still not answering' "$T_LAUNCH")
log "first failed re-probe at $T_P1 (launch $T_LAUNCH)"
sleep 4; T_UP=$(now); start_proxy || res A.proxy FAIL "proxy did not start"; log "daemon UP (proxy socket) at $T_UP"
waitfor "$AL" 'switching to the cloned voice' 45; T_SW=$(ts "$AL" 'switching to the cloned voice' "$T_LAUNCH")
LAST=$(grep -E 'still not answering' "$AL" | wc -l); T_LASTP=$(ts "$AL" 'still not answering' "$T_LAUNCH" "$LAST")
if [ -n "$T_SW" ]; then
  d_up=$(ms "$T_UP" "$T_SW"); d_last=$(ms "$T_LASTP" "$T_SW")
  [ "$d_last" -le 31500 ] && res 4.switch_window PASS "switch logged $d_up ms after daemon up; $d_last ms after the last failed probe (period 30 s); failed probes before: $LAST" \
    || res 4.switch_window FAIL "switch $d_up ms after up, $d_last ms after last probe (> 30 s)"
else res 4.switch_window FAIL "no switch logged within 45 s of daemon up"; fi
# mid-line: the espeak line running at the switch must end on its own and no daemon say may start before it ends
waitfor "$ESL" 'END|KILLED' 45
SPID=$(grep -m1 'START.*darling' "$ESL" | grep -oE 'pid=[0-9]+'); T_END=$(grep -E "(END|KILLED) $SPID\b" "$ESL" | awk '{print $1}' | head -1); kind=$(grep -E "(END|KILLED) $SPID\b" "$ESL" | awk '{print $2}' | head -1)
T_SAY0=$(ts "$R/proxy.log" '> .*"say"' "$T_UP")
[ "$kind" = END ] && { [ -z "$T_SAY0" ] || awk -v a="$T_SAY0" -v b="$T_END" 'BEGIN{exit !(a>=b)}'; } && awk -v s="$T_SW" -v e="$T_END" 'BEGIN{exit !(s<e)}' \
  && MID="espeak line ($SPID) was still playing at the switch (switch $(ms "$T_SW" "$T_END") ms before its END), ended normally; no daemon say before it" \
  || MID="espeak line $SPID: $kind at $T_END; switch $T_SW; first daemon say $T_SAY0"
sleep 1; chat "hello again lillith"; T_C2=$T_CHAT
if waitfor "$R/proxy.log" '> .*"say".*darling' 20; then
  sl=$(grep -m1 -E '> .*"say".*darling' "$R/proxy.log" | cut -d' ' -f3- | grep -oE '"preset": ?"[^"]*"')
  esp2=$(awk -v t="$T_C2" '$1>t && /START/' "$ESL" | wc -l)
  echo "$MID" | grep -q 'ended normally' && echo "$sl" | grep -q '"lillith"' && [ "$esp2" = 0 ] \
    && res 4.switch_between_lines PASS "$MID; next reply -> daemon say $sl ($(ms "$T_C2" "$(ts "$R/proxy.log" '> .*"say".*darling' "$T_C2")") ms after Return), espeak not used again" \
    || res 4.switch_between_lines FAIL "$MID; next say $sl; espeak starts after: $esp2"
  waitfor "$R/proxy.log" '< .*"ev": ?"chunk_start"' 20
  SUB=$(ts "$R/proxy.log" '> .*"subscribe"' "$T_UP"); ev=$(grep -cE '< .*"ev": ?"(envelope|chunk_start|chunk_end|utt_end)"' "$R/proxy.log")
  kinds=$(grep -oE '< .*"ev": ?"[a-z_]+"' "$R/proxy.log" | grep -oE '"[a-z_]+"$' | sort | uniq -c | tr -s ' ' | tr '\n' ',')
  [ -n "$SUB" ] && [ "$ev" -gt 0 ] && res 5.lipsync_after_switch PASS "subscribe $(ms "$T_SW" "$SUB") ms after the switch; events: $kinds" \
    || res 5.lipsync_after_switch FAIL "subscribe=${SUB:-none} events=$ev"
else res 4.switch_between_lines FAIL "$MID; next reply never reached the daemon"; res 5.lipsync_after_switch SKIP ""; fi
waitfor "$R/proxy.log" '< .*"ev": ?"utt_end"' 60; sleep 1

######## LEG C: point 6 / (c) / (d): daemon killed mid-line, returns; twice
log "LEG C"
WV() { sed 's/\x1b\[[0-9;]*m//g' "$AL" | grep -cE "WARN lillith_voice::speech: voice: "; }
SWN() { grep -c 'switching to the cloned voice' "$AL"; }
lastprobe() { # epoch of the latest probe: failed-probe log line or a ping seen by the proxy
  local n a="" b=""; n=$(grep -cE 'still not answering' "$AL"); [ "$n" -gt 0 ] && a=$(ts "$AL" 'still not answering' 0 "$n")
  n=$(grep -cE '> \{"cmd":"ping"' "$R/proxy.log"); [ "$n" -gt 0 ] && b=$(ts "$R/proxy.log" '> \{"cmd":"ping"' 0 "$n")
  awk -v a="${a:-0}" -v b="${b:-0}" 'BEGIN{print (a>b)?a:b}'; }
espwait() { local d=$((SECONDS + ${2:-10})) t=""; while [ $SECONDS -lt $d ]; do t=$(ts "$ESL" 'START.*darling' "$1"); [ -n "$t" ] && { echo "$t"; return 0; }; sleep 0.05; done; return 1; }
saywait() { local d=$((SECONDS + ${2:-10})) t=""; while [ $SECONDS -lt $d ]; do t=$(ts "$R/proxy.log" '> .*"say".*darling' "$1"); [ -n "$t" ] && { echo "$t"; return 0; }; sleep 0.05; done; return 1; }
W0=$(WV); S0=$(SWN)
chat "talk to me lillith"; T_C3=$T_CHAT
d=$((SECONDS + 20)); while [ $SECONDS -lt $d ] && [ -z "$(ts "$R/proxy.log" '< .*"ev": ?"chunk_start"' "$T_C3")" ]; do sleep 0.1; done
if [ -n "$(ts "$R/proxy.log" '< .*"ev": ?"chunk_start"' "$T_C3")" ]; then
  sleep 1.5; T_KILL=$(now); kill -9 "$DPID" "$XPID"; log "kill#1 daemon+proxy SIGKILLed at $T_KILL (sockets left behind like a crash)"
  te=$(espwait "$T_KILL" 10); tw=$(ts "$AL" 'WARN lillith_voice::speech: voice: ' "$T_KILL")
  wl=$(sed 's/\x1b\[[0-9;]*m//g' "$AL" | grep -E 'WARN lillith_voice::speech: voice: ' | tail -1 | sed 's/^.*WARN/WARN/' | cut -c1-200)
  sleep 3; alive1=$(kill -0 $APP 2>/dev/null && echo y || echo n)
  chat "are you there lillith" && ui="UI ok" || ui="UI did NOT send a chat request"; T_C4=$T_CHAT
  te4=$(espwait "$T_C4" 10); sy4=$(ts "$R/proxy.log" '> .*"say"' "$T_KILL")
  [ "$alive1" = y ] && echo "$ui" | grep -q 'UI ok' && res 6a.kill_no_hang PASS "app alive, $ui after the kill" || res 6a.kill_no_hang FAIL "alive=$alive1 $ui"
  W1=$(WV); [ -n "$te" ] && [ $((W1 - W0)) = 1 ] && res c1.failed_line_replayed PASS "cut line re-spoken on espeak $(ms "$T_KILL" "$te") ms after the kill; 1 WARN ($(ms "$T_KILL" "$tw") ms): $wl" \
    || res c1.failed_line_replayed FAIL "espeak replay=${te:-none}; WARNs since kill=$((W1 - W0)): $wl"
  [ -n "$te4" ] && res c2.later_lines_on_fallback PASS "next reply on espeak $(ms "$T_C4" "$te4") ms after Return; daemon says after kill: ${sy4:-none}" || res c2.later_lines_on_fallback FAIL "next reply not on espeak"
  # daemon returns while the 40 s espeak line plays: timer re-probe, switch between lines
  rm -f "$DSOCK"; T_UP2=$(now); start_daemon "$DL" && start_proxy; T_BACK=$(now); log "daemon back#1 at $T_BACK (start took $(ms "$T_UP2" "$T_BACK") ms)"
  d=$((SECONDS + 45)); T_SW2=""; while [ $SECONDS -lt $d ] && [ -z "$T_SW2" ]; do T_SW2=$(ts "$AL" 'switching to the cloned voice' "$T_BACK"); sleep 0.2; done
  LP=$(sed 's/\x1b\[[0-9;]*m//g' "$AL" | grep -E 'still not answering' | tail -1 | cut -c1-30)
  SP4=$(awk -v t="$T_C4" '$1>t && /START.*darling/{print $3; exit}' "$ESL"); T_END4=$(grep -E "(END|KILLED) $SP4\b" "$ESL" | awk '{print $1}' | head -1); k4=$(grep -E "(END|KILLED) $SP4\b" "$ESL" | awk '{print $2}' | head -1)
  sleep 1; while [ -z "$k4" ] && [ $SECONDS -lt $((d+10)) ]; do sleep 0.5; T_END4=$(grep -E "(END|KILLED) $SP4\b" "$ESL" | awk '{print $1}' | head -1); k4=$(grep -E "(END|KILLED) $SP4\b" "$ESL" | awk '{print $2}' | head -1); done
  fs=$(ts "$R/proxy.log" '> .*"say"' "$T_BACK")
  S1=$(SWN); inf=$((S1 - S0))
  [ -n "$T_SW2" ] && [ "$k4" = END ] && awk -v s="$T_SW2" -v e="$T_END4" 'BEGIN{exit !(s<e)}' && { [ -z "$fs" ] || awk -v a="$fs" -v b="$T_END4" 'BEGIN{exit !(a>=b)}'; } && [ "$inf" = 1 ] \
    && res c3.switch_back_between_lines PASS "switch back $(ms "$T_BACK" "$T_SW2") ms after the daemon returned (timer; last failed probe $LP); espeak line $SP4 still playing ($(ms "$T_SW2" "$T_END4") ms left), ended normally; first daemon say after it; 1 INFO" \
    || res c3.switch_back_between_lines FAIL "switch=${T_SW2:-none} espeak $SP4 $k4 at $T_END4; first say ${fs:-none}; INFOs=$inf"
  chat "welcome back lillith"; T_C5=$T_CHAT; got=$(saywait "$T_C5" 15)
  sleep 5; SUB2=$(ts "$R/proxy.log" '> .*"subscribe"' "$T_BACK"); EV2=$(ts "$R/proxy.log" '< .*"ev": ?"chunk_start"' "$T_C5")
  [ -n "$got" ] && [ -n "$SUB2" ] && [ -n "$EV2" ] && res c4.recover_say_lipsync PASS "reply on the daemon $(ms "$T_C5" "$got") ms after Return; lip-sync re-subscribed $(ms "$T_BACK" "$SUB2") ms after the daemon returned; chunk_start delivered" \
    || res c4.recover_say_lipsync FAIL "say=${got:-none} resubscribe=${SUB2:-none} chunk_start=${EV2:-none}"
  # ---- kill #2, then (d): first line after the return
  W2=$(WV); S2=$(SWN); d=$((SECONDS + 20)); while [ $SECONDS -lt $d ] && [ -z "$(ts "$R/proxy.log" '< .*"ev": ?"chunk_start"' "$T_C5")" ]; do sleep 0.1; done; sleep 1
  chat "one more story lillith"; T_C6=$T_CHAT
  d=$((SECONDS + 20)); while [ $SECONDS -lt $d ] && [ -z "$(ts "$R/proxy.log" '< .*"ev": ?"chunk_start"' "$T_C6")" ]; do sleep 0.1; done; sleep 1.5
  T_KILL2=$(now); kill -9 "$DPID" "$XPID"; log "kill#2 at $T_KILL2"
  te6=$(espwait "$T_KILL2" 10); sleep 2
  rm -f "$DSOCK"; start_daemon "$DL" && start_proxy; T_BACK2=$(now); LP2=$(lastprobe); log "daemon back#2 at $T_BACK2; last probe $LP2"
  # first line after the return, < 10 s since the last probe
  sleep 0.5; chat "quick hello lillith"; T_C7=$T_CHAT; LPa=$(lastprobe)
  sleep 2; e7=$(ts "$ESL" 'START.*darling' "$T_C7"); s7=$(ts "$R/proxy.log" '> .*"say".*darling' "$T_C7")
  gap7=$(ms "$LPa" "$T_C7")
  # then the first line >= 10 s after the last probe
  LPb=$(lastprobe); s=$(awk -v p="$LPb" -v n="$(now)" 'BEGIN{s=10.5-(n-p); if (s<0) s=0; print s}'); sleep "$s"
  chat "now are you back lillith"; T_C8=$T_CHAT; s8=$(saywait "$T_C8" 5); sw8=$(ts "$AL" 'switching to the cloned voice' "$T_C8"); e8=$(ts "$ESL" 'START.*darling' "$T_C8")
  W3=$(WV); S3=$(SWN)
  res c5.kill2_transitions $( [ -n "$te6" ] && [ $((W3 - W2)) = 1 ] && [ $((S3 - S2)) -le 1 ] && echo PASS || echo FAIL) "kill#2: replay on espeak $(ms "$T_KILL2" "$te6") ms after the kill; WARNs this outage $((W3 - W2)), switch INFOs $((S3 - S2))"
  if [ -n "$s7" ]; then d7="line 1 after return (${gap7} ms after last probe) went to the daemon $(ms "$T_C7" "$s7") ms after Return"; else d7="line 1 after return (${gap7} ms after last probe, < 10 s rate limit) spoken by ${e7:+espeak (+$(ms "$T_C7" "$e7") ms)}${e7:-nothing}, no check"; fi
  [ -n "$s8" ] && [ -n "$sw8" ] && [ -z "$e8" ] && res d.first_line_after_return_checks PASS "$d7; line $(ms "$LPb" "$T_C8") ms after last probe: switch logged $(ms "$T_C8" "$sw8") ms and that same line said on the daemon $(ms "$T_C8" "$s8") ms after Return" \
    || res d.first_line_after_return_checks FAIL "$d7; line at +$(ms "$LPb" "$T_C8") ms: switch=${sw8:-none} say=${s8:-none} espeak=${e8:-none}"
  res c6.warn_info_totals INFO "app-a totals: WARN 'voice:' lines $(WV) (expect 3: start + 2 outages), switch INFOs $(SWN) (expect 3)"
else res 6a.kill_no_hang FAIL "she never started speaking before the kill"; fi
stop_app; kill "$XPID" 2>/dev/null
grep -q panicked "$AL" && res A.no_panic FAIL "$(grep -m1 panicked "$AL")" || res A.no_panic PASS ""

######## LEG B: say-kick trigger + rate limit
log "LEG B"; kill "$XPID" 2>/dev/null; rm -f "$ASOCK"; mkapp b "$ASOCK"; BL=$R/app-b.log
launch_app "$BL" && open_bubble_ui || { res B.app FAIL "start/bubble"; }
sleep $(awk -v t="$T_LAUNCH" -v n="$(now)" 'BEGIN{s=12-(n-t); if (s<0) s=0; print s}')
chat "hello lillith"; T_B1=$T_CHAT; sleep 1.5
T_PB1=$(ts "$BL" 'still not answering' "$T_LAUNCH"); start_proxy; T_UPB=$(now)
sleep 3; chat "quick one lillith"; T_B2=$T_CHAT     # < 10 s after the last probe: must NOT probe
sleep 4; nB2=$(awk -v t="$T_B2" '1' "$BL" | grep -cE 'switching to the cloned|still not answering'); swB=$(ts "$BL" 'switching to the cloned voice' "$T_B2")
s=$(awk -v p="$T_PB1" -v n="$(now)" 'BEGIN{s=10.5-(n-p); if (s<0) s=0; print s}'); sleep "$s"
chat "now lillith"; T_B3=$T_CHAT
waitfor "$BL" 'switching to the cloned voice' 5; swB3=$(ts "$BL" 'switching to the cloned voice' "$T_LAUNCH")
[ -z "$swB" ] && [ -n "$swB3" ] && awk -v a="$swB3" -v b="$T_B3" 'BEGIN{exit !(a>=b && a-b<2)}' \
  && res 4b.say_kick_rate_limit PASS "say $(ms "$T_PB1" "$T_B2") ms after the last probe: no probe; say $(ms "$T_PB1" "$T_B3") ms after it: switch logged $(ms "$T_B3" "$swB3") ms after Return (first failed probe at chat1 +$(ms "$T_B1" "$T_PB1") ms)" \
  || res 4b.say_kick_rate_limit FAIL "probe/switch at say#2=${swB:-none}, switch=${swB3:-none} (say#3 at $T_B3)"
s3=$(ts "$R/proxy.log" '> .*"say".*darling' "$T_B3"); e3=$(ts "$ESL" 'START.*darling' "$T_B3"); res 4b2.kicking_line_itself INFO "the line that triggered the switch: daemon say ${s3:+$(ms "$T_B3" "$s3") ms}${s3:-none}; espeak ${e3:-none}"
sleep 2; chat "and again lillith"; T_B4=$T_CHAT
waitfor "$R/proxy.log" '> .*"say".*darling' 15; sb=$(ts "$R/proxy.log" '> .*"say".*darling' "$T_B4")
[ -n "$sb" ] && res 4c.say_after_kick_switch PASS "next reply on the daemon $(ms "$T_B4" "$sb") ms after Return" || res 4c.say_after_kick_switch FAIL "no daemon say after the switch"
stop_app; kill "$XPID" 2>/dev/null; rm -f "$ASOCK"

fi
######## LEG D: hung daemon at start-up (ping timeout 800 ms) + background re-probes
log "LEG D"; HS=$RUN/hang.sock; "$VPY" "$RP/fakevoiced.py" "$HS" hang "$R/fake-hang.log" & PIDS+=($!); FH=$!; sleep 0.5
mkapp d "$HS"; DLG=$R/app-d.log
if launch_app "$DLG"; then
  sleep 2; t0=$(ts "$DLG" 'Lillith overlay|lillith' "$T_LAUNCH"); tp=$(ts "$R/fake-hang.log" '> .*ping' "$T_LAUNCH"); tv=$(ts "$DLG" 'voice: espeak-ng|voice off' "$T_LAUNCH")
  res 6d.ping_timeout_startup $( [ -n "$tv" ] && awk -v a="$tp" -v b="$tv" 'BEGIN{d=(b-a)*1000; exit !(d>=700 && d<=2500)}' && echo PASS || echo FAIL) "hung daemon: ping sent -> fallback chosen in $(ms "$tp" "$tv") ms (spec 800 ms); window up $(ms "$T_LAUNCH" "$(now)") ms after launch"
  open_bubble_ui; sleep $(awk -v t="$T_LAUNCH" -v n="$(now)" 'BEGIN{s=12-(n-t); if (s<0) s=0; print s}')
  espw() { local d=$((SECONDS + ${2:-10})) t=""; while [ $SECONDS -lt $d ]; do t=$(ts "$ESL" 'START.*darling' "$1"); [ -n "$t" ] && { echo "$t"; return 0; }; sleep 0.05; done; return 1; }
  chat "hello lillith"; T_D1=$T_CHAT; te=$(espw "$T_D1" 10)
  sleep 1; tp2=$(ts "$R/fake-hang.log" '> .*ping' "$T_D1"); cid=$(awk -v t="$T_D1" '$1>t && /ping/{print $2; exit}' "$R/fake-hang.log"); te2=$(awk -v c="$cid" '$2==c && /EOF/{print $1; exit}' "$R/fake-hang.log")
  log "re-probe ping $cid: sent $tp2, app closed it $te2 ($(ms "$tp2" "$te2") ms); espeak START $te"
  sleep 2; chat "and now lillith"; T_D2=$T_CHAT; te_2=$(espw "$T_D2" 10); np2=$(awk -v t="$T_D2" '$1>t && /> .*ping/' "$R/fake-hang.log" | wc -l)
  s=$(awk -v p="$tp2" -v n="$(now)" 'BEGIN{s=10.5-(n-p); if (s<0) s=0; print s}'); sleep "$s"
  chat "once more lillith"; T_D3=$T_CHAT; te_3=$(espw "$T_D3" 10); tp3=$(ts "$R/fake-hang.log" '> .*ping' "$T_D3")
  d1=$(ms "$T_D1" "$te"); d2=$(ms "$T_D2" "$te_2"); d3=$(ms "$T_D3" "$te_3")
  [ -n "$te" ] && res 6e.reprobe_hung_nonblocking PASS "fallback spoke every line while the daemon hung (3/3)" || res 6e.reprobe_hung_nonblocking FAIL "fallback did not speak while the re-probe hung"
  awk -v a="$d1" -v b="$d2" -v c="$d3" -v n="$np2" 'BEGIN{exit !(a>=500 && a<=1300 && b<400 && c>=500 && c<=1300 && n==0)}' \
    && res e.say_ping_delay PASS "line 1 (ping on the speaking thread $(ms "$T_D1" "$tp2") ms after Return, timed out after $(ms "$tp2" "$te2") ms): espeak START $d1 ms after Return; line 2 ($(ms "$tp2" "$T_D2") ms after that probe, no ping): $d2 ms; line 3 ($(ms "$tp2" "$T_D3") ms after it, ping again +$(ms "$T_D3" "$tp3") ms): $d3 ms" \
    || res e.say_ping_delay FAIL "delays: line1 $d1 ms, line2 $d2 ms (pings in window $np2), line3 $d3 ms"
  stop_app
else res 6d.ping_timeout_startup FAIL "app did not start against a hung daemon: $(tail -3 "$DLG" | tr '\n' ' ')"; fi
kill $FH 2>/dev/null

######## LEG E: say to a daemon that answers ping but hangs on say (point 7)
log "LEG E"; ES=$RUN/hangsay.sock; FL=$R/fake-hangsay.log; "$VPY" "$RP/fakevoiced.py" "$ES" hangsay "$FL" & PIDS+=($!); FE=$!; sleep 0.5
mkapp e "$ES"; EL=$R/app-e.log
if launch_app "$EL" && open_bubble_ui; then
  sleep 4
  chat "hello lillith"; T_E1=$T_CHAT
  waitfor "$FL" 'HOLDING' 15; tsay=$(ts "$FL" '"say"' "$T_E1"); txt=$(grep -m1 '"say"' "$FL" | grep -oE '"text": ?"[^"]*"' | sed 's/^"text": *"//; s/"$//')
  nch=$(printf '%s' "$txt" | "$VPY" -c 'import sys; print(len(sys.stdin.read()))'); exp=$(awk -v n="$nch" 'BEGIN{e=30+0.25*n; if(e>600)e=600; printf "%.2f", e}')
  log "say held: $nch chars, expected deadline $exp s"
  waitfor "$EL" 'say did not finish in time' $(awk -v e="$exp" 'BEGIN{printf "%d", e+15}')
  tw=$(ts "$EL" 'say did not finish in time' "$T_E1"); tst=$(ts "$FL" '"stop"' "$tsay")
  alive=$(kill -0 $APP 2>/dev/null && echo y || echo n)
  dd=$(awk -v a="$tsay" -v b="$tw" -v e="$exp" 'BEGIN{ if (b=="") print "none"; else printf "%.2f", b-a }')
  [ -n "$tw" ] && [ -n "$tst" ] && awk -v d="$dd" -v e="$exp" 'BEGIN{exit !(d>=e-0.5 && d<=e+1.5)}' && [ "$alive" = y ] \
    && res 7a.hung_say_deadline PASS "$nch-char say: dropped after ${dd} s (spec 30 s + 250 ms x $nch = ${exp} s); stop sent $(ms "$tw" "$tst") ms after the warn; app alive" \
    || res 7a.hung_say_deadline FAIL "chars=$nch expected=${exp}s dropped_after=${dd}s stop=${tst:-none} alive=$alive"
  er=$(ts "$ESL" 'START.*darling' "$tw"); fw=$(sed 's/\x1b\[[0-9;]*m//g' "$EL" | grep -E 'WARN lillith_voice::speech: voice: ' | tail -1 | sed 's/^.*WARN/WARN/' | cut -c1-200)
  [ -n "$er" ] && res 7c.hung_line_replayed PASS "after the deadline the line was re-spoken on espeak $(ms "$tw" "$er") ms after the warn; $fw" || res 7c.hung_line_replayed FAIL "no espeak replay after the deadline; $fw"
  sleep 1; chat "hello again lillith" && ui=y || ui=n; T_E2=$T_CHAT
  d=$((SECONDS + 10)); while [ $SECONDS -lt $d ] && [ -z "$(ts "$FL" HOLDING "$T_E2")" ]; do sleep 0.2; done; sleep 4; T_E3P=$(now); chat "interrupt you lillith"; T_E3=$T_CHAT
  sleep 3; ts3=$(ts "$FL" '"stop"' "$T_E3"); s3=$(ts "$FL" '"say"' "$T_E3")
  [ "$ui" = y ] && [ -n "$ts3" ] && [ -n "$s3" ] && res 7b.hung_say_interruptible PASS "UI kept working during a hung say; a new reply stopped the hung line ($(ms "$T_E3" "$ts3") ms) and its say reached the daemon ($(ms "$T_E3" "$s3") ms)" \
    || res 7b.hung_say_interruptible FAIL "ui=$ui stop=${ts3:-none} next say=${s3:-none}"
  stop_app
else res 7a.hung_say_deadline FAIL "app/bubble did not start: $(tail -3 "$EL" | tr '\n' ' ')"; fi
kill $FE 2>/dev/null
for f in "$R"/app-*.log; do grep -q panicked "$f" && res "no_panic.$(basename "$f" .log)" FAIL "$(grep -m1 panicked "$f" | cut -c1-160)"; done
{ echo; echo "BIN $BIN"; echo "verdict: $(grep -q ' FAIL ' "$SUM" && echo FAIL || echo PASS) ($(grep -c ' PASS ' "$SUM") pass, $(grep -c ' FAIL ' "$SUM") fail)"; } >> "$SUM"
cat "$SUM"
