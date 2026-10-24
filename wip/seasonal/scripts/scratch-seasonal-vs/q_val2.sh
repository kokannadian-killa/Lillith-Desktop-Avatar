S=/workspace/lillith/scratch-seasonal-vs
while ! grep -q "Q_VAL1 DONE" $S/queue.log 2>/dev/null; do sleep 20; done
$S/build.sh valentines_strap_oops s1 s2 --focus-src 656,225 --snap --facing "three-quarter front"
echo Q_VAL2 DONE >> $S/queue.log
