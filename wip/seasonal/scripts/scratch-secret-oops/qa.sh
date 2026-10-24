#!/bin/bash
c=$1; WT=/workspace/lillith/wt-secret-oops; S=/workspace/lillith/scratch-secret-oops; cd $WT
[ "$2" = "mc" ] && CC_REPO=$WT MC_BACKUP=$S/mc_orig MC_JOBS=1 nice python3 tools/matte-clean/matte_clean.py assets/generated/$c 2>&1 | tail -n 1
python3 tools/video-pipeline/qa_all.py --white-edge --mp4 assets/generated/$c 2>&1 | grep -v "^white-edge:" | tail -n 3
FQA_ALL=1 python3 /workspace/lillith/scratch-fringeqa/fringeqa.py secretoops $c assets/generated/$c 2>&1 | python3 -c "
import sys,json
for l in sys.stdin:
    l=l.strip()
    if l.startswith('{'):
        r=json.loads(l); print('fringeqa',r['clip'],r.get('verdict'),'cleaned',r.get('cleaned'),'frames',r.get('frames'),'max white',r.get('max_white_any_frame'),'rim',r.get('max_light_rim_any_frame'),'pocket',r.get('max_pocket_any_frame'),'worst',r.get('worst_frame'))
    elif l: print(l)"
python3 $S/popcheck.py $c
