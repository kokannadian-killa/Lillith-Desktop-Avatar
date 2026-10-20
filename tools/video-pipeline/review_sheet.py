"""review_sheet.py <out.png> <title> clip1 clip2 ...  : contact rows (<=12 frames each) with baseline/anchor/points."""
import sys, glob, json
from PIL import Image, ImageDraw
out_p, title, clips = sys.argv[1], sys.argv[2], sys.argv[3:]
c = 150; per = 12
W = 190 + per * c; H = 30 + len(clips) * (c + 8)
out = Image.new('RGBA', (W, H), (128, 128, 128, 255)); dr = ImageDraw.Draw(out)
dr.text((4, 6), title + '  red=baseline 392, cyan=anchor x, yellow=named point / seat', fill=(255, 255, 255, 255))
for r, k in enumerate(clips):
    fs = sorted(glob.glob(f'{k}/frames/*.png')); d = json.load(open(f'{k}/{k}.json'))
    step = -(-len(fs) // per); sel = list(range(0, len(fs), step))[:per]; y = 30 + r * (c + 8)
    tim = 'durations' if d.get('frame_durations_ms') else f"@{d['fps']:g}fps"
    dr.multiline_text((4, y + 36), f"{k}\n{d['frame_count']}f {tim}\n{'loop' if d['loop'] else 'once'}\n{'every %dth' % step if step > 1 else 'all frames'}", fill=(255, 255, 255, 255))
    for i, fi in enumerate(sel):
        x = 190 + i * c; out.alpha_composite(Image.open(fs[fi]).convert('RGBA').resize((c, c), Image.LANCZOS), (x, y))
        dr.line([(x, y + c * 392 // 400), (x + c, y + c * 392 // 400)], fill=(255, 80, 80, 255))
        ax = x + c * d['anchor'][0] // 400; dr.line([(ax, y + c - 8), (ax, y + c)], fill=(0, 255, 255, 255))
        fp = d.get('frame_points', {})
        for key in ('grab_point', 'beam_origin'):
            if key in d:
                px, py = fp[key][fi] if key in fp and len(fp[key]) > fi else d[key]
                dr.ellipse([x + px * c // 400 - 2, y + py * c // 400 - 2, x + px * c // 400 + 2, y + py * c // 400 + 2], fill=(255, 255, 0, 255))
out.convert('RGB').save(out_p); print(out_p, out.size)
