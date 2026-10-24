import sys,json,glob,numpy as np
from PIL import Image
from scipy import ndimage as nd
c=sys.argv[1]; d=f'/workspace/lillith/wt-seasonal-sn/assets/generated/{c}/'
j=json.load(open(d+f'{c}.json')); sh=np.array(Image.open(d+f'{c}_sheet.png').convert('RGBA'))
N=sh.shape[1]//400; z=j.get('cleavage_zoom',{}).get('zoom_per_frame',[1]*N)
cen=[];spk=0;faint=0;low=0;S8=np.ones((3,3),bool)
for k in range(N):
    f=sh[:,k*400:(k+1)*400]; a=f[...,3]
    ys,xs=np.nonzero(a>128); cen.append((xs.mean(),ys.mean()))
    lab,n=nd.label(a>8,S8)
    if n>1:
        sz=nd.sum(np.ones(lab.shape),lab,range(1,n+1)); big=int(np.argmax(sz))+1
        dist=nd.distance_transform_edt(lab!=big)
        for i in range(1,n+1):
            if i==big: continue
            m=lab==i
            if dist[m].min()>3 and a[m].max()>16: spk+=1
            else: faint+=1
    low+=int((a[393:]>0).sum())
cen=np.array(cen); jm=np.hypot(*np.diff(cen,axis=0).T)
nz=[k for k in range(N-1) if abs(z[k]-1)<1e-6 and abs(z[k+1]-1)<1e-6]
dz=max(abs(z[k+1]-z[k]) for k in range(N-1))
print(f'{c}: frames {N} ({N/12:.1f}s) max centroid jump normal-framing {jm[nz].max():.2f}px, all {jm.max():.2f}px (zoom dz max {dz:.3f}/frame), loop seam {np.hypot(*(cen[0]-cen[-1])):.2f}px, floating specks (>3px from body, a>16) {spk}, faint edge fragments <=3px from body {faint}, rows>=393 nonzero px {low}')
