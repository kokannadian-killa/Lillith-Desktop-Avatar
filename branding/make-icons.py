#!/usr/bin/env python3
"""Lillith's one icon, for every platform.

The master is branding/icon-1024.png (transparent RGBA, her head and upper
body). Everything else is generated from it:

    branding/png/lillith-<N>.png         N = 16 24 32 48 64 128 256 512
    windows/lillith.ico                  16-256, embedded in both .exe files
    android/app/res/mipmap-*/ic_launcher.png        legacy launcher icon
    android/app/res/mipmap-*/ic_launcher_round.png  round launcher icon
    android/app/res/mipmap-*/ic_launcher_foreground.png  adaptive foreground
    android/app/res/mipmap-anydpi-v26/ic_launcher{,_round}.xml + values/ic_launcher_colors.xml

Small sizes (<= 32 px) use a tighter head crop, so she still reads at 16 px.

Usage (from anywhere; needs Pillow):
    python3 branding/make-icons.py                # regenerate everything from the master
    python3 branding/make-icons.py --from-video   # re-cut the master first (ffmpeg, numpy, scipy)
    python3 branding/make-icons.py --check        # fail if any generated file is missing

--from-video takes frame 0 of assets/source/video/idle/idle_v2_a.mp4 (the
canonical v2 idle, 1280x720 on white), keys out the white background and crops
a square of her head and upper body. The art files are only read, never changed.
"""
import argparse
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
MASTER = HERE / "icon-1024.png"
PNG_DIR = HERE / "png"
SIZES = [16, 24, 32, 48, 64, 128, 256, 512]
ICO = ROOT / "windows" / "lillith.ico"
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]
RES = ROOT / "android" / "app" / "res"
# density: (launcher px, adaptive layer px = 108 dp)
DENSITIES = {
    "mdpi": (48, 108),
    "hdpi": (72, 162),
    "xhdpi": (96, 216),
    "xxhdpi": (144, 324),
    "xxxhdpi": (192, 432),
}
BADGE = (42, 22, 51, 255)  # deep plum, round / adaptive backdrop
BADGE_HEX = "#FF2A1633"

VIDEO = ROOT / "assets" / "source" / "video" / "idle" / "idle_v2_a.mp4"
# Square crop of the 1280x720 frame: centre x, top y, side (head + upper body).
CROP = (646, 4, 212)


def master_from_video() -> None:
    import numpy as np
    from scipy import ndimage

    with tempfile.TemporaryDirectory() as td:
        frame = Path(td) / "f0.png"
        subprocess.run(
            ["ffmpeg", "-v", "error", "-y", "-i", str(VIDEO), "-frames:v", "1", str(frame)],
            check=True,
        )
        im = np.array(Image.open(frame).convert("RGB")).astype(float)
    mn = im.min(axis=2)
    sat = im.max(axis=2) - mn
    white = (mn > 228) & (sat < 20)
    lab, n = ndimage.label(white)
    sizes = ndimage.sum(white, lab, range(1, n + 1))
    border = set(np.unique(np.concatenate([lab[0], lab[-1], lab[:, 0], lab[:, -1]]))) - {0}
    # Background = white touching the border, plus enclosed white pockets
    # (between twin-tails and neck); eye highlights are tiny and stay.
    keep = [i + 1 for i, s in enumerate(sizes) if s > 40 or (i + 1) in border]
    bg = np.isin(lab, keep)
    fg = ~bg
    edge = ndimage.binary_dilation(bg, iterations=2) & fg
    a = np.where(edge, np.clip((255 - mn) / 55, 0, 1), fg.astype(float))
    a = ndimage.gaussian_filter(a, 0.5) * fg
    af = np.clip(a, 1e-3, 1)[..., None]
    rgb = np.where(edge[..., None], np.clip((im - (1 - af) * 255) / af, 0, 255), im)
    cut = Image.fromarray(np.dstack([rgb, a * 255]).astype("uint8"), "RGBA")
    cx, top, side = CROP
    sq = cut.crop((cx - side // 2, top, cx + side // 2, top + side))
    sq = sq.resize((1024, 1024), Image.LANCZOS)
    sq.save(MASTER, optimize=True)
    print(f"wrote {MASTER.relative_to(ROOT)}")


def head_crop(img: Image.Image) -> Image.Image:
    """Tighter square for tiny sizes: horns to chin, twin-tails trimmed."""
    s = img.width
    side = int(s * 0.74)
    left = (s - side) // 2
    return img.crop((left, 0, left + side, side))


def render(master: Image.Image, px: int) -> Image.Image:
    src = head_crop(master) if px <= 32 else master
    out = src.resize((px, px), Image.LANCZOS)
    if px <= 48:
        out = out.filter(ImageFilter.UnsharpMask(radius=0.6, percent=60, threshold=1))
    return out


def badge(master: Image.Image, px: int, inset: float) -> Image.Image:
    """Her icon on a plum circle (round launcher icon)."""
    big = px * 4
    bg = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    ImageDraw.Draw(bg).ellipse((0, 0, big - 1, big - 1), fill=BADGE)
    inner = int(big * inset)
    art = master.resize((inner, inner), Image.LANCZOS)
    mask = Image.new("L", (big, big), 0)
    ImageDraw.Draw(mask).ellipse((0, 0, big - 1, big - 1), fill=255)
    layer = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    layer.alpha_composite(art, ((big - inner) // 2, big - inner))
    bg.paste(Image.alpha_composite(bg, layer), (0, 0), mask)
    return bg.resize((px, px), Image.LANCZOS)


def foreground(master: Image.Image, px: int) -> Image.Image:
    """Adaptive-icon foreground: 108 dp canvas, art in the 72 dp middle."""
    out = Image.new("RGBA", (px, px), (0, 0, 0, 0))
    inner = round(px * 72 / 108)
    art = master.resize((inner, inner), Image.LANCZOS)
    out.alpha_composite(art, ((px - inner) // 2, (px - inner) // 2 + px // 20))
    return out


ADAPTIVE = """<?xml version="1.0" encoding="utf-8"?>
<!-- Generated by branding/make-icons.py: do not edit by hand. -->
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@color/ic_launcher_background" />
    <foreground android:drawable="@mipmap/ic_launcher_foreground" />
</adaptive-icon>
"""

COLORS = f"""<?xml version="1.0" encoding="utf-8"?>
<!-- Generated by branding/make-icons.py: do not edit by hand. -->
<resources>
    <color name="ic_launcher_background">{BADGE_HEX}</color>
</resources>
"""


def outputs() -> list[Path]:
    out = [PNG_DIR / f"lillith-{s}.png" for s in SIZES] + [ICO]
    for d in DENSITIES:
        for n in ("ic_launcher", "ic_launcher_round", "ic_launcher_foreground"):
            out.append(RES / f"mipmap-{d}" / f"{n}.png")
    out += [
        RES / "mipmap-anydpi-v26" / "ic_launcher.xml",
        RES / "mipmap-anydpi-v26" / "ic_launcher_round.xml",
        RES / "values" / "ic_launcher_colors.xml",
    ]
    return out


def generate() -> None:
    master = Image.open(MASTER).convert("RGBA")
    if master.size != (1024, 1024):
        sys.exit(f"{MASTER} must be 1024x1024, is {master.size}")
    PNG_DIR.mkdir(parents=True, exist_ok=True)
    for s in SIZES:
        render(master, s).save(PNG_DIR / f"lillith-{s}.png", optimize=True)
    # .ico: Pillow writes one PNG/BMP entry per size from the largest frame,
    # so hand it the hand-tuned small renders via append_images.
    frames = [render(master, s) for s in ICO_SIZES]
    frames[-1].save(ICO, sizes=[(s, s) for s in ICO_SIZES], append_images=frames[:-1])
    for d, (px, fg_px) in DENSITIES.items():
        dd = RES / f"mipmap-{d}"
        dd.mkdir(parents=True, exist_ok=True)
        render(master, px).save(dd / "ic_launcher.png", optimize=True)
        badge(master, px, 0.92).save(dd / "ic_launcher_round.png", optimize=True)
        foreground(master, fg_px).save(dd / "ic_launcher_foreground.png", optimize=True)
    any_dir = RES / "mipmap-anydpi-v26"
    any_dir.mkdir(parents=True, exist_ok=True)
    (any_dir / "ic_launcher.xml").write_text(ADAPTIVE)
    (any_dir / "ic_launcher_round.xml").write_text(ADAPTIVE)
    (RES / "values" / "ic_launcher_colors.xml").write_text(COLORS)
    print(f"wrote {len(outputs())} files from {MASTER.relative_to(ROOT)}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--from-video", action="store_true", help="re-cut the master first")
    ap.add_argument("--check", action="store_true", help="only verify the outputs exist")
    a = ap.parse_args()
    if a.check:
        missing = [p for p in outputs() if not p.is_file()]
        for p in missing:
            print(f"missing: {p.relative_to(ROOT)}", file=sys.stderr)
        return 1 if missing else 0
    if a.from_video:
        master_from_video()
    generate()
    return 0


if __name__ == "__main__":
    sys.exit(main())
