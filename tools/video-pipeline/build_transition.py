#!/usr/bin/env python3
"""Blend clip from a real transition video (first frame = clip A boundary frame, last = clip B boundary frame).
Placement is registered to the two clips so the blend joins them without a jump: video frame 1 is matched to
A's canvas frame and the last video frame to B's canvas frame (scale = each clip's own scale, offset from the
alpha centroids), and scale/offset are interpolated linearly in between. Interior frames only (the boundary
frames already belong to A and B).
usage: build_transition.py <blend_name> <video> <clipA> <idxA> <clipB> <idxB> [n=6] [fps=10] [--snap]"""
import sys, os, glob, json
import numpy as np
from PIL import Image
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import build_clip as bc
from key_white import key_combined
from horn_fix import fix_frames
import os as _o, sys as _s; _s.path.insert(0, _o.path.dirname(_o.path.abspath(__file__))); import paths
V2 = bc.ROOT; G = paths.V1
args = [a for a in sys.argv[1:] if not a.startswith('--')]; snap = '--snap' in sys.argv
name, video, ca, ia, cb, ib = args[:6]; ia, ib = int(ia), int(ib)
n = int(args[6]) if len(args) > 6 else 6; fps = float(args[7]) if len(args) > 7 else 10
def canvas(c, i):
    fs = sorted(glob.glob(f'{V2}/{c}/frames/*.png')); return np.asarray(Image.open(fs[i % len(fs)]).convert('RGBA')), (i % len(fs)), json.load(open(f'{V2}/{c}/{c}.json'))
A, ia, ja = canvas(ca, ia); B, ib, jb = canvas(cb, ib)
fd = bc.extract(os.path.abspath(video)); nf = len(glob.glob(f'{fd}/f_*.png'))
load = lambda i: key_combined(np.asarray(Image.open(f'{fd}/f_{i:03d}.png').convert('RGB')))
def cen(al): ys, xs = np.nonzero(al > 128); return xs.mean(), ys.mean()
def reg(kv, canv, s):
    vx, vy = cen(kv[..., 3]); cx, cy = cen(canv[..., 3]); return cx - vx * s, cy - vy * s
k1, kN = load(1), load(nf)
def hgt(al): ys = np.where((al > 128).any(1))[0]; return ys.max() - ys.min()
def scale_for(kv, canv, js):
    # clip scale when the video end frame is the clip's own source frame; else (different framing) height ratio
    est = hgt(canv[..., 3]) / hgt(kv[..., 3]); s = float(js['scale']) if not isinstance(js['scale'], str) else est
    return s if abs(est / s - 1) < 0.04 else float(est)
sa, sb = scale_for(k1, A, ja), scale_for(kN, B, jb)
txa, tya = reg(k1, A, sa); txb, tyb = reg(kN, B, sb)
sel = [int(round(x)) for x in np.linspace(1, nf, n + 2)[1:-1]]
frames = []
for i in sel:
    t = (i - 1) / (nf - 1); s = sa + (sb - sa) * t
    k = load(i); f = bc.place(k, s, txa + (txb - txa) * t, tya + (tyb - tya) * t)
    a = np.asarray(f).copy()
    ys = np.where((a[..., 3] > 0).any(1))[0]; lo = ys.max()
    if snap and 380 <= lo < 392: a = np.roll(a, 392 - lo, 0); a[:392 - lo] = 0
    a[393:] = 0; frames.append(Image.fromarray(a))
frames, horn = fix_frames(frames, Image.fromarray(A))
old = json.load(open(f'{G}/{name}/{name}.json'))
rel = os.path.relpath(os.path.abspath(video), f'{V2}/{name}')
extra = {'from_clip': ca, 'to_clip': cb, 'source_frames': {ca: ia, cb: ib}, 'facing': old.get('facing'),
         'source_files': [rel], 'source_video_frames': sel, 'horn_fix': horn,
         'blend_method': 'real transition video (Grok Imagine I2V, first frame = clip A boundary frame, last = clip B boundary frame); placement registered to both clips, scale/offset interpolated; interior frames only',
         'scale_from_to': [sa, sb], 'baked_secondary_motion': ['full_body_bounce', 'pigtails', 'tail'],
         'notes': f'Transition helper {ca.replace("idle_a", "idle")} -> {cb.replace("idle_a", "idle")}: {n} frames from a dedicated transition video between {ca}[{ia}] and {cb}[{ib}]. idle endpoints use idle_a frame 0 (= idle_b frame 0 = idle still).'}
m = bc.write_clip(name, frames, fps, 'once', 200, round((sa + sb) / 2, 3), extra, f'{V2}/{name}', None)
print(name, len(frames), 'video frames', sel, 'scale', sa, '->', sb, 'compliant', m['canvas_compliant'])
