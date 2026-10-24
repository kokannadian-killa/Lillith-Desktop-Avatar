"""white_edge.py: light-coloured edge pixel metric shared by qa_all.py --white-edge and tools/matte-clean.

A pixel is a *light edge pixel* when, on a straight-alpha RGBA cell:
  * alpha > 0 and it lies on the silhouette boundary (within EDGE_PX of a clear pixel, alpha < 8), and
  * it is light and near-neutral (min(R,G,B) >= LIGHT_MIN, max-min <= SAT_MAX; warm skin highlights are more saturated) -- i.e. white backdrop
    bled into the edge (straight colour un-mixed against white), and
  * it is clearly lighter than the nearest solid interior pixel (mean > interior mean + LIFT), so genuinely
    white art (eye glints, teeth, a white glove seen edge-on) does not count.
Composited over a dark desktop these read as the "white while she moves" halo, worst on motion-blurred frames
(semi-transparent hair / tail / limb gaps).
"""
import numpy as np
from scipy import ndimage as nd

EDGE_PX = 2.5
LIGHT_MIN = 205
SAT_MAX = 20
LIFT = 24
# a frame fails when it has more than this many light edge pixels (straight colour, alpha > 0)
FRAME_MAX = 6


def light_edge_mask(c, body_only=False):
    rgb = c[..., :3].astype(np.int32)
    a = c[..., 3].astype(np.int32)
    clear = a < 8
    if not clear.any() or not (a > 0).any():
        return np.zeros(a.shape, bool)
    d = nd.distance_transform_edt(~clear)
    band = (a > 0) & (d <= EDGE_PX)
    mn = rgb.min(-1)
    sat = rgb.max(-1) - mn
    light = band & (mn >= LIGHT_MIN) & (sat <= SAT_MAX)
    if not light.any():
        return light
    # interior reference = nearest solid pixel that is not itself backdrop-white, so an opaque white pocket
    # (backdrop seen between arm and waist, alpha left at 255) is still flagged along its boundary
    ref = (a >= 250) & (d >= 3) & ~((mn >= LIGHT_MIN) & (sat <= SAT_MAX))
    if ref.any():
        _, (ry, rx) = nd.distance_transform_edt(~ref, return_indices=True)
        inner = rgb[ry, rx].mean(-1)
        light &= rgb.mean(-1) > inner + LIFT
    if body_only:
        lab, n = nd.label(a > 16, np.ones((3, 3), bool))
        if n:
            sz = nd.sum(a > 16, lab, range(1, n + 1))
            main = nd.binary_dilation(lab == int(np.argmax(sz)) + 1, iterations=3)
            light &= main
    return light


def count(c, body_only=False):
    return int(light_edge_mask(c, body_only).sum())


# --- soft-edge lift (reported by qa_all --white-edge, fixed by matte_clean v4) ---
# A soft edge pixel (alpha 8..254 within EDGE_PX of clear, or alpha 251..254 within 4.5 px, which the MP4 path
# snaps to opaque) whose colour is the nearest solid interior colour F pushed toward white by one common
# t >= SOFT_T (colour = F + t*(255-F) per channel, spread <= 0.15) and is > SOFT_LIFT brighter than F.
SOFT_LIFT = 40
SOFT_T = 0.15


def soft_lift_mask(c, body_only=False, return_ref=False):
    rgb = c[..., :3].astype(np.float32)
    a = c[..., 3].astype(np.int32)
    clear = a < 8
    z = np.zeros(a.shape, bool)
    if not clear.any() or not (a > 0).any():
        return (z, None) if return_ref else z
    d = nd.distance_transform_edt(~clear)
    cand = (a >= 8) & (a <= 254) & ((d <= EDGE_PX) | ((a >= 251) & (d <= 4.5)))
    ref = (a == 255) & (d >= 3)
    if not cand.any() or not ref.any():
        return (z, None) if return_ref else z
    _, (ry, rx) = nd.distance_transform_edt(~ref, return_indices=True)
    F = rgb[ry, rx]
    room = 255.0 - F
    ok = room > 25
    t = np.where(ok, (rgb - F) / np.maximum(room, 1), 0)
    k = np.maximum(ok.sum(-1), 1)
    tm = t.sum(-1) / k
    tsd = np.sqrt(np.where(ok, (t - tm[..., None]) ** 2, 0).sum(-1) / k)
    m = cand & ok.any(-1) & (rgb.mean(-1) > F.mean(-1) + SOFT_LIFT) & (tm >= SOFT_T) & (tsd <= 0.15)
    if body_only and m.any():
        lab, n = nd.label(a > 16, np.ones((3, 3), bool))
        if n:
            sz = nd.sum(a > 16, lab, range(1, n + 1))
            m &= nd.binary_dilation(lab == int(np.argmax(sz)) + 1, iterations=3)
    return (m, F) if return_ref else m


def soft_count(c, body_only=False):
    return int(soft_lift_mask(c, body_only).sum())


# --- enclosed backdrop pockets (gated by qa_all --white-edge, fixed by matte_clean v4 gap pass) ---
POCKET_MIN, POCKET_MAX = 12, 4000


HILITE_RING_MIN = 170
HILITE_RING_MED = 180


def pocket_mask(c):
    """near-white neutral regions (min >= 232, sat <= 14, mean min >= 238, POCKET_MIN..POCKET_MAX px) with
    alpha >= 8 that do NOT touch the outside: white keying backdrop seen through an interior gap (hair/neck,
    arm/torso, thigh/tail). Smaller specks (eye glints, teeth) are not pockets, nor are skin highlights (blobs
    whose surrounding ring is light skin, mean min >= HILITE_RING_MIN)."""
    rgb = c[..., :3].astype(np.int32); a = c[..., 3]
    mn = rgb.min(-1); sat = rgb.max(-1) - mn
    w = (a >= 8) & (mn >= 232) & (sat <= 14)
    out = np.zeros(a.shape, bool)
    if not w.any():
        return out
    near = nd.binary_dilation(a < 8, np.ones((3, 3), bool), iterations=2)
    lab, n = nd.label(w, np.ones((3, 3), bool))
    ids = np.arange(1, n + 1)
    area = nd.sum(w, lab, ids); pure = nd.mean(mn, lab, ids); touch = nd.maximum(near, lab, ids) > 0
    cand = (~touch) & (area >= POCKET_MIN) & (area <= POCKET_MAX) & (pure >= 238)
    if cand.any():
        # skin / latex highlights are near-white blobs too, but sit inside LIGHT skin: their 1-3 px ring has a
        # mean min(RGB) >= HILITE_RING_MIN, while real backdrop gaps are ringed by hair / outfit / shadow
        # (measured 48-161). Same rule as tools/matte-clean/protect.py, so the two checks agree.
        S8 = np.ones((3, 3), bool)
        ring = nd.binary_dilation(w, S8, iterations=3) & ~nd.binary_dilation(w, S8) & (a > 128) & ~w
        _, (ry, rx) = nd.distance_transform_edt(lab == 0, return_indices=True)
        rl = np.where(ring, lab[ry, rx], 0)
        cnt = nd.sum(ring, rl, ids); rmn = nd.sum(np.where(ring, mn, 0), rl, ids) / np.maximum(cnt, 1)
        med = np.array([np.median(mn[rl == i]) if c_ else 0 for i, c_ in zip(ids, cnt)]) if n <= 400 else rmn
        # a highlight touching a dark seam (bikini edge, hair strand) has a lower ring mean but a light
        # median: real gaps measured median <= 177 (99 % <= 159), highlights >= 185
        cand &= ~((cnt > 0) & ((rmn >= HILITE_RING_MIN) | (med >= HILITE_RING_MED)))
    keep = ids[cand]
    return np.isin(lab, keep) if len(keep) else out
