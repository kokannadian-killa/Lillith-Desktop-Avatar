import cv2,numpy as np,sys
from PIL import Image
# chest.py video out.png every x0 y0 w h : dense crops of torso for exposure review
v,out=sys.argv[1],sys.argv[2]; ev=int(sys.argv[3]); x0,y0,w,h=map(int,sys.argv[4:8])
cap=cv2.VideoCapture(v); i=0; th=[]
while True:
    ok,f=cap.read()
    if not ok: break
    if i%ev==0:
        t=cv2.cvtColor(f[y0:y0+h,x0:x0+w],cv2.COLOR_BGR2RGB); t=cv2.resize(t,(160,160)); cv2.putText(t,str(i),(3,14),0,0.45,(0,0,255),1); th.append(t)
    i+=1
C=13; rows=[np.hstack(th[k:k+C]+[np.zeros_like(th[0])]*(C-len(th[k:k+C]))) for k in range(0,len(th),C)]
Image.fromarray(np.vstack(rows)).save(out)
