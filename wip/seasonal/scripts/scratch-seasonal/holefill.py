"""holefill.py clip [maxarea] [--dry]: fill small fully-enclosed alpha holes (punched specular highlights) inside the body."""
import sys,json,subprocess,numpy as np,cv2
from PIL import Image
from scipy import ndimage as nd
import os; WT=os.environ.get('HF_WT','/workspace/lillith/wt-seasonal'); c=sys.argv[1]; mx=int(sys.argv[2]) if len(sys.argv)>2 and sys.argv[2].isdigit() else 80
dry='--dry' in sys.argv; DEP=5; d=f'{WT}/assets/generated/{c}'; p=f'{d}/{c}_sheet.png'
im=np.array(Image.open(p).convert('RGBA')); N=im.shape[1]//400; log={}; boxes=[]
for i in range(N):
    cell=im[:,i*400:(i+1)*400]; a=cell[...,3]
    bk=(cell[...,:3].max(-1)==0)&(a==255)
    if bk.any():
        fx=cv2.inpaint(np.ascontiguousarray(cell[...,:3]),(bk*255).astype(np.uint8),3,cv2.INPAINT_TELEA); cell[bk,:3]=fx[bk]; log[f'b{i}']=int(bk.sum())
    solid=a>=200; filled=nd.binary_fill_holes(solid); hole=filled&~solid; dep=nd.distance_transform_edt(filled)
    lab,nl=nd.label(hole,np.ones((3,3),bool)); n=0
    for k in range(1,nl+1):
        m=lab==k; s=int(m.sum())
        if s<=mx and dep[m].min()>=DEP:
            if True:
                rgb=np.ascontiguousarray(cell[...,:3]); fix=cv2.inpaint(rgb,(m*255).astype(np.uint8),3,cv2.INPAINT_TELEA); cell[m,:3]=fix[m]
            cell[m,3]=255; n+=s
            ys,xs=np.nonzero(m); boxes.append((i,int(ys.mean()),int(xs.mean()),s))
    if n: log[i]=n
print(c,'holefill',log); json.dump(boxes,open(os.environ.get('HF_REV','/workspace/lillith/scratch-seasonal/review/')+f'holes_{c}.json','w'))
if not dry and log:
    Image.fromarray(im).save(p)
    js=json.load(open(f'{d}/{c}.json')); v=js.get('video'); crf=v.get('crf',16) if isinstance(v,dict) else 16
    subprocess.run([sys.executable,f'{WT}/tools/video-pipeline/pack_mp4.py',d,'--force','--crf',str(crf),'--manifest',f'{WT}/assets/generated/manifest.json'],check=True,capture_output=True)
