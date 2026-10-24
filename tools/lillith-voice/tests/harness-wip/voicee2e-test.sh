#!/usr/bin/env bash
R=/workspace/lillith/scratch/vdt-tmp/results/main-3156237; WT=/workspace/lillith/wt-voicee2e
ncargo() { ps -eo args | grep -cE '^[^ ]*cargo (build|test|check|clippy|run)'; }
while [ "$(ncargo)" -ge 2 ]; do echo "$(date +%T) $(ncargo) cargo running, waiting" >> $R/test-wait.log; sleep 30; done
echo "$(date +%T) starting test" >> $R/test-wait.log
cd $WT && env HOME=/home/box CARGO_TARGET_DIR=/workspace/lillith/target-voicee2e CARGO_INCREMENTAL=0 \
  CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_PROFILE_TEST_DEBUG=line-tables-only \
  RUSTFLAGS="--remap-path-prefix=$WT=wt-voicee2e" cargo test --locked --offline -j 2 -p lillith-voice -p lillith-panel -p lillith-overlay -p lillith-core -p lillith > $R/test.log 2>&1
echo "$(date +%T) test rc=$?" >> $R/test-wait.log
