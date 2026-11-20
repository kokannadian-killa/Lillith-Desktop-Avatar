#!/usr/bin/env python3
"""Generate the DemonOS / PhantomCore boot + desktop art for the ISO.

Everything is drawn from what is already in the repo (read-only):
  branding/icons/demonos-emblem/icon-1024.png   (wallpaper, GRUB, SDDM)
  branding/icons/phantomcore-red/icon-1024.png  (Plymouth, syslinux)
  crates/lillith-theme/fonts/*.ttf              (demon lettering + mono)
Palette (lillith-theme): void #0a0a0f, crimson #ff3e6c, cyan #00f0ff, purple #b829f7.

Usage: make-theme-assets.py REPO_ROOT OUT_DIR
Writes OUT_DIR/{wallpaper,grub,syslinux,plymouth,sddm}/... (nothing in the repo changes).
Needs Pillow (python-pillow).
"""
import sys
from pathlib import Path
from PIL import Image, ImageDraw, ImageFilter, ImageFont

VOID = (10, 10, 15)
CRIMSON = (255, 62, 108)
CYAN = (0, 240, 255)
PURPLE = (184, 41, 247)

repo = Path(sys.argv[1]).resolve()
out = Path(sys.argv[2]).resolve()
FONTS = repo / "crates/lillith-theme/fonts"
EMBLEM = repo / "branding/icons/demonos-emblem/icon-1024.png"
PHANTOM = repo / "branding/icons/phantomcore-red/icon-1024.png"


def font(name, size):
    return ImageFont.truetype(str(FONTS / name), size)


def rounded(img, radius):
    mask = Image.new("L", img.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, *img.size), radius, fill=255)
    img = img.convert("RGBA")
    img.putalpha(mask)
    return img


def glow(base, xy, r, color, alpha=110):
    layer = Image.new("RGBA", base.size, (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    x, y = xy
    d.ellipse((x - r, y - r, x + r, y + r), fill=(*color, alpha))
    layer = layer.filter(ImageFilter.GaussianBlur(r // 2))
    base.alpha_composite(layer)


def grid(base, step, color, alpha=28):
    layer = Image.new("RGBA", base.size, (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    w, h = base.size
    for x in range(0, w, step):
        d.line((x, 0, x, h), fill=(*color, alpha))
    for y in range(0, h, step):
        d.line((0, y, w, y), fill=(*color, alpha))
    base.alpha_composite(layer)


def scanlines(base, alpha=18):
    layer = Image.new("RGBA", base.size, (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    for y in range(0, base.size[1], 4):
        d.line((0, y, base.size[0], y), fill=(0, 0, 0, alpha))
    base.alpha_composite(layer)


def neon_text(base, xy, text, fnt, color, anchor="la", blur=6):
    layer = Image.new("RGBA", base.size, (0, 0, 0, 0))
    ImageDraw.Draw(layer).text(xy, text, font=fnt, fill=(*color, 255), anchor=anchor)
    if blur:
        base.alpha_composite(layer.filter(ImageFilter.GaussianBlur(blur)))
    base.alpha_composite(layer)


def emblem_card(path, size, radius_frac=0.12, border=CRIMSON):
    im = Image.open(path).convert("RGBA").resize((size, size), Image.LANCZOS)
    im = rounded(im, int(size * radius_frac))
    card = Image.new("RGBA", (size + 40, size + 40), (0, 0, 0, 0))
    d = ImageDraw.Draw(card)
    d.rounded_rectangle((8, 8, size + 32, size + 32), int(size * radius_frac) + 12, outline=(*border, 255), width=4)
    card = card.filter(ImageFilter.GaussianBlur(5))
    ImageDraw.Draw(card).rounded_rectangle((14, 14, size + 26, size + 26), int(size * radius_frac) + 6, outline=(*border, 255), width=2)
    card.alpha_composite(im, (20, 20))
    return card


def backdrop(w, h):
    img = Image.new("RGBA", (w, h), (*VOID, 255))
    glow(img, (int(w * 0.18), int(h * 0.85)), int(h * 0.45), PURPLE, 90)
    glow(img, (int(w * 0.85), int(h * 0.15)), int(h * 0.40), CRIMSON, 70)
    glow(img, (int(w * 0.70), int(h * 0.95)), int(h * 0.30), CYAN, 40)
    grid(img, max(24, w // 48), CYAN, 22)
    scanlines(img)
    return img


def save(img, rel, mode="RGB"):
    p = out / rel
    p.parent.mkdir(parents=True, exist_ok=True)
    img.convert(mode).save(p, optimize=True)
    print("wrote", p)


def wallpaper(w=1920, h=1080):
    img = backdrop(w, h)
    s = int(h * 0.56)
    card = emblem_card(EMBLEM, s)
    img.alpha_composite(card, (int(w * 0.62) - card.width // 2, int(h * 0.44) - card.height // 2))
    neon_text(img, (int(w * 0.08), int(h * 0.36)), "DemonOS", font("CinzelDecorative-Bold.ttf", int(h * 0.11)), CRIMSON, blur=10)
    neon_text(img, (int(w * 0.08), int(h * 0.49)), "PHANTOMCORE", font("Michroma-Regular.ttf", int(h * 0.04)), CYAN, blur=6)
    neon_text(img, (int(w * 0.08), int(h * 0.56)), "LILLITH OS  //  QUANTUMSEC", font("JetBrainsMono.ttf", int(h * 0.024)), PURPLE, blur=4)
    neon_text(img, (int(w * 0.08), int(h * 0.92)), "local-only · consent-gated · whisper from the core",
              font("JetBrainsMono.ttf", int(h * 0.016)), (150, 150, 170), blur=0)
    return img


def grub_bg(w=1920, h=1080):
    img = backdrop(w, h)
    s = int(h * 0.42)
    card = emblem_card(EMBLEM, s)
    img.alpha_composite(card, (int(w * 0.20) - card.width // 2, int(h * 0.42) - card.height // 2))
    neon_text(img, (int(w * 0.20), int(h * 0.74)), "DemonOS", font("CinzelDecorative-Bold.ttf", int(h * 0.07)), CRIMSON, anchor="ma", blur=8)
    neon_text(img, (int(w * 0.20), int(h * 0.83)), "PHANTOMCORE", font("Michroma-Regular.ttf", int(h * 0.028)), CYAN, anchor="ma", blur=5)
    # menu frame (GRUB draws the entries inside it, see grub/themes/demonos/theme.txt)
    layer = Image.new("RGBA", img.size, (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    box = (int(w * 0.42), int(h * 0.22), int(w * 0.92), int(h * 0.78))
    d.rectangle(box, fill=(*VOID, 200))
    d.rectangle(box, outline=(*PURPLE, 255), width=3)
    img.alpha_composite(layer.filter(ImageFilter.GaussianBlur(4)))
    img.alpha_composite(layer)
    neon_text(img, (box[0], box[1] - int(h * 0.05)), "> SUMMON THE CORE_", font("JetBrainsMono.ttf", int(h * 0.026)), CYAN, blur=3)
    return img


def syslinux_splash():
    img = backdrop(640, 480)
    card = emblem_card(PHANTOM, 150, border=CRIMSON)
    img.alpha_composite(card, (640 - card.width - 24, 24))
    neon_text(img, (24, 30), "DemonOS", font("CinzelDecorative-Bold.ttf", 44), CRIMSON, blur=6)
    neon_text(img, (26, 92), "PHANTOMCORE", font("Michroma-Regular.ttf", 18), CYAN, blur=3)
    return img


def plymouth():
    logo = Image.open(PHANTOM).convert("RGBA").resize((256, 256), Image.LANCZOS)
    logo = rounded(logo, 34)
    save(logo, "plymouth/logo.png", "RGBA")
    save(backdrop(1920, 1080), "plymouth/background.png")
    bar_bg = Image.new("RGBA", (400, 10), (*PURPLE, 90))
    ImageDraw.Draw(bar_bg).rectangle((0, 0, 399, 9), outline=(*PURPLE, 255))
    save(bar_bg, "plymouth/progress-bg.png", "RGBA")
    save(Image.new("RGBA", (400, 10), (*CRIMSON, 255)), "plymouth/progress-fg.png", "RGBA")
    txt = Image.new("RGBA", (900, 60), (0, 0, 0, 0))
    neon_text(txt, (450, 30), "AWAKENING THE CORE", font("Michroma-Regular.ttf", 28), CYAN, anchor="mm", blur=4)
    save(txt, "plymouth/title.png", "RGBA")


def main():
    save(wallpaper(), "wallpaper/DemonOS/contents/images/1920x1080.png")
    save(wallpaper(1280, 800), "wallpaper/DemonOS/contents/images/1280x800.png")
    save(wallpaper(400, 225), "wallpaper/DemonOS/contents/screenshot.png")
    save(grub_bg(), "grub/background.png")
    save(syslinux_splash(), "syslinux/splash.png")
    plymouth()
    save(wallpaper(), "sddm/background.png")
    sel = Image.new("RGBA", (16, 16), (*CRIMSON, 200))
    for k in ("c", "n", "s", "e", "w", "ne", "nw", "se", "sw"):
        save(sel, f"grub/select_{k}.png", "RGBA")


if __name__ == "__main__":
    main()
