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
  post(cell)
      post.sh order: holefill -> edgefix -> holefill (holefill is always the last image step).
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


def post(cell, max_area=80):
    c, a = holefill(cell, max_area)
    c, b = edgefix(c)
    c, d = holefill(c, max_area)
    c[c[..., 3] == 0] = 0
    return c, {'holefill_px': a + d, 'edgefix_px': b}
