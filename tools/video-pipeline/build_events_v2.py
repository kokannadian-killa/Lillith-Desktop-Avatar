#!/usr/bin/env python3
"""event_* clips for generated_v2: new video-based body clip (generated_v2/<base>) +
the original code-drawn icons/effects (painters reused verbatim from
generated/_tools/build_events.py, no edits there).

usage: build_events_v2.py cpu_busy wifi ...   (event slugs from the old EV table)
Base frames: N old frames -> N frames sampled evenly over the v2 base clip
(BASE_RANGE can restrict the part of the base clip that is used)."""
import sys, os, json, glob
import numpy as np
from PIL import Image
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import build_clip as bc
import os as _o, sys as _s; _s.path.insert(0, _o.path.dirname(_o.path.abspath(__file__))); import paths
OLDT = paths.V1_TOOLS
sys.path.insert(0, OLDT)
src = open(f'{OLDT}/build_events.py').read().split('\ntodo=')[0]
ns = {'__name__': 'old_events'}
exec(compile(src, 'build_events.py', 'exec'), ns)
EV, DOTTED, Pen = ns['EV'], ns['DOTTED'], ns['Pen']
FING, BADGE, ZOOM, NOZOOM, FING_EV, ease = ns['FING'], ns['BADGE'], ns['ZOOM'], ns['NOZOOM'], ns['FING_EV'], ns['ease']
V2 = bc.ROOT
# which part of the v2 base clip to sample (fractions of its length); default whole clip
BASE_RANGE = {'archiving': (0.6, 1.0), 'window_open': (0.4, 1.0)}

def v2_frames(base):
    b = base.replace('clean:', '')
    fs = sorted(glob.glob(f'{V2}/{b}/frames/*.png'))
    if not fs: raise SystemExit(f'v2 base clip {b} missing')
    return [Image.open(f).convert('RGBA') for f in fs], b

for name in sys.argv[1:]:
    base, idx_old, fn, desc = EV[name]
    frames, bname = v2_frames(base)
    bj = json.load(open(f'{V2}/{bname}/{bname}.json'))
    N = len(idx_old); Nb = len(frames)
    if name == 'incognito':
        idx = [0] * N
    else:
        lo, hi = BASE_RANGE.get(name, (0.0, 1.0))
        idx = [int(round(x)) for x in np.linspace(lo * (Nb - 1), hi * (Nb - 1), N)]
    out = []
    for k, i in enumerate(idx):
        c = frames[i].copy()
        if name == 'incognito' and k >= 4:
            a = np.asarray(c).copy(); a[..., 3] = (a[..., 3] * (1 - 0.55 * ease((k - 4) / 4))).astype(np.uint8); c = Image.fromarray(a)
        p = Pen(); fn(p, k, N); ov = p.render()
        if name not in NOZOOM:
            ax, ay = FING if name in FING_EV else BADGE
            ov = ov.transform(ov.size, Image.AFFINE, (1 / ZOOM, 0, ax - ax / ZOOM, 0, 1 / ZOOM, ay - ay / ZOOM), resample=Image.BICUBIC)
        c.alpha_composite(ov); out.append(c)
    en = f'event_{name}'
    extra = {"facing": bj.get('facing', 'front'), "source": "grok-imagine-video + code icons",
             "event": DOTTED[name][0], "rule_events": DOTTED[name], "event_slug": name,
             "base_clip": bname, "base_frames": idx, "source_files": bj.get('source_files'),
             "effects": [desc], "palette": ["#e23d8a", "#c026d3", "#5fd0e6"],
             "generator": "generated_v2 base clip (Grok Imagine video) + original code-drawn icons (generated/_tools/build_events.py painters)",
             "baked_secondary_motion": bj.get('baked_secondary_motion'),
             "notes": f"System-event reaction (plays once). Body frames sampled evenly from generated_v2/{bname} (video-based, baked bounce); icon/effect drawn in code exactly as the old set: {desc}."}
    m = bc.write_clip(en, out, 10, 'once', 200, bj.get('scale'), extra, f'{V2}/{en}', en)
    bb = m['bbox_per_frame']
    print(en, N, 'base', bname, idx, 'compliant', m['canvas_compliant'], 'x', min(b[0] for b in bb), max(b[1] for b in bb), flush=True)
