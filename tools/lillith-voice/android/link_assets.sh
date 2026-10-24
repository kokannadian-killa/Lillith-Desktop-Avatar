#!/usr/bin/env bash
# Symlink (never copy) the voice models into an Android assets dir for a local build.
# usage: link_assets.sh ASSETS_DIR [quality|small] [--no-stt]   (small = int8 transformer: experimental, see ANDROID_ASSETS.md)
# The build tool must follow symlinks (Gradle does when packaging assets; android/build-apk.sh
# turns them into hard links/copies inside its stage dir).
# Env: LILLITH_VOICE_MODELS  models dir to link from [<this kit>/models]
#      LINK_KOKORO=1         also link the Kokoro p1 fallback (kokoro-v1.0.onnx + voices-v1.0.bin)
#                            into voice/kokoro/ (+353 MB; needs an espeak-ng G2P port to be usable).
set -euo pipefail
DST="${1:?assets dir}"; FLAVOR="${2:-quality}"; STT=1; [ "${3:-}" = "--no-stt" ] && STT=0
SRC="$(cd "${LILLITH_VOICE_MODELS:-$(dirname "$0")/../models}" && pwd)"
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
if [ "${LINK_KOKORO:-0}" = 1 ]; then
  mkdir -p "$DST/voice/kokoro"
  for f in kokoro-v1.0.onnx voices-v1.0.bin; do ln -sfn "$SRC/$f" "$DST/voice/kokoro/$f"; done
fi
echo "linked ($FLAVOR, stt=$STT, kokoro=${LINK_KOKORO:-0}) into $DST/voice"
