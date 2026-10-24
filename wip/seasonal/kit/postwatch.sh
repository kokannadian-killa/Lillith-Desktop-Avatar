#!/bin/bash
# sequentially post-process clips as zoom_tease finishes them (new_pose rewrites manifest.json: never parallel)
S=/workspace/lillith/scratch-seasonal; touch $S/queue.log $S/post_done.log
while [ ! -f $S/STOPPOST ]; do
  for c in $(grep " rc=0" $S/queue.log | awk '{print $1}'); do
    grep -qx "$c" $S/post_done.log && continue
    bash $S/proc.sh $c
    if grep -q FAIL $S/proc_$c.txt || ! grep -q "floating specks (>3px from body, a>16) 0," $S/proc_$c.txt; then bash $S/fixloop.sh $c > $S/fix_$c.txt 2>&1; fi
    echo "$c" >> $S/post_done.log
  done
  sleep 60
done
