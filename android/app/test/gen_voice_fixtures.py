#!/usr/bin/env python3
"""Generate parity fixtures for the Android voice port from the desktop kit (the reference).

  python3 android/app/test/gen_voice_fixtures.py KIT_DIR OUT_DIR

KIT_DIR = tools/lillith-voice (or the unpacked lillith-voice-kit) with models/pocket/tokenizer.model.
Writes TSV files (fields escaped: \\\\ \\n \\t \\r) read by VoiceTests.java. Offline: imports only the kit.
"""
import os
import random
import sys

kit = os.path.abspath(sys.argv[1])
out = os.path.abspath(sys.argv[2])
sys.path.insert(0, kit)
os.makedirs(out, exist_ok=True)

from lillith_voice.textnorm import normalize  # noqa: E402
from lillith_voice.core import clean_text, split_sentences  # noqa: E402
from lillith_voice.stt import _SAFE_RE  # noqa: E402
import sentencepiece as spm  # noqa: E402

sp = spm.SentencePieceProcessor()
sp.Load(os.path.join(kit, "models", "pocket", "tokenizer.model"))


def esc(s):
    return s.replace("\\", "\\\\").replace("\n", "\\n").replace("\t", "\\t").replace("\r", "\\r")


HAND = [
    "", " ", "Hello there, Ko. Mmm, I missed you!", "at 5:30 today", "I have 5, maybe 6", "It's 3:05 pm.",
    "Meet me at 12:00 AM or 7:45pm, ok?", "That costs $5.99, or $1,250 total.", "€20 and £3.50", "$0.01",
    "In 1999 and 2024 and 1066 and 2000 and 1900.", "the 1st, 2nd, 3rd, 4th, 11th, 21st, 102nd and 113th",
    "Dr. Smith vs. Mr. Jones, e.g. Mrs. Brown etc.", "approx. 3.14159 km", "-42 degrees, +7, 1e5", "50% off!",
    "1,000,000 stars and 123456789012345678901234567890 grains", "007 and 0.5 and .5 and 5.",
    "OK, NASA and the FBI and USB-C, also AI.", "I'm 25yo, 6'2\", 180lbs.", "Call 555-1234 or +1 (403) 555-0199.",
    "Room 101; Apt #4B.", "v2.0.1 released 10/24/2026.", "It's 11:59:59.", "St. Patrick's Day on St. Mary St.",
    "Hmm... well… okay?!", "*smiles softly* Hi Ko. *leans in* You came back.", "```python\nprint(1)\n``` done",
    "See https://example.com/path?x=1 and www.test.org now.", "<think>secret</think>Visible answer.",
    "Emoji 😀👍🏽❤️ test ✨ and ☕.", "**bold** _it_ `code` # head > quote | pipe [link](x) {braces}",
    "Ünïcödé façade, naïve café — résumé.", "日本語のテキスト。中文。한국어!", "Tab\tand\nnewline\r\nreturns.",
    "A very long sentence " * 30, "One. Two! Three? Four… Five; six: seven, eight - nine — ten.",
    "\"Quoted.\" (Paren.) [Bracket.] 'Single.'", "Mr.Smith 5pm 5 pm 5PM 17:00 0:00 24:00 99:99",
    "$5 million, $3.2 billion, 5k, 10K, 2x, x2", "#1 fan, No. 1, 1/2, 3/4, 2^10, 5*3=15", "100°C 212°F",
    "Ko's 2nd birthday; Lily's 3rd.", "ready red read rad", "  leading and trailing  ", "a" * 300,
    # textnorm fix (648277e): times keep the following space, a trailing comma is not a thousands separator
    "Meet me at 5:30 today", "at 5:30 tonight, okay?", "5:30 amazing", "at 5:30pm",
    "at 5:30 p.m. sharp", "at 7:05 AM", "at 9:00", "5, maybe 6",
    "I want 3, no 4, kisses", "1,000 kisses", "1,234,567 stars", "$5, maybe $6",
    "$1,250.50 total", "$1", "in 1999", "3.14",
    "50%", "the 21st", "hiii~ hehe", "at 5pm",
]

random.seed(20261024)
POOL = ["Ko", "Lily", "Lillith", "red", "stop", "OOC:", "5", "42", "3.5", "1,234", "$9.99", "£2", "€7.50",
        "1st", "22nd", "1999", "2026", "4:20", "10:15pm", "Dr.", "Mr.", "e.g.", "etc.", "NASA", "OK", "hmm",
        "...", "…", "!", "?", ",", ";", ":", "-", "—", "(", ")", "\"", "'", "*", "*hugs*", "😀", "✨", "é", "日本",
        "https://x.io/a", "\n", "\t", "  ", "a", "the", "really", "I'm", "don't", "you're", "100%", "#3",
        "0", "007", "12345678901", "-3", "+4", "x", "Y", "z", "café", "naïve", "ß", "İ", "ﬁ", "\u00a0", "\u2009",
        "\u200b", "ǅ", "Ⅻ", "²", "٣", "໕"]
FUZZ = []
for _ in range(400):
    n = random.randint(1, 18)
    FUZZ.append("".join(random.choice(POOL) + random.choice(["", " ", " ", " ", ""]) for _ in range(n)))
CORPUS = HAND + FUZZ

with open(os.path.join(out, "textnorm.tsv"), "w", encoding="utf-8") as f:
    for s in CORPUS:
        f.write(esc(s) + "\t" + esc(normalize(s)) + "\n")

with open(os.path.join(out, "clean_split.tsv"), "w", encoding="utf-8") as f:
    for s in CORPUS:
        c = clean_text(s)
        f.write(esc(s) + "\t" + esc(c) + "\t" + "\t".join(esc(x) for x in split_sentences(c)) + "\n")

TOK = list(CORPUS)
for s in CORPUS:
    TOK.append(normalize(s))
rng = random.Random(7)
for _ in range(300):  # random unicode incl. astral + private use: exercises unk / byte fallback
    cps = [rng.choice([rng.randint(0x20, 0x7e), rng.randint(0xa0, 0x2fff), rng.randint(0x3000, 0x9fff),
                       rng.randint(0x1f300, 0x1faff), 0x20, 0x20]) for _ in range(rng.randint(1, 25))]
    TOK.append("".join(chr(c) for c in cps if not 0xd800 <= c <= 0xdfff))
with open(os.path.join(out, "tokenizer.tsv"), "w", encoding="utf-8") as f:
    for s in TOK:
        f.write(esc(s) + "\t" + " ".join(map(str, sp.Encode(s))) + "\n")

# full text chain: speaker._play -> render(normalize) -> pocket.split -> prepare -> Encode, plus the seed
import json  # noqa: E402
meta = json.load(open(os.path.join(kit, "models", "pocket", "bundle.json")))


class _Pv:  # pocket.PocketVoice text methods without loading the ONNX graphs
    from lillith_voice.pocket import PocketVoice as _P
    prepare = _P.prepare
    split = _P.split

    def __init__(self):
        self.meta, self.tok, self.max_tok = meta, sp, int(meta.get("max_token_per_chunk", 50))


pv = _Pv()
with open(os.path.join(out, "chain.tsv"), "w", encoding="utf-8") as f:
    for s in CORPUS:
        for c in split_sentences(clean_text(s)):
            t = normalize(c)
            seed = (1234 + sum(map(ord, t)) * 31) % (2 ** 31)
            parts = pv.split(t)
            row = [esc(c), esc(t), str(seed)]
            for p in parts:
                pt, after = pv.prepare(p)
                row += [esc(pt), str(after), " ".join(map(str, sp.Encode(pt)))]
            f.write("\t".join(row) + "\n")

# typed safeword: kit regex verdicts (the Java test adds the Rust kill_switch cases itself)
SAFE = ["Red.", "red", "Red!", "Read.", "Rad.", "Rid.", "Rhett.", "Wred", "Red, red.", "Right.", "OOC: stop",
        "Oh, see, stop.", "O O C stop", "red red red", "Oh, I'll see, stop.", "O-C. Start.", "Oh, see, start.",
        "O.O.C. stop!", "out of character, stop", "That's right.", "I read it.", "Yes.", "Stop reading.",
        "All right then.", "Right, let's go to the red room.", "Read me a story.", "Yes, I agree.", "Wait.",
        "Oh, stop it.", "You'll stop.", "I see, stop that.", "Oh, I'll see you start.", "Let's start.", "Stop.",
        "Ready.", "Rate.", "", "RED", " Red! ", "red.", "ooc stop please", "(OOC: STOP)", "redo",
        "I read a red book", "stop", "ooc: go on", "reddit", "red red red red red", "ooc  :  stop", "OOC:stop",
        "o.o.c. stop", "ooc, stop", "please ooc stop now", "red\n", "\tred\t", "RED RED", "rEd?!", "red 🙂",
        "Ｒｅｄ", "ooc: stopped", "ooc: stopping", "ooc: stop.", "ok see stop", "oh see stop", "oc stop",
        "Ooc stop", "OOC Stop!", "bored", "red-", "-red-", "re d", "redd"]
with open(os.path.join(out, "safeword_kit.tsv"), "w", encoding="utf-8") as f:
    for s in SAFE + FUZZ[:150]:
        f.write(esc(s) + "\t" + ("1" if _SAFE_RE.search(s.strip()) else "0") + "\n")
print("fixtures ->", out, len(CORPUS), "texts,", len(TOK), "tokenizer rows")

# DSP: polish chain on deterministic synthetic "speech" (tone bursts + noise + silences), float32 .npy
import numpy as np  # noqa: E402
from lillith_voice import dsp  # noqa: E402

r = np.random.default_rng(5)
SR = 24000
lines = []
for k, secs in enumerate((2.37, 1.0, 0.31, 0.05)):
    n = int(SR * secs)
    t = np.arange(n) / SR
    env = (np.sin(2 * np.pi * 3.1 * t) > -0.2).astype(np.float32)
    x = (0.3 * np.sin(2 * np.pi * 220 * t) + 0.25 * np.sin(2 * np.pi * 6500 * t) * (t > secs / 2)
         + 0.05 * r.standard_normal(n)) * env * (0.5 + 1.5 * k % 3)
    x = np.concatenate([np.zeros(1500), x, np.zeros(3000)]).astype(np.float32)
    np.save(os.path.join(out, f"dsp_raw{k}.npy"), x)
    y, g = dsp.polish(x, SR, -17.0, -1.5, 90.0, [(250, 1.0, -1.5), (3000, 0.9, 1.5)], (9000, -3.0), 6.0, None)
    np.save(os.path.join(out, f"dsp_pol{k}.npy"), np.asarray(y, np.float32))
    y2, _ = dsp.polish(x, SR, -17.0, -1.5, 90.0, [(250, 1.0, -1.5), (3000, 0.9, 1.5)], (9000, -3.0), 6.0, 4.25)
    np.save(os.path.join(out, f"dsp_polref{k}.npy"), np.asarray(y2, np.float32))
    L = dsp.loudness(dsp.eq_deess(dsp.trim(x, SR), SR), SR)
    lines.append(f"{k}\t{'nan' if g is None else repr(float(g))}\t{'nan' if L is None else repr(L)}")
    cf = dsp.crossfade_concat([x[:700], x[700:2000], x[2000:]], SR, 3.0)
    np.save(os.path.join(out, f"dsp_xf{k}.npy"), cf.astype(np.float32))
open(os.path.join(out, "dsp.tsv"), "w").write("\n".join(lines) + "\n")
print("dsp fixtures ok")
