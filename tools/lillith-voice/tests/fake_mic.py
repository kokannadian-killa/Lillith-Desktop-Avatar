"""Simulated microphone for tests: writes s16le 16 kHz mono to stdout in real time.
Silence (faint noise) until a WAV is dropped at <dir>/mic_play.wav; then that WAV is "spoken" once.
Run through a shim named `arecord` (tests/e2e.sh), since $LILLITH_MIC_CMD only accepts an allowlisted recorder:
  <OUT>/bin/arecord = `exec python fake_mic.py DIR`;  LILLITH_MIC_CMD=<OUT>/bin/arecord"""
import os
import sys
import time
import wave

import numpy as np

D = sys.argv[1]
RATE, WIN = 16000, 512
rng = np.random.default_rng(0)
out = sys.stdout.buffer
queue = np.zeros(0, np.float32)
t_next = time.monotonic()
while True:
    ctl = os.path.join(D, "mic_play.wav")
    if os.path.exists(ctl):
        with wave.open(ctl) as w:
            sr, n = w.getframerate(), w.getnframes()
            a = np.frombuffer(w.readframes(n), "<i2").astype(np.float32) / 32768
        a = np.interp(np.arange(0, len(a), sr / RATE), np.arange(len(a)), a).astype(np.float32)
        os.replace(ctl, ctl + ".played")
        queue = np.concatenate([queue, a])
    if len(queue):
        f, queue = queue[:WIN], queue[WIN:]
        f = np.pad(f, (0, WIN - len(f)))
    else:
        f = (rng.standard_normal(WIN) * 10 ** (-70 / 20)).astype(np.float32)
    try:
        out.write((np.clip(f, -1, 1) * 32767).astype("<i2").tobytes())
        out.flush()
    except BrokenPipeError:
        break
    t_next += WIN / RATE
    time.sleep(max(0.0, t_next - time.monotonic()))
