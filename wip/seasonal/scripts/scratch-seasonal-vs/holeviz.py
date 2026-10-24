import sys,numpy as np
from PIL import Image
from scipy import ndimage as nd
c=sys.argv[1]; fr=[int(x) for x in sys.argv[2].split(',')]; mx=int(sys.argv[3]) if len(sys.argv)>3 else 80
im=np.array(Image.open(f'/workspace/lillith/wt-seasonal-vs/assets/generated/{c}/{c}_sheet.png').convert('RGBA'))
t=[]
for i in fr:
    cell=im[:,i*400:(i+1)*400]; a=cell[...,3]; solid=a>=200; F=nd.binary_fill_holes(solid); hole=F&~solid; dep=nd.distance_transform_edt(F)
    lab,nl=nd.label(hole,np.ones((3,3),bool)); sz=nd.sum(hole,lab,range(1,nl+1)); small=np.isin(lab,[k+1 for k,s in enumerate(sz) if s<=mx and dep[lab==k+1].min()>=5])
    bg=np.full((400,400,3),(40,40,40),float); al=a[...,None]/255; comp=(cell[...,:3]*al+bg*(1-al)).astype(np.uint8)
    comp[small]=(0,255,0); t.append(comp)
Image.fromarray(np.concatenate(t,1)).resize((len(fr)*600,600)).save(f'review/holes_{c}.png')
