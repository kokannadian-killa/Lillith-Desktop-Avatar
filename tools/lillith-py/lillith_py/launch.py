"""The ONE wrapper for every external program lillith-py starts.

Project decision (review X5): pdftotext, pdfinfo, ImageMagick (magick/convert),
tesseract and notify-send are EXEMPT from a Soul Contract card. They run
as before, but every launch goes through :func:`run`, which writes exactly
ONE append-only line per launch to Lillith's shared ``audit.log``.

Audit file and format: the Rust ones, not a new file.
  * Path: ``audit_log_path()`` in crates/lillith-core/src/config.rs, i.e.
    ``$XDG_DATA_HOME/lillith/audit.log``, else ``~/.local/share/lillith/audit.log``.
    ``$LILLITH_HOME`` is ignored here on purpose, so both sides share one file.
  * Line: the format of ``SoulContract::audit`` in crates/lillith-core/src/soul_contract.rs,
    ``<rfc3339 UTC>\\t<DECISION>\\t<kind>\\t<what>\\n`` (tabs and newlines in fields
    become spaces). For lillith-py:

        2026-10-24T10:14:03Z\tEXEMPT\tlillith-py:launch\tpdftotext $ pdftotext -layout -enc UTF-8 ~/.local/share/lillith/attachments/blobs/ab/<sha>.pdf - -> exit=0 [attachments: PDF text]

    DECISION is ``EXEMPT`` for a launch that ran (the outcome comes after ``->``:
    ``exit=N``, ``timeout``, ``error: ...``), and ``REFUSED`` for a launch that
    tried to bypass this wrapper (see below).

Masking: the home directory prefix becomes ``~``. A value after ``--api-key``,
``--token``, ``--password`` and the like (or ``--x=value``, ``key=value``,
``Bearer x``) becomes ``<redacted>``. A token-looking blob (24+ chars of
base64/hex with no ``/``) becomes ``<redacted>``. Any other argument longer
than 80 chars (a notification body, for example) becomes ``<N chars>``.

Nothing else in lillith_py may import ``subprocess`` or call ``os.system`` and the
like (a source scan in the tests enforces this). :func:`install_guard`, run when
``lillith_py`` is imported, refuses and audits any ``subprocess.Popen`` in the
process that did not come through :func:`run`.
"""
from __future__ import annotations

import contextvars
import os
import re
import shutil
import subprocess
import time
from pathlib import Path

COMPONENT = "lillith-py"
AUDIT_KIND = "lillith-py:launch"
REASON_BYPASS = "bypassed the lillith_py.launch wrapper"

_INSIDE: contextvars.ContextVar[bool] = contextvars.ContextVar("lillith_py_launch", default=False)

_SECRET_FLAG = re.compile(r"^-{1,2}[\w-]*(api[-_]?key|token|secret|passw(or)?d|auth|bearer|cookie)[\w-]*$",
                          re.I)
_SECRET_EQ = re.compile(r"^(-{0,2}[\w-]*(api[-_]?key|token|secret|passw(or)?d|auth|cookie)[\w-]*=).+$",
                        re.I)
_BEARER = re.compile(r"(?i)\b(bearer|basic)\s+\S+")
_TOKENISH = re.compile(r"^[A-Za-z0-9+_=.\-]{24,}$")


# Re-exported so callers never need to import subprocess themselves.
TimeoutExpired = subprocess.TimeoutExpired
SubprocessError = subprocess.SubprocessError


class LaunchRefused(PermissionError):
    """A subprocess was started without going through run() (refused, audited)."""

    def __init__(self, program: str, reason: str = REASON_BYPASS):
        super().__init__(f"{program}: launch refused ({reason})")
        self.program, self.reason = program, reason


# ---------- audit ----------
def audit_log_path() -> Path:
    """Same file as the Rust ``config::audit_log_path()``."""
    base = os.environ.get("XDG_DATA_HOME") or str(Path.home() / ".local/share")
    return Path(base) / "lillith" / "audit.log"


def _clean(s) -> str:
    return str(s).replace("\t", " ").replace("\r", " ").replace("\n", " ")


def _mask_home(a: str) -> str:
    home = str(Path.home()).rstrip("/")
    if home and home != "/" and (a == home or home + "/" in a):
        a = a.replace(home + "/", "~/")
        a = "~" if a == home else a
    return a


def mask_argv(argv: list[str], max_arg: int = 80) -> list[str]:
    out, hide_next = [], False
    for i, raw in enumerate(argv):
        a = os.fsdecode(raw) if isinstance(raw, (bytes, os.PathLike)) else str(raw)
        if i == 0:
            out.append(os.path.basename(a))
            continue
        if hide_next:
            out.append("<redacted>")
            hide_next = False
            continue
        if _SECRET_FLAG.match(a):
            out.append(a)
            hide_next = True
            continue
        m = _SECRET_EQ.match(a)
        if m:
            out.append(m.group(1) + "<redacted>")
            continue
        a = _BEARER.sub(lambda m: m.group(1) + " <redacted>", a)
        if "/" not in a and _TOKENISH.match(a):
            out.append("<redacted>")
            continue
        a = _mask_home(a)
        out.append(a if len(a) <= max_arg else f"<{len(a)} chars>")
    return out


def audit(decision: str, argv: list[str], outcome: str, purpose: str = "") -> None:
    """Append one Rust-format line. A logging failure never blocks the launch."""
    ts = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    masked = mask_argv(list(argv) or ["?"])
    what = f"{masked[0]} $ {' '.join(masked)} -> {outcome}"
    if purpose:
        what += f" [{purpose}]"
    what = what if len(what) <= 1000 else what[:1000] + "…"
    line = f"{ts}\t{_clean(decision)}\t{AUDIT_KIND}\t{_clean(what)}\n"
    try:
        p = audit_log_path()
        p.parent.mkdir(parents=True, exist_ok=True)
        with open(p, "a", encoding="utf-8") as f:
            f.write(line)
    except OSError:
        pass


# ---------- the wrapper ----------
def which(name: str) -> str | None:
    """PATH lookup only; starts nothing."""
    return shutil.which(name)


def run(argv: list, *, purpose: str = "", **kwargs) -> subprocess.CompletedProcess:
    """subprocess.run(argv, **kwargs) plus exactly one audit line with the outcome.
    Exceptions (TimeoutExpired, OSError, ...) are audited and re-raised unchanged."""
    argv = [os.fspath(a) if isinstance(a, os.PathLike) else a for a in argv]
    token = _INSIDE.set(True)
    try:
        r = subprocess.run(argv, **kwargs)
    except subprocess.TimeoutExpired:
        audit("EXEMPT", argv, "timeout", purpose)
        raise
    except BaseException as e:
        audit("EXEMPT", argv, f"error: {type(e).__name__}: {e}"[:200], purpose)
        raise
    finally:
        _INSIDE.reset(token)
    audit("EXEMPT", argv, f"exit={r.returncode}", purpose)
    return r


def inside_wrapper() -> bool:
    return _INSIDE.get()


# ---------- runtime guard against bypasses ----------
_ORIG_POPEN = None


def install_guard() -> None:
    """Wrap subprocess.Popen in this process so a launch that skipped run() is
    refused and audited. Idempotent."""
    global _ORIG_POPEN
    if _ORIG_POPEN is not None:
        return
    _ORIG_POPEN = subprocess.Popen

    class GuardedPopen(_ORIG_POPEN):  # type: ignore[misc, valid-type]
        def __init__(self, args, *a, **kw):
            if not _INSIDE.get():
                argv = [args] if isinstance(args, (str, bytes, os.PathLike)) else list(args)
                argv = argv or ["?"]
                prog = os.path.basename(str(os.fsdecode(argv[0]) if isinstance(argv[0], (bytes, os.PathLike))
                                            else argv[0]).split(" ")[0])
                audit("REFUSED", argv, REASON_BYPASS)
                raise LaunchRefused(prog)
            super().__init__(args, *a, **kw)

    subprocess.Popen = GuardedPopen  # type: ignore[misc]


def uninstall_guard() -> None:
    global _ORIG_POPEN
    if _ORIG_POPEN is not None:
        subprocess.Popen = _ORIG_POPEN  # type: ignore[misc]
        _ORIG_POPEN = None
