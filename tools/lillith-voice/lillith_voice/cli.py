"""lillith-say: speak text in Lillith's voice. 100% offline."""
from . import netguard  # noqa: F401

import argparse
import json
import os
import re
import sys
import tempfile
import wave

from .core import PRESETS, ALIASES, DEFAULT_PRESET, SR, runtime_dir, has_rubberband, player_cmd, pocket_available, POCKET_DIR

_FLUSH_RE = re.compile(r"^(.*?[.!?…]+[\"')\]]*)(\s+)(.*)$", re.S)


def build_parser():
    ap = argparse.ArgumentParser(
        prog="lillith-say",
        description="Speak text in Lillith's local voice (default: Ko's cloned voice). Text from args, or stdin if none / '-'.")
    ap.add_argument("text", nargs="*")
    ap.add_argument("-p", "--preset", default=None, help=f"voice preset (default: {DEFAULT_PRESET}); see --list")
    ap.add_argument("-o", "--out", help="write WAV here instead of playing")
    ap.add_argument("--raw", action="store_true", help="write raw s16le mono PCM to stdout (piper-style)")
    ap.add_argument("--rate", type=int, default=None,
                    help="output sample rate for --out/--raw (default: the preset's, 24000; p1 = 44100)")
    ap.add_argument("-s", "--speed", type=float, help="override preset speed (0.5..2.0)")
    ap.add_argument("--pitch", type=float, help="override preset pitch shift in semitones")
    ap.add_argument("--no-fx", action="store_true", help="raw engine output, no pitch/EQ/echo/loudness")
    ap.add_argument("--clone", nargs=2, metavar=("REF_WAV", "NAME"),
                    help="make a new voice from a 24 kHz mono WAV of a CONSENTING speaker (then: -p with voice NAME "
                         "via presets.json)")
    ap.add_argument("--bench", action="store_true", help="measure speed of the chosen preset on this machine")
    ap.add_argument("--actions", choices=["drop", "read"], default="drop", help="*giggles* style text: drop or read")
    ap.add_argument("--stream", action="store_true",
                    help="read stdin incrementally and speak each sentence as soon as it is complete")
    ap.add_argument("--tee", action="store_true", help="with --stream: echo stdin to stdout")
    ap.add_argument("--no-wait", action="store_true", help="queue speech on the daemon and return")
    mode = ap.add_mutually_exclusive_group()
    mode.add_argument("--no-daemon", action="store_true", help="always run in-process (one-shot)")
    mode.add_argument("--daemon", action="store_true", help="require the daemon")
    ap.add_argument("--threads", type=int, help="ONNX threads in one-shot mode")
    ap.add_argument("--stop", action="store_true", help="stop current speech (daemon)")
    ap.add_argument("--list", "--list-presets", dest="list", action="store_true")
    ap.add_argument("--status", action="store_true", help="show daemon / DSP / player status")
    ap.add_argument("-v", "--verbose", action="store_true", help="print timing / RTF as JSON on stderr")
    return ap


def main(argv=None):
    a = build_parser().parse_args(argv)
    if a.list:
        for n, p in PRESETS.items():
            what = (f"pocket:{p.voice}" + (f" int8={p.int8}" if p.int8 else "") if p.engine == "pocket"
                    else "+".join(f"{v}*{w:g}" for v, w in p.mix.items()))
            star = "*" if n == DEFAULT_PRESET else " "
            print(f"{star}{n:17} {what:42} speed={p.speed:<5g} chain={p.chain:7} {p.note}")
        if not pocket_available():
            print(f"(cloned-voice models not installed in {POCKET_DIR}: 'lillith' falls back to p1)")
        print("aliases:", ", ".join(f"{k}->{v}" for k, v in ALIASES.items()))
        return 0

    if a.clone:
        from .core import Voice
        v = Voice(threads=a.threads, preload=False)
        print(v.clone(a.clone[0], a.clone[1]))
        return 0
    if a.bench:
        return _bench(a)

    from .client import connect, DaemonBackend
    if a.status:
        try:
            info = DaemonBackend().ping()
        except OSError:
            info = {"daemon": "not running"}
        info.update({"rubberband": has_rubberband(), "player": player_cmd(), "runtime_dir": runtime_dir(),
                     "cloned_voice_models": pocket_available(), "default_preset": DEFAULT_PRESET})
        print(json.dumps(info, indent=1))
        return 0
    if a.stop:
        try:
            DaemonBackend().stop()
        except OSError:
            pass
        return 0

    knobs = {k: v for k, v in dict(preset=a.preset, speed=a.speed, pitch=a.pitch).items() if v is not None}
    knobs["fx"] = not a.no_fx
    knobs["actions"] = a.actions
    be = connect("local" if a.no_daemon else ("daemon" if a.daemon else "auto"), a.threads, a.preset)

    def log(st):
        if a.verbose:
            st = dict(st, backend=be.name)
            print(json.dumps(st), file=sys.stderr)

    if a.stream:
        return _stream(be, knobs, a, log)

    text = " ".join(a.text) if a.text and a.text != ["-"] else None
    if text is None:
        if sys.stdin.isatty():
            print("lillith-say: no text (pass it as arguments or on stdin)", file=sys.stderr)
            return 2
        text = sys.stdin.read()
    if not text.strip():
        return 0

    if a.out or a.raw:
        out = a.out
        if a.raw:
            fd, out = tempfile.mkstemp(suffix=".wav", dir=runtime_dir())
            os.close(fd)
        rate = a.rate or (SR if a.raw else None)
        st = be.file(text, out, **({"rate": rate} if rate else {}), **knobs)
        log(st)
        if a.raw:
            try:
                with wave.open(out, "rb") as w:
                    sys.stdout.buffer.write(w.readframes(w.getnframes()))
                sys.stdout.buffer.flush()
            finally:
                os.unlink(out)
        return 0
    st = be.say(text, wait=not a.no_wait, **knobs)
    log(st)
    return 0


def _bench(a):
    import time
    from .core import Voice, get_preset
    p = get_preset(a.preset)
    v = Voice(threads=a.threads, preload=p.name)
    lines = ["Oh my god, hiii! Like, there you are?",
             "I've been, like, sooo bored without you, it's literally so not fair.",
             "Come here, babe. Closer? Hehe. Are you gonna, like, play with me tonight, or what?"]
    v.render("Hi.", p)
    tot_a = tot_c = 0.0
    first = None
    c0 = time.process_time()
    for ln in lines:
        _, st = v.render(ln, p)
        tot_a += st["audio_s"]
        tot_c += st["synth_s"] + st["dsp_s"]
        first = first or st["synth_s"] + st["dsp_s"]
    cpu = time.process_time() - c0
    rtf = tot_c / tot_a
    print(json.dumps({"preset": p.name, "engine": p.engine, "threads": v.threads, "load_s": round(v.load_s, 2),
                      "audio_s": round(tot_a, 2), "rtf": round(rtf, 3), "cpu_s_per_audio_s": round(cpu / tot_a, 2),
                      "first_sentence_s": round(first, 2)}))
    if rtf > 0.9 and p.engine == "pocket" and not p.int8:
        print("tip: too slow for gap-free live speech here; try  -p lillith_fast  "
              "(or set LILLITH_VOICE_PRESET=lillith_fast)", file=sys.stderr)
    return 0


def _stream(be, knobs, a, log):
    buf = ""
    last = None
    fd = sys.stdin.fileno()
    while True:
        chunk = os.read(fd, 4096)
        if not chunk:
            break
        s = chunk.decode(errors="replace")
        if a.tee:
            sys.stdout.write(s)
            sys.stdout.flush()
        buf += s
        while True:
            m = _FLUSH_RE.match(buf)
            if m and len(m.group(1).strip()) >= 2:
                be.say(m.group(1), wait=False, **knobs)
                last = True
                buf = m.group(3)
            elif len(buf) > 400 and ", " in buf[200:]:
                cut = buf.rfind(", ") + 1
                be.say(buf[:cut], wait=False, **knobs)
                buf = buf[cut:]
            else:
                break
    if buf.strip():
        st = be.say(buf, wait=True, **knobs)
        log(st)
    elif last:
        be.say(" ", wait=True, **knobs)  # barrier: returns once the queue is drained... (empty job)
    return 0


if __name__ == "__main__":
    sys.exit(main())
