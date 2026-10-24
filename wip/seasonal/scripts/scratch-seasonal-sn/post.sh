#!/bin/bash
# post.sh clip: holefill (fill enclosed matte holes, inpaint colour), outer-edge light fix, holefill again, qa without matte_clean
S=/workspace/lillith/scratch-seasonal-sn; c=$1
nice python3 $S/holefill.py $c 80 | cut -c1-160
nice python3 $S/edgefix.py $c | cut -c1-160
nice python3 $S/holefill.py $c 80 | cut -c1-160
$S/qa.sh $c 2>&1 | tail -n 4
