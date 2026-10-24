#!/bin/bash
S=/workspace/lillith/scratch-seasonal; c=halloween_pumpkin_hug_candy; cd $S
until [ -f $S/pumpkin_done ]; do sleep 30; done
f=proc_$c.txt; [ -f fix_$c.txt ] && f=fix_$c.txt
tail -n3 $f > qa_$c.final
if grep -q FAIL qa_$c.final || ! grep -q "floating specks (>3px from body, a>16) 0," qa_$c.final; then echo "QA NOT PASSED"; exit 2; fi
while pgrep -f "new_pose.py (halloween|xmas)" >/dev/null; do sleep 5; done
python3 tags_seasonal.py $c
python3 strip.py $c
bash batch2.sh "$c" "art: Halloween seasonal clip halloween_pumpkin_hug_candy (completes Halloween set)"
python3 sheet.py halloween halloween_hat_tip_wink halloween_broom_sit_lollipop halloween_pumpkin_hug_candy halloween_cape_swirl_arch
