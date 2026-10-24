"""edgefix.py clip: after holefill, zero white_edge light-edge pixels only on the OUTER silhouette (never inside enclosed body areas), repack."""
import sys, json, subprocess, numpy as np
from PIL import Image
from scipy import ndimage as nd
WT='/workspace/lillith/wt-seasonal-sn'
sys.path.insert(0,f'{WT}/tools/video-pipeline'); import white_edge as we
c=sys.argv[1]; d=f'{WT}/assets/generated/{c}'; p=f'{d}/{c}_sheet.png'
im=np.array(Image.open(p).convert('RGBA')); N=im.shape[1]//400; log={}
for i in range(N):
    cell=im[:,i*400:(i+1)*400]
    for _ in range(3):
        m=we.light_edge_mask(cell)
        if m.sum()<=we.FRAME_MAX-3: break
        outside=nd.binary_dilation(~nd.binary_fill_holes(cell[...,3]>=200),iterations=3)
        m=m&outside
        if not m.any(): break
        cell[m,3]=0; log[i]=log.get(i,0)+int(m.sum())
print(c,'edgefix',log)
if log:
    Image.fromarray(im,'RGBA').save(p,optimize=True)
    js=json.load(open(f'{d}/{c}.json')); v=js.get('video'); crf=v.get('crf',16) if isinstance(v,dict) else 16
    subprocess.run([sys.executable,f'{WT}/tools/video-pipeline/pack_mp4.py',d,'--force','--crf',str(crf),'--manifest',f'{WT}/assets/generated/manifest.json'],check=True,capture_output=True)
