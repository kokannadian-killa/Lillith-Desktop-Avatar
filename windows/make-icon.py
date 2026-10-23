#!/usr/bin/env python3
"""Makes windows/lillith.ico from the first idle frame (read-only use of
assets/generated; the art itself is never modified). Needs Pillow.

Usage: python3 windows/make-icon.py [OUT.ico]
"""
import sys
import tomllib
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
GEN = ROOT / "assets" / "generated"
OUT = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent / "lillith.ico"

clip = tomllib.loads((GEN / "manifest.toml").read_text())["clips"]["idle"]
f = clip["frames"][0]
img = Image.open(GEN / clip["page"]).convert("RGBA")
img = img.crop((f["x"], f["y"], f["x"] + f["w"], f["y"] + f["h"]))
box = img.getbbox() or (0, 0, img.width, img.height)
img = img.crop(box)
# Head and shoulders read best at 16-48 px: a top-centred square.
side = int(img.width * 0.72)
left = (img.width - side) // 2
sq = Image.new("RGBA", (side, side), (0, 0, 0, 0))
sq.paste(img.crop((left, 0, left + side, min(side, img.height))), (0, 0))
sq = sq.resize((256, 256), Image.LANCZOS)
sq.save(OUT, sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
print(f"wrote {OUT}")
