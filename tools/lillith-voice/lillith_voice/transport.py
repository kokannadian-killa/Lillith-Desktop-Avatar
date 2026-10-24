"""Local-only transports for lillith-voiced: one JSON object per line, both directions.

- Linux/macOS: a unix stream socket (mode 0600, in a 0700 runtime dir). No TCP anywhere.
- Windows: the named pipe  \\\\.\\pipe\\lillith-voice  (override: LILLITH_VOICE_PIPE), stdlib ctypes only:
  byte mode (not message mode), duplex, unlimited instances (one per client, each served on its own thread),
  PIPE_REJECT_REMOTE_CLIENTS, FILE_FLAG_FIRST_PIPE_INSTANCE (refuses to start if someone already owns the
  name, so nobody can squat it in front of us), and a DACL that allows only the current user
  (protected, no inherited ACEs; network logons explicitly denied).

The request handling (serve_lines + Hub) is transport-independent and has no third-party imports, so it can be
tested on its own (tests/transport_test.py) on Linux and on Windows.
"""
import json
import os
import queue
import socketserver
import threading
import time

PIPE_NAME = r"\\.\pipe\lillith-voice"
MAX_LINE = 1 << 20  # 1 MiB per request line; longer lines are rejected and the connection closed

TOPICS = {"envelope": "lipsync", "visemes": "lipsync", "chunk_start": "lipsync", "chunk_end": "lipsync",
          "utt_end": "lipsync", "stop": "lipsync", "transcript": "stt", "listening": "stt", "barge_in": "stt",
          "safeword": "stt", "listen_mode": "stt"}


def pipe_name() -> str:
    return os.environ.get("LILLITH_VOICE_PIPE") or PIPE_NAME


# --------------------------------------------------------------------------- events
class Hub:
    """Fan-out of events to subscribed connections (bounded queues; slow clients drop oldest)."""

    def __init__(self):
        self.subs = []
        self.lock = threading.Lock()

    def add(self, topics):
        q = queue.Queue(maxsize=2000)
        with self.lock:
            self.subs.append((set(topics), q))
        return q

    def remove(self, q):
        with self.lock:
            self.subs = [s for s in self.subs if s[1] is not q]

    def emit(self, ev):
        topic = TOPICS.get(ev.get("ev"), "misc")
        with self.lock:
            subs = list(self.subs)
        for topics, q in subs:
            if topic in topics or "all" in topics or ev.get("ev") in topics:
                try:
                    q.put_nowait(ev)
                except queue.Full:
                    try:
                        q.get_nowait()
                        q.put_nowait(ev)
                    except Exception:
                        pass


def serve_lines(core, lines, write):
    """One connection: JSON lines in, JSON lines out; 'subscribe' turns it into an event stream.
    `core` needs .hub (Hub) and .dispatch(dict) -> dict; `lines` yields bytes lines; `write(obj)` sends one line."""
    for line in lines:
        if not line.strip():
            continue
        try:
            req = json.loads(line)
            if not isinstance(req, dict):
                raise ValueError("request must be a JSON object")
            if (req.get("op") or req.get("cmd")) == "subscribe":
                topics = req.get("topics") or ["lipsync"]
                q = core.hub.add(topics)
                try:
                    write({"ok": True, "subscribed": topics})
                    while True:
                        write(q.get())
                except (OSError, ValueError):
                    return
                finally:
                    core.hub.remove(q)
            rep = core.dispatch(req)
        except Exception as e:
            rep = {"ok": False, "error": f"{type(e).__name__}: {e}"}
        write(rep)


def _locked_writer(send):
    lock = threading.Lock()

    def write(obj):
        data = (json.dumps(obj) + "\n").encode()
        with lock:
            send(data)
    return write


# --------------------------------------------------------------------------- unix socket
class _UnixHandler(socketserver.StreamRequestHandler):
    def handle(self):
        def send(data):
            self.wfile.write(data)
            self.wfile.flush()

        def lines():
            while True:
                ln = self.rfile.readline(MAX_LINE + 1)
                if not ln or len(ln) > MAX_LINE:
                    return
                yield ln
        try:
            serve_lines(self.server.core, lines(), _locked_writer(send))
        except (BrokenPipeError, ConnectionResetError):
            pass


if hasattr(socketserver, "UnixStreamServer"):  # not on Windows (no AF_UNIX server class there)
    class UnixServer(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
        daemon_threads = True
        request_queue_size = 64  # listen backlog (socketserver default 5 -> EAGAIN when many clients connect at once)

        def __init__(self, path, core):
            self.core = core
            d = os.path.dirname(path)
            if d:
                os.makedirs(d, mode=0o700, exist_ok=True)
            old = os.umask(0o177)  # socket file 0600
            try:
                super().__init__(path, _UnixHandler)
            finally:
                os.umask(old)
else:  # pragma: no cover
    UnixServer = None


# --------------------------------------------------------------------------- Windows named pipe (ctypes)
class PipeServer:
    """Windows named-pipe server, stdlib only. serve_forever() blocks; close() stops accepting."""

    def __init__(self, core, name=None):
        import ctypes
        from ctypes import wintypes as w
        self.core, self.name = core, name or pipe_name()
        self.ct, self.w = ctypes, w
        k = ctypes.WinDLL("kernel32", use_last_error=True)
        a = ctypes.WinDLL("advapi32", use_last_error=True)
        self.k, self.a = k, a
        H = w.HANDLE
        k.CreateNamedPipeW.restype = H
        k.CreateNamedPipeW.argtypes = [w.LPCWSTR, w.DWORD, w.DWORD, w.DWORD, w.DWORD, w.DWORD, w.DWORD, ctypes.c_void_p]
        k.ConnectNamedPipe.argtypes = [H, ctypes.c_void_p]
        k.ReadFile.argtypes = [H, ctypes.c_void_p, w.DWORD, ctypes.POINTER(w.DWORD), ctypes.c_void_p]
        k.WriteFile.argtypes = [H, ctypes.c_void_p, w.DWORD, ctypes.POINTER(w.DWORD), ctypes.c_void_p]
        k.FlushFileBuffers.argtypes = [H]
        k.DisconnectNamedPipe.argtypes = [H]
        k.CloseHandle.argtypes = [H]
        k.GetCurrentProcess.restype = H
        k.LocalFree.argtypes = [ctypes.c_void_p]
        a.OpenProcessToken.argtypes = [H, w.DWORD, ctypes.POINTER(H)]
        a.GetTokenInformation.argtypes = [H, ctypes.c_int, ctypes.c_void_p, w.DWORD, ctypes.POINTER(w.DWORD)]
        a.ConvertSidToStringSidW.argtypes = [ctypes.c_void_p, ctypes.POINTER(w.LPWSTR)]
        a.ConvertStringSecurityDescriptorToSecurityDescriptorW.argtypes = [
            w.LPCWSTR, w.DWORD, ctypes.POINTER(ctypes.c_void_p), ctypes.POINTER(w.DWORD)]

        class SA(ctypes.Structure):
            _fields_ = [("nLength", w.DWORD), ("lpSecurityDescriptor", ctypes.c_void_p), ("bInheritHandle", w.BOOL)]
        self.sddl = f"D:P(D;;GA;;;NU)(A;;GA;;;{self._user_sid()})"   # deny network logons; allow only me
        psd = ctypes.c_void_p()
        if not a.ConvertStringSecurityDescriptorToSecurityDescriptorW(self.sddl, 1, ctypes.byref(psd), None):
            raise self._err("ConvertStringSecurityDescriptorToSecurityDescriptorW")
        self._psd = psd  # kept alive for the server's lifetime
        self.sa = SA(ctypes.sizeof(SA), psd, False)
        self._first = True
        self._closed = False
        self._pending = None

    def _err(self, what):
        e = self.ct.get_last_error()
        return OSError(e, f"{what} failed: {self.ct.FormatError(e).strip()}")

    def _user_sid(self):
        ct, w, k, a = self.ct, self.w, self.k, self.a
        tok = w.HANDLE()
        if not a.OpenProcessToken(k.GetCurrentProcess(), 0x0008, ct.byref(tok)):  # TOKEN_QUERY
            raise self._err("OpenProcessToken")
        try:
            n = w.DWORD()
            a.GetTokenInformation(tok, 1, None, 0, ct.byref(n))  # TokenUser
            buf = ct.create_string_buffer(n.value)
            if not a.GetTokenInformation(tok, 1, buf, n, ct.byref(n)):
                raise self._err("GetTokenInformation")
            psid = ct.c_void_p.from_buffer(buf).value  # TOKEN_USER.User.Sid
            s = w.LPWSTR()
            if not a.ConvertSidToStringSidW(psid, ct.byref(s)):
                raise self._err("ConvertSidToStringSidW")
            try:
                return s.value
            finally:
                k.LocalFree(s)
        finally:
            k.CloseHandle(tok)

    def _create(self):
        PIPE_ACCESS_DUPLEX, FIRST = 0x3, 0x00080000
        PIPE_TYPE_BYTE = PIPE_READMODE_BYTE = PIPE_WAIT = 0x0
        PIPE_REJECT_REMOTE_CLIENTS = 0x8
        self.ct.set_last_error(0)
        h = self.k.CreateNamedPipeW(
            self.name, PIPE_ACCESS_DUPLEX | (FIRST if self._first else 0),
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            255, 65536, 65536, 0, self.ct.byref(self.sa))
        if h is None or h == self.ct.c_void_p(-1).value:
            e = self.ct.get_last_error()
            if self._first and e in (5, 231):  # ACCESS_DENIED / PIPE_BUSY: the name is already taken
                raise OSError(e, f"{self.name} is already in use (another lillith-voiced?)")
            raise self._err("CreateNamedPipeW")
        if self._first and self.ct.get_last_error() == 183:
            # ERROR_ALREADY_EXISTS on "success": the flag was ignored (seen under wine); treat as taken
            self.k.CloseHandle(h)
            raise OSError(5, f"{self.name} is already in use (another lillith-voiced?)")
        self._first = False
        return h

    def bind(self):
        """Create the first instance now, so a second daemon (or a squatter) fails fast."""
        if self._pending is not None:
            return
        try:  # belt and braces next to FILE_FLAG_FIRST_PIPE_INSTANCE: is somebody already serving this name?
            open(self.name, "r+b", buffering=0).close()
            raise OSError(5, f"{self.name} is already in use (another lillith-voiced?)")
        except OSError as e:
            if getattr(e, "winerror", None) == 231:  # exists, all instances busy
                raise OSError(231, f"{self.name} is already in use (another lillith-voiced?)")
            if e.errno == 5 and getattr(e, "winerror", None) is None:
                raise
        self._pending = self._create()

    ACCEPTORS = 2  # listening instances kept ready, so a client never hits the gap while one is re-created

    def serve_forever(self):
        self.bind()
        self._serving = True
        first, self._pending = self._pending, None
        for _ in range(self.ACCEPTORS - 1):
            threading.Thread(target=self._accept_loop, args=(self._create(),), daemon=True,
                             name="lillith-pipe-accept").start()
        self._accept_loop(first)

    def _accept_loop(self, h):
        while True:
            ok = self.k.ConnectNamedPipe(h, None)
            if not ok and self.ct.get_last_error() != 535:  # ERROR_PIPE_CONNECTED is success
                self.k.CloseHandle(h)
                if self._closed:
                    return
                time.sleep(0.05)
            elif self._closed:
                self.k.DisconnectNamedPipe(h)
                self.k.CloseHandle(h)
                return
            else:
                threading.Thread(target=self._client, args=(h,), daemon=True, name="lillith-pipe").start()
            if self._closed:
                return
            h = self._create()

    def close(self):
        """Stop accepting. Connected clients keep their threads until they hang up."""
        if self._closed:
            return
        self._closed = True
        if getattr(self, "_serving", False):
            for _ in range(self.ACCEPTORS):  # wake the accept loops blocked in ConnectNamedPipe
                try:
                    open(self.name, "r+b", buffering=0).close()
                except OSError:
                    pass
        elif self._pending is not None:
            self.k.CloseHandle(self._pending)
            self._pending = None

    def _client(self, h):
        ct, w, k = self.ct, self.w, self.k
        buf = ct.create_string_buffer(65536)

        def lines():
            pend = b""
            while True:
                while b"\n" in pend:
                    ln, pend = pend.split(b"\n", 1)
                    yield ln + b"\n"
                if len(pend) > MAX_LINE:
                    return
                n = w.DWORD()
                if not k.ReadFile(h, buf, len(buf), ct.byref(n), None) or n.value == 0:
                    if ct.get_last_error() == 234 and n.value:  # ERROR_MORE_DATA (message mode only)
                        pass
                    else:
                        if pend.strip():
                            yield pend
                        return
                pend += buf.raw[:n.value]

        def send(data):
            mv = memoryview(data)
            while len(mv):
                n = w.DWORD()
                if not k.WriteFile(h, ct.c_char_p(bytes(mv[:65536])), min(len(mv), 65536), ct.byref(n), None):
                    raise self._err("WriteFile")  # client gone (ERROR_NO_DATA / BROKEN_PIPE) -> OSError
                mv = mv[n.value:]
        try:
            serve_lines(self.core, lines(), _locked_writer(send))
        except OSError:
            pass
        finally:
            k.FlushFileBuffers(h)
            k.DisconnectNamedPipe(h)
            k.CloseHandle(h)


# --------------------------------------------------------------------------- client side
class Conn:
    """A client connection with .call(**req) -> reply, .send(obj), .recv() -> obj|None, .close()."""

    def __init__(self, path=None, timeout=1.0):
        if os.name == "nt":
            name = path or pipe_name()
            t_start = time.monotonic()
            t_end = t_start + max(timeout, 0.3)
            while True:
                try:
                    raw = open(name, "r+b", buffering=0)  # CreateFileW on the pipe
                    break
                except OSError as e:
                    # 231 ERROR_PIPE_BUSY: all instances taken for a moment; the server makes a new one.
                    # ENOENT/EINVAL right after another client hung up: an instance is being recycled.
                    busy = getattr(e, "winerror", None) == 231
                    transient = e.errno in (2, 22) and time.monotonic() < t_start + 0.3
                    if (busy or transient) and time.monotonic() < t_end:
                        time.sleep(0.02)
                        continue
                    raise
            import io
            self._raw = raw
            self.r = io.BufferedReader(raw, 65536)
            self._w = raw.write
            self._flush = lambda: None
            return
        import socket
        from .core import socket_path
        self.s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.s.settimeout(timeout)
        self.s.connect(path or socket_path())  # raises if no daemon
        self.s.settimeout(None)
        f = self.s.makefile("rwb")
        self._raw, self.r, self._w, self._flush = f, f, f.write, f.flush

    def send(self, obj):
        data = (json.dumps(obj) + "\n").encode()
        while data:
            n = self._w(data)
            data = data[n:] if n else b""
        self._flush()

    def recv(self):
        line = self.r.readline()
        return json.loads(line) if line else None

    def call(self, **req):
        self.send(req)
        rep = self.recv()
        if rep is None:
            raise RuntimeError("daemon closed the connection")
        return rep

    def close(self):
        for f in (getattr(self, "r", None), self._raw, getattr(self, "s", None)):
            try:
                f and f.close()
            except Exception:
                pass
