#!/usr/bin/env bash
# Builds the Lillith debug APK (arm64-v8a) without Gradle:
#   cargo (Rust engine .so) + CMake/NDK (llama.cpp shim .so)
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
set -euo pipefail

LLAMA_CPP_TAG=b11829
API=26
TARGET_SDK=34
BUILD_TOOLS=34.0.0
NDK_VER=27.2.12479018
CMAKE_VER=3.22.1

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
[ -n "$SDK" ] || { echo "set ANDROID_HOME to your Android SDK" >&2; exit 1; }
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

for f in "$NDK" "$BT/aapt2" "$BT/d8" "$BT/zipalign" "$BT/apksigner" "$PLATFORM_JAR" "$CMAKE_BIN/cmake"; do
  [ -e "$f" ] || { echo "missing: $f (see android/INSTALL-android.md)" >&2; exit 1; }
done

mkdir -p "$OUT"
STAGE="$OUT/stage"
rm -rf "$STAGE"
mkdir -p "$STAGE/lib/arm64-v8a" "$STAGE/assets/generated" "$OUT/classes" "$OUT/dex"

echo "==> 1/6 Rust engine (liblillith_android.so)"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$LLVM/aarch64-linux-android$API-clang"
"$CARGO" build --locked --release -p lillith-android --target aarch64-linux-android \
  --manifest-path "$REPO/Cargo.toml"
cp "$CARGO_TARGET_DIR/aarch64-linux-android/release/liblillith_android.so" "$STAGE/lib/arm64-v8a/"
"$LLVM/llvm-strip" --strip-unneeded "$STAGE/lib/arm64-v8a/liblillith_android.so"

echo "==> 2/6 llama.cpp shim (liblillith_llm.so, CPU, on-device)"
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

echo "==> 3/6 clips (assets/generated: manifest.toml + sheets, art unchanged)"
cp "$REPO/assets/generated/manifest.toml" "$STAGE/assets/generated/"
( cd "$REPO/assets/generated" && find . -path '*/frames' -prune -o -type f -name '*.png' -print ) |
  while read -r f; do
    mkdir -p "$STAGE/assets/generated/$(dirname "$f")"
    cp "$REPO/assets/generated/$f" "$STAGE/assets/generated/$f"
  done

echo "==> 4/6 Java shell (javac + d8)"
rm -rf "$OUT/classes" "$OUT/dex" && mkdir -p "$OUT/classes" "$OUT/dex"
find "$HERE/app/src" -name '*.java' > "$OUT/sources.txt"
# --release 8 gives javac the JDK's java.* (android.jar lacks the
# LambdaMetafactory javac needs); d8 desugars lambdas for API 26.
"$JAVAC" --release 8 -Xlint:-options -encoding UTF-8 -classpath "$PLATFORM_JAR" \
  -d "$OUT/classes" @"$OUT/sources.txt"
"$BT/d8" --release --min-api $API --lib "$PLATFORM_JAR" --output "$OUT/dex" \
  $(find "$OUT/classes" -name '*.class')

echo "==> 5/6 package (aapt2)"
rm -rf "$OUT/res-compiled" && mkdir -p "$OUT/res-compiled"
"$BT/aapt2" compile --dir "$HERE/app/res" -o "$OUT/res-compiled/res.zip"
"$BT/aapt2" link -o "$OUT/unsigned.apk" -I "$PLATFORM_JAR" \
  --manifest "$HERE/app/AndroidManifest.xml" \
  --min-sdk-version $API --target-sdk-version $TARGET_SDK \
  -A "$STAGE/assets" -0 png -0 toml \
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

echo "==> 6/6 align + sign (debug key)"
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
