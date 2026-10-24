#!/usr/bin/env bash
# End-to-end test of an installed kit. Run:  bash tests/e2e.sh [outdir]
# Proves it offline by running inside a network namespace if `unshare -rn` works
# (only loopback exists there, so any network call would fail):  NETNS=1 bash tests/e2e.sh
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
if [ "${NETNS:-0}" = 1 ] && [ -z "${LV_IN_NETNS:-}" ]; then
  exec unshare -rn env LV_IN_NETNS=1 bash -c 'ip link set lo up; exec bash "$0" "$@"' "$0" "$@"
fi
OUT="${1:-$(mktemp -d -t lillith-e2e-XXXX)}"; mkdir -p "$OUT/played"
LV_HOME="${LILLITH_VOICE_HOME:-${XDG_DATA_HOME:-$HOME/.local/share}/lillith-voice}"
VPY="$LV_HOME/venv/bin/python"
export PATH="$HOME/.local/bin:$PATH"
# capture "playback" as files, taking real time like a speaker would (needed for barge-in / lip-sync timing)
export LILLITH_PLAYER="$VPY $HERE/fake_player.py {} $OUT/played/"
# simulated microphone: silence until a WAV is dropped at $OUT/mic_play.wav (real hardware can't be tested here)
# LILLITH_MIC_CMD only accepts an allowlisted recorder (absolute path), so the fake mic runs behind an `arecord` shim
mkdir -p "$OUT/bin"
printf '#!/bin/sh\nexec "%s" "%s" "%s"\n' "$VPY" "$HERE/fake_mic.py" "$OUT" > "$OUT/bin/arecord"; chmod 755 "$OUT/bin/arecord"
export LILLITH_MIC_CMD="$OUT/bin/arecord"
# the mic gate shows a Soul Contract card; here a fake card helper answers "yes" (no screen, no mouse in a test)
printf '#!/bin/sh\ncat >/dev/null\necho yes\n' > "$OUT/bin/fake-card"; chmod 755 "$OUT/bin/fake-card"
export LILLITH_CARD_CMD="$OUT/bin/fake-card"
export XDG_CONFIG_HOME="$OUT/config"
export LILLITH_VOICE_SOCKET="$OUT/voiced.sock"
PASS=0; FAIL=0
ok(){ echo "PASS  $*"; PASS=$((PASS+1)); }
no(){ echo "FAIL  $*"; FAIL=$((FAIL+1)); }
T="Oh my god, hiii! Like, there you are? I've been, like, sooo bored without you, it's literally so not fair."
PRESETS="${PRESETS:-lillith lillith_fast p1 bella bella85_heart15 bella80_nicole20 bella75_sky25}"
DPRESETS="${DPRESETS:-lillith p1 bella}"
[ -n "${LV_IN_NETNS:-}" ] && { ip -o link | grep -qv ' lo:' && no "netns has non-loopback links" || ok "running with NO network (only loopback)"; }

echo "== mic gate + launch guards (no models)"
"$VPY" "$HERE/micgate_test.py" >"$OUT/micgate.log" 2>&1 && ok "mic gate: $(grep -c '^PASS' "$OUT/micgate.log") checks (card/token gate, MIC_CMD allowlist, echo, audit)" \
  || no "mic gate: $(grep -E '^FAIL|Error' "$OUT/micgate.log" | head -3)"

echo "== push-to-talk races (real card + capture processes: release / safeword / timeout before Yes)"
"$VPY" "$HERE/ptt_race_test.py" >"$OUT/ptt_race.log" 2>&1 && ok "ptt races: $(grep -c '^PASS' "$OUT/ptt_race.log") checks (a late Yes never opens the mic)" \
  || no "ptt races: $(grep -E '^FAIL|Error' "$OUT/ptt_race.log" | head -3)"

echo "== transport (fake core, no models)"
"$VPY" "$HERE/transport_test.py" >"$OUT/transport.log" 2>&1 && ok "transport: $(grep -c '^PASS' "$OUT/transport.log") checks (framing, 24 clients, subscriber + say, stop on 2nd conn)" \
  || no "transport: $(grep -E '^FAIL|Error' "$OUT/transport.log" | head -3)"

echo "== one-shot (no daemon)"
for p in $PRESETS; do
  if lillith-say --no-daemon -v -p $p -o "$OUT/oneshot_$p.wav" "$T" 2>"$OUT/oneshot_$p.json"; then
    ok "one-shot $p $(tail -1 "$OUT/oneshot_$p.json" | cut -c1-120)"
  else no "one-shot $p: $(tail -3 "$OUT/oneshot_$p.json")"; fi
done

echo "== daemon"
lillith-voiced --threads "${LILLITH_THREADS:-4}" 2>"$OUT/daemon.log" & DPID=$!
for i in $(seq 1 120); do [ -S "$LILLITH_VOICE_SOCKET" ] && lillith-say --status >/dev/null 2>&1 && grep -q ready "$OUT/daemon.log" && break; sleep 0.5; done
grep -q ready "$OUT/daemon.log" && ok "daemon up: $(grep ready "$OUT/daemon.log")" || { no "daemon did not start"; cat "$OUT/daemon.log"; }
for p in $DPRESETS; do
  if lillith-say --daemon -v -p $p -o "$OUT/daemon_$p.wav" "$T" 2>"$OUT/daemon_$p.json"; then
    ok "daemon file $p $(tail -1 "$OUT/daemon_$p.json" | cut -c1-120)"
  else no "daemon file $p: $(tail -3 "$OUT/daemon_$p.json")"; fi
done
# playback path: 3 sentences -> 3 chunks played back to back, first audio before the whole thing is rendered
rm -f "$OUT/played/"*
if lillith-say --daemon -v "Hiii babe. Like, I missed you sooo much today. Come sit with me and tell me everything, okay?" 2>"$OUT/play.json"; then
  n=$(ls "$OUT/played" | wc -l); ok "daemon play: $n chunks played; $(tail -1 "$OUT/play.json" | cut -c1-160)"
  [ "$n" -ge 2 ] || no "expected >=2 played chunks"
else no "daemon play: $(cat "$OUT/play.json")"; fi
# streaming stdin (sentence-by-sentence)
rm -f "$OUT/played/"*
( for w in Okay so like, this is streaming. Every sentence starts as soon as it ends. Cute, right?; do printf '%s ' "$w"; sleep 0.05; done ) \
  | lillith-say --daemon --stream -v 2>"$OUT/stream.json" && ok "stream: $(ls "$OUT/played" | wc -l) chunks played" || no "stream"
# piper shim (what wt-tts's lillith-voice crate would call)
echo "Hey, it's Lillith through the piper shim." | lillith-piper --model x.onnx --output_raw > "$OUT/shim.raw" \
  && [ "$(stat -c %s "$OUT/shim.raw")" -gt 48000 ] && ok "piper shim raw s16le: $(stat -c %s "$OUT/shim.raw") bytes" || no "piper shim"
# stop
lillith-say --daemon --no-wait "This is a long line that should get cut off right away, like, immediately, okay?" && lillith-say --stop && ok "stop accepted" || no "stop"

echo "== protocol: op/cmd aliases, ping"
"$VPY" - <<'PY' && ok "ping via op and cmd keys; protocol v2; default preset lillith" || no "ping/op alias"
import json, os, socket
s = socket.socket(socket.AF_UNIX); s.connect(os.environ["LILLITH_VOICE_SOCKET"]); f = s.makefile("rwb")
for k in ("op", "cmd"):
    f.write((json.dumps({k: "ping"}) + "\n").encode()); f.flush(); r = json.loads(f.readline())
    assert r["ok"] and r["protocol"] == 2 and r["default"] == "lillith" and r["cloned_voice"], r
PY
echo "== lip-sync events (scripted subscriber) + stop from a second connection"
"$VPY" "$HERE/lipsync_sub.py" > "$OUT/lipsync.txt" 2>&1 && ok "$(grep -E '^chunks=' "$OUT/lipsync.txt")" || { no "lipsync"; cat "$OUT/lipsync.txt"; }
echo "== safeword / consent classifier (lone fuzzy 'red' stops her; sentences never do; voice is never consent)"
"$VPY" "$HERE/safeword_test.py" "$LV_HOME/lib" > "$OUT/safeword.txt" 2>&1 && ok "safeword table ($(grep -c ^PASS "$OUT/safeword.txt") cases)" || { no "safeword table"; grep ^FAIL "$OUT/safeword.txt"; }
echo "== text normalisation (times keep the next word's space, commas kept unless thousands separators)"
"$VPY" "$HERE/textnorm_test.py" "$LV_HOME/lib" > "$OUT/textnorm.txt" 2>&1 && ok "textnorm ($(grep -c ^PASS "$OUT/textnorm.txt") cases)" || { no "textnorm"; grep ^FAIL "$OUT/textnorm.txt"; }
echo "== speech-to-text: round trip, VAD, push-to-talk, barge-in, echo safety, safeword"
lillith-say --daemon -o "$OUT/q.wav" "Hey Lillith, what should we do tonight?" \
  && lillith-say --daemon -o "$OUT/red.wav" "Red." && lillith-say --daemon -o "$OUT/yes.wav" "Yes, I agree." \
  && "$VPY" "$HERE/stt_test.py" "$OUT" > "$OUT/stt.txt" 2>&1; R=$?
sed 's/^/      /' "$OUT/stt.txt"; [ $R = 0 ] && ok "stt suite ($(grep -c ^PASS "$OUT/stt.txt") checks)" || no "stt suite"

echo "== llama-server bridge (fake server on 127.0.0.1:18080)"
"$VPY" "$HERE/fake_llama_server.py" 18080 & FPID=$!; sleep 1
rm -f "$OUT/played/"*
if timeout 120 lillith-chat --url http://127.0.0.1:18080/v1/chat/completions "hey" > "$OUT/chat.txt" 2>&1; then
  ok "lillith-chat: '$(head -c 70 "$OUT/chat.txt")…' -> $(ls "$OUT/played" | wc -l) chunks spoken"
else no "lillith-chat: $(cat "$OUT/chat.txt")"; fi
# full voice loop: (simulated) mic -> VAD -> whisper -> fake llama-server -> cloned voice
rm -f "$OUT/played/"*
( timeout 240 lillith-chat --mic --turns 1 --url http://127.0.0.1:18080/v1/chat/completions > "$OUT/voiceloop.txt" 2>&1 ) & VL=$!
for i in $(seq 1 60); do grep -q "listening" "$OUT/voiceloop.txt" 2>/dev/null && break; sleep 0.5; done
sleep 1; cp "$OUT/q.wav" "$OUT/mic_play.wav"
wait $VL; R=$?
if [ $R = 0 ] && grep -q "you (voice)>" "$OUT/voiceloop.txt" && [ "$(ls "$OUT/played" | wc -l)" -ge 1 ]; then
  ok "voice loop: '$(grep -o 'you (voice)>.*' "$OUT/voiceloop.txt" | head -1)' -> reply spoken in $(ls "$OUT/played" | wc -l) chunks"
else no "voice loop (rc=$R): $(tail -5 "$OUT/voiceloop.txt")"; fi
kill $FPID 2>/dev/null
lillith-chat --url http://example.com/v1/chat/completions hi >/dev/null 2>&1 && no "chat accepted remote URL" || ok "lillith-chat refuses non-loopback URLs"

echo "== network guard"
PYTHONPATH="$LV_HOME/lib" "$VPY" - <<'PY' && ok "netguard blocks 1.1.1.1:443 and DNS, allows loopback" || no "netguard"
import socket, lillith_voice
from lillith_voice.netguard import NetworkBlocked
for f in (lambda: socket.create_connection(("1.1.1.1", 443), timeout=2),
          lambda: socket.getaddrinfo("huggingface.co", 443)):
    try: f(); raise SystemExit("not blocked")
    except NetworkBlocked: pass
socket.getaddrinfo("127.0.0.1", 8080)
PY

lillith-say --daemon --stop >/dev/null 2>&1; kill $DPID 2>/dev/null; wait $DPID 2>/dev/null

echo "== audio checks"
"$VPY" "$HERE/check_wav.py" "$OUT"/oneshot_*.wav "$OUT"/daemon_*.wav && ok "all WAVs sane" || no "WAV check"
echo
echo "RESULT: $PASS passed, $FAIL failed   (outputs in $OUT)"
[ "$FAIL" = 0 ]
