"""tags_seasonal.py <clip>...: add seasonal tags/fields to manifest.json entries + per-clip json."""
import json,sys
W='/workspace/lillith/wt-seasonal/assets/generated'; man=f'{W}/manifest.json'
m=json.load(open(man)); by={a['name']:a for a in m['animations']}
for n in sys.argv[1:]:
    sid=n.split('_')[0]; kv=dict(season=sid,outfit=sid,allure=True)
    tags=sorted(set(by[n].get('tags',[])+['allure','baked_motion','cleavage_zoom','seasonal',f'season:{sid}',f'outfit:{sid}']))
    by[n].update(kv,tags=tags); jp=f'{W}/{n}/{n}.json'; j=json.load(open(jp)); j.update(kv,tags=tags); json.dump(j,open(jp,'w'),indent=2)
    print(n,tags)
open(man,'w').write(json.dumps(m,indent=1)+'\n')
