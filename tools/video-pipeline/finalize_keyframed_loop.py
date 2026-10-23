#!/usr/bin/env python3
"""finalize_keyframed_loop.py <clip> [--fmin 0.95] [--no-zoom-fix]

Template for keyframed loops (first=last = shared v2 base still, pose still pinned mid-clip).
Input : $LILLITH_V2/_matte_v2/<clip>/ (matte_v2 output + json).
Output: $LILLITH_V2/<clip>/ (frames, sheet, sheet_on_gray, preview gif, json tagged baked_motion).

Fixes the two artefacts the mid keyframe causes when the pose still was drawn a little
larger/lower than the base still:
 1. zoom creep: per-frame scale f = clip(H0/H(t), fmin, 1) (H = alpha bbox height, H0 = frame 0),
    3-tap smoothed, applied about (anchor_x, lowest pixel) on premultiplied RGBA.
 2. ground: per-frame snap of the lowest opaque pixel to baseline 392 (feet planted clips only).
"""
import sys, os, json, glob, argparse
import numpy as np, cv2
from PIL import Image
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import paths
from build_clip import write_clip
V2 = paths.V2

ap = argparse.ArgumentParser(); ap.add_argument('clip'); ap.add_argument('--fmin', type=float, default=0.95)
ap.add_argument('--no-zoom-fix', action='store_true'); ap.add_argument('--note', default='')
a = ap.parse_args(); c = a.clip
src = f'{V2}/_matte_v2/{c}'
js = json.load(open(f'{src}/{c}.json'))
fr = [np.asarray(Image.open(p).convert('RGBA')).astype(np.float32) for p in sorted(glob.glob(f'{src}/frames/{c}_*.png'), key=lambda q: int(q.rsplit('_', 1)[1][:-4]))]
def ext(f):
    ys = np.where((f[..., 3] > 16).any(1))[0]; return ys.min(), ys.max()
E_ = [ext(f) for f in fr]; H = np.array([b - t for t, b in E_], float); L = np.array([b for t, b in E_], float)
f = np.ones(len(fr)) if a.no_zoom_fix else np.clip(H[0] / H, a.fmin, 1.0)
if len(f) > 2:
    fp = np.r_[f[-1], f, f[0]] if js.get('loop', True) else np.r_[f[0], f, f[-1]]
    f = (fp[:-2] + 2 * fp[1:-1] + fp[2:]) / 4
ax = js.get('anchor', [200, 392])[0]; out = []; scales = []
for i, im in enumerate(fr):
    s = float(f[i]); pre = im.copy(); pre[..., :3] *= pre[..., 3:4] / 255.0
    M = np.float32([[s, 0, ax - s * ax], [0, s, 392 - s * L[i]]])
    w = cv2.warpAffine(pre, M, (400, 400), flags=cv2.INTER_LANCZOS4 if s > 0.999 else cv2.INTER_AREA,
                       borderMode=cv2.BORDER_CONSTANT, borderValue=0)
    w = np.clip(w, 0, 255); al = w[..., 3:4]
    rgb = np.where(al > 0, w[..., :3] * 255.0 / np.maximum(al, 1e-3), 0)
    o = np.dstack([np.clip(rgb, 0, 255), al]).round().astype(np.uint8)
    o[o[..., 3] < 1] = 0; o[393:] = 0
    out.append(Image.fromarray(o, 'RGBA')); scales.append(round(s, 4))
keep = {k: v for k, v in js.items() if k not in ('bbox_per_frame', 'canvas_compliant')}
keep['tags'] = sorted(set(keep.get('tags', []) + ['baked_motion']))
keep['baseline_snap'] = 'per-frame (lowest opaque pixel -> 392, finalize_keyframed_loop.py)'
keep['zoom_fix_scale_per_frame'] = scales
keep['finalize_tool'] = 'tools/video-pipeline/finalize_keyframed_loop.py'
if a.note: keep['notes'] = (keep.get('notes', '') + ' ' + a.note).strip()
write_clip(c, out, js.get('fps', 12), 'loop' if js.get('loop', True) else 'once', ax, js.get('scale', 0.54), keep, f'{V2}/{c}')
print(c, 'scale min %.3f' % min(scales), 'lows before', sorted(set(L.astype(int).tolist())))
