import sys,numpy as np
from PIL import Image
c=sys.argv[1]; fr=[int(x) for x in sys.argv[2].split(',')]
im=np.array(Image.open(f'/workspace/lillith/wt-seasonal-vs/assets/generated/{c}/{c}_sheet.png').convert('RGBA'))
t=[]
for i in fr:
    cell=im[:,i*400:(i+1)*400].astype(float); al=cell[...,3:4]/255; bg=np.full((400,400,3),(30,60,30),float)
    t.append((cell[...,:3]*al+bg*(1-al)).astype(np.uint8))
Image.fromarray(np.concatenate(t,1)).save(f'review/look_{c}.png')
