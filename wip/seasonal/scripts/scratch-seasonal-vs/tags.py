"""tags.py <clip>...: seasonal tags into manifest.json entry + per-clip json (recipe section 5). Kneeling clips get seat fields."""
import json, sys
W='/workspace/lillith/wt-seasonal-vs/assets/generated'; man=f'{W}/manifest.json'
m=json.load(open(man)); by={a['name']:a for a in m['animations']}
for n in sys.argv[1:]:
    sid=n.split('_')[0]
    kv=dict(tags=sorted(set(by[n].get('tags',[])+['allure','baked_motion','cleavage_zoom','seasonal',f'season:{sid}',f'outfit:{sid}'])), allure=True, season=sid, outfit=sid)
    by[n].update(kv); jp=f'{W}/{n}/{n}.json'; j=json.load(open(jp)); j.update(kv); json.dump(j,open(jp,'w'),indent=2)
    print(n, by[n]['tags'])
open(man,'w').write(json.dumps(m,indent=1)+'\n')
