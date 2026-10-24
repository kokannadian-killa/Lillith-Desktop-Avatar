import sys,numpy as np,cv2
from PIL import Image
from scipy import ndimage as nd
def holes(f,smax=60,bfrac=0.55):
    a=f[...,3]; lab,n=nd.label(a<250); border=set(np.unique(np.r_[lab[0],lab[-1],lab[:,0],lab[:,-1]]))
    sel=np.zeros(a.shape,bool)
    blk=(f[...,:3].max(2)<70)&(a>200)
    reg=nd.binary_fill_holes(nd.binary_closing(blk,iterations=4))
    for i in range(1,n+1):
        if i in border: continue
        m=lab==i; s=m.sum()
        if s>smax: continue
        ring=nd.binary_dilation(m,iterations=2)&~m
        if s<=smax and a[ring].mean()>=235: sel|=m
    return sel
if __name__=='__main__':
    mode=sys.argv[1]; p=sys.argv[2]
    sh=np.array(Image.open(p).convert('RGBA')); N=sh.shape[1]//400
    if mode=='viz':
        tiles=[]
        for k in map(int,sys.argv[4].split(',')):
            f=sh[:,k*400:(k+1)*400]; a=f[...,3:]/255; out=f[...,:3]*a+np.array([0,255,0])*(1-a)
            out[holes(f)]=[255,0,0]; tiles.append(out.astype(np.uint8))
        Image.fromarray(np.vstack([np.hstack(tiles[j:j+4]) for j in range(0,len(tiles),4)])).save(sys.argv[3])
    else:
        tot={}
        for k in range(N):
            f=sh[:,k*400:(k+1)*400]; m=holes(f)
            if m.any():
                bgr=np.ascontiguousarray(f[...,:3]); 
                ip=cv2.inpaint(bgr,(m|(f[...,3]<250)&nd.binary_dilation(m,iterations=1)).astype(np.uint8),3,cv2.INPAINT_TELEA)
                f[m,:3]=ip[m]; f[m,3]=255; tot[k]=int(m.sum())
        Image.fromarray(sh,'RGBA').save(p,optimize=True); print('filled frames',len(tot),'px',sum(tot.values()))
