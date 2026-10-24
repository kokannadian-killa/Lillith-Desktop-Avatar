"""sheet.py <season> <clip>...: per clip, normal-framing frame 0 + peak-zoom frame -> chat_shots/seasonal_<season>_sheet.png"""
import json,os,sys
from PIL import Image, ImageDraw
W='/workspace/lillith/wt-seasonal/assets/generated'; sid=sys.argv[1]; CL=sys.argv[2:]
out=Image.new('RGBA',(len(CL)*400,2*400+30),(48,48,52,255)); d=ImageDraw.Draw(out)
for i,c in enumerate(CL):
    x=i*400; j=f'{W}/{c}/{c}.json'
    if not os.path.exists(j): d.text((x+10,200),c+'\n(pending)',fill='white'); continue
    z=json.load(open(j))['cleavage_zoom']['zoom_per_frame']; f=max(range(len(z)),key=lambda k:z[k])
    sh=Image.open(f'{W}/{c}/{c}_sheet.png')
    for r,k in enumerate([0,f]): out.alpha_composite(sh.crop((k*400,0,k*400+400,400)),(x,r*400))
    d.text((x+6,805),f'{c}  f0 / f{f} z{z[f]:.2f}  {len(z)}f',fill='white')
os.makedirs('/workspace/lillith/chat_shots',exist_ok=True); p=f'/workspace/lillith/chat_shots/seasonal_{sid}_sheet.png'; out.convert('RGB').save(p); print(p)
