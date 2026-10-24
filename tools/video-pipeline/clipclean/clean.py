"""Clean every unique clip sheet in the worktree in place (cc_lib.clean), optional whole-clip
integer shift (shifts.json: {clip:[dx,dy]}), refresh bbox_per_frame. Backs up originals to BK."""
import os as _os
_ROOT = _os.environ.get('CC_REPO', _os.path.abspath(_os.path.join(_os.path.dirname(__file__), '..', '..', '..')))
_OUT = _os.environ.get('CC_OUT', '/workspace/verify/clipclean')
import json, os, sys, shutil, numpy as np
from multiprocessing import Pool
from PIL import Image
sys.path.insert(0, os.path.dirname(__file__))
import cc_lib
G = _ROOT + '/assets/generated'
BK = _OUT + '/orig_sheets'
SH = json.load(open(_os.path.join(_os.path.dirname(_os.path.abspath(__file__)), 'shifts.json'))) if os.path.exists(_os.path.join(_os.path.dirname(_os.path.abspath(__file__)), 'shifts.json')) else {}

def shift(c, dx, dy):
    if not dx and not dy: return c
    o = np.zeros_like(c); H, W = c.shape[:2]
    ys = slice(max(0, dy), H + min(0, dy)); yd = slice(max(0, -dy), H + min(0, -dy))
    xs = slice(max(0, dx), W + min(0, dx)); xd = slice(max(0, -dx), W + min(0, -dx))
    o[ys, xs] = c[yd, xd]; return o

BASE_CACHE = {}
def orig_cell(d, i):
    b = f'{BK}/{d}_sheet.png'
    src = b if os.path.exists(b) else f'{G}/{d}/{d}_sheet.png'
    im = np.asarray(Image.open(src).convert('RGBA'))
    return im[:, i*400:(i+1)*400]

def clean_base(b, i):
    k = (b, i)
    if k not in BASE_CACHE:
        BASE_CACHE[k] = (orig_cell(b, i), cc_lib.clean(orig_cell(b, i), b)[0])
    return BASE_CACHE[k]

def event_base(d):
    j = json.load(open(f'{G}/{d}/{d}.json'))
    b = j.get('base_clip')
    if not b: return None, None
    b = b.split('/')[-1]
    if not os.path.exists(f'{G}/{b}/{b}_sheet.png'): return None, None
    return b, j.get('base_frames')

def run(d):
    p = f'{G}/{d}/{d}_sheet.png'
    os.makedirs(BK, exist_ok=True)
    b = f'{BK}/{d}_sheet.png'
    if not os.path.exists(b): shutil.copy2(p, b)
    im = np.asarray(Image.open(b).convert('RGBA'))
    N = im.shape[1] // 400
    dx, dy = SH.get(d, [0, 0])
    out = np.zeros_like(im); bbs = []; tot = {}
    base, bfr = event_base(d) if d.startswith('event_') else (None, None)
    for i in range(N):
        if base is not None and bfr and i < len(bfr):
            # code-drawn event: body pixels are a verbatim copy of the base clip frame. Swap in the cleaned base
            # wherever the event still equals the base; icons/FX/fades (pixels that differ) stay untouched.
            e = im[:, i*400:(i+1)*400]
            bo, bc = clean_base(base, bfr[i])
            same = np.abs(e.astype(int) - bo.astype(int)).max(-1) <= 2
            r = np.where(same[..., None], bc, e).astype(np.uint8)
            st = {'event_body_px_swapped': int((same & (bo[..., 3] > 0)).sum()),
                  'event_body_px_kept': int((~same & (bo[..., 3] > 0)).sum()),
                  'event_px_changed': int((np.abs(r.astype(int) - e.astype(int)).max(-1) > 0).sum())}
            r = shift(r, dx, dy)
        else:
            c = shift(im[:, i*400:(i+1)*400], dx, dy)
            r, st = cc_lib.clean(c, d)
        out[:, i*400:(i+1)*400] = r
        for k, v in st.items(): tot[k] = tot.get(k, 0) + int(v)
        ys, xs = np.nonzero(r[..., 3] > 0)
        bbs.append([int(xs.min()), int(xs.max()), int(ys.min()), int(ys.max())] if len(xs) else [0, 0, 0, 0])
    Image.fromarray(out, 'RGBA').save(p, optimize=True)
    jp = f'{G}/{d}/{d}.json'; j = json.load(open(jp))
    if 'bbox_per_frame' in j:
        old = j['bbox_per_frame'][0]
        # keep the existing [x0,x1,y0,y1] convention
        j['bbox_per_frame'] = bbs
    open(jp, "w").write(json.dumps(j, indent=2))
    return d, N, dx, dy, tot

if __name__ == '__main__':
    m = json.load(open(f'{G}/manifest.json'))
    dirs = sorted({e['sheet'].split('/')[0] for e in m['animations']})
    if len(sys.argv) > 1: dirs = sys.argv[1:]
    with Pool(int(os.environ.get('CC_JOBS', '3'))) as pool:
        for r in pool.imap_unordered(run, dirs):
            print(*r, flush=True)
