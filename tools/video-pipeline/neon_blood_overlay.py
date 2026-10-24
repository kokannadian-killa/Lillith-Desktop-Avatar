#!/usr/bin/env python3
"""Neon "blood" splatter FX overlay for a knife clip (stylized cyan/magenta, not gory).

Usage:
  neon_blood_overlay.py IN_CLIP_DIR OUT_DIR [options]
  e.g. neon_blood_overlay.py ../knife_tap ../_fx_test/knife_tap_neon_blood

Reads IN_CLIP_DIR/<name>.json + frames/*.png (400x400 RGBA, baseline 392, same
conventions as build_clip.py), writes to OUT_DIR:
  frames/<out>_NN.png   FX composited with the clip (character on top by default,
                        so every opaque character pixel is byte-identical)
  fx_only/<out>_NN.png  FX layer alone on transparent (separate overlay layer)
  <out>_sheet.png       horizontal strip of frames/ (same layout as source)
  <out>.json            copy of the source JSON + "fx" block (impacts, contact points, params)
  preview.gif           frames on 50% gray at --fps

Tip tracking follows beam_overlay.py: alpha>128 mask, restrict to a vertical band
of the body bbox (top .. top+--band*H, which excludes the feet), take the
frontmost (max-x) opaque pixel; tip y = median y of pixels within 2px of that x.
Impact frames: the clip JSON is checked first for explicit events
("impact_frames", "contact_frames", "events" with type impact/contact). Otherwise an
impact is a circular local maximum of tip x that comes within --near px of the
"window edge". The edge is --edge-x if given, else the clip's maximum tip x
(the v2 knife never reaches the 400px cell border; it peaks ~52px short).
--impact-frames overrides everything.

The splat hugs the vertical edge plane at x = edge: nothing is drawn past it,
the burst is flattened along it, droplets spray back toward the character, and
2-4 drips run down the edge. Each impact lasts --fade frames (wrapping around
for looping clips). All randomness is seeded (--seed), so output is deterministic.
"""
import argparse, glob, json, os, sys
import numpy as np
from PIL import Image

def hexrgb(s):
    s = s.lstrip('#'); return np.array([int(s[i:i + 2], 16) for i in (0, 2, 4)], float) / 255.0

def track_tips(frames, band):
    tips = []
    for f in frames:
        al = np.asarray(f)[..., 3] > 128
        ys, xs = np.where(al); top, bot = ys.min(), ys.max(); H = bot - top
        y1 = int(top + band * H)
        by, bx = np.where(al[top:y1])
        xm = int(bx.max())
        tips.append((xm, int(np.median(by[bx >= xm - 2]) + top)))
    return tips

def json_impacts(j):
    for k in ('impact_frames', 'contact_frames'):
        if k in j: return [int(v) for v in j[k]], f'json:{k}'
    ev = j.get('events')
    if isinstance(ev, list):
        fr = [int(e['frame']) for e in ev if isinstance(e, dict) and str(e.get('type', '')).lower() in ('impact', 'contact', 'hit')]
        if fr: return fr, 'json:events'
    return None, None

def detect(tips, edge_x, near, loop):
    xs = [t[0] for t in tips]; N = len(xs); out = []
    for k in range(N):
        p = xs[(k - 1) % N] if (loop or k > 0) else -1
        n = xs[(k + 1) % N] if (loop or k < N - 1) else -1
        if xs[k] >= p and xs[k] > n and edge_x - xs[k] <= near: out.append(k)
    return out

class Light:
    """Additive light buffer (linear RGB); converted to straight-alpha RGBA at the end."""
    def __init__(s, W, H): s.W, s.H = W, H; s.b = np.zeros((H, W, 3)); s.yy, s.xx = np.mgrid[0:H, 0:W].astype(float)
    def blob(s, cx, cy, rx, ry, col, inten, soft=1.0):
        d2 = ((s.xx - cx) / max(rx, .5)) ** 2 + ((s.yy - cy) / max(ry, .5)) ** 2
        core = np.clip(1.2 - d2, 0, 1)                       # hard-ish core
        glow = np.exp(-d2 / (2.0 * soft * soft)) * 0.55       # soft halo
        s.b += (core + glow)[..., None] * col * inten
    def rgba(s, edge, pixel, scan):
        b = s.b.copy(); b[:, edge + 1:] = 0; b[393:] = 0
        if scan: b[1::2] *= scan
        if pixel > 1:  # pixel flavor: block-average then re-expand
            H2, W2 = s.H // pixel * pixel, s.W // pixel * pixel
            q = b[:H2, :W2].reshape(H2 // pixel, pixel, W2 // pixel, pixel, 3).mean((1, 3))
            b[:H2, :W2] = np.repeat(np.repeat(q, pixel, 0), pixel, 1)
        b = 1 - np.exp(-b * 1.6)                              # filmic clamp keeps highlights in gamut
        a = np.clip(b.max(-1), 0, 1); c = np.where(a[..., None] > 1e-4, b / np.maximum(a, 1e-4)[..., None], 0)
        out = np.dstack([np.clip(c, 0, 1) * 255, a * 255]).round().astype(np.uint8)
        out[out[..., 3] == 0] = 0
        return Image.fromarray(out, 'RGBA')

def splat_spec(rng, size, drips):
    """Per-impact random layout (fixed for the impact's lifetime -> no flicker)."""
    drops = [(rng.uniform(0.6, 2.6) * size, rng.uniform(-1.25, 1.25), rng.uniform(0.10, 0.32) * size, rng.random() < .5) for _ in range(9)]
    xs = np.linspace(0.25, 1.9, max(drips, 1)) + rng.uniform(-0.12, 0.12, max(drips, 1))
    dr = [(rng.uniform(-0.2, 0.8) * size, rng.uniform(0.16, 0.24) * size, rng.uniform(1.8, 3.4) * size, k % 2 == 0, xs[k] * size) for k in range(drips)]
    spikes = [(ang, rng.uniform(1.1, 1.9) * size) for ang in np.linspace(-1.35, 1.35, 6) + rng.uniform(-0.15, 0.15, 6)]
    return drops, dr, spikes

def draw_splat(L, cx, cy, edge, age, fade, size, cyan, mag, spec):
    drops, dr, spikes = spec
    t = age / max(fade, 1)                     # 0 at impact -> (fade-1)/fade on the last visible frame
    I = (1 - t) ** 1.3                         # fade envelope
    if age == 0:                               # impact flash
        L.blob(edge - 1, cy, size * 0.45, size * 0.9, np.array([1, 1, 1.0]), 0.8, 1.4)
    # radial splash spikes (short streaks fanning back from the contact point)
    for ang, ln in spikes:
        reach = ln * (0.7 + 0.3 * t)
        for k in np.linspace(0.2, 1, 6):
            x = edge - np.cos(ang) * reach * k; y = cy + np.sin(ang) * reach * k * 1.1
            r = size * 0.16 * (1.1 - 0.6 * k)
            L.blob(x, y, r, r, mag if k < 0.6 else cyan, 0.75 * I * (1.15 - 0.5 * k) * (1 - 0.5 * t), 1.0)
    # burst flattened against the edge plane (wider along the wall than off it)
    L.blob(edge - size * 0.25, cy, size * (0.45 + 0.15 * t), size * (0.95 + 0.25 * t), mag, 0.9 * I, 1.4)
    L.blob(edge - size * 0.15, cy, size * 0.28, size * 0.60, cyan, 0.9 * I, 1.2)
    # droplets sprayed back toward the character, drifting out a little and down (gravity)
    for dist, ang, r, c in drops:
        d = dist * (0.75 + 0.35 * t)
        x = edge - abs(np.cos(ang)) * d; y = cy + np.sin(ang) * d * 0.9 + 6 * t * t
        L.blob(x, y, r * (1 - 0.3 * t), r * (1 - 0.3 * t), cyan if c else mag, 1.1 * I, 1.3)
    # drips run down the edge, growing over the fade
    for dy0, w, ln, c, dx in dr:
        top = cy + dy0; length = ln * (0.25 + 0.75 * min(1, t * 1.4 + 0.15))
        x = edge - dx
        for k in np.linspace(0, 1, 10):
            L.blob(x, top + k * length, w * (1 - 0.35 * k), w * 0.9, mag if c else cyan, 0.75 * I * (1 - 0.3 * k), 1.0)
        L.blob(x, top + length, w * 1.25, w * 1.35, cyan if c else mag, 0.9 * I, 1.2)   # bead at the drip end

def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('in_dir'); ap.add_argument('out_dir')
    ap.add_argument('--name', help='output name (default <clip>_neon_blood)')
    ap.add_argument('--color-a', default='#00f6ff', help='cyan component (hex)')
    ap.add_argument('--color-b', default='#ff2bd6', help='magenta component (hex)')
    ap.add_argument('--size', type=float, default=11.0, help='splat radius px (character ~370px tall)')
    ap.add_argument('--fade', type=int, default=5, help='frames each impact stays visible (4-6 typical)')
    ap.add_argument('--drips', type=int, default=3, help='drip trails per impact (2-4)')
    ap.add_argument('--seed', type=int, default=217)
    ap.add_argument('--edge-x', type=int, help='x of the window edge plane (default: max tracked tip x)')
    ap.add_argument('--near', type=int, default=8, help='tip within N px of the edge counts as contact')
    ap.add_argument('--band', type=float, default=0.62, help='tip search band (fraction of bbox height from top; excludes feet)')
    ap.add_argument('--impact-frames', help='comma list, overrides detection')
    ap.add_argument('--pixel', type=int, default=2, help='pixel-block size for digital look (1 = off)')
    ap.add_argument('--scanline', type=float, default=0.72, help='odd-row gain (1 = off)')
    ap.add_argument('--fx-over', action='store_true', help='draw FX over the character (default: behind, character pixels untouched)')
    ap.add_argument('--gif-scale', type=float, default=1.0)
    a = ap.parse_args(argv)

    src = json.load(open(glob.glob(os.path.join(a.in_dir, '*.json'))[0]))
    clip = src['name']; name = a.name or f'{clip}_neon_blood'
    paths = sorted(glob.glob(os.path.join(a.in_dir, src.get('frames_dir', 'frames/'), '*.png')))
    frames = [Image.open(p).convert('RGBA') for p in paths]; N = len(frames); W, H = frames[0].size
    loop = bool(src.get('loop', True))
    tips = track_tips(frames, a.band)
    edge = a.edge_x if a.edge_x is not None else max(t[0] for t in tips)
    if a.impact_frames:
        imp, how = [int(v) for v in a.impact_frames.split(',')], 'cli:--impact-frames'
    else:
        imp, how = json_impacts(src)
        if imp is None: imp, how = detect(tips, edge, a.near, loop), f'tip-x local max within {a.near}px of edge_x={edge}'
    cyan, mag = hexrgb(a.color_a), hexrgb(a.color_b)
    rng = np.random.default_rng(a.seed)
    specs = {k: splat_spec(rng, a.size, a.drips) for k in imp}
    fx_frames, comp, active = [], [], []
    for n in range(N):
        L = Light(W, H); act = []
        for k in imp:
            age = (n - k) % N if loop else n - k
            if 0 <= age < a.fade:
                draw_splat(L, tips[k][0], tips[k][1], min(edge, tips[k][0] + 1), age, a.fade, a.size, cyan, mag, specs[k]); act.append([k, age])
        fx = L.rgba(min(edge, max(tips[k][0] for k in imp) + 1) if imp else edge, a.pixel, a.scanline if a.scanline < 1 else 0)
        if a.fx_over: c = frames[n].copy(); c.alpha_composite(fx)
        else: c = fx.copy(); c.alpha_composite(frames[n])
        fx_frames.append(fx); comp.append(c); active.append(act)

    # verify: opaque character pixels untouched, nothing at/below baseline+1 added
    for n in range(N):
        s = np.asarray(frames[n]); o = np.asarray(comp[n]); f = np.asarray(fx_frames[n])
        if not a.fx_over: assert (o[s[..., 3] == 255] == s[s[..., 3] == 255]).all(), f'char pixels changed f{n}'
        assert f[393:, :, 3].max() == 0, f'fx below baseline f{n}'

    os.makedirs(f'{a.out_dir}/frames', exist_ok=True); os.makedirs(f'{a.out_dir}/fx_only', exist_ok=True)
    for n in range(N):
        comp[n].save(f'{a.out_dir}/frames/{name}_{n:02d}.png'); fx_frames[n].save(f'{a.out_dir}/fx_only/{name}_fx_{n:02d}.png')
    sheet = Image.new('RGBA', (W * N, H), (0, 0, 0, 0))
    for n, c in enumerate(comp): sheet.paste(c, (W * n, 0))
    sheet.save(f'{a.out_dir}/{name}_sheet.png')
    gs = max(1, int(W * a.gif_scale)); gif = []
    for c in comp:
        b = Image.new('RGBA', (W, H), (128, 128, 128, 255)); b.alpha_composite(c)
        gif.append(b.convert('RGB').resize((gs, gs), Image.LANCZOS) if gs != W else b.convert('RGB'))
    gif[0].save(f'{a.out_dir}/preview.gif', save_all=True, append_images=gif[1:], duration=int(round(1000 / src.get('fps', 12))), loop=0 if loop else 1)

    def bb(im):
        al = np.asarray(im)[..., 3] > 0
        if not al.any(): return None
        ys, xs = np.where(al); return [int(xs.min()), int(xs.max()), int(ys.min()), int(ys.max())]
    meta = dict(src); meta.update({"name": name, "sheet": f"{name}_sheet.png", "frames_dir": "frames/", "preview": "preview.gif",
        "derived_from": clip,
        "fx": {"type": "neon_blood_splat", "layer_dir": "fx_only/", "composite": "fx_over_character" if a.fx_over else "fx_behind_character",
               "impact_frames": imp, "impact_detection": how, "edge_x": int(edge),
               "contact_points": {str(k): list(tips[k]) for k in imp},
               "active_per_frame": active, "fx_bbox_per_frame": [bb(f) for f in fx_frames],
               "frame_points": {"knife_tip": [list(t) for t in tips]},
               "params": {"color_a": a.color_a, "color_b": a.color_b, "size": a.size, "fade_frames": a.fade, "drips": a.drips,
                          "seed": a.seed, "near": a.near, "band": a.band, "pixel": a.pixel, "scanline": a.scanline},
               "generator": "tools/video-pipeline/neon_blood_overlay.py"}})
    json.dump(meta, open(f'{a.out_dir}/{name}.json', 'w'), indent=2)
    print('tips', tips); print('edge_x', edge, 'impacts', imp, '(', how, ')'); print('active', active)

if __name__ == '__main__':
    main()
