"""Safeword / consent classifier table (no models, no daemon): python tests/safeword_test.py [LIB_DIR]"""
import os, sys
sys.path.insert(0, sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from lillith_voice.stt import classify
# (text, speech seconds or None, expected safeword)
CASES = [("Red.", 0.5, True), ("red", None, True), ("Red!", 0.4, True), ("Read.", 0.6, True), ("Rad.", 0.5, True),
         ("Rid.", 0.4, True), ("Rhett.", 0.5, True), ("Wred", 0.5, True), ("Red, red.", 1.2, True), ("Right.", 0.5, True),
         ("OOC: stop", 1.0, True), ("Oh, see, stop.", 1.2, True), ("O O C stop", 1.2, True), ("red red red", 1.6, True), ("Oh, I'll see, stop.", 1.3, True), ("O-C. Start.", 1.0, True), ("Oh, see, start.", 1.0, True), ("O.O.C. stop!", 1.2, True), ("out of character, stop", 1.6, True),
         ("That's right.", 0.8, False), ("I read it.", 0.9, False), ("Yes.", 0.4, False), ("Stop reading.", 0.9, False),
         ("All right then.", 0.9, False), ("Right, let's go to the red room.", 2.0, False), ("Read me a story.", 1.1, False),
         ("Right.", 1.4, False), ("Read.", 2.4, False), ("Yes, I agree.", 1.0, False), ("Wait.", 0.4, False), ("Oh, stop it.", 0.9, False), ("You'll stop.", 0.7, False), ("I see, stop that.", 1.2, False), ("Oh, I'll see you start.", 1.2, False), ("Let's start.", 0.6, False), ("Stop.", 0.4, False),
         ("Ready.", 0.5, False), ("Rate.", 0.4, False), ("", None, False)]
bad = 0
for t, s, want in CASES:
    c = classify(t, s)
    ok = c["safeword"] == want and c["consent_ok"] is False
    bad += not ok
    print(("PASS " if ok else "FAIL ") + f"{t!r:40} speech_s={s} safeword={c['safeword']} (want {want})")
for t in ("Yes.", "Yes, I agree.", "I consent", "of course"):
    c = classify(t, 0.6)
    ok = c["affirmation"] and c["consent_ok"] is False
    bad += not ok
    print(("PASS " if ok else "FAIL ") + f"{t!r:40} affirmation={c['affirmation']} consent_ok={c['consent_ok']}")
sys.exit(1 if bad else 0)
