"""lillith-chat: stream a reply from a LOCAL llama-server (127.0.0.1:8080) and speak it sentence by sentence.

Voice loop:  --mic  (always listening, VAD turn-taking, barge-in)  or  --ptt  (Enter to talk / Enter to stop)
  mic -> silero VAD -> whisper (local) -> llama-server 127.0.0.1 -> Lillith's cloned voice.
Safety: the safeword ("red" / "OOC: stop") stops her at once and is never sent to the model as chat;
a spoken (or typed) "yes" is never accepted as agreement to a Soul Contract or plan: only a mouse click on
Yes on the card approves.
Only loopback URLs are accepted (netguard enforces it as well).
"""
from . import netguard  # noqa: F401

import argparse
import json
import queue
import re
import sys
import threading
import urllib.parse
import urllib.request

from .client import connect, DaemonBackend

_CONSENT_CTX = re.compile(r"soul\s*contract|\bplan\b|\bcontract\b|do you (agree|accept|consent)|confirm", re.I)
SAFEWORD_NOTE = "[safeword heard - Lillith stopped. Type to continue.]"
CONSENT_NOTE = ("[voice can't agree to a Soul Contract or plan - click Yes on the card if you mean it]")

_SENT = re.compile(r"^(.*?[.!?…]+[\"')\]]*)(\s+)(.*)$", re.S)


def stream_reply(url, messages, temperature, max_tokens):
    body = json.dumps({"messages": messages, "stream": True, "temperature": temperature,
                       "max_tokens": max_tokens, "cache_prompt": True}).encode()
    req = urllib.request.Request(url, body, {"Content-Type": "application/json"})
    # no proxies, ever (a proxy env var must not route this off-box)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(req, timeout=600) as r:
        for raw in r:
            line = raw.decode(errors="replace").strip()
            if not line.startswith("data:"):
                continue
            data = line[5:].strip()
            if data == "[DONE]":
                return
            try:
                delta = json.loads(data)["choices"][0].get("delta", {}).get("content") or ""
            except (KeyError, IndexError, ValueError):
                continue
            if delta:
                yield delta


def main(argv=None):
    ap = argparse.ArgumentParser(prog="lillith-chat")
    ap.add_argument("prompt", nargs="*", help="one prompt; omit for an interactive chat")
    ap.add_argument("--url", default="http://127.0.0.1:8080/v1/chat/completions")
    ap.add_argument("--system", default="You are Lillith, a playful, flirty desktop companion. "
                                        "Reply in short, spoken-style sentences. No emoji, no markdown.")
    ap.add_argument("-p", "--preset")
    ap.add_argument("--temperature", type=float, default=0.95)  # same as the app (lillith-mind ChatOptions)
    ap.add_argument("--max-tokens", type=int, default=160)  # same cap as the app; short spoken replies
    ap.add_argument("--mute", action="store_true", help="print only")
    mic = ap.add_mutually_exclusive_group()
    mic.add_argument("--mic", action="store_true", help="talk to her: always-listening voice loop (VAD, barge-in)")
    mic.add_argument("--ptt", action="store_true", help="talk to her: push-to-talk (Enter to start / Enter to stop)")
    ap.add_argument("--turns", type=int, default=0, help="voice loop: stop after N turns (0 = forever)")
    a = ap.parse_args(argv)

    host = urllib.parse.urlparse(a.url).hostname
    if host not in ("127.0.0.1", "localhost", "::1"):
        raise SystemExit("lillith-chat only talks to a llama-server on this machine (127.0.0.1)")
    voice = None if a.mute else connect("auto", preset=a.preset)
    knobs = {"preset": a.preset} if a.preset else {}
    messages = [{"role": "system", "content": a.system}]

    def turn(prompt):
        messages.append({"role": "user", "content": prompt})
        buf, full = "", ""
        for tok in stream_reply(a.url, messages, a.temperature, a.max_tokens):
            sys.stdout.write(tok)
            sys.stdout.flush()
            full += tok
            buf += tok
            while voice:
                m = _SENT.match(buf)
                if not m:
                    break
                voice.say(m.group(1), wait=False, **knobs)
                buf = m.group(3)
        print()
        messages.append({"role": "assistant", "content": full})
        if voice:
            voice.say(buf, wait=True, **knobs)  # also waits for earlier sentences to finish

    if a.prompt:
        turn(" ".join(a.prompt))
        return 0
    if a.mic or a.ptt:
        return voice_loop(a, voice or connect("auto", preset=a.preset), messages, turn)
    try:
        while True:
            p = input("you> ").strip()
            if p in ("/quit", "/exit"):
                break
            if p:
                print("lillith> ", end="")
                turn(p)
    except (EOFError, KeyboardInterrupt):
        print()
    return 0


def guard(text, messages):
    """None if the transcript may go to the model, else the note to print instead."""
    from .stt import classify
    c = classify(text)
    if c["safeword"]:
        return SAFEWORD_NOTE
    last = next((m["content"] for m in reversed(messages) if m["role"] == "assistant"), "")
    if c["affirmation"] and _CONSENT_CTX.search(last or ""):
        return CONSENT_NOTE
    return None


def voice_loop(a, be, messages, turn):
    events = queue.Queue()
    if isinstance(be, DaemonBackend):
        sub = DaemonBackend()
        sub.conn.call(op="subscribe", topics=["stt"])

        def pump():
            while True:
                ev = sub.conn.recv()
                if ev is None:
                    return
                events.put(ev)
        threading.Thread(target=pump, daemon=True).start()
    else:
        be.listener.emit = events.put
    n = 0
    if a.ptt:
        try:
            while not a.turns or n < a.turns:
                input("[Enter] talk ")
                be.listen("start")
                input("[Enter] done ")
                r = be.listen("stop")
                n += _voice_turn(r.get("text", ""), messages, turn, be)
        except (EOFError, KeyboardInterrupt):
            print()
        return 0
    ctl = DaemonBackend() if isinstance(be, DaemonBackend) else None
    (ctl.call(cmd="listen_mode", on=True) if ctl else be.listener.set_always(True))
    print("lillith-chat: listening (mic is muted while she talks; talk over her to interrupt; 'red' = stop)",
          file=sys.stderr)
    try:
        while not a.turns or n < a.turns:
            ev = events.get()
            if ev.get("ev") == "transcript" and ev.get("text"):
                n += _voice_turn(ev["text"], messages, turn, be)
    except KeyboardInterrupt:
        print()
    finally:
        (ctl.call(cmd="listen_mode", on=False) if ctl else be.listener.set_always(False))
    return 0


def _voice_turn(text, messages, turn, be):
    if not text.strip():
        return 0
    print(f"you (voice)> {text}")
    note = guard(text, messages)
    if note:
        be.stop()
        print(note)
        return 1
    print("lillith> ", end="")
    turn(text)
    return 1


if __name__ == "__main__":
    sys.exit(main())
