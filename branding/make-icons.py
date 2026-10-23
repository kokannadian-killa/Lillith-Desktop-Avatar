#!/usr/bin/env python3
"""Lillith's icon set: every icon, for every platform.

branding/icons.toml lists the icons (id, display name, files); the default
is `phantomcore-red`. Each icon lives in branding/icons/<id>/:

    icon-1024.png                 master (transparent outside the rounded square)
    lillith-<N>.png               N = 16 24 32 48 64 128 256 (Linux hicolor, picker thumbnails)
    <id>.ico                      16-256 (Windows shortcuts)

Platform outputs of the set:

    windows/lillith.ico                        the default icon, embedded in both .exe files
    android/app/res/mipmap-xxxhdpi/ic_launcher_foreground.png   default adaptive foreground
    android/app/res/mipmap-xxxhdpi/ic_icon_<id>_fg.png          one per icon (activity-alias)
    android/app/res/mipmap-anydpi-v26/ic_launcher{,_round}.xml, ic_icon_<id>.xml
    android/app/res/values/ic_launcher_colors.xml
    android/app/AndroidManifest.xml            activity-alias block between the ICON-ALIASES markers
    android/app/src/com/kokannadian/lillith/IconSet.java   ids + names for the picker

minSdk is 26, so adaptive icons (anydpi-v26) are all Android ever reads; one
xxxhdpi foreground per icon is enough (the launcher scales it down).

Usage (needs Pillow; numpy + scipy for --from-sources / --from-video;
pngquant is used when installed to keep the repo small):

    python3 branding/make-icons.py                       # regenerate everything from the masters
    python3 branding/make-icons.py --from-sources DIR    # re-cut the masters from DIR/NN-<id>.jpg
    python3 branding/make-icons.py --from-video          # re-cut the Classic master from the idle clip
    python3 branding/make-icons.py --sheet PNG           # also write a contact sheet (256 + 32 px)
    python3 branding/make-icons.py --check               # fail if any generated file is missing

--from-sources takes Ko's 1024x1024 JPGs (a rounded-square icon drawn on
black): it finds the rounded square (corner flood fill from black), crops it,
and makes everything outside it transparent with a clean anti-aliased
rounded-rect mask (detected corner radius, at least 18%).
"""
import argparse
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
ICONS = HERE / "icons"
TOML = HERE / "icons.toml"
SIZES = [16, 24, 32, 48, 64, 128, 256]
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]
WIN_ICO = ROOT / "windows" / "lillith.ico"
ANDROID = ROOT / "android" / "app"
RES = ANDROID / "res"
MANIFEST = ANDROID / "AndroidManifest.xml"
ICONSET_JAVA = ANDROID / "src" / "com" / "kokannadian" / "lillith" / "IconSet.java"
FG_PX = 432  # 108 dp at xxxhdpi
FG_ART = 80 / 108  # art size inside the adaptive canvas (the launcher masks ~66 dp)
BG_HEX = "#FF000000"  # the art is drawn on black

DEFAULT = "phantomcore-red"
# (id, display name, source stem in --from-sources; None = not from a JPG)
SET = [
    ("neon-portrait", "Neon Portrait", "01-neon-portrait"),
    ("wink-chibi", "Wink Chibi", "02-wink-chibi"),
    ("phantomcore-red", "Phantomcore Red", "03-phantomcore-red"),
    ("demonos-green-chibi", "DemonOS Green Chibi", "04-demonos-green-chibi"),
    ("neon-sign", "Neon Sign", "05-neon-sign"),
    ("demonos-emblem", "DemonOS Emblem", "06-demonos-emblem"),
    ("hooded-phantom", "Hooded Phantom", "07-hooded-phantom"),
    ("green-hologram", "Green Hologram", "08-green-hologram"),
    ("chibi-perched", "Chibi Perched", "09-chibi-perched"),
    ("kiss-skullframe", "Kiss Skullframe", "10-kiss-skullframe"),
    ("winged-sigil", "Winged Sigil", "11-winged-sigil"),
    ("pixel-chibi", "Pixel Chibi", "12-pixel-chibi"),
    ("glitch", "Glitch", "13-glitch"),
    ("chibi-terminal", "Chibi Terminal", "14-chibi-terminal"),
    ("classic", "Classic", None),
]

# Classic: frame 0 of the canonical idle clip (1280x720 on white).
VIDEO = ROOT / "assets" / "source" / "video" / "idle" / "idle_v2_a.mp4"
CROP = (646, 4, 212)  # centre x, top y, side


def master(icon_id: str) -> Path:
    return ICONS / icon_id / "icon-1024.png"


def java_id(icon_id: str) -> str:
    return icon_id.replace("-", "_")


# --------------------------------------------------------------------------
# masters
# --------------------------------------------------------------------------


def detect_rounded_square(rgb):
    """(left, top, right, bottom) insets and corner radius (px) of the
    rounded square in a black-backed 1024 image."""
    import numpy as np
    from scipy import ndimage

    a = ndimage.uniform_filter(rgb.max(axis=2).astype(float), 3)
    lab, _ = ndimage.label(a < 10)
    n = a.shape[0]
    corners = {lab[0, 0], lab[0, -1], lab[-1, 0], lab[-1, -1]} - {0}
    out = np.isin(lab, list(corners))
    band = slice(n * 3 // 8, n * 5 // 8)

    def inset(m):
        firsts = [np.argmax(~row) if (~row).any() else n for row in m]
        return int(np.median(firsts))

    ins = (inset(out[band, :]), inset(out[:, band].T),
           inset(out[band, ::-1]), inset(out[::-1, band].T))
    L, T, R, B = ins
    radii = []
    for fy, fx, iy, ix in [(0, 0, T, L), (0, 1, T, R), (1, 0, B, L), (1, 1, B, R)]:
        o = out[::-1 if fy else 1, ::-1 if fx else 1]
        k = 0
        while k < n // 2 and o[k, k]:
            k += 1
        radii.append((k - min(iy, ix)) / (1 - 0.70711))
    return ins, float(np.median(radii))


def rounded_mask(side: int, radius: float, inset: float = 2.0) -> Image.Image:
    """Anti-aliased rounded-rect alpha (4x supersampled, ~1.5 px soft edge)."""
    ss = 4
    m = Image.new("L", (side * ss, side * ss), 0)
    i = inset * ss
    ImageDraw.Draw(m).rounded_rectangle(
        (i, i, side * ss - 1 - i, side * ss - 1 - i), radius=radius * ss, fill=255
    )
    return m.resize((side, side), Image.LANCZOS).filter(ImageFilter.GaussianBlur(0.6))


def master_from_jpg(src: Path, dst: Path) -> str:
    import numpy as np

    im = Image.open(src).convert("RGB")
    if im.size != (1024, 1024):
        im = im.resize((1024, 1024), Image.LANCZOS)
    (L, T, R, B), r = detect_rounded_square(np.asarray(im))
    box = (L, T, 1024 - R, 1024 - B)
    side = (box[2] - box[0] + box[3] - box[1]) / 2
    crop = im.crop(box).resize((1024, 1024), Image.LANCZOS)
    r = r * 1024 / side
    # Full-bleed art (no black corners found) gets the standard 18 %;
    # otherwise a hair more than detected so no black arc is left.
    radius = 0.18 * 1024 if r < 0.12 * 1024 else min(max(r * 1.02 + 4, 0.18 * 1024), 0.27 * 1024)
    out = crop.convert("RGBA")
    out.putalpha(rounded_mask(1024, radius))
    dst.parent.mkdir(parents=True, exist_ok=True)
    out.save(dst)
    return f"box {box} radius {radius / 1024:.1%}"


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
    master("classic").parent.mkdir(parents=True, exist_ok=True)
    sq.resize((1024, 1024), Image.LANCZOS).save(master("classic"))
    print("wrote", master("classic").relative_to(ROOT))


# --------------------------------------------------------------------------
# renders
# --------------------------------------------------------------------------


def render(icon_id: str, m: Image.Image, px: int) -> Image.Image:
    src = m
    if icon_id == "classic" and px <= 32:
        # tighter head crop so the cut-out character still reads at 16 px
        side = int(m.width * 0.74)
        left = (m.width - side) // 2
        src = m.crop((left, 0, left + side, side))
    out = src.resize((px, px), Image.LANCZOS)
    if px <= 48:
        out = out.filter(ImageFilter.UnsharpMask(radius=0.6, percent=60, threshold=1))
    return out


def foreground(m: Image.Image) -> Image.Image:
    """Adaptive-icon foreground: 108 dp canvas, art centred at 80 dp."""
    out = Image.new("RGBA", (FG_PX, FG_PX), (0, 0, 0, 0))
    inner = round(FG_PX * FG_ART)
    out.alpha_composite(m.resize((inner, inner), Image.LANCZOS), ((FG_PX - inner) // 2,) * 2)
    return out


def adaptive(fg: str, bg: str) -> str:
    return f"""<?xml version="1.0" encoding="utf-8"?>
<!-- Generated by branding/make-icons.py: do not edit by hand. -->
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@color/{bg}" />
    <foreground android:drawable="@mipmap/{fg}" />
</adaptive-icon>
"""


def save_png(img: Image.Image, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    img.save(path, optimize=True)


def squeeze(paths: list[Path]) -> None:
    """pngquant (lossy palette, dithered) when available: ~3-4x smaller."""
    exe = shutil.which("pngquant")
    if not exe:
        print("note: pngquant not found; PNGs are only zlib-optimised")
        return
    for p in paths:
        w = Image.open(p).width
        # tiny sizes stay lossless: there the palette costs detail, not bytes
        if w < 32:
            continue
        # busy neon art needs a lower floor at 1024 to fit 256 colours
        q = "50-92" if w >= 1024 else "65-95"
        subprocess.run(
            [exe, f"--quality={q}", "--speed=1", "--strip", "--skip-if-larger",
             "--force", "--ext", ".png", str(p)],
            check=False,
        )


def write_ico(pngs: list[Path], dst: Path) -> None:
    """.ico with PNG-compressed entries (Windows Vista+), from the already
    squeezed size PNGs, so it costs about what the PNGs cost."""
    import struct

    import io

    blobs, dims = [], []
    for p in pngs:
        # Entries are stored as 32-bit RGBA PNGs (some Windows builds reject
        # palette PNGs inside an .ico); the colours are the squeezed ones.
        im = Image.open(p).convert("RGBA")
        buf = io.BytesIO()
        im.save(buf, "PNG", optimize=True)
        blobs.append(buf.getvalue())
        dims.append(im.size)
    out = struct.pack("<HHH", 0, 1, len(blobs))
    off = 6 + 16 * len(blobs)
    for (w, h), b in zip(dims, blobs):
        out += struct.pack("<BBBBHHII", w % 256, h % 256, 0, 0, 1, 32, len(b), off)
        off += len(b)
    dst.write_bytes(out + b"".join(blobs))


def write_toml() -> None:
    lines = [
        "# Lillith's icon set. Generated by branding/make-icons.py (edit SET there).",
        "# Paths are relative to this file. `default` is used when config.toml has",
        "# no `icon` key or names an unknown id.",
        "",
        f'default = "{DEFAULT}"',
        f"sizes = [{', '.join(map(str, SIZES))}]",
        "",
    ]
    for icon_id, name, _ in SET:
        lines += [
            "[[icon]]",
            f'id = "{icon_id}"',
            f'name = "{name}"',
            f'master = "icons/{icon_id}/icon-1024.png"',
            f'png = "icons/{icon_id}/lillith-{{size}}.png"',
            f'ico = "icons/{icon_id}/{icon_id}.ico"',
            f'android = "@mipmap/ic_icon_{java_id(icon_id)}"',
            "",
        ]
    TOML.write_text("\n".join(lines))


ALIAS_BEGIN = "<!-- ICON-ALIASES:BEGIN (generated by branding/make-icons.py) -->"
ALIAS_END = "<!-- ICON-ALIASES:END -->"


def write_android(fgs: dict[str, Image.Image]) -> list[Path]:
    pngs = []
    d = RES / "mipmap-xxxhdpi"
    anyd = RES / "mipmap-anydpi-v26"
    anyd.mkdir(parents=True, exist_ok=True)
    for icon_id, fg in fgs.items():
        j = java_id(icon_id)
        p = d / f"ic_icon_{j}_fg.png"
        save_png(fg, p)
        pngs.append(p)
        (anyd / f"ic_icon_{j}.xml").write_text(adaptive(f"ic_icon_{j}_fg", "ic_icon_background"))
    p = d / "ic_launcher_foreground.png"
    save_png(fgs[DEFAULT], p)
    pngs.append(p)
    for n in ("ic_launcher", "ic_launcher_round"):
        (anyd / f"{n}.xml").write_text(adaptive("ic_launcher_foreground", "ic_launcher_background"))
    (RES / "values" / "ic_launcher_colors.xml").write_text(
        '<?xml version="1.0" encoding="utf-8"?>\n'
        "<!-- Generated by branding/make-icons.py: do not edit by hand. -->\n"
        "<resources>\n"
        f'    <color name="ic_launcher_background">{BG_HEX}</color>\n'
        f'    <color name="ic_icon_background">{BG_HEX}</color>\n'
        "</resources>\n"
    )
    # activity-alias per icon (the launcher entry); the default one enabled
    aliases = []
    for icon_id, name, _ in SET:
        j = java_id(icon_id)
        on = "true" if icon_id == DEFAULT else "false"
        aliases.append(
            f"""        <activity-alias
            android:name=".Icon_{j}"
            android:targetActivity=".SetupActivity"
            android:label="Lillith"
            android:icon="@mipmap/ic_icon_{j}"
            android:roundIcon="@mipmap/ic_icon_{j}"
            android:enabled="{on}"
            android:exported="true">
            <intent-filter>
                <action android:name="android.intent.action.MAIN" />
                <category android:name="android.intent.category.LAUNCHER" />
            </intent-filter>
        </activity-alias>"""
        )
    block = f"        {ALIAS_BEGIN}\n" + "\n".join(aliases) + f"\n        {ALIAS_END}"
    text = MANIFEST.read_text()
    pat = re.compile(r"[ \t]*" + re.escape(ALIAS_BEGIN) + r".*?" + re.escape(ALIAS_END), re.S)
    if not pat.search(text):
        sys.exit(f"{MANIFEST}: missing the {ALIAS_BEGIN} / {ALIAS_END} markers")
    MANIFEST.write_text(pat.sub(lambda _: block, text))
    ids = ",\n        ".join(f'"{i}"' for i, _, _ in SET)
    names = ",\n        ".join(f'"{n}"' for _, n, _ in SET)
    ICONSET_JAVA.write_text(f"""// Generated by branding/make-icons.py: do not edit by hand.
package com.kokannadian.lillith;

/** Lillith's launcher icons: one activity-alias {{@code .Icon_<id>}} each. */
final class IconSet {{
    private IconSet() {{}}

    static final String DEFAULT = "{DEFAULT}";

    static final String[] IDS = {{
        {ids}
    }};

    static final String[] NAMES = {{
        {names}
    }};

    /** activity-alias class name for an icon id. */
    static String alias(String pkg, String id) {{
        return pkg + ".Icon_" + id.replace('-', '_');
    }}

    /** mipmap resource name of an icon (adaptive xml). */
    static String mipmap(String id) {{
        return "ic_icon_" + id.replace('-', '_');
    }}
}}
""")
    return pngs


def outputs() -> list[Path]:
    out = [TOML, WIN_ICO, MANIFEST, ICONSET_JAVA, RES / "values" / "ic_launcher_colors.xml",
           RES / "mipmap-xxxhdpi" / "ic_launcher_foreground.png",
           RES / "mipmap-anydpi-v26" / "ic_launcher.xml",
           RES / "mipmap-anydpi-v26" / "ic_launcher_round.xml"]
    for icon_id, _, _ in SET:
        d = ICONS / icon_id
        out += [d / "icon-1024.png", d / f"{icon_id}.ico"]
        out += [d / f"lillith-{s}.png" for s in SIZES]
        out += [RES / "mipmap-xxxhdpi" / f"ic_icon_{java_id(icon_id)}_fg.png",
                RES / "mipmap-anydpi-v26" / f"ic_icon_{java_id(icon_id)}.xml"]
    return out


def generate(squeeze_masters: bool) -> None:
    pngs = []
    fgs = {}
    for icon_id, _, _ in SET:
        mp = master(icon_id)
        m = Image.open(mp).convert("RGBA")
        if m.size != (1024, 1024):
            sys.exit(f"{mp} must be 1024x1024, is {m.size}")
        d = ICONS / icon_id
        for s in SIZES:
            save_png(render(icon_id, m, s), d / f"lillith-{s}.png")
            pngs.append(d / f"lillith-{s}.png")
        fgs[icon_id] = foreground(m)
        if squeeze_masters:
            pngs.append(mp)
    pngs += write_android(fgs)
    write_toml()
    squeeze(pngs)
    for icon_id, _, _ in SET:
        d = ICONS / icon_id
        write_ico([d / f"lillith-{s}.png" for s in ICO_SIZES], d / f"{icon_id}.ico")
    shutil.copyfile(ICONS / DEFAULT / f"{DEFAULT}.ico", WIN_ICO)
    print(f"wrote {len(outputs())} files for {len(SET)} icons (default {DEFAULT})")


def sheet(path: Path) -> None:
    """Contact sheet: each icon at 256 and 32 px (x1 and x4), dark + light."""
    cols, cw, ch = 5, 300, 400
    rows = (len(SET) + cols - 1) // cols
    img = Image.new("RGBA", (cols * cw, rows * ch), (24, 18, 28, 255))
    dr = ImageDraw.Draw(img)
    for i, (icon_id, name, _) in enumerate(SET):
        x, y = (i % cols) * cw, (i // cols) * ch
        if i % 2:
            dr.rectangle((x, y, x + cw - 1, y + ch - 1), fill=(32, 24, 38, 255))
        d = ICONS / icon_id
        big = Image.open(d / "lillith-256.png").convert("RGBA")
        img.alpha_composite(big, (x + 22, y + 14))
        small = Image.open(d / "lillith-32.png").convert("RGBA")
        img.alpha_composite(small, (x + 22, y + 290))
        img.alpha_composite(small.resize((128, 128), Image.NEAREST).crop((0, 0, 128, 64)),
                            (x + 70, y + 290))
        light = Image.new("RGBA", (40, 40), (236, 236, 236, 255))
        light.alpha_composite(small, (4, 4))
        img.alpha_composite(light, (x + 18, y + 330))
        label = name + (" (default)" if icon_id == DEFAULT else "")
        dr.text((x + 70, y + 362), label, fill=(240, 220, 240, 255))
        dr.text((x + 70, y + 376), icon_id, fill=(160, 140, 170, 255))
    path.parent.mkdir(parents=True, exist_ok=True)
    img.convert("RGB").save(path)
    print("wrote", path)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--from-sources", type=Path, metavar="DIR",
                    help="re-cut the masters from DIR/NN-<id>.jpg first")
    ap.add_argument("--from-video", action="store_true", help="re-cut the Classic master first")
    ap.add_argument("--sheet", type=Path, metavar="PNG", help="also write a contact sheet")
    ap.add_argument("--check", action="store_true", help="only verify the outputs exist")
    a = ap.parse_args()
    if a.check:
        missing = [p for p in outputs() if not p.is_file()]
        for p in missing:
            print(f"missing: {p.relative_to(ROOT)}", file=sys.stderr)
        return 1 if missing else 0
    if a.from_sources:
        for icon_id, _, stem in SET:
            if stem is None:
                continue
            src = a.from_sources / f"{stem}.jpg"
            if not src.is_file():
                sys.exit(f"missing source {src}")
            print(f"{icon_id}: {master_from_jpg(src, master(icon_id))}")
    if a.from_video:
        master_from_video()
    generate(squeeze_masters=bool(a.from_sources or a.from_video))
    if a.sheet:
        sheet(a.sheet)
    return 0


if __name__ == "__main__":
    sys.exit(main())
