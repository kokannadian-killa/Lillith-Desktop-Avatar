#!/usr/bin/env bash
# waits until <2 cargo builds run, then builds lillith at wt-voicee2e into target-voicee2e
R=/workspace/lillith/scratch/vdt-tmp/results/main-3156237
WT=/workspace/lillith/wt-voicee2e
ncargo() { ps -eo args | grep -cE '^[^ ]*cargo (build|test|check|clippy|run)'; }
while [ "$(ncargo)" -ge 2 ]; do echo "$(date +%T) $(ncargo) cargo builds running, waiting" >> $R/build-wait.log; sleep 30; done
echo "$(date +%T) starting build" >> $R/build-wait.log
cd $WT && env HOME=/home/box CARGO_TARGET_DIR=/workspace/lillith/target-voicee2e CARGO_INCREMENTAL=0 \
  CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_PROFILE_TEST_DEBUG=line-tables-only \
  RUSTFLAGS="--remap-path-prefix=$WT=wt-voicee2e" cargo build -p lillith --locked --offline -j 2 > $R/build.log 2>&1
echo "$(date +%T) build rc=$?" >> $R/build-wait.log
