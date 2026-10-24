#!/bin/bash
# zt.sh <clip> <video> [zoom_tease args...]  -- one heavy matte (zoom_tease) of mine at a time
c=$1; v=$2; shift 2; S=/workspace/lillith/scratch-seasonal-vs; cd /workspace/lillith/wt-seasonal-vs
exec 9>$S/zt.lock; flock 9
while pgrep -f "zoom_tease.py (valentines|spring)_" >/dev/null; do sleep 30; done
while [ $(free -m | awk '/Mem:/{print $7}') -lt 2500 ]; do sleep 20; done
nice python3 tools/video-pipeline/zoom_tease.py $c --video $v "$@" > $S/log_$c.txt 2>&1; echo "$c rc=$?" >> $S/queue.log
