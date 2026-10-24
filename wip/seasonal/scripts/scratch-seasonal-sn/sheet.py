import json,os,sys
from PIL import Image, ImageDraw
W='/workspace/lillith/wt-seasonal-sn/assets/generated'
S=sys.argv[1]
CL={'summer':['summer_popsicle_lick','summer_shades_peek_wink','summer_lotion_squeeze','summer_string_oops'],
    'newyear':['newyear_champagne_rim_lick','newyear_strawberry_bite','newyear_midnight_kiss_squeeze','newyear_strap_oops']}[S]
out=Image.new('RGBA',(4*400,2*430),(48,48,52,255)); d=ImageDraw.Draw(out)
for i,c in enumerate(CL):
    x=i*400
    j=f'{W}/{c}/{c}.json'
    if not os.path.exists(j): d.text((x+10,200),c+'\n(pending)',fill='white'); continue
    z=json.load(open(j))['cleavage_zoom']['zoom_per_frame']; f=max(range(len(z)),key=lambda k:z[k]); n=len(z)//2-30
    n=min(range(len(z)),key=lambda k:(z[k],abs(k-len(z)//2)))
    sh=Image.open(f'{W}/{c}/{c}_sheet.png')
    for r,(ff,lab) in enumerate([(n,'normal'),(f,'peak')]):
        out.alpha_composite(sh.crop((ff*400,0,ff*400+400,400)),(x,r*430))
        d.text((x+6,r*430+405),f'{c} {lab} f{ff} z{z[ff]:.2f} {len(z)}f',fill='white')
os.makedirs('/workspace/lillith/chat_shots',exist_ok=True); out.convert('RGB').save(f'/workspace/lillith/chat_shots/seasonal_{S}_sheet.png')
