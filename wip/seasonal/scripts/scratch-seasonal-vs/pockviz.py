import sys,numpy as np
from PIL import Image
sys.path.insert(0,'/workspace/lillith/scratch-fringeqa'); import fringeqa as F
c=sys.argv[1]; d=f'/workspace/lillith/wt-seasonal-vs/assets/generated/{c}'
im=np.array(Image.open(f'{d}/{c}_sheet.png').convert('RGBA')); N=im.shape[1]//400
res=[]
for i in range(N):
    cell=im[:,i*400:(i+1)*400]
    col=cell[...,:3].astype(np.int32); araw=cell[...,3].astype(np.int32)
    a=np.where(araw<=3,0,np.where(araw>=251,255,araw)); P=(col*a[...,None]+127)//255
    w,rim,snap,white,halo,per=F.analyse(col,araw,a,P,None)
    res.append((per['pocket'],i))
res.sort(reverse=True); print(res[:8])
tiles=[]
for p,i in res[:4]:
    cell=im[:,i*400:(i+1)*400].copy(); bg=np.zeros((400,400,3),np.uint8); bg[:]=(40,40,40)
    al=cell[...,3:4]/255; comp=(cell[...,:3]*al+bg*(1-al)).astype(np.uint8)
    col=cell[...,:3].astype(np.int32); araw=cell[...,3].astype(np.int32)
    a=np.where(araw<=3,0,np.where(araw>=251,255,araw)); P=(col*a[...,None]+127)//255
    *_,halo,per=F.analyse(col,araw,a,P,None)
    mn=col.min(-1); sat=col.max(-1)-mn; nw=(mn>=236)&(sat<=12)&(a>=128)
    comp[halo&nw]=(0,255,0); tiles.append(comp)
Image.fromarray(np.concatenate(tiles,1)).resize((1600*1,400)).save(f'review/pock_{c}.png')
