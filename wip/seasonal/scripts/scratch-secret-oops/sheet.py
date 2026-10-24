import cv2,sys,numpy as np
d,start,stop,step,x0,y0,x1,y1,cols,w,out=sys.argv[1],*map(int,sys.argv[2:11]),sys.argv[11]
tiles=[]
for i in range(start,stop+1,step):
    im=cv2.imread(f'{d}/f_{i:03d}.png')
    if im is None: break
    c=im[y0:y1,x0:x1]; h=int(c.shape[0]*w/c.shape[1]); c=cv2.resize(c,(w,h),interpolation=cv2.INTER_AREA)
    cv2.putText(c,str(i),(3,16),0,0.5,(0,0,255),1); tiles.append(c)
while len(tiles)%cols: tiles.append(np.full_like(tiles[0],255))
rows=[np.hstack(tiles[j:j+cols]) for j in range(0,len(tiles),cols)]
cv2.imwrite(out,np.vstack(rows))
