"""fixall.py clip: batch-19 style residual fixes on the sheet (after matte-clean): drop detached specks (<=40 px),
zero alpha on fringeqa white mask (>=4 px/frame) and white_edge light-edge mask (frames over FRAME_MAX-2), repack MP4,
then the same two masks decoded from the MP4 (1 px dilated into soft alpha), repack."""
import sys, json, subprocess, numpy as np
from PIL import Image
from scipy import ndimage as nd
WT='/workspace/lillith/wt-seasonal'
sys.path.insert(0,f'{WT}/tools/video-pipeline'); import white_edge as we
sys.path.insert(0,'/workspace/lillith/scratch-fringeqa'); import fringeqa as F
S8=np.ones((3,3),bool); c=sys.argv[1]; d=f'{WT}/assets/generated/{c}'; p=f'{d}/{c}_sheet.png'
def repack():
    js=json.load(open(f'{d}/{c}.json')); v=js.get('video'); crf=v.get('crf',16) if isinstance(v,dict) else 16
    subprocess.run([sys.executable,f'{WT}/tools/video-pipeline/pack_mp4.py',d,'--force','--crf',str(crf),'--manifest',f'{WT}/assets/generated/manifest.json'],check=True,capture_output=True)
def ana(cell):
    col=cell[...,:3].astype(np.int32); araw=cell[...,3].astype(np.int32)
    a=np.where(araw<=3,0,np.where(araw>=251,255,araw)); P=(col*a[...,None]+127)//255
    return F.analyse(col,araw,a,P,None)
im=np.array(Image.open(p).convert('RGBA')); N=im.shape[1]//400; log={}
for i in range(N):
    cell=im[:,i*400:(i+1)*400]; ch=0
    lab,nl=nd.label(cell[...,3]>3,S8)
    if nl>1:
        sz=nd.sum(np.ones(lab.shape),lab,range(1,nl+1)); big=int(np.argmax(sz))+1
        for k,s in enumerate(sz,1):
            if k!=big and s<=40: cell[lab==k,3]=0; ch+=int(s)
    for _ in range(2):
        w,rim,snap,white,halo,per=ana(cell)
        if w<4: break
        cell[white,3]=0; ch+=int(white.sum())
    for _ in range(3):
        m=we.light_edge_mask(cell)
        if m.sum()<=we.FRAME_MAX-2: break
        cell[m,3]=0; ch+=int(m.sum())
    if ch: log[i]=ch
Image.fromarray(im,'RGBA').save(p,optimize=True); repack()
im=np.array(Image.open(p).convert('RGBA')); m4={}
for i,pk in F.frames_mp4(f'{d}/{c}.mp4',400,400):
    col,araw,a,P=F.unpack(pk,400); wm=F.analyse(col,araw,a,P,None)[3]; lm=we.light_edge_mask(np.dstack([col,araw]).astype(np.uint8))
    m=(wm if wm.sum()>=4 else np.zeros_like(wm)) | (lm if lm.sum()>we.FRAME_MAX-2 else np.zeros_like(lm))
    if m.any():
        cell=im[:,i*400:(i+1)*400]; cell[nd.binary_dilation(m)&(cell[...,3]<255),3]=0; cell[m,3]=0; m4[i]=int(m.sum())
if m4: Image.fromarray(im,'RGBA').save(p,optimize=True); repack()
print(c,'sheet fixes (frame:px)',log,'| mp4-decoded fixes',m4)
