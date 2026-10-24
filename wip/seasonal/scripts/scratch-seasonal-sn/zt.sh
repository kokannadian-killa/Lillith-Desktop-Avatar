#!/bin/bash
# zt.sh <clip> <video> [zoom_tease args...]
c=$1; v=$2; shift 2; S=/workspace/lillith/scratch-seasonal-sn; cd /workspace/lillith/wt-seasonal-sn
while [ $(free -m | awk '/Mem:/{print $7}') -lt 2500 ]; do sleep 20; done
nice python3 tools/video-pipeline/zoom_tease.py $c --video $v "$@" > $S/log_$c.txt 2>&1; echo "$c rc=$?" >> $S/queue.log
