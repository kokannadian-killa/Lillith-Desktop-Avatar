"""Launch guards for the voice daemon (stdlib only, no models).

* ``audit(...)``: one line per launch in Lillith's one audit log,
  ``<data>/lillith/audit.log`` (append mode), in the same column layout as the
  Rust ``lillith_core::launch_audit``::

      <utc>\t<TAG>\t<kind>\t<program>\t<card=<id>|no-card>\t$ <masked argv>

  Tabs / newlines are folded to spaces (no forged columns or lines) and
  secret-looking values (``--token x``, ``--api-key=x``) are masked.
* ``checked_exe(path)``: env/config-chosen programs must be an absolute path to
  an existing regular executable file.
* ``env_exe`` / ``env_command``: ``$LILLITH_FFMPEG`` / ``$LILLITH_PLAYER`` through
  that check (a command string is split with ``shlex``, never a shell).
* ``mic_env_command``: ``$LILLITH_MIC_CMD`` only as an allowlisted recorder with
  conservative arguments.
* ``sanitize_echo``: the presets.json ``echo`` value before it reaches an
  ffmpeg filter graph: numbers only, clamped ranges, anything else rejected.
"""
import os
import re
import shlex
import sys
import threading
import time

LAUNCH = "LAUNCH"
NO_CARD = "no-card"
_SECRET = ("api-key", "api_key", "apikey", "token", "password", "passwd", "secret")


def _log(msg):
    print(f"lillith-voice: {msg}", file=sys.stderr, flush=True)


def data_dir(env=None, windows=None):
    """Mirror of lillith_core::config::data_dir_from: $XDG_DATA_HOME, (Windows: %LOCALAPPDATA%), ~/.local/share."""
    env = os.environ if env is None else env
    windows = (os.name == "nt") if windows is None else windows
    base = env.get("XDG_DATA_HOME") or None
    if base is None and windows:
        base = env.get("LOCALAPPDATA") or None
    if base is None:
        home = env.get("HOME") or (None if env is not os.environ else os.path.expanduser("~"))
        if not home:
            return None
        base = os.path.join(home, ".local", "share")
    return os.path.join(base, "lillith")


def audit_log_path():
    d = data_dir()
    return os.path.join(d, "audit.log") if d else None


def one_column(s):
    return str(s).replace("\t", " ").replace("\r", " ").replace("\n", " ")


def _secret_flag(a):
    f = a.lstrip("-").lower()
    return a.startswith("-") and any(f.startswith(s) or f.endswith(s) for s in _SECRET)


def mask_argv(argv):
    out, mask_next = [], False
    for a in argv:
        a = str(a)
        if mask_next:
            out.append("***")
            mask_next = False
            continue
        flag, eq, _ = a.partition("=")
        if eq and _secret_flag(flag):
            out.append(f"{flag}=***")
        else:
            mask_next = _secret_flag(a)
            out.append(a)
    return out


def utc_now(t=None):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() if t is None else t))


def format_line(ts, tag, kind, argv, card=None):
    program = one_column(argv[0] if argv else "")
    card = f"card={one_column(card)}" if card else NO_CARD
    shown = one_column("$ " + " ".join(mask_argv(argv)))
    return f"{one_column(ts)}\t{one_column(tag)}\t{one_column(kind)}\t{program}\t{card}\t{shown}\n"


def audit(tag, kind, argv, card=None, log=None):
    """Appends one line; a failure is reported on stderr, never raised."""
    log = log or audit_log_path()
    if not log:
        return
    try:
        os.makedirs(os.path.dirname(log), exist_ok=True)
        with open(log, "a", encoding="utf-8") as f:
            f.write(format_line(utc_now(), tag, kind, list(argv), card))
    except OSError as e:
        _log(f"could not write audit log {log}: {e}")


_seen = set()
_seen_lock = threading.Lock()


def audit_once(kind, argv, card=None, log=None):
    """`audit(LAUNCH, ...)` once per process for each (kind, program): high-frequency launches."""
    key = (kind, str(argv[0]) if argv else "")
    with _seen_lock:
        if key in _seen:
            return
        _seen.add(key)
    audit(LAUNCH, kind, argv, card, log)


# --------------------------------------------------------------------------- programs
def checked_exe(path):
    """(path, None) when `path` is an absolute path to an existing regular executable file, else (None, why)."""
    if not path or not os.path.isabs(path):
        return None, "not an absolute path"
    if not os.path.exists(path):
        return None, "does not exist"
    if not os.path.isfile(path):
        return None, "not a regular file"
    if os.name == "nt":
        if not path.lower().endswith((".exe", ".com")):
            return None, "not an .exe"
    elif not os.access(path, os.X_OK):
        return None, "not executable"
    return path, None


def checked_or_log(what, path):
    p, why = checked_exe(path)
    if p is None:
        _log(f"{what} = {path!r} refused: {why} (needs an absolute path to an existing executable file); "
             f"using the default instead")
    return p


def env_exe(var, env=None):
    env = os.environ if env is None else env
    v = env.get(var)
    return checked_or_log(var, v) if v else None


def env_command(var, env=None):
    """A whole command in $var, split with shlex (no shell); its program must pass checked_exe.
    Returns the argv list, None when unset, or False when refused (logged)."""
    env = os.environ if env is None else env
    v = env.get(var)
    if not v:
        return None
    try:
        argv = shlex.split(v)
    except ValueError as e:
        _log(f"{var} refused: cannot parse it ({e}); using the default instead")
        return False
    if not argv or checked_or_log(var, argv[0]) is None:
        return False
    return argv


# --------------------------------------------------------------------------- microphone
MIC_ALLOWED = ("arecord", "parecord", "parec", "pw-record", "ffmpeg")
MIC_MAX_ARGS = 32
_MIC_ARG = re.compile(r"^[A-Za-z0-9_.,:=+@%/-]{1,128}$")
_FILE_EXT = re.compile(r"\.(wav|raw|pcm|mp3|ogg|oga|opus|flac|m4a|aac|txt|sh|py|json|conf|cfg|log)$", re.I)


def mic_arg_ok(a):
    """A recorder argument: a conservative charset, no URLs, no file names (paths only under /dev/)."""
    if not _MIC_ARG.match(a) or "://" in a or ".." in a:
        return False
    if "/" in a and not (a.startswith("/dev/") or a.split("=", 1)[-1].startswith("/dev/")):
        return False
    return not _FILE_EXT.search(a)


def check_mic_command(argv):
    """(argv, None) when the capture command is allowed, else (None, why)."""
    if not argv:
        return None, "empty"
    p, why = checked_exe(argv[0])
    if p is None:
        return None, f"program {why}"
    base = os.path.basename(p)
    if base.lower().endswith(".exe"):
        base = base[:-4]
    if base not in MIC_ALLOWED:
        return None, f"program {base!r} is not an allowed recorder ({', '.join(MIC_ALLOWED)})"
    args = argv[1:]
    if len(args) > MIC_MAX_ARGS:
        return None, f"more than {MIC_MAX_ARGS} arguments"
    for a in args:
        if not mic_arg_ok(a):
            return None, f"argument {a!r} not allowed (letters, digits, _.,:=+@%-; paths only under /dev/)"
    return list(argv), None


def mic_env_command(env=None):
    """$LILLITH_MIC_CMD as a checked argv, None when unset or refused (refusal is logged)."""
    env = os.environ if env is None else env
    v = env.get("LILLITH_MIC_CMD")
    if not v:
        return None
    try:
        argv = shlex.split(v)
    except ValueError as e:
        _log(f"LILLITH_MIC_CMD refused: cannot parse it ({e}); using the default recorder")
        return None
    ok, why = check_mic_command(argv)
    if ok is None:
        _log(f"LILLITH_MIC_CMD refused: {why}; using the default recorder")
    return ok


# --------------------------------------------------------------------------- presets.json
ECHO_MAX_TAPS = 8


def _num(s, lo, hi):
    if not re.fullmatch(r"\d{1,6}(\.\d{1,6})?", s or ""):
        raise ValueError(s)
    v = float(s)
    if not (lo <= v <= hi):
        raise ValueError(s)
    return v


def sanitize_echo(spec):
    """ffmpeg `aecho` in_gain:out_gain:delays:decays (delays/decays `|`-separated, same count) with
    numbers only: gains 0..1, delays 0 < ms <= 2000, decays 0..1, at most 8 taps. Returns the
    normalised string, "" for off, or None when rejected."""
    if spec is None or spec == "":
        return ""
    if not isinstance(spec, str):
        return None
    try:
        ig, og, delays, decays = spec.split(":")
        ig, og = _num(ig, 0.0, 1.0), _num(og, 0.0, 1.0)
        d = [_num(x, 0.001, 2000.0) for x in delays.split("|")]
        g = [_num(x, 0.0, 1.0) for x in decays.split("|")]
    except ValueError:
        return None
    if not d or len(d) != len(g) or len(d) > ECHO_MAX_TAPS:
        return None
    f = lambda v: f"{v:g}"
    return f"{f(ig)}:{f(og)}:{'|'.join(map(f, d))}:{'|'.join(map(f, g))}"


def safe_echo(spec, where="preset"):
    """sanitize_echo, logging and turning the echo off when rejected."""
    e = sanitize_echo(spec)
    if e is None:
        _log(f"{where}: echo {spec!r} rejected (numbers only: in:out:delays|..:decays|..); echo off")
        return ""
    return e


_NUM_FIELDS = {"speed": (0.25, 4.0), "pitch": (-24.0, 24.0), "highpass": (0.0, 2000.0), "lufs": (-60.0, 0.0),
               "true_peak": (-20.0, 0.0), "temperature": (0.0, 2.0), "f0_target": (0.0, 1000.0),
               "lift": (-12.0, 12.0), "gap_scale": (0.0, 10.0), "out_rate": (8000, 96000)}


def sanitize_override(name, d):
    """A presets.json entry with every value that reaches an ffmpeg filter checked: numbers clamped to
    sane ranges (a non-number is dropped), eq/treble as lists of numbers, echo via sanitize_echo."""
    out = dict(d)
    for k, (lo, hi) in _NUM_FIELDS.items():
        if k in out:
            v = out[k]
            if isinstance(v, bool) or not isinstance(v, (int, float)):
                _log(f"presets.json {name}.{k} = {v!r} dropped: not a number")
                del out[k]
            else:
                out[k] = type(lo)(min(max(v, lo), hi))
    if "eq" in out:
        eq = out["eq"]
        ok = isinstance(eq, list) and len(eq) <= 16 and all(
            isinstance(b, list) and len(b) == 3 and all(isinstance(x, (int, float)) and not isinstance(x, bool)
                                                         for x in b) for b in eq)
        if ok:
            out["eq"] = [[min(max(float(f), 10.0), 24000.0), min(max(float(w), 0.05), 10.0),
                          min(max(float(g), -24.0), 24.0)] for f, w, g in eq]
        else:
            _log(f"presets.json {name}.eq dropped: needs [[Hz, width, dB], ...] numbers")
            del out["eq"]
    if "treble" in out:
        t = out["treble"]
        if isinstance(t, list) and (len(t) == 0 or (len(t) == 2 and all(
                isinstance(x, (int, float)) and not isinstance(x, bool) for x in t))):
            out["treble"] = [min(max(float(t[0]), -24.0), 24.0), min(max(float(t[1]), 100.0), 24000.0)] if t else []
        else:
            _log(f"presets.json {name}.treble dropped: needs [dB, Hz]")
            del out["treble"]
    if "echo" in out:
        out["echo"] = safe_echo(out["echo"], f"presets.json {name}")
    return out
