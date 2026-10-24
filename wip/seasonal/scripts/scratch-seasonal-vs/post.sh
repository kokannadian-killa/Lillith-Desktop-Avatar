#!/bin/bash
# post.sh clip: holefill (fill enclosed matte holes, inpaint colour) then qa without re-running matte_clean
S=/workspace/lillith/scratch-seasonal-vs; c=$1
nice python3 $S/holefill.py $c 80 | cut -c1-160
$S/qa.sh $c 2>&1 | tail -n 4
