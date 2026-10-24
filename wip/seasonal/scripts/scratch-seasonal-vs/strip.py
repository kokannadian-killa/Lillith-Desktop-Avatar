import cv2,numpy as np,sys
from PIL import Image
# strip.py video out.png [every]: 6-col grid of frames every N, with bbox stats
v=sys.argv[1]; out=sys.argv[2]; ev=int(sys.argv[3]) if len(sys.argv)>3 else 12
cap=cv2.VideoCapture(v); i=0; th=[]; bb=[]
while True:
    ok,f=cap.read()
    if not ok: break
    m=(f.min(2)<235); ys,xs=np.nonzero(m)
    if len(ys): bb.append((ys.min(),ys.max()))
    if i%ev==0:
        t=cv2.cvtColor(f[:, 320:960],cv2.COLOR_BGR2RGB); t=cv2.resize(t,(320,360)); cv2.putText(t,str(i),(5,20),0,0.6,(0,0,255),2); th.append(t)
    i+=1
bb=np.array(bb); print(v.split('/')[-1],'frames',i,'top',bb[:,0].min(),bb[:,0].max(),'bottom',bb[:,1].min(),bb[:,1].max())
C=6; rows=[np.hstack(th[k:k+C]+[np.zeros_like(th[0])]*(C-len(th[k:k+C]))) for k in range(0,len(th),C)]
Image.fromarray(np.vstack(rows)).save(out)
