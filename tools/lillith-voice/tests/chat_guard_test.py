"""lillith-chat consent wording (no models, no daemon):  python tests/chat_guard_test.py    exit 0 = all passed
A spoken yes after a contract / plan question is held back, and the note points to the card (mouse-only)."""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from lillith_voice import chat  # noqa: E402

FAILS = []


def check(name, cond, detail=""):
    print(("PASS " if cond else "FAIL ") + name + (f"  ({detail})" if detail and not cond else ""))
    if not cond:
        FAILS.append(name)


ask = [{"role": "assistant", "content": "Want me to write that file? It needs a Soul Contract."}]
for said in ("Yes.", "yeah", "Sure, I agree", "okay"):
    n = chat.guard(said, ask)
    check(f"spoken {said!r} after a contract question is held back", n == chat.CONSENT_NOTE, str(n))
check("the consent note says to click Yes on the card", "click Yes on the card" in chat.CONSENT_NOTE)
check("the consent note never says to type the answer", "type" not in chat.CONSENT_NOTE.lower())
check("the module doc never says 'type it instead'", "type it instead" not in (chat.__doc__ or ""))
check("the safeword is never sent as chat", chat.guard("red", ask) == chat.SAFEWORD_NOTE)
check("ordinary speech goes through", chat.guard("tell me about your day", ask) is None)
print("ALL PASSED" if not FAILS else f"{len(FAILS)} FAILED")
sys.exit(1 if FAILS else 0)
