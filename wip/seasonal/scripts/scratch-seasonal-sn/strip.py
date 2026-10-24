import cv2,numpy as np,sys
from PIL import Image
v,out=sys.argv[1],sys.argv[2]; step=int(sys.argv[3]) if len(sys.argv)>3 else 12
cap=cv2.VideoCapture(v); i=0; th=[]
while True:
    ok,f=cap.read()
    if not ok: break
    if i%step==0:
        c=f[:, 440:840]; c=cv2.resize(c,(200,360)); cv2.putText(c,str(i),(4,14),0,0.45,(0,0,255),1); th.append(cv2.cvtColor(c,cv2.COLOR_BGR2RGB))
    i+=1
n=9; rows=[np.hstack(th[k:k+n]+[np.full((360,200,3),255,np.uint8)]*(n-len(th[k:k+n]))) for k in range(0,len(th),n)]
Image.fromarray(np.vstack(rows)).save(out)
