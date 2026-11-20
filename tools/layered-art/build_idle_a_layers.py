#!/usr/bin/env python3
"""Split idle_a into base / covered-chest / pigtail layers for engine springs.

Add-only technical separation of the already-shipped clip. Outfit is unchanged
and stays on; default composite must match the source frame within 1 level.
"""
from __future__ import annotations

import json
import math
import sys
from pathlib import Path

import cv2
import numpy as np
from PIL import Image, ImageDraw, ImageFont
from scipy import ndimage as nd

ROOT = Path("/workspace/lillith/scratch-layers1")
SHEET = ROOT / "idle_a_sheet.png"
JSON_IN = ROOT / "idle_a.json"
OUT = ROOT
CELL = 400
CHEST_SLACK = 10
PIG_SLACK = 16
FEATHER = 2.0
INPAINT_A = 0.996  # only replace nearly-opaque holdout on the base

# physics.toml [attachments.clips.idle] medians, 400 px canvas
IDLE_MEDIAN = {
    "chest": np.array([0.49 * CELL, 0.34 * CELL], dtype=np.float32),
    "pigtail_left": np.array([0.39 * CELL, 0.1875 * CELL], dtype=np.float32),
    "pigtail_right": np.array([0.5725 * CELL, 0.185 * CELL], dtype=np.float32),
    "hip": np.array([0.4825 * CELL, 0.48 * CELL], dtype=np.float32),
}

yy, xx = np.mgrid[0:CELL, 0:CELL].astype(np.float32)


def sm(x, a, b):
    t = np.clip((x - a) / max(b - a, 1e-6), 0, 1)
    return t * t * (3 - 2 * t)


def gauss(c, sx, sy):
    return np.exp(-((xx - c[0]) ** 2 / (2 * sx * sx) + (yy - c[1]) ** 2 / (2 * sy * sy)))


def hairmask(a):
    r, g, b, al = [a[..., i].astype(np.float32) for i in range(4)]
    # v1 bake_jiggle blonde lock, slightly relaxed for anti-aliased edges
    core = (al > 80) & (r > 140) & (g > 105) & (g - b > 42) & (r - b > 60) & (g <= r + 8)
    soft = (al > 40) & (r > 120) & (g > 90) & (g - b > 28) & (r - b > 40) & (g <= r + 16)
    return core, soft


def magenta_garment(a):
    r, g, b, al = [a[..., i].astype(np.int16) for i in range(4)]
    return (al > 60) & (r > 140) & (g < 110) & (b > 80) & (r - g > 50)


def horn_mask(a):
    ai = a.astype(np.int32)
    horn = (ai[..., 3] > 100) & (ai[..., 2] > ai[..., 0] + 25) & (ai[..., 2] > ai[..., 1] + 25)
    # keep it in the upper third so midriff cyan tattoos don't count
    horn &= yy < 120
    return nd.binary_dilation(horn, iterations=2)


def skin_mask(a):
    hsv = cv2.cvtColor(a[:, :, :3], cv2.COLOR_RGB2HSV)
    h, s, v = hsv[:, :, 0], hsv[:, :, 1], hsv[:, :, 2]
    al = a[:, :, 3]
    # anime skin: warm hue, mid sat, bright
    return (al > 60) & (h <= 30) & (s >= 25) & (s <= 180) & (v >= 110) & (v <= 250)


def load_frames(path: Path):
    sheet = np.asarray(Image.open(path).convert("RGBA"))
    assert sheet.shape[0] == CELL, sheet.shape
    n = sheet.shape[1] // CELL
    frames = [sheet[:, i * CELL : (i + 1) * CELL].copy() for i in range(n)]
    return frames


def detect_hip(a):
    mag = magenta_garment(a)
    # shorts live in the lower-middle; take the top of the largest lower blob
    ys, xs = np.nonzero(mag & (yy > 150) & (yy < 280))
    if len(ys) < 80:
        ys, xs = np.nonzero(a[:, :, 3] > 60)
        return np.array([xs.mean(), ys.mean()], dtype=np.float32)
    top = np.percentile(ys, 5)
    band = ys < top + 22
    return np.array([xs[band].mean(), top + 10], dtype=np.float32)


def detect_face_center(a):
    sk = skin_mask(a) & (yy < 130) & (xx > 130) & (xx < 270)
    ys, xs = np.nonzero(sk)
    if len(ys) < 40:
        return np.array([200.0, 80.0], dtype=np.float32)
    return np.array([xs.mean(), ys.mean()], dtype=np.float32)


def detect_pigtail_roots(a, hip, face):
    core, soft = hairmask(a)
    hm = core | (soft & nd.binary_dilation(core, iterations=2))
    hm = hm & ~horn_mask(a)
    hx, hy = float(face[0]), float(face[1])
    # v1 band relative to hip, plus a face-relative band
    band = (yy > hy - 160) & (yy < hy - 30) & (yy < face[1] + 70)
    pl = hm & band & (xx < hx - 14)
    pr = hm & band & (xx > hx + 14)

    def root_of(m, fallback, side):
        if m.sum() < 30:
            return fallback.copy()
        ys, xs = np.nonzero(m)
        # the root is near the head: innermost + highest cluster
        if side < 0:
            # left: high x (towards face) and low y
            score = (xs.astype(np.float32) * 0.6) + (-ys.astype(np.float32) * 1.0)
        else:
            score = (-xs.astype(np.float32) * 0.6) + (-ys.astype(np.float32) * 1.0)
        # take the top 20% by score and use their median
        k = max(8, int(0.18 * len(score)))
        idx = np.argpartition(score, -k)[-k:]
        return np.array([xs[idx].mean(), ys[idx].mean()], dtype=np.float32)

    pl_fb = IDLE_MEDIAN["pigtail_left"].copy()
    pr_fb = IDLE_MEDIAN["pigtail_right"].copy()
    # shift fallback with face motion vs median face (~200, 80)
    shift = face - np.array([200.0, 80.0], dtype=np.float32)
    pl_fb = pl_fb + shift * 0.6
    pr_fb = pr_fb + shift * 0.6
    return root_of(pl, pl_fb, -1), root_of(pr, pr_fb, 1)


def pigtail_coverage(a, root, face, side):
    """Soft coverage 0..1 for one pigtail. side=-1 left, +1 right."""
    core, soft = hairmask(a)
    hm = (core | soft).astype(np.float32)
    # include purple/cyan tie pixels near the root
    ai = a.astype(np.int16)
    al = a[:, :, 3] > 40
    purple = al & (ai[..., 0] > 120) & (ai[..., 2] > 120) & (ai[..., 1] < 110)
    cyan = al & (ai[..., 2] > ai[..., 0] + 15) & (ai[..., 2] > ai[..., 1] + 15)
    near = gauss(root, 14, 12) > 0.15
    acc = (purple | cyan) & near & (yy > root[1] - 18)
    # hair + ties, dilated a touch for AA
    seed = (hm > 0) | acc
    seed = nd.binary_dilation(seed, iterations=1)

    # spatial prior: gaussian hanging from the root, mostly below it
    spatial = gauss([root[0], root[1] + 16], 20, 32) * sm(yy, root[1] - 16, root[1] - 2)
    # keep off the face
    fx = float(face[0])
    face_gate = sm(np.abs(xx - fx), 18, 30)
    # keep off horns and crown (above the root, towards centre)
    horns = horn_mask(a)
    horn_g = 1.0 - np.clip(
        1.4 * nd.gaussian_filter(horns.astype(np.float32), 1.5), 0, 1
    )
    # crown: between the ties, above mid-root, near centre
    if side < 0:
        outward = sm(side * (xx - root[0]), -10, 4)  # allow a little inward
    else:
        outward = sm(side * (xx - root[0]), -10, 4)
    crown_cut = 1.0 - sm(yy, root[1] - 8, root[1] + 2) * sm(np.abs(xx - fx), 8, 36) * (
        1.0 - outward
    )

    field = seed.astype(np.float32) * spatial * face_gate * horn_g * crown_cut

    # connected keep: flood from the root through field>thresh
    hard = field > 0.08
    # also take hair pixels under a slightly looser field so the fluff is complete
    loose = (seed & (gauss([root[0], root[1] + 18], 26, 40) > 0.04) & (face_gate > 0.2)
             & (yy > root[1] - 8) & ~horns)
    if side < 0:
        loose &= xx < fx + 6
    else:
        loose &= xx > fx - 6
    hard = hard | loose

    # connected component from a seed disk at the root
    lab, n = nd.label(hard, np.ones((3, 3), bool))
    sy, sx = int(round(root[1])), int(round(root[0]))
    sy = int(np.clip(sy, 0, CELL - 1))
    sx = int(np.clip(sx, 0, CELL - 1))
    keep = np.zeros_like(hard)
    # seed: any labeled pixel in a 10px disk around the root, plus a bit below
    yy0, xx0 = np.ogrid[-12:13, -12:13]
    disk = xx0 ** 2 + yy0 ** 2 <= 12 ** 2
    y0, y1 = max(0, sy - 12), min(CELL, sy + 13)
    x0, x1 = max(0, sx - 12), min(CELL, sx + 13)
    dy0, dy1 = y0 - (sy - 12), 25 - ((sy + 13) - y1)
    dx0, dx1 = x0 - (sx - 12), 25 - ((sx + 13) - x1)
    seed_disk = np.zeros_like(hard)
    seed_disk[y0:y1, x0:x1] = disk[dy0:dy0 + (y1 - y0), dx0:dx0 + (x1 - x0)]
    ids = set(lab[seed_disk & (lab > 0)].tolist())
    # also accept components whose centroid is clearly in the pigtail lobe
    for i in range(1, n + 1):
        m = lab == i
        if i in ids:
            keep |= m
            continue
        ys, xs = np.nonzero(m)
        if len(ys) < 20:
            continue
        cx, cy = xs.mean(), ys.mean()
        if side * (cx - fx) > 16 and cy > root[1] - 6 and abs(cx - root[0]) < 70:
            keep |= m

    # soft coverage
    dist_out = nd.distance_transform_edt(~keep)
    dist_in = nd.distance_transform_edt(keep)
    cov = np.zeros_like(field, dtype=np.float32)
    cov[keep] = 1.0
    # feather out
    cov = np.clip(dist_in / max(FEATHER, 0.5), 0, 1) * keep + np.clip(
        1.0 - dist_out / max(FEATHER, 0.5), 0, 1
    ) * (~keep)
    cov = np.clip(cov, 0, 1) * (a[:, :, 3] > 8).astype(np.float32)
    # suppress residual face/horn
    cov = cov * face_gate * horn_g
    return cov.astype(np.float32)


def torso_x_runs(a, y0, y1, mid_x):
    """Per-row central opaque run (torso, not arms) between y0 and y1."""
    al = a[:, :, 3] > 40
    left = np.full(CELL, np.nan, np.float32)
    right = np.full(CELL, np.nan, np.float32)
    mid = int(round(mid_x))
    for y in range(max(0, int(y0)), min(CELL, int(y1) + 1)):
        xs = np.nonzero(al[y])[0]
        if len(xs) == 0:
            continue
        # split into runs
        gaps = np.where(np.diff(xs) > 3)[0]
        starts = np.r_[xs[0], xs[gaps + 1]]
        ends = np.r_[xs[gaps], xs[-1]]
        # pick the run that contains mid_x, else closest
        pick = None
        best = 1e9
        for s, e in zip(starts, ends):
            if s - 2 <= mid <= e + 2:
                pick = (s, e)
                break
            d = min(abs(s - mid), abs(e - mid))
            if d < best:
                best, pick = d, (s, e)
        if pick:
            left[y], right[y] = pick
    return left, right


def detect_chest_point(a, hip, face, prev=None):
    # start from hip-relative v1 estimate, blended with idle median + face shift
    est = np.array([hip[0] + 3, hip[1] - 56], dtype=np.float32)
    med = IDLE_MEDIAN["chest"] + (face - np.array([200.0, 80.0], np.float32)) * 0.4
    est = 0.45 * est + 0.55 * med
    if prev is not None:
        est = 0.65 * prev + 0.35 * est
    # snap to COM of garment+bust in a window
    y0, y1 = int(est[1] - 28), int(est[1] + 24)
    x0, x1 = int(est[0] - 32), int(est[0] + 32)
    y0, y1 = max(0, y0), min(CELL, y1)
    x0, x1 = max(0, x0), min(CELL, x1)
    mag = magenta_garment(a)
    sk = skin_mask(a)
    al = a[:, :, 3] > 60
    # black cups: dark, in window, not sheer-sleeve gray
    hsv = cv2.cvtColor(a[:, :, :3], cv2.COLOR_RGB2HSV)
    dark = al & (hsv[:, :, 2] < 80) & (yy > est[1] - 30) & (yy < est[1] + 22)
    region = np.zeros((CELL, CELL), bool)
    region[y0:y1, x0:x1] = True
    m = region & (mag | (sk & (yy > face[1] + 20)) | dark)
    ys, xs = np.nonzero(m)
    if len(ys) < 30:
        return est
    # COM weighted a bit toward mid-x
    cx, cy = float(xs.mean()), float(ys.mean())
    return np.array([0.7 * cx + 0.3 * est[0], 0.7 * cy + 0.3 * est[1]], dtype=np.float32)


def detect_underbust(a, chest, hip, mag, dark, sk, al):
    """Last row of the top garment in the torso column, just above midriff skin."""
    mid = int(round(chest[0]))
    y0 = int(chest[1])
    y1 = int(min(hip[1] - 8, chest[1] + 40))
    last = int(chest[1] + 16)
    for y in range(y0, y1 + 1):
        xs = np.nonzero(al[y] & (np.abs(np.arange(CELL) - mid) < 36))[0]
        if len(xs) == 0:
            continue
        # garment (top) vs skin-only in the central 28 px
        cen = (np.abs(np.arange(CELL) - mid) < 22)
        g = int(np.count_nonzero((mag[y] | dark[y]) & cen))
        s = int(np.count_nonzero(sk[y] & cen & ~(mag[y] | dark[y])))
        if g >= 6:
            last = y
        elif y > chest[1] + 8 and s > g * 2 and s >= 8 and g <= 3:
            break
    return float(last)


def chest_coverage(a, chest, face, hip):
    """Soft coverage of the covered bust + top (top stays on)."""
    al = a[:, :, 3] > 40
    mag = magenta_garment(a)
    sk = skin_mask(a)
    hsv = cv2.cvtColor(a[:, :, :3], cv2.COLOR_RGB2HSV)
    dark = al & (hsv[:, :, 2] < 90)
    # cyan gem / jewellery on the sternum (not the neck choker too high)
    ai = a.astype(np.int16)
    cyan = al & (ai[..., 2] > ai[..., 0] + 15) & (ai[..., 2] > ai[..., 1] + 10) & (
        yy > chest[1] - 22
    ) & (yy < chest[1] + 16) & (np.abs(xx - chest[0]) < 22)

    y_bot = detect_underbust(a, chest, hip, mag, dark, sk, al)
    y_bot = min(y_bot + 2.0, chest[1] + 24, hip[1] - 24)  # a couple of px of skin, not midriff
    y_top = chest[1] - 26
    y_gate = sm(yy, y_top - 4, y_top + 4) * (1.0 - sm(yy, y_bot - 1, y_bot + 5))

    # torso column, not arms: prefer the central run, then clamp to bust width
    left, right = torso_x_runs(a, y_top - 8, y_bot + 6, chest[0])
    # bust width at the cup row (not the merged shoulder/arm row)
    cy = int(round(chest[1] + 2))
    half = 28.0
    if 0 <= cy < CELL and not np.isnan(left[cy]):
        half = 0.5 * float(right[cy] - left[cy])
        half = float(np.clip(half, 22, 34))
    torso = np.zeros((CELL, CELL), np.float32)
    for y in range(CELL):
        if not np.isnan(left[y]):
            l = max(0, int(left[y]))
            r = min(CELL, int(right[y]) + 1)
            # clamp to bust half-width (straps are inside this; sleeve caps are not)
            l = max(l, int(round(chest[0] - half - 3)))
            r = min(r, int(round(chest[0] + half + 3)))
            if r > l:
                torso[y, l:r] = 1.0
    torso = nd.gaussian_filter(torso, 0.6)
    torso = np.clip(torso * 1.25, 0, 1)

    spatial = gauss([chest[0], chest[1] + 1], 20, 14) * y_gate

    garment = (mag | dark | cyan | (sk & (yy > chest[1] - 22) & (yy < y_bot + 1)))
    hard = garment & (torso > 0.45) & (spatial > 0.06) & al

    # close small holes in cups / gem
    hard = nd.binary_closing(hard, iterations=2)
    hard = nd.binary_fill_holes(hard)

    lab, nlab = nd.label(hard, np.ones((3, 3), bool))
    cyi, cxi = int(round(chest[1])), int(round(chest[0]))
    cyi = int(np.clip(cyi, 0, CELL - 1))
    cxi = int(np.clip(cxi, 0, CELL - 1))
    keep = np.zeros_like(hard)
    if lab[cyi, cxi] > 0:
        keep = lab == lab[cyi, cxi]
    else:
        best, bid = 1e9, None
        for i in range(1, nlab + 1):
            ys, xs = np.nonzero(lab == i)
            d = (xs.mean() - chest[0]) ** 2 + (ys.mean() - chest[1]) ** 2
            if d < best:
                best, bid = d, i
        if bid is not None:
            keep = lab == bid

    extra = (spatial > 0.16) & (torso > 0.5) & al & (sk | mag | dark | cyan) & y_gate.astype(bool)
    keep = (keep | extra) & (yy > face[1] + 18) & (torso > 0.35)
    # drop sleeve caps: dark/magenta stubs lateral to the bust, not the upward straps
    hsv2 = cv2.cvtColor(a[:, :, :3], cv2.COLOR_RGB2HSV)
    dark2 = (a[:, :, 3] > 40) & (hsv2[:, :, 2] < 90)
    cy0, cy1 = int(max(0, chest[1] - 4)), int(min(CELL, chest[1] + 12))
    sk_band = skin_mask(a)[cy0:cy1] & (np.abs(xx[cy0:cy1] - chest[0]) < 48)
    if sk_band.any():
        xs = np.where(sk_band)[1]
        half_skin = float(np.clip(np.percentile(np.abs(xs.astype(np.float32) - chest[0]), 97), 22, 32))
    else:
        half_skin = 28.0
    sleeve = (np.abs(xx - chest[0]) > half_skin + 0.5) & (yy > chest[1] - 8) & (dark2 | magenta_garment(a))
    keep = keep & ~nd.binary_dilation(sleeve, iterations=1)
    keep = nd.binary_fill_holes(keep)

    dist_out = nd.distance_transform_edt(~keep)
    dist_in = nd.distance_transform_edt(keep)
    cov = np.zeros((CELL, CELL), np.float32)
    cov[keep] = 1.0
    cov = np.clip(dist_in / max(FEATHER, 0.5), 0, 1) * keep + np.clip(
        1.0 - dist_out / max(FEATHER, 0.5), 0, 1
    ) * (~keep)
    cov = np.clip(cov * y_gate * np.clip(torso + 0.05, 0, 1), 0, 1)
    cov = cov * (a[:, :, 3] > 8).astype(np.float32)
    return cov.astype(np.float32)


def to_premult(a):
    p = a.astype(np.float32)
    alpha = p[:, :, 3:4] / 255.0
    rgb = p[:, :, :3] * alpha
    return np.concatenate([rgb, p[:, :, 3:4]], axis=2)


def from_premult(p):
    a = np.clip(p[:, :, 3], 0, 255)
    rgb = np.zeros_like(p[:, :, :3])
    m = a > 0.5
    rgb[m] = p[:, :, :3][m] / (a[m, None] / 255.0)
    out = np.concatenate([rgb, a[..., None]], axis=2)
    out = np.clip(np.rint(out), 0, 255).astype(np.uint8)
    out[out[:, :, 3] == 0, :3] = 0  # no RGB junk in clear pixels
    return out


def peel_layer(src_pre, coverage):
    """Un-over: extract layer = src * M, remainder satisfies over(layer, rest)=src."""
    M = np.clip(coverage, 0, 1).astype(np.float32)
    layer = src_pre.copy()
    layer[:, :, :3] = src_pre[:, :, :3] * M[..., None]
    layer[:, :, 3] = src_pre[:, :, 3] * M
    la = layer[:, :, 3] / 255.0
    rest = np.zeros_like(src_pre)
    denom = np.clip(1.0 - la, 0, 1)
    visible = denom > 1e-4
    rest[:, :, :3][visible] = (src_pre[:, :, :3][visible] - layer[:, :, :3][visible]) / denom[visible, None]
    rest[:, :, 3][visible] = (src_pre[:, :, 3][visible] - layer[:, :, 3][visible]) / denom[visible]
    # hidden pixels left at 0 for now (inpainted later)
    return layer, rest, la


def inpaint_base(rest_pre, hidden_or_slack, src_u8, chest_donor=False):
    """Inpaint premultiplied rest in the slack/hidden region. Preserve exact remainder elsewhere."""
    # Work in straight RGBA for cv2.inpaint, then convert back.
    rest_u8 = from_premult(rest_pre)
    mask = (hidden_or_slack > 0.5).astype(np.uint8)
    if mask.sum() == 0:
        return rest_pre
    # dilate slightly so inpaint has a clean boundary
    mask_d = cv2.dilate(mask, np.ones((3, 3), np.uint8))
    rgb = rest_u8[:, :, :3].copy()
    alpha = rest_u8[:, :, 3].copy()
    rgb_src = src_u8[:, :, :3].copy()
    a_src = src_u8[:, :, 3].copy()
    hint = rgb.copy()
    hint_a = alpha.copy()
    # For body-behind-chest: copy midriff / lower-torso rows upward as a donor
    # so the hole fills with plausible covered-torso skin, not a ghost of the bust.
    midriff = (a_src > 80) & (yy > 150) & (yy < 230)
    if chest_donor and midriff.any() and mask_d[100:170, 150:250].sum() > 200:
        ys, xs = np.nonzero(midriff)
        y_top_m = int(ys.min())
        donor_row = None
        for y in range(y_top_m, min(y_top_m + 18, CELL)):
            if np.count_nonzero((a_src[y] > 80) & (xx[y] > 160) & (xx[y] < 240)) >= 20:
                donor_row = y
                break
        if donor_row is not None:
            row_rgb = rgb_src[donor_row : donor_row + 8].mean(axis=0)
            # use remainder alpha just below the hole if available, else source
            row_a = hint_a[donor_row : donor_row + 8].mean(axis=0)
            if row_a.max() < 80:
                row_a = a_src[donor_row : donor_row + 8].mean(axis=0)
            row_a = np.maximum(row_a, 200)  # body stays opaque
            hole_ys = np.where(mask_d.any(axis=1))[0]
            for y in hole_ys:
                sel = mask_d[y] > 0
                hint[y, sel] = row_rgb[sel]
                hint_a[y, sel] = row_a[sel]
            hint_a = np.clip(hint_a, 0, 255)
    in_rgb = cv2.inpaint(hint, mask_d, 7, cv2.INPAINT_TELEA)
    in_a = cv2.inpaint(hint_a, mask_d, 7, cv2.INPAINT_TELEA)
    out = rest_u8.copy()
    m = mask_d.astype(bool)
    out[:, :, :3][m] = in_rgb[m]
    out[:, :, 3][m] = in_a[m]
    # Air vs enclosed body hole:
    #   * chest_donor: the bust hole is enclosed by the torso — always fill.
    #   * otherwise: a transparent component that touches the image border is
    #     outside the silhouette (pigtail hanging in the air) and stays clear.
    #     Enclosed components (hole over the shoulder/head) stay filled.
    if not chest_donor:
        known_body = (out[:, :, 3] > 20) & (mask_d == 0)
        # Pixels in the hole that sit against remaining head/shoulder/hair:
        # fill these (nearby hair), do not treat as air, otherwise a 16px
        # pigtail move opens a bite in the scalp.
        root_band = (mask_d > 0) & (nd.binary_dilation(known_body, iterations=14))
        trans = ((out[:, :, 3] < 20) | (mask_d > 0)) & ~known_body & ~root_band
        lab, nlab = nd.label(trans, np.ones((3, 3), bool))
        border = np.unique(np.concatenate([
            lab[0], lab[-1], lab[:, 0], lab[:, -1]
        ]))
        air = np.zeros_like(mask_d, bool)
        for lid in border:
            if lid == 0:
                continue
            air |= lab == lid
        out[m.astype(bool) & air] = 0
    return to_premult(out)


def over(top_pre, bot_pre):
    ta = np.clip(top_pre[:, :, 3:4] / 255.0, 0, 1)
    out = np.empty_like(top_pre)
    out[:, :, :3] = top_pre[:, :, :3] + bot_pre[:, :, :3] * (1.0 - ta)
    out[:, :, 3] = top_pre[:, :, 3] + bot_pre[:, :, 3] * (1.0 - ta[:, :, 0])
    return out


def shift_pre(img_pre, dx, dy, pivot=None, angle_deg=0.0):
    """Translate (and optional rotate about pivot) a premultiplied layer."""
    straight = from_premult(img_pre)
    M = np.array([[1, 0, dx], [0, 1, dy]], dtype=np.float32)
    if angle_deg and pivot is not None:
        rot = cv2.getRotationMatrix2D((float(pivot[0]), float(pivot[1])), angle_deg, 1.0)
        rot[0, 2] += dx
        rot[1, 2] += dy
        M = rot
    out = cv2.warpAffine(
        straight,
        M,
        (CELL, CELL),
        flags=cv2.INTER_LINEAR,
        borderMode=cv2.BORDER_CONSTANT,
        borderValue=(0, 0, 0, 0),
    )
    return to_premult(out)


def pack_sheet(frames_u8):
    n = len(frames_u8)
    sheet = np.zeros((CELL, CELL * n, 4), np.uint8)
    for i, fr in enumerate(frames_u8):
        sheet[:, i * CELL : (i + 1) * CELL] = fr
    return sheet


def overlay_debug(src, chest_c, pigl_c, pigr_c, chest_pt, pl, pr, hip, face, torso_pt):
    vis = src.copy()
    # tint layers
    def tint(cov, color):
        nonlocal vis
        a = np.clip(cov, 0, 1)
        vis = vis.astype(np.float32)
        vis[:, :, :3] = vis[:, :, :3] * (1 - 0.45 * a[..., None]) + np.array(color) * (0.45 * a[..., None])
        vis = np.clip(vis, 0, 255).astype(np.uint8)

    tint(chest_c, (255, 60, 160))
    tint(pigl_c, (80, 200, 255))
    tint(pigr_c, (80, 255, 120))
    im = Image.fromarray(vis)
    d = ImageDraw.Draw(im)

    def dot(p, col, r=3):
        x, y = float(p[0]), float(p[1])
        d.ellipse([x - r, y - r, x + r, y + r], outline=col, fill=col)

    dot(chest_pt, (255, 0, 0), 4)
    dot(pl, (0, 180, 255), 4)
    dot(pr, (0, 255, 80), 4)
    dot(hip, (255, 200, 0), 3)
    dot(face, (255, 255, 0), 3)
    dot(torso_pt, (255, 128, 0), 3)
    return np.asarray(im)


def finite_diff_loop(P, fps):
    """P: (N,2) positions. Returns delta, vel (px/s), acc (px/s^2), wrapped for a loop."""
    n = len(P)
    dt = 1.0 / float(fps)
    delta = np.zeros_like(P)
    vel = np.zeros_like(P)
    acc = np.zeros_like(P)
    for i in range(n):
        pm = P[(i - 1) % n]
        pp = P[(i + 1) % n]
        delta[i] = P[i] - pm
        vel[i] = (pp - pm) / (2 * dt)
        acc[i] = (pp - 2 * P[i] + pm) / (dt * dt)
    return delta, vel, acc


def main():
    meta = json.loads(JSON_IN.read_text())
    fps = float(meta["fps"])
    frames = load_frames(SHEET)
    n = len(frames)
    assert n == int(meta["frame_count"]) == 72, n
    bake = meta.get("chest_bounce_bake") or {}
    baked_off = np.array(bake.get("baked_chest_offsets_px") or [[0, 0]] * n, dtype=np.float32)
    if len(baked_off) != n:
        baked_off = np.zeros((n, 2), np.float32)

    faces, hips, chests, pigl, pigr, torsos = [], [], [], [], [], []
    cov_chest, cov_pl, cov_pr = [], [], []

    prev_c = None
    for i, fr in enumerate(frames):
        hip = detect_hip(fr)
        face = detect_face_center(fr)
        chest = detect_chest_point(fr, hip, face, prev_c)
        pl, pr = detect_pigtail_roots(fr, hip, face)
        # roots live at the ties (~y 70-80), never up on the horns
        def _clamp_root(p, fb, prev):
            p = p.copy()
            ymin, ymax = float(face[1] - 18), float(face[1] + 28)
            ymin = max(58.0, ymin)
            ymax = min(100.0, ymax)
            if p[1] < ymin or p[1] > ymax:
                p = fb.copy()
                p = p + (face - np.array([200.0, 80.0], np.float32)) * 0.5
            if prev is not None and np.linalg.norm(p - prev) > 14:
                p = 0.75 * prev + 0.25 * p
            p[1] = float(np.clip(p[1], ymin, ymax))
            return p
        fb_l = IDLE_MEDIAN["pigtail_left"]
        fb_r = IDLE_MEDIAN["pigtail_right"]
        pl = _clamp_root(pl, fb_l, pigl[-1] if pigl else None)
        pr = _clamp_root(pr, fb_r, pigr[-1] if pigr else None)
        # torso anchor: midriff just below underbust (body, not the bouncing bust)
        torso = np.array([chest[0], min(hip[1] - 8, chest[1] + 28)], dtype=np.float32)
        # COM of midriff skin band
        sk = skin_mask(fr)
        y0, y1 = int(chest[1] + 18), int(hip[1] - 4)
        band = sk & (yy >= y0) & (yy <= y1) & (np.abs(xx - chest[0]) < 30)
        ys, xs = np.nonzero(band)
        if len(ys) >= 20:
            torso = np.array([xs.mean(), ys.mean()], dtype=np.float32)

        cc = chest_coverage(fr, chest, face, hip)
        cl = pigtail_coverage(fr, pl, face, -1)
        cr = pigtail_coverage(fr, pr, face, 1)
        # resolve overlap: pigtails win over chest (hair in front)
        overlap_l = np.minimum(cc, cl)
        overlap_r = np.minimum(cc, cr)
        cc = np.clip(cc - overlap_l - overlap_r, 0, 1)
        # left/right pigtail overlap (rare): split by side of face
        both = np.minimum(cl, cr)
        if both.max() > 0:
            mid = 0.5 * (pl[0] + pr[0])
            cl = np.where(xx <= mid, cl, cl - both)
            cr = np.where(xx > mid, cr, cr - both)
            cl = np.clip(cl, 0, 1)
            cr = np.clip(cr, 0, 1)

        faces.append(face)
        hips.append(hip)
        chests.append(chest)
        pigl.append(pl)
        pigr.append(pr)
        torsos.append(torso)
        cov_chest.append(cc)
        cov_pl.append(cl)
        cov_pr.append(cr)
        prev_c = chest
        print(
            f"f{i:02d} chest={chest[0]:.1f},{chest[1]:.1f} pl={pl[0]:.1f},{pl[1]:.1f} "
            f"pr={pr[0]:.1f},{pr[1]:.1f} hip={hip[0]:.1f},{hip[1]:.1f} "
            f"cov px C/L/R={cc.sum():.0f}/{cl.sum():.0f}/{cr.sum():.0f}",
            flush=True,
        )

    # temporal smooth of pivots (light)
    def smooth(arr, w=0.35):
        a = np.stack(arr)
        out = a.copy()
        for _ in range(2):
            nxt = out.copy()
            for i in range(len(a)):
                nxt[i] = (1 - 2 * w / 2) * out[i] + (w / 2) * out[(i - 1) % n] + (w / 2) * out[(i + 1) % n]
            # keep first close? it's a loop, fine
            out = nxt
        return out

    chests = smooth(chests, 0.25)
    pigl = smooth(pigl, 0.25)
    pigr = smooth(pigr, 0.25)
    torsos = smooth(torsos, 0.25)
    hips = smooth(hips, 0.25)

    torso_delta, torso_vel, torso_acc = finite_diff_loop(torsos, fps)
    chest_delta, chest_vel, chest_acc = finite_diff_loop(chests, fps)

    # peel + fill
    z_order = ["base", "chest", "pigtail_left", "pigtail_right"]
    layers_out = {k: [] for k in z_order}
    qa = []
    debug_idx = {0, 8, 18, 27, 36, 45, 54, 63, 71}

    for i, fr in enumerate(frames):
        src_pre = to_premult(fr)
        # peel top-first: right pigtail, left pigtail, chest
        rest = src_pre
        peeled = {}
        las = {}
        for name, cov in (
            ("pigtail_right", cov_pr[i]),
            ("pigtail_left", cov_pl[i]),
            ("chest", cov_chest[i]),
        ):
            layer, rest, la = peel_layer(rest, cov)
            peeled[name] = layer
            las[name] = la
            rest = rest

        # Sequential fill: pigtails first (neighbors = leftover head hair / air),
        # then chest (midriff donor). Then keep fill on a pixel only when
        # over(layers, fill) still matches the source within 1 level; otherwise
        # keep the exact un-over remainder. This maximises slack-fill coverage
        # without breaking the zero-offset contract.
        pig_wide = np.maximum((cov_pl[i] > 0.12).astype(np.uint8), (cov_pr[i] > 0.12).astype(np.uint8))
        chest_wide = (cov_chest[i] > 0.12).astype(np.uint8)
        filled = rest
        if chest_wide.any():
            filled = inpaint_base(filled, chest_wide.astype(np.float32), fr, chest_donor=True)
        if pig_wide.any():
            filled = inpaint_base(filled, pig_wide.astype(np.float32), fr)
        tops = peeled["chest"]
        tops = over(peeled["pigtail_left"], tops)
        tops = over(peeled["pigtail_right"], tops)
        la = np.clip(tops[:, :, 3] / 255.0, 0, 1)
        # Choose fill only where the straight 8-bit composite stays within 1
        # level of the shipped frame (premult error is not the acceptance test).
        comp_fill = from_premult(over(tops, filled))
        use_fill = np.max(np.abs(comp_fill.astype(np.int16) - fr.astype(np.int16)), axis=2) <= 1
        use_fill |= la >= INPAINT_A  # fully hidden interiors always take fill
        rest_filled = rest.copy()
        rest_filled[use_fill] = filled[use_fill]

        layers_out["base"].append(from_premult(rest_filled))
        layers_out["chest"].append(from_premult(peeled["chest"]))
        layers_out["pigtail_left"].append(from_premult(peeled["pigtail_left"]))
        layers_out["pigtail_right"].append(from_premult(peeled["pigtail_right"]))

        # zero-offset composite exactness
        comp = rest_filled
        for name in ("chest", "pigtail_left", "pigtail_right"):
            comp = over(peeled[name], comp)
        src_u = fr.astype(np.int16)
        cu = from_premult(comp).astype(np.int16)
        # only compare where source or comp has alpha (ignore both-empty)
        diff = np.abs(src_u - cu)
        mae = diff.mean()
        maxd = int(diff.max())
        # per-channel max among pixels with src or comp alpha > 0
        active = (src_u[:, :, 3] > 0) | (cu[:, :, 3] > 0)
        max_active = int(diff[active].max()) if active.any() else 0
        n_bad = int((diff.max(axis=2) > 1).sum())
        qa.append(dict(i=i, mae=round(float(mae), 4), max=maxd, max_active=max_active, n_gt1=n_bad))

        if i in debug_idx:
            ov = overlay_debug(
                fr,
                cov_chest[i],
                cov_pl[i],
                cov_pr[i],
                chests[i],
                pigl[i],
                pigr[i],
                hips[i],
                faces[i],
                torsos[i],
            )
            Image.fromarray(ov).save(OUT / "overlays" / f"mask_{i:02d}.png")
            # also write individual layer previews on dark
            for nm, im in (
                ("base", layers_out["base"][-1]),
                ("chest", layers_out["chest"][-1]),
                ("pigl", layers_out["pigtail_left"][-1]),
                ("pigr", layers_out["pigtail_right"][-1]),
                ("comp", from_premult(comp)),
            ):
                Image.fromarray(im).save(OUT / "overlays" / f"{nm}_{i:02d}.png")
        print(f"peel {i:02d} mae={mae:.4f} max={maxd} max_active={max_active} n>1={n_bad}", flush=True)

    # write sheets
    layer_dir = OUT / "layers"
    layer_dir.mkdir(exist_ok=True)
    names = {
        "base": "base_sheet.png",
        "chest": "chest_sheet.png",
        "pigtail_left": "pigtail_l_sheet.png",
        "pigtail_right": "pigtail_r_sheet.png",
    }
    for k, fn in names.items():
        sh = pack_sheet(layers_out[k])
        Image.fromarray(sh).save(layer_dir / fn, compress_level=6)
        print("wrote", fn, sh.shape, (layer_dir / fn).stat().st_size, flush=True)

    # layers json
    frames_json = []
    for i in range(n):
        frames_json.append(
            {
                "i": i,
                "pivots": {
                    "chest": [round(float(chests[i][0]), 2), round(float(chests[i][1]), 2)],
                    "pigtail_left": [round(float(pigl[i][0]), 2), round(float(pigl[i][1]), 2)],
                    "pigtail_right": [round(float(pigr[i][0]), 2), round(float(pigr[i][1]), 2)],
                    "torso_anchor": [round(float(torsos[i][0]), 2), round(float(torsos[i][1]), 2)],
                    "hip": [round(float(hips[i][0]), 2), round(float(hips[i][1]), 2)],
                    "face": [round(float(faces[i][0]), 2), round(float(faces[i][1]), 2)],
                },
                "torso_delta_px": [round(float(torso_delta[i][0]), 3), round(float(torso_delta[i][1]), 3)],
                "torso_vel_px_s": [round(float(torso_vel[i][0]), 3), round(float(torso_vel[i][1]), 3)],
                "torso_acc_px_s2": [round(float(torso_acc[i][0]), 3), round(float(torso_acc[i][1]), 3)],
                "chest_pivot_delta_px": [round(float(chest_delta[i][0]), 3), round(float(chest_delta[i][1]), 3)],
                "chest_pivot_vel_px_s": [round(float(chest_vel[i][0]), 3), round(float(chest_vel[i][1]), 3)],
                "chest_pivot_acc_px_s2": [round(float(chest_acc[i][0]), 3), round(float(chest_acc[i][1]), 3)],
                "baked_chest_offset_px": [round(float(baked_off[i][0]), 3), round(float(baked_off[i][1]), 3)],
            }
        )

    max_err = max(q["max_active"] for q in qa)
    n_bad_frames = sum(1 for q in qa if q["n_gt1"] > 0)
    doc = {
        "schema": "lillith.layered_art.v1",
        "clip": "idle_a",
        "aliases": ["idle"],
        "source_sheet": "idle_a_sheet.png",
        "canvas": [400, 400],
        "fps": fps,
        "frame_count": n,
        "layout": "horizontal_strip",
        "composite_rule": "over",
        "color": "straight_rgba",
        "z_order": z_order,
        "fill_slack_px": {"chest": CHEST_SLACK, "pigtail_left": PIG_SLACK, "pigtail_right": PIG_SLACK},
        "feather_px": FEATHER,
        "y_down": True,
        "pivot_origin": "frame_top_left",
        "baked_motion": {
            "note": "idle_a already has chest-bounce-3 baked into the pixels (pigtails/hip/tail were not re-warped in that bake). Layer pivots move with that baked motion. Engine springs should be added on top only as extra bounce (see docs/CHEST_BOUNCE.md baked_layer), or used in place of the bake after a rebake; do not double the same motion at 100% intensity.",
            "batch": bake.get("batch", "art/chest-bounce-3"),
            "max_chest_offset_px": bake.get("max_chest_offset_px_normal_framing"),
            "chest_track_ncc_min": bake.get("chest_track_ncc_min"),
        },
        "sheets": {k: f"layers/{fn}" for k, fn in names.items()},
        "layers": [
            {
                "id": "base",
                "sheet": "layers/base_sheet.png",
                "z": 0,
                "movable": False,
                "spring": None,
                "role": "body, head, arms, outfit, tail; chest and pigtail footprints inpainted with slack fill",
            },
            {
                "id": "chest",
                "sheet": "layers/chest_sheet.png",
                "z": 1,
                "movable": True,
                "spring": "chest",
                "spring_sign": 1.0,
                "pivot": "chest",
                "max_offset_px": CHEST_SLACK,
                "motion": "translation around pivot (vertical bounce + optional squash)",
                "role": "covered upper torso and bust with her top on, opaque, same coverage as the source",
            },
            {
                "id": "pigtail_left",
                "sheet": "layers/pigtail_l_sheet.png",
                "z": 2,
                "movable": True,
                "spring": "pigtails",
                "spring_sign": -1.0,
                "pivot": "pigtail_left",
                "max_offset_px": PIG_SLACK,
                "motion": "translation + small rotation about the root at the head",
                "role": "screen-left pigtail (hair + tie); scalp, bangs, horns and face stay on base",
            },
            {
                "id": "pigtail_right",
                "sheet": "layers/pigtail_r_sheet.png",
                "z": 3,
                "movable": True,
                "spring": "pigtails",
                "spring_sign": 1.0,
                "pivot": "pigtail_right",
                "max_offset_px": PIG_SLACK,
                "motion": "translation + small rotation about the root at the head",
                "role": "screen-right pigtail (hair + tie); scalp, bangs, horns and face stay on base",
            },
        ],
        "body_hints_note": "Per-frame torso_anchor is a midriff point just below the bust (not the bouncing chest). torso_delta_px is the wrap-around delta from the previous frame; vel/acc use central differences on the loop, units px/s and px/s^2, +y down. Drive chest/pigtail springs from torso_acc (and/or torso_vel) so bounce ramps up with body motion. chest_pivot_* includes baked chest bounce; baked_chest_offset_px is the chest-bounce-3 bake residual.",
        "composite_exactness": {
            "goal": "zero-offset over() composite matches the shipped frame within 1 level",
            "max_abs_active": max_err,
            "frames_with_gt1": n_bad_frames,
            "per_frame": qa,
        },
        "engine_notes": {
            "channels": "Existing lillith-core springs: Channel::Chest ('chest') and Channel::Pigtail ('pigtails'/'pigtail'/'pig'). Attachment signs match assets/physics.toml: pigtail_left sign -1, pigtail_right sign +1. feat/layered-jiggle-physics had no unique layer format yet (branch pointed at origin/main 1184b90d with no extra commits); this schema is the proposed input.",
            "consume": "Load the four sheets, composite in z_order with over(), apply spring offsets as translations (and pigtail rotation) about the per-frame pivot. At zero offsets the result must match the current atlas cell. Do not change manifest.toml to enable this; a clip-json 'layers' pointer is enough for a later importer.",
        },
        "frames": frames_json,
    }
    out_json = OUT / "idle_a.layers.json"
    out_json.write_text(json.dumps(doc, indent=2) + "\n")
    print("wrote", out_json, "max_active", max_err, "frames>1", n_bad_frames)
    # compact qa
    (OUT / "qa_zero.json").write_text(json.dumps(qa, indent=2) + "\n")


if __name__ == "__main__":
    main()
