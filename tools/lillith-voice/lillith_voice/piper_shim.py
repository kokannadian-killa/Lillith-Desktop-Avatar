"""Piper-compatible shim so wt-tts's lillith-voice crate speaks in Lillith's (cloned) voice with zero code changes.

The crate runs:  piper --model M --output_raw [--length_scale X]   (text on stdin)
and plays raw s16le mono at the "sample_rate" found in M.json.  We always emit 24000 Hz (the .json says so),
whatever the preset (p1 is resampled down). The default preset (LILLITH_VOICE_PRESET, else 'lillith') is used.
"""
from . import netguard  # noqa: F401

import argparse
import sys

from .cli import main as say_main


def main(argv=None):
    ap = argparse.ArgumentParser(prog="piper (lillith-voice shim)")
    ap.add_argument("--model", "-m")
    ap.add_argument("--output_raw", "--output-raw", action="store_true")
    ap.add_argument("--output_file", "--output-file", "-f")
    ap.add_argument("--length_scale", "--length-scale", type=float)
    ap.add_argument("--speaker", "-s")
    a, _ = ap.parse_known_args(argv)
    text = sys.stdin.read()
    args = []
    if a.length_scale:
        from .core import get_preset
        if abs(a.length_scale - 1.0) > 0.01:
            args += ["--speed", f"{get_preset(None).speed / a.length_scale:.3f}"]
    if a.speaker:
        from .core import PRESETS, ALIASES
        if a.speaker in PRESETS or a.speaker.lower() in ALIASES:  # piper speaker ids (numbers) are ignored
            args += ["--preset", a.speaker]
    if a.output_file and not a.output_raw:
        args += ["--out", a.output_file]
    else:
        args += ["--raw", "--rate", "24000"]
    return say_main(args + ["--", text])


if __name__ == "__main__":
    sys.exit(main())
