#!/bin/bash
WT=/workspace/lillith/wt-seasonal; cd $WT
while :; do
  n=$(pgrep -f "bin/cargo (build|check|test|run)" | wc -l); free=$(df -BG --output=avail / | tail -1 | tr -dc 0-9)
  [ "$free" -lt 8 ] && { echo "disk ${free}G <8 stop"; exit 3; }
  [ "$n" -lt 2 ] && break; echo "$(date +%T) waiting: $n other cargo builds"; sleep 30
done
export CARGO_TARGET_DIR=/workspace/lillith/target-seasonal CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_PROFILE_TEST_DEBUG=line-tables-only RUSTFLAGS="--remap-path-prefix=$WT=wt-seasonal"
nice cargo build -j 2 -p import-manifest 2>&1 | tail -3 || exit 5

