"""bench.py [threads ...] : real-time factor of Kokoro synth (+DSP) on this machine.
RTF = compute time / audio duration (lower is better; < 1 means faster than real time).
Env: LILLITH_VOICE_MODEL_FILE=kokoro-v1.0.int8.onnx to bench the int8 model."""
import os
import statistics
import sys
import time

from lillith_voice.core import Voice, get_preset, process, MODEL_FILE

LINES = ["Hiii babe.",
         "Oh my god, like, there you are? I've been sooo bored without you, it's literally so not fair.",
         "Okay, wait, you're like, totally staring at me right now. It's fine, I like it. "
         "Sooo, are you gonna play with me tonight, or what?"]
threads = [int(t) for t in sys.argv[1:]] or [os.cpu_count()]
print(f"model={MODEL_FILE} cpus={os.cpu_count()} affinity={sorted(os.sched_getaffinity(0))} "
      f"load={os.getloadavg()[0]:.1f}")
for th in threads:
    v = Voice(threads=th)
    p = get_preset("bella")
    v.synth("Hi.", p, 1.0)
    for line in LINES:
        walls, cpus, dsps = [], [], []
        for _ in range(3):
            c0, t0 = time.process_time(), time.time()
            a = v.synth(line, p, 1.0)
            t1, c1 = time.time(), time.process_time()
            process(a, p, p.pitch)
            dsps.append(time.time() - t1)
            walls.append(t1 - t0)
            cpus.append(c1 - c0)
        dur = len(a) / 24000
        print(f"threads={th} load={v.load_s:.1f}s  audio={dur:5.2f}s  synth_wall={statistics.median(walls):.2f}s "
              f"RTF={statistics.median(walls) / dur:.2f} (best {min(walls) / dur:.2f})  "
              f"cpu/audio={statistics.median(cpus) / dur:.2f}  dsp={statistics.median(dsps):.2f}s", flush=True)
