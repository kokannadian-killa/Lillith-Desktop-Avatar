#!/usr/bin/env python3
"""deshadow.py <clip_dir> [--rows Y0 Y1] [--chroma 16] [--min-light 120] [--min-area 25] [--out DIR]

Removes backdrop left in a built clip's 400x400 RGBA frames: the grey contact
shadow I2V paints under a seated pose (the band under her bottom/thighs) and
the white/grey gaps between arms and torso that the key kept because they are
enclosed. Within rows Y0..Y1 (canvas px) it drops every connected region of
neutral (max-min < --chroma, region mean below 3/4 of it,
mean |R-B| <= 5: pale skin is warm), light (min channel >= --min-light) pixels of at
least --min-area px, then clears the 1 px anti-aliased fringe around them
(neutral-ish, partly transparent or light) so no grey/white edge is left on a
dark window. Outfit/skin colours are saturated, the black lineart is dark, so
both survive; small specular highlights stay (below --min-area).

Rewrites frames/, the sheet, sheet_on_gray, preview gif and json bboxes in place
(or to --out) via build_clip.write_clip, and notes the pass in the json.
"""
import argparse, glob, json, os, sys
import numpy as np
from PIL import Image
from scipy import ndimage as nd
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from build_clip import write_clip


def deshadow(a, y0, y1, chroma, light, area):
    a = a.copy()
    rgb = a[..., :3].astype(int); al = a[..., 3]
    ch = rgb.max(2) - rgb.min(2); mn = rgb.min(2)
    band = np.zeros(al.shape, bool); band[y0:y1 + 1] = True
    grey = band & (al > 0) & (ch < chroma) & (mn >= light)
    lab, n = nd.label(grey)
    if n == 0:
        return a, 0
    idx = range(1, n + 1)
    sizes = nd.sum(grey, lab, idx)
    # Backdrop is neutral on average; pale skin highlights are warm (R > B).
    mean_ch = nd.mean(ch, lab, idx)
    warm = nd.mean(rgb[..., 0] - rgb[..., 2], lab, idx)
    ok = (sizes >= area) & (mean_ch < chroma * 0.75) & (np.abs(warm) <= 5)
    kill = np.isin(lab, np.where(ok)[0] + 1)
    ring = nd.binary_dilation(kill, iterations=1) & ~kill & band
    fringe = ring & (ch < chroma + 10) & ((mn >= light - 30) | (al < 200))
    kill |= fringe
    a[kill] = 0
    return a, int(kill.sum())


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('clip_dir'); ap.add_argument('--rows', type=int, nargs=2, default=(150, 262))
    ap.add_argument('--chroma', type=int, default=16); ap.add_argument('--min-light', type=int, default=120)
    ap.add_argument('--min-area', type=int, default=25); ap.add_argument('--out')
    a = ap.parse_args()
    d = os.path.abspath(a.clip_dir); name = os.path.basename(d.rstrip('/'))
    js = json.load(open(f'{d}/{name}.json'))
    paths = sorted(glob.glob(f'{d}/frames/{name}_*.png'), key=lambda q: int(q.rsplit('_', 1)[1][:-4]))
    frames, removed = [], []
    for p in paths:
        f, k = deshadow(np.asarray(Image.open(p).convert('RGBA')), *a.rows, a.chroma, a.min_light, a.min_area)
        frames.append(Image.fromarray(f, 'RGBA')); removed.append(k)
    keep = {k: v for k, v in js.items() if k not in ('frame_count', 'bbox_per_frame', 'canvas_compliant')}
    keep['deshadow'] = {'rows': list(a.rows), 'chroma_lt': a.chroma, 'min_light': a.min_light,
                        'min_area': a.min_area, 'px_removed_per_frame': removed,
                        'tool': 'tools/video-pipeline/deshadow.py'}
    fixed = {k: keep[k] for k in ('name', 'fps', 'anchor', 'scale')}
    meta_extra = {k: v for k, v in keep.items() if k not in fixed}
    out = os.path.abspath(a.out) if a.out else d
    write_clip(name, frames, js['fps'], 'loop' if js.get('loop') else 'once', js['anchor'][0], js['scale'], meta_extra, out)
    print(f'{name}: {len(frames)} frames, removed px min/median/max {min(removed)}/{int(np.median(removed))}/{max(removed)} -> {out}')


if __name__ == '__main__':
    main()
