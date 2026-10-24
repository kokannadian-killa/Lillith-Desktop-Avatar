#!/bin/bash
WT=/workspace/lillith/wt-reprobe; R=/workspace/lillith/scratch/vdt-tmp/results/reprobe-ec9e1c5
mkdir -p $R; cd $WT
export HOME=/home/box CARGO_TARGET_DIR=/workspace/lillith/target-reprobe CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_PROFILE_TEST_DEBUG=line-tables-only RUSTFLAGS="--remap-path-prefix=$WT=wt-reprobe"
waitslot(){ while :; do n=$(ps -eo comm= | grep -cE '^cargo$'); [ "$n" -lt 2 ] && break; echo "$(date +%T) waiting: $n cargo" ; sleep 20; done; }
waitslot; echo "$(date +%T) build start, free $(df -h --output=avail /workspace|tail -1)"
cargo build -p lillith --locked --offline -j 2 > $R/build.log 2>&1; echo "build rc=$?"
mkdir -p /workspace/lillith/scratch/vdt-tmp/bin; cp -f $CARGO_TARGET_DIR/debug/lillith /workspace/lillith/scratch/vdt-tmp/bin/lillith-ec9e1c5 && echo copied
waitslot; echo "$(date +%T) test start"
cargo test --locked --offline -j 2 -p lillith-voice --features voice > $R/test.log 2>&1; echo "test rc=$?"
echo "$(date +%T) DONE"
