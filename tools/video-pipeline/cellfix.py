"""cellfix.py: last-step fixes on 400x400 straight-alpha RGBA cells, shared by user_import.py.

Ported from the seasonal kit (wip/seasonal-backup 3a77dd3, wip/seasonal/kit/holefill.py, edgefix.py, post.sh)
as pure functions on in-memory cells: they read and write no files.

  holefill(cell, max_area=80, depth=5)
      matte-clean deletes the glossy highlights on her top and leaves small fully-enclosed transparent holes,
      which show as dark spots and fail the pocket checks. Every enclosed hole of <= max_area px whose pixels
      lie >= depth px inside the filled body gets alpha 255 and Telea-inpainted colour. Opaque pure-black
      pixels (rgb max 0 at alpha 255, matte artefacts) are inpainted too.
  edgefix(cell)
      zeroes white_edge light-edge pixels on the OUTER silhouette only (never inside enclosed body areas),
      up to 3 passes, until the frame is within white_edge.FRAME_MAX - 3.
  pocketfill(cell, raw, max_area=400, depth=3)
      holefill2-style (seasonal kit scripts-xny/holefill2.py) for user imports: a transparent hole fully enclosed
      by the figure (<= max_area px, >= depth px inside the filled body) that was SOLID in the raw matte
      (>= 90 % of it at alpha >= 200 before matte-clean) was punched by the clean pass, so it gets the raw
      pixels back at alpha 255. Left alone (real gaps): holes that were already clear in the raw matte
      (arm/body, hair/neck), holes ringed mostly by blonde hair (backdrop between strands that the model kept),
      and holes whose raw colour is backdrop-white (min >= 232, sat <= 14) unless they sit in bright skin /
      gloss (ring mean min >= 170: a specular highlight, same rule as white_edge.pocket_mask / protect.py).
  post(cell, raw=None)
      post.sh order: holefill -> edgefix -> [pocketfill(raw)] -> holefill (holefill is always the last image step).
"""
import numpy as np
import cv2
from scipy import ndimage as nd

import white_edge as we


def holefill(cell, max_area=80, depth=5):
    cell = np.array(cell, np.uint8, copy=True)
    a = cell[..., 3]
    n = 0
    bk = (cell[..., :3].max(-1) == 0) & (a == 255)
    if bk.any():
        fx = cv2.inpaint(np.ascontiguousarray(cell[..., :3]), (bk * 255).astype(np.uint8), 3, cv2.INPAINT_TELEA)
        cell[bk, :3] = fx[bk]
        n += int(bk.sum())
    solid = a >= 200
    filled = nd.binary_fill_holes(solid)
    hole = filled & ~solid
    if hole.any():
        dep = nd.distance_transform_edt(filled)
        lab, nl = nd.label(hole, np.ones((3, 3), bool))
        for k in range(1, nl + 1):
            m = lab == k
            s = int(m.sum())
            if s <= max_area and dep[m].min() >= depth:
                fix = cv2.inpaint(np.ascontiguousarray(cell[..., :3]), (m * 255).astype(np.uint8), 3, cv2.INPAINT_TELEA)
                cell[m, :3] = fix[m]
                cell[m, 3] = 255
                n += s
    return cell, n


def edgefix(cell):
    cell = np.array(cell, np.uint8, copy=True)
    n = 0
    for _ in range(3):
        m = we.light_edge_mask(cell)
        if m.sum() <= we.FRAME_MAX - 3:
            break
        outside = nd.binary_dilation(~nd.binary_fill_holes(cell[..., 3] >= 200), iterations=3)
        m = m & outside
        if not m.any():
            break
        cell[m, 3] = 0
        cell[m, :3] = 0
        n += int(m.sum())
    return cell, n


S8 = np.ones((3, 3), bool)
BACK_MIN, BACK_SAT, RING_BRIGHT = 232, 14, 170


def _hair_frac(rgb):
    r = rgb.astype(np.float32) + 1
    return float(((r[:, 2] / r[:, 0] < 0.70) & (r[:, 1] / r[:, 0] > 0.80) & (r[:, 0] > 170)).mean()) if len(r) else 1.0


def pocketfill(cell, raw, max_area=400, depth=3):
    cell = np.array(cell, np.uint8, copy=True)
    raw = np.asarray(raw, np.uint8)
    a = cell[..., 3]
    solid = a >= 200
    filled = nd.binary_fill_holes(solid)
    hole = filled & ~solid
    st = {'pocketfill_px': 0, 'pocket_skipped_px': 0}
    if not hole.any():
        return cell, st
    dep = nd.distance_transform_edt(filled)
    lab, nl = nd.label(hole, S8)
    ids = np.arange(1, nl + 1)
    area = nd.sum(hole, lab, ids)
    ra = raw[..., 3]
    fm = np.zeros(a.shape, bool)
    for k in ids[area <= max_area]:
        m = lab == k
        if dep[m].min() < depth:
            continue
        if (ra[m] >= 200).mean() < 0.9:
            continue                                   # clear in the raw matte too: a real gap
        u = raw[..., :3][m].astype(np.int32)
        ring = nd.binary_dilation(m, S8, iterations=3) & ~m & (a > 200)
        rr = cell[..., :3][ring]
        if _hair_frac(rr) > 0.35:
            st['pocket_skipped_px'] += int(m.sum()); continue   # backdrop between hair strands
        mn = u.min(1); sat = u.max(1) - mn
        if ((mn >= BACK_MIN) & (sat <= BACK_SAT)).mean() > 0.5:
            if not len(rr) or rr.min(1).mean() < RING_BRIGHT:
                st['pocket_skipped_px'] += int(m.sum()); continue   # backdrop-white, not inside bright skin
        fm |= m
    if fm.any():
        cell[fm, :3] = raw[fm, :3]
        cell[fm, 3] = 255
        st['pocketfill_px'] = int(fm.sum())
    return cell, st


def post(cell, max_area=80, raw=None):
    c, a = holefill(cell, max_area)
    c, b = edgefix(c)
    pf = {'pocketfill_px': 0, 'pocket_skipped_px': 0}
    if raw is not None:
        c, pf = pocketfill(c, raw)
    c, d = holefill(c, max_area)
    c[c[..., 3] == 0] = 0
    return c, {'holefill_px': a + d, 'edgefix_px': b, **pf}
