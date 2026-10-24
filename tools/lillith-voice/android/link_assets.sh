#!/usr/bin/env bash
# Symlink (never copy) the voice models into an Android assets dir for a local build.
# usage: link_assets.sh ASSETS_DIR [quality|small] [--no-stt]   (small = int8 transformer: experimental, see ANDROID_ASSETS.md)
# The build tool must follow symlinks (Gradle does when packaging assets).
set -euo pipefail
DST="${1:?assets dir}"; FLAVOR="${2:-quality}"; STT=1; [ "${3:-}" = "--no-stt" ] && STT=0
SRC="$(cd "$(dirname "$0")/../models" && pwd)"
mkdir -p "$DST/voice/pocket/voices" "$DST/voice/stt"
for f in text_conditioner.onnx flow_lm_flow.onnx mimi_decoder.onnx tokenizer.model bundle.json bos_before_voice.npy voices/lillith.npy; do
  ln -sfn "$SRC/pocket/$f" "$DST/voice/pocket/$f"
done
if [ "$FLAVOR" = quality ]; then ln -sfn "$SRC/pocket/flow_lm_main.onnx" "$DST/voice/pocket/flow_lm_main.onnx"
else ln -sfn "$SRC/pocket/flow_lm_main_int8.onnx" "$DST/voice/pocket/flow_lm_main_int8.onnx"; fi
if [ "$STT" = 1 ]; then
  for f in silero_vad.onnx tiny.en-encoder.int8.onnx tiny.en-decoder.int8.onnx tiny.en-tokens.txt; do
    ln -sfn "$SRC/stt/$f" "$DST/voice/stt/$f"
  done
fi
echo "linked ($FLAVOR, stt=$STT) into $DST/voice"
