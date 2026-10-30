#!/usr/bin/env bash
# Reproducible Arch Linux build chroot (archiso + grub) on a Debian host.
# Usage: scripts/setup-arch-chroot.sh            # download, verify, init, install
#        scripts/setup-arch-chroot.sh shell      # interactive shell inside the chroot
#        scripts/setup-arch-chroot.sh run CMD... # run a command inside the chroot
# Needs: sudo, curl, zstd, sqv (Debian 13 default), arch-install-scripts (arch-chroot).
set -euo pipefail

ARCH_BASE=${ARCH_BASE:-/workspace/qsec/arch-root}
ROOT="$ARCH_BASE/root.x86_64"
MIRROR=${MIRROR:-https://geo.mirror.pkgbuild.com}
# Release signing key (Pierre Schmitz), fingerprint as published on https://archlinux.org/download/
ARCH_ISO_KEY_FPR=3E80CA1A8B89F69CBA57D98A76A5EF9054449A5C
PACKAGES=(archiso grub)

log(){ printf '\n==> %s\n' "$*"; }

ensure_mounted(){
  # arch-chroot needs the root to be a mount point.
  mountpoint -q "$ROOT" || sudo mount --bind "$ROOT" "$ROOT"
}
in_chroot(){ ensure_mounted; sudo arch-chroot "$ROOT" "$@"; }

case "${1:-setup}" in
  shell) in_chroot /bin/bash -l; exit ;;
  run)   shift; in_chroot "$@"; exit ;;
  umount) sudo umount -R "$ROOT" 2>/dev/null || true; exit ;;
  setup) ;;
  *) echo "unknown subcommand: $1" >&2; exit 2 ;;
esac

mkdir -p "$ARCH_BASE/dl"
cd "$ARCH_BASE/dl"

if [ ! -x "$ROOT/usr/bin/pacman" ]; then
  log "Resolve latest bootstrap release"
  VER=$(curl -sfL "$MIRROR/iso/latest/" | grep -oE 'archlinux-bootstrap-[0-9]{4}\.[0-9]{2}\.[0-9]{2}-x86_64\.tar\.zst' | head -1 | sed -E 's/archlinux-bootstrap-(.*)-x86_64.tar.zst/\1/')
  [ -n "$VER" ] || { echo "could not resolve release version" >&2; exit 1; }
  TB="archlinux-bootstrap-$VER-x86_64.tar.zst"
  echo "release $VER"

  log "Download tarball + detached signature from $MIRROR"
  curl -sfSL --retry 3 -o "$TB"     "$MIRROR/iso/$VER/$TB"
  curl -sfSL --retry 3 -o "$TB.sig" "$MIRROR/iso/$VER/$TB.sig"

  log "Verify SHA256 against archlinux.org (independent host)"
  curl -sfSL -o sha256sums.txt "https://archlinux.org/iso/$VER/sha256sums.txt"
  grep " $TB\$" sha256sums.txt | sha256sum -c -

  log "Verify OpenPGP signature with release key $ARCH_ISO_KEY_FPR"
  curl -sfSL -o release-key.asc "https://keyserver.ubuntu.com/pks/lookup?op=get&options=mr&search=0x$ARCH_ISO_KEY_FPR"
  sqv --keyring release-key.asc --signature-file "$TB.sig" "$TB" | grep -qi "$ARCH_ISO_KEY_FPR" \
    || { echo "signature not made by $ARCH_ISO_KEY_FPR" >&2; exit 1; }
  echo "signature OK ($ARCH_ISO_KEY_FPR)"

  log "Extract into $ARCH_BASE (as root, preserving ownership/xattrs)"
  sudo tar --zstd -xpf "$TB" -C "$ARCH_BASE" --numeric-owner
  rm -f "$TB"   # keep only .sig/checksums; the extracted tree is the artifact
fi

log "Configure mirror"
echo "Server = $MIRROR/\$repo/os/\$arch" | sudo tee "$ROOT/etc/pacman.d/mirrorlist" >/dev/null
# The bootstrap image has no space-check-friendly mount table inside the chroot.
sudo sed -i 's/^CheckSpace/#CheckSpace/' "$ROOT/etc/pacman.conf"

log "pacman keyring init + populate"
if [ ! -s "$ROOT/etc/pacman.d/gnupg/pubring.gpg" ] && [ ! -s "$ROOT/etc/pacman.d/gnupg/pubring.kbx" ]; then
  in_chroot pacman-key --init
fi
in_chroot pacman-key --populate archlinux

log "Full upgrade + install ${PACKAGES[*]}"
in_chroot pacman -Syu --needed --noconfirm "${PACKAGES[@]}"
log "Trim package cache"
sudo find "$ROOT/var/cache/pacman/pkg" -mindepth 1 -delete

log "Smoke test"
in_chroot mkarchiso -h | head -5
in_chroot pacman -Q archiso grub
log "Done. Enter with: $0 shell"
