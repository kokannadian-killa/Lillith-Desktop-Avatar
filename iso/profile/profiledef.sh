#!/usr/bin/env bash
# shellcheck disable=SC2034
# DemonOS-PhantomCore live ISO (Lillith OS + QuantumSec), based on archiso releng.

iso_name="demonos-phantomcore"
iso_label="DEMONOS_$(date --date="@${SOURCE_DATE_EPOCH:-$(date +%s)}" +%Y%m)"
iso_publisher="Ko Kannadian <https://github.com/kokannadian-killa/Lillith-Desktop-Avatar>"
iso_application="DemonOS PhantomCore Live (Lillith OS + QuantumSec)"
iso_version="$(date --date="@${SOURCE_DATE_EPOCH:-$(date +%s)}" +%Y.%m.%d)"
install_dir="demonos"
# BIOS: syslinux (themed splash). UEFI: GRUB (DemonOS theme in grub/themes/demonos).
bootmodes=('bios.syslinux'
           'uefi.grub')
pacman_conf="pacman.conf"
airootfs_image_type="squashfs"
# zstd instead of releng's xz: much faster to build on the box, slightly larger ISO.
airootfs_image_tool_options=('-comp' 'zstd' '-Xcompression-level' '15' '-b' '1M')
bootstrap_tarball_compression=('zstd' '-c' '-T0' '--auto-threads=logical' '--long' '-19')
file_permissions=(
  ["/etc/shadow"]="0:0:400"
  ["/etc/gshadow"]="0:0:400"
  ["/etc/sudoers.d/10-demonos-live"]="0:0:440"
  ["/root"]="0:0:750"
  ["/root/.automated_script.sh"]="0:0:755"
  ["/root/.gnupg"]="0:0:700"
  ["/usr/local/bin/choose-mirror"]="0:0:755"
  ["/usr/local/bin/Installation_guide"]="0:0:755"
  ["/usr/local/bin/livecd-sound"]="0:0:755"
  ["/usr/local/bin/demonos-awakening"]="0:0:755"
  ["/usr/local/bin/demonos-firstboot"]="0:0:755"
)
