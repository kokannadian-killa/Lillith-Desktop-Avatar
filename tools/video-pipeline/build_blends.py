#!/usr/bin/env python3
"""12 blend_* transition helpers for generated_v2, connecting the v2 clips.

Crossfade-free: every output pixel comes from ONE endpoint frame, pose-interpolated with dense optical
flow (Farneback on the gray-composited RGB + alpha). In-between t<0.5 = A warped forward by t,
t>=0.5 = B warped back by (1-t). No alpha dissolve. Lowest opaque row is interpolated between the
endpoints' lowest rows and snapped (keeps boots on y=392 for standing pairs).
blend_wake_to_sleep uses real pose frames instead: the v2 wake clip played backwards (5 frames,
old frame durations), since wake already contains the full stand<->sit motion.
usage: build_blends.py [blend names...]   (default all 12)"""
import sys, os, glob, json
import numpy as np, cv2
from PIL import Image
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import build_clip as bc
import os as _o, sys as _s; _s.path.insert(0, _o.path.dirname(_o.path.abspath(__file__))); import paths
V2 = bc.ROOT; G = paths.V1
def fr(c): return [np.asarray(Image.open(f).convert('RGBA')) for f in sorted(glob.glob(f'{V2}/{c}/frames/*.png'))]
IDLE = 'idle_a'
# name -> (A clip, A frame, B clip, B frame, n in-betweens)
B = {
 'blend_idle_to_walk_right': (IDLE, 0, 'walk_right', 0, 4), 'blend_walk_right_to_idle': ('walk_right', 0, IDLE, 0, 4),
 'blend_idle_to_walk_left': (IDLE, 0, 'walk_left', 0, 4), 'blend_walk_left_to_idle': ('walk_left', 0, IDLE, 0, 4),
 'blend_idle_to_wave': (IDLE, 0, 'wave', 0, 4), 'blend_wave_to_idle': ('wave', -1, IDLE, 0, 4),
 'blend_idle_to_sit_swing': (IDLE, 0, 'sit_swing', 0, 4), 'blend_sit_swing_to_idle': ('sit_swing', -1, IDLE, 0, 4),
 'blend_sleep_to_wake': ('sleep', -1, 'wake', 0, 3),
 'blend_idle_to_knife_tap': (IDLE, 0, 'knife_tap', 0, 4), 'blend_knife_tap_to_idle': ('knife_tap', -1, IDLE, 0, 4),
 'blend_wake_to_sleep': None,
}
def comp(a):
    al = a[..., 3:4].astype(np.float32) / 255
    g = (a[..., :3].astype(np.float32) * al + 128 * (1 - al)).astype(np.uint8)
    return cv2.cvtColor(g, cv2.COLOR_RGB2GRAY)
def flow(a, b):
    return cv2.calcOpticalFlowFarneback(comp(a), comp(b), None, 0.5, 5, 31, 5, 7, 1.5, 0)
def warp(img, f, t):
    h, w = f.shape[:2]; gx, gy = np.meshgrid(np.arange(w, dtype=np.float32), np.arange(h, dtype=np.float32))
    pm = img.astype(np.float32); pm[..., :3] *= pm[..., 3:4] / 255  # premultiplied resample (no colour fringes)
    o = cv2.remap(pm, gx - t * f[..., 0], gy - t * f[..., 1], cv2.INTER_LINEAR, borderMode=cv2.BORDER_CONSTANT, borderValue=0)
    a = o[..., 3:4]; o[..., :3] = np.where(a > 0, o[..., :3] * 255 / np.maximum(a, 1e-3), 0)
    return np.clip(o, 0, 255).astype(np.uint8)
def low(a): return np.where((a[..., 3] > 128).any(1))[0].max()
def snap(a, target):
    d = int(round(target - low(a)));
    if d == 0: return a
    o = np.zeros_like(a)
    if d > 0: o[d:] = a[:-d]
    else: o[:d] = a[-d:]
    return o
def build(name):
    if B[name] is None:
        w = fr('wake'); wj = json.load(open(f'{V2}/wake/wake.json')); n = len(w)
        idx = [int(round(x)) for x in np.linspace(n - 2, 1, 5)]
        out = [Image.fromarray(w[i]) for i in idx]
        old = json.load(open(f'{G}/{name}/{name}.json'))
        extra = {'from_clip': 'wake', 'to_clip': 'sleep', 'blend_method': 'v2 wake frames played backwards (real poses, no morph, no dissolve)',
                 'source_frames': {'wake': idx}, 'frame_durations_ms': old.get('frame_durations_ms'), 'facing': old.get('facing'),
                 'notes': 'Lying-down helper wake(end = idle pose) -> sleep: v2 wake clip frames in reverse (stand -> kneel -> sit), old durations.'}
        return bc.write_clip(name, out, 8, 'once', 200, wj.get('scale'), extra, f'{V2}/{name}', None), idx
    ca, ia, cb, ib, n = B[name]
    A = fr(ca)[ia]; Bf = fr(cb)[ib]
    fab, fba = flow(A, Bf), flow(Bf, A); la, lb = low(A), low(Bf)
    out = []
    for k in range(n):
        t = (k + 1) / (n + 1)
        im = warp(A, fab, t) if t < 0.5 else warp(Bf, fba, 1 - t)
        im = snap(im, min(392, la + (lb - la) * t)); im[393:] = 0  # no soft resample tail below the baseline
        out.append(Image.fromarray(im))
    old = json.load(open(f'{G}/{name}/{name}.json'))
    extra = {'from_clip': ca, 'to_clip': cb, 'source_frames': {ca: ia, cb: ib}, 'facing': old.get('facing'),
             'blend_method': 'dense optical flow pose interpolation, crossfade-free (t<0.5 warped A, t>=0.5 warped B), lowest row interpolated',
             'notes': f'Transition helper {ca.replace("idle_a", "idle")} -> {cb.replace("idle_a", "idle")}: {n} in-between frames between the v2 boundary frames ({ca}[{ia}] -> {cb}[{ib}]). idle endpoints use idle_a frame 0 = idle_b frame 0 = idle still, so it connects both idle variants.'}
    js = json.load(open(f'{V2}/{cb}/{cb}.json'))
    return bc.write_clip(name, out, 12, 'once', 200, js.get('scale'), extra, f'{V2}/{name}', None), (ca, ia, cb, ib)
if __name__ == '__main__':
    for nme in (sys.argv[1:] or list(B)):
        m, info = build(nme); print(nme, m['frame_count'], info, 'compliant', m['canvas_compliant'], flush=True)
