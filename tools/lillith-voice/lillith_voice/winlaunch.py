"""Windows launcher for the bundled voice daemon (installer payload, %LOCALAPPDATA%\\Lillith\\voice).

    pythonw.exe -m lillith_voice.winlaunch [daemon args]   start lillith-voiced on \\\\.\\pipe\\lillith-voice, hidden,
                                                          logging to <voice home>\\logs\\voiced.log
    python.exe  -m lillith_voice.winlaunch --ping          print the daemon's ping reply (exit 1 if not running)
    python.exe  -m lillith_voice.winlaunch --stop          ask a running daemon to shut down (exit 0 either way)
    pythonw.exe -m lillith_voice.winlaunch --start-app <exe> [args]
                                                          make sure the daemon is up (start it if needed), wait
                                                          until it answers ping (<= 60 s), then start <exe>

--start-app is what the installer's "Start Lillith when I sign in" shortcut uses when the voice is installed:
the app probes the daemon once at startup and falls back to SAPI if it is not there yet, so at sign-in it
must not race the HKCU Run entry.

The per-user autostart (HKCU Run "LillithVoice", written by windows/installer.nsi) runs the first form.
A second start is harmless: the pipe is created with FILE_FLAG_FIRST_PIPE_INSTANCE, so it exits at once.
No admin, no network: netguard is imported first, the only transport is the local named pipe.
"""
from . import netguard  # noqa: F401  must stay first

import json
import os
import sys
import time


def _log_to_file():
    from .core import VOICE_HOME
    d = os.path.join(VOICE_HOME, "logs")
    try:
        os.makedirs(d, exist_ok=True)
        path = os.path.join(d, "voiced.log")
        if os.path.exists(path) and os.path.getsize(path) > 2_000_000:
            os.replace(path, path + ".1")
        f = open(path, "a", buffering=1, encoding="utf-8", errors="replace")
    except OSError:
        return
    f.write(f"\n=== lillith-voiced start {time.strftime('%Y-%m-%d %H:%M:%S')} pid {os.getpid()} ===\n")
    # pythonw.exe has no console: sys.stdout / sys.stderr are None there
    if sys.stdout is None or os.path.basename(sys.executable).lower() == "pythonw.exe":
        sys.stdout = f
    sys.stderr = f


def _client(timeout=2.0):
    from .client import DaemonBackend
    return DaemonBackend(timeout=timeout)


def ping():
    try:
        print(json.dumps(_client().ping()))
        return 0
    except Exception as e:  # not running
        print(f"lillith-voiced not running ({e})", file=sys.stderr)
        return 1


def stop(wait_s=5.0):
    try:
        c = _client()
        c.call(cmd="shutdown")
    except Exception:
        return 0  # not running
    t = time.monotonic() + wait_s
    while time.monotonic() < t:
        time.sleep(0.2)
        try:
            _client(0.5).ping()
        except Exception:
            return 0
    print("lillith-voiced did not stop in time", file=sys.stderr)
    return 1


def _pinged(timeout):
    try:
        _client(timeout).ping()
        return True
    except Exception:
        return False


def start_app(cmd, wait_s=60.0):
    import subprocess
    from .core import NO_WINDOW
    if not cmd:
        print("usage: winlaunch --start-app <exe> [args]", file=sys.stderr)
        return 2
    t0 = time.monotonic()
    started = False
    while time.monotonic() - t0 < wait_s:
        if _pinged(30.0):  # the pipe exists while the models load; ping answers once loaded
            break
        if not started and time.monotonic() - t0 > 3.0:
            # not up 3 s in (Run entry disabled, or this is the portable copy): start it; a duplicate exits at once
            exe = sys.executable
            w = os.path.join(os.path.dirname(exe), "pythonw.exe")
            subprocess.Popen([w if os.path.exists(w) else exe, "-m", "lillith_voice.winlaunch"],
                             close_fds=True, **NO_WINDOW)
            started = True
        time.sleep(0.5)
    # start the app regardless: without the daemon it uses its SAPI fallback (and says so)
    flags = (0x00000008 | 0x00000200) if os.name == "nt" else 0  # DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    subprocess.Popen(cmd, close_fds=True, creationflags=flags, cwd=os.path.dirname(cmd[0]) or None)
    return 0


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv[:1] == ["--ping"]:
        return ping()
    if argv[:1] == ["--stop"]:
        return stop()
    if argv[:1] == ["--start-app"]:
        return start_app(argv[1:])
    _log_to_file()
    from .daemon import main as daemon_main
    return daemon_main(argv)


if __name__ == "__main__":
    sys.exit(main())
