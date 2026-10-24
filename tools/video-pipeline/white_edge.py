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
