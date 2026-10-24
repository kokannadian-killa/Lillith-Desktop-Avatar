#!/usr/bin/env bash
# JVM unit tests for the Android voice runtime (no device, no Gradle, no network):
#   tokenizer, textnorm, clean/split, safeword, polish DSP and the stop/drop speech queue,
#   against fixtures generated from the desktop Python kit (the reference implementation).
#
#   JAVA_HOME=... PYTHON=/path/to/python-with-numpy+sentencepiece \
#   LILLITH_VOICE_KIT=tools/lillith-voice  android/voice-tests.sh
#
# Optional smoke test (real ONNX synthesis on the JVM, compared with Python at temperature 0):
#   ORT_JVM_JAR=onnxruntime-1.31.0.jar LILLITH_VOICE_MODELS=.../models android/voice-tests.sh --smoke
set -euo pipefail
cd "$(dirname "$0")"
ROOT=$(cd .. && pwd)
JAVA_HOME=${JAVA_HOME:-}
JAVAC=${JAVA_HOME:+$JAVA_HOME/bin/}javac
JAVA=${JAVA_HOME:+$JAVA_HOME/bin/}java
PY=${PYTHON:-python3}
KIT=${LILLITH_VOICE_KIT:-$ROOT/tools/lillith-voice}
MODELS=${LILLITH_VOICE_MODELS:-$KIT/models}
[ -f "$MODELS/pocket/tokenizer.model" ] || { echo "need $MODELS/pocket/tokenizer.model (LILLITH_VOICE_MODELS)"; exit 2; }
KITLIB=$KIT
[ -d "$KITLIB/lillith_voice" ] || { echo "need the kit's lillith_voice package in $KIT"; exit 2; }
ORT_CP=${ORT_CLASSES:-}
if [ -z "$ORT_CP" ]; then
  for c in "${ORT_JVM_JAR:-}" "$ROOT/../android-toolchain/ort/classes.jar"; do [ -n "$c" ] && [ -f "$c" ] && { ORT_CP=$c; break; }; done
fi
[ -n "$ORT_CP" ] || { echo "need ORT classes (ORT_JVM_JAR or ORT_CLASSES) to compile PocketTts"; exit 2; }
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
mkdir -p "$W/fx" "$W/cls"
# fixtures from the Python reference; kit models dir may live outside the kit
ln -s "$MODELS" "$W/models" 2>/dev/null || true
mkdir -p "$W/kit" && ln -s "$KITLIB/lillith_voice" "$W/kit/lillith_voice" && ln -s "$MODELS" "$W/kit/models"
"$PY" app/test/gen_voice_fixtures.py "$W/kit" "$W/fx" >/dev/null
"$JAVAC" --release 8 -Xlint:-options -encoding UTF-8 -d "$W/cls" -cp "$ORT_CP" \
  app/src/com/kokannadian/lillith/voice/*.java app/test/java/com/kokannadian/lillith/voice/*.java
"$JAVA" -cp "$W/cls:$ORT_CP" com.kokannadian.lillith.voice.VoiceTests "$W/fx" "$MODELS/pocket/tokenizer.model"
if [ "${1:-}" = "--smoke" ]; then
  "$JAVA" -Xmx3g -cp "$W/cls:$ORT_CP" com.kokannadian.lillith.voice.SmokeTest "$MODELS/pocket" "$W/java.f32" "${SMOKE_TEXT:-Hello there, Ko. Mmm, I missed you!}"
  "$PY" app/test/smoke_compare.py "$W/kit" "$W/java.f32" "${SMOKE_TEXT:-Hello there, Ko. Mmm, I missed you!}"
fi
