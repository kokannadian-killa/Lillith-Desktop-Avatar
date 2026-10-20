"""event_strip.py out.png clip...: all frames per row, small, with icon_origin dot"""
import sys,glob,json
from PIL import Image,ImageDraw
out,clips=sys.argv[1],sys.argv[2:];c=120
rows=[(k,sorted(glob.glob(f'{k}/frames/*.png')),json.load(open(f'{k}/{k}.json'))) for k in clips]
W=150+max(len(r[1]) for r in rows)*c;o=Image.new('RGBA',(W,len(rows)*c),(128,128,128,255));d=ImageDraw.Draw(o)
for y,(k,fs,j) in enumerate(rows):
  d.text((2,y*c+50),k.replace('event_',''),fill='white')
  for x,f in enumerate(fs):
    o.alpha_composite(Image.open(f).convert('RGBA').resize((c,c),Image.LANCZOS),(150+x*c,y*c))
    io=j.get('frame_points',{}).get('icon_origin')
    if io: px,py=io[x];X=150+x*c+px*c//400;Y=y*c+py*c//400;d.ellipse([X-2,Y-2,X+2,Y+2],fill='yellow')
o.convert('RGB').save(out)
