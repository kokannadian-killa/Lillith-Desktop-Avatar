"""Windows payload smoke test (run with the payload's own Python, on Windows or under wine):

    <voice>\\python\\python.exe <voice>\\tests\\win_smoke.py [OUTDIR]

Starts the bundled daemon exactly like the installer's autostart does (pythonw -m lillith_voice.winlaunch),
on the named pipe, with a stand-in player that writes each played chunk to OUTDIR\\played (no speakers needed),
then checks: ping (cloned voice loaded), netguard blocks non-loopback sockets, say with the cloned 'lillith'
voice and the 'p1' fallback (bundled ffmpeg + rubberband), file renders, STT round trip on a WAV, the 'red'
safeword, stop from a 2nd connection, a second start refusing the taken pipe, and --stop. Prints PASS/FAIL lines
and ALL PASSED. Uses a private pipe name so it never touches a daemon that is already running."""
import json
import os
import socket
import subprocess
import sys
import tempfile
import threading
import time
import wave

HOME = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PY = os.path.join(HOME, "python", "python.exe")
PYW = os.path.join(HOME, "python", "pythonw.exe")
OUT = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else tempfile.mkdtemp(prefix="lillith-winsmoke-"))
PLAYED = os.path.join(OUT, "played")
os.makedirs(PLAYED, exist_ok=True)
PIPE = r"\\.\pipe\lillith-voice-smoke-%d" % os.getpid()
os.environ["LILLITH_VOICE_PIPE"] = PIPE
sys.path.insert(0, os.path.join(HOME, "lib"))
from lillith_voice.transport import Conn  # noqa: E402

fails = []


def ok(cond, what, extra=""):
    print(("PASS  " if cond else "FAIL  ") + what + (f"  {extra}" if extra else ""), flush=True)
    if not cond:
        fails.append(what)


def call(timeout=2.0, **req):
    c = Conn(PIPE, timeout)
    try:
        return c.call(**req)
    finally:
        try:
            c._raw.close()
        except Exception:
            pass


def wav_info(path):
    import numpy as np
    with wave.open(path) as w:
        n, sr = w.getnframes(), w.getframerate()
        a = np.frombuffer(w.readframes(n), "<i2").astype("float32") / 32768
    rms = float(20 * np.log10(np.sqrt(np.mean(a ** 2)) + 1e-9)) if n else -120.0
    return n / sr, sr, rms


env = dict(os.environ)
fwd = lambda p: p.replace("\\", "/")  # LILLITH_PLAYER is shlex-split
env["LILLITH_PLAYER"] = f'"{fwd(PY)}" "{fwd(os.path.join(HOME, "tests", "fake_player.py"))}" {{}} "{fwd(PLAYED)}"'
env["LILLITH_VOICE_HOME"] = HOME
t0 = time.time()
exe = PYW if os.path.exists(PYW) and os.environ.get("SMOKE_CONSOLE") != "1" else PY
d = subprocess.Popen([exe, "-m", "lillith_voice.winlaunch", "--threads", "4"], env=env,
                     stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
ping = None
while time.time() - t0 < 240 and d.poll() is None:
    try:
        ping = call(timeout=1.0, cmd="ping")
        break
    except Exception:
        time.sleep(0.5)
ok(ping is not None and ping.get("ok"), "daemon up on the named pipe", f"{time.time() - t0:.1f}s  {json.dumps(ping)[:300]}")
if not ping:
    log = os.path.join(HOME, "logs", "voiced.log")
    if os.path.exists(log):
        print(open(log, encoding="utf-8", errors="replace").read()[-3000:])
    sys.exit(1)
ok(ping.get("cloned_voice") is True, "cloned 'lillith' voice available (no fallback)")
ok(ping.get("default") == "lillith", "default preset is 'lillith'", ping.get("default"))
ok(ping.get("protocol") == 2, "protocol v2")

# netguard: in this (client) process the kit's guard blocks everything but loopback
import lillith_voice.netguard  # noqa: E402,F401
try:
    socket.create_connection(("93.184.216.34", 80), timeout=2)
    ok(False, "netguard blocks non-loopback connect")
except lillith_voice.netguard.NetworkBlocked:
    ok(True, "netguard blocks non-loopback connect")
except OSError as e:
    ok(False, "netguard blocks non-loopback connect", f"got plain OSError {e} (not the guard)")

T = "Oh my god, hiii! Like, there you are? I've been sooo bored without you."
for preset in ("lillith", "p1"):
    before = set(os.listdir(PLAYED))
    t = time.time()
    r = call(timeout=5.0, cmd="say", text=T, wait=True, preset=preset)
    new = sorted(set(os.listdir(PLAYED)) - before)
    played = [wav_info(os.path.join(PLAYED, f)) for f in new]
    dur = sum(p[0] for p in played)
    ok(r.get("ok") and new and dur > 1.5 and all(p[2] > -45 for p in played),
       f"say preset={preset} played", f"{len(new)} chunks, {dur:.1f}s audio, {time.time() - t:.1f}s wall, reply {json.dumps(r)[:200]}")
    out = os.path.join(OUT, f"file_{preset}.wav")
    r = call(timeout=5.0, cmd="file", text=T, out=out, preset=preset)
    ok(r.get("ok") and os.path.exists(out), f"file preset={preset}",
       ("%.1fs %d Hz %.1f dBFS" % wav_info(out)) if os.path.exists(out) else json.dumps(r))

# STT round trip (Kokoro p1 renders a clear sentence; whisper tiny.en transcribes it)
q = os.path.join(OUT, "q.wav")
call(timeout=5.0, cmd="file", text="Hey Lillith, can you hear me?", out=q, preset="p1")
r = call(timeout=5.0, cmd="stt", wav=q)
txt = (r.get("text") or "").lower()
ok(r.get("ok", True) and "hear" in txt and "you" in txt, "stt round trip", repr(r.get("text")))
red = os.path.join(OUT, "red.wav")
call(timeout=5.0, cmd="file", text="Red.", out=red, preset="lillith")
r = call(timeout=5.0, cmd="stt", wav=red)
ok(bool(r.get("safeword")), "safeword 'red' recognised from the cloned voice", json.dumps(r)[:200])

# stop from a 2nd connection while a long say is waiting; queued lines are dropped
res = {}
long = " ".join(["This is a long line she would keep talking through for a while."] * 6)
th = threading.Thread(target=lambda: res.update(call(timeout=5.0, cmd="say", text=long, wait=True)))
before = set(os.listdir(PLAYED))
th.start()
t = time.time()
while time.time() - t < 60 and len(set(os.listdir(PLAYED)) - before) == 0:
    time.sleep(0.05)
time.sleep(0.5)
ts = time.time()
s = call(cmd="stop")
th.join(30)
lat = time.time() - ts
time.sleep(1.5)
n_after = len(set(os.listdir(PLAYED)) - before)
ok(s.get("ok") and not th.is_alive() and lat < 2.0, "stop on a 2nd connection ends the say",
   f"say replied {lat * 1000:.0f} ms after stop; {n_after} chunk(s) started; reply {json.dumps(res)[:160]}")

# a 2nd daemon must refuse the taken pipe and exit
r2 = subprocess.run([PY, "-m", "lillith_voice.daemon", "--no-warmup"], env=env, capture_output=True, text=True, timeout=120)
ok(r2.returncode == 0 and d.poll() is None, "second start refuses the owned pipe, first keeps running", r2.stderr.strip()[-160:])

r = subprocess.run([PY, "-m", "lillith_voice.winlaunch", "--stop"], env=env, capture_output=True, text=True, timeout=60)
try:
    d.wait(15)
except subprocess.TimeoutExpired:
    d.kill()
ok(r.returncode == 0 and d.poll() is not None, "winlaunch --stop shuts the daemon down")
log = os.path.join(HOME, "logs", "voiced.log")
ok(os.path.exists(log) and "ready on " + PIPE in open(log, encoding="utf-8", errors="replace").read(),
   "pythonw daemon logged to logs\\voiced.log")
print("ALL PASSED" if not fails else f"{len(fails)} FAILED: {fails}")
sys.exit(1 if fails else 0)
