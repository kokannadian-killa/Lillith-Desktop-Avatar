#!/bin/bash
c=$1; S=/workspace/lillith/scratch-seasonal
for r in 1 2 3; do
  nice python3 $S/fixall.py $c > /dev/null
  out=$($S/qa.sh $c mc); echo "round $r"; echo "$out" | grep -E "white-edge|fringeqa|centroid"
  echo "$out" | grep -qE "FAIL" || { echo "$out" | grep -q "floating specks (>3px from body, a>16) 0," && break; }
done
