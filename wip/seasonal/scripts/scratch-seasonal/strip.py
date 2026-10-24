import sys
from PIL import Image
c=sys.argv[1]; sh=Image.open(f'/workspace/lillith/wt-seasonal/assets/generated/{c}/{c}_sheet.png'); N=sh.width//400; ks=list(range(0,N,12))
out=Image.new('RGB',(8*200,((len(ks)+7)//8)*200),(60,60,64))
for i,k in enumerate(ks):
    f=sh.crop((k*400,0,k*400+400,400)).resize((200,200)); out.paste(f,((i%8)*200,(i//8)*200),f)
out.save(f'review/strip_{c}.png')
