#!/bin/bash
# finish.sh clip: fixloop if needed, then holefill + qa (post.sh). Writes fin_<clip>.txt, appends "<clip> FIN" to queue.log
S=/workspace/lillith/scratch-seasonal-vs; c=$1
if grep -qE "FAIL" $S/proc_$c.txt || ! grep -q "floating specks (>3px from body, a>16) 0," $S/proc_$c.txt; then
  $S/fixloop.sh $c > $S/fix_$c.txt 2>&1
fi
$S/post.sh $c > $S/fin_$c.txt 2>&1
echo "$c FIN" >> $S/queue.log
