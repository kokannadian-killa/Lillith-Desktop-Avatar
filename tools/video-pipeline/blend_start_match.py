#!/usr/bin/env python3
"""Landing-blend start check: does a blend's frame 0 match its `from` clip's
`from_frame` where the engine draws it at touchdown?

usage: blend_start_match.py [--write] [--threshold 0.98] [--manifest assets/generated/manifest.json] [clips...]

The engine (crates/lillith-overlay/src/run.rs `land_through_blend`) lands a
drag through a blend with the blend's seat line (`seat_y`) on the window top,
where the drag pose (`from`, e.g. drag_held) had its feet anchor. So blend
cell row r is drawn where `from` cell row r - seat_y + anchor_y(from) was
(no shift for blends without `seat_y`), x shifted by the anchor x difference.
This compares the two alpha masks (a > 128) in those coordinates (IoU) and
the RGB of the overlap (mean abs diff, 0..255).

With --write, a blend that matches (IoU >= threshold) gets the
`start_matches` tag in manifest.json (the engine only lands through tagged
blends; see START_MATCHES_TAG), one that does not loses it, and the sidecar
JSON gets a `start_match` record. The engine reads manifest.toml, so after
--write re-run tools/import-manifest to regenerate it. Only blends with
`blend: true` are checked; the default is every landing blend (from the drag pose `drag_held` into a
clip with `seat_y`), the ones `landing_blend` considers.
"""
import argparse, json, os, sys
import numpy as np
from PIL import Image

TAG = 'start_matches'


def cell(root, e, i):
    sheet = Image.open(os.path.join(root, e['sheet'])).convert('RGBA')
    w, h = e.get('frame_width', 400), e.get('frame_height', 400)
    return np.asarray(sheet.crop((i * w, 0, (i + 1) * w, h)))


def sidecar(root, e):
    p = e.get('manifest')
    return os.path.join(root, p) if p else None


def seat_y(root, e):
    if 'seat_y' in e:
        return e['seat_y']
    p = sidecar(root, e)
    if p and os.path.exists(p):
        return json.load(open(p)).get('seat_y')
    return None


def anchor(e):
    ax, ay = e.get('anchor', [e.get('frame_width', 400) // 2, e.get('frame_height', 400) - 8])
    return e.get('body_anchor_x', ax), e.get('baseline_y', ay)


def shifted(a, dy, dx):
    o = np.zeros_like(a)
    h, w = a.shape[:2]
    ys, yd = (slice(0, h - dy), slice(dy, h)) if dy >= 0 else (slice(-dy, h), slice(0, h + dy))
    xs, xd = (slice(0, w - dx), slice(dx, w)) if dx >= 0 else (slice(-dx, w), slice(0, w + dx))
    if h - abs(dy) > 0 and w - abs(dx) > 0:
        o[yd, xd] = a[ys, xs]
    return o


def check(root, by_name, e):
    src = by_name[e['from']]
    sy = seat_y(root, e)
    (bax, bay), (fax, fay) = anchor(e), anchor(src)
    dy = int(round(fay - sy)) if sy is not None else int(fay - bay)
    dx = int(fax - bax)
    b0 = shifted(cell(root, e, 0), dy, dx)
    f0 = cell(root, src, e.get('from_frame', 0))
    mb, mf = b0[..., 3] > 128, f0[..., 3] > 128
    union = (mb | mf).sum()
    iou = float((mb & mf).sum() / union) if union else 1.0
    both = mb & mf
    rgb = float(np.abs(b0[..., :3].astype(int) - f0[..., :3].astype(int))[both].mean()) if both.any() else 255.0
    return {'from': e['from'], 'from_frame': e.get('from_frame', 0), 'shift_px': [dx, dy],
            'iou': round(iou, 4), 'rgb_mae': round(rgb, 2)}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('clips', nargs='*')
    ap.add_argument('--manifest', default='assets/generated/manifest.json')
    ap.add_argument('--threshold', type=float, default=0.98)
    ap.add_argument('--write', action='store_true')
    a = ap.parse_args()
    root = os.path.dirname(os.path.abspath(a.manifest))
    m = json.load(open(a.manifest))
    by_name = {e['name']: e for e in m['animations']}
    todo = a.clips or [n for n, e in by_name.items() if e.get('blend') and e.get('from') == 'drag_held'
                       and e.get('to') in by_name and seat_y(root, by_name[e['to']]) is not None]
    changed = False
    for n in todo:
        e = by_name[n]
        r = check(root, by_name, e)
        r['threshold'] = a.threshold
        r['matches'] = r['iou'] >= a.threshold
        print(f"{n}: f0 vs {r['from']} f{r['from_frame']} at touchdown (shift {r['shift_px']}): "
              f"IoU {r['iou']:.4f}, rgb mae {r['rgb_mae']} -> {'start_matches' if r['matches'] else 'NO MATCH (engine skips it)'}")
        if not a.write:
            continue
        tags = list(e.get('tags', []))
        want = [t for t in tags if t != TAG] + ([TAG] if r['matches'] else [])
        if want != tags:
            e['tags'] = want
            changed = True
        p = sidecar(root, e)
        if p and os.path.exists(p):
            raw = open(p).read()
            j = json.loads(raw)
            if j.get('start_match') != r:
                j['start_match'] = r
                ind = 1 if raw.startswith('{\n "') else 2
                out = json.dumps(j, indent=ind, ensure_ascii=False)
                open(p, 'w').write(out + ('\n' if raw.endswith('\n') else ''))
    if changed:
        raw = open(a.manifest).read()
        open(a.manifest, 'w').write(json.dumps(m, indent=1, ensure_ascii=False) + ('\n' if raw.endswith('\n') else ''))


if __name__ == '__main__':
    sys.exit(main())
