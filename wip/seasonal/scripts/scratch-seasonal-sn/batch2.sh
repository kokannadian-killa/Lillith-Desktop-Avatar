#!/bin/bash
# usage: batch2.sh "<clips to add>" "<msg>" : filtered manifest staged via index; working manifest.json untouched
set -u; WT=/workspace/lillith/wt-seasonal-sn; CL="$1"; MSG="$2"; cd $WT
free=$(df -BG --output=avail / | tail -1 | tr -dc 0-9); [ "$free" -lt 8 ] && { echo "disk ${free}G <8 stop"; exit 3; }
F=assets/generated/.batch_manifest.json
CL="$CL" python3 - <<'PY'
import json,subprocess,os
base={a['name'] for a in json.loads(subprocess.check_output(['git','show','HEAD:assets/generated/manifest.json']))['animations']}
committed={l.split('/')[2] for l in subprocess.check_output(['git','ls-files','assets/generated'],text=True).split() if l.count('/')>=3}
keep=set(os.environ['CL'].split()); m=json.load(open('assets/generated/manifest.json'))
allowed=lambda n: n in keep or (n in base and (n in committed or not n.startswith(('halloween_','xmas_','valentines_','spring_','summer_','newyear_'))) ) or (n in committed)
m['animations']=[a for a in m['animations'] if allowed(a['name'])]
names={a['name'] for a in m['animations']}; assert keep<=names, keep-names
print('manifest clips',len(names),'new in batch',sorted(keep))
open('assets/generated/.batch_manifest.json','w').write(json.dumps(m,indent=1)+'\n')
PY
[ $? -eq 0 ] || exit 4
/workspace/lillith/target-seasonal-sn/debug/import-manifest --input $F --output assets/generated/manifest.toml --tag canonical --check > /workspace/lillith/scratch-seasonal-sn/imp_last.txt 2>&1; rc=$?
grep -E "^wrote|^ok (summer|newyear)_|error|Error" /workspace/lillith/scratch-seasonal-sn/imp_last.txt; echo "import-manifest rc=$rc"
if [ $rc -eq 0 ]; then
  for c in $CL; do git add assets/generated/$c; done; for e in ${EXTRA:-}; do git add "$e"; done
  h=$(git hash-object -w $F); git update-index --cacheinfo 100644,$h,assets/generated/manifest.json
  git add assets/generated/manifest.toml
  git commit -q -m "$MSG" && echo "lfs files in batch: $(git lfs ls-files -n | grep -cE "$(echo $CL | tr ' ' '|')")" && \
  git push origin HEAD:refs/heads/feat/seasonal-summer-newyear && echo "PUSHED $(git rev-parse HEAD)"
fi
rm -f $F
