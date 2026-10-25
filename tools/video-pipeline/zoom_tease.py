#!/usr/bin/env python3
"""zoom_tease.py: raw Grok Imagine take -> 400x400 clip with a baked, eased "cleavage zoom" during the peak.

  zoom_tease.py <clip> --video RAW.mp4 [--take a] [--start 1 --stop 289 --step 2] [--scale 0.54]
                [--zmax 1.8] --focus-src X,Y [--target 200,195] --zoom K0,K1,K2,K3[,K0',K1',K2',K3'...] [--snap] [--facing TEXT]

Replaces build_clip.py + matte_v2.py + finalize for zoom clips, so the matte is pulled ONCE at the full
1280x720 source resolution (matte_v2 functions: isnet-anime soft matte, backdrop-plate edge alpha, flow-
compensated temporal edge median, colour decontamination, speck drop) and every frame is then resampled
straight from that full-res premultiplied RGBA:

  normal framing (z=1): canvas = S*src + t          (S = --scale, t = fixed x like build_clip --center fixed,
                                                     y = lowest opaque px -> baseline 392, per frame if --snap)
  zoomed (z>1):         canvas = C + z*(S*src + t - C) + e*(T - C),   e = (z-1)/(zmax-1)

C = --focus-src mapped to normal-framing canvas px (her chest), T = --target (where the chest sits at full zoom).
z(k) = 1 until K0, smoothstep ease-in K0->K1, hold zmax K1->K2, smoothstep ease-out K2->K3, 1 after, so the
first and last frames are exactly the normal framing (same canvas, anchor, baseline as every other clip).
At zmax <= 1/S the source is never upsampled (0.54 * 1.8 = 0.97 source px per canvas px), and the downscale is a
premultiplied INTER_AREA resize. Rows >= 393 stay clear (baseline rule); where the zoom crops her at the canvas
sides the cut edge is fully opaque. Output: $LILLITH_V2/<clip>/ (frames, sheet, sheet_on_gray, gif, json),
ready for new_pose.py --from copy.
"""
import argparse, glob, json, os, subprocess, sys, shutil
import numpy as np, cv2
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import paths
import matte_v2 as mv
from build_clip import body_x

ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('clip'); ap.add_argument('--video', required=True); ap.add_argument('--take', default='a')
ap.add_argument('--start', type=int, default=1); ap.add_argument('--stop', type=int, default=289)
ap.add_argument('--step', type=int, default=2); ap.add_argument('--fps', type=float, default=12)
ap.add_argument('--scale', type=float, default=0.54); ap.add_argument('--anchor-x', type=int, default=200)
ap.add_argument('--zmax', type=float, default=1.8)
ap.add_argument('--focus-src', required=True, help='chest X,Y in source video px'); ap.add_argument('--target', default='200,195')
ap.add_argument('--zoom', required=True, help='K0,K1,K2,K3 output-frame indices')
ap.add_argument('--snap', action='store_true', help='per-frame baseline snap (feet planted)')
ap.add_argument('--facing', default='front'); ap.add_argument('--threads', type=int, default=2)
ap.add_argument('--tspan', type=int, default=2)
ap.add_argument('--center-bbox', action='store_true', help='x: centre of the frame-0 alpha bbox (lying poses) instead of body_x')
a = ap.parse_args()
c = a.clip; S = a.scale
Csrc = np.array([float(v) for v in a.focus_src.split(',')]); T = np.array([float(v) for v in a.target.split(',')])
KS = [int(v) for v in a.zoom.split(',')]
assert len(KS) % 4 == 0 and KS == sorted(KS), '--zoom needs groups of 4 ascending keyframes (one group per zoom cycle)'
CYCLES = [KS[j:j + 4] for j in range(0, len(KS), 4)]
K0, K1, K2, K3 = CYCLES[0]

fd = f'/tmp/zoom_tease_cache/{c}_{a.take}'
if not glob.glob(f'{fd}/f_*.png'):
    os.makedirs(fd, exist_ok=True)
    subprocess.run(['ffmpeg', '-v', 'error', '-nostdin', '-y', '-i', os.path.abspath(a.video), f'{fd}/f_%03d.png'], check=True)
nvid = len(glob.glob(f'{fd}/f_*.png'))
idx = list(range(a.start, min(a.stop, nvid + 1), a.step))
N = len(idx)
load = lambda i: cv2.cvtColor(cv2.imread(f'{fd}/f_{i:03d}.png'), cv2.COLOR_BGR2RGB)
def mask(i):
    p = f'{fd}/m_{i:03d}.png'
    if os.path.exists(p): return cv2.imread(p, cv2.IMREAD_GRAYSCALE).astype(np.float32) / 255
    m = mv.isnet(load(i), a.threads); cv2.imwrite(p, (m * 255).round().astype(np.uint8)); return m
cache = {}
def alpha_of(i):
    if i not in cache:
        rgb = load(i); A, aux = mv.raw_alpha(rgb, mask(i)); cache[i] = (A, aux, cv2.cvtColor(rgb, cv2.COLOR_RGB2GRAY))
    return cache[i]

def zoom_of(k):
    def ss(u): u = min(max(u, 0.0), 1.0); return u * u * (3 - 2 * u)
    u = 0.0
    for K0, K1, K2, K3 in CYCLES:
        if k <= K0 or k >= K3: continue
        elif k < K1: u = ss((k - K0) / (K1 - K0))
        elif k <= K2: u = 1.0
        else: u = 1 - ss((k - K2) / (K3 - K2))
    return 1 + (a.zmax - 1) * u, u

WHITE_TRIM = os.environ.get('LILLITH_WHITE_TRIM') == '1'
def ivory(f):
    """white-trim mode: white outfit parts (Santa fur) -> warm ivory so the backdrop rules (neutral white = backdrop,
    sat <= 40) never key or flag them; max channel kept, so shading is unchanged."""
    rgb = f[..., :3].astype(np.int32); mn = rgb.min(-1); v = rgb.max(-1)
    m = (f[..., 3] > 0) & (mn >= 170) & (v - mn < 44)
    out = f.copy(); out[..., 0][m] = v[m]; out[..., 1][m] = np.clip(v[m] - 12, 0, 255); out[..., 2][m] = np.clip(v[m] - 44, 0, 255)
    return out
full = []  # full-res straight RGBA per output frame (uint8)
for k, i in enumerate(idx):
    A, aux, g = alpha_of(i)
    nb = [j for j in range(i - a.tspan, i + a.tspan + 1) if j != i and 1 <= j <= nvid]
    nbs = [alpha_of(j) for j in nb]
    A = mv.temporal(A, g, [n[0] for n in nbs], [n[2] for n in nbs], aux['band'])
    fr = mv.drop_specks(mv.colour(A, aux))
    if WHITE_TRIM: fr = ivory(fr)
    full.append(fr)
    for j in list(cache):
        if j < i - a.tspan: del cache[j]
    print(f'  {c} matte {k + 1}/{N} (video {i})', flush=True)

H, W = full[0].shape[:2]
if a.center_bbox:
    xs = np.where((full[0][..., 3] > 128).any(0))[0]; cx_src = (xs.min() + xs.max()) / 2
else:
    cx_src = body_x(full[0][..., 3])
tx = int(round(a.anchor_x - cx_src * S))
def low(f):
    ys = np.where((f[..., 3] > 128).any(1))[0]; return int(ys.max())
lows = np.array([low(f) for f in full], float)
if a.snap:
    lp = np.r_[lows[-1], lows, lows[0]]; lows = (lp[:-2] + 2 * lp[1:-1] + lp[2:]) / 4
else:
    lows[:] = lows[0]
tys = [int(np.floor(392 - (l + 1) * S + 1)) for l in lows]
C = S * Csrc + np.array([tx, tys[0]], float)  # chest in normal-framing canvas px

out = []; zs = []
for k, f in enumerate(full):
    z, e = zoom_of(k); zs.append(round(z, 4))
    s = S * z
    pm = f.astype(np.float32) / 255.0; pm[..., :3] *= pm[..., 3:]
    nw, nh = int(round(W * s)), int(round(H * s))
    r = cv2.resize(pm, (nw, nh), interpolation=cv2.INTER_AREA)
    t = np.array([tx, tys[k]], float)
    o = z * t + C * (1 - z) + e * (T - C)
    if z == 1.0: o = np.round(o)
    M = np.float32([[1, 0, o[0]], [0, 1, o[1]]])
    q = cv2.warpAffine(r, M, (400, 400), flags=cv2.INTER_NEAREST if z == 1.0 else cv2.INTER_LINEAR,
                       borderMode=cv2.BORDER_CONSTANT, borderValue=0)
    al = q[..., 3:]
    col = np.where(al > 1e-4, q[..., :3] / np.maximum(al, 1e-4), 0)
    u8 = np.clip(np.round(np.dstack([col, al]) * 255), 0, 255).astype(np.uint8)
    u8[u8[..., 3] == 0] = 0; u8[393:] = 0
    out.append(u8)

rel = os.path.relpath(os.path.abspath(a.video), os.path.join(paths.V2, c))
js = {'name': c, 'placeholder': False, 'frame_count': N, 'frame_width': 400, 'frame_height': 400,
      'canvas_w': 400, 'canvas_h': 400, 'layout': 'horizontal_strip', 'sheet': f'{c}_sheet.png', 'frames_dir': 'frames/',
      'fps': a.fps, 'loop': True, 'baseline_y': 392, 'anchor': [200, 392], 'style': 'canonical_ko (thigh-highs + shorts)',
      'source': 'grok-imagine-video', 'scale': S, 'body_anchor_x': 200, 'facing': a.facing,
      'source_files': [rel], 'source_frame_range': {'first': idx[0], 'last': idx[-1], 'step': a.step, 'video_fps': 24,
                                                     'note': '1-based ffmpeg frame numbers'},
      'generator': 'Grok Imagine image-to-video (GenerateVideo, 720p, first=last frame = pose still) + tools/video-pipeline/zoom_tease.py',
      'matte_version': mv.VERSION + ' (full-res, single pass)', 'matte_tool': 'tools/video-pipeline/zoom_tease.py (matte_v2 functions)',
      'centering': 'fixed', 'baseline_snap': 'per-frame (smoothed)' if a.snap else 'fixed (frame 0)',
      'cleavage_zoom': {'zmax': a.zmax, 'focus_canvas_xy': C.tolist(), 'target_canvas_xy': T.tolist(),
                        'keyframes_ease_in_hold_out': CYCLES, 'cycles': len(CYCLES), 'easing': 'smoothstep',
                        'source_px_per_canvas_px_at_zmax': round(1 / (S * a.zmax), 3), 'zoom_per_frame': zs,
                        'note': 'baked progressive crop+scale from the full-res matte; first/last frames at normal framing'},
      'baked_secondary_motion': ['breathing', 'sway', 'pigtails', 'tail', 'chest_follow'],
      'engine_note': 'v2 bakes secondary motion into the pixels. Mute or strongly reduce engine chest/pigtail springs and breathing for this clip, or the motion doubles.',
      'tags': ['baked_motion']}
outdir = os.path.join(paths.V2, c)
os.makedirs(outdir, exist_ok=True)
js = mv.write_outputs(c, out, js, outdir, int(round(1000 / a.fps)))
print(c, 'frames', N, 'tx', tx, 'ty', tys[0], 'zmax', a.zmax, 'canvas_compliant', js['canvas_compliant'])
shutil.rmtree(fd, ignore_errors=True)
