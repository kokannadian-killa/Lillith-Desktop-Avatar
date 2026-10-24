S=/workspace/lillith/scratch-seasonal-vs
while ! grep -q "Q_VAL2 DONE" $S/queue.log 2>/dev/null; do sleep 20; done
$S/build.sh valentines_heart_squeeze s2 s1b --focus-src 659,222 --snap --facing front
echo Q_VAL3 DONE >> $S/queue.log
