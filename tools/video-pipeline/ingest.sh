#!/bin/bash
# ingest.sh <clip> <variant...> : copy a downloaded I2V take into $LILLITH_SRC_VIDEO/<clip>/, extract frames,
# write a contact sheet and list loop candidates.
# LILLITH_RAW_IN (required): folder holding the freshly generated <clip>_v3_<variant>.mp4 files.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
A=${LILLITH_RAW_IN:?set LILLITH_RAW_IN to the folder with the new mp4s}
S=${LILLITH_SRC_VIDEO:-/workspace/lillith/assets/source/video}
mkdir -p /tmp/vid
c=$1; shift
for v in "$@"; do f=${c}_v3_$v; mkdir -p $S/$c; cp -n $A/$f.mp4 $S/$c/ ; d=/tmp/lillith_frames/$f; mkdir -p $d
  [ -f $d/f_001.png ] || ffmpeg -nostdin -v error -y -protocol_whitelist file,pipe -i $S/$c/$f.mp4 $d/f_%03d.png
  ffmpeg -nostdin -v error -y -protocol_whitelist file,pipe -i $S/$c/$f.mp4 -vf "fps=4,scale=200:-1,tile=8x2" -frames:v 1 /tmp/vid/${f}_c.png
  python3 "$HERE/find_loops.py" $d ${MINL:-12} ${MAXL:-48} 3
done
