#!/usr/bin/env python3
"""Composite idle_a layered art with test offsets; render a preview and a seam report.

Usage:
  preview_layers.py --manifest path/to/idle_a.layers.json --outdir dir
  preview_layers.py --manifest ... --frame 18 --dx-chest 0,-10 --dx-pigl -12,4 --dx-pigr 12,4 --png out.png
"""
from __future__ import annotations

import argparse
import json
import math
import subprocess
import sys
from pathlib import Path

import cv2
import numpy as np
from PIL import Image, ImageDraw, ImageFont

CELL = 400


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
    out[out[:, :, 3] == 0, :3] = 0
    return out


def over(top, bot):
    ta = np.clip(top[:, :, 3:4] / 255.0, 0, 1)
    out = np.empty_like(top)
    out[:, :, :3] = top[:, :, :3] + bot[:, :, :3] * (1.0 - ta)
    out[:, :, 3] = top[:, :, 3] + bot[:, :, 3] * (1.0 - ta[:, :, 0])
    return out


def slice_frame(sheet, i, w=CELL):
    return sheet[:, i * w : (i + 1) * w].copy()


def xform(img, dx, dy, angle_deg, pivot):
    if dx == 0 and dy == 0 and abs(angle_deg) < 1e-6:
        return img
    M = cv2.getRotationMatrix2D((float(pivot[0]), float(pivot[1])), float(angle_deg), 1.0)
    M[0, 2] += dx
    M[1, 2] += dy
    return cv2.warpAffine(
        img, M, (CELL, CELL), flags=cv2.INTER_LINEAR,
        borderMode=cv2.BORDER_CONSTANT, borderValue=(0, 0, 0, 0),
    )


def composite(base, chest, pl, pr, pivots, off):
    """off: dict id -> (dx, dy, angle_deg)."""
    c = xform(chest, *off.get("chest", (0, 0, 0)), pivots["chest"])
    l = xform(pl, *off.get("pigtail_left", (0, 0, 0)), pivots["pigtail_left"])
    r = xform(pr, *off.get("pigtail_right", (0, 0, 0)), pivots["pigtail_right"])
    out = to_premult(base)
    out = over(to_premult(c), out)
    out = over(to_premult(l), out)
    out = over(to_premult(r), out)
    return from_premult(out)


def checker(h, w, cell=16):
    img = np.zeros((h, w, 3), np.uint8)
    img[:, :] = (24, 24, 28)
    yy, xx = np.mgrid[0:h, 0:w]
    img[((xx // cell) + (yy // cell)) % 2 == 0] = (40, 40, 48)
    return img


def over_bg(fg, bg):
    a = fg[:, :, 3:4].astype(np.float32) / 255.0
    return np.clip(fg[:, :, :3] * a + bg * (1 - a), 0, 255).astype(np.uint8)


def load_bundle(manifest: Path):
    doc = json.loads(manifest.read_text())
    root = manifest.parent
    sheets = {}
    for lid, rel in doc["sheets"].items():
        p = root / rel
        sheets[lid] = np.asarray(Image.open(p).convert("RGBA"))
        assert sheets[lid].shape[0] == CELL
        assert sheets[lid].shape[1] == CELL * doc["frame_count"]
    return doc, sheets


def pivots_of(doc, i):
    p = doc["frames"][i]["pivots"]
    return {k: (float(v[0]), float(v[1])) for k, v in p.items()}


def seam_score(src, comp, bg_is_clear_thresh=8):
    """Pixels that were opaque in the source but went nearly clear in the composite
    (a hole opened onto the backdrop) and are not on the original silhouette edge
    facing already-clear outside.
    """
    src_a = src[:, :, 3]
    comp_a = comp[:, :, 3]
    hole = (src_a >= 40) & (comp_a < bg_is_clear_thresh)
    # ignore a 1px original outer silhouette (true outline can shift)
    from scipy import ndimage as nd
    outer = (src_a >= 40) & (nd.binary_dilation(src_a < 40, iterations=1))
    crack = hole & ~outer
    n = int(crack.sum())
    ys, xs = np.nonzero(crack)
    bbox = None
    if n:
        bbox = [int(xs.min()), int(ys.min()), int(xs.max()), int(ys.max())]
    return n, bbox, crack


def label(im, text, y=8):
    canvas = im.copy()
    if canvas.ndim == 2 or canvas.shape[2] == 3:
        canvas = np.dstack([canvas, np.full(canvas.shape[:2], 255, np.uint8)]) if canvas.ndim == 2 or canvas.shape[2] == 3 else canvas
    if canvas.shape[2] == 3:
        canvas = np.dstack([canvas, np.full(canvas.shape[:2], 255, np.uint8)])
    pil = Image.fromarray(canvas)
    d = ImageDraw.Draw(pil)
    d.rectangle([6, y - 2, 6 + 7 * len(text) + 8, y + 16], fill=(0, 0, 0, 180))
    d.text((10, y), text, fill=(255, 255, 255, 255))
    return np.asarray(pil)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--manifest", required=True)
    ap.add_argument("--source-sheet", default=None, help="original idle_a_sheet.png (for QA)")
    ap.add_argument("--outdir", required=True)
    ap.add_argument("--skip-video", action="store_true")
    args = ap.parse_args()
    man = Path(args.manifest)
    out = Path(args.outdir)
    out.mkdir(parents=True, exist_ok=True)
    doc, sheets = load_bundle(man)
    n = doc["frame_count"]
    fps = float(doc["fps"])
    src_path = Path(args.source_sheet) if args.source_sheet else man.parent / doc["source_sheet"]
    src_sheet = np.asarray(Image.open(src_path).convert("RGBA")) if src_path.exists() else None

    # zero-offset exactness (after PNG round-trip)
    zero = []
    for i in range(n):
        pv = pivots_of(doc, i)
        base, chest, pl, pr = [slice_frame(sheets[k], i) for k in ("base", "chest", "pigtail_left", "pigtail_right")]
        comp = composite(base, chest, pl, pr, pv, {})
        rec = {"i": i}
        if src_sheet is not None:
            src = slice_frame(src_sheet, i)
            su, cu = src.copy(), comp.copy()
            su[su[:, :, 3] == 0, :3] = 0
            diff = np.abs(cu.astype(int) - su.astype(int))
            vis = (su[:, :, 3] >= 8) | (cu[:, :, 3] >= 8)
            rec["max_vis8"] = int(diff[vis].max()) if vis.any() else 0
            rec["n_gt1_vis8"] = int(((diff.max(2) > 1) & vis).sum())
            rec["max_vis16"] = int(diff[(su[:, :, 3] >= 16) | (cu[:, :, 3] >= 16)].max()) if ((su[:, :, 3] >= 16) | (cu[:, :, 3] >= 16)).any() else 0
        zero.append(rec)
    (out / "qa_zero_png.json").write_text(json.dumps(zero, indent=2) + "\n")
    if src_sheet is not None:
        print("zero-offset PNG round-trip max vis>=8", max(r["max_vis8"] for r in zero),
              "max vis>=16", max(r["max_vis16"] for r in zero))

    # seam checks at max offsets, sampled frames
    samples = [0, 8, 18, 27, 36, 45, 54, 63, 71]
    cases = {
        "chest_up": {"chest": (0, -10, 0)},
        "chest_down": {"chest": (0, 10, 0)},
        "chest_left": {"chest": (-10, 0, 0)},
        "chest_right": {"chest": (10, 0, 0)},
        "pigs_out": {"pigtail_left": (-16, 4, 12), "pigtail_right": (16, 4, -12)},
        "pigs_in": {"pigtail_left": (10, -4, -10), "pigtail_right": (-10, -4, 10)},
        "combined": {"chest": (0, -10, 0), "pigtail_left": (-16, 6, 14), "pigtail_right": (16, 6, -14)},
    }
    seams = []
    for i in samples:
        pv = pivots_of(doc, i)
        layers = [slice_frame(sheets[k], i) for k in ("base", "chest", "pigtail_left", "pigtail_right")]
        src = slice_frame(src_sheet, i) if src_sheet is not None else layers[0]
        for name, off in cases.items():
            comp = composite(*layers, pv, off)
            cracks, bbox, mask = seam_score(src, comp)
            rec = {"frame": i, "case": name, "crack_px": cracks, "bbox": bbox}
            seams.append(rec)
            if i in (0, 18) and name in ("chest_up", "pigs_out", "combined"):
                bg = checker(CELL, CELL)
                vis = label(over_bg(comp, bg), f"f{i} {name} cracks={cracks}")
                Image.fromarray(vis).save(out / f"seam_f{i:02d}_{name}.png")
    (out / "qa_seams.json").write_text(json.dumps(seams, indent=2) + "\n")
    by = {}
    for r in seams:
        by.setdefault(r["case"], []).append(r["crack_px"])
    print("seam cracks (max over sampled frames):")
    for k, vs in by.items():
        print(f"  {k:12} max={max(vs):4} mean={sum(vs)/len(vs):.1f}")

    # contact sheet
    def tile(img, title, scale=1.0):
        if img.shape[2] == 4:
            img = over_bg(img, checker(*img.shape[:2]))
        if scale != 1:
            img = np.asarray(Image.fromarray(img).resize(
                (int(img.shape[1] * scale), int(img.shape[0] * scale)), Image.Resampling.NEAREST
            ))
        return label(img, title)[:, :, :3]

    rows = []
    for i in (0, 18):
        pv = pivots_of(doc, i)
        base, chest, pl, pr = [slice_frame(sheets[k], i) for k in ("base", "chest", "pigtail_left", "pigtail_right")]
        src = slice_frame(src_sheet, i) if src_sheet is not None else composite(base, chest, pl, pr, pv, {})
        zero_c = composite(base, chest, pl, pr, pv, {})
        cup = composite(base, chest, pl, pr, pv, {"chest": (0, -10, 0)})
        pigs = composite(base, chest, pl, pr, pv, {"pigtail_left": (-16, 6, 14), "pigtail_right": (16, 6, -14)})
        both = composite(base, chest, pl, pr, pv, {"chest": (0, -10, 0), "pigtail_left": (-16, 6, 14), "pigtail_right": (16, 6, -14)})
        sc = 0.7
        row = np.concatenate([
            tile(src, f"f{i} source", sc),
            tile(base, f"f{i} base (fill)", sc),
            tile(chest, f"f{i} chest (top on)", sc),
            tile(pl, f"f{i} pigtail L", sc),
            tile(pr, f"f{i} pigtail R", sc),
            tile(zero_c, f"f{i} zero offset", sc),
            tile(cup, f"f{i} chest -10y", sc),
            tile(pigs, f"f{i} pigs ±16", sc),
            tile(both, f"f{i} combined", sc),
        ], axis=1)
        rows.append(row)
    contact = np.concatenate(rows, axis=0)
    # header bar
    header = np.zeros((36, contact.shape[1], 3), np.uint8)
    header[:, :] = (16, 16, 20)
    hp = Image.fromarray(header)
    ImageDraw.Draw(hp).text((12, 10), "idle_a layered-art pilot  ·  base / covered chest / pigtails  ·  zero offset matches source  ·  test offsets 10px chest, 16px pigtails", fill=(240, 240, 240))
    contact = np.concatenate([np.asarray(hp), contact], axis=0)
    Image.fromarray(contact).save(out / "contact_layers1.png", optimize=True)
    print("contact", out / "contact_layers1.png", contact.shape)

    if args.skip_video:
        return

    # preview animation over the 72-frame clip
    frames = []
    for i in range(n):
        t = i / max(n - 1, 1)  # 0..1
        # program: 0-0.22 zero, 0.22-0.45 chest, 0.45-0.70 pigs, 0.70-1 combined
        def ramp(a, b, x):
            if x < a or x > b:
                return 0.0
            mid = 0.5 * (a + b)
            return 0.5 - 0.5 * math.cos(2 * math.pi * (x - a) / (b - a)) if True else 1.0

        # use sine so it eases; amplitude tapers with a window
        def win(a, b, x):
            if x <= a or x >= b:
                return 0.0
            u = (x - a) / (b - a)
            return math.sin(math.pi * u)  # 0 at ends, 1 at mid

        w_c = win(0.18, 0.48, t)
        w_p = win(0.42, 0.72, t)
        w_b = win(0.66, 1.00, t)
        phase = 2 * math.pi * (i / 11.0)  # ~1 Hz-ish vs 11 fps
        chest_off = (0.0, -10.0 * math.sin(phase) * max(w_c, w_b), 0.0)
        # pigtails: opposite rotation + lateral, bounce ramps with window
        amp = 16.0 * max(w_p, w_b)
        ang = 14.0 * math.sin(phase + 0.4) * max(w_p, w_b)
        pigl = (-amp * math.sin(phase), 5.0 * math.sin(phase * 2) * max(w_p, w_b), ang)
        pigr = (amp * math.sin(phase), 5.0 * math.sin(phase * 2 + 0.3) * max(w_p, w_b), -ang)
        pv = pivots_of(doc, i)
        layers = [slice_frame(sheets[k], i) for k in ("base", "chest", "pigtail_left", "pigtail_right")]
        comp = composite(*layers, pv, {"chest": chest_off, "pigtail_left": pigl, "pigtail_right": pigr})
        if t < 0.18:
            tag = "zero extra offset (baked idle + baked chest bounce only)"
        elif t < 0.45:
            tag = "chest layer bounce  up to 10 px"
        elif t < 0.68:
            tag = "pigtails independent sway  up to 16 px"
        else:
            tag = "combined chest + pigtails"
        vis = label(over_bg(comp, checker(CELL, CELL)), f"idle_a  f{i:02d}/{n-1}  {tag}")
        frames.append(vis[:, :, :3])

    # write png sequence and mp4 + gif
    seq = out / "seq"
    seq.mkdir(exist_ok=True)
    for i, fr in enumerate(frames):
        Image.fromarray(fr).save(seq / f"f{i:03d}.png")
    mp4 = out / "layers1_preview.mp4"
    gif = out / "layers1_preview.gif"
    # fps 11
    cmd = [
        "ffmpeg", "-y", "-framerate", str(fps), "-i", str(seq / "f%03d.png"),
        "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "16", "-movflags", "+faststart",
        str(mp4),
    ]
    subprocess.check_call(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT)
    # smaller gif (every frame, paletted)
    cmdg = [
        "ffmpeg", "-y", "-framerate", str(fps), "-i", str(seq / "f%03d.png"),
        "-vf", "split[s0][s1];[s0]palettegen=max_colors=128[p];[s1][p]paletteuse=dither=bayer",
        "-loop", "0", str(gif),
    ]
    subprocess.check_call(cmdg, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT)
    print("video", mp4, mp4.stat().st_size, "gif", gif, gif.stat().st_size)


if __name__ == "__main__":
    main()
