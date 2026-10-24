"""textnorm cases (stdlib + the kit's lillith_voice package, no models). Run: python tests/textnorm_test.py [libdir]"""
import importlib.util
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
LIB = sys.argv[1] if len(sys.argv) > 1 else os.path.dirname(HERE)
# load textnorm.py directly (it is pure stdlib), so this runs without numpy / onnxruntime
spec = importlib.util.spec_from_file_location("textnorm", os.path.join(LIB, "lillith_voice", "textnorm.py"))
tn = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tn)

CASES = [
    # times: am/pm only as its own word, the following word keeps its space
    ("Meet me at 5:30 today", "Meet me at five thirty today"),
    ("at 5:30 tonight, okay?", "at five thirty tonight, okay?"),
    ("5:30 amazing", "five thirty amazing"),
    ("at 5:30pm", "at five thirty P M"),
    ("at 5:30 p.m. sharp", "at five thirty P M sharp"),
    ("at 7:05 AM", "at seven oh five A M"),
    ("at 9:00", "at nine o'clock"),
    # commas: only a thousands separator before exactly three digits
    ("5, maybe 6", "five, maybe six"),
    ("I want 3, no 4, kisses", "I want three, no four, kisses"),
    ("1,000 kisses", "one thousand kisses"),
    ("1,234,567 stars", "one million two hundred thirty-four thousand five hundred sixty-seven stars"),
    ("$5, maybe $6", "five dollars, maybe six dollars"),
    ("$1,250.50 total", "one thousand two hundred fifty dollars and fifty cents total"),
    ("$1", "one dollar"),
    # unchanged behaviour
    ("in 1999", "in nineteen ninety-nine"),
    ("3.14", "three point one four"),
    ("50%", "fifty percent"),
    ("the 21st", "the twenty-first"),
    ("hiii~ hehe", "hiii Hehe."),
    ("at 5pm", "at five P M"),
]

fail = 0
for src, want in CASES:
    got = tn.normalize(src)
    if got == want:
        print(f"PASS  {src!r} -> {got!r}")
    else:
        fail += 1
        print(f"FAIL  {src!r} -> {got!r} (want {want!r})")
print(f"{len(CASES) - fail}/{len(CASES)} passed")
sys.exit(1 if fail else 0)
