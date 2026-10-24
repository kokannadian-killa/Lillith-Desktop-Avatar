#!/usr/bin/env bash
# daemon-level safeword e2e against a kit source tree. usage: run_e2e.sh KIT_SRC OUTDIR
set -u
KSRC=$1; OUT=$2; VT=/workspace/lillith/scratch/vdt-tmp; C=$VT/redcorpus
LV=$VT/home/.local/share/lillith-voice; VPY=$LV/venv/bin/python
mkdir -p "$OUT"; RUN=$(mktemp -d /tmp/swe2e-XXXX); chmod 700 $RUN
PIDS=(); trap 'for p in "${PIDS[@]}"; do kill $p 2>/dev/null; done; sleep 1; for p in "${PIDS[@]}"; do kill -9 $p 2>/dev/null; done; rm -rf $RUN' EXIT
startd() { # name micjson
  local n=$1; mkdir -p $RUN/$n/{mic,played,cfg/lillith-voice,rt}; chmod 700 $RUN/$n/rt
  [ -n "$2" ] && echo "$2" > $RUN/$n/cfg/lillith-voice/mic.json
  env -i PATH=/usr/bin:/bin HOME=$VT/home XDG_CONFIG_HOME=$RUN/$n/cfg XDG_RUNTIME_DIR=$RUN/$n/rt PYTHONPATH=$KSRC HF_HUB_OFFLINE=1 \
    LILLITH_VOICE_SOCKET=$RUN/$n/s.sock LILLITH_PLAYER="$VPY $KSRC/tests/fake_player.py {} $RUN/$n/played/" \
    LILLITH_MIC_CMD="$VPY $KSRC/tests/fake_mic.py $RUN/$n/mic" $VPY -s -m lillith_voice.daemon --threads 2 > $OUT/daemon-$n.log 2>&1 &
  PIDS+=($!); for i in $(seq 400); do grep -q ready $OUT/daemon-$n.log && return 0; sleep 0.5; done; return 1; }
t() { local n=$1 sc=$2; shift 2; $VPY $VT/e2e/safeword_e2e.py $RUN/$n/s.sock $RUN/$n/mic $RUN/$n/played $sc "$@" 2>>$OUT/e2e.err | tee -a $OUT/results.jsonl; }
RED="$C/pos/bella_red.wav $C/pos/p1_red.wav $VT/assets/red.wav"
OOC="$C/pos/bella_ohohsee.wav $C/pos/lillith_ohohsee.wav"
OOCLIT="$C/pos/bella_ooc.wav $C/pos/p1_ooc.wav $C/pos/lillith_ooc.wav"
NEG="$C/neg/bella_4.wav $C/neg/bella_1.wav $C/neg/bella_5.wav $C/neg/lillith_2.wav"
: > $OUT/results.jsonl
startd def "" || { echo "daemon def not ready"; exit 1; }
t def stt $RED $OOC ${EXTRA_OOC:-}
t def neg $NEG
t def ptt $VT/assets/red.wav $C/pos/bella_ohohsee.wav
t def always $C/pos/bella_red.wav $C/pos/bella_ohohsee.wav
kill ${PIDS[0]}
startd nobarge '{"barge_in": false, "echo": "open"}' || { echo "daemon nobarge not ready"; exit 1; }
t nobarge always $C/pos/bella_red.wav $C/pos/p1_red.wav $C/pos/bella_ohohsee.wav $C/pos/lillith_ohohsee.wav
[ -n "${OOCLIT_RUN:-}" ] && t nobarge stt $OOCLIT
echo DONE
