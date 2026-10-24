#!/bin/bash
# fix/tease-holes: holefill the 10 pushed tease/squeeze clips off origin/main
S=/workspace/lillith/scratch-seasonal; W=/workspace/lillith/wt-teasefix; cd $S
while pgrep -f "bash hfpush.sh" >/dev/null; do sleep 30; done
L="tease_arms_crossed_lift tease_hipsway_chest_press tease_side_lie_cleavage tease_shoulder_glance_arch squeeze_lean_forward_arms squeeze_hands_press_tease squeeze_bent_over_cleavage squeeze_seated_lip_bite squeeze_kneel_press_wink squeeze_chest_push_shimmy"
cd /workspace/lillith/wt-seasonal && git fetch -q origin main
[ -d $W ] || GIT_LFS_SKIP_SMUDGE=1 git worktree add -b fix/tease-holes $W origin/main || exit 3
cd $W; inc=$(for c in $L; do printf "assets/generated/%s/**," $c; done); git lfs pull --include="${inc%,}" || exit 4
cd $S; ok=1
for c in $L; do HF_WT=$W HF_REV=$S/review/tease_ nice python3 holefill.py $c 80 | cut -c1-150 > tf_$c.txt; QA_WT=$W bash qa.sh $c >> tf_$c.txt 2>&1
  t=$(tail -n3 tf_$c.txt); echo "$t" | grep -q FAIL && ok=0; echo "$t" | grep -q "floating specks (>3px from body, a>16) 0," || ok=0; echo "$c ok=$ok"; done
[ $ok = 1 ] || { echo "QA NOT PASSED"; exit 2; }
cd $W; /workspace/lillith/target-seasonal/debug/import-manifest --input assets/generated/manifest.json --output assets/generated/manifest.toml --tag canonical --check > $S/tf_imp.txt 2>&1 || { echo "IMPORT FAIL"; exit 6; }; echo import-manifest ok
git add assets/generated/manifest.toml assets/generated/manifest.json $(for c in $L; do echo assets/generated/$c; done)
git diff --cached --diff-filter=D --quiet || { echo "DELETIONS STAGED"; exit 5; }
git commit -qm "fix: fill enclosed specular holes punched by matte-clean in tease_/squeeze_ clips (holefill)" && git push -u origin fix/tease-holes 2>&1 | tail -n 2; git rev-parse HEAD
