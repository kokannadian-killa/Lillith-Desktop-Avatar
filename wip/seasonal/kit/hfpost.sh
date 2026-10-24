#!/bin/bash
# hfpost.sh clip: holefill then QA without matte-clean
S=/workspace/lillith/scratch-seasonal; c=$1
nice python3 $S/holefill.py $c 80 | cut -c1-200
$S/qa.sh $c 2>&1 | tail -n 3
