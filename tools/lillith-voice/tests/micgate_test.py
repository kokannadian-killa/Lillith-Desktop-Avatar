"""Mic gate + launch guards (no models, no daemon, no mic):  python tests/micgate_test.py      exit 0 = all passed
  * stt.Listener: push-to-talk / listen / always-listening start the mic only after a card Yes or a valid token;
    No / timeout / closed card / no card UI leave the mic off; tokens are one-time, per kind and expire
  * daemon.Core: mic_card / mic_revoke / listen_mode over dispatch
  * launchguard: absolute-executable check, $LILLITH_MIC_CMD allowlist, echo sanitizing, audit-line format
"""
import os
import stat
import sys
import tempfile
import threading

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
TMP = tempfile.mkdtemp(prefix="lillith-micgate-")
os.environ["XDG_DATA_HOME"] = os.path.join(TMP, "data")      # audit log goes here, not into the real one
os.environ["XDG_CONFIG_HOME"] = os.path.join(TMP, "config")
os.environ.pop("LILLITH_MIC_CMD", None)
os.environ.pop("LILLITH_CARD_CMD", None)

from lillith_voice import launchguard as G  # noqa: E402
from lillith_voice import micgate as M  # noqa: E402
from lillith_voice import stt  # noqa: E402

FAILS = []
AUDIT = os.path.join(TMP, "data", "lillith", "audit.log")


def check(name, cond, detail=""):
    print(("PASS " if cond else "FAIL ") + name + (f"  ({detail})" if detail and not cond else ""))
    if not cond:
        FAILS.append(name)


def audit_lines():
    try:
        with open(AUDIT, encoding="utf-8") as f:
            return f.read().splitlines()
    except FileNotFoundError:
        return []


class FakeMic:
    def __init__(self):
        self.running, self.starts, self.cards = False, 0, []

    def start(self, card=None):
        self.running = True
        self.starts += 1
        self.cards.append(card)

    def stop(self):
        self.running = False


class Clock:
    t = 1000.0

    def __call__(self):
        return self.t


def listener(answers, clock=None):
    """A Listener without models: only the parts the gate touches."""
    asked = []

    def ask(kind, title, summary, target, detail):
        asked.append((kind, title, detail))
        return answers.pop(0) if answers else "no_ui"

    L = object.__new__(stt.Listener)
    L.speaker, L.emit, L.always, L._mode, L._ptt, L._once = None, (lambda ev: None), False, None, [], None
    L._lock = threading.Lock()
    L._press_lock, L._press, L._press_seq = threading.Lock(), None, 0
    L._drops, L._once_dropped, L._stt_inflight = 0, False, 0
    L.mic = FakeMic()
    L.gate = M.MicGate(mic_argv=lambda: ["/usr/bin/arecord", "-q", "-t", "raw"], ask=ask,
                       clock=clock or time_clock())
    L.vad = type("V", (), {"reset": lambda self: None})()
    L.transcribe = lambda audio, gen=None: {"ok": True, "text": "", "samples": len(audio)}   # no models here
    return L, asked


def time_clock():
    import time
    return time.monotonic


# ------------------------------------------------------------------ the stt.py gate
for word in ("no", "timeout", "cancelled", "no_ui", "garbage"):
    L, asked = listener([word])
    r = L.ptt_start()
    check(f"ptt without Yes ({word}) leaves the mic off", r.get("ok") is False and r.get("need_card")
          and L.mic.starts == 0 and len(asked) == 1, str(r))
    L, _ = listener([word])
    r = L.set_always(True)
    check(f"always-listening without Yes ({word}) stays off", r.get("ok") is False and not L.always
          and L.mic.starts == 0, str(r))

L, asked = listener(["yes"])
r = L.ptt_start()
check("ptt: inline card Yes starts the mic", r.get("ok") and L.mic.starts == 1 and L.mic.cards[0]
      and asked[0][0] == "listen", str(r))
check("card shows the recorder one argument per line", "$ /usr/bin/arecord\n    -q\n    -t\n    raw" in asked[0][2],
      asked[0][2])
L.mic.stop()
r = L.ptt_start()
check("ptt: the next press needs a fresh card (no session grant)", r.get("ok") is False and r.get("need_card")
      and len(asked) == 2 and L.mic.starts == 1, str(r))
r = L.set_always(True)
check("always-listening is NOT covered by a ptt Yes", r.get("ok") is False and len(asked) == 3 and not L.always)

# card on EVERY press: 5 presses, 5 cards, 5 captures; a No in between leaves that press off
L, asked = listener(["yes", "yes", "no", "yes", "yes"])
oks = []
for _ in range(5):
    oks.append(bool(L.ptt_start().get("ok")))
    L.mic.stop()
check("ptt: one card per press (5 presses -> 5 cards)", len(asked) == 5 and all(a[0] == "listen" for a in asked),
      str(asked))
check("ptt: only the presses answered Yes open the mic", oks == [True, True, False, True, True]
      and L.mic.starts == 4, str(oks))
check("ptt: each capture carries its own card id", len(set(L.mic.cards)) == 4 and all(L.mic.cards), str(L.mic.cards))
check("ptt card text says it covers one press", "one press" in M._TEXT["listen"][1])
check("no standing grant attribute is left", not hasattr(L.gate, "listen_card"))

# a mic left open by an earlier press (no stop) is not reused without a new Yes
L, asked = listener(["yes", "no"])
L.ptt_start()
r = L.ptt_start()
check("ptt: a second press with the mic still open needs a card too", r.get("ok") is False and len(asked) == 2,
      str(r))

# a press while always-listening is on reuses that switch-on's open mic (no new capture, no card)
L, asked = listener(["yes"])
L.set_always(True)
r = L.ptt_start()
check("ptt during always-listening: no extra card (the mic is already open under that Yes)",
      r.get("ok") and len(asked) == 1 and L.always, str(r))

# a ptt token works for exactly one press
clock = Clock()
L, asked = listener(["yes"], clock)
t = L.gate.card("listen")
r = L.ptt_start(token=t["token"])
check("ptt with a valid listen token opens the mic once", r.get("ok") and L.mic.starts == 1
      and L.mic.cards[-1] == t["card"], str(r))
L.mic.stop()
r = L.ptt_start(token=t["token"])
check("the same ptt token is refused on the next press", r.get("ok") is False and L.mic.starts == 1, str(r))
r = L.ptt_start()
check("...and without a token the next press shows a card again", r.get("ok") is False and len(asked) == 2)

L, asked = listener(["yes", "yes"])
check("listen once: card Yes allows exactly one", L.gate.allow("listen") is None)
L.mic.running = False
check("listen once: the next listen asks again", L.gate.allow("listen") is None and len(asked) == 2)
L.gate.revoke()

# ------------------------------------------------------------------ the card belongs to ONE press (race fixes)
def blocking_listener(answer):
    """A listener whose card waits until `go` is set, then answers `answer` (unless cancelled meanwhile)."""
    shown, go = threading.Event(), threading.Event()
    asked = []

    def ask(kind, *a):
        asked.append(kind)
        shown.set()
        go.wait(5)
        return answer

    L, _ = listener([])
    L.gate._ask = ask
    return L, shown, go, asked


def press_async(L):
    out = {}
    th = threading.Thread(target=lambda: out.setdefault("r", L.ptt_start()))
    th.start()
    return th, out


def tags_for(card):
    return [ln.split("\t")[1] for ln in audit_lines() if f"card={card}" in ln]


# release before Yes (the voice bot's repro: card Yes at 3 s, release at 1 s)
L, shown, go, asked = blocking_listener("yes")
th, out = press_async(L)
shown.wait(5)
rel = L.ptt_stop()
check("release before Yes: the release is answered at once, nothing captured", rel.get("ok") and rel.get("cancelled")
      and rel.get("text") == "" and L.mic.starts == 0, str(rel))
go.set()          # the late Yes
th.join(5)
r = out.get("r", {})
check("release before Yes: a late Yes never opens the mic", r.get("ok") is False and r.get("released")
      and L.mic.starts == 0 and not L.mic.running, str(r))
check("release before Yes: the card is audited CANCELLED (not CONFIRM)", tags_for(r.get("card")) == ["PROPOSED", "CANCELLED"],
      str(tags_for(r.get("card"))))
check("release before Yes: no capture LAUNCH line", not any("\tmic_capture\t" in ln and r.get("card", "?") in ln
                                                         for ln in audit_lines()))
r = L.ptt_start()   # the next press asks again (its own new card)
check("after a cancelled press the next press shows a new card", len(asked) == 2 and r.get("ok")
      and L.gate.last_card != out["r"].get("card"), str(r))
L.ptt_stop()

# safeword before Yes (typed "red" -> {"cmd":"stop","reason":"safeword"} -> cancel_pending)
L, shown, go, asked = blocking_listener("yes")
th, out = press_async(L)
shown.wait(5)
import time as _t
t0 = _t.monotonic()
closed = L.cancel_pending("safeword")
dt = _t.monotonic() - t0
check("safeword before Yes closes the open card without blocking", closed and dt < 0.05, f"{dt * 1000:.1f} ms")
go.set()
th.join(5)
r = out.get("r", {})
check("safeword before Yes: the mic never opens", r.get("ok") is False and L.mic.starts == 0, str(r))
check("safeword before Yes: audited CANCELLED", tags_for(r.get("card")) == ["PROPOSED", "CANCELLED"],
      str(tags_for(r.get("card"))))
L.ptt_stop()
check("...and the release afterwards still stops (mic off)", not L.mic.running)

# timeout before Yes
L, shown, go, asked = blocking_listener("timeout")
go.set()
r = L.ptt_start()
check("timeout before Yes: the mic never opens", r.get("ok") is False and r.get("need_card") and L.mic.starts == 0,
      str(r))
check("timeout before Yes: audited TIMEOUT", tags_for(r.get("card")) == ["PROPOSED", "TIMEOUT"],
      str(tags_for(r.get("card"))))

# released after the Yes but before the mic opened (the narrow window): stays shut, audited
L, _ = listener(["yes"])
real_allow = L.gate.allow


def allow_then_release(*a, **k):
    res = real_allow(*a, **k)
    L._press = None          # what ptt_stop does, landing between the Yes and mic.start
    return res


L.gate.allow = allow_then_release
r = L.ptt_start()
check("released between Yes and mic start: the mic stays shut", r.get("ok") is False and r.get("released")
      and L.mic.starts == 0, str(r))
check("...and it is audited CANCELLED after the CONFIRM", audit_lines()[-1].split("\t")[1] == "CANCELLED"
      and "\tmic_listen\t" in audit_lines()[-1], audit_lines()[-1])

# the mic always stops on release
L, _ = listener(["yes"])
L.ptt_start()
L._mode = None     # even if the press bookkeeping were off
L.ptt_stop()
check("release always stops the mic", not L.mic.running)

# a release only closes its own press's card, not another client's always-listening card
g = M.MicGate(mic_argv=lambda: ["x"], ask=None)
g._cancel, g._owner = threading.Event(), None
check("a release (owner=press) leaves another card open", g.cancel(owner=7) is False and not g._cancel.is_set())
check("the safeword (no owner) closes any card", g.cancel() is True and g._cancel.is_set())

# a real card helper process is killed when the press is released
slow = os.path.join(TMP, "slow-card")
with open(slow, "w") as f:
    f.write("#!/bin/sh\ncat >/dev/null\nsleep 30\necho yes\n")
os.chmod(slow, 0o755)
ev = threading.Event()
threading.Timer(0.3, ev.set).start()
t0 = _t.monotonic()
w = M.run_card(slow, "t", "s", "x", "d", timeout=60, cancel=ev)
check("run_card: cancel closes a hanging card helper quickly and answers cancelled", w == "cancelled"
      and _t.monotonic() - t0 < 3, f"{w} {_t.monotonic() - t0:.1f}s")

# tokens
clock = Clock()
L, asked = listener(["yes", "yes"], clock)
t = L.gate.card("always")
check("mic_card Yes returns a one-time token", t.get("ok") and isinstance(t.get("token"), str)
      and len(t["token"]) >= 24, str(t))
r = L.ptt_start(token=t["token"])
check("an always token is refused for push-to-talk", r.get("ok") is False and L.mic.starts == 0, str(r))
t = L.gate.card("always")
r = L.set_always(True, token=t["token"])
check("always-listening with a valid token starts the mic", r.get("ok") and L.always and L.mic.starts == 1
      and L.mic.cards[-1] == t["card"], str(r))
L.set_always(False)
check("listen_mode off is never gated", not L.always and not L.mic.running)
r = L.set_always(True, token=t["token"])
check("a used token is refused (one-time)", r.get("ok") is False and not L.always, str(r))
r = L.set_always(True, token="forged-token-forged-token-forged")
check("a forged token is refused", r.get("ok") is False and not L.always)

L, asked = listener(["yes"], clock)
t = L.gate.card("always")
clock.t += M.TOKEN_TTL_S + 1
r = L.set_always(True, token=t["token"])
check("an expired token is refused", r.get("ok") is False and not L.always and len(asked) == 1, str(r))

L, asked = listener(["no"])
r = L.listen_once(timeout=0.01)
check("listen once without Yes leaves the mic off", r.get("ok") is False and L.mic.starts == 0, str(r))
L, asked = listener(["yes"])
r = L.gate.allow("listen")
check("listen once: card Yes allows", r is None and L.gate.last_card)

# one card at a time
gate_ev, release = threading.Event(), threading.Event()


def slow_ask(*a):
    gate_ev.set()
    release.wait(5)
    return "yes"


g = M.MicGate(mic_argv=lambda: ["x"], ask=slow_ask)
res = {}
th = threading.Thread(target=lambda: res.setdefault("a", g.card("always")))
th.start()
gate_ev.wait(5)
second = g.card("always")
release.set()
th.join(5)
check("a second card while one is open is refused", second.get("ok") is False and second.get("need_card")
      and res["a"].get("ok"), str(second))

# audit
lines = audit_lines()
tags = [ln.split("\t")[1] for ln in lines]
check("card decisions are audited", all(t in tags for t in ("PROPOSED", "CONFIRM", "DENY", "TIMEOUT", "CANCELLED",
                                                            "NO_UI")), str(set(tags)))
check("audit lines carry card=<id> and 6 columns", all(len(ln.split("\t")) == 6 and
                                                       ln.split("\t")[4].startswith("card=mic-") for ln in lines
                                                       if "\tmic_card_ui\t" not in ln         # helper LAUNCH: no card
                                                       and "\tREVOKED\t" not in ln))          # revokes: no card

# no card helper at all -> no_ui
check("no card program -> no_ui", M.run_card(None, "t", "s", "x", "d") == "no_ui")
fake = os.path.join(TMP, "fake-card")
with open(fake, "w") as f:
    f.write("#!/bin/sh\ncat >/dev/null\necho yes\n")
os.chmod(fake, 0o755)
check("card helper answer 'yes' is read", M.run_card(fake, "t", "s", "x", "d") == "yes")
with open(fake, "w") as f:
    f.write("#!/bin/sh\ncat >/dev/null\necho 'yes please'\n")
check("anything but an exact 'yes' is no_ui", M.run_card(fake, "t", "s", "x", "d") == "no_ui")
check("LILLITH_CARD_CMD must be absolute", M.card_program({"LILLITH_CARD_CMD": "fake-card", "PATH": ""}) is None)
check("LILLITH_CARD_CMD absolute executable is used", M.card_program({"LILLITH_CARD_CMD": fake}) == fake)

# ------------------------------------------------------------------ daemon dispatch
try:
    from lillith_voice import daemon as D

    class _Sp:
        voice = None

    core = object.__new__(D.Core)
    core.speaker, core._listener, core._llock = _Sp(), None, threading.Lock()
    core.gate = M.MicGate(mic_argv=lambda: ["x"], ask=lambda *a: "no")
    r = core.dispatch({"cmd": "mic_card", "kind": "always"})
    check("daemon mic_card: No -> need_card", r.get("ok") is False and r.get("need_card"), str(r))
    r = core.dispatch({"cmd": "mic_card", "kind": "camera"})
    check("daemon mic_card: unknown kind refused", r.get("ok") is False)
    check("daemon mic_revoke", core.dispatch({"cmd": "mic_revoke"}).get("revoked"))

    class _Sp2:
        voice = None
        stops = []

        def stop(self, reason="stop"):
            self.stops.append(reason)

    core.speaker = _Sp2()
    core.gate._cancel, core.gate._owner = threading.Event(), 3
    import time as _t2
    t0 = _t2.monotonic()
    r = core.dispatch({"cmd": "stop", "reason": "safeword"})
    check("daemon stop (safeword) is ungated, fast, and closes an open mic card",
          r.get("ok") and core.gate._cancel.is_set() and _t2.monotonic() - t0 < 0.05 and core.speaker.stops == ["safeword"])
    core.gate._cancel = threading.Event()
    core.dispatch({"cmd": "stop"})
    check("a plain stop (not the safeword) leaves a mic card alone", not core.gate._cancel.is_set())
    # a token minted before a stop / safeword is dead afterwards (daemon level, no listener loaded)
    for req in ({"cmd": "stop", "reason": "safeword"}, {"cmd": "stop"}):
        core.gate = M.MicGate(mic_argv=lambda: ["x"], ask=lambda *a: "yes")
        tok = core.dispatch({"cmd": "mic_card", "kind": "listen"}).get("token")
        core.dispatch(req)
        r = core.gate.allow("listen", tok)
        check(f"daemon {req}: an earlier mic token is revoked (no listener branch)", bool(tok) and r is not None
              and r.get("need_card"), str(r))
    check("REVOKED audit lines carry no speech text", all(ln.split("\t")[2].startswith("mic_revoke_")
                                                          for ln in audit_lines() if "\tREVOKED\t" in ln))
    check("PROTOCOL documents the gate", "mic_card" in D.PROTOCOL and "token" in D.PROTOCOL)
except ImportError as e:   # the daemon needs the TTS stack only for Speaker; tolerate a missing optional dep
    print(f"SKIP daemon dispatch ({e})")

# ------------------------------------------------------------------ launchguard
exe = os.path.join(TMP, "bin", "arecord")
os.makedirs(os.path.dirname(exe))
with open(exe, "w") as f:
    f.write("#!/bin/sh\n")
os.chmod(exe, 0o755)
noexec = os.path.join(TMP, "bin", "parec")
open(noexec, "w").close()
os.chmod(noexec, 0o644)
check("checked_exe: absolute executable ok", G.checked_exe(exe)[0] == exe)
check("checked_exe: relative refused", G.checked_exe("arecord")[0] is None)
check("checked_exe: missing refused", G.checked_exe(exe + "-nope")[0] is None)
check("checked_exe: directory refused", G.checked_exe(os.path.dirname(exe))[0] is None)
check("checked_exe: not executable refused", G.checked_exe(noexec)[0] is None)
check("env_exe refuses a bare name", G.env_exe("LILLITH_FFMPEG", {"LILLITH_FFMPEG": "ffmpeg"}) is None)
check("env_command splits without a shell + checks argv0",
      G.env_command("P", {"P": f"{exe} --volume 0.8 {{}}"}) == [exe, "--volume", "0.8", "{}"])
check("env_command refuses a relative player", G.env_command("P", {"P": "pw-play {}"}) is False)
check("env_command refuses unparsable quoting", G.env_command("P", {"P": f"{exe} 'oops"}) is False)

ok_cmd = f"{exe} -q -t raw -f S16_LE -r 16000 -c 1 -D hw:1,0"
check("mic cmd: allowlisted recorder ok", G.mic_env_command({"LILLITH_MIC_CMD": ok_cmd}) == ok_cmd.split())
check("mic cmd: /dev/ device ok", G.mic_env_command({"LILLITH_MIC_CMD": f"{exe} -D /dev/snd/pcmC0D0c"}) is not None)
bad = {
    "relative program": "arecord -q",
    "not allowlisted": f"{os.path.join(TMP, 'bin', 'sh')} -c x",
    "python wrapper": f"{sys.executable} fake_mic.py /tmp",
    "file path arg": f"{exe} /tmp/out",
    "file name arg": f"{exe} out.wav",
    "url": f"{exe} http://x",
    "shell metachar": f"{exe} -q;rm",
    "dotdot": f"{exe} -D /dev/../etc/passwd",
    "too many args": exe + " -q" * 40,
    "not executable": f"{noexec} --raw",
}
for why, c in bad.items():
    check(f"mic cmd refused: {why}", G.mic_env_command({"LILLITH_MIC_CMD": c}) is None, c)
check("mic device with a space refused", not G.mic_arg_ok("my mic"))
check("pulse source names allowed", G.mic_arg_ok("alsa_input.pci-0000_00_1f.3.analog-stereo"))

E = G.sanitize_echo
check("echo: default preset value kept", E("0.8:0.85:28|47:0.06|0.03") == "0.8:0.85:28|47:0.06|0.03")
check("echo: empty is off", E("") == "")
for bad_e in ("0.8:0.85:28|47:0.06", "0.8:0.85:28:0.06,volume=10", "1.5:0.8:28:0.1", "0.8:0.85:-5:0.1",
              "0.8:0.85:99999:0.1", "0.8:0.85:28:0.1:9", "a:b:c:d", "0.8:0.85:28:0.1[x]", "1e3:0.8:28:0.1",
              ":".join(["0.8", "0.8", "|".join(["10"] * 9), "|".join(["0.1"] * 9)]), 5, None):
    if bad_e is None:
        continue
    check(f"echo rejected: {bad_e!r}", E(bad_e) is None)
check("safe_echo turns a bad echo off", G.safe_echo("0.8:0.8:28:0.1,amix") == "")
o = G.sanitize_override("x", {"highpass": "90,volume=9", "speed": 99, "eq": [[300, 1, "x"]], "echo": "0.8;x",
                              "treble": [2, 12000], "mix": {"af_bella": 1.0}})
check("override: non-number dropped, out-of-range clamped, bad eq dropped, bad echo off",
      "highpass" not in o and o["speed"] == 4.0 and "eq" not in o and o["echo"] == "" and o["treble"] == [2.0, 12000.0]
      and o["mix"] == {"af_bella": 1.0}, str(o))

ln = G.format_line("2026-10-24T00:00:00Z", "LAUNCH", "k\tx", ["/bin/x", "a\nb", "--token", "s3cret", "--api-key=k"],
                   "c1")
check("audit format: 6 tab columns, one line", ln.count("\t") == 5 and ln.endswith("\n") and ln.count("\n") == 1, ln)
check("audit format: masked + card", "s3cret" not in ln and "--api-key=***" in ln and "\tcard=c1\t" in ln, ln)
check("audit format: no card", "\tno-card\t" in G.format_line("t", "LAUNCH", "k", ["/bin/x"]))
check("data dir mirrors the Rust app", G.data_dir({"XDG_DATA_HOME": "/d"}, False) == "/d/lillith"
      and G.data_dir({"HOME": "/h"}, False) == "/h/.local/share/lillith"
      and G.data_dir({"LOCALAPPDATA": "C:/L", "HOME": "/h"}, True) == os.path.join("C:/L", "lillith"))

# ------------------------------------------------------------------ core._find_ffmpeg (bundled Windows ffmpeg.exe)
from lillith_voice import core as C  # noqa: E402

vh = os.path.join(TMP, "voicehome")
os.makedirs(os.path.join(vh, "bin"))
bundled = os.path.join(vh, "bin", "ffmpeg.exe")
saved = (C.WINDOWS, C.VOICE_HOME, os.environ.get("LILLITH_FFMPEG"), C.shutil.which)
try:
    C.WINDOWS, C.VOICE_HOME = True, vh
    C.shutil.which = lambda name, *a, **k: None
    os.environ.pop("LILLITH_FFMPEG", None)
    check("ffmpeg: missing bundled ffmpeg.exe is not used", C._find_ffmpeg() is None)
    with open(bundled, "w") as f:
        f.write("#!/bin/sh\n")
    os.chmod(bundled, 0o644)
    if os.name != "nt":
        check("ffmpeg: non-executable bundled ffmpeg.exe refused", C._find_ffmpeg() is None)
    os.chmod(bundled, 0o755)
    check("ffmpeg: bundled ffmpeg.exe passes the absolute executable check", C._find_ffmpeg() == bundled)
    os.environ["LILLITH_FFMPEG"] = "ffmpeg"
    check("ffmpeg: relative $LILLITH_FFMPEG refused, bundled used", C._find_ffmpeg() == bundled)
    os.environ["LILLITH_FFMPEG"] = exe
    check("ffmpeg: absolute executable $LILLITH_FFMPEG wins", C._find_ffmpeg() == exe)
    os.environ.pop("LILLITH_FFMPEG", None)
    C.VOICE_HOME = "relative/voicehome"
    check("ffmpeg: a relative voice home is refused", C._find_ffmpeg() is None)
    check("NO_WINDOW is one definition", C.NO_WINDOW is G.NO_WINDOW)
finally:
    C.WINDOWS, C.VOICE_HOME, env_ff, C.shutil.which = saved
    if env_ff is not None:
        os.environ["LILLITH_FFMPEG"] = env_ff

print(f"{'ALL PASSED' if not FAILS else str(len(FAILS)) + ' FAILED'}")
sys.exit(1 if FAILS else 0)
