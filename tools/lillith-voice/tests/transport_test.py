"""Transport test (stdlib only; no models needed): runs the real transport (unix socket on Linux/macOS,
the ctypes named pipe on Windows) against a fake core and checks framing + concurrency:
  ping, a lipsync subscriber receiving events while a `say` is running, `stop` from a second connection
  interrupting a waiting `say`, errors on bad JSON, many clients at once, and (Windows) the security setup.
Usage:  python tests/transport_test.py        exit 0 = all passed
"""
import os
import sys
import tempfile
import threading
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from lillith_voice import transport as T  # noqa: E402

FAILS = []


def check(name, cond, detail=""):
    print(("PASS " if cond else "FAIL ") + name + (f"  ({detail})" if detail and not cond else ""))
    if not cond:
        FAILS.append(name)


class FakeCore:
    def __init__(self):
        self.hub = T.Hub()
        self.stop_ev = threading.Event()
        self.utt = 0

    def dispatch(self, r):
        cmd = r.get("cmd") or r.get("op")
        if cmd == "ping":
            return {"ok": True, "pong": True}
        if cmd == "stop":
            self.stop_ev.set()
            self.hub.emit({"ev": "stop"})
            return {"ok": True, "stopped": True}
        if cmd == "say":
            self.utt += 1
            u = self.utt
            self.stop_ev.clear()
            for i in range(int(r.get("frames", 200))):  # ~10 ms per "envelope" frame
                if self.stop_ev.is_set():
                    self.hub.emit({"ev": "utt_end", "utt": u, "interrupted": True})
                    return {"ok": True, "utt": u, "interrupted": True}
                self.hub.emit({"ev": "envelope", "utt": u, "i": i, "v": 0.5})
                time.sleep(0.01)
            self.hub.emit({"ev": "utt_end", "utt": u})
            return {"ok": True, "utt": u, "interrupted": False, "text": r.get("text")}
        if cmd == "echo":
            return {"ok": True, "text": r.get("text")}
        raise ValueError(f"unknown cmd {cmd!r}")


def pipe_sddl(name):
    """Read the DACL of the live pipe back via GetSecurityInfo on a client handle (Windows only)."""
    import ctypes
    from ctypes import wintypes as w
    import msvcrt
    a = ctypes.WinDLL("advapi32", use_last_error=True)
    k = ctypes.WinDLL("kernel32", use_last_error=True)
    a.GetSecurityInfo.argtypes = [w.HANDLE, ctypes.c_int, w.DWORD] + [ctypes.c_void_p] * 4 + [
        ctypes.POINTER(ctypes.c_void_p)]
    a.ConvertSecurityDescriptorToStringSecurityDescriptorW.argtypes = [
        ctypes.c_void_p, w.DWORD, w.DWORD, ctypes.POINTER(w.LPWSTR), ctypes.c_void_p]
    k.LocalFree.argtypes = [ctypes.c_void_p]
    f = open(name, "r+b", buffering=0)
    try:
        sd = ctypes.c_void_p()
        rc = a.GetSecurityInfo(msvcrt.get_osfhandle(f.fileno()), 1, 4, None, None, None, None, ctypes.byref(sd))
        if rc:
            return f"GetSecurityInfo error {rc}"
        out = w.LPWSTR()
        if not a.ConvertSecurityDescriptorToStringSecurityDescriptorW(sd, 1, 4, ctypes.byref(out), None):
            return f"convert error {ctypes.get_last_error()}"
        val = out.value
        k.LocalFree(out)
        k.LocalFree(sd)
        return val
    finally:
        f.close()


def start_server(core):
    if os.name == "nt":
        name = rf"\\.\pipe\lillith-voice-test-{os.getpid()}"
        srv = T.PipeServer(core, name)
        srv.bind()
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        return srv, name, srv.close
    d = tempfile.mkdtemp(prefix="lv-transport-")
    path = os.path.join(d, "voiced.sock")
    srv = T.UnixServer(path, core)
    threading.Thread(target=srv.serve_forever, daemon=True).start()

    def close():
        srv.shutdown()
        srv.server_close()
        os.unlink(path)
        os.rmdir(d)
    return srv, path, close


def main():
    core = FakeCore()
    srv, addr, close = start_server(core)
    print(f"transport: {'named pipe' if os.name == 'nt' else 'unix socket'} {addr}")

    if os.name != "nt":
        import stat
        check("socket mode 0600", stat.S_IMODE(os.stat(addr).st_mode) == 0o600, oct(os.stat(addr).st_mode))
        check("socket dir mode 0700", stat.S_IMODE(os.stat(os.path.dirname(addr)).st_mode) == 0o700)
    else:
        sid = srv.sddl.split("(A;;GA;;;")[1].rstrip(")")
        check("DACL is protected, user-only", srv.sddl.startswith("D:P(D;;GA;;;NU)(A;;GA;;;S-1-") and
              srv.sddl.count("(A;") == 1, srv.sddl)
        print("   sddl:", srv.sddl, " user sid:", sid)
        applied = pipe_sddl(addr)
        print("   DACL read back from the live pipe:", applied)
        check("live pipe DACL = current user only (+deny network)",
              applied is not None and f"(A;;FA;;;{sid})" in applied.replace("GA", "FA") and applied.count("(A;") == 1,
              str(applied))
        try:  # a second server on the same name must refuse (FILE_FLAG_FIRST_PIPE_INSTANCE + liveness probe)
            s2 = T.PipeServer(core, addr)
            s2.bind()
            s2.close()
            check("first-instance guard refuses a 2nd server", False, "second bind succeeded")
        except OSError as e:
            check("first-instance guard refuses a 2nd server", True)
            print("   2nd server:", e)

    c1 = T.Conn(addr)
    check("ping", c1.call(cmd="ping") == {"ok": True, "pong": True})
    check("op synonym", c1.call(op="ping").get("pong") is True)
    big = "ä" * 100_000 + " 💋"
    check("utf-8 + 100k line round-trip", c1.call(cmd="echo", text=big).get("text") == big)
    c1.send({"cmd": "echo", "text": "a"})
    c1.send({"cmd": "echo", "text": "b"})
    check("pipelined requests answered in order", [c1.recv()["text"], c1.recv()["text"]] == ["a", "b"])
    c1._w(b"not json\n")
    c1._flush()
    r = c1.recv()
    check("bad JSON -> ok:false, connection stays up", r.get("ok") is False and c1.call(cmd="ping")["ok"])
    c1._w(b"\n\n")
    c1._flush()
    check("blank lines ignored", c1.call(cmd="ping")["ok"])

    # subscriber alongside a say; stop from a 3rd connection while say waits
    sub = T.Conn(addr)
    check("subscribe ack", sub.call(op="subscribe", topics=["lipsync"]).get("subscribed") == ["lipsync"])
    got = []

    def pump():
        while True:
            ev = sub.recv()
            if ev is None:
                return
            got.append(ev)
            if ev.get("ev") == "utt_end":
                return
    pt = threading.Thread(target=pump, daemon=True)
    pt.start()
    res = {}
    t0 = time.monotonic()
    st = threading.Thread(target=lambda: res.update(c1.call(cmd="say", text="hi", frames=500)), daemon=True)
    st.start()
    time.sleep(0.4)
    c3 = T.Conn(addr)
    check("stop on a 2nd connection while say waits", c3.call(cmd="stop").get("stopped") is True)
    st.join(5)
    dt = time.monotonic() - t0
    check("waiting say returned interrupted (not after 5 s)", res.get("interrupted") is True and dt < 2.0,
          f"{res} after {dt:.2f}s")
    pt.join(3)
    env = [e for e in got if e.get("ev") == "envelope"]
    check("subscriber got envelope events during say", len(env) >= 10, f"{len(env)} envelopes")
    check("subscriber got stop + utt_end(interrupted)",
          any(e.get("ev") == "stop" for e in got) and got and got[-1].get("interrupted") is True,
          str(got[-3:]))

    # many concurrent clients
    errs = []

    def worker(i):
        try:
            c = T.Conn(addr, timeout=5)
            for j in range(20):
                if c.call(cmd="echo", text=f"{i}-{j}").get("text") != f"{i}-{j}":
                    errs.append((i, j))
            c.close()
        except Exception as e:
            errs.append((i, repr(e)))
    ws = [threading.Thread(target=worker, args=(i,)) for i in range(24)]
    [w.start() for w in ws]
    [w.join(20) for w in ws]
    check("24 concurrent clients x 20 requests", not errs, str(errs[:3]))

    # a client that disconnects mid-subscription must not break the server
    s2 = T.Conn(addr)
    s2.call(op="subscribe", topics=["all"])
    s2.close()
    c1.call(cmd="say", text="x", frames=5)
    check("server fine after subscriber hangs up", T.Conn(addr).call(cmd="ping")["ok"])

    for c in (c1, sub, c3):
        c.close()
    close()
    print("ALL PASSED" if not FAILS else f"{len(FAILS)} FAILED: {FAILS}")
    return 1 if FAILS else 0


if __name__ == "__main__":
    sys.exit(main())
