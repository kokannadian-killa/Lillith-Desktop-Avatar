#!/usr/bin/env python3
"""Compare the JVM render (temperature 0) with the desktop kit's Voice.render path at temperature 0."""
import os
import sys

kit, jf, text = sys.argv[1], sys.argv[2], sys.argv[3]
sys.path.insert(0, kit)
import numpy as np  # noqa: E402
from lillith_voice.pocket import PocketVoice  # noqa: E402
from lillith_voice.textnorm import normalize  # noqa: E402
from lillith_voice.dsp import crossfade_concat, polish  # noqa: E402

pd = os.path.join(kit, "models", "pocket")
pv = PocketVoice(pd, os.path.join(pd, "voices", "lillith.npy"), threads=4, temperature=0.0)
t = normalize(text)
blocks = [crossfade_concat(list(pv.stream(c, 0.0, 1)), 24000, 3.0) for c in pv.split(t)]
raw = crossfade_concat(blocks, 24000, 8.0)
py, g = polish(raw, 24000, -17.0, -1.5, 90.0, [(250, 1.0, -1.5), (3000, 0.9, 1.5)], (9000, -3.0), 6.0, None)
jv = np.fromfile(jf, "<f4")
n = min(len(py), len(jv))
d = np.abs(py[:n] - jv[:n])
c = float(np.corrcoef(py[:n], jv[:n])[0, 1]) if n > 1 else 0.0
print(f"smoke: python {len(py)} samples gain {g:.2f} dB | java {len(jv)} samples | max|diff| {d.max():.2e} "
      f"mean|diff| {d.mean():.2e} corr {c:.6f}")
ok = len(py) == len(jv) and c > 0.999
print("SMOKE " + ("PASS" if ok else "FAIL"))
sys.exit(0 if ok else 1)
