import sys,json,numpy as np
from PIL import Image
from scipy import ndimage as nd
c=sys.argv[1]; d=f'/workspace/lillith/wt-seasonal-sn/assets/generated/{c}/'
sh=np.array(Image.open(d+f'{c}_sheet.png').convert('RGBA')); N=sh.shape[1]//400; S8=np.ones((3,3),bool)
for k in range(N):
    a=sh[:,k*400:(k+1)*400,3]; lab,n=nd.label(a>8,S8)
    if n<2: continue
    sz=nd.sum(np.ones(lab.shape),lab,range(1,n+1)); big=int(np.argmax(sz))+1; dist=nd.distance_transform_edt(lab!=big)
    for i in range(1,n+1):
        if i==big: continue
        m=lab==i
        if dist[m].min()>3 and a[m].max()>16:
            ys,xs=np.nonzero(m); print(k,'size',int(m.sum()),'at',int(xs.mean()),int(ys.mean()),'dist',round(float(dist[m].min()),1),'amax',int(a[m].max()))
