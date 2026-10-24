#!/usr/bin/env bash
# Build the Windows voice payload (Lillith's cloned voice + Kokoro fallback + offline STT) on Linux:
#
#     tools/lillith-voice/windows/build-voice-win.sh --kit-tarball lillith-voice-kit-<sha>.tar.xz [--out DIR]
#
# Produces  $OUT/lillith-voice-win-<sha>.zip (+ .sha256), whose top folder voice\ is what windows/installer.nsi
# installs to %LOCALAPPDATA%\Lillith\voice\ :
#   python\                 official CPython embeddable (amd64), python313._pth -> Lib\site-packages, ..\lib, import site
#   python\Lib\site-packages win_amd64 wheels from voice-win.lock.json, installed offline (pip --target, no index)
#   lib\lillith_voice\      the kit code from this tree (incl. winlaunch.py, the hidden daemon launcher)
#   models\                 the release kit's models (checked against its SHA256SUMS)
#   bin\ffmpeg.exe          static ffmpeg with rubberband (p1 fallback preset, --speed/--pitch)
#   lillith-voiced.cmd, lillith-say.cmd, lillith-listen.cmd, voice-win.lock.json, licenses\, tests\
#
# Every download (python zip, ffmpeg zip, wheels) happens HERE, on the build machine, from the URLs pinned in
# voice-win.lock.json, each checked against its sha256. The installed app never downloads anything.
#
# Options / env:
#   --kit-tarball F   release voice kit (tools/lillith-voice/models/** is taken from it)  [or KIT_MODELS=<models dir>]
#   --out DIR         output dir (default: ./dist-voice-win)
#   CACHE=DIR         download cache (default: ~/.cache/lillith-voice-win); FFMPEG_ZIP / PYTHON_ZIP override files
#   NO_ZIP=1          stop after staging $OUT/voice (no zip)
# Needs: bash, curl, python3 (3.9+) with pip, unzip, zstd, sha256sum, tar (xz); osslsigncode (optional signature check).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KIT="$(cd "$HERE/.." && pwd)"              # tools/lillith-voice
LOCK="$HERE/voice-win.lock.json"
OUT="$PWD/dist-voice-win"; TARBALL=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --kit-tarball) TARBALL="$2"; shift 2;;
    --out) OUT="$2"; shift 2;;
    -h|--help) sed -n '2,24p' "$0"; exit 0;;
    *) echo "unknown arg: $1" >&2; exit 2;;
  esac
done
[[ -n "$TARBALL" || -n "${KIT_MODELS:-}" ]] || { echo "need --kit-tarball <kit.tar.xz> or KIT_MODELS=<dir>" >&2; exit 2; }
CACHE="${CACHE:-$HOME/.cache/lillith-voice-win}"
mkdir -p "$CACHE/wheels" "$OUT"
OUT="$(cd "$OUT" && pwd)"
STAGE="$OUT/voice"
j() { python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); 
for k in sys.argv[2].split("."): d=d[k]
print(d)' "$LOCK" "$1"; }

fetch() {  # url sha256 dest
  local url="$1" sum="$2" dest="$3"
  if [[ -f "$dest" ]] && echo "$sum  $dest" | sha256sum -c --status; then return 0; fi
  echo "  fetch $url"
  curl -fL --retry 3 -o "$dest.part" "$url"
  echo "$sum  $dest.part" | sha256sum -c --status || { echo "sha256 mismatch: $url" >&2; rm -f "$dest.part"; exit 1; }
  mv "$dest.part" "$dest"
}
check() {  # sha256 file
  echo "$1  $2" | sha256sum -c --status || { echo "sha256 mismatch: $2 (expected $1)" >&2; exit 1; }
}

echo "==> downloads (build machine only), pinned in $(basename "$LOCK")"
PY_ZIP="${PYTHON_ZIP:-$CACHE/$(j python.file)}"
[[ -n "${PYTHON_ZIP:-}" ]] && check "$(j python.sha256)" "$PY_ZIP" || fetch "$(j python.url)" "$(j python.sha256)" "$PY_ZIP"
VC_PKG="$CACHE/$(j vcruntime.file)"
fetch "$(j vcruntime.url)" "$(j vcruntime.sha256)" "$VC_PKG"
FF_ZIP="${FFMPEG_ZIP:-$CACHE/$(j ffmpeg.file)}"
[[ -n "${FFMPEG_ZIP:-}" ]] && check "$(j ffmpeg.sha256)" "$FF_ZIP" || fetch "$(j ffmpeg.url)" "$(j ffmpeg.sha256)" "$FF_ZIP"
python3 - "$LOCK" > "$CACHE/wheels.list" <<'PY'
import json, sys
for w in json.load(open(sys.argv[1]))["wheels"]:
    print(w["url"], w["sha256"], w["file"])
PY
while read -r url sum file; do fetch "$url" "$sum" "$CACHE/wheels/$file"; done < "$CACHE/wheels.list"

echo "==> staging $STAGE"
rm -rf "$STAGE"; mkdir -p "$STAGE/python" "$STAGE/lib" "$STAGE/bin" "$STAGE/licenses"
unzip -q "$PY_ZIP" -d "$STAGE/python"
PTH="$(cd "$STAGE/python" && ls python3*._pth)"
PYZIP="$(cd "$STAGE/python" && ls python3*.zip)"
# The embeddable Python ignores PYTHONPATH and user site; the ._pth is the whole sys.path.
printf '%s\r\n' "$PYZIP" "." "Lib\\site-packages" "..\\lib" "" "# site: needed for .pth files in site-packages" "import site" > "$STAGE/python/$PTH"
mkdir -p "$STAGE/python/Lib/site-packages"
WT=$(j wheels_target.platform); WV=$(j wheels_target.python_version); WI=$(j wheels_target.implementation)
WHEELS=(); while read -r _ _ file; do WHEELS+=("$CACHE/wheels/$file"); done < "$CACHE/wheels.list"
PIP_TMP="$(mktemp -d)"; trap 'rm -rf "$PIP_TMP"' EXIT
# offline: --no-index, only the pinned, hash-checked wheel files; --no-deps (the lock is the full closure)
PIP_NO_INDEX=1 python3 -m pip install -q --disable-pip-version-check --no-index --no-deps --no-compile \
  --only-binary=:all: --platform "$WT" --python-version "$WV" --implementation "$WI" \
  --target "$PIP_TMP/sp" "${WHEELS[@]}"
# drop the console-script launchers pip writes for Linux (bin/) and duplicate *.data payloads; keep the packages
rm -rf "$PIP_TMP/sp/bin" "$PIP_TMP/sp/Scripts"   # Scripts: sherpa_onnx_core's duplicate DLLs (sherpa_onnx/lib has them)
find "$PIP_TMP/sp" -name '*.lib' -delete            # import libraries, link-time only
cp -a "$PIP_TMP/sp/." "$STAGE/python/Lib/site-packages/"

# kit code (this tree, not the tarball), tests (self-checks) and docs
cp -a "$KIT/lillith_voice" "$STAGE/lib/"
find "$STAGE/lib" -name '__pycache__' -prune -exec rm -rf {} +
mkdir -p "$STAGE/tests"
for t in win_smoke.py fake_player.py transport_test.py check_wav.py stt_test.py safeword_test.py lipsync_sub.py; do
  cp "$KIT/tests/$t" "$STAGE/tests/"
done
cp "$KIT/PROTOCOL.md" "$KIT/README.md" "$STAGE/"
cp "$HERE/INSTALL-windows.md" "$STAGE/INSTALL-windows.md"
cp "$HERE"/lillith-voiced.cmd "$HERE"/lillith-say.cmd "$HERE"/lillith-listen.cmd "$STAGE/"
cp "$LOCK" "$STAGE/"

# ffmpeg.exe (+ license) from the pinned zip
python3 - "$LOCK" "$FF_ZIP" "$STAGE" <<'PY'
import json, sys, zipfile, os, shutil
lock, zp, stage = sys.argv[1:4]
members = json.load(open(lock))["ffmpeg"]["members"]
with zipfile.ZipFile(zp) as z:
    for src, dst in members.items():
        os.makedirs(os.path.dirname(os.path.join(stage, dst)), exist_ok=True)
        with z.open(src) as f, open(os.path.join(stage, dst), "wb") as o:
            shutil.copyfileobj(f, o, 1 << 20)
PY
# MSVC C++ runtime (msvcp140*.dll) app-local beside python.exe: onnxruntime/sentencepiece/espeak-ng need it
mkdir -p "$PIP_TMP/vc"
python3 -c 'import sys, zipfile; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])' "$VC_PKG" "$PIP_TMP/vc"
zstd -q -dc "$PIP_TMP"/vc/pkg-*.tar.zst | tar -x -C "$PIP_TMP/vc"
python3 - "$LOCK" "$PIP_TMP/vc" "$STAGE" <<'PY'
import json, os, shutil, sys
lock, src, stage = sys.argv[1:4]
for a, b in json.load(open(lock))["vcruntime"]["members"].items():
    shutil.copyfile(os.path.join(src, a), os.path.join(stage, b))
PY
if command -v osslsigncode >/dev/null; then
  for d in "$STAGE"/python/msvcp140*.dll; do
    sig="$(osslsigncode verify -in "$d" 2>&1 || true)"
    [[ "$sig" == *"O=Microsoft Corporation"* ]] || { echo "not Microsoft-signed: $d" >&2; exit 1; }
  done
fi
cp "$STAGE/python/LICENSE.txt" "$STAGE/licenses/python-LICENSE.txt" 2>/dev/null || true

# models from the release kit, verified
if [[ -n "${KIT_MODELS:-}" ]]; then
  cp -a "$KIT_MODELS" "$STAGE/models"
else
  echo "  models from $(basename "$TARBALL")"
  tar -xJf "$TARBALL" -C "$PIP_TMP" --wildcards 'tools/lillith-voice/models/*' 'tools/lillith-voice/models'
  mv "$PIP_TMP/tools/lillith-voice/models" "$STAGE/models"
fi
(cd "$STAGE/models" && sha256sum -c --quiet SHA256SUMS) || { echo "models: SHA256SUMS mismatch" >&2; exit 1; }
for need in pocket/voices/lillith.npy pocket/flow_lm_main.onnx kokoro-v1.0.onnx voices-v1.0.bin stt/silero_vad.onnx stt/tiny.en-encoder.int8.onnx; do
  [[ -f "$STAGE/models/$need" ]] || { echo "models: missing $need" >&2; exit 1; }
done

# refuse anything that could phone home / is not ours to ship
if grep -rIl --include='*.cmd' -iE 'curl|bitsadmin|Invoke-WebRequest|https?://' "$STAGE"/*.cmd; then
  echo "launchers must not download anything" >&2; exit 1
fi

# stable payload id: hash of the file list + contents
ID="$(cd "$STAGE" && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 sha256sum | sha256sum | cut -c1-12)"
echo "$ID" > "$STAGE/PAYLOAD_ID"
(cd "$STAGE" && find . -type f ! -name SHA256SUMS -print0 | LC_ALL=C sort -z | xargs -0 sha256sum > "$OUT/voice-SHA256SUMS")
cp "$OUT/voice-SHA256SUMS" "$STAGE/SHA256SUMS"
echo "  payload $ID: $(du -sh "$STAGE" | cut -f1) uncompressed, $(find "$STAGE" -type f | wc -l) files"

if [[ "${NO_ZIP:-}" != 1 ]]; then
  ZIP="$OUT/lillith-voice-win-$ID.zip"
  rm -f "$ZIP"
  # deterministic order + timestamps; deflate (the .onnx weights barely compress either way)
  python3 - "$OUT" "$ZIP" <<'PY'
import os, sys, zipfile
root, zp = sys.argv[1], sys.argv[2]
files = []
for d, _, fs in os.walk(os.path.join(root, "voice")):
    for f in fs:
        files.append(os.path.relpath(os.path.join(d, f), root))
with zipfile.ZipFile(zp, "w", zipfile.ZIP_DEFLATED, compresslevel=6, allowZip64=True) as z:
    for rel in sorted(files):
        zi = zipfile.ZipInfo(rel.replace(os.sep, "/"), (2026, 1, 1, 0, 0, 0))
        zi.compress_type = zipfile.ZIP_DEFLATED
        zi.external_attr = 0o100644 << 16
        with open(os.path.join(root, rel), "rb") as f, z.open(zi, "w", force_zip64=True) as o:
            while True:
                b = f.read(1 << 20)
                if not b:
                    break
                o.write(b)
PY
  (cd "$OUT" && sha256sum "$(basename "$ZIP")" > "$(basename "$ZIP").sha256")
  echo "==> $ZIP  $(stat -c %s "$ZIP") bytes  sha256 $(cut -d' ' -f1 "$ZIP.sha256")"
fi
