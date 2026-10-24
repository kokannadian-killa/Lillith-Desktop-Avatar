"""Test-run guard: any program launch that does not go through
lillith_py.launch.run is recorded and fails the run (independent of the
runtime guard in lillith_py.launch)."""
from __future__ import annotations

import contextlib
import os
import subprocess

VIOLATIONS: list[str] = []
_saved: dict = {}


class BypassError(RuntimeError):
    pass


def _inside() -> bool:
    from lillith_py import launch
    return launch.inside_wrapper()


def _violation(what: str):
    VIOLATIONS.append(what)
    raise BypassError(f"program launch bypassed lillith_py.launch: {what}")


_OS_NAMES = ["system", "popen", "posix_spawn", "posix_spawnp", "fork", "forkpty",
             "spawnl", "spawnle", "spawnlp", "spawnlpe", "spawnv", "spawnve", "spawnvp", "spawnvpe",
             "execl", "execle", "execlp", "execlpe", "execv", "execve", "execvp", "execvpe"]


def install():
    if _saved:
        return
    base = subprocess.Popen
    _saved["Popen"] = base

    class TestGuardPopen(base):  # type: ignore[misc, valid-type]
        def __init__(self, args, *a, **kw):
            if not _inside():
                _violation(f"subprocess.Popen({args!r})")
            super().__init__(args, *a, **kw)

    subprocess.Popen = TestGuardPopen  # type: ignore[misc]
    for name in _OS_NAMES:
        fn = getattr(os, name, None)
        if fn is None:
            continue
        _saved["os." + name] = fn

        def wrapper(*a, _n=name, _f=fn, **kw):
            # subprocess.Popen may use os.posix_spawn internally; that is fine
            # only while lillith_py.launch.run is starting the program.
            if not _inside():
                _violation(f"os.{_n}{a!r}")
            return _f(*a, **kw)
        setattr(os, name, wrapper)


def uninstall():
    for k, v in list(_saved.items()):
        if k == "Popen":
            subprocess.Popen = v  # type: ignore[misc]
        else:
            setattr(os, k[3:], v)
    _saved.clear()


@contextlib.contextmanager
def expect_violation():
    """For the test that proves the guard works: swallow exactly the recorded
    violations made inside the block."""
    n = len(VIOLATIONS)
    try:
        yield
    finally:
        del VIOLATIONS[n:]


@contextlib.contextmanager
def suspended():
    """Let a direct launch reach the runtime guard (lillith_py.launch) underneath."""
    saved = dict(_saved)
    uninstall()
    try:
        yield
    finally:
        _saved.clear()
        install()
        assert set(saved) == set(_saved)
