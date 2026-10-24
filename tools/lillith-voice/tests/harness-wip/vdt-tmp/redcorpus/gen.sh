#!/bin/bash
# render positives/negatives with the kit's voices, in-process (no daemon), from the kit SOURCE tree
set -u
K=/workspace/lillith/lillith-voice-kit; PY=/workspace/lillith/scratch/vdt-tmp/home/.local/share/lillith-voice/venv/bin/python
export PYTHONPATH=$K LILLITH_VOICE_MODELS=$K/models HOME=/workspace/lillith/scratch/vdt-tmp/home HF_HUB_OFFLINE=1 PYTHONDONTWRITEBYTECODE=1
say(){ out=$1; shift; [ -s "$out" ] || nice -n 5 $PY -s -m lillith_voice.cli --no-daemon --threads 2 -o "$out" "$@" >/dev/null 2>>gen.err || echo "fail $out" >> gen.err; }
mkdir -p pos neg
for v in lillith p1 bella; do
  for t in "Red." "Red!" "red"; do n=$(echo "$t" | tr -dc 'a-zA-Z!' | tr '!' 'X'); say pos/${v}_$n.wav -p $v "$t"; done
  for sp in 0.8 1.25; do say pos/${v}_s$sp.wav -p $v -s $sp "Red."; done
  for pi in -3 3; do say pos/${v}_p$pi.wav -p $v --pitch=$pi "Red."; done
  say pos/${v}_ooc.wav -p $v "OOC, stop."
  i=0; for t in "That's right." "I read it." "Yes." "Stop reading." "Right, let's go to the red room." "Yes, I agree." "Read me a story." "All right then."; do i=$((i+1)); say neg/${v}_$i.wav -p $v "$t"; done
done
echo gen-done
