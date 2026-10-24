#!/bin/bash
S=/workspace/lillith/scratch-seasonal; c=halloween_cape_swirl_arch; cd $S
while pgrep -f "bash capeC.sh" >/dev/null; do sleep 30; done
[ -f capeC_ready ] || { echo "capeC not ready"; tail -5 capeC.log; exit 2; }
while pgrep -f "new_pose.py (halloween|xmas)" >/dev/null; do sleep 5; done
bash batch2.sh "$c" "art: Halloween cape_swirl_arch retake: candy-corn lick, lip bite, fingertip-on-lip tease (holefilled)" 2>&1 | tail -4
python3 sheet.py halloween halloween_hat_tip_wink halloween_broom_sit_lollipop halloween_pumpkin_hug_candy halloween_cape_swirl_arch | tail -1
grep -q PUSHED pushcape.log && { rm -rf /workspace/lillith/assets/generated_v2/$c /tmp/zoom_tease_cache/${c}_a $S/mc_orig/$c*; echo CLEANED; }
df -h / | tail -1
