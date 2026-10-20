"""all_sheet.py out.png : one representative frame per manifest_v2 clip (middle frame; events at 60%), labeled, for sign-off."""
import json, sys
from PIL import Image, ImageDraw
m = json.load(open('manifest_v2.json')); A = [a for a in m['animations'] if a['name'] != 'idle']
cols, c, lab = 12, 170, 30; rows = -(-len(A) // cols)
o = Image.new('RGBA', (cols * c, 40 + rows * (c + lab)), (112, 112, 120, 255)); d = ImageDraw.Draw(o)
d.text((6, 8), f'Lillith generated_v2 - {len(A)} clips (idle = alias of idle_a). One representative frame each; red line = baseline y=392. Draft manifest_v2, not live.', fill='white')
for k, a in enumerate(A):
    n = a['frame_count']; i = int(n * (0.6 if a['name'].startswith('event_') else 0.5)); i = min(i, n - 1)
    sh = Image.open(a['sheet']).crop((400 * i, 0, 400 * i + 400, 400)).convert('RGBA').resize((c, c), Image.LANCZOS)
    x, y = (k % cols) * c, 40 + (k // cols) * (c + lab)
    o.alpha_composite(sh, (x, y)); d.line([(x, y + c * 392 // 400), (x + c, y + c * 392 // 400)], fill=(255, 80, 80, 255))
    tim = 'loop' if a['loop'] else 'once'
    d.text((x + 3, y + c + 2), a['name'][:27], fill='white'); d.text((x + 3, y + c + 15), f"{n}f {tim}", fill=(210, 210, 210, 255))
o.convert('RGB').save(sys.argv[1]); print(sys.argv[1], o.size, len(A))
