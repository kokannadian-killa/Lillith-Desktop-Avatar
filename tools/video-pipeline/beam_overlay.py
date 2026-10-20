"""Re-add the beam to generated_v2/beam_cast (the keyer removes the video's bright beam).
Detects beam frames in the raw video, finds the hand tip per frame (frontmost opaque
pixel of the arm band), draws the old code beam (fx.beam/glow_orb) from there to x=393,
recomputes beam_origin, rewrites sheet/gif/json via build_clip.write_clip."""
import sys, os, json, glob
import numpy as np
from PIL import Image
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import os as _o, sys as _s; _s.path.insert(0, _o.path.dirname(_o.path.abspath(__file__))); import paths
sys.path.insert(0, paths.V1_TOOLS)
from fx import beam, glow_orb, sparkles, ICE, WHITE
import build_clip as bc
D = os.path.join(bc.ROOT, 'beam_cast'); j = json.load(open(f'{D}/beam_cast.json'))
r = j['source_frame_range']; idx = list(range(r['first'], r['last'] + 1, r['step']))
src = '/tmp/lillith_frames/beam_cast_v3_a'
def beam_px(i):
    a = np.asarray(Image.open(f'{src}/f_{i:03d}.png').convert('RGB')).astype(int)
    reg = a[:, 900:]
    mag = (reg[..., 0] > 150) & (reg[..., 1] < 150) & (reg[..., 2] > 150)
    cy = (reg[..., 2] > 180) & (reg[..., 0] < 170)
    return int((mag | cy).sum())
bp = [beam_px(i) for i in idx]
on = [b > 2000 for b in bp]
frames = [Image.open(p).convert('RGBA') for p in sorted(glob.glob(f'{D}/frames/*.png'))]
rng = np.random.default_rng(3); tips = []; out = []
for k, (f, o) in enumerate(zip(frames, on)):
    al = np.asarray(f)[..., 3] > 128
    ys, xs = np.where(al); top, bot = ys.min(), ys.max(); H = bot - top
    band = al[int(top + 0.12 * H):int(top + 0.40 * H)]
    by, bx = np.where(band)
    tip = (int(bx.max()), int(np.median(by[bx >= bx.max() - 3]) + top + 0.12 * H))
    tips.append(tip)
    c = f
    if o:
        strength = min(1.0, bp[k] / 20000)
        w = 6 + 6 * strength
        c = beam(c, tip[0] - 2, tip[1], 393, w, flick=np.sin(k * 2.3), alpha=0.75 + 0.25 * strength)
        c = glow_orb(c, tip[0], tip[1], w * 0.8, 1.0)
        c = sparkles(c, [(rng.uniform(tip[0] + 10, 388), tip[1] + rng.uniform(-w, w), rng.uniform(0.6, 1.2)) for _ in range(4)], 3, WHITE)
    out.append(c)
beam_tips = [t for t, o in zip(tips, on) if o]
origin = [int(np.median([t[0] for t in beam_tips])), int(np.median([t[1] for t in beam_tips]))] if beam_tips else tips[-1]
extra = {k: v for k, v in j.items() if k not in ('frame_count', 'bbox_per_frame', 'canvas_compliant')}
extra.update({"beam_origin": origin, "frame_points": {"beam_origin": [list(t) for t in tips]}, "beam_frames": [k for k, o in enumerate(on) if o],
              "effects": ["beam (code-drawn, old fx.beam palette) on the frames where the video fires; the video's own beam is removed by keying"]})
m = bc.write_clip('beam_cast', out, j['fps'], 'once', j['anchor'][0], j['scale'], extra, D, 'beam_cast')
print('beam frames', extra['beam_frames'], 'origin', origin, 'compliant', m['canvas_compliant'])
