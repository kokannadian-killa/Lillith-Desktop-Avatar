#!/bin/bash
# finish.sh clip [nofix]: serial (flock); waits for my proc.sh jobs and this clip's fixloop, then fixloop if FAIL, holefill + QA
S=/workspace/lillith/scratch-seasonal-sn; c=$1
busy(){ pgrep -f "fixloop.sh $c" >/dev/null && return 0; for p in $(pgrep -f "proc.sh"); do [ "$(readlink /proc/$p/cwd)" = "/workspace/lillith/wt-seasonal-sn" ] && return 0; done; return 1; }
exec 9>$S/finish.lock; flock 9
while busy; do sleep 20; done
grep -q "FAIL" $S/proc_$c.txt && [ "$2" != "nofix" ] && $S/fixloop.sh $c > $S/fix_$c.txt 2>&1
$S/post.sh $c > $S/post_$c.txt 2>&1
python3 $S/speckinfo.py $c > $S/speck_$c.txt 2>&1
echo "$c finished" >> $S/proc.log
