"""Find loop cut points in an extracted frame dir (f_NNN.png).
Usage: find_loops.py <dir> [minlen] [maxlen] [top]"""
import sys, glob
import numpy as np
from PIL import Image
d = sys.argv[1]; lo = int(sys.argv[2]) if len(sys.argv) > 2 else 18
hi = int(sys.argv[3]) if len(sys.argv) > 3 else 60; top = int(sys.argv[4]) if len(sys.argv) > 4 else 6
fs = sorted(glob.glob(f'{d}/f_*.png'))
sm = [np.asarray(Image.open(p).convert('L').resize((320, 180)), np.float32) for p in fs]
n = len(sm); res = []
for i in range(n):
    for L in range(lo, hi + 1):
        if i + L < n:
            res.append((float(np.abs(sm[i] - sm[i + L]).mean()), i + 1, i + L + 1, L))
res.sort()
print(d, 'frames', n, 'first-last %.2f' % np.abs(sm[0] - sm[-1]).mean())
for r in res[:top]: print('  %.2f  f%d -> f%d  len %d (%.2fs)' % r[:3] + (r[3], r[3] / 24) if False else '  %.2f  f%d -> f%d  len %d (%.2fs)' % (r[0], r[1], r[2], r[3], r[3] / 24))
