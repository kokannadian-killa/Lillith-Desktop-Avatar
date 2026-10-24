#!/bin/bash
cd /workspace/lillith/wt-secret-oops
while [ $(free -m | awk '/Mem:/{print $7}') -lt 3500 ]; do sleep 20; done
LILLITH_V2=/workspace/lillith/scratch-secret-oops/v2 nice python3 tools/video-pipeline/zoom_tease.py secret_oops_retie --video /workspace/lillith/scratch-secret-oops/secret_oops_retie_v1_a.mp4 --start 1 --stop 482 --step 2 --zmax 2.3 --focus-src 640,200 --zoom 14,26,212,236 --snap --facing front > /workspace/lillith/scratch-secret-oops/log_zt.txt 2>&1; echo "rc=$?" >> /workspace/lillith/scratch-secret-oops/log_zt.txt
