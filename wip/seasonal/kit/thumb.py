"""thumb.py <outfit id> <clip> [frame]: 128x128 RGBA transparent thumbnail (head-to-hips crop of a normal-framing frame)."""
import sys,numpy as np
from PIL import Image
W='/workspace/lillith/wt-seasonal/assets/generated'; oid,c=sys.argv[1],sys.argv[2]; k=int(sys.argv[3]) if len(sys.argv)>3 else 0
im=Image.open(f'{W}/{c}/{c}_sheet.png').crop((k*400,0,k*400+400,400)); a=np.asarray(im)[...,3]
ys,xs=np.nonzero(a>16); top=ys.min(); h=ys.max()-top; side=int(h*0.55); cx=int(np.median(xs))
box=(cx-side//2,top-4,cx-side//2+side,top-4+side); t=im.crop(box).resize((128,128),Image.LANCZOS)
import os; os.makedirs(f'{W}/outfits',exist_ok=True); t.save(f'{W}/outfits/{oid}.png'); print(f'{W}/outfits/{oid}.png',box)
