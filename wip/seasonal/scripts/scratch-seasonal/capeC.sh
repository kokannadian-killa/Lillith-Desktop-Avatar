#!/bin/bash
# cape re-take: s1c (candy-corn lick, lip bite, fingertip on lip, chest rub) + s2 (oops catch) -> zoom -> proc -> holefill -> qa -> push
S=/workspace/lillith/scratch-seasonal; c=halloween_cape_swirl_arch; V=/workspace/lillith/assets/source/video/$c; cd $S
ffmpeg -v error -y -i $V/s1c.mp4 -i $V/s2.mp4 -filter_complex "[1:v]trim=start_frame=1,setpts=PTS-STARTPTS[b];[0:v][b]concat=n=2:v=1[v]" -map "[v]" -c:v libx264 -crf 8 -pix_fmt yuv444p $V/chain_c.mp4 || exit 1
rm -rf /tmp/zoom_tease_cache/${c}_a
bash zt.sh $c $V/chain_c.mp4 --stop 384 --zmax 2.3 --zoom 14,40,58,84,104,130,148,174 --focus-src 635,235 --snap --facing front
tail -n1 queue.log | grep -q "$c rc=0" || { echo "ZT FAIL"; exit 2; }
while pgrep -f "new_pose.py" >/dev/null; do sleep 5; done
bash proc.sh $c
if grep -q FAIL proc_$c.txt || ! grep -q "floating specks (>3px from body, a>16) 0," proc_$c.txt; then bash fixloop.sh $c > fix_$c.txt 2>&1; fi
bash hfpost.sh $c > hf_$c.txt 2>&1
t=$(tail -n3 hf_$c.txt); echo "$t"
if echo "$t" | grep -q FAIL; then nice python3 fixall.py $c | tail -n1; bash hfpost.sh $c > hf_$c.txt 2>&1; t=$(tail -n3 hf_$c.txt); echo "$t"; fi
echo "$t" | grep -q FAIL && { echo "QA FAIL"; exit 3; }
echo "$t" | grep -q "floating specks (>3px from body, a>16) 0," || { echo "SPECKS"; exit 3; }
python3 tags_seasonal.py $c; python3 strip.py $c
touch capeC_ready; echo READY_FOR_VISUAL
