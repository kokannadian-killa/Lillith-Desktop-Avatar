#!/usr/bin/env bash
# Runs INSIDE the Arch build chroot (as root), started by iso/build-iso.sh:
#   /build      = host build dir (/workspace/qsec/iso-build), read-write
#   /build/src  = the repo checkout, read-only
#   /build/pkgcache, /build/repo = pacman downloads + local repo (see profile/pacman.conf)
# Steps: build deps -> makepkg (as user "builder") -> repo-add -> theme art ->
#        free build trees -> mkarchiso -> leave /build/out/*.iso
set -euo pipefail

SRC=/build/src
STAGE=/build/staging/profile
REPO=/build/repo                       # matches iso/profile/pacman.conf
PKGCACHE=/build/pkgcache               # matches iso/profile/pacman.conf
MIN_FREE_MB_BUILD=${MIN_FREE_MB_BUILD:-6000}
MIN_FREE_MB_ISO=${MIN_FREE_MB_ISO:-9500}
JOBS=${JOBS:-4}                         # box RAM is shared with other builds
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(date +%s)}

log(){ printf '\n[chroot-build %(%F %T)T] %s\n' -1 "$*"; }
free_mb(){ df -Pm /build | awk 'NR==2{print $4}'; }
need_free(){ # $1 = MB, $2 = what
  local f; f=$(free_mb)
  if (( f < $1 )); then
    log "STOP: only ${f} MB free on /build, need ${1} MB for $2. Not deleting anything; free space and re-run."
    exit 3
  fi
  log "disk ok for $2: ${f} MB free (need ${1})"
}

if [[ "${SKIP_PACKAGES:-0}" != 1 ]]; then
log "1/7 build dependencies"
mkdir -p "$PKGCACHE" "$REPO"
pacman -Syu --noconfirm --needed --cachedir "$PKGCACHE" \
  base-devel rust pkgconf python python-pillow freetype2 grub archiso \
  gcc-libs glibc libxkbcommon wayland dbus libpulse systemd-libs alsa-lib \
  openssl zlib zstd
rm -f "$PKGCACHE"/*.pkg.tar.* 2>/dev/null || true
rm -f "$REPO"/*.pkg.tar.* "$REPO"/demonos-local.* 2>/dev/null || true

log "2/7 unprivileged build user"
id builder &>/dev/null || useradd -m -U builder
install -d -o builder -g builder /build/makepkg /build/srcdest /build/logs \
  /build/cargo-target /build/cargo-home /build/pkgwork "$REPO"
need_free "$MIN_FREE_MB_BUILD" "cargo + makepkg"

mk(){ # $1 = dir with PKGBUILD
  sudo -u builder env -i HOME=/home/builder PATH=/usr/bin:/usr/local/bin LANG=C.UTF-8 \
    SOURCE_DATE_EPOCH="$SOURCE_DATE_EPOCH" \
    BUILDDIR=/build/makepkg PKGDEST="$REPO" SRCDEST=/build/srcdest LOGDEST=/build/logs \
    CARGO_TARGET_DIR=/build/cargo-target CARGO_HOME=/build/cargo-home \
    MAKEFLAGS="-j$JOBS" CARGO_BUILD_JOBS="$JOBS" \
    bash -c "cd '$1' && makepkg -f -c --noconfirm --log"
}

log "3/7 makepkg lillith (repo linux/PKGBUILD, unmodified)"
mk "$SRC/linux"
log "3/7 makepkg lillith-llama-bin (iso/pkgs, pinned per bundle/llama/llama.lock)"
rm -rf /build/pkgwork/lillith-llama-bin
cp -r "$SRC/iso/pkgs/lillith-llama-bin" /build/pkgwork/
chown -R builder:builder /build/pkgwork
mk /build/pkgwork/lillith-llama-bin

log "4/7 repo-add"
repo-add "$REPO/demonos-local.db.tar.gz" "$REPO"/*.pkg.tar.zst
install -d /build/packages
ls -la "$REPO"/*.pkg.tar.zst | tee /build/packages/built-packages.txt
# keep the small llama package; the ~1.4 GB lillith one is deleted during mkarchiso
cp -f "$REPO"/lillith-llama-bin-*.pkg.tar.zst /build/packages/ || true

log "5/7 free build trees (cargo target/home, makepkg dirs, rust toolchain)"
rm -rf /build/cargo-target /build/cargo-home /build/makepkg /build/srcdest /build/pkgwork
if [[ "${KEEP_RUST:-0}" != 1 ]]; then pacman -Rns --noconfirm rust || true; fi
fi  # SKIP_PACKAGES

log "6/7 theme art (iso/theme/make-theme-assets.py) + GRUB fonts"
T=/build/theme-out; rm -rf "$T"
python3 "$SRC/iso/theme/make-theme-assets.py" "$SRC" "$T"
G="$STAGE/grub/themes/demonos"
cp "$T/grub/background.png" "$T"/grub/select_*.png "$G/"
FONT="$SRC/crates/lillith-theme/fonts/JetBrainsMono.ttf"
for s in 16 24; do
  grub-mkfont -s "$s" -n "DemonOS Mono Regular $s" -o "$G/demonos-mono-$s.pf2" "$FONT"
  # The theme refers to the font by this name; fail loudly if grub-mkfont named it differently.
  grep -aq "DemonOS Mono Regular $s" "$G/demonos-mono-$s.pf2" || { log "font name mismatch for size $s"; exit 4; }
done
cp "$T/syslinux/splash.png" "$STAGE/syslinux/splash.png"
cp "$T"/plymouth/*.png "$STAGE/airootfs/usr/share/plymouth/themes/demonos/"
cp "$T/sddm/background.png" "$STAGE/airootfs/usr/share/sddm/themes/demonos/background.png"
cp -r "$T/wallpaper/DemonOS/contents" "$STAGE/airootfs/usr/share/wallpapers/DemonOS/"

log "7/7 mkarchiso"
need_free "$MIN_FREE_MB_ISO" "mkarchiso (airootfs + squashfs)"
rm -rf /build/work
mkarchiso -v -r -w /build/work -o /build/out "$STAGE" &
mk_pid=$!
# Once mksquashfs runs, every package is installed in the airootfs: drop the
# download cache and the local repo (both rebuildable) to cut peak disk use.
( while kill -0 "$mk_pid" 2>/dev/null; do
    if pgrep -x mksquashfs >/dev/null; then
      log "mksquashfs started: removing downloaded + local packages ($(du -shc "$PKGCACHE" "$REPO" | tail -1))"
      rm -f "$PKGCACHE"/*.pkg.tar.* "$REPO"/*.pkg.tar.*; break
    fi; sleep 5
  done ) &
wait "$mk_pid"
rm -rf /build/work
ls -la /build/out
log "done"
