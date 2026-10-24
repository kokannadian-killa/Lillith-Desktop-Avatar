import json,sys
W='/workspace/lillith/wt-seasonal-sn/assets/generated'
c=sys.argv[1]; s=c.split('_')[0]
T=["allure","baked_motion","cleavage_zoom","seasonal",f"season:{s}",f"outfit:{s}"]
def add(e):
    t=e.get('tags',[]); [t.append(x) for x in T if x not in t]; e['tags']=t; e['season']=s; e['outfit']=s
ind=lambda r: len(r.split('\n')[1])-len(r.split('\n')[1].lstrip())
p=f'{W}/manifest.json'; raw=open(p).read(); m=json.loads(raw)
n=0
for e in m['animations']:
    if e['name']==c: add(e); n+=1
assert n==1,n
open(p,'w').write(json.dumps(m,indent=ind(raw))+'\n')
j=f'{W}/{c}/{c}.json'; r2=open(j).read(); d=json.loads(r2); add(d); open(j,'w').write(json.dumps(d,indent=ind(r2))+'\n')
print(c,'tagged',e['tags'] if False else T)
