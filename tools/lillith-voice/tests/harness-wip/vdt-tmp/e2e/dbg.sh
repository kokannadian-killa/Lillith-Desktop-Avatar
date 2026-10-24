#!/usr/bin/env bash
set -u
KSRC=/workspace/lillith/wt-voicee2e/tools/lillith-voice; OUT=$1; VT=/workspace/lillith/scratch/vdt-tmp; C=$VT/redcorpus
LV=$VT/home/.local/share/lillith-voice; VPY=$LV/venv/bin/python
mkdir -p $OUT; RUN=$(mktemp -d /tmp/swdbg-XXXX); n=nb; mkdir -p $RUN/$n/{mic,played,cfg/lillith-voice,rt}; chmod 700 $RUN/$n/rt
echo "${MICJSON:-{\"barge_in\": false, \"echo\": \"open\"}}" > $RUN/$n/cfg/lillith-voice/mic.json
env -i PATH=/usr/bin:/bin HOME=$VT/home XDG_CONFIG_HOME=$RUN/$n/cfg XDG_RUNTIME_DIR=$RUN/$n/rt PYTHONPATH=$KSRC HF_HUB_OFFLINE=1 \
    LILLITH_VOICE_SOCKET=$RUN/$n/s.sock LILLITH_PLAYER="$VPY $KSRC/tests/fake_player.py {} $RUN/$n/played/" \
    LILLITH_MIC_CMD="$VPY $KSRC/tests/fake_mic.py $RUN/$n/mic" $VPY -s -m lillith_voice.daemon --threads 2 > $OUT/daemon.log 2>&1 &
DP=$!; for i in $(seq 400); do grep -q ready $OUT/daemon.log && break; sleep 0.5; done
EVDUMP=$OUT/ev.jsonl $VPY $VT/e2e/safeword_e2e.py $RUN/$n/s.sock $RUN/$n/mic $RUN/$n/played "$2" "${@:3}" | tee -a $OUT/results.jsonl
kill $DP; sleep 1; kill -9 $DP 2>/dev/null; rm -rf $RUN
