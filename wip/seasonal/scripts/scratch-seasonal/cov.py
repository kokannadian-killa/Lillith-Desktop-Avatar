import sys
from PIL import Image
# cov.py <wt> <clip> <out> [start] [end] [step]: tiles of upper-body crops on green
wt,c,out=sys.argv[1:4]; a=int(sys.argv[4]) if len(sys.argv)>4 else 0
sh=Image.open(f'{wt}/assets/generated/{c}/{c}_sheet.png').convert('RGBA'); n=sh.width//400
b=int(sys.argv[5]) if len(sys.argv)>5 else n; st=int(sys.argv[6]) if len(sys.argv)>6 else 3
ks=list(range(a,min(b,n),st)); cols=10; rows=(len(ks)+cols-1)//cols
o=Image.new('RGB',(cols*120,rows*120),(0,150,0))
for i,k in enumerate(ks):
    f=sh.crop((k*400,0,k*400+400,400)).crop((60,30,340,310)).resize((120,120)); bg=Image.new('RGBA',f.size,(0,150,0,255)); bg.alpha_composite(f)
    o.paste(bg.convert('RGB'),((i%cols)*120,(i//cols)*120))
o.save(out); print(out,len(ks),n)
