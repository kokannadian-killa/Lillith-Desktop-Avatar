import cv2,numpy as np,sys
from PIL import Image
v,out,a,b=sys.argv[1],sys.argv[2],int(sys.argv[3]),int(sys.argv[4]); step=int(sys.argv[5]) if len(sys.argv)>5 else 4
cap=cv2.VideoCapture(v); i=0; th=[]
while True:
    ok,f=cap.read()
    if not ok: break
    if a<=i<=b and (i-a)%step==0:
        c=cv2.resize(f[110:350,520:760],(240,240)); cv2.putText(c,str(i),(4,14),0,0.45,(0,0,255),1); th.append(cv2.cvtColor(c,cv2.COLOR_BGR2RGB))
    i+=1
n=8; rows=[np.hstack(th[k:k+n]+[np.full((240,240,3),255,np.uint8)]*(n-len(th[k:k+n]))) for k in range(0,len(th),n)]
Image.fromarray(np.vstack(rows)).save(out)
