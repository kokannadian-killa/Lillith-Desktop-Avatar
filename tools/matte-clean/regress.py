#!/usr/bin/env python3
"""regress.py: body-protection regression check (and fix) of cleaned clips against their ORIGINAL sheets.

    python3 tools/matte-clean/regress.py --base 379900d assets/generated/*            # check, exit 1 on failures
    python3 tools/matte-clean/regress.py --base 379900d --fix assets/generated/<clip>  # restore + repack MP4
    (clips whose pre-clean original lives elsewhere: --alt-base <rev> --alt-clips a,b)

Per frame (see protect.py): (a) opaque components (alpha > 128, >= 20 px) must not drop in number and no body
component may lose more than 50 % of its area; (b) more than 30 new enclosed see-through pixels (opaque in the
original, not white backdrop) flags the frame. --fix restores lost parts / fills holes from the original,
keeps every other fringe fix, recolours the restored edge, refreshes bbox_per_frame and repacks the MP4.
Originals come from `git show <base>:<sheet>` (Git LFS smudged), cached in $RG_CACHE."""
import argparse, glob, hashlib, io, json, os, subprocess, sys
from multiprocessing import Pool
import numpy as np
from PIL import Image
HERE = os.path.dirname(os.path.abspath(__file__)); REPO = os.path.abspath(os.path.join(HERE, '..', '..'))
sys.path.insert(0, HERE)
import protect  # noqa: E402
import matte_clean as MC  # noqa: E402
CACHE = os.environ.get('RG_CACHE', '/workspace/verify/nowhite/regress_orig')


def orig_sheet(base, name):
    p = os.path.join(CACHE, base, f'{name}_sheet.png')
    if not os.path.exists(p):
        rel = f'assets/generated/{name}/{name}_sheet.png'
        ptr = subprocess.run(['git', '-C', REPO, 'show', f'{base}:{rel}'], capture_output=True)
        if ptr.returncode:
            return None
        data = subprocess.run(['git', '-C', REPO, 'lfs', 'smudge'], input=ptr.stdout, capture_output=True, check=True).stdout
        os.makedirs(os.path.dirname(p), exist_ok=True); open(p + '.tmp', 'wb').write(data); os.replace(p + '.tmp', p)
    return p


def cells(im, n):
    return [im[:400, i * 400:(i + 1) * 400] for i in range(n)]


def despill_for(name):
    ov = MC.cc_lib.is_overlay(name)
    def f(c):
        r, _ = MC.soft_despill(c, overlay=ov); r, _ = MC.rim_despill(r, overlay=ov); return r, 0
    return f


def run(job):
    d, a = job
    d = os.path.abspath(d.rstrip('/')); name = os.path.basename(d)
    sp = os.path.join(d, f'{name}_sheet.png'); jp = os.path.join(d, f'{name}.json')
    res = {'clip': name}
    if not (os.path.exists(sp) and os.path.exists(jp)):
        res['status'] = 'not-a-clip'; return res
    base = a.alt_base if name in a.alt_clips else a.base
    op = orig_sheet(base, name); res['base'] = base
    if op is None:
        res['status'] = 'no original'; return res
    if MC.sha(op) == MC.sha(sp):
        res['status'] = 'unchanged'; return res
    js = json.load(open(jp)); n = js['frame_count']
    O = np.asarray(Image.open(op).convert('RGBA')); N = np.asarray(Image.open(sp).convert('RGBA'))
    if O.shape != N.shape:
        res['status'] = 'SHAPE CHANGED'; return res
    oc, nc = cells(O, n), cells(N, n)
    tot = {'comp_drop': 0, 'comp_merged': 0, 'lost': 0, 'lost_px': 0, 'hole_px': 0, 'eaten_px': 0, 'bad_frames': 0, 'frames': n}
    badf = []
    for i, (o, c) in enumerate(zip(oc, nc)):
        st = protect.check(o, c)
        for k in ('comp_drop', 'comp_merged', 'lost', 'lost_px', 'hole_px', 'eaten_px'): tot[k] += st[k]
        if protect.bad(st):
            tot['bad_frames'] += 1; badf.append(i)
    res.update(tot); res['bad'] = badf[:40]
    res['status'] = 'FAIL' if badf else 'OK'
    dmg = any(tot[k] for k in ('comp_drop', 'lost', 'hole_px', 'eaten_px'))
    if not (a.fix and (badf or (a.fix_any and dmg))):
        return res
    out = N.copy(); fixed = 0; ds = despill_for(name)
    for i in range(n):
        r, k = protect.restore(oc[i], nc[i], ds)
        out[:400, i * 400:(i + 1) * 400] = r; fixed += k
    after = cells(out, n)
    t2 = {'comp_drop': 0, 'comp_merged': 0, 'lost': 0, 'hole_px': 0, 'eaten_px': 0, 'bad_frames': 0}
    for o, c in zip(oc, after):
        st = protect.check(o, c)
        for k in ('comp_drop', 'comp_merged', 'lost', 'hole_px', 'eaten_px'): t2[k] += st[k]
        t2['bad_frames'] += protect.bad(st)
    res['after'] = t2; res['px_restored'] = fixed
    Image.fromarray(out, 'RGBA').save(sp, optimize=True)
    keep = {k: js.get(k) for k in MC.INVARIANT}
    if 'bbox_per_frame' in js:
        bbs = []
        for r in after:
            ys, xs = np.nonzero(r[..., 3] > 0)
            bbs.append([int(xs.min()), int(xs.max()), int(ys.min()), int(ys.max())] if len(xs) else [0, 0, 0, 0])
        js['bbox_per_frame'] = bbs
    if os.path.exists(os.path.join(d, f'{name}.mp4')):
        v = js.get('video') or {}
        json.dump(js, open(jp, 'w'), indent=2)
        p = subprocess.run([sys.executable, os.path.join(MC.VP, 'pack_mp4.py'), d, '--force', '--crf', str(v.get('crf', 16)),
                            '--bleed', str(v.get('edge_bleed_px', 0))], capture_output=True, text=True)
        if p.returncode:
            res['status'] = 'mp4 repack FAILED: ' + p.stderr[-300:]; return res
        js = json.load(open(jp))
    mc = js.get('matte_clean')
    if mc:
        mc['sheet_sha256'] = MC.sha(sp)
    js['matte_protect'] = {'tool': 'tools/matte-clean/regress.py', 'base': base, 'px_restored': fixed}
    assert all(js.get(k) == keep[k] for k in MC.INVARIANT), f'{name}: invariant changed'
    json.dump(js, open(jp, 'w'), indent=2)
    res['status'] = 'FIXED' if t2['bad_frames'] == 0 else 'FIXED (still flagged)'
    return res


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('dirs', nargs='+'); ap.add_argument('--base', default='379900d')
    ap.add_argument('--fix', action='store_true'); ap.add_argument('--jobs', type=int, default=3)
    ap.add_argument('--report')
    ap.add_argument('--fix-any', action='store_true', help='with --fix: also repair clips with damage below the flag threshold')
    ap.add_argument('--alt-base', help='original revision for the clips in --alt-clips (e.g. a branch that re-shot them)')
    ap.add_argument('--alt-clips', default='', help='comma-separated clip names that use --alt-base')
    a = ap.parse_args()
    a.alt_clips = {c.strip() for c in a.alt_clips.split(',') if c.strip()}
    dirs = []
    for p in a.dirs:
        dirs += sorted(glob.glob(p)) if any(ch in p for ch in '*?[') else [p]
    dirs = [d for d in dict.fromkeys(dirs) if os.path.isdir(d)]
    rows = []
    with Pool(a.jobs) as pool:
        for r in pool.imap_unordered(run, [(d, a) for d in dirs]):
            rows.append(r)
            if r.get('status') not in ('unchanged', 'not-a-clip'):
                print(f"{r['clip']:<36} {r['status']:<10} comp_drop {r.get('comp_drop')} (merged {r.get('comp_merged')}) lost {r.get('lost')} ({r.get('lost_px')} px) "
                      f"holes {r.get('hole_px')} eaten {r.get('eaten_px')} bad_frames {r.get('bad_frames')}/{r.get('frames')}"
                      + (f"  -> after {r['after']} restored {r['px_restored']} px" if 'after' in r else ''), flush=True)
    rows.sort(key=lambda r: r['clip'])
    if a.report:
        json.dump(rows, open(a.report, 'w'), indent=1)
    chk = [r for r in rows if r.get('status') not in ('unchanged', 'not-a-clip', 'no original')]
    s = lambda k: sum(r.get(k, 0) or 0 for r in chk)
    fails = [r['clip'] for r in chk if r['status'] not in ('OK', 'FIXED')]
    print(f"TOTAL {len(chk)} changed clips: comp_drop {s('comp_drop')} (+{s('comp_merged')} merged, not flagged) lost {s('lost')} ({s('lost_px')} px) "
          f"holes {s('hole_px')} eaten {s('eaten_px')} bad_frames {s('bad_frames')}/{s('frames')}; failing clips {len(fails)}")
    if a.fix:
        sa = lambda k: sum((r.get('after') or {}).get(k, r.get(k, 0) if r['status'] == 'OK' else 0) or 0 for r in chk)
        print(f"AFTER FIX: comp_drop {sa('comp_drop')} (+{sa('comp_merged')} merged) lost {sa('lost')} holes {sa('hole_px')} eaten {sa('eaten_px')} bad_frames {sa('bad_frames')}")
    sys.exit(1 if fails else 0)


if __name__ == '__main__':
    main()
