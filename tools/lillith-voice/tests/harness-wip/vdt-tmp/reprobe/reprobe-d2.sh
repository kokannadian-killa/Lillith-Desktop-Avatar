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

######## LEG D2: (d) first line after the daemon returns (after a death), >= 10 s after the last check, before the 30 s timer
log "LEG D2"; start_proxy; mkapp a "$ASOCK"; AL=$R/app-d2.log
launch_app "$AL" && open_bubble_ui || { res D2.app FAIL start; exit 1; }
sleep 3; chat "talk to me lillith"; T1=$T_CHAT
d=$((SECONDS + 20)); while [ $SECONDS -lt $d ] && [ -z "$(ts "$R/proxy.log" '< .*"ev": ?"chunk_start"' "$T1")" ]; do sleep 0.1; done; sleep 1.5
T_KILL=$(now); kill -9 "$DPID" "$XPID"; log "kill at $T_KILL"
d=$((SECONDS + 40)); TP=""; while [ $SECONDS -lt $d ] && [ -z "$TP" ]; do TP=$(ts "$AL" 'still not answering' "$T_KILL"); sleep 0.1; done
log "failed check after the kill at $TP (+$(ms "$T_KILL" "$TP") ms)"
rm -f "$DSOCK"; start_daemon "$DL" && start_proxy; T_BACK=$(now); log "daemon back at $T_BACK ($(ms "$TP" "$T_BACK") ms after that check)"
s=$(awk -v p="$TP" -v n="$(now)" 'BEGIN{s=10.5-(n-p); if (s<0) s=0; print s}'); sleep "$s"
chat "welcome back lillith"; T2=$T_CHAT
sy=""; d=$((SECONDS + 10)); while [ $SECONDS -lt $d ] && [ -z "$sy" ]; do sy=$(ts "$R/proxy.log" '> .*"say".*darling' "$T2"); sleep 0.05; done
sw=$(ts "$AL" 'switching to the cloned voice' "$T_KILL"); sleep 1; es=$(ts "$ESL" 'START.*darling' "$T2")
np=$(awk -v a="$T2" -v b="${sy:-9999999999}" '$1>a && $1<b && /> .*"ping"/' "$R/proxy.log" | wc -l)
[ -n "$sy" ] && [ -n "$sw" ] && awk -v a="$sw" -v b="$T2" 'BEGIN{exit !(a>=b)}' && [ -z "$es" ] \
  && res d.first_line_after_return_checks PASS "daemon died, came back; first line $(ms "$TP" "$T2") ms after the last check (and $(ms "$T_BACK" "$T2") ms after the return, before the 30 s timer): check ping +$(ms "$T2" "$(ts "$R/proxy.log" '> .*"ping"' "$T2")") ms, switch logged +$(ms "$T2" "$sw") ms, that same line said on the daemon +$(ms "$T2" "$sy") ms ($np ping before it), espeak not used" \
  || res d.first_line_after_return_checks FAIL "switch=${sw:-none} (Return $T2) say=${sy:-none} espeak=${es:-none} pings=$np"
stop_app
cat "$SUM"
