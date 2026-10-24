"""Heuristic sex-appeal/bounce read per clip: chest visibility + chest jiggle (high-passed vertical motion of the
bikini top) + chest-region motion. Ranking aid for reshoot candidates, not ground truth."""
import os as _os
_ROOT = _os.environ.get('CC_REPO', _os.path.abspath(_os.path.join(_os.path.dirname(__file__), '..', '..', '..')))
_OUT = _os.environ.get('CC_OUT', '/workspace/verify/clipclean')
import json, sys, numpy as np
from PIL import Image
G = _ROOT + '/assets/generated'; R = _OUT
m = json.load(open(f'{G}/manifest.json'))
out = {}
for e in m['animations']:
    if e.get('alias_of'): continue
    d = e['sheet'].split('/')[0]
    im = np.asarray(Image.open(f'{G}/{d}/{d}_sheet.png').convert('RGBA')).astype(np.int16)
    N = im.shape[1] // 400
    cys, vis, rois = [], [], []
    for i in range(N):
        c = im[:, i*400:(i+1)*400]; a = c[..., 3]
        ys = np.nonzero((a > 128).any(1))[0]
        if not len(ys): continue
        y0, y1 = ys.min(), ys.max(); h = y1 - y0
        b0, b1 = int(y0 + 0.27*h), int(y0 + 0.45*h)
        band = a[b0:b1] > 128
        xs = np.nonzero(band.any(0))[0]
        cx = int(np.median(np.nonzero(band)[1])) if band.any() else 200
        roi = c[b0:b1, max(cx-45, 0):cx+45]
        rgb = roi[..., :3]; al = roi[..., 3]
        lum = rgb.mean(-1); sat = rgb.max(-1) - rgb.min(-1)
        skin = (al > 200) & (rgb[..., 0] > 170) & (rgb[..., 0] > rgb[..., 2] + 15) & (lum > 140) & (sat < 110)
        dark = (al > 200) & (lum < 70)
        vis.append(float((skin | dark).mean()))
        yy = np.nonzero(dark)[0]
        cys.append(float(yy.mean() + b0 - y0) if len(yy) > 30 else np.nan)
        rois.append(np.where(al[..., None] > 0, rgb, 0)[:, :90] if roi.shape[1] >= 90 else None)
    cy = np.array(cys)
    ok = ~np.isnan(cy)
    if ok.sum() >= 5:
        c2 = np.interp(np.arange(len(cy)), np.nonzero(ok)[0], cy[ok])
        k = 5; pad = np.pad(c2, k//2, mode='wrap' if e.get('loop') else 'edge')
        hp = c2 - np.convolve(pad, np.ones(k)/k, 'valid')
        jig = float(np.std(hp)); turns = int((np.diff(np.sign(np.diff(c2))) != 0).sum())
    else:
        jig, turns = 0.0, 0
    md = [np.abs(rois[i+1].astype(int) - rois[i].astype(int)).mean() for i in range(len(rois)-1)
          if rois[i] is not None and rois[i+1] is not None and rois[i].shape == rois[i+1].shape]
    out[e['name']] = dict(frames=N, chest_visible=round(float(np.mean(vis)), 3), jiggle_px=round(jig, 2),
                          chest_turns=turns, chest_motion_per_frame=round(float(np.mean(md)) if md else 0, 2))
    print(e['name'], out[e['name']], flush=True)
for n, r in out.items():
    r['bounce_score'] = round(r['chest_visible'] * (r['jiggle_px'] + 0.15 * r['chest_motion_per_frame']), 3)
json.dump(out, open(f'{R}/bounce_scores.json', 'w'), indent=1)
