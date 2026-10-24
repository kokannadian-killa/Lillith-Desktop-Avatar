import cv2,numpy as np,sys
from PIL import Image
v=sys.argv[1]; out=sys.argv[2]; cap=cv2.VideoCapture(v); i=0; bb=[]; th=[]
while True:
    ok,f=cap.read()
    if not ok: break
    if i%4==0:
        m=(f.min(2)<235); ys,xs=np.nonzero(m); bb.append((i,ys.min(),ys.max(),xs.min(),xs.max()))
    if i%24==0: th.append(cv2.cvtColor(cv2.resize(f,(320,180)),cv2.COLOR_BGR2RGB))
    i+=1
bb=np.array(bb); h=bb[:,2]-bb[:,1]
print(v.split('/')[-1],'frames',i,'top min/max',bb[:,1].min(),bb[:,1].max(),'bottom min/max',bb[:,2].min(),bb[:,2].max(),'height min/max',h.min(),h.max())
rows=[np.hstack(th[k:k+4]+[np.zeros((180,320,3),np.uint8)]*(4-len(th[k:k+4]))) for k in range(0,len(th),4)]
Image.fromarray(np.vstack(rows)).save(out)
