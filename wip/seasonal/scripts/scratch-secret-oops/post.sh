#!/bin/bash
S=/workspace/lillith/scratch-secret-oops; WT=/workspace/lillith/wt-secret-oops; c=secret_oops_retie; cd $WT
until grep -q "^rc=" $S/log_zt.txt; do sleep 15; done
grep -q "^rc=0" $S/log_zt.txt || { echo "zoom_tease failed"; exit 1; }
export LILLITH_V2=$S/v2
python3 - <<'PY'
import json
p='/workspace/lillith/scratch-secret-oops/v2/secret_oops_retie/secret_oops_retie.json'; j=json.load(open(p))
j['loop']=False; j['style']='halloween_micro_witch (seasonal outfit: black/neon-red micro witch bikini, short cape, witch hat, rune trim)'
j['source_files']=['../../source/video/secret_oops_retie/secret_oops_retie_v1_a.mp4']
j['generator']='Grok Imagine image-to-video (GenerateVideo, 720p): take a 15 s from the Halloween outfit still + take b 5 s from take a last frame, concatenated (481 frames @24 fps) + tools/video-pipeline/zoom_tease.py'
j['secret']=True; j['hidden']=True; j['unlock']='see docs/SECRET_POSES.md'
j['tags']=sorted(set(j.get('tags',[])+['secret','easter_egg','cleavage_zoom','season:halloween','outfit:halloween_witch','unlock:konami_or_triple_click_hat']))
json.dump(j,open(p,'w'),indent=1)
PY
mkdir -p assets/source/video/$c && cp -n $S/secret_oops_retie_v1_a.mp4 assets/source/video/$c/
python3 tools/video-pipeline/new_pose.py $c --from copy --to pack --repo $WT --tag secret --tag easter_egg --tag cleavage_zoom --tag season:halloween --tag outfit:halloween_witch --tag unlock:konami_or_triple_click_hat 2>&1 | tail -n 4
CC_REPO=$WT MC_BACKUP=$S/mc_orig MC_JOBS=1 nice python3 tools/matte-clean/matte_clean.py assets/generated/$c 2>&1 | tail -n 2
python3 tools/video-pipeline/qa_all.py --white-edge --mp4 assets/generated/$c 2>&1 | tail -n 6
FQA_ALL=1 python3 /workspace/lillith/scratch-fringeqa/fringeqa.py secretoops $c assets/generated/$c 2>&1 | tail -n 4
python3 $S/popcheck.py $c
echo POSTDONE
