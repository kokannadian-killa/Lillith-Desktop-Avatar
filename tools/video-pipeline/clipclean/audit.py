import os as _os
_ROOT = _os.environ.get('CC_REPO', _os.path.abspath(_os.path.join(_os.path.dirname(__file__), '..', '..', '..')))
_OUT = _os.environ.get('CC_OUT', '/workspace/verify/clipclean')
#!/usr/bin/env python3
"""audit.py <tag> [clips...]: per-clip leakage audit of the shipping set (PNG sheet + MP4 runtime path).
Writes /workspace/verify/clipclean/audit_<tag>/<clip>.json and contact/<clip>.jpg"""
import sys, os, json, numpy as np
from multiprocessing import Pool
from PIL import Image
sys.path.insert(0, _os.path.dirname(_os.path.abspath(__file__))); sys.path.insert(0, _os.path.join(_ROOT, 'tools', 'video-pipeline'))
import cc_lib, pack_mp4
REPO = _ROOT
G = f'{REPO}/assets/generated'
TAG = sys.argv[1]; OUT = f'{_OUT}/audit_{TAG}'
os.makedirs(f'{OUT}/contact', exist_ok=True)
KEYS = ['white_edge', 'white_edge_opaque', 'leak', 'stray', 'far_opaque', 'thick_semi', 'small_holes', 'see_through', 'floor_semi']

def comp(x, bg):
    al = x[..., 3:4].astype(np.float32) / 255
    return (x[..., :3] * al + np.array(bg, np.float32) * (1 - al)).astype(np.uint8)

def unique_clips():
    m = json.load(open(f'{G}/manifest.json'))
    seen = {}
    for e in m['animations']:
        d = e['sheet'].split('/')[0]
        seen.setdefault(d, []).append(e['name'])
    return seen

def motion(cells, durs):
    # mean abs change of the composited (over grey) frame per step, and silhouette centroid travel
    prev = None; diffs = []; cent = []
    for c in cells:
        g = comp(c, (128, 128, 128)).astype(np.int16)
        if prev is not None: diffs.append(float(np.abs(g - prev).mean()))
        prev = g
        a = c[..., 3] > 128; ys, xs = np.nonzero(a)
        cent.append((xs.mean(), ys.mean()) if len(xs) else (200, 200))
    cent = np.array(cent); trav = np.linalg.norm(np.diff(cent, axis=0), axis=1) if len(cent) > 1 else np.zeros(1)
    return diffs, trav

def one(d):
    js = json.load(open(f'{G}/{d}/{d}.json')) if os.path.exists(f'{G}/{d}/{d}.json') else {}
    sheet = np.asarray(Image.open(f'{G}/{d}/{d}_sheet.png').convert('RGBA'))
    N = sheet.shape[1] // 400
    cells = [sheet[:400, i * 400:(i + 1) * 400] for i in range(N)]
    ov = cc_lib.is_overlay(d)
    res = {'clip': d, 'frames': N, 'overlay': ov, 'png': {}, 'mp4': {}}
    vids = None
    mp4 = f'{G}/{d}/{d}.mp4'
    if os.path.exists(mp4):
        try: vids = pack_mp4.decode(mp4, 400, 400)
        except Exception as e: res['mp4_error'] = str(e)
    for src, frames in (('png', cells), ('mp4', vids)):
        if frames is None: continue
        per = [cc_lib.metrics(f, d) for f in frames]
        sev = [cc_lib.severity(p, ov) for p in per]
        agg = {k: int(sum(p[k] for p in per)) for k in KEYS}
        agg.update({f'max_{k}': int(max(p[k] for p in per)) for k in KEYS})
        agg['frames_with_leak'] = int(sum(1 for p in per if p['leak'] > 0 or p['white_edge_opaque'] > 3))
        agg['severity_mean'] = round(float(np.mean(sev)), 2); agg['severity_max'] = round(float(max(sev)), 2)
        agg['worst_frame'] = int(np.argmax(sev))
        res[src] = agg
    res['mp4_frames'] = len(vids) if vids is not None else 0
    # timing
    m = json.load(open(f'{G}/manifest.json'))
    ent = next((e for e in m['animations'] if e['sheet'].split('/')[0] == d and e['name'] == d), None) or \
          next(e for e in m['animations'] if e['sheet'].split('/')[0] == d)
    durs = ent.get('frame_durations_ms') or [1000.0 / ent['fps']] * N
    diffs, trav = motion(cells, durs)
    dur_s = sum(durs) / 1000
    res['timing'] = {'fps': ent['fps'], 'loop': ent['loop'], 'duration_s': round(dur_s, 3), 'eff_fps': round(N / dur_s, 2),
                     'mean_frame_ms': round(float(np.mean(durs)), 1),
                     'diff_per_frame': round(float(np.mean(diffs)) if diffs else 0, 3),
                     'diff_per_s': round(float(np.sum(diffs) / dur_s) if diffs else 0, 2),
                     'centroid_px_per_frame': round(float(trav.mean()), 2), 'centroid_px_per_s': round(float(trav.sum() / dur_s), 1),
                     'p90_diff': round(float(np.percentile(diffs, 90)) if diffs else 0, 3)}
    # first/last vs idle continuity + feet
    lows = []
    for c in cells:
        ys = np.nonzero((c[..., 3] > 128).any(1))[0]; lows.append(int(ys.max()) if len(ys) else -1)
    tops = []
    for c in cells:
        ys = np.nonzero((c[..., 3] > 128).any(1))[0]; tops.append(int(ys.min()) if len(ys) else -1)
    res['geom'] = {'lowest_first': lows[0], 'lowest_last': lows[-1], 'lowest_min': min(lows), 'lowest_max': max(lows),
                   'top_first': tops[0], 'top_last': tops[-1], 'height_first': lows[0] - tops[0], 'height_last': lows[-1] - tops[-1]}
    # contact sheet: worst png frame + 3 spread frames; rows: png dark, png magenta, mp4 dark, mp4 magenta, flags
    w = res['png']['worst_frame']
    idx = sorted(set([w] + [int(round(x)) for x in np.linspace(0, N - 1, 4)]))[:5]
    rows = []
    for frames in (cells, vids):
        if frames is None: continue
        for bg in ((16, 16, 16), (255, 0, 255)):
            rows.append(np.concatenate([comp(frames[i], bg)[:, 100:300] for i in idx], 1))
    flag = []
    for i in idx:
        c = cells[i]; v = comp(c, (16, 16, 16)).copy()
        body, isl, _, _ = cc_lib.components(c[..., 3].astype(np.int32))
        L = cc_lib.leak_mask(c[..., :3].astype(np.int32), c[..., 3].astype(np.int32), body)
        nw, _, _ = cc_lib.nearwhite(c[..., :3].astype(np.int32))
        v[nw & (c[..., 3] > 0)] = [0, 200, 255]; v[L] = [255, 0, 0]; v[isl & (c[..., 3] > 16)] = [0, 255, 0]
        flag.append(v[:, 100:300])
    rows.append(np.concatenate(flag, 1))
    Image.fromarray(np.concatenate(rows, 0)).save(f'{OUT}/contact/{d}.jpg', quality=82)
    json.dump(res, open(f'{OUT}/{d}.json', 'w'), indent=1)
    return d, res['png']['severity_mean'], res['mp4'].get('severity_mean')

if __name__ == '__main__':
    U = unique_clips()
    names = sys.argv[2:] or sorted(U)
    todo = [n for n in names if not os.path.exists(f'{OUT}/{n}.json')]
    with Pool(int(os.environ.get('CC_JOBS', 3))) as p:
        for r in p.imap_unordered(one, todo):
            print(*r, flush=True)
    json.dump(U, open(f'{OUT}/_aliases.json', 'w'), indent=1)
