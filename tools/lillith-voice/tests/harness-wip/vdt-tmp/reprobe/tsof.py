"""tsof.py FILE REGEX [AFTER_EPOCH] [nth]: epoch of the nth (default 1) line matching REGEX (app logs: ISO UTC prefix; others: leading epoch)."""
import re, sys, datetime
f, rx = sys.argv[1], re.compile(sys.argv[2]); after = float(sys.argv[3]) if len(sys.argv) > 3 and sys.argv[3] else 0.0
nth = int(sys.argv[4]) if len(sys.argv) > 4 else 1
ansi = re.compile(r'\x1b\[[0-9;]*m'); k = 0
for line in open(f, errors="replace"):
    l = ansi.sub("", line)
    m = re.match(r'(\d{4}-\d\d-\d\dT[\d:.]+)Z', l)
    if m:
        s = m.group(1); s = s[:26] if '.' in s else s
        t = datetime.datetime.fromisoformat(s).replace(tzinfo=datetime.timezone.utc).timestamp()
    else:
        m = re.match(r'(\d{9,}\.\d+)', l)
        if not m: continue
        t = float(m.group(1))
    if t >= after and rx.search(l):
        k += 1
        if k == nth: print(f"{t:.3f}"); sys.exit(0)
sys.exit(1)
