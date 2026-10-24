import json,os,sys
from PIL import Image, ImageDraw
W='/workspace/lillith/wt-seasonal-vs/assets/generated'; sid=sys.argv[1]; CL=sys.argv[2:]
# per clip: normal-framing first frame + peak zoom frame, on dark grey
out=Image.new('RGBA',(len(CL)*800,430),(40,40,46,255)); d=ImageDraw.Draw(out)
for i,c in enumerate(CL):
    z=json.load(open(f'{W}/{c}/{c}.json'))['cleavage_zoom']['zoom_per_frame']; f=max(range(len(z)),key=lambda k:z[k])
    sh=Image.open(f'{W}/{c}/{c}_sheet.png').convert('RGBA'); mid=[k for k in range(len(z)) if abs(z[k]-1)<1e-6][len(z)//4]
    out.alpha_composite(sh.crop((mid*400,0,mid*400+400,400)),(i*800,0)); out.alpha_composite(sh.crop((f*400,0,f*400+400,400)),(i*800+400,0))
    d.text((i*800+6,405),f'{c}  f{mid} z1.00 | f{f} z{z[f]:.2f}  {len(z)}f/{len(z)/12:.1f}s',fill='white')
os.makedirs('/workspace/lillith/chat_shots',exist_ok=True); out.convert('RGB').save(f'/workspace/lillith/chat_shots/seasonal_{sid}_sheet.png'); print('ok')
