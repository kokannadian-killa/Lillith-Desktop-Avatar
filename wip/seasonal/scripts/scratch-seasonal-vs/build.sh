#!/bin/bash
# build.sh <clip> <seg1> <seg2> [zoom_tease args...] : chain -> zt -> proc -> clean cache
c=$1; s1=$2; s2=$3; shift 3; V=/workspace/lillith/assets/source/video/$c; S=/workspace/lillith/scratch-seasonal-vs
free=$(df -BG --output=avail / | tail -1 | tr -dc 0-9); [ "$free" -lt 8 ] && { echo "$c disk ${free}G <8 stop" >> $S/queue.log; exit 3; }
ffmpeg -v error -nostdin -y -i $V/${c}_$s1.mp4 -i $V/${c}_$s2.mp4 -filter_complex "[1:v]trim=start_frame=1,setpts=PTS-STARTPTS[b];[0:v][b]concat=n=2:v=1[v]" -map "[v]" -c:v libx264 -crf 8 -pix_fmt yuv444p $V/${c}_chain.mp4 || exit 2
$S/zt.sh $c $V/${c}_chain.mp4 --stop 385 --zmax 2.3 --zoom 14,40,58,84,104,130,148,174 "$@"
grep -q "^$c rc=0" $S/queue.log || exit 4
rm -rf /tmp/zoom_tease_cache/${c}_*
$S/proc.sh $c
