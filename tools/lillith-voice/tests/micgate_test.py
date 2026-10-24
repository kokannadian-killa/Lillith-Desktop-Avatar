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
    L.mic = FakeMic()
    L.gate = M.MicGate(mic_argv=lambda: ["/usr/bin/arecord", "-q", "-t", "raw"], ask=ask,
                       clock=clock or time_clock())
    L.vad = type("V", (), {"reset": lambda self: None})()
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
check("ptt: the session grant covers later presses (no second card)", r.get("ok") and len(asked) == 1
      and L.mic.starts == 2)
r = L.set_always(True)
check("always-listening is NOT covered by the ptt grant", r.get("ok") is False and len(asked) == 2 and not L.always)
L.gate.revoke()
L.mic.stop()
r = L.ptt_start()
check("mic_revoke ends the ptt grant (card again)", r.get("ok") is False and len(asked) == 3)

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
                                                       ln.split("\t")[4].startswith("card=mic-") for ln in lines))

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

print(f"{'ALL PASSED' if not FAILS else str(len(FAILS)) + ' FAILED'}")
sys.exit(1 if FAILS else 0)
