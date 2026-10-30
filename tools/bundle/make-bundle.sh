#!/usr/bin/env bash
# Builds Lillith's all-in-one download: the release package + a prebuilt,
# pinned llama.cpp `llama-server` for Linux x86-64 and Windows x64.
#
#     tools/bundle/make-bundle.sh --base Lillith-Desktop-Avatar-<sha>.zip
#     tools/bundle/make-bundle.sh --fetch-only      # just fill bundle/llama/<platform>/
#
# BUILD MACHINE ONLY. This is the one place that downloads llama.cpp: the
# official ggml-org/llama.cpp release assets pinned in bundle/llama/llama.lock
# (tag + SHA-256 each; a mismatch aborts). The finished zip contains
# everything, so nothing on the user's computer downloads at run time.
#
# Steps: fetch (cached) -> verify SHA-256 -> unpack the server + the libraries
# it loads into bundle/llama/<platform>/ (plus llama.cpp's LICENSE and any
# third-party license in the asset) -> assemble the zip. Only llama-server is
# kept: no llama-connect (WebRTC remote tunnel), no RPC server/backend, no
# other tools. No model weights are ever added (bundle/models/ holds a README).
#
# Options:
#   --base ZIP|DIR    release package to merge into (zip with one top folder,
#                     e.g. Lillith-Desktop-Avatar-<sha>.zip, or that folder).
#                     Without it the zip holds only the bundle/ folder, to be
#                     unzipped over a release.
#   --out FILE        output zip (default dist/bundle/<base>-all-in-one.zip)
#   --platforms LIST  comma list (default linux-x64,linux-x64-vulkan,windows-x64)
#   --no-vulkan       drop linux-x64-vulkan
#   --fetch-only      stop after bundle/llama/<platform>/ (windows/build.sh and
#                     linux/install.sh pick it up from there)
#   --offline         never download; the cache must already hold the assets
#   --cache DIR       download cache (default ${XDG_CACHE_HOME:-~/.cache}/lillith-bundle)
#
# Needs: bash, curl, sha256sum, tar, unzip, python3.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
LOCK="$ROOT/bundle/llama/llama.lock"
CACHE="${LILLITH_BUNDLE_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/lillith-bundle}"
PLATFORMS="linux-x64,linux-x64-vulkan,windows-x64"
BASE="" OUT="" FETCH_ONLY=0 OFFLINE=0 NO_VULKAN=0

die() { printf 'error: %s\n' "$*" >&2; exit 1; }
say() { printf '==> %s\n' "$*"; }
usage() { sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d; s/^# \{0,1\}//'; }

while (($#)); do
  case "$1" in
    --base) BASE="${2:?--base needs a zip or folder}"; shift ;;
    --out) OUT="${2:?--out needs a file}"; shift ;;
    --platforms) PLATFORMS="${2:?--platforms needs a list}"; shift ;;
    --no-vulkan) NO_VULKAN=1 ;;
    --fetch-only) FETCH_ONLY=1 ;;
    --offline) OFFLINE=1 ;;
    --cache) CACHE="${2:?--cache needs a folder}"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option $1 (see --help)" ;;
  esac
  shift
done

for t in curl sha256sum tar unzip python3; do
  command -v "$t" >/dev/null 2>&1 || die "'$t' not found"
done
[[ -f $LOCK ]] || die "missing $LOCK"

lock_field() { awk -v k="$1" '$1 == k { print $2; exit }' "$LOCK"; }
TAG="$(lock_field tag)"
COMMIT="$(lock_field commit)"
[[ $TAG =~ ^b[0-9]+$ ]] || die "bad tag in $LOCK: '$TAG'"

IFS=, read -r -a PLATS <<<"$PLATFORMS"
if ((NO_VULKAN)); then
  keep=(); for p in "${PLATS[@]}"; do [[ $p == linux-x64-vulkan ]] || keep+=("$p"); done
  PLATS=("${keep[@]}")
fi
((${#PLATS[@]})) || die "no platforms selected"

mkdir -p "$CACHE"

# fetch_verified <url> <sha256> <dest>: cached download, SHA-256 checked.
fetch_verified() {
  local url="$1" sha="$2" dest="$3"
  [[ $url == https://github.com/ggml-org/llama.cpp/* || $url == https://raw.githubusercontent.com/ggml-org/llama.cpp/* ]] \
    || die "refusing non-ggml-org URL: $url"
  if [[ -f $dest ]] && printf '%s  %s\n' "$sha" "$dest" | sha256sum -c --quiet --status; then
    printf '    cached   %s\n' "$(basename "$dest")"
    return
  fi
  ((OFFLINE)) && die "--offline and $(basename "$dest") is not in $CACHE (or its SHA-256 doesn't match)"
  printf '    fetch    %s\n' "$url"
  curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$dest.part" "$url" \
    || die "download failed: $url"
  mv -f "$dest.part" "$dest"
  if ! printf '%s  %s\n' "$sha" "$dest" | sha256sum -c --quiet --status; then
    local got; got="$(sha256sum "$dest" | cut -d' ' -f1)"
    rm -f "$dest"
    die "SHA-256 mismatch for $(basename "$dest"): want $sha, got $got"
  fi
}

# ── 1+2. fetch + verify ──────────────────────────────────────────────
say "llama.cpp $TAG ($COMMIT), platforms: ${PLATS[*]}"
read -r _ LIC_NAME LIC_SHA LIC_URL < <(awk '$1 == "license" { print; exit }' "$LOCK")
[[ -n ${LIC_URL:-} ]] || die "no license line in $LOCK"
LIC_FILE="$CACHE/$TAG-$LIC_NAME"
fetch_verified "$LIC_URL" "$LIC_SHA" "$LIC_FILE"

declare -A ASSET
for p in "${PLATS[@]}"; do
  read -r _ _ sha size url < <(awk -v p="$p" '$1 == "asset" && $2 == p { print; exit }' "$LOCK") \
    || die "no asset for '$p' in $LOCK"
  [[ -n ${url:-} ]] || die "no asset for '$p' in $LOCK"
  f="$CACHE/$(basename "$url")"
  fetch_verified "$url" "$sha" "$f"
  [[ $(stat -c %s "$f") == "$size" ]] || die "size mismatch for $(basename "$f")"
  printf '    sha256   %s  %s\n' "$sha" "$(basename "$f")"
  ASSET[$p]="$f|$sha|$url"
done

# ── 3. unpack (llama-server + what it loads, nothing else) ───────────
# Linux: copy the sonames the binaries link (real files, no symlinks, so any
# unzip tool works); the CPU variants are dlopen()ed from the exe's folder.
LINUX_LIBS=(libllama-server-impl.so libllama-common.so.0 libllama.so.0 libmtmd.so.0 libggml.so.0 libggml-base.so.0)
WIN_FILES=(llama-server.exe llama-server-impl.dll llama-common.dll llama.dll mtmd.dll ggml.dll ggml-base.dll libomp.dll)

unpack() {
  local p="$1" file="${ASSET[$1]%%|*}" rest="${ASSET[$1]#*|}"
  local sha="${rest%%|*}" url="${rest#*|}"
  local dest="$ROOT/bundle/llama/$p" tmp src f
  tmp="$(mktemp -d)"
  case "$file" in
    *.tar.gz) tar -xzf "$file" -C "$tmp" ;;
    *.zip) unzip -q "$file" -d "$tmp" ;;
  esac
  src="$tmp"
  [[ -d "$tmp/llama-$TAG" ]] && src="$tmp/llama-$TAG"
  rm -rf "$dest"; mkdir -p "$dest"
  case "$p" in
    linux-*)
      [[ -x "$src/llama-server" ]] || die "$p: llama-server missing from $(basename "$file")"
      cp "$src/llama-server" "$dest/"
      for f in "${LINUX_LIBS[@]}"; do
        [[ -e "$src/$f" ]] || die "$p: $f missing from $(basename "$file")"
        cp -L "$src/$f" "$dest/$f"
      done
      cp "$src"/libggml-cpu-*.so "$dest/"
      if [[ $p == *vulkan* ]]; then
        [[ -f "$src/libggml-vulkan.so" ]] || die "$p: libggml-vulkan.so missing"
        cp "$src/libggml-vulkan.so" "$dest/"
      fi
      ;;
    windows-*)
      for f in "${WIN_FILES[@]}"; do
        [[ -f "$src/$f" ]] || die "$p: $f missing from $(basename "$file")"
        cp "$src/$f" "$dest/"
      done
      cp "$src"/ggml-cpu-*.dll "$dest/"
      ;;
  esac
  ls "$dest"/*ggml-cpu-haswell* >/dev/null 2>&1 || die "$p: no AVX2 (haswell) CPU backend"
  # Licenses: llama.cpp's MIT LICENSE (pinned) + any shipped with the asset.
  cp "$LIC_FILE" "$dest/$LIC_NAME"
  for f in "$src"/LICENSE*; do
    [[ -f $f && $(basename "$f") != LICENSE ]] && cp "$f" "$dest/"
  done
  rm -rf "$tmp"
  {
    printf 'llama.cpp %s (commit %s), unmodified official build\n' "$TAG" "$COMMIT"
    printf 'source asset: %s\nsha256:       %s\n\n' "$url" "$sha"
    printf 'Lillith starts it herself, loopback only:\n'
    printf '  llama-server -m <model.gguf> --host 127.0.0.1 --port 8080 -c 4096 -t <physical cores> --jinja\n\n'
    printf 'Files (sha256):\n'
    (cd "$dest" && sha256sum -- * | grep -v ' BUNDLE.txt$')
  } >"$dest/BUNDLE.txt"
  printf '    unpacked %s (%s files, %s)\n' "bundle/llama/$p" "$(ls "$dest" | wc -l)" "$(du -sh "$dest" | cut -f1)"
}

say "unpacking into bundle/llama/"
for p in "${PLATS[@]}"; do unpack "$p"; done

# Local smoke test (no network, no model): the Linux build runs here.
if [[ $(uname -sm) == "Linux x86_64" && -x "$ROOT/bundle/llama/linux-x64/llama-server" ]]; then
  v="$("$ROOT/bundle/llama/linux-x64/llama-server" --version 2>&1 | grep -m1 version || true)"
  [[ $v == *"build ${TAG#b}"* ]] || die "bundled llama-server didn't run: $v"
  printf '    runs     %s\n' "$v"
fi

((FETCH_ONLY)) && { say "done (fetch only)"; exit 0; }

# ── 4. assemble the zip ──────────────────────────────────────────────
mkdir -p "$ROOT/dist/bundle"
STAGE="$(mktemp -d "$ROOT/dist/bundle/stage.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT
if [[ -n $BASE ]]; then
  if [[ -f $BASE ]]; then
    say "base $(basename "$BASE")"
    unzip -q "$BASE" -d "$STAGE"
  elif [[ -d $BASE ]]; then
    say "base $BASE/"
    cp -a "$BASE" "$STAGE/"
  else
    die "--base $BASE: not a file or folder"
  fi
  mapfile -t tops < <(find "$STAGE" -mindepth 1 -maxdepth 1)
  ((${#tops[@]} == 1)) && [[ -d ${tops[0]} ]] || die "--base must hold exactly one top folder"
  TOP="$(basename "${tops[0]}")"
  [[ -n $OUT ]] || OUT="$ROOT/dist/bundle/$TOP-all-in-one.zip"
else
  TOP="Lillith-llama-$TAG-bundle"
  mkdir -p "$STAGE/$TOP"
  [[ -n $OUT ]] || OUT="$ROOT/dist/bundle/$TOP.zip"
fi
PKG="$STAGE/$TOP"
mkdir -p "$PKG/bundle/llama" "$PKG/bundle/models"
for p in "${PLATS[@]}"; do cp -a "$ROOT/bundle/llama/$p" "$PKG/bundle/llama/"; done
cp "$LOCK" "$PKG/bundle/llama/llama.lock"
cp "$ROOT/bundle/models/README.txt" "$PKG/bundle/models/README.txt"
# Pose Builder's AI cut-out model (isnet-anime.onnx, sha256-checked; never in
# git, never downloaded): models/rembg/, where linux/install.sh and the
# pipeline (tools/video-pipeline/../../models) look. Source:
# $LILLITH_MATTE_MODEL, else /workspace/lillith/matte-dl/isnet-anime.onnx.
# NO_MATTE_MODEL=1 leaves it out.
if [[ "${NO_MATTE_MODEL:-}" != 1 ]]; then
  bash "$ROOT/tools/release/stage-matte-model.sh" "$PKG/models"
fi
cat >"$PKG/bundle/README.txt" <<TXT
Lillith all-in-one bundle
=========================

bundle/llama/<platform>/  llama.cpp $TAG llama-server (official prebuilt,
                          SHA-256 pinned in llama/llama.lock; see BUNDLE.txt)
  linux-x64         CPU (picks AVX2 etc. at run time)   <- used by default
  linux-x64-vulkan  same + Vulkan GPU (needs vulkan-icd-loader + vulkan-intel
                    or your GPU's Mesa driver); opt in under Settings > Brain
  windows-x64       CPU (AVX2 etc.); needs the Microsoft Visual C++ 2015-2022
                    Redistributable (x64), which most Windows PCs already have
bundle/models/            put your .gguf here (no model is included)
models/rembg/             isnet-anime.onnx, Pose Builder's AI cut-out
                          (sha256 f15622d8...1a6e99; installed for you)

The setup wizard finds the bundled llama-server and models first. Lillith
starts llama-server on 127.0.0.1:8080 only. Nothing downloads, ever.
TXT

say "zip -> $OUT"
mkdir -p "$(dirname "$OUT")"
rm -f "$OUT"
python3 - "$STAGE" "$TOP" "$OUT" <<'PY'
import os, sys, zipfile
stage, top, out = sys.argv[1:]
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as z:
    for dirpath, dirnames, filenames in os.walk(os.path.join(stage, top)):
        dirnames.sort()
        for name in sorted(dirnames) + sorted(filenames):
            full = os.path.join(dirpath, name)
            arc = os.path.relpath(full, stage)
            if os.path.islink(full):  # keep symlinks from the base as links
                info = zipfile.ZipInfo(arc)
                info.create_system = 3
                info.external_attr = 0o120777 << 16
                z.writestr(info, os.readlink(full))
            elif os.path.isdir(full):
                if not os.listdir(full):
                    z.write(full, arc + "/")
            else:
                z.write(full, arc)  # keeps the Unix mode (exec bits)
PY
(cd "$(dirname "$OUT")" && sha256sum "$(basename "$OUT")" >"$(basename "$OUT").sha256")
printf '    %s bytes  %s\n' "$(stat -c %s "$OUT")" "$(cut -d' ' -f1 "$OUT.sha256")"
say "done"
