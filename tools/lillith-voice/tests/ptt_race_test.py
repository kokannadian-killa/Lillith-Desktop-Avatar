"""Push-to-talk race regression, with real processes (no models, no daemon):  python tests/ptt_race_test.py
Mirrors the voice-QA repro (race_test.py / race_run.sh): a real card helper that answers Yes late, a real
capture process (tests/fake_mic.py behind an `arecord` shim), and checks the process table, not just flags.
  * release before Yes  -> the late Yes never starts a capture process; audited CANCELLED
  * safeword before Yes -> the card helper is closed at once; no capture process; audited CANCELLED
  * timeout before Yes  -> no capture process; audited TIMEOUT
  * Yes, then release   -> the capture process is gone after the release
exit 0 = all passed
"""
import os
import subprocess
import sys
import tempfile
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
TMP = tempfile.mkdtemp(prefix="lillith-pttrace-")
os.environ["XDG_DATA_HOME"] = os.path.join(TMP, "data")
os.environ["XDG_CONFIG_HOME"] = os.path.join(TMP, "config")
BIN = os.path.join(TMP, "bin")
os.makedirs(BIN)
with open(os.path.join(BIN, "arecord"), "w") as f:
    f.write(f'#!/bin/sh\nexec "{sys.executable}" "{os.path.join(HERE, "fake_mic.py")}" "{TMP}" pttrace-mic-marker\n')
MODE = os.path.join(TMP, "card_mode")
with open(os.path.join(BIN, "card"), "w") as f:
    f.write(f"""#!/bin/sh
cat >/dev/null
m=$(cat "{MODE}")
[ "$m" = delay ] && {{ sleep 3; echo yes; exit 0; }}
[ "$m" = hang ] && exec "{sys.executable}" -c "import time; time.sleep(300)" pttrace-card-marker
echo "$m"
""")
for n in ("arecord", "card"):
    os.chmod(os.path.join(BIN, n), 0o755)
os.environ["LILLITH_MIC_CMD"] = os.path.join(BIN, "arecord")
os.environ["LILLITH_CARD_CMD"] = os.path.join(BIN, "card")

from lillith_voice import micgate as M  # noqa: E402
from lillith_voice import stt  # noqa: E402

FAILS = []
AUDIT = os.path.join(TMP, "data", "lillith", "audit.log")


def check(name, cond, detail=""):
    print(("PASS " if cond else "FAIL ") + name + (f"  ({detail})" if detail else ""), flush=True)
    if not cond:
        FAILS.append(name)


def procs(marker):
    out = subprocess.run(["ps", "-eo", "pid=,args="], capture_output=True, text=True).stdout.splitlines()
    return [ln.strip() for ln in out if marker in ln and "ps -eo" not in ln]


def mic_procs():
    return procs("pttrace-mic-marker")


def card(mode):
    with open(MODE, "w") as f:
        f.write(mode)


def tags(card_id):
    try:
        with open(AUDIT, encoding="utf-8") as f:
            return [ln.split("\t")[1] for ln in f.read().splitlines() if f"card={card_id}" in ln]
    except FileNotFoundError:
        return []


def listener():
    L = object.__new__(stt.Listener)
    L.speaker, L.emit, L.always, L._mode, L._ptt, L._once = None, (lambda ev: None), False, None, [], None
    L._lock = threading.Lock()
    L._press_lock, L._press, L._press_seq = threading.Lock(), None, 0
    L._drops, L._once_dropped = 0, False
    L.device = None
    L.mic = stt.Mic(L._on_audio, None)
    L.gate = M.MicGate(mic_argv=stt.mic_argv)
    L.vad = type("V", (), {"reset": lambda self: None})()
    L.transcribe = lambda audio, gen=None: {"ok": True, "text": "", "samples": len(audio)}   # no models
    return L


def press(L):
    res = {}

    def go():
        t = time.monotonic()
        res["r"] = L.ptt_start()
        res["dt"] = time.monotonic() - t
    th = threading.Thread(target=go)
    th.start()
    return th, res


check("the capture command is the fake recorder", stt.mic_argv()[0] == os.path.join(BIN, "arecord"), str(stt.mic_argv()))

# 1. release before Yes (the repro: Yes after 3 s, release after 1 s)
card("delay")
L = listener()
th, res = press(L)
time.sleep(1.0)
rel = L.ptt_stop()
check("release while the card is pending: no capture process", mic_procs() == [], str(mic_procs()))
check("...and the release reply is immediate and empty", rel.get("cancelled") and rel.get("text") == "", str(rel))
th.join(15)
time.sleep(1.5)
r = res.get("r", {})
check("after a late Yes for a released press, no capture process", mic_procs() == [] and not L.mic.running,
      str(mic_procs()))
check("the press answers {ok:false, released:true}", r.get("ok") is False and r.get("released") is True, str(r))
check("the card was closed when released (not waited for)", res.get("dt", 99) < 2.0, f"{res.get('dt', 0):.2f}s")
check("audited PROPOSED then CANCELLED, never CONFIRM", tags(r.get("card")) == ["PROPOSED", "CANCELLED"],
      str(tags(r.get("card"))))

# 2. safeword before Yes: the card helper hangs; the safeword closes it at once
card("hang")
L = listener()
th, res = press(L)
for _ in range(100):
    if procs("pttrace-card-marker"):
        break
    time.sleep(0.05)
t0 = time.monotonic()
L.cancel_pending("safeword")
dt = time.monotonic() - t0
th.join(5)
r = res.get("r", {})
check("safeword: cancel_pending returns at once", dt < 0.05, f"{dt * 1000:.1f} ms")
check("safeword: the press returns quickly", not th.is_alive(), "")
time.sleep(0.3)
check("safeword: the card helper process is gone", procs("pttrace-card-marker") == [], str(procs("pttrace-card-marker")))
check("safeword: no capture process", mic_procs() == [], str(mic_procs()))
check("safeword: audited CANCELLED", tags(r.get("card")) == ["PROPOSED", "CANCELLED"], str(tags(r.get("card"))))
L.ptt_stop()

# 3. timeout before Yes
card("timeout")
L = listener()
r = L.ptt_start()
check("timeout: no capture process", r.get("ok") is False and mic_procs() == [], str(r))
check("timeout: audited TIMEOUT", tags(r.get("card")) == ["PROPOSED", "TIMEOUT"], str(tags(r.get("card"))))
L.ptt_stop()

# 4. Yes then release: the mic opens for this press only, and the release always stops it
card("yes")
L = listener()
r = L.ptt_start()
time.sleep(0.4)
check("Yes: the press opens one capture process", r.get("ok") and len(mic_procs()) == 1, str(mic_procs()))
L.ptt_stop()
for _ in range(40):
    if not mic_procs():
        break
    time.sleep(0.05)
check("release: the capture process is gone", mic_procs() == [] and not L.mic.running, str(mic_procs()))
r2 = L.ptt_start()
time.sleep(0.3)
check("the next press shows a new card", r2.get("ok") and L.gate.last_card != r.get("card"))
L.ptt_stop()
time.sleep(0.3)
check("...and its release stops the mic too", mic_procs() == [], str(mic_procs()))

# 5. a one-time token minted before a safeword / stop / cancelled or timed-out card is dead afterwards
try:
    from lillith_voice import daemon as D
except ImportError as e:
    D = None
    print(f"SKIP daemon-level token checks ({e})")


class _Sp:
    voice = None

    def stop(self, reason="stop"):
        pass


def core_for(L):
    c = object.__new__(D.Core)
    c.speaker, c._listener, c._llock, c.gate = _Sp(), L, threading.Lock(), L.gate
    return c


def kill_cancel(L):
    card("hang")
    th = threading.Thread(target=lambda: L.gate.card("listen"))
    th.start()
    for _ in range(100):
        if procs("pttrace-card-marker"):
            break
        time.sleep(0.05)
    L.gate.cancel()
    th.join(5)


def kill_timeout(L):
    card("timeout")
    L.gate.card("listen")


def kill_release(L):
    card("hang")
    th = threading.Thread(target=L.ptt_start)
    th.start()
    for _ in range(100):
        if procs("pttrace-card-marker"):
            break
        time.sleep(0.05)
    L.ptt_stop()
    th.join(5)


cases = [("safeword (spoken red / OOC -> cancel_pending)", lambda L: L.cancel_pending("safeword")),
         ("a cancelled card", kill_cancel), ("a timed-out card", kill_timeout),
         ("a press released before Yes", kill_release)]
if D is not None:
    cases += [("{cmd:stop, reason:safeword}", lambda L: core_for(L).dispatch({"cmd": "stop", "reason": "safeword"})),
              ("{cmd:stop, reason:safeword} (no listener loaded)",
               lambda L: (lambda c: (setattr(c, "_listener", None), c.dispatch({"cmd": "stop", "reason": "safeword"})))(
                   core_for(L))),
              ("a plain {cmd:stop}", lambda L: core_for(L).dispatch({"cmd": "stop"}))]
for name, kill in cases:
    L = listener()
    card("yes")
    tok = L.gate.card("listen").get("token")
    kill(L)
    card("yes")       # even if a card were shown now it would say yes: the token itself must be refused
    r = L.ptt_start(token=tok)
    time.sleep(0.3)
    check(f"token then {name}: listen start with the old token is refused, no mic process",
          bool(tok) and r.get("ok") is False and r.get("need_card") and mic_procs() == [], f"{r} {mic_procs()}")
    L.ptt_stop()
with open(AUDIT, encoding="utf-8") as f:
    rv = [ln for ln in f.read().splitlines() if "\tREVOKED\t" in ln]
check("revokes are audited REVOKED (kind mic_revoke_<why>, no speech text)", rv and all(
    ln.split("\t")[2].startswith("mic_revoke_") and ln.split("\t")[4] == "no-card" for ln in rv), str(rv[:2]))

print("ALL PASSED" if not FAILS else f"{len(FAILS)} FAILED")
sys.exit(1 if FAILS else 0)
