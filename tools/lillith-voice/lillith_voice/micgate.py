"""Mic gate: the microphone starts only after Ko's Yes on a Soul Contract card (mouse click only).

Minimal design (PROTOCOL.md, "Mic gate"):

* The daemon shows the card itself, with ``lillith contract-card`` (the same mouse-only card the app
  uses; ``$LILLITH_CARD_CMD`` or ``lillith`` on PATH, absolute and executable). Voice, typed text and
  keys can never answer it.
* ``{"cmd":"mic_card","kind":"listen"|"always"}`` shows a card and, on Yes, replies with a one-time
  ``token`` (random, valid 120 s, usable once, only for that kind). Pass it as ``"token"`` on
  ``listen`` / ``listen_mode``.
* A gated request without a token shows the card inline (the reply waits for Ko) and, on Yes, uses a
  freshly minted one-time token at once, so old clients keep working.
* ``kind:"listen"`` (push-to-talk and one-shot listen): EVERY press / listen start needs its own fresh
  card Yes (inline card or one-time token). There is no session-long grant: a Yes covers exactly one
  capture (Ko's rule, "Push-to-talk: card on every press").
* The card belongs to that one press: if the press is released, the safeword is said, or the card times
  out before Yes, the card is closed and audited ``CANCELLED`` (``TIMEOUT`` for a timeout) and the mic
  never opens, even if a Yes lands a moment later (``cancel()``, ``card(..., wanted=...)``).
* ``kind:"always"`` (always-listening on): every switch-on needs its own card or token.
* No, timeout, a closed card or no card UI: the mic stays off (``{"ok":false,"need_card":true,...}``).
  ``listen_mode`` off, ``stt`` on a file and ``mic_revoke`` (drops unused tokens) are never gated.
* Every decision is audited (``PROPOSED`` / ``CONFIRM`` / ``DENY`` / ``TIMEOUT`` / ``CANCELLED`` /
  ``NO_UI``, ``card=<id>``) and so is every capture start (``LAUNCH``).

Threat model: this guards the mic against other local socket clients (and a confused app); whoever
controls the daemon's own environment ($LILLITH_CARD_CMD, $PATH) is trusted, as for any program.
"""
import os
import secrets
import shutil
import signal
import subprocess
import threading
import time

from . import launchguard as G

TOKEN_TTL_S = 120.0
CARD_TIMEOUT_S = 60
KINDS = {"listen": "mic_listen", "always": "mic_always"}

_TEXT = {
    "listen": ("MICROPHONE", "listen on your microphone for this one press of talk (the next press asks again)"),
    "always": ("ALWAYS LISTENING", "keep your microphone open and transcribe everything until you switch it off"),
}


def card_program(env=None):
    """The card helper: $LILLITH_CARD_CMD (checked) else `lillith` on PATH (absolute, checked)."""
    env = os.environ if env is None else env
    if env.get("LILLITH_CARD_CMD"):
        return G.env_exe("LILLITH_CARD_CMD", env)
    w = shutil.which("lillith", path=env.get("PATH"))
    return G.checked_exe(os.path.abspath(w))[0] if w and os.path.isabs(w) else None


def run_card(program, title, summary, target, detail, timeout=CARD_TIMEOUT_S, cancel=None):
    """Shows the card; returns yes / no / timeout / cancelled / no_ui (anything unexpected is no_ui).

    `cancel` (a threading.Event): when it is set the card window is closed at once and the answer is
    "cancelled", whatever the helper would have said."""
    if not program:
        return "no_ui"   # the card helper itself answers no_ui when it cannot open a window
    text = f"{title}\n{summary}\n{target}\n{detail}"
    argv = [program, "contract-card", "--timeout", str(int(timeout))]
    G.audit_once("mic_card_ui", argv)
    try:
        p = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                             start_new_session=(os.name != "nt"), **G.NO_WINDOW)
    except OSError:
        return "no_ui"
    box = {}

    def talk():
        try:
            box["out"] = p.communicate(text.encode())[0]
        except (OSError, ValueError):
            box["out"] = b""

    th = threading.Thread(target=talk, daemon=True)
    th.start()
    end = time.monotonic() + timeout + 10
    while th.is_alive():
        if cancel is not None and cancel.is_set():
            _kill(p)
            th.join(2)
            return "cancelled"
        if time.monotonic() > end:
            _kill(p)
            th.join(2)
            return "timeout"
        th.join(0.02)
    if cancel is not None and cancel.is_set():
        return "cancelled"
    w = box.get("out", b"").decode(errors="replace").strip()
    return w if w in ("yes", "no", "timeout", "cancelled") else "no_ui"


def _kill(p):
    """Closes the card helper (and, on POSIX, anything it started: it runs in its own session)."""
    try:
        if os.name != "nt":
            os.killpg(p.pid, signal.SIGKILL)
        else:
            p.kill()
    except OSError:
        pass


_TAG = {"yes": "CONFIRM", "no": "DENY", "timeout": "TIMEOUT", "cancelled": "CANCELLED", "no_ui": "NO_UI"}


class MicGate:
    """One-time tokens, no standing grant. `ask(kind, title, summary, target, detail) -> word` is injectable."""

    def __init__(self, mic_argv=None, ask=None, log=None, clock=time.monotonic):
        self.mic_argv = mic_argv or (lambda: ["(sounddevice)"])
        self._ask = ask
        self.log = log
        self.clock = clock
        self._lock = threading.Lock()
        self._card_open = False
        self._cancel = None        # threading.Event of the open card (set: close it, answer cancelled)
        self._owner = None         # who opened it (a press id), so a release only closes its own card
        self._tokens = {}          # token -> (kind, expires, card id)
        self.last_card = None      # card id of the last approved capture (for LAUNCH lines)

    # ------------------------------------------------------------------ cards
    def _show(self, kind, card, cancel):
        title, summary = _TEXT[kind]
        argv = self.mic_argv()
        detail = "Recorder command (one argument per line):\n$ " + "\n    ".join(argv) + \
                 "\n\nAudio stays on this computer (local speech-to-text). No / Esc / closing: the mic stays off."
        if self._ask is not None:
            word = self._ask(kind, title, summary, f"microphone (card {card})", detail)
            return "cancelled" if cancel.is_set() else word
        return run_card(card_program(), title, summary, f"microphone (card {card})", detail, cancel=cancel)

    def cancel(self, owner=None):
        """Closes the open card (released press: only that press's `owner` card; safeword: any card). It
        answers `cancelled`, never Yes. Never blocks (safe from the stop path). Returns whether one closed."""
        with self._lock:
            ev = self._cancel
            if owner is not None and self._owner != owner:
                ev = None
        if ev is None:
            return False
        ev.set()
        return True

    def card(self, kind, wanted=None, owner=None):
        """Shows one card for `kind`; on Yes returns {"ok":True,"token":...,"card":id}.

        `wanted()` (optional) is asked once the card has answered: False (the press was released or the
        safeword said meanwhile) turns even a Yes into `cancelled`."""
        if kind not in KINDS:
            return {"ok": False, "error": f"unknown mic kind {kind!r} (listen | always)"}
        cancel = threading.Event()
        with self._lock:
            if self._card_open:
                return {"ok": False, "error": "a mic card is already open", "need_card": True}
            self._card_open = True
            self._cancel, self._owner = cancel, owner
        card = f"mic-{int(time.time() * 1000)}-{os.getpid()}-{secrets.token_hex(2)}"
        try:
            G.audit("PROPOSED", KINDS[kind], self.mic_argv(), card, self.log)
            word = self._show(kind, card, cancel)
            if word == "yes" and wanted is not None and not wanted():
                word = "cancelled"
            G.audit(_TAG.get(word, "NO_UI"), KINDS[kind], self.mic_argv(), card, self.log)
        finally:
            with self._lock:
                self._card_open = False
                self._cancel = self._owner = None
        if word != "yes":
            why = {"no": "Ko said no", "timeout": "the card timed out",
                   "cancelled": "the card was closed (press released or safeword)",
                   }.get(word, "no card could be shown (no display or no `lillith` binary)")
            return {"ok": False, "error": f"microphone not allowed: {why}", "need_card": True, "card": card,
                    "answer": word}
        token = secrets.token_urlsafe(24)
        with self._lock:
            self._tokens[token] = (kind, self.clock() + TOKEN_TTL_S, card)
        return {"ok": True, "token": token, "card": card, "expires_s": TOKEN_TTL_S}

    def _take(self, kind, token):
        """Consumes a valid one-time token for `kind`; returns its card id or None."""
        now = self.clock()
        with self._lock:
            for t in [t for t, v in self._tokens.items() if v[1] < now]:
                del self._tokens[t]
            v = self._tokens.get(token) if isinstance(token, str) else None
            if v is None or v[0] != kind:
                return None
            del self._tokens[token]
            return v[2]

    # ------------------------------------------------------------------ gate
    def allow(self, kind, token=None, wanted=None, owner=None):
        """None when ONE capture may start (self.last_card = the approving card), else the error reply.

        Every call needs a fresh Yes: a valid unused token for `kind`, or an inline card answered Yes now.
        Nothing is remembered between presses (no session grant for push-to-talk)."""
        if token:
            card = self._take(kind, token)
            if card is None:
                return {"ok": False, "error": "mic token invalid, expired, used or for another kind",
                        "need_card": True}
        else:
            r = self.card(kind, wanted, owner)
            if not r.get("ok"):
                return r
            card = self._take(kind, r["token"])
        self.last_card = card
        return None

    def revoke(self):
        with self._lock:
            self._tokens.clear()
        return {"ok": True, "revoked": True}
