#!/usr/bin/env bash
# Builds the Lillith debug APK (arm64-v8a) without Gradle:
#   cargo (Rust engine .so) + CMake/NDK (llama.cpp shim .so)
#   + clips + voice models (assets/, stored uncompressed)
#   + javac/d8 (Java shell) + aapt2 (manifest/resources) + zipalign/apksigner.
#
# Usage: android/build-apk.sh            (from anywhere in the repo)
# Env (defaults in brackets):
#   ANDROID_HOME     Android SDK with platforms;android-34, build-tools;34.0.0,
#                    ndk;27.2.12479018, cmake;3.22.1      [$ANDROID_SDK_ROOT]
#   JAVA_HOME        JDK 17                               [from PATH]
#   LLAMA_CPP_DIR    llama.cpp checkout (tag below). If unset, it is cloned
#                    once into $OUT/llama.cpp (build-time only; the app
#                    itself never touches the network).
#   CARGO_TARGET_DIR Rust target dir                      [<repo>/target]
#   OUT              build output dir                     [$CARGO_TARGET_DIR/apk]
#   (needs python3 for zip packaging)
#   CARGO            cargo with the aarch64-linux-android std installed
#                    (rustup target add aarch64-linux-android)   [cargo]
# Voice (Ko's cloned voice + offline STT, from the lillith-voice kit; see
# tools/lillith-voice/android/ANDROID_ASSETS.md). Everything ships inside the
# APK under assets/voice/; nothing is downloaded at runtime.
#   LILLITH_VOICE         1 = bundle the voice models (required), 0 = build without voice  [1]
#   LILLITH_VOICE_MODELS  unpacked kit models dir (the release tarball's
#                         tools/lillith-voice/models)  [first of <repo>/tools/lillith-voice/models,
#                         ~/.local/share/lillith-voice/models that has SHA256SUMS]
#   LILLITH_VOICE_FLAVOR  quality = fp32 flow_lm_main (recommended, ANDROID_ASSETS.md)
#                         small   = int8 flow_lm_main (experimental: can cut lines short)  [quality]
#   LILLITH_VOICE_STT     0 = speech output only (Ko, 2026-10-24: no mic on Android for now)
#                         1 = opt-in: also ship whisper tiny.en + silero VAD (+~100 MiB) for a
#                         future mic feature (the app has no RECORD_AUDIO / STT runtime yet)   [0]
#   LILLITH_VOICE_KOKORO  1 = also ship the Kokoro p1 fallback (+353 MB; no Android
#                         runtime or espeak-ng G2P for it yet)   [0]
#   STAGE_ONLY=1          only stage assets (clips + voice) into $OUT/stage and list
#                         them; needs no SDK/NDK/cargo
set -euo pipefail

LLAMA_CPP_TAG=b11829
API=26
TARGET_SDK=34
BUILD_TOOLS=34.0.0
NDK_VER=27.2.12479018
CMAKE_VER=3.22.1

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
STAGE_ONLY="${STAGE_ONLY:-0}"
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
[ -n "$SDK" ] || [ "$STAGE_ONLY" = 1 ] || { echo "set ANDROID_HOME to your Android SDK" >&2; exit 1; }
NDK="${ANDROID_NDK_HOME:-$SDK/ndk/$NDK_VER}"
BT="$SDK/build-tools/$BUILD_TOOLS"
PLATFORM_JAR="$SDK/platforms/android-$TARGET_SDK/android.jar"
CMAKE_BIN="$SDK/cmake/$CMAKE_VER/bin"
CARGO="${CARGO:-cargo}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$REPO/target}"
OUT="${OUT:-$CARGO_TARGET_DIR/apk}"
JAVAC="${JAVA_HOME:+$JAVA_HOME/bin/}javac"
KEYTOOL="${JAVA_HOME:+$JAVA_HOME/bin/}keytool"
LLVM="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin"
# d8/apksigner are shell wrappers that exec `java` from PATH.
[ -n "${JAVA_HOME:-}" ] && export PATH="$JAVA_HOME/bin:$PATH"

if [ "$STAGE_ONLY" != 1 ]; then
  for f in "$NDK" "$BT/aapt2" "$BT/d8" "$BT/zipalign" "$BT/apksigner" "$PLATFORM_JAR" "$CMAKE_BIN/cmake"; do
    [ -e "$f" ] || { echo "missing: $f (see android/INSTALL-android.md)" >&2; exit 1; }
  done
fi

# ── voice: locate + verify the kit models before any long build step ──
VOICE="${LILLITH_VOICE:-1}"
VOICE_FLAVOR="${LILLITH_VOICE_FLAVOR:-quality}"
VOICE_STT="${LILLITH_VOICE_STT:-0}"   # speaking only by default; 1 = opt-in STT models
VOICE_KOKORO="${LILLITH_VOICE_KOKORO:-0}"
LINK_ASSETS="$REPO/tools/lillith-voice/android/link_assets.sh"
VOICE_MODELS=""
if [ "$VOICE" = 1 ]; then
  case "$VOICE_FLAVOR" in quality|small) ;; *) echo "LILLITH_VOICE_FLAVOR must be quality or small" >&2; exit 1;; esac
  for d in "${LILLITH_VOICE_MODELS:-}" "$REPO/tools/lillith-voice/models" "$HOME/.local/share/lillith-voice/models"; do
    if [ -n "$d" ] && [ -f "$d/SHA256SUMS" ]; then VOICE_MODELS="$(cd "$d" && pwd)"; break; fi
  done
  if [ -z "$VOICE_MODELS" ]; then
    echo "voice models not found. Unpack the voice kit (lillith-voice-kit-*.tar.xz) and set" >&2
    echo "LILLITH_VOICE_MODELS=<kit>/tools/lillith-voice/models, or LILLITH_VOICE=0 to build without voice." >&2
    exit 1
  fi
  # Exact file list (relative to models/): ANDROID_ASSETS.md sections 1 + 2.
  VOICE_FILES="pocket/text_conditioner.onnx pocket/flow_lm_flow.onnx pocket/mimi_decoder.onnx
    pocket/tokenizer.model pocket/bundle.json pocket/bos_before_voice.npy pocket/voices/lillith.npy"
  if [ "$VOICE_FLAVOR" = quality ]; then VOICE_FILES="$VOICE_FILES pocket/flow_lm_main.onnx"
  else VOICE_FILES="$VOICE_FILES pocket/flow_lm_main_int8.onnx"; fi
  [ "$VOICE_STT" = 1 ] && VOICE_FILES="$VOICE_FILES stt/silero_vad.onnx stt/tiny.en-encoder.int8.onnx
    stt/tiny.en-decoder.int8.onnx stt/tiny.en-tokens.txt"
  [ "$VOICE_KOKORO" = 1 ] && VOICE_FILES="$VOICE_FILES kokoro-v1.0.onnx voices-v1.0.bin"
  echo "==> voice: verifying $(echo $VOICE_FILES | wc -w) models in $VOICE_MODELS ($VOICE_FLAVOR, stt=$VOICE_STT, kokoro=$VOICE_KOKORO)"
  ( cd "$VOICE_MODELS"
    for f in $VOICE_FILES; do
      [ -f "$f" ] || { echo "voice model missing: $VOICE_MODELS/$f" >&2; exit 1; }
      grep -q "  $f\$" SHA256SUMS || { echo "no SHA256SUMS entry for $f" >&2; exit 1; }
    done
    for f in $VOICE_FILES; do grep "  $f\$" SHA256SUMS; done | sha256sum -c --quiet - ) ||
    { echo "voice models failed verification (see above)" >&2; exit 1; }
fi

mkdir -p "$OUT"
STAGE="$OUT/stage"
rm -rf "$STAGE"
mkdir -p "$STAGE/lib/arm64-v8a" "$STAGE/assets/generated" "$OUT/classes" "$OUT/dex"

# Clips + voice into $STAGE/assets (steps 3 + 4).
stage_assets() {
echo "==> 3/7 clips (assets/generated: manifest.toml + sheets, art unchanged)"
cp "$REPO/assets/generated/manifest.toml" "$STAGE/assets/generated/"
( cd "$REPO/assets/generated" && find . -path '*/frames' -prune -o -type f -name '*.png' -print ) |
  while read -r f; do
    mkdir -p "$STAGE/assets/generated/$(dirname "$f")"
    cp "$REPO/assets/generated/$f" "$STAGE/assets/generated/$f"
  done

if [ "$VOICE" != 1 ]; then
  echo "==> 4/7 voice: skipped (LILLITH_VOICE=0): the app will log that it has no voice models"
  return
fi
local stt_flag="" stt_what=", STT (opt-in)"; [ "$VOICE_STT" = 1 ] || { stt_flag="--no-stt"; stt_what=", speaking only"; }
echo "==> 4/7 voice (assets/voice: cloned 'lillith' Pocket TTS$stt_what; stored uncompressed)"
LILLITH_VOICE_MODELS="$VOICE_MODELS" LINK_KOKORO="$VOICE_KOKORO" \
  bash "$LINK_ASSETS" "$STAGE/assets" "$VOICE_FLAVOR" $stt_flag
[ "$VOICE_STT" = 1 ] || rmdir "$STAGE/assets/voice/stt" 2>/dev/null || true   # no empty stt/ in the APK
# link_assets.sh makes symlinks; turn them into hard links (no extra disk) or
# copies, so aapt2 packages real files whatever it does with links.
find "$STAGE/assets/voice" -type l | while read -r l; do
  t="$(readlink -f "$l")"; rm "$l"; ln "$t" "$l" 2>/dev/null || cp "$t" "$l"
done
# SHA256SUMS (paths relative to assets/voice/) drives AssetInstaller's
# first-run copy + check. Kokoro files live under voice/kokoro/.
( cd "$VOICE_MODELS"
  for f in $VOICE_FILES; do
    case "$f" in pocket/*|stt/*) dst="$f";; *) dst="kokoro/$f";; esac
    printf '%s  %s\n' "$(grep "  $f\$" SHA256SUMS | cut -d' ' -f1)" "$dst"
  done ) > "$STAGE/assets/voice/SHA256SUMS"
cat > "$STAGE/assets/voice/voice.properties" <<EOF
# written by android/build-apk.sh
flavor=$VOICE_FLAVOR
cloned_voice=lillith
stt=$VOICE_STT
kokoro_p1=$VOICE_KOKORO
fallback=$([ "$VOICE_KOKORO" = 1 ] && echo kokoro-p1 || echo android-tts)
EOF
( cd "$STAGE/assets/voice" && sha256sum -c --quiet SHA256SUMS ) ||
  { echo "staged voice assets don't match SHA256SUMS" >&2; exit 1; }
[ "$(find "$STAGE/assets/voice" -type f ! -name SHA256SUMS ! -name voice.properties | wc -l)" = \
  "$(wc -l < "$STAGE/assets/voice/SHA256SUMS")" ] || { echo "stray files in assets/voice" >&2; exit 1; }
echo "    voice: $(wc -l < "$STAGE/assets/voice/SHA256SUMS") files, $(du -shL "$STAGE/assets/voice" | cut -f1)"
}

if [ "$STAGE_ONLY" = 1 ]; then
  stage_assets
  echo "==> staged (STAGE_ONLY=1): $STAGE/assets"
  [ -d "$STAGE/assets/voice" ] && ( cd "$STAGE/assets" && find voice -type f -printf '%10s  %p\n' | sort -k2 )
  echo "clips: $(find "$STAGE/assets/generated" -type f | wc -l) files, $(du -sh "$STAGE/assets/generated" | cut -f1)"
  exit 0
fi

echo "==> 1/7 Rust engine (liblillith_android.so)"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$LLVM/aarch64-linux-android$API-clang"
"$CARGO" build --locked --release -p lillith-android --target aarch64-linux-android \
  --manifest-path "$REPO/Cargo.toml"
cp "$CARGO_TARGET_DIR/aarch64-linux-android/release/liblillith_android.so" "$STAGE/lib/arm64-v8a/"
"$LLVM/llvm-strip" --strip-unneeded "$STAGE/lib/arm64-v8a/liblillith_android.so"

echo "==> 2/7 llama.cpp shim (liblillith_llm.so, CPU, on-device)"
LLAMA_CPP_DIR="${LLAMA_CPP_DIR:-$OUT/llama.cpp}"
if [ ! -f "$LLAMA_CPP_DIR/include/llama.h" ]; then
  git clone --depth 1 --branch "$LLAMA_CPP_TAG" https://github.com/ggml-org/llama.cpp "$LLAMA_CPP_DIR"
fi
"$CMAKE_BIN/cmake" -S "$HERE/llm" -B "$OUT/build-llm" -G Ninja \
  -DCMAKE_MAKE_PROGRAM="$CMAKE_BIN/ninja" \
  -DCMAKE_TOOLCHAIN_FILE="$NDK/build/cmake/android.toolchain.cmake" \
  -DANDROID_ABI=arm64-v8a -DANDROID_PLATFORM="android-$API" \
  -DCMAKE_BUILD_TYPE=Release -DLLAMA_CPP_DIR="$LLAMA_CPP_DIR" >/dev/null
"$CMAKE_BIN/ninja" -j "${NINJA_JOBS:-2}" -C "$OUT/build-llm" lillith_llm
cp "$OUT/build-llm/liblillith_llm.so" "$STAGE/lib/arm64-v8a/"
"$LLVM/llvm-strip" --strip-unneeded "$STAGE/lib/arm64-v8a/liblillith_llm.so"

stage_assets

echo "==> 5/7 Java shell (javac + d8)"
rm -rf "$OUT/classes" "$OUT/dex" && mkdir -p "$OUT/classes" "$OUT/dex"
find "$HERE/app/src" -name '*.java' > "$OUT/sources.txt"
# --release 8 gives javac the JDK's java.* (android.jar lacks the
# LambdaMetafactory javac needs); d8 desugars lambdas for API 26.
"$JAVAC" --release 8 -Xlint:-options -encoding UTF-8 -classpath "$PLATFORM_JAR" \
  -d "$OUT/classes" @"$OUT/sources.txt"
"$BT/d8" --release --min-api $API --lib "$PLATFORM_JAR" --output "$OUT/dex" \
  $(find "$OUT/classes" -name '*.class')

echo "==> 6/7 package (aapt2)"
rm -rf "$OUT/res-compiled" && mkdir -p "$OUT/res-compiled"
"$BT/aapt2" compile --dir "$HERE/app/res" -o "$OUT/res-compiled/res.zip"
"$BT/aapt2" link -o "$OUT/unsigned.apk" -I "$PLATFORM_JAR" \
  --manifest "$HERE/app/AndroidManifest.xml" \
  --min-sdk-version $API --target-sdk-version $TARGET_SDK \
  -A "$STAGE/assets" -0 png -0 toml -0 onnx -0 bin -0 npy -0 model \
  "$OUT/res-compiled/res.zip"
# Add classes.dex and lib/ (python3 zipfile: no dependency on `zip`).
python3 - "$OUT/unsigned.apk" "$OUT/dex/classes.dex" "$STAGE" <<'PY'
import os, sys, zipfile
apk, dex, stage = sys.argv[1:4]
with zipfile.ZipFile(apk, "a", zipfile.ZIP_DEFLATED) as z:
    z.write(dex, "classes.dex")
    for root, _, files in os.walk(os.path.join(stage, "lib")):
        for f in files:
            full = os.path.join(root, f)
            z.write(full, os.path.relpath(full, stage))
PY

echo "==> 7/7 align + sign (debug key)"
KS="${DEBUG_KEYSTORE:-$OUT/debug.keystore}"
if [ ! -f "$KS" ]; then
  "$KEYTOOL" -genkeypair -keystore "$KS" -storepass android -keypass android \
    -alias androiddebugkey -keyalg RSA -keysize 2048 -validity 10000 \
    -dname "CN=Android Debug,O=Android,C=US" >/dev/null 2>&1
fi
"$BT/zipalign" -p -f 4 "$OUT/unsigned.apk" "$OUT/aligned.apk"
"$BT/apksigner" sign --ks "$KS" --ks-pass pass:android --key-pass pass:android \
  --out "$OUT/lillith-debug-arm64.apk" "$OUT/aligned.apk"
"$BT/apksigner" verify "$OUT/lillith-debug-arm64.apk"
rm -f "$OUT/unsigned.apk" "$OUT/aligned.apk" "$OUT/lillith-debug-arm64.apk.idsig"
ls -lh "$OUT/lillith-debug-arm64.apk"
echo "APK: $OUT/lillith-debug-arm64.apk"
