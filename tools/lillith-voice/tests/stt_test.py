"""STT / VAD / push-to-talk / barge-in / safeword against a running daemon that uses fake_mic.py.
argv: OUTDIR (contains rendered WAVs: q.wav, red.wav, long text not needed)"""
import difflib
import json
import os
import re
import shutil
import socket
import sys
import threading
import time
import wave

OUT = sys.argv[1]
SOCK = os.environ["LILLITH_VOICE_SOCKET"]
errs = []


def conn():
    s = socket.socket(socket.AF_UNIX)
    s.connect(SOCK)
    return s.makefile("rwb")


def call(f, **req):
    f.write((json.dumps(req) + "\n").encode())
    f.flush()
    return json.loads(f.readline())


def sim(a, b):
    n = lambda t: re.sub(r"[^a-z ]", "", t.lower()).split()
    return difflib.SequenceMatcher(None, n(a), n(b)).ratio()


def wav_s(p):
    with wave.open(p) as w:
        return w.getnframes() / w.getframerate()


def speak_into_mic(p):
    shutil.copy(p, os.path.join(OUT, "mic_play.wav.tmp"))
    os.replace(os.path.join(OUT, "mic_play.wav.tmp"), os.path.join(OUT, "mic_play.wav"))


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name + (f"  {info}" if info else ""))
    if not cond:
        errs.append(name)


Q = "Hey Lillith, what should we do tonight?"
c = conn()
# 1. STT round trip on a WAV rendered by our own TTS
r = call(c, cmd="stt", wav=os.path.join(OUT, "q.wav"))
check("stt round-trip (own TTS wav)", r["ok"] and sim(r["text"], Q) >= 0.6, f"heard={r['text']!r} sim={sim(r['text'], Q):.2f} stt_s={r.get('stt_s')}")
r = call(c, cmd="stt", wav=os.path.join(OUT, "red.wav"))
check("safeword 'red' by voice", r.get("safeword") is True, f"heard={r['text']!r}")
r = call(c, cmd="stt", wav=os.path.join(OUT, "yes.wav"))
check("voice 'yes' is never consent", r.get("affirmation") is True and r.get("consent_ok") is False, f"heard={r['text']!r}")

# 2. VAD: one utterance ends on its own after speech; silence -> timeout, empty
t0 = time.time()
speak_into_mic(os.path.join(OUT, "q.wav"))
r = call(c, cmd="listen", timeout=20)
el = time.time() - t0
check("VAD end-of-speech (listen once)", sim(r.get("text", ""), Q) >= 0.6 and el < wav_s(os.path.join(OUT, "q.wav")) + 4,
      f"heard={r.get('text')!r} after {el:.1f}s (speech {wav_s(os.path.join(OUT, 'q.wav')):.1f}s)")
r = call(c, cmd="listen", timeout=2.5)
check("VAD ignores silence", r.get("text") == "" and r.get("timeout"), json.dumps(r)[:120])

# 3. push-to-talk
r1 = call(c, op="listen", action="start")
speak_into_mic(os.path.join(OUT, "q.wav"))
time.sleep(wav_s(os.path.join(OUT, "q.wav")) + 0.6)
r = call(c, op="listen", action="stop")
check("push-to-talk start/stop", r1.get("listening") and sim(r.get("text", ""), Q) >= 0.6, f"heard={r.get('text')!r}")
r2 = call(c, op="listen", action="start")
call(c, op="listen", action="stop")
check("push-to-talk: every press has its own card", r2.get("listening") and r1.get("card") and r2.get("card")
      and r1["card"] != r2["card"], f"{r1.get('card')} / {r2.get('card')}")

# 4. barge-in: she talks (real-time fake player), user speaks -> she stops, his words are transcribed
ev = conn()
ev.write(b'{"op":"subscribe","topics":["all"]}\n')
ev.flush()
ev.readline()
evs = []
threading.Thread(target=lambda: [evs.append(json.loads(l)) for l in ev], daemon=True).start()
call(c, cmd="listen_mode", on=True)
rep = {}
th = threading.Thread(target=lambda: rep.update(call(conn(), cmd="say", wait=True, text=(
    "Okay so like, I'm going to tell you a really long story now. It starts at the mall, obviously. "
    "And then it keeps going, and going, and going, for like, ever."))))
th.start()
for _ in range(1200):
    if any(e.get("ev") == "chunk_start" for e in evs):
        break
    time.sleep(0.05)
time.sleep(0.8)
tb = time.time()
speak_into_mic(os.path.join(OUT, "q.wav"))
th.join(30)
lat = time.time() - tb
for _ in range(200):
    if any(e.get("ev") == "transcript" for e in evs):
        break
    time.sleep(0.1)
kinds = [e.get("ev") for e in evs]
tr = [e for e in evs if e.get("ev") == "transcript"]
check("barge-in stops playback", rep.get("stopped") and "barge_in" in kinds and
      any(e.get("ev") == "stop" and e.get("reason") == "barge_in" for e in evs), f"say returned after {lat:.2f}s; events={kinds[-6:]}")
check("barge-in speech is transcribed", tr and sim(tr[-1]["text"], Q) >= 0.4, f"{[t['text'] for t in tr]}")
# echo safety: while she plays and nobody talks, nothing of hers is transcribed
evs.clear()
call(c, cmd="say", wait=True, text="Mmm, I'm just talking to myself now. Nobody say anything, okay?")
time.sleep(1.0)
check("echo safety (mic muted while she talks)", not any(e.get("ev") == "transcript" for e in evs),
      f"{[e.get('text') for e in evs if e.get('ev') == 'transcript']}")
call(c, cmd="listen_mode", on=False)
print("STT", "OK" if not errs else "FAIL " + ", ".join(errs))
sys.exit(1 if errs else 0)
