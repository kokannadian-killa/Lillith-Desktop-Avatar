"""blend_sheet.py out.png : every blend_* row = clip A boundary frame | blend frames | clip B boundary frame"""
import json, glob, sys
from PIL import Image, ImageDraw
names = sorted(glob.glob('blend_*')); c = 130
o = Image.new('RGBA', (170 + 8 * c, len(names) * c), (128, 128, 128, 255)); d = ImageDraw.Draw(o)
for y, n in enumerate(names):
    j = json.load(open(f'{n}/{n}.json')); ims = [Image.open(f).convert('RGBA') for f in sorted(glob.glob(f'{n}/frames/*.png'))]
    if len(j.get('source_frames', {})) == 2:
        sf = j['source_frames']; ca, cb = j['from_clip'], j['to_clip']
        ims = [Image.open(sorted(glob.glob(f'{ca}/frames/*.png'))[sf[ca]]).convert('RGBA')] + ims + [Image.open(sorted(glob.glob(f'{cb}/frames/*.png'))[sf[cb]]).convert('RGBA')]
    meth = 'video' if 'video' in j.get('blend_method', '') else ('reversed' if j.get('from_clip') == 'wake' else 'warp')
    d.text((2, y * c + 40), n.replace('blend_', '') + f'\n[{meth}]' + '\nA | blend | B', fill='white')
    for x, i in enumerate(ims):
        o.alpha_composite(i.resize((c, c), Image.LANCZOS), (170 + x * c, y * c))
        d.line([(170 + x * c, y * c + c * 392 // 400), (170 + x * c + c, y * c + c * 392 // 400)], fill=(255, 80, 80, 255))
o.convert('RGB').save(sys.argv[1])
