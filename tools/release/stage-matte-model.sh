#!/usr/bin/env bash
# Stages Pose Builder's AI cut-out model (isnet-anime.onnx, 176 MB) into a
# release payload at pack time. The model is NEVER in git (not even LFS) and
# NEVER downloaded at run time: Lillith stays fully local.
#
#   tools/release/stage-matte-model.sh <models dir> [--optional]
#
# Copies the model to <models dir>/rembg/isnet-anime.onnx after checking its
# size and sha256 (the constants in tools/video-pipeline/matte_model.py).
# <models dir> is the install's models folder in the payload:
#   Arch package       <pkgdir>/opt/lillith/models
#   Windows Setup.exe  <stage>/models        (-> <install dir>\models)
#   all-in-one zip     <zip root>/models
#
# Where the file comes from, first one set wins:
#   1. $LILLITH_MATTE_MODEL   path to isnet-anime.onnx (e.g. the release
#                             asset downloaded once by the release packer)
#   2. /workspace/lillith/matte-dl/isnet-anime.onnx   (the build box's
#                             verified copy)
# A hardlink is used when possible (disk is tight on the build box), else a copy.
#
# --optional: if no source file exists, print a warning and exit 0 (a dev
# build without the model; Pose Builder then falls back to the backdrop key
# and --matte isnet says exactly where to put the file). A file that exists
# but fails the checksum is always an error.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MM="$HERE/../video-pipeline/matte_model.py"
DEST="${1:-}"; OPT="${2:-}"
[[ -n $DEST && ( -z $OPT || $OPT == --optional ) ]] || { echo "usage: $0 <models dir> [--optional]" >&2; exit 2; }

NAME=isnet-anime.onnx
SIZE=$(sed -n "s/^MODEL_SIZE = \([0-9]*\).*/\1/p" "$MM")
SUM=$(sed -n "s/^MODEL_SHA256 = '\([0-9a-f]*\)'.*/\1/p" "$MM")
[[ ${#SUM} == 64 && -n $SIZE ]] || { echo "error: can't read MODEL_SIZE / MODEL_SHA256 from $MM" >&2; exit 1; }

SRC="${LILLITH_MATTE_MODEL:-/workspace/lillith/matte-dl/$NAME}"
if [[ ! -f $SRC ]]; then
  msg="$NAME not found at $SRC (set LILLITH_MATTE_MODEL to the verified file)"
  if [[ $OPT == --optional ]]; then echo "warning: $msg; the payload has no AI cut-out model" >&2; exit 0; fi
  echo "error: $msg" >&2; exit 1
fi
got=$(stat -c %s "$SRC")
[[ $got == "$SIZE" ]] || { echo "error: $SRC is $got bytes, needs $SIZE" >&2; exit 1; }
echo "$SUM  $SRC" | sha256sum --quiet -c - || { echo "error: $SRC: sha256 mismatch (needs $SUM)" >&2; exit 1; }

OUT="$DEST/rembg/$NAME"
mkdir -p "$DEST/rembg"
rm -f "$OUT"
ln "$SRC" "$OUT" 2>/dev/null || install -m644 "$SRC" "$OUT"
chmod 644 "$OUT" 2>/dev/null || true
[[ $(stat -c %s "$OUT") == "$SIZE" ]] || { echo "error: staging $OUT failed" >&2; exit 1; }
echo "==> $NAME: sha256 OK -> $OUT"
