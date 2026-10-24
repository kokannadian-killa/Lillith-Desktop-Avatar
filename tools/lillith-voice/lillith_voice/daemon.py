"""lillith-voiced: keeps the voice (and, on demand, the STT) loaded and serves JSON lines locally.

Transport: a unix socket on Linux/macOS (no TCP at all); on Windows the named pipe \\\\.\\pipe\\lillith-voice
(same JSON lines). Full contract: PROTOCOL.md.
"""
from . import netguard  # noqa: F401

import argparse
import json
import os
import signal
import socket
import sys
import threading
import time

from .core import socket_path, PRESETS, SR, get_preset, pocket_available
from .speaker import Speaker, Job
from .transport import TOPICS, Hub, serve_lines, UnixServer, PipeServer, pipe_name, PIPE_NAME  # noqa: F401

PROTOCOL_VERSION = 2
PROTOCOL = """\
One JSON object per line, one JSON reply per line. "cmd" and "op" are synonyms.
  {"cmd":"ping"}
  {"cmd":"say","text":"...","wait":true,"preset":"lillith"}      -> {"ok":true,"utt":N,...} when playback ends
  {"cmd":"file","text":"...","out":"/abs/path.wav"}
  {"cmd":"stop"}            (works from a second connection while a say is waiting)
  {"cmd":"listen","action":"start"|"stop"}   push-to-talk -> {"ok":true,"text":"...","safeword":false,...}
  {"cmd":"listen"}          one utterance, ends on silence (VAD)
  {"cmd":"listen_mode","on":true|false}      always-listening; transcripts arrive as events
  Mic gate: starting the mic needs Ko's mouse-click Yes on a Soul Contract card, on EVERY push-to-talk
  press / listen and every always-listening switch-on (no session grant). Either pass "token" from
  {"cmd":"mic_card","kind":"listen"|"always"} (one-time, 120 s) or let the daemon show the card
  inline. No / timeout / no card UI -> {"ok":false,"need_card":true,...}, mic stays off.
  {"cmd":"mic_revoke"}      drop unused mic tokens
  {"cmd":"stt","wav":"/abs/file.wav"}        transcribe a file
  {"op":"subscribe","topics":["lipsync","stt"]}   this connection then only receives events
  {"cmd":"shutdown"}
Reply: {"ok":true, ...} or {"ok":false,"error":"..."}"""

class Core:
    """Transport-independent request handling (shared by the unix socket and the Windows pipe)."""

    def __init__(self, speaker, hub, stt_threads=2):
        self.speaker, self.hub = speaker, hub
        self.started = time.time()
        self._listener = None
        self._llock = threading.Lock()
        self.stt_threads = stt_threads
        from .micgate import MicGate
        from .stt import mic_argv
        self.gate = MicGate(mic_argv=mic_argv)   # shared by the listener; works before whisper is loaded

    @property
    def listener(self):
        with self._llock:
            if self._listener is None:
                from .stt import Listener
                self._listener = Listener(self.speaker, self.hub.emit, threads=self.stt_threads,
                                          gate=self.gate)
            return self._listener

    def dispatch(self, r):
        cmd = r.get("cmd") or r.get("op") or "say"
        sp = self.speaker
        if cmd == "ping":
            return {"ok": True, "pid": os.getpid(), "protocol": PROTOCOL_VERSION, "presets": list(PRESETS),
                    "default": get_preset(None).name, "cloned_voice": pocket_available(),
                    "threads": sp.voice.threads, "load_s": round(sp.voice.load_s, 2),
                    "uptime_s": round(time.time() - self.started, 1), "speaking": sp.busy(),
                    "stt_loaded": self._listener is not None,
                    "listening_always": bool(self._listener and self._listener.always)}
        if cmd == "stop":
            sp.stop(reason=r.get("reason", "stop"))
            return {"ok": True}
        if cmd == "shutdown":
            sp.stop()
            threading.Timer(0.2, os.kill, (os.getpid(), signal.SIGTERM)).start()
            return {"ok": True}
        if cmd in ("listen", "stt", "listen_mode"):
            return self._stt(cmd, r)
        if cmd == "mic_card":
            return self.gate.card(r.get("kind", "listen"))
        if cmd == "mic_revoke":
            return self.gate.revoke()
        if cmd not in ("say", "file"):
            return {"ok": False, "error": f"unknown cmd {cmd!r}"}
        text = r.get("text", "")
        out = r.get("out")
        if cmd == "file" and (not out or not os.path.isabs(out)):
            return {"ok": False, "error": "file needs an absolute 'out' path"}
        rate = r.get("rate")
        job = sp.submit(Job(cmd, text, r.get("preset"), r.get("speed"), r.get("pitch"), r.get("fx", True),
                            out, int(rate) if rate else None, r.get("actions", "drop")))
        if cmd == "say" and not r.get("wait", True):
            return {"ok": True, "queued": True, "utt": job.utt}
        job.done.wait()
        if job.error:
            return {"ok": False, "error": job.error, "utt": job.utt}
        return {"ok": True, "utt": job.utt, **job.stats}

    def _stt(self, cmd, r):
        L = self.listener
        if cmd == "stt":
            wav = r.get("wav")
            if not wav or not os.path.isabs(wav):
                return {"ok": False, "error": "stt needs an absolute 'wav' path"}
            from .stt import read_wav16k
            return L.transcribe(read_wav16k(wav))
        if cmd == "listen_mode":
            return L.set_always(bool(r.get("on", True)), token=r.get("token"))
        act = r.get("action")
        if act == "start":
            return L.ptt_start(token=r.get("token"))
        if act == "stop":
            return L.ptt_stop()
        return L.listen_once(float(r.get("timeout", 15)), token=r.get("token"))


# Transports (unix socket / Windows named pipe, stdlib only) live in transport.py.
Server = UnixServer  # backwards-compatible name


def main(argv=None):
    ap = argparse.ArgumentParser(prog="lillith-voiced", description="Lillith voice daemon (local socket / pipe only).",
                                 epilog=PROTOCOL, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--socket", default=None if os.name == "nt" else socket_path())
    ap.add_argument("--threads", type=int, default=None, help="ONNX threads (default: min(4, cores))")
    ap.add_argument("--stt-threads", type=int, default=2)
    ap.add_argument("--no-warmup", action="store_true")
    ap.add_argument("--stt-preload", action="store_true", help="load whisper + VAD at start (else on first use)")
    a = ap.parse_args(argv)

    hub = Hub()
    path = a.socket
    pipe = None
    if os.name == "nt":
        pipe = PipeServer(None, a.socket or pipe_name())
        try:
            pipe.bind()  # first instance now: refuses to start if the name is already owned (running / squatted)
        except OSError as e:
            print(f"lillith-voiced: {e}", file=sys.stderr)
            return 0 if getattr(e, "errno", None) in (5, 231) else 1
    elif os.path.exists(path):
        s = socket.socket(socket.AF_UNIX)
        try:
            s.connect(path)
            print(f"lillith-voiced already running on {path}", file=sys.stderr)
            return 0
        except OSError:
            os.unlink(path)  # stale
        finally:
            s.close()
    speaker = Speaker(threads=a.threads, emit=hub.emit)
    if not a.no_warmup:
        speaker.warmup()
    core = Core(speaker, hub, a.stt_threads)
    if a.stt_preload:
        _ = core.listener
    msg = (f"(model load {speaker.voice.load_s:.1f}s, {speaker.voice.threads} threads, default preset "
           f"{get_preset(None).name}, player={speaker.player.cmd and speaker.player.cmd[0]})")
    if pipe is not None:
        pipe.core = core
        print(f"lillith-voiced ready on {pipe.name} {msg}", file=sys.stderr, flush=True)
        t = threading.Thread(target=pipe.serve_forever, daemon=True, name="lillith-pipe-accept")
        t.start()
        try:
            while t.is_alive():  # keep the main thread interruptible (Ctrl+C) on Windows
                t.join(0.5)
        except KeyboardInterrupt:
            pass
        pipe.close()
        os._exit(0)
    srv = UnixServer(path, core)  # same as before: 0700 dir, socket created under umask 0177 (0600)

    def bye(*_):
        try:
            os.unlink(path)
        except OSError:
            pass
        os._exit(0)
    signal.signal(signal.SIGTERM, bye)
    signal.signal(signal.SIGINT, bye)
    print(f"lillith-voiced ready on {path} {msg}", file=sys.stderr, flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    sys.exit(main())
