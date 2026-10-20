#!/usr/bin/env python3
"""event_* clips for generated_v2, with body-tracked icons.

usage: build_events_v3.py <slug> ...   (slugs from generated/_tools/build_events.py EV or build_events2.py EV)

Body: the v2 video clip named as the old base. Old frame indices are mapped proportionally onto the
v2 clip (old_i/(old_N-1)*(new_N-1)), so holds/repeats in the old index lists survive.
Icons: the original code painters (unchanged) draw in OLD canvas coordinates; each frame's overlay is
then shifted by a per-frame tracking offset so it rides with the v2 body:
  - point-based events (FING_EV etc.): fingertip of the v2 frame minus fingertip of the old base frame
    (fingertip = farthest opaque pixel toward screen-left in the upper body band)
  - everything else ("badge"): head crown of the v2 frame minus crown of the old base frame,
    3-tap smoothed and scaled by HEAD_GAIN=0.6 (gentle: rides the bounce, ignores single-frame jitter)
  - frame-wide effects (screenshot crop corners): no tracking.
Named points (icon_origin, finger_point, ...) are written per frame in frame_points and as the frame-0 value."""
import sys, os, json, glob
import numpy as np
from PIL import Image
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import build_clip as bc
import os as _o, sys as _s; _s.path.insert(0, _o.path.dirname(_o.path.abspath(__file__))); import paths
G = paths.V1; OLDT = paths.V1_TOOLS; V2 = bc.ROOT
sys.path.insert(0, OLDT)
def load_ns(fn):
    ns = {'__name__': 'old_' + fn}
    exec(compile(open(f'{OLDT}/{fn}').read().split('\ntodo=')[0], fn, 'exec'), ns); return ns
N1 = load_ns('build_events.py'); N2 = load_ns('build_events2.py')
Pen, ease = N1['Pen'], N1['ease']
TABLE = {}
for s, (base, idx, fn, desc) in N1['EV'].items():
    zoom = s not in N1['NOZOOM']; fing = s in N1['FING_EV']
    TABLE[s] = dict(ev=N1['DOTTED'][s], base=base, idx=idx, fn=fn, desc=desc, zoom=N1['ZOOM'] if zoom else None,
                    origin=N1['FING'] if fing else N1['BADGE'], track='finger' if fing else 'head', pts={})
for s, (ev, base, idx, fn, origin, zoom, pts, desc) in N2['EV'].items():
    TABLE[s] = dict(ev=[ev], base=base, idx=idx, fn=fn, desc=desc, zoom=N2['ZOOM'] if zoom else None, origin=origin,
                    track='finger' if base == 'point' else 'head', pts=pts)
TABLE['screenshot']['track'] = None
# v2 frame lists that replace the proportional map (spell_orb_cast v2 has the orb baked in frames 8-18:
# use only orb-free hands-together frames)
SPELL_CLEAN = [20, 20, 21, 21, 22, 22, 21, 21, 22, 22]
# constant extra shift (canvas px) on top of tracking: v2 hands sit lower relative to the head than in the old clean spell pose
EXTRA_OFF = {'archiving': (0, 30), 'hotspot_on': (0, 30), 'update_available': (0, 30)}
HEAD_GAIN = 0.6
OVERRIDE = {'archiving': SPELL_CLEAN, 'hotspot_on': SPELL_CLEAN, 'update_available': SPELL_CLEAN}

def frames_of(d): return [Image.open(f).convert('RGBA') for f in sorted(glob.glob(f'{d}/*.png'))]
def crown(im):
    a = np.asarray(im)[..., 3] > 128; cnt = a.sum(1); ys = np.where(cnt >= 18)[0]
    y0 = ys.min(); band = a[y0:y0 + 14]; xs = np.where(band.any(0))[0]
    return np.array([xs.mean(), float(y0)])
def fingertip(im):
    a = np.asarray(im)[..., 3] > 128; ys = np.where(a.any(1))[0]; top = ys.min()
    sub = a[top:top + 150]; cols = np.where(sub.any(0))[0]; x = cols.min()
    yy = np.where(sub[:, x:x + 3].any(1))[0]
    return np.array([float(x), top + yy.mean()])
def smooth(v):
    v = np.asarray(v, float); p = np.vstack([v[:1], v, v[-1:]])
    return (p[:-2] + 2 * p[1:-1] + p[2:]) / 4

def build(name):
    t = TABLE[name]; base = t['base']
    if base.startswith('pose:'):
        # own-pose events: v2 base = new pose video built into _poses/pose_<name>; old reference bodies are
        # re-rendered with the old build_events2 pose_frames() (icon-free) for tracking
        _, src, kind = base.split(':'); b = '_poses/pose_' + src.replace('_01', '')
        old, _info = N2['pose_frames'](src, t['idx'], kind); idx_old = list(range(t['idx']))
    else:
        b = base.replace('clean:', '')
        old = frames_of(f'{G}/_work/clean/{b}' if base.startswith('clean:') else f'{G}/{b}/frames'); idx_old = t['idx']
    new = frames_of(f'{V2}/{b}/frames')
    if not new: raise SystemExit(f'v2 base {b} missing')
    bn = b.split('/')[-1]
    bj = json.load(open(f'{V2}/{b}/{bn}.json'))
    N = len(idx_old); Nn, No = len(new), len(old)
    if name in OVERRIDE: idx = OVERRIDE[name]
    elif name == 'incognito': idx = [0] * N
    else: idx = [int(round(i / max(No - 1, 1) * (Nn - 1))) for i in idx_old]
    # tracking offsets (canvas px)
    if t['track'] is None: off = np.zeros((N, 2))
    else:
        f = fingertip if t['track'] == 'finger' else crown
        ref = [f(old[min(i, No - 1)]) for i in idx_old]
        if name in OVERRIDE: ref = [f(old[min(i, No - 1)]) for i in range(N)]
        cur = [f(new[i]) for i in idx]
        off = np.array(cur) - np.array(ref)
        if t['track'] == 'finger':
            # arm-drop guard: frames whose tip jumps >25px from the clip median fall back to the median
            med = np.median(off, 0); bad = np.linalg.norm(off - med, axis=1) > 25; off[bad] = med
        off = smooth(off)
        if t['track'] == 'head': off = off * HEAD_GAIN   # gentle: badge rides ~60% of the head motion
    off = off + np.array(EXTRA_OFF.get(name, (0, 0)), float)
    out = []; origins = []
    for k, i in enumerate(idx):
        c = new[i].copy()
        if name == 'incognito' and k >= 4:
            a = np.asarray(c).copy(); a[..., 3] = (a[..., 3] * (1 - 0.55 * ease((k - 4) / 4))).astype(np.uint8); c = Image.fromarray(a)
        if name == 'vpn_up' and 2 <= k <= 7:
            fa = 0.72 + 0.28 * abs(np.cos((k - 2) * np.pi / 5.0)); a = np.asarray(c).copy(); a[..., 3] = (a[..., 3] * fa).astype(np.uint8); c = Image.fromarray(a)
        p = Pen(); t['fn'](p, k, N); ov = p.render()
        if t['zoom']:
            Z = t['zoom']; ax, ay = t['origin']
            ov = ov.transform(ov.size, Image.AFFINE, (1 / Z, 0, ax - ax / Z, 0, 1 / Z, ay - ay / Z), resample=Image.BICUBIC)
        dx, dy = off[k]
        if dx or dy: ov = ov.transform(ov.size, Image.AFFINE, (1, 0, -dx, 0, 1, -dy), resample=Image.BICUBIC)
        c.alpha_composite(ov); out.append(c)
        origins.append([int(round(t['origin'][0] + dx)), int(round(t['origin'][1] + dy))])
    fp = {'icon_origin': origins}
    for kk, v in t['pts'].items(): fp[kk] = [[int(round(v[0] + o[0])), int(round(v[1] + o[1]))] for o in off]
    en = f'event_{name}'
    extra = {"facing": bj.get('facing', 'front'), "source": "grok-imagine-video + code icons",
             "event": t['ev'][0], "rule_events": t['ev'], "event_slug": name, "base_clip": b, "base_frames": idx,
             "source_files": bj.get('source_files'), "effects": [t['desc']], "palette": ["#e23d8a", "#c026d3", "#5fd0e6"],
             "icon_origin": origins[0], **{kk: v[0] for kk, v in fp.items() if kk != 'icon_origin'},
             "frame_points": fp, "icon_tracking": t['track'] or 'none',
             "icon_offset_per_frame": [[round(float(x), 1), round(float(y), 1)] for x, y in off],
             "generator": "generated_v2 base clip (Grok Imagine video) + original code-drawn icons (old painters, unchanged) + per-frame body tracking (_tools/build_events_v3.py)",
             "baked_secondary_motion": bj.get('baked_secondary_motion'),
             "engine_note": bj.get('engine_note'),
             "notes": f"System-event reaction (plays once). Body frames from generated_v2/{b} (video-based, baked bounce); icon/effect drawn in code as the old set and shifted per frame to follow the body ({t['track'] or 'no'} tracking): {t['desc']}."}
    m = bc.write_clip(en, out, 10, 'once', 200, bj.get('scale'), extra, f'{V2}/{en}', en)
    bb = m['bbox_per_frame']
    print(en, N, 'base', b, idx, 'track', t['track'], 'off range x %.0f..%.0f y %.0f..%.0f' % (off[:, 0].min(), off[:, 0].max(), off[:, 1].min(), off[:, 1].max()),
          'compliant', m['canvas_compliant'], flush=True)
    return m

if __name__ == '__main__':
    for n in sys.argv[1:]: build(n)
