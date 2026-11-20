#!/usr/bin/env bash
# JVM unit tests for Android clip-pack pieces (no device, no Gradle, no network to GitHub):
#   piece list parsing, resume of a partial piece, checksum mismatch rejection,
#   successful join, and the legacy single-file clippack.properties path.
#
#   JAVA_HOME=... ANDROID_HOME=... android/clippack-tests.sh
set -euo pipefail
cd "$(dirname "$0")"
ROOT=$(cd .. && pwd)
JAVA_HOME=${JAVA_HOME:-}
JAVAC=${JAVA_HOME:+$JAVA_HOME/bin/}javac
JAVA=${JAVA_HOME:+$JAVA_HOME/bin/}java
PLATFORM_JAR=${PLATFORM_JAR:-}
if [ -z "$PLATFORM_JAR" ]; then
  for c in       "${ANDROID_HOME:-}/platforms/android-34/android.jar"       "$ROOT/../lillith/android-toolchain/sdk/platforms/android-34/android.jar"       /workspace/lillith/android-toolchain/sdk/platforms/android-34/android.jar; do
    [ -n "${c:-}" ] && [ -f "$c" ] && { PLATFORM_JAR=$c; break; }
  done
fi
[ -n "$PLATFORM_JAR" ] || { echo "need android.jar (PLATFORM_JAR or ANDROID_HOME)" >&2; exit 2; }
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
mkdir -p "$W/cls"
# ClipPack + its tests only (android.jar provides Context/Log/JSON stubs we don't call)
"$JAVAC" --release 8 -Xlint:-options -encoding UTF-8 -classpath "$PLATFORM_JAR" -d "$W/cls" \
  app/src/com/kokannadian/lillith/ClipPack.java \
  app/test/java/com/kokannadian/lillith/ClipPackTests.java
"$JAVA" -cp "$W/cls:$PLATFORM_JAR" com.kokannadian.lillith.ClipPackTests
