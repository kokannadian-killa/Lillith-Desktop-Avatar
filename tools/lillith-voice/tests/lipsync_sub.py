"""Scripted lip-sync subscriber: subscribe, speak a line on another connection, check the event stream."""
import json
import os
import socket
import sys
import threading
import time

SOCK = os.environ["LILLITH_VOICE_SOCKET"]


def conn():
    s = socket.socket(socket.AF_UNIX)
    s.connect(SOCK)
    return s, s.makefile("rwb")


def call(f, **req):
    f.write((json.dumps(req) + "\n").encode())
    f.flush()
    return json.loads(f.readline())


s, f = conn()
f.write(b'{"op":"subscribe","topics":["lipsync"]}\n')
f.flush()
assert json.loads(f.readline())["ok"]
evs = []


def pump():
    for line in f:
        evs.append((time.monotonic() * 1000, json.loads(line)))
threading.Thread(target=pump, daemon=True).start()

_, c = conn()
t0 = time.monotonic()
rep = call(c, op="say", text="Hiii babe. Mmm, I missed you sooo much. Hehe.", wait=True)
time.sleep(0.3)
utt = rep["utt"]
mine = [e for _, e in evs if e.get("utt") == utt]
kinds = [e["ev"] for e in mine]
print("say reply:", {k: rep[k] for k in ("ok", "utt", "chunks", "audio_s", "played_s") if k in rep})
print("events:", " ".join(f"{e['ev']}#{e.get('chunk', '')}" for e in mine))
env = [e for e in mine if e["ev"] == "envelope"]
starts = [(t, e) for t, e in evs if e.get("utt") == utt and e["ev"] == "chunk_start"]
errs = []
if not env:
    errs.append("no envelope")
for e in env:
    if e["fps"] != 30 or not e["pts"] or any(not (0 <= v <= 1) for _, v in e["pts"]):
        errs.append("bad envelope")
    if [p[0] for p in e["pts"][:3]] != [0, 33, 67]:
        errs.append(f"bad t_ms {e['pts'][:3]}")
for i, e in enumerate(env):  # envelope must come before its chunk_start
    if kinds.index("envelope") > kinds.index("chunk_start"):
        errs.append("envelope after chunk_start")
if len(starts) != len(env):
    errs.append(f"{len(starts)} chunk_start for {len(env)} chunks")
for t, e in starts:
    if abs(t - e["mono_ms"]) > 250:
        errs.append(f"chunk_start mono_ms off by {t - e['mono_ms']:.0f} ms")
if kinds[-1] != "utt_end" or kinds.count("chunk_end") != len(env):
    errs.append(f"bad tail {kinds[-3:]}")
peak = max(v for e in env for _, v in e["pts"])
print(f"chunks={len(env)} frames={sum(len(e['pts']) for e in env)} peak={peak:.2f} "
      f"first chunk_start-say={starts[0][1]['mono_ms'] - t0 * 1000:.0f}ms" if starts else "")

# interrupt on a second connection -> {"ev":"stop"}
evs.clear()
rep2 = {}
th = threading.Thread(target=lambda: rep2.update(call(conn()[1], cmd="say", wait=True,
                      text="This is a long line that should get cut off. It keeps going and going, like, forever.")))
th.start()
for _ in range(600):
    if any(e["ev"] == "chunk_start" for _, e in evs):
        break
    time.sleep(0.05)
st = call(conn()[1], cmd="stop")
th.join(10)
stops = [e for _, e in evs if e["ev"] == "stop"]
print("stop reply:", st, "waiting say reply:", {k: rep2.get(k) for k in ("ok", "utt", "stopped")}, "stop events:", stops)
if not (st["ok"] and rep2.get("stopped") and stops and stops[0]["utt"] == rep2.get("utt")):
    errs.append("stop from second connection did not interrupt + notify")
print("LIPSYNC", "OK" if not errs else "FAIL " + "; ".join(errs))
sys.exit(1 if errs else 0)
