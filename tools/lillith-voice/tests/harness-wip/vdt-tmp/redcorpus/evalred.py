"""usage: evalred.py KIT_DIR  -> runs every pos/neg wav (+ quiet/loud/noise/tight-crop variants) through Listener.transcribe"""
import glob, json, os, sys
import numpy as np
kit = sys.argv[1]; sys.path.insert(0, kit)
os.environ.setdefault("LILLITH_VOICE_MODELS", "/workspace/lillith/lillith-voice-kit/models")
from lillith_voice import stt
L = stt.Listener(speaker=None, threads=2)
rng = np.random.default_rng(7)
def variants(a):
    yield "base", a
    yield "quiet", a * 10 ** (-20 / 20)
    yield "loud", np.clip(a / (np.abs(a).max() + 1e-9) * 2.0, -1, 1)
    yield "noise", a + rng.normal(0, np.sqrt(np.mean(a ** 2)) / 10 ** (15 / 20), len(a)).astype(np.float32)
    nz = np.nonzero(np.abs(a) > 0.02)[0]
    if len(nz): yield "tight", a[max(0, nz[0] - 800): nz[-1] + 800]
    yield "mic", np.concatenate([np.zeros(16000, np.float32), a, np.zeros(19200, np.float32)])  # PTT-like air
bad = []; n = {"pos": 0, "neg": 0}
for kind in ("pos", "neg"):
    for f in sorted(glob.glob(f"{kind}/*.wav")):
        a = stt.read_wav16k(f)
        for v, x in variants(a):
            r = L.transcribe(np.asarray(x, np.float32))
            n[kind] += 1
            want = kind == "pos"
            ok = r["safeword"] == want and r["consent_ok"] is False
            if not ok:
                bad.append((f, v, r["text"], r["speech_s"], r["safeword"]))
            print(("ok  " if ok else "BAD ") + f"{f:32} {v:6} {r['text']!r:34} sw={r['safeword']} aff={r['affirmation']} consent={r['consent_ok']} stt={r['stt_s']}", flush=True)
print(json.dumps({"pos_total": n["pos"], "neg_total": n["neg"], "bad": len(bad),
                  "pos_missed": sum(1 for b in bad if b[0].startswith("pos")), "neg_false": sum(1 for b in bad if b[0].startswith("neg"))}))
