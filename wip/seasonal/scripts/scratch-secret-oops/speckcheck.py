import numpy as np,json
from PIL import Image
from scipy import ndimage as nd
c='secret_oops_retie'; d=f'/workspace/lillith/wt-secret-oops/assets/generated/{c}/'
sh=np.array(Image.open(d+f'{c}_sheet.png').convert('RGBA')); N=sh.shape[1]//400
z=json.load(open(d+f'{c}.json'))['cleavage_zoom']['zoom_per_frame']
S8=np.ones((3,3),bool); edge=[];free=[]
for k in range(N):
    a=sh[:,k*400:(k+1)*400,3]; lab,n=nd.label(a>8,S8)
    if n<2: continue
    sz=nd.sum(np.ones(lab.shape),lab,range(1,n+1)); big=int(np.argmax(sz))+1; dist=nd.distance_transform_edt(lab!=big)
    for i in range(1,n+1):
        if i==big: continue
        m=lab==i
        if dist[m].min()>3 and a[m].max()>16:
            ys,xs=np.where(m); t=(ys.min()==0 or xs.min()==0 or xs.max()==399)
            (edge if t else free).append((k,int(m.sum()),int(xs.mean()),int(ys.mean()),z[k]))
print('crop-cut parts touching canvas edge',len(edge),'(all at zoom>1:',all(e[4]>1 for e in edge),')')
print('free floating specks',len(free),free[:30])
