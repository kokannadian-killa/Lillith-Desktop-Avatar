S=/workspace/lillith/scratch-seasonal; c=halloween_pumpkin_hug_candy
bash $S/proc.sh $c
if grep -q FAIL $S/proc_$c.txt || ! grep -q "floating specks (>3px from body, a>16) 0," $S/proc_$c.txt; then bash $S/fixloop.sh $c > $S/fix_$c.txt 2>&1; fi
touch $S/pumpkin_done
