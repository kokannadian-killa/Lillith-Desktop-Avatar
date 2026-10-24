S=/workspace/lillith/scratch-seasonal-vs
for c in valentines_kneel_chocolate valentines_strap_oops valentines_heart_squeeze spring_lollipop_lick spring_petal_squeeze spring_kneel_berries spring_vine_oops; do
  ( while ! { grep -q "rows>=393" $S/proc_$c.txt 2>/dev/null && ! pgrep -f "proc.sh $c" >/dev/null; }; do sleep 30; done; $S/finish.sh $c ) &
done; wait
