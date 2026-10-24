"""Talk to lillith-voiced, or fall back to an in-process engine (one-shot)."""
from . import netguard  # noqa: F401

import os

from .core import socket_path
from .transport import Conn, pipe_name


class DaemonBackend:
    name = "daemon"

    def __init__(self, path=None, timeout=1.0):
        # unix socket on Linux/macOS; on Windows the named pipe \\.\pipe\lillith-voice (same JSON lines,
        # with a short retry while all pipe instances are busy). Raises OSError if no daemon is running.
        self.path = path or (pipe_name() if os.name == "nt" else socket_path())
        self.conn = Conn(self.path, timeout)

    def call(self, **req):
        rep = self.conn.call(**req)
        if not rep.get("ok"):
            raise RuntimeError(rep.get("error", "daemon error"))
        return rep

    def say(self, text, wait=True, **kw):
        return self.call(cmd="say", text=text, wait=wait, **kw)

    def file(self, text, out, **kw):
        return self.call(cmd="file", text=text, out=os.path.abspath(out), **kw)

    def stop(self):
        return self.call(cmd="stop")

    def ping(self):
        return self.call(cmd="ping")

    def listen(self, action=None, timeout=15):
        return self.call(cmd="listen", **({"action": action} if action else {"timeout": timeout}))

    def stt(self, wav):
        return self.call(cmd="stt", wav=os.path.abspath(wav))


class LocalBackend:
    name = "one-shot"

    def __init__(self, threads=None, preset=None):
        from .speaker import Speaker
        self.sp = Speaker(threads=threads, preload=preset)
        self._last = None
        self._listener = None

    @property
    def listener(self):
        if self._listener is None:
            from .stt import Listener
            self._listener = Listener(self.sp)
        return self._listener

    def listen(self, action=None, timeout=15):
        if action == "start":
            return self.listener.ptt_start()
        if action == "stop":
            return self.listener.ptt_stop()
        return self.listener.listen_once(timeout)

    def stt(self, wav):
        from .stt import read_wav16k
        return self.listener.transcribe(read_wav16k(wav))

    def _job(self, kind, text, out=None, **kw):
        from .speaker import Job
        rate = kw.get("rate")
        return self.sp.submit(Job(kind, text, kw.get("preset"), kw.get("speed"), kw.get("pitch"),
                                  kw.get("fx", True), out, int(rate) if rate else None,
                                  kw.get("actions", "drop")))

    def say(self, text, wait=True, **kw):
        j = self._job("say", text, **kw)
        self._last = j
        if not wait:
            return {"ok": True, "queued": True}
        j.done.wait()
        if j.error:
            raise RuntimeError(j.error)
        return {"ok": True, **j.stats, "load_s": round(self.sp.voice.load_s, 2)}

    def file(self, text, out, **kw):
        j = self._job("file", text, os.path.abspath(out), **kw)
        j.done.wait()
        if j.error:
            raise RuntimeError(j.error)
        return {"ok": True, **j.stats, "load_s": round(self.sp.voice.load_s, 2)}

    def stop(self):
        self.sp.stop()
        return {"ok": True}

    def ping(self):
        return {"ok": True, "mode": "one-shot"}


def connect(mode="auto", threads=None, preset=None):
    """mode: auto (daemon if running else in-process) | daemon | local"""
    if mode in ("auto", "daemon"):
        try:
            return DaemonBackend()
        except OSError:
            if mode == "daemon":
                raise SystemExit(f"lillith-say: daemon not running ({socket_path()}). "
                                 "Start it: systemctl --user start lillith-voice  (or: lillith-voiced &)")
    return LocalBackend(threads=threads, preset=preset)
