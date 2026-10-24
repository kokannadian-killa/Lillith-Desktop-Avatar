#!/bin/bash
S=/workspace/lillith/scratch-seasonal-sn
while pgrep -f q_summer1.sh >/dev/null; do sleep 30; done
$S/zt.sh summer_lotion_squeeze /workspace/lillith/assets/source/video/summer_lotion_squeeze/summer_lotion_squeeze_chain.mp4 --stop 385 --zmax 2.3 --focus-src 650,206 --zoom 14,40,58,84,104,130,148,174 --snap --facing front
