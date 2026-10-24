"""check_wav.py FILE... : prints duration / rate / loudness / median F0, exits 1 if any looks broken."""
import subprocess
import re
import sys
import wave

import numpy as np

bad = 0
for p in sys.argv[1:]:
    with wave.open(p) as w:
        sr, n = w.getframerate(), w.getnframes()
        a = np.frombuffer(w.readframes(n), "<i2").astype(np.float32) / 32768
    dur = n / sr
    err = subprocess.run(["ffmpeg", "-hide_banner", "-nostats", "-i", p, "-af",
                          "ebur128=framelog=quiet:peak=true", "-f", "null", "-"],
                         capture_output=True, text=True).stderr
    lufs = float(re.findall(r"I:\s+(-?[\d.]+) LUFS", err)[-1])
    tp = float(re.findall(r"Peak:\s+(-?[\d.]+) dBFS", err)[-1])
    # median F0 (autocorrelation, voiced frames)
    fl, hop, f0 = int(0.04 * sr), int(0.01 * sr), []
    lo, hi = int(sr / 500), int(sr / 120)
    thr = 0.03 * np.abs(a).max()
    for i in range(0, len(a) - fl - hi, hop * 3):
        x = a[i:i + fl + hi]
        if np.sqrt(np.mean(x[:fl] ** 2)) < thr:
            continue
        ac = np.array([np.dot(x[:fl], x[t:t + fl]) for t in range(lo, hi)])
        if ac.max() > 0.5 * np.dot(x[:fl], x[:fl]):
            f0.append(sr / (lo + ac.argmax()))
    med = float(np.median(f0)) if f0 else 0
    ok = dur > 0.8 and sr in (22050, 24000, 44100, 48000) and -19 < lufs < -13 and tp <= -0.5 and 150 < med < 400
    bad += not ok
    print(f"{'PASS' if ok else 'FAIL'} {p}: {dur:.2f}s {sr}Hz {lufs:.1f} LUFS TP {tp:.1f} dBTP F0~{med:.0f}Hz")
sys.exit(1 if bad else 0)
