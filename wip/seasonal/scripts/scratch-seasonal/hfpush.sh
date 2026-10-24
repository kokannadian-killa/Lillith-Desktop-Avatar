#!/bin/bash
S=/workspace/lillith/scratch-seasonal; cd $S
H="halloween_hat_tip_wink halloween_broom_sit_lollipop halloween_cape_swirl_arch"
for c in $H; do bash hfpost.sh $c > hf_$c.txt 2>&1; done
until [ -f pumpkin_done ]; do sleep 30; done
c=halloween_pumpkin_hug_candy; bash hfpost.sh $c > hf_$c.txt 2>&1
ok=1; for c in $H halloween_pumpkin_hug_candy; do t=$(tail -n3 hf_$c.txt); echo "$t" | grep -q FAIL && ok=0; echo "$t" | grep -q "floating specks (>3px from body, a>16) 0," || ok=0; done
[ $ok = 1 ] || { echo "QA NOT PASSED"; exit 2; }
while pgrep -f "new_pose.py (halloween|xmas)" >/dev/null; do sleep 5; done
python3 tags_seasonal.py halloween_pumpkin_hug_candy
bash batch2.sh "$H halloween_pumpkin_hug_candy" "art: Halloween pumpkin_hug_candy clip; fill punched specular holes in Halloween clips (holefill)"
python3 sheet.py halloween halloween_hat_tip_wink halloween_broom_sit_lollipop halloween_pumpkin_hug_candy halloween_cape_swirl_arch
