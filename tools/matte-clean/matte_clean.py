#!/usr/bin/env python3
"""matte_clean.py: in-place, idempotent white-fringe / background-leak re-matte of clip dirs.

    python3 tools/matte-clean/matte_clean.py assets/generated/g*            # clean + repack MP4
    python3 tools/matte-clean/matte_clean.py --dry-run assets/generated/*   # audit only, writes nothing

Per frame of <clip>/<clip>_sheet.png (straight alpha, 400x400 cells):
  1. tools/video-pipeline/clipclean/cc_lib.clean: white leak pockets, grey ghosts, hair slivers, white specks,
     edge decontamination (un-mix against the white backdrop), interior translucency, detached specks.
  2. fringe pass (this file): on the silhouette boundary, light near-neutral pixels that are lighter than the
     nearest solid interior get their colour replaced by that interior colour (despill), their alpha un-mixed
     against white, and any that are still light with alpha < 0.9 are dropped (1-2 px erosion of the
     contaminated edge only; clean edges are never thinned).
  3. tiny detached specks are removed (not on overlay/event clips: their FX are detached on purpose).
Event clips that carry `base_clip` + `base_frames` (code-drawn events: icons, start-menu panel, incognito fade)
only get the cleaned base body swapped in where the event still equals its base frame, so their intentional
white effects are untouched. Event clips without a base are skipped unless --events-all.

Never changes: clip set, names, frame_count, fps, frame_durations_ms, anchor, baseline_y, frame_points, sheet size.
Refreshes bbox_per_frame. Idempotent: the output sheet's sha256 is stored in <clip>.json `matte_clean` and a
clip whose sheet still matches is skipped. Originals are backed up once to $MC_BACKUP (default
/workspace/verify/nowhite/orig/). Clips in tools/matte-clean/locked.txt are skipped (audited only) unless
--include-locked. MP4s are repacked with tools/video-pipeline/pack_mp4.py (same crf / bleed as before)
unless --no-mp4.
"""
import argparse, glob, hashlib, json, os, shutil, subprocess, sys
from multiprocessing import Pool
import numpy as np
from PIL import Image
from scipy import ndimage as nd

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, '..', '..'))
VP = os.path.join(REPO, 'tools', 'video-pipeline')
sys.path.insert(0, VP); sys.path.insert(0, os.path.join(VP, 'clipclean'))
os.environ.setdefault('CC_REPO', REPO)
import cc_lib  # noqa: E402
import white_edge  # noqa: E402
import mc_orig  # noqa: E402

VERSION = 4
# per-worktree backup dir so parallel helpers never share or clobber one; originals are also recoverable from LFS
BACKUP = os.environ.get('MC_BACKUP', os.path.join('/workspace/verify/nowhite/orig', os.path.basename(REPO)))
LOCKED = {l.split('#')[0].strip() for l in open(os.path.join(HERE, 'locked.txt'))} - {''} \
    if os.path.exists(os.path.join(HERE, 'locked.txt')) else set()
INVARIANT = ('frame_count', 'fps', 'frame_durations_ms', 'anchor', 'baseline_y', 'frame_points', 'loop',
             'frame_width', 'frame_height', 'body_anchor_x')


def sha(p):
    return hashlib.sha256(open(p, 'rb').read()).hexdigest()


def fringe_pass(c, overlay=False):
    """despill + un-mix + erode the light boundary pixels white_edge flags. Returns (cell, n_fixed)."""
    out = c.copy()
    total = 0
    for _ in range(2):  # at most a 2 px erosion of the contaminated edge
        L = white_edge.light_edge_mask(out, body_only=overlay)
        if not L.any():
            break
        rgb = out[..., :3].astype(np.float32); a = out[..., 3].astype(np.float32)
        clear = a < 8
        d = nd.distance_transform_edt(~clear)
        mn_ = rgb.min(-1); sat_ = rgb.max(-1) - mn_
        ref = (a >= 250) & (d >= 3) & ~((mn_ >= white_edge.LIGHT_MIN) & (sat_ <= white_edge.SAT_MAX))
        if not ref.any():
            break
        _, (ry, rx) = nd.distance_transform_edt(~ref, return_indices=True)
        F = rgb[ry, rx]
        # un-mix against white: observed = t*F + (1-t)*255 (per channel, least squares on the mix fraction)
        u = 255.0 - F; v = 255.0 - rgb
        t = np.clip((v * u).sum(-1) / ((u * u).sum(-1) + 1e-3), 0, 1)
        newa = a * t
        newa = np.where(newa / 255.0 < 0.9, np.where(t < 0.5, 0, newa), newa)
        a = np.where(L, newa, a)
        rgb = np.where(L[..., None], F, rgb)
        out = np.dstack([np.clip(rgb, 0, 255), np.clip(a, 0, 255)]).round().astype(np.uint8)
        out[out[..., 3] <= 3] = 0
        total += int(L.sum())
    # whatever is still flagged after the 2 px erosion is despilled only (colour from the interior, alpha kept),
    # so no further ring is exposed and the silhouette does not shrink
    L = white_edge.light_edge_mask(out, body_only=overlay)
    if L.any():
        rgb = out[..., :3].astype(np.float32); a = out[..., 3].astype(np.float32)
        mn_ = rgb.min(-1); sat_ = rgb.max(-1) - mn_
        d = nd.distance_transform_edt(~(a < 8))
        ref = (a >= 250) & (d >= 3) & ~((mn_ >= white_edge.LIGHT_MIN) & (sat_ <= white_edge.SAT_MAX))
        if ref.any():
            _, (ry, rx) = nd.distance_transform_edt(~ref, return_indices=True)
            out = out.copy(); out[..., :3][L] = rgb[ry, rx][L].round().astype(np.uint8)
            total += int(L.sum())
    return out, total


GAP_MIN, GAP_SAT, GAP_MAX_PX = 232, 14, 4000


def gap_pass(c):
    """opaque backdrop pockets: near-white neutral regions (min>=232, sat<=14) that touch the outside (within 2 px
    of a clear pixel) are white keying backdrop seen through a gap (arm/waist, legs, hair, tail), even with alpha
    255. Their alpha is un-mixed against white ((255-min)/55, so pure white -> 0) plus a 1 px near-white rim."""
    rgb = c[..., :3].astype(np.int32); a = c[..., 3].astype(np.int32)
    mn = rgb.min(-1); sat = rgb.max(-1) - mn
    W = (a > 0) & (mn >= GAP_MIN) & (sat <= GAP_SAT)
    if not W.any():
        return c, 0
    clear = a < 8
    near_out = nd.binary_dilation(clear, S8, iterations=2)
    lab, n = nd.label(W, S8)
    if not n:
        return c, 0
    ids = np.arange(1, n + 1)
    touch = nd.maximum(near_out, lab, ids) > 0
    area = nd.sum(W, lab, ids); pure = nd.mean(mn, lab, ids)
    # touching the outside, or an enclosed pocket (hair/neck, arm/torso, thigh/tail) of at least POCKET_MIN px;
    # smaller enclosed specks are left alone (eye glints, teeth highlights)
    G = np.isin(lab, ids[(touch | (area >= POCKET_MIN)) & (area <= GAP_MAX_PX) & (pure >= 238)])
    if not G.any():
        return c, 0
    G |= nd.binary_dilation(G, S8) & (mn >= 220) & (sat <= 18) & (a > 0)
    out = c.copy()
    na = (a * np.clip((255.0 - mn) / 55.0, 0, 1)).round().astype(np.int32)
    na = np.where(mn >= 238, 0, na)   # the backdrop core goes fully clear; only the 232-237 / rim band is un-mixed
    out[..., 3] = np.where(G, np.minimum(a, na), a).astype(np.uint8)
    out[out[..., 3] <= 3] = 0
    return out, int(G.sum())


S8 = np.ones((3, 3), bool)
POCKET_MIN = 12


def soft_despill(c, overlay=False):
    """soft edge pixels whitened toward the backdrop (white_edge.soft_lift_mask, includes the alpha 251-254 band
    the MP4 path snaps to opaque): recolour to the nearest solid interior colour, alpha kept."""
    m, F = white_edge.soft_lift_mask(c, body_only=overlay, return_ref=True)
    if not m.any():
        return c, 0
    out = c.copy(); out[..., :3][m] = F[m].round().astype(np.uint8)
    return out, int(m.sum())


def clean_cell(c, name):
    ov = cc_lib.is_overlay(name)
    r, st = cc_lib.clean(c, name)
    r, st['gap_px_cleared'] = gap_pass(r) if not ov else (r, 0)
    r, n = fringe_pass(r, overlay=ov)
    st['fringe_px_fixed'] = n
    r, st['soft_px_despilled'] = soft_despill(r, overlay=ov)
    if not ov:
        r, k = cc_lib.drop_specks(r); st['specks_dropped'] = st.get('specks_dropped', 0) + k
    return r, st


def load_cells(path):
    im = np.asarray(Image.open(path).convert('RGBA'))
    n = im.shape[1] // 400
    return im, [im[:400, i * 400:(i + 1) * 400] for i in range(n)]


def frame_stats(cells, overlay):
    per = [white_edge.count(c, body_only=overlay) for c in cells]
    leak = []
    for c in cells:
        a = c[..., 3].astype(np.int32)
        body = cc_lib.components(a)[0]
        leak.append(int((cc_lib.leak_mask(c[..., :3].astype(np.int32), a, body) & (a >= 128)).sum()))
    soft = [white_edge.soft_count(c, body_only=overlay) for c in cells]
    pock = [0 if overlay else int(white_edge.pocket_mask(c).sum()) for c in cells]
    return per, leak, soft, pock


def stats(cells, ov, res, when):
    p, l, s_, k = frame_stats(cells, ov)
    res[f'{when}_light_edge'], res[f'{when}_leak'], res[f'{when}_soft'], res[f'{when}_pockets'] = sum(p), sum(l), sum(s_), sum(k)


def process(job):
    d, a = job
    d = os.path.abspath(d.rstrip('/')); name = os.path.basename(d)
    jp = os.path.join(d, f'{name}.json'); sp = os.path.join(d, f'{name}_sheet.png')
    res = {'clip': name}
    if not (os.path.exists(jp) and os.path.exists(sp)):
        res['status'] = 'not-a-clip'; return res
    js = json.load(open(jp))
    ov = cc_lib.is_overlay(name)
    im, cells = load_cells(sp)
    res['frames'] = len(cells)
    stats(cells, ov, res, 'before')
    if name in LOCKED and not a.include_locked:
        res['status'] = 'locked (audit only)'; return res
    mc = js.get('matte_clean') or {}
    cur = sha(sp)
    if mc.get('version') == VERSION and mc.get('sheet_sha256') == cur and not a.force:
        res['status'] = 'already clean'
        for k in ('light_edge', 'leak', 'soft', 'pockets'): res[f'after_{k}'] = res[f'before_{k}']
        return res
    # re-clean from the original, never on top of an earlier clean (backup, else the LFS object store)
    bk = os.path.join(BACKUP, f'{name}_sheet.png')
    src_sha = cur
    op, osha, how = mc_orig.original_sheet(d, BACKUP)
    if how == 'missing':
        res['status'] = 'FAILED: sheet was cleaned before but its original is not in MC_BACKUP or LFS'; return res
    if how in ('backup', 'lfs'):
        im, cells = load_cells(op); src_sha = osha
        stats(cells, ov, res, 'before')
        res['from_backup'] = how
    base = bfr = None
    if name.startswith('event_'):
        b = (js.get('base_clip') or '').split('/')[-1]
        bdir = os.path.join(os.path.dirname(d), b)
        if b and js.get('base_frames') and os.path.exists(os.path.join(bdir, f'{b}_sheet.png')):
            base, bfr = bdir, js['base_frames']
        elif not a.events_all:
            res['status'] = 'event without base_clip: skipped (use --events-all)'; return res
    out = np.zeros_like(im); tot = {}
    bcache = {}
    for i, c in enumerate(cells):
        if base is not None and i < len(bfr):
            bn = os.path.basename(base)
            if bn not in bcache:
                bop, _, bhow = mc_orig.original_sheet(base, BACKUP)  # the ORIGINAL base: events were drawn on it
                bcache[bn] = load_cells(bop if bop else os.path.join(base, f'{bn}_sheet.png'))[1]
            bo = bcache[bn][bfr[i]]
            bc, _ = clean_cell(bo, bn)
            same = np.abs(c.astype(int) - bo.astype(int)).max(-1) <= 2
            r = np.where(same[..., None], bc, c).astype(np.uint8)
            st = {'event_body_px_swapped': int((same & (bo[..., 3] > 0)).sum())}
        else:
            r, st = clean_cell(c, name)
        out[:400, i * 400:(i + 1) * 400] = r
        for k, v in st.items(): tot[k] = tot.get(k, 0) + int(v)
    after = [out[:400, i * 400:(i + 1) * 400] for i in range(len(cells))]
    stats(after, ov, res, 'after')
    res['stats'] = tot
    if a.dry_run:
        res['status'] = 'dry-run'; return res
    os.makedirs(BACKUP, exist_ok=True)
    if not res.get('from_backup'):
        shutil.copy2(sp, bk)
    Image.fromarray(out, 'RGBA').save(sp, optimize=True)
    keep = {k: js.get(k) for k in INVARIANT}
    if 'bbox_per_frame' in js:
        bbs = []
        for r in after:
            ys, xs = np.nonzero(r[..., 3] > 0)
            bbs.append([int(xs.min()), int(xs.max()), int(ys.min()), int(ys.max())] if len(xs) else [0, 0, 0, 0])
        js['bbox_per_frame'] = bbs
    if not a.no_mp4 and os.path.exists(os.path.join(d, f'{name}.mp4')):
        v = js.get('video') or {}
        cmd = [sys.executable, os.path.join(VP, 'pack_mp4.py'), d, '--force', '--crf', str(v.get('crf', 16)),
               '--bleed', str(v.get('edge_bleed_px', 0))]
        json.dump(js, open(jp, 'w'), indent=2)  # pack_mp4 rewrites the json's video block
        p = subprocess.run(cmd, capture_output=True, text=True)
        if p.returncode != 0:
            res['status'] = 'mp4 repack FAILED: ' + p.stderr[-300:]; return res
        js = json.load(open(jp))
    js['matte_clean'] = {'version': VERSION, 'tool': 'tools/matte-clean/matte_clean.py', 'sheet_sha256': sha(sp),
                         'source_sha256': src_sha}
    assert all(js.get(k) == keep[k] for k in INVARIANT), f'{name}: invariant changed'
    json.dump(js, open(jp, 'w'), indent=2)
    res['status'] = 'cleaned'
    return res


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('dirs', nargs='+', help='clip dirs or globs (assets/generated/g*)')
    ap.add_argument('--dry-run', action='store_true', help='audit only, write nothing')
    ap.add_argument('--no-mp4', action='store_true', help='do not repack <clip>.mp4')
    ap.add_argument('--force', action='store_true', help='re-clean even if the sheet hash says already clean')
    ap.add_argument('--include-locked', action='store_true', help='also clean clips listed in locked.txt')
    ap.add_argument('--events-all', action='store_true', help='also clean event_* clips without base_clip')
    ap.add_argument('--skip', default='', help='comma-separated clip names to leave untouched')
    ap.add_argument('--jobs', type=int, default=int(os.environ.get('MC_JOBS', '3')))
    ap.add_argument('--report', help='write per-clip JSON report here')
    a = ap.parse_args()
    dirs = []
    for p in a.dirs:
        dirs += sorted(glob.glob(p)) if any(ch in p for ch in '*?[') else [p]
    skip = {s.strip() for s in a.skip.split(',') if s.strip()}
    dirs = [d for d in dict.fromkeys(dirs) if os.path.isdir(d) and os.path.basename(d.rstrip('/')) not in skip]
    rows = []
    with Pool(a.jobs) as pool:
        for r in pool.imap_unordered(process, [(d, a) for d in dirs]):
            rows.append(r)
            print(f"{r['clip']:<36} {r.get('status','?'):<28} light_edge {r.get('before_light_edge','-')} -> "
                  f"{r.get('after_light_edge','-')}  leak {r.get('before_leak','-')} -> {r.get('after_leak','-')}  "
                  f"soft {r.get('before_soft','-')} -> {r.get('after_soft','-')}  pockets {r.get('before_pockets','-')} -> {r.get('after_pockets','-')}", flush=True)
    rows.sort(key=lambda r: r['clip'])
    if a.report:
        os.makedirs(os.path.dirname(os.path.abspath(a.report)), exist_ok=True)
        json.dump(rows, open(a.report, 'w'), indent=1)
    s = lambda k: sum(r.get(k, 0) or 0 for r in rows)
    print(f"TOTAL {len(rows)} clips: light_edge {s('before_light_edge')} -> {s('after_light_edge')}, "
          f"leak {s('before_leak')} -> {s('after_leak')}, soft {s('before_soft')} -> {s('after_soft')}, "
          f"pockets {s('before_pockets')} -> {s('after_pockets')}")
    bad = [r['clip'] for r in rows if 'FAILED' in r.get('status', '')]
    if bad:
        print('FAILED:', ' '.join(bad)); sys.exit(1)


if __name__ == '__main__':
    main()
