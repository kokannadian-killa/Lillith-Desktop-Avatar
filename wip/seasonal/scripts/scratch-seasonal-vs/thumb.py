import sys,numpy as np
from PIL import Image
c,sid=sys.argv[1],sys.argv[2]; f=int(sys.argv[3]) if len(sys.argv)>3 else 0
WT='/workspace/lillith/wt-seasonal-vs'
im=np.array(Image.open(f'{WT}/assets/generated/{c}/{c}_sheet.png').convert('RGBA'))[:,f*400:(f+1)*400]
a=im[...,3]>16; ys,xs=np.nonzero(a); top=ys.min(); H=ys.max()-top
side=int(H*0.36); cy=top; rows=a[top:top+side]; cx=int(np.nonzero(rows.any(0))[0].mean())
x0=max(0,min(400-side,cx-side//2)); crop=im[top:top+side,x0:x0+side]
out=Image.fromarray(crop).resize((128,128),Image.LANCZOS)
import os; os.makedirs(f'{WT}/assets/generated/outfits',exist_ok=True); out.save(f'{WT}/assets/generated/outfits/{sid}.png')
print(top,side,x0,out.size,out.mode)
