"""lillith-listen: local mic -> text (whisper + silero VAD), plus the first-run mic setup step.

  lillith-listen                 one utterance (ends on silence), prints the transcript
  lillith-listen --ptt           Enter to start, Enter to stop
  lillith-listen --wav f.wav     transcribe a file
  lillith-listen --devices       list capture devices
  lillith-listen --setup         first-run: pick a mic, test level + transcription, save ~/.config/lillith-voice/mic.json
"""
from . import netguard  # noqa: F401

import argparse
import json
import os
import sys
import time

import numpy as np


def setup(a):
    from .stt import list_devices, Mic, Listener, MIC_CONFIG, mic_config, RATE
    print("Lillith needs your microphone so you can talk to her. Audio never leaves this computer.")
    devs = list_devices()
    for i, d in enumerate(devs):
        print(f"  [{i}] {d['id']}  {d['desc']}")
    cfg = mic_config()
    pick = a.device
    if pick is None and devs and sys.stdin.isatty():
        s = input(f"mic number (Enter = system default{', current: ' + cfg['device'] if cfg.get('device') else ''}): ").strip()
        pick = devs[int(s)]["id"] if s.isdigit() and int(s) < len(devs) else None
    cfg["device"] = pick
    buf = []
    m = Mic(buf.append, pick)
    print("Say something for 4 seconds (e.g. 'Hey Lillith, can you hear me?') ...")
    try:
        m.start()
    except RuntimeError as e:
        print(f"mic error: {e}\nOn Linux the first use may need permission in your desktop's privacy settings "
              "(PipeWire/portal) - allow it and run --setup again.")
        return 1
    time.sleep(4.0)
    m.stop()
    audio = np.concatenate(buf) if buf else np.zeros(0, np.float32)
    lvl = 20 * np.log10(np.sqrt(np.mean(audio ** 2)) + 1e-9) if len(audio) else -120
    print(f"level: {lvl:.1f} dBFS ({len(audio) / RATE:.1f} s captured)")
    if len(audio) < RATE or lvl < -60:
        print("No sound captured - check the device / that the mic is not muted / permissions.")
    L = Listener(None)
    r = L.transcribe(audio)
    print(f"heard: {r['text']!r}")
    cfg.setdefault("echo", "mute")          # mute the mic while she talks (no echo loops)
    cfg.setdefault("barge_in", True)        # talk over her to interrupt
    cfg.setdefault("barge_in_dbfs", round(min(-20.0, max(-45.0, lvl + 6)), 1) if lvl > -60 else -32.0)
    cfg.setdefault("mode", "ptt")           # ptt | always
    os.makedirs(os.path.dirname(MIC_CONFIG), exist_ok=True)
    with open(MIC_CONFIG, "w", encoding="utf-8") as f:
        json.dump(cfg, f, indent=1)
    print(f"saved {MIC_CONFIG}")
    return 0


def main(argv=None):
    ap = argparse.ArgumentParser(prog="lillith-listen", description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--wav")
    ap.add_argument("--ptt", action="store_true")
    ap.add_argument("--devices", action="store_true")
    ap.add_argument("--setup", action="store_true")
    ap.add_argument("--device")
    ap.add_argument("--timeout", type=float, default=15)
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--no-daemon", action="store_true")
    a = ap.parse_args(argv)
    if a.devices:
        from .stt import list_devices
        for d in list_devices():
            print(f"{d['id']}\t{d['desc']}")
        return 0
    if a.setup:
        return setup(a)
    be = None
    if not a.no_daemon:
        try:
            from .client import DaemonBackend
            be = DaemonBackend()
        except OSError:
            be = None
    if be is None:
        from .stt import Listener, read_wav16k

        class _L:
            L = Listener(None)

            def stt(self, wav):
                return self.L.transcribe(read_wav16k(wav))

            def listen(self, action=None, timeout=15):
                return (self.L.ptt_start() if action == "start" else self.L.ptt_stop() if action == "stop"
                        else self.L.listen_once(timeout))
        be = _L()
    if a.wav:
        r = be.stt(a.wav)
    elif a.ptt:
        input("[Enter] talk ")
        be.listen("start")
        input("[Enter] done ")
        r = be.listen("stop")
    else:
        r = be.listen(timeout=a.timeout)
    print(json.dumps(r) if a.json else r.get("text", ""))
    return 0


if __name__ == "__main__":
    sys.exit(main())
