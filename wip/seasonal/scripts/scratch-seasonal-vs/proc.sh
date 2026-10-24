#!/bin/bash
c=$1; WT=/workspace/lillith/wt-seasonal-vs; S=/workspace/lillith/scratch-seasonal-vs; cd $WT
{ python3 tools/video-pipeline/new_pose.py $c --from copy --to pack --allure --repo $WT 2>&1 | tail -n 3
CC_REPO=$WT MC_BACKUP=$S/mc_orig MC_JOBS=1 nice python3 tools/matte-clean/matte_clean.py assets/generated/$c 2>&1 | tail -n 1
nice python3 $S/fixall.py $c
$S/qa.sh $c mc; } > $S/proc_$c.txt 2>&1
echo "$c processed" >> $S/proc.log
