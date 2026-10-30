#!/usr/bin/env bash
# Build the DemonOS-PhantomCore live ISO from this checkout.
#
#   iso/build-iso.sh                 # full build -> $BUILD/out/demonos-phantomcore-*.iso
#
# Host side (this script, as a normal user with sudo):
#   1. preflight: disk, LFS art present, llama pin == bundle/llama/llama.lock, model file
#   2. stage a copy of iso/profile in $BUILD/staging (never edits the repo) and
#      hardlink the bundled GGUF into it
#   3. bind-mount $BUILD -> <chroot>/build and the checkout -> <chroot>/build/src (read-only)
#      inside the Arch chroot made by scripts/setup-arch-chroot.sh
#   4. run iso/chroot-build.sh in the chroot (makepkg as non-root, repo-add, mkarchiso)
#   5. unmount, remove work/, print ISO path, size, sha256
# A disk watcher stops the build (and says so) if free space drops below
# $ABORT_FREE_MB; nothing outside $BUILD is ever deleted.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
BUILD="${DEMONOS_BUILD_DIR:-/workspace/qsec/iso-build}"
CHROOT_SH="${ARCH_CHROOT_SCRIPT:-$HERE/setup-arch-chroot.sh}"
ROOT="${ARCH_ROOT:-/workspace/qsec/arch-root/root.x86_64}"
MODEL="${DEMONOS_MODEL:-/workspace/models/qwen2.5-0.5b-instruct-q4_k_m.gguf}"
MODEL_SHA="${DEMONOS_MODEL_SHA:-}"         # optional pin
ABORT_FREE_MB="${ABORT_FREE_MB:-1500}"
MIN_START_FREE_MB="${MIN_START_FREE_MB:-10000}"
TS="$(date +%Y%m%d-%H%M%S)"
LOG="$BUILD/logs/build-iso-$TS.log"

mkdir -p "$BUILD/logs" "$BUILD/out"
exec > >(tee -a "$LOG") 2>&1
say(){ printf '[build-iso %(%F %T)T] %s\n' -1 "$*"; }
free_mb(){ df -Pm "$BUILD" | awk 'NR==2{print $4}'; }

say "repo $REPO @ $(git -C "$REPO" rev-parse --short HEAD 2>/dev/null || echo '?')  build dir $BUILD  log $LOG"

# ---- 1. preflight ---------------------------------------------------------
f=$(free_mb); say "free on $BUILD: ${f} MB"
(( f >= MIN_START_FREE_MB )) || { say "STOP: need >= ${MIN_START_FREE_MB} MB free to start (have ${f}). Nothing deleted."; exit 3; }
python3 "$REPO/linux/stage-assets.py" --check "$REPO/assets/generated" \
  || { say "STOP: animation art is Git LFS pointers; run: git lfs pull --include='assets/generated/**'"; exit 2; }
lock_sha=$(awk '$1=="asset" && $2=="linux-x64"{print $3}' "$REPO/bundle/llama/llama.lock")
lock_tag=$(awk '$1=="tag"{print $2}' "$REPO/bundle/llama/llama.lock")
grep -q "^_tag=$lock_tag\$" "$HERE/pkgs/lillith-llama-bin/PKGBUILD" && grep -q "$lock_sha" "$HERE/pkgs/lillith-llama-bin/PKGBUILD" \
  || { say "STOP: iso/pkgs/lillith-llama-bin/PKGBUILD is not in step with bundle/llama/llama.lock ($lock_tag $lock_sha)"; exit 2; }
[[ -f "$MODEL" ]] || { say "STOP: model $MODEL not found"; exit 2; }
if [[ -n "$MODEL_SHA" ]]; then echo "$MODEL_SHA  $MODEL" | sha256sum -c - || exit 2; fi
SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$REPO" log -1 --format=%ct 2>/dev/null || date +%s)}"
export SOURCE_DATE_EPOCH
say "SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH"

# ---- 2. stage the profile ---------------------------------------------------
sudo rm -rf "$BUILD/staging" "$BUILD/work"
mkdir -p "$BUILD/staging"
cp -a "$HERE/profile" "$BUILD/staging/profile"
M="$BUILD/staging/profile/airootfs/usr/share/lillith/models"
mkdir -p "$M"
ln "$MODEL" "$M/$(basename "$MODEL")" 2>/dev/null || cp "$MODEL" "$M/"
say "staged profile; model $(basename "$MODEL") ($(stat -c %s "$MODEL") bytes)"

# ---- 3. mounts ----------------------------------------------------------------
"$CHROOT_SH" run true             # makes $ROOT a mount point (bind) if needed
cleanup(){
  set +e
  [[ -n "${WATCH_PID:-}" ]] && kill "$WATCH_PID" 2>/dev/null
  sudo umount "$ROOT/build/src" 2>/dev/null; sudo umount "$ROOT/build" 2>/dev/null
  sudo rm -rf "$BUILD/work"
}
trap cleanup EXIT
sudo mkdir -p "$ROOT/build"
mountpoint -q "$ROOT/build" || sudo mount --bind "$BUILD" "$ROOT/build"
sudo mkdir -p "$ROOT/build/src"
mountpoint -q "$ROOT/build/src" || { sudo mount --bind "$REPO" "$ROOT/build/src" && sudo mount -o remount,bind,ro "$ROOT/build/src"; }

# ---- disk watcher ---------------------------------------------------------------
(
  while sleep 15; do
    f=$(free_mb)
    if (( f < ABORT_FREE_MB )); then
      echo "[disk-watch] STOP: ${f} MB free < ${ABORT_FREE_MB} MB; stopping the build (nothing else deleted)." | tee -a "$LOG"
      # only processes running inside the build chroot (never other users' builds)
      for p in /proc/[0-9]*; do
        [[ "$(sudo readlink "$p/root" 2>/dev/null)" == "$ROOT" ]] && sudo kill -TERM "${p#/proc/}" 2>/dev/null
      done
      touch "$BUILD/logs/ABORTED-LOW-DISK-$TS"
      exit 0
    fi
  done
) & WATCH_PID=$!

# ---- 4. build in the chroot -----------------------------------------------------
say "running iso/chroot-build.sh in the Arch chroot"
"$CHROOT_SH" run env SOURCE_DATE_EPOCH="$SOURCE_DATE_EPOCH" \
  MIN_FREE_MB_ISO="${MIN_FREE_MB_ISO:-9500}" JOBS="${JOBS:-4}" KEEP_RUST="${KEEP_RUST:-0}" \
  SKIP_PACKAGES="${SKIP_PACKAGES:-0}" \
  /build/src/iso/chroot-build.sh

# ---- 5. result ----------------------------------------------------------------
ISO="$(ls -t "$BUILD"/out/*.iso | head -n1)"
sudo chown "$(id -u):$(id -g)" "$ISO"
( cd "$(dirname "$ISO")" && sha256sum "$(basename "$ISO")" | tee "$(basename "$ISO").sha256" )
say "ISO: $ISO ($(du -h "$ISO" | cut -f1))"
say "free on $BUILD now: $(free_mb) MB"
