#!/bin/bash
# auto.sh: for each clip, wait for zoom_tease rc=0 in queue.log, then proc.sh + finish.sh (serial)
S=/workspace/lillith/scratch-seasonal-sn; cd $S
for c in summer_popsicle_lick summer_lotion_squeeze newyear_champagne_rim_lick newyear_strawberry_bite newyear_midnight_kiss_squeeze newyear_strap_oops; do
  until grep -q "^$c rc=0" queue.log; do sleep 30; done
  while pgrep -f "finish.sh" -u $(id -u) >/dev/null && [ -n "$(for p in $(pgrep -f '\./finish.sh'); do [ "$(readlink /proc/$p/cwd)" = "$S" ] && echo y; done)" ]; do sleep 30; done
  ./proc.sh $c; ./finish.sh $c
done
