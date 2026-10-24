"""The talk button's owner goes away mid press, with real processes (no models):  python tests/close_test.py
In-tree version of the voice-QA repro (legacy-results/mouse-3cff41d/safety/close_test.py + close_run.sh): a
real daemon transport (UnixServer + Core) in front of a model-free Listener, a real card helper that answers
Yes late, a real capture process (tests/fake_mic.py behind an `arecord` shim); checks the process table.
  * the client exits / is killed / closes its connection while the card waits, Yes at 3 s -> no capture
    process ever; audited PROPOSED, CANCELLED; tokens revoked
  * the owning connection (hold:true) closes while the mic is open -> the capture process stops, audio dropped
  * a start whose reply can't be written -> the mic is closed again
  * a press with no owning connection is closed after LILLITH_PTT_MAX_S (orphan cap)
  * N1: {cmd:stop, reason:safeword} while the mic is open -> capture ends, nothing transcribed or broadcast
 * the hush race (B2): listen stop first, safeword stop while its transcription is in flight (and either
    order) -> no text in the reply, nothing broadcast; listen stop drop:true; listen once whose owner goes
    away -> its tail is dropped
 * the real Rust button: closing while listening drops the speech; Mic::hush x10 sends the safeword stop
    before the press's listen stop drop and nothing is broadcast
  * the real Rust talk button (crates/lillith-panel/examples/mic_driver) quit / drop+quit / SIGKILL while the
    card waits -> no capture process; Drop's listen stop reaches the daemon within 1 s. Set
    LILLITH_MIC_DRIVER to the built example (cargo build -p lillith-panel --example mic_driver); skipped
    (reported) when it is not built.
exit 0 = all passed
"""
import json
import os
import socket
import subprocess
import sys
import tempfile
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
TMP = tempfile.mkdtemp(prefix="lillith-close-")
os.environ["XDG_DATA_HOME"] = os.path.join(TMP, "data")
os.environ["XDG_CONFIG_HOME"] = os.path.join(TMP, "config")
os.environ["LILLITH_PTT_MAX_S"] = "2"
BIN = os.path.join(TMP, "bin")
os.makedirs(BIN)
with open(os.path.join(BIN, "arecord"), "w") as f:
    f.write(f'#!/bin/sh\necho x >> "{TMP}/mic_opens.log"\n'
            f'exec "{sys.executable}" "{os.path.join(HERE, "fake_mic.py")}" "{TMP}" close-mic-marker\n')
MODE = os.path.join(TMP, "card_mode")
with open(os.path.join(BIN, "card"), "w") as f:
    f.write(f"""#!/bin/sh
cat >/dev/null
m=$(cat "{MODE}")
[ "$m" = delay ] && exec "{sys.executable}" -c "import time; time.sleep(3); print('yes')" close-card-marker
echo "$m"
""")
for n in ("arecord", "card"):
    os.chmod(os.path.join(BIN, n), 0o755)
os.environ["LILLITH_MIC_CMD"] = os.path.join(BIN, "arecord")
os.environ["LILLITH_CARD_CMD"] = os.path.join(BIN, "card")
SOCK = os.path.join(TMP, "run", "voiced.sock")
os.environ["LILLITH_VOICE_SOCKET"] = SOCK

from lillith_voice import daemon as D  # noqa: E402
from lillith_voice import micgate as M  # noqa: E402
from lillith_voice import stt  # noqa: E402
from lillith_voice import transport as T  # noqa: E402

FAILS = []
NPASS = [0]
AUDIT = os.path.join(TMP, "data", "lillith", "audit.log")


def check(name, cond, detail=""):
    print(("PASS " if cond else "FAIL ") + name + (f"  ({detail})" if detail else ""), flush=True)
    if cond:
        NPASS[0] += 1
    else:
        FAILS.append(name)


def mic_procs():
    out = subprocess.run(["ps", "-eo", "pid=,args="], capture_output=True, text=True).stdout.splitlines()
    return [ln.strip() for ln in out if "close-mic-marker" in ln and "ps -eo" not in ln]


def card_procs():
    out = subprocess.run(["ps", "-eo", "pid=,args="], capture_output=True, text=True).stdout.splitlines()
    return [ln.strip() for ln in out if "close-card-marker" in ln and "ps -eo" not in ln]


def n_mics():
    try:
        return len(open(os.path.join(TMP, "mic_opens.log")).read().split())
    except FileNotFoundError:
        return 0


def card(mode):
    with open(MODE, "w") as f:
        f.write(mode)


def audit():
    try:
        with open(AUDIT, encoding="utf-8") as f:
            return [ln.split("\t") for ln in f.read().splitlines() if ln.strip()]
    except FileNotFoundError:
        return []


def tags_since(k):
    return [a[1] for a in audit()[k:]]


def wait(pred, to):
    end = time.monotonic() + to
    while time.monotonic() < end:
        if pred():
            return True
        time.sleep(0.05)
    return pred()


# ---- a daemon with the real transport and Core, and a Listener without models
EVENTS = []
TRANSCRIBED = []


class Speaker:
    player = type("P", (), {"playing": False})()
    voice = type("V", (), {"threads": 1, "load_s": 0.0})()    # for ping (the Rust button probes it)

    def stop(self, reason=None):
        pass

    def busy(self):
        return False


hub = T.Hub()
_emit = hub.emit
hub.emit = lambda ev: (EVENTS.append(ev), _emit(ev))
core = object.__new__(D.Core)
core.speaker, core.hub, core.started, core.stt_threads = Speaker(), hub, time.time(), 1
core._llock = threading.Lock()
core.gate = M.MicGate(mic_argv=stt.mic_argv)
L = object.__new__(stt.Listener)
L.speaker, L.emit, L.always, L._mode, L._ptt, L._once = core.speaker, hub.emit, False, None, [], None
L._lock = threading.Lock()
L._press_lock, L._press, L._press_seq = threading.Lock(), None, 0
L._drops, L._once_dropped, L._stt_inflight = 0, False, 0
L.device = None
L.mic = stt.Mic(L._on_audio, None)
L.gate = core.gate


class OnceVad:
    """listen once: nothing ends on its own; whatever was heard is the tail."""
    speaking = False

    def reset(self):
        pass

    def feed(self, a):
        return []

    def flush(self):
        return [stt.np.full(8000, 0.1, stt.np.float32)]


L.vad = OnceVad()


class FakeVad:
    """The real Listener.transcribe runs; only the models are fakes (all of it is speech)."""

    def feed(self, a):
        return [a] if len(a) else []

    def flush(self):
        return []


class FakeRec:
    delay = 0.0          # seconds a transcription takes (the in-flight window)
    busy = threading.Event()

    def transcribe(self, speech, voiced_s):
        FakeRec.busy.set()
        time.sleep(FakeRec.delay)
        TRANSCRIBED.append(len(speech))
        return "something said"


stt.Vad = FakeVad
L.rec = FakeRec()
core._listener = L
server = T.UnixServer(SOCK, core)
threading.Thread(target=server.serve_forever, daemon=True).start()


class Client:
    """One connection; close() really closes it (the socket and its file)."""

    def __init__(self):
        self.s = socket.socket(socket.AF_UNIX)
        self.s.connect(SOCK)
        self.f = self.s.makefile("rwb")

    def write(self, b):
        self.f.write(b)
        self.f.flush()

    def flush(self):
        pass

    def readline(self):
        return self.f.readline()

    def close(self):
        self.f.close()
        self.s.close()


def connect():
    c = Client()
    return c, c


def call(**req):
    s, f = connect()
    f.write((json.dumps(req) + "\n").encode())
    f.flush()
    r = json.loads(f.readline() or b"{}")
    f.close()
    return r


def settle(to=5):
    wait(lambda: not mic_procs() and not card_procs() and L._press is None, to)


check("the capture command is the fake recorder", stt.mic_argv()[0] == os.path.join(BIN, "arecord"))
check("the orphan cap is set for the test", stt.PTT_ORPHAN_MAX_S == 2.0, str(stt.PTT_ORPHAN_MAX_S))

# ---- 1. raw clients go away while the card waits (Yes lands at 3 s)
CLIENT = r"""
import json, socket, sys, time
s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1])
s.sendall((json.dumps({"cmd": "listen", "action": "start", "hold": sys.argv[2] == "hold"}) + "\n").encode())
print("sent", flush=True)
time.sleep(float(sys.argv[3]))
"""
for how in ("hold+exit", "nohold+exit", "hold+kill", "nohold+kill"):
    card("delay")
    m0, a0 = n_mics(), len(audit())
    p = subprocess.Popen([sys.executable, "-c", CLIENT, SOCK, how.split("+")[0], "0.7" if "exit" in how else "60"],
                         stdout=subprocess.PIPE, text=True)
    p.stdout.readline()
    wait(lambda: card_procs(), 5)
    if "kill" in how:
        time.sleep(0.7)
        p.kill()
    p.wait(10)
    gone = time.monotonic()
    closed = wait(lambda: not card_procs(), 2)
    took = time.monotonic() - gone
    time.sleep(3.5)                      # past the late Yes
    check(f"close.{how}: client gone while the card waits, Yes at 3 s -> no capture process ever",
          n_mics() == m0 and not mic_procs(), f"opened={n_mics() - m0} procs={mic_procs()} {tags_since(a0)}")
    check(f"close.{how}: the card closed when the client went away", closed and took < 1.5, f"{took:.2f}s")
    t = [x for x in tags_since(a0) if x != "LAUNCH"]   # LAUNCH: the card helper itself
    check(f"close.{how}: audited PROPOSED then CANCELLED, never CONFIRM", t[:2] == ["PROPOSED", "CANCELLED"]
          and "CONFIRM" not in t, str(t))
    settle()

# a token minted before is revoked when the owner goes
card("yes")
tok = call(cmd="mic_card", kind="listen").get("token")
card("delay")
a0 = len(audit())
s, f = connect()
f.write(b'{"cmd":"listen","action":"start","hold":true}\n')
f.flush()
wait(lambda: card_procs(), 5)
s.close()
time.sleep(1.0)
r = call(cmd="listen", action="start", token=tok)
check("close.token: an unused token does not survive the owner going away",
      r.get("ok") is False and r.get("need_card") is True and not mic_procs(), str(r))
settle()
time.sleep(3)

# ---- 2. the owning connection closes while the mic is open -> the capture stops
card("yes")
a0 = len(audit())
s, f = connect()
f.write(b'{"cmd":"listen","action":"start","hold":true}\n')
f.flush()
r = json.loads(f.readline())
opened = wait(lambda: mic_procs(), 5)
check("listen.hold: Yes opens the mic", r.get("ok") is True and opened, str(r))
time.sleep(0.5)
n_tr = len(TRANSCRIBED)
s.close()
check("listen.hold: the owner's connection closes -> the capture process stops", wait(lambda: not mic_procs(), 2),
      str(mic_procs()))
time.sleep(0.3)
check("listen.hold: its audio is dropped (nothing transcribed)", len(TRANSCRIBED) == n_tr)
check("listen.hold: audited CANCELLED", "CANCELLED" in tags_since(a0), str(tags_since(a0)))
r = call(cmd="listen", action="stop")
check("listen.hold: a later stop has nothing left", r.get("cancelled") is True, str(r))
settle()

# a held press survives until its owner releases it normally (stop, then close)
s, f = connect()
f.write(b'{"cmd":"listen","action":"start","hold":true}\n')
f.flush()
json.loads(f.readline())
wait(lambda: mic_procs(), 5)
time.sleep(0.5)
check("listen.hold: the mic stays open while its owner is connected", bool(mic_procs()))
r = call(cmd="listen", action="stop")
check("listen.hold: a normal stop transcribes", r.get("text") == "something said" and not mic_procs(), str(r))
s.close()
settle()

# ---- 3. the start reply can't be written -> the mic is closed again
card("yes")
a0 = len(audit())
s, f = connect()
f.write(b'{"cmd":"listen","action":"start"}\n')
s.close()                                  # gone before the (immediate) Yes and the reply
time.sleep(1.0)
check("reply.unwritten: no capture process left for a client that never got its reply", not mic_procs(),
      f"{mic_procs()} {tags_since(a0)}")
settle()
time.sleep(2.5)

# ---- 4. orphan cap: a non-hold press (one request per connection) is closed after LILLITH_PTT_MAX_S
card("yes")
a0 = len(audit())
r = call(cmd="listen", action="start")
check("orphan: a one-shot start opens the mic", r.get("ok") is True and wait(lambda: mic_procs(), 5), str(r))
t0 = time.monotonic()
stopped = wait(lambda: not mic_procs(), 6)
check("orphan: a mic with no owner is closed by the cap", stopped, f"{time.monotonic() - t0:.1f}s")
check("orphan: audited CANCELLED", wait(lambda: "CANCELLED" in tags_since(a0), 2), str(tags_since(a0)))
settle()

# ---- 5. N1: a safeword stop while the mic is open drops the audio daemon-side
card("yes")
a0 = len(audit())
EVENTS.clear()
s, f = connect()
f.write(b'{"cmd":"listen","action":"start","hold":true}\n')
f.flush()
json.loads(f.readline())
wait(lambda: mic_procs(), 5)
time.sleep(0.5)
n_tr = len(TRANSCRIBED)
check("safeword: {cmd:stop, reason:safeword} replies ok", call(cmd="stop", reason="safeword").get("ok") is True)
check("safeword: the capture process stops", wait(lambda: not mic_procs(), 2), str(mic_procs()))
r = call(cmd="listen", action="stop")
check("safeword: the following listen stop is cancelled with no text", r.get("cancelled") is True
      and not r.get("text"), str(r))
check("safeword: nothing was transcribed", len(TRANSCRIBED) == n_tr)
check("safeword: no transcript was broadcast", not [e for e in EVENTS if e.get("ev") == "transcript"],
      str(EVENTS))
check("safeword: audited CANCELLED + REVOKED (no speech text)",
      {"CANCELLED", "REVOKED"} <= set(tags_since(a0)), str(tags_since(a0)))
s.close()
settle()

# in-flight transcription when the safeword lands: the result is dropped too
L._ptt = []
gen = L._drops
L.cancel_pending("safeword", drop_capture=False)
check("safeword: a spoken-safeword transcript does not drop anything (gen unchanged)", L._drops == gen)
L._mode = "ptt"
L.cancel_pending("safeword", drop_capture=True)
check("safeword: a safeword stop bumps the drop counter", L._drops == gen + 1 and L._mode is None)

# ---- 5b. B2 / the hush race: the press's listen stop first, the safeword stop a moment later (while
# that stop's transcription is in flight), and the other way round: nothing is returned or broadcast
FakeRec.delay = 0.6
for i, lag in enumerate((0.03, 0.0, 0.01, 0.03, 0.1, 0.03, 0.0, 0.2, 0.03, 0.05)):
    card("yes")
    a0, n_tr = len(audit()), len(TRANSCRIBED)
    EVENTS.clear()
    FakeRec.busy.clear()
    s, f = connect()
    f.write(b'{"cmd":"listen","action":"start","hold":true}\n')
    json.loads(f.readline())
    wait(lambda: mic_procs(), 5)
    time.sleep(0.3)
    got = {}
    th = threading.Thread(target=lambda: got.update(call(cmd="listen", action="stop")))
    th.start()
    if i % 2 == 0:
        FakeRec.busy.wait(3)               # the stop's transcription is running now
    time.sleep(lag)
    call(cmd="stop", reason="safeword")
    th.join(10)
    trs = [e for e in EVENTS if e.get("ev") == "transcript"]
    check(f"race{i} ({'in flight' if i % 2 == 0 else 'any order'}, +{lag * 1000:.0f} ms): listen stop then "
          "safeword -> no text in the reply, nothing broadcast",
          not got.get("text") and got.get("cancelled") is True and not trs, f"reply={got} broadcast={trs}")
    check(f"race{i}: audited CANCELLED", wait(lambda: "CANCELLED" in tags_since(a0), 2), str(tags_since(a0)))
    s.close()
    settle()
FakeRec.delay = 0.0

# control: a plain listen stop still transcribes and broadcasts
card("yes")
EVENTS.clear()
s, f = connect()
f.write(b'{"cmd":"listen","action":"start","hold":true}\n')
json.loads(f.readline())
wait(lambda: mic_procs(), 5)
time.sleep(0.3)
r = call(cmd="listen", action="stop")
check("control: a listen stop without a safeword returns and broadcasts the text",
      r.get("text") == "something said" and [e for e in EVENTS if e.get("ev") == "transcript"], str(r))
s.close()
settle()

# ---- 5c. listen stop with drop (the bubble closing): the audio is dropped, the mic stops
card("yes")
a0, n_tr = len(audit()), len(TRANSCRIBED)
EVENTS.clear()
s, f = connect()
f.write(b'{"cmd":"listen","action":"start","hold":true}\n')
json.loads(f.readline())
wait(lambda: mic_procs(), 5)
time.sleep(0.3)
r = call(cmd="listen", action="stop", drop=True)
check("drop: listen stop drop:true replies cancelled + dropped, no text",
      r.get("cancelled") is True and r.get("dropped") is True and not r.get("text"), str(r))
check("drop: the capture process stops", wait(lambda: not mic_procs(), 2), str(mic_procs()))
check("drop: nothing transcribed or broadcast", len(TRANSCRIBED) == n_tr
      and not [e for e in EVENTS if e.get("ev") == "transcript"], str(EVENTS))
check("drop: audited CANCELLED", wait(lambda: "CANCELLED" in tags_since(a0), 2), str(tags_since(a0)))
s.close()
settle()

# ---- 5d. listen once whose owner goes away: the tail is dropped, not transcribed or broadcast
card("yes")
a0, n_tr = len(audit()), len(TRANSCRIBED)
EVENTS.clear()
s, f = connect()
f.write(b'{"cmd":"listen","timeout":20}\n')
check("once: the mic opens (Yes)", wait(lambda: mic_procs(), 5))
time.sleep(0.3)
s.close()
check("once: the owner goes away -> the capture process stops", wait(lambda: not mic_procs(), 2), str(mic_procs()))
time.sleep(0.5)
check("once: its tail is not transcribed or broadcast", len(TRANSCRIBED) == n_tr
      and not [e for e in EVENTS if e.get("ev") == "transcript"], str(EVENTS))
check("once: audited CANCELLED", wait(lambda: "CANCELLED" in tags_since(a0), 2), str(tags_since(a0)))
settle()
# control: listen once with its owner still there returns the tail
card("yes")
r = call(cmd="listen", timeout=1)
check("once control: an owner that stays gets the tail's text", r.get("text") == "something said", str(r))
settle()

# ---- 6. the real Rust talk button (examples/mic_driver)
DRV = os.environ.get("LILLITH_MIC_DRIVER")
if not DRV or not os.access(DRV, os.X_OK):
    print("SKIP the Rust talk button (set LILLITH_MIC_DRIVER to the built examples/mic_driver)", flush=True)
else:
    class Drv:
        def __init__(self):
            self.p = subprocess.Popen([DRV], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
            self.out = []
            threading.Thread(target=self._rd, daemon=True).start()
            DRIVERS.append(self.p)
            if not wait(lambda: any("ready=true" in ln for _, ln in self.out), 15):
                self.p.kill()
                raise SystemExit(f"driver never got Ready: {[ln for _, ln in self.out]}")

        def _rd(self):
            for ln in self.p.stdout:
                self.out.append((time.monotonic(), ln.rstrip()))

        def cmd(self, c):
            self.p.stdin.write(c + "\n")
            self.p.stdin.flush()

    DRIVERS = []
    import atexit
    atexit.register(lambda: [p.kill() for p in DRIVERS if p.poll() is None])
    STOPS = []
    _d = core.dispatch

    SEEN = []

    def spy(r, owner=None):
        if r.get("cmd") == "listen" and r.get("action") == "stop":
            STOPS.append(time.monotonic())
        if r.get("cmd") in ("listen", "stop"):
            SEEN.append(" ".join(str(x) for x in (r.get("cmd"), r.get("action") or r.get("reason") or "",
                                                  "drop" if r.get("drop") else "")).strip())
        return _d(r, owner=owner)
    core.dispatch = spy
    for how in ("quit", "drop+quit", "kill"):
        card("delay")
        m0, a0, s0 = n_mics(), len(audit()), len(STOPS)
        d = Drv()
        d.cmd("toggle")
        wait(lambda: card_procs(), 5)
        time.sleep(0.5)
        t0 = time.monotonic()
        if how == "quit":
            d.cmd("quit")
        elif how == "drop+quit":
            d.cmd("drop")
            d.cmd("quit")
        else:
            d.p.kill()
        d.p.wait(5)
        exited = time.monotonic() - t0
        time.sleep(4.5)
        check(f"button.{how}: the bubble exits while the card waits, Yes at 3 s -> no capture process",
              n_mics() == m0 and not mic_procs(), f"opened={n_mics() - m0} {tags_since(a0)}")
        t = tags_since(a0)
        check(f"button.{how}: audited CANCELLED, never CONFIRM", "CANCELLED" in t and "CONFIRM" not in t, str(t))
        if how != "kill":
            got = STOPS[s0:]
            check(f"button.{how}: Drop's listen stop reached the daemon within 1 s",
                  bool(got) and got[0] - t0 < 1.0, f"{[round(g - t0, 2) for g in got]}")
            check(f"button.{how}: the process exited within about 1 s", exited < 1.5, f"{exited:.2f}s")
        settle()
    # listening, then the bubble exits -> the mic stops
    for how in ("quit", "kill"):
        card("yes")
        EVENTS.clear()
        d = Drv()
        d.cmd("toggle")
        check(f"button.listen.{how}: Yes opens the mic", wait(lambda: mic_procs(), 5))
        time.sleep(0.5)
        if how == "quit":
            d.cmd("quit")
        else:
            d.p.kill()
        d.p.wait(5)
        check(f"button.listen.{how}: the bubble exits while listening -> the capture process stops",
              wait(lambda: not mic_procs(), 2), str(mic_procs()))
        time.sleep(0.5)
        check(f"button.listen.{how}: what it heard is dropped (nothing broadcast)",
              not [e for e in EVENTS if e.get("ev") == "transcript"], str(EVENTS))
        settle()
    # the safeword mid press (Mic::hush), x10, with a slow transcription: nothing is broadcast, and the
    # safeword stop reaches the daemon before the press's listen stop
    FakeRec.delay = 0.5
    for i in range(10):
        card("yes")
        a0 = len(audit())
        EVENTS.clear()
        SEEN.clear()
        d = Drv()
        d.cmd("toggle")
        wait(lambda: mic_procs(), 5)
        time.sleep(0.4)
        d.cmd("hush")
        stopped = wait(lambda: not mic_procs(), 3)
        time.sleep(1.0)
        trs = [e for e in EVENTS if e.get("ev") == "transcript"]
        heard = [ln for _, ln in d.out if "Heard" in ln]
        check(f"button.hush{i}: the safeword mid press -> mic closed, nothing broadcast or shown",
              stopped and not trs and not heard, f"broadcast={trs} bubble={heard} audit={tags_since(a0)}")
        sw = SEEN.index("stop safeword") if "stop safeword" in SEEN else None
        ls = SEEN.index("listen stop drop") if "listen stop drop" in SEEN else None
        check(f"button.hush{i}: safeword stop first, then listen stop drop", sw is not None and ls is not None
              and sw < ls and "listen stop" not in SEEN, str(SEEN))
        d.cmd("quit")
        d.p.wait(5)
        settle()
    FakeRec.delay = 0.0

server.shutdown()
L.mic.stop()
print(f"RESULT {'ALL PASSED' if not FAILS else 'FAILED: ' + ', '.join(FAILS)} ({NPASS[0]} pass, {len(FAILS)} fail)",
      flush=True)
sys.exit(1 if FAILS else 0)
