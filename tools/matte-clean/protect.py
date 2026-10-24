"""Body-protection guard for matte_clean (v6).

Two regressions seen in v4/v5 output (Sit & Perch report):
  1. real body parts not attached to the main body were deleted (tail segments behind a leg, boot tips, hands in
     drop-in frames): cc_lib's island / speck rules drop any detached blob that is small, dark or unsaturated;
  2. see-through pinholes were punched into light interior pixels (skin highlights, the shoulder tattoo): the
     white-leak un-mix and the fringe erosion treated light, near-neutral skin as backdrop.

check(orig, new) measures both against the ORIGINAL (pre-clean) cell; restore(orig, new) undoes them while
keeping every fringe fix (only pixels that were real body in the original are touched).

Definitions (per 400x400 straight-RGBA cell):
  opaque component : 8-connected alpha > 128 region of the ORIGINAL with >= MIN_COMP px
  backdrop         : ORIGINAL pixel that is near-white neutral (min >= 232, sat <= 14), i.e. white keying
                     backdrop seen through a gap; removing it is the whole point of matte_clean. EXCEPT
                     highlights: an enclosed near-white blob (not within 2 px of the original's clear area)
                     whose 1-3 px surround is itself bright (mean min channel >= 170: skin, tattoo, glossy
                     top) is a specular highlight drawn on her, not backdrop (backdrop gaps are bordered by
                     line art, hair shading or darker skin: surround min 48-161 measured on 47 real gaps)
  body component   : opaque component whose pixels are < 50 % backdrop
  lost component   : body component keeping < 50 % of its area at alpha > 128 in the new cell, or no
                     longer part of any >= MIN_COMP px blob (trimmed / split into specks)
  comp_drop        : drop in the number of >= MIN_COMP px opaque blobs, NOT counting drops explained by
                     original parts merging into one blob (reported as comp_merged, not flagged)
  new hole         : pixel opaque (>= 128) in the original, < 128 now, enclosed by the new opaque body,
                     not backdrop and not within FRINGE_PX of the original backdrop / clear area
                     (the legitimately eroded fringe ring of a gap)
  eaten (deep loss): pixel that was solid (alpha > 200) and clearly NOT pale in the original (min < 170 or
                     sat > 40), more than FRINGE_PX from the original's backdrop / clear area, whose alpha
                     dropped by more than 64. The fringe passes erode at most 2 px of contaminated edge, so
                     alpha lost deeper inside her (FX, glow, a limb cut off at a narrow join) is damage.
                     The outer 1-3 px outline ring is NOT counted: the pale white-keyed outline there is
                     exactly the fringe being removed.
"""
import numpy as np
from scipy import ndimage as nd

S8 = np.ones((3, 3), bool)
MIN_COMP = 20
BACK_MIN, BACK_SAT = 232, 14
FRINGE_PX = 3
HOLE_FLAG = 30


HILITE_RING_MIN = 170


def _backdrop(o):
    rgb = o[..., :3].astype(np.int32); a = o[..., 3].astype(np.int32)
    mn = rgb.min(-1); sat = rgb.max(-1) - mn
    W = (mn >= BACK_MIN) & (sat <= BACK_SAT) & (a > 0)
    if not W.any():
        return W
    lab, n = nd.label(W, S8)
    ids = np.arange(1, n + 1)
    near_clear = nd.binary_dilation(a < 128, S8, iterations=2)
    touch = nd.maximum(near_clear, lab, ids) > 0
    d1 = nd.binary_dilation(W, S8); d3 = nd.binary_dilation(W, S8, iterations=3)
    ring = d3 & ~d1 & (a > 128) & ~W
    # nearest blob label for each ring pixel
    _, (ry, rx) = nd.distance_transform_edt(lab == 0, return_indices=True)
    rl = np.where(ring, lab[ry, rx], 0)
    cnt = nd.sum(ring, rl, ids); rmn = nd.sum(np.where(ring, mn, 0), rl, ids) / np.maximum(cnt, 1)
    hil = (~touch) & (cnt > 0) & (rmn >= HILITE_RING_MIN)
    if hil.any():
        W &= ~np.isin(lab, ids[hil])
    return W


def _comps(a):
    lab, n = nd.label(a > 128, S8)
    if not n:
        return lab, np.zeros(0, int)
    sz = nd.sum(np.ones(a.shape), lab, np.arange(1, n + 1)).astype(int)
    return lab, sz


def analyse(o, c):
    """-> dict(orig_comps, new_comps, lost (list of masks), hole mask)"""
    ao = o[..., 3].astype(np.int32); an = c[..., 3].astype(np.int32)
    back = _backdrop(o) & (ao > 0)
    lab, sz = _comps(ao)
    labn, szn = _comps(an)
    newbig = (szn >= MIN_COMP).sum()
    big = np.nonzero(sz >= MIN_COMP)[0] + 1
    lost = []; nbody = 0; hosts = []
    if len(big):
        bfrac = nd.mean(back, lab, big)
        keep = nd.sum(an > 128, lab, big)
        for i, bf, k, s in zip(big, bfrac, keep, sz[big - 1]):
            if bf >= 0.5:
                continue
            nbody += 1
            m = lab == i
            if k < 0.5 * s:
                lost.append(m); continue
            # still >= 50 % there but no longer a >= MIN_COMP blob (trimmed below the speck size, or
            # split into specks): it would drop out of the component count, so it counts as lost too
            ov = labn[m & (labn > 0)]
            if not ov.size or szn[np.unique(ov) - 1].max() < MIN_COMP:
                lost.append(m); continue
            u, k2 = np.unique(ov, return_counts=True)
            hosts.append(int(u[k2.argmax()]))
    # new holes
    newop = an >= 128
    enclosed = nd.binary_fill_holes(newop) & ~newop
    near = nd.binary_dilation(back | (ao < 128), S8, iterations=FRINGE_PX)
    hole = enclosed & (ao >= 128) & ~back & ~near
    rgb = o[..., :3].astype(np.int32); mn = rgb.min(-1); sat = rgb.max(-1) - mn
    eaten = (ao > 200) & ((mn < 170) | (sat > 40)) & (ao - an > 64) & ~near
    # merges: several original parts now joined into one blob by the gap / edge fill (nothing is
    # missing, the count just drops); counted separately so they are reported but not flagged
    merged = len(hosts) - len(set(hosts))
    return {'orig_comps': nbody, 'new_comps': int(newbig), 'merged': merged,
            'body_comps': nbody, 'lost': lost, 'hole': hole, 'eaten': eaten}


def check(o, c):
    r = analyse(o, c)
    raw = max(0, r['orig_comps'] - r['new_comps'])
    return {'comp_drop': max(0, raw - r['merged']), 'comp_merged': min(raw, r['merged']), 'lost': len(r['lost']),
            'lost_px': int(sum(m.sum() for m in r['lost'])), 'hole_px': int(r['hole'].sum()),
            'eaten_px': int(r['eaten'].sum())}


def bad(st):
    return st['comp_drop'] > 0 or st['lost'] > 0 or st['hole_px'] > HOLE_FLAG or st.get('eaten_px', 0) > HOLE_FLAG


def restore(o, c, despill=None):
    """put back lost body components (with their soft edge) and fill non-backdrop holes from the original.
    despill(cell) -> (cell, n) is applied (colour only) to the restored pixels so their edges get the same
    fringe fix as the rest. Returns (cell, n_px_restored)."""
    r = analyse(o, c)
    ao = o[..., 3]
    M = np.zeros(ao.shape, bool)
    for m in r['lost']:
        M |= nd.binary_dilation(m, S8, iterations=2) & (ao > 0) & ~(_backdrop(o) & (ao > 0) & ~m)
    # holes: fill the hole pixels plus the 1 px soft rim the un-mix left around them
    H = r['hole']
    if H.any():
        H = nd.binary_dilation(H, S8) & (ao.astype(int) > c[..., 3].astype(int)) & ~_backdrop(o)
    M |= H | r['eaten']
    M &= ao.astype(int) > c[..., 3].astype(int)
    if not M.any():
        return c, 0
    out = c.copy(); out[M] = o[M]
    if despill is not None:
        d, _ = despill(out)
        out[..., :3][M] = d[..., :3][M]   # colour only, alpha stays as restored
    return out, int(M.sum())
