# DemonOS-PhantomCore live ISO (Phase 4)

A bootable Arch Linux live ISO that contains the whole Lillith OS + QuantumSec
system: KDE Plasma 6 (Wayland), the `lillith` package built from
`linux/PKGBUILD`, the pinned `llama-server`, and a small bundled local model.
It boots straight to the desktop as the live user `demon`, starts Lillith, and
shows the QuantumSec awakening once.

All of this is **additive**. Nothing outside `iso/` changes; the build copies
`iso/profile` to a staging dir and never writes into the checkout.

## Layout

| Path | What it is |
|---|---|
| `profile/` | archiso profile, based on `releng` from archiso 91 (see "Changes from releng") |
| `profile/profiledef.sh` | name `demonos-phantomcore`, label `DEMONOS_YYYYMM`, boot modes `bios.syslinux` + `uefi.grub`, zstd squashfs |
| `profile/packages.x86_64` | releng list minus `cloud-init`, plus the DemonOS additions (bottom of file) |
| `profile/pacman.conf` | releng + `[demonos-local]` (file:///build/repo) + build cache dirs |
| `profile/grub/themes/demonos/` | GRUB theme (`theme.txt`; art + fonts are generated at build time) |
| `profile/syslinux/` | BIOS menu in the DemonOS palette (`splash.png` is regenerated at build time) |
| `profile/airootfs/etc/sddm.conf.d/10-demonos.conf` | autologin `demon` → Plasma (Wayland), theme `demonos` |
| `profile/airootfs/usr/share/sddm/themes/demonos/` | Qt6 QML SDDM theme |
| `profile/airootfs/usr/share/plymouth/themes/demonos/` | Plymouth script theme (`plymouth` hook added to `mkinitcpio.conf.d/archiso.conf`) |
| `profile/airootfs/etc/skel/.config/autostart/` | `lillith.desktop`, `demonos-awakening.desktop`, `demonos-firstboot.desktop` |
| `profile/airootfs/etc/skel/.config/lillith/config.toml` | `[chat]` → `http://127.0.0.1:8080`, `[chat.server]` → `/opt/llama/llama-server` + bundled GGUF |
| `profile/airootfs/usr/local/bin/demonos-awakening` | runs `lillith-settings --tab quantumsec --step awakening` until `~/.biotek/config/awakening.json` exists |
| `profile/airootfs/usr/local/bin/demonos-firstboot` | applies the `DemonOS` colour scheme and wallpaper once per user |
| `profile/airootfs/usr/share/{color-schemes,wallpapers,plasma}` | KDE colour scheme, wallpaper package and a Plasma update script |
| `pkgs/lillith-llama-bin/PKGBUILD` | llama.cpp `llama-server` b11829, pinned to the sha256 in `bundle/llama/llama.lock`, installed to `/opt/llama` |
| `theme/make-theme-assets.py` | draws the wallpaper, GRUB, syslinux, Plymouth and SDDM art from `branding/icons/{demonos-emblem,phantomcore-red}` and the `lillith-theme` fonts |
| `build-iso.sh` | host driver (below) |
| `chroot-build.sh` | runs inside the Arch chroot: makepkg, repo-add, art, mkarchiso |
| `test-boot.sh` | QEMU **TCG-only** boot test (OVMF + BIOS), with a serial log and PNG screendumps |

Palette: void `#0a0a0f`, crimson `#ff3e6c`, cyan `#00f0ff`, purple `#b829f7`.

## What is on the ISO

* **Desktop:** `plasma-desktop plasma-workspace kwin sddm konsole dolphin
  xdg-desktop-portal-kde qt6-wayland breeze mesa plymouth`.
* **Shell, tools and audio:** `fish` (the shell for `demon`), `tor torsocks`,
  `python python-psutil`, `pipewire pipewire-alsa pipewire-pulse wireplumber
  espeak-ng ffmpeg`, and the fonts `ttf-jetbrains-mono ttf-dejavu`.
* **Lillith runtime deps** (from the `linux/PKGBUILD` `depends`): `gcc-libs
  glibc libxkbcommon wayland dbus libpulse systemd-libs alsa-lib`.
* **Local repo `[demonos-local]`:**
  * `lillith`, built from the repo's own `linux/PKGBUILD`, unmodified.
  * `lillith-llama-bin`, built from `iso/pkgs`.
* **Model:** `/usr/share/lillith/models/qwen2.5-0.5b-instruct-q4_k_m.gguf`.
  It is the file at `/workspace/models` (491,400,032 bytes), hardlinked into
  the staging profile.
* **Optional second runtime:** Arch's `ollama` package. It is installed but not
  enabled. `llama-server` is the default runtime.

**Network and consent.** Nothing listens on the network at boot.
* `sshd` and `cloud-init` are not enabled.
* `tor` and `ollama` are installed but not enabled.
* `llama-server` is started only by Lillith, after the Soul Contract consent,
  and only on 127.0.0.1. The host is hard-coded in
  `lillith-mind/src/server.rs`.

**Live user.** The live user `demon` (uid 1000) has no password and is in the
groups `wheel audio video input`. `/etc/sudoers.d/10-demonos-live` gives
`wheel` NOPASSWD.
* This only exists because a passwordless live session can't answer a sudo
  prompt.
* Every system change Lillith or QuantumSec offers still goes through its own
  consent dialog, which shows the exact command first.
* Remove this file on any installed system.

**Red-team items** stay local simulation only, exactly as in the app.

## Changes from releng

All of these are in `iso/profile`; the repo itself is untouched.
* `profiledef.sh`:
  * new name, label and publisher;
  * boot modes `bios.syslinux` + `uefi.grub`, where releng uses `uefi.systemd-boot`, because GRUB can be themed;
  * squashfs compression `zstd -Xcompression-level 15`, where releng uses `xz`, to make builds faster;
  * extra `file_permissions` entries.
* `packages.x86_64`: `cloud-init` removed, and the DemonOS packages appended.
* Removed the enablement symlinks for `cloud-init*.service` and `sshd.service`, and the root autologin on tty1 (`getty@tty1.service.d/autologin.conf`).
* `display-manager.service` → `sddm.service`, and `default.target` → `graphical.target`.
* `etc/{passwd,shadow,group,gshadow}`: added `demon`.
* `etc/hostname`: `demonos`.
* `etc/motd`: replaced.
* `mkinitcpio.conf.d/archiso.conf`: added the `plymouth` hook after `udev`.
* `grub/grub.cfg`:
  * `gfxterm` and the theme;
  * entries renamed;
  * a "safe graphics" entry with `nomodeset`;
  * live kernel params `quiet splash loglevel=3 plymouth.ignore-serial-consoles console=ttyS0,115200 console=tty0`;
  * timeout 8 s.
* `syslinux/*`: title, colours, labels and the same kernel params.

## Building

Requirements:
* The Arch chroot from `/workspace/qsec/scripts/setup-arch-chroot.sh`, with archiso 91 and grub.
* sudo.
* About **10 GB free** on the disk that holds `/workspace/qsec/iso-build`.
* The animation art checked out from LFS. Only `assets/generated/**` and `branding/**` are needed: `git lfs pull --include='assets/generated/**,branding/**'`.

```bash
# once: Arch bootstrap chroot (verified download, pacman keyring, archiso, grub)
/workspace/qsec/scripts/setup-arch-chroot.sh

# build (from the checkout's root)
iso/build-iso.sh
# -> /workspace/qsec/iso-build/out/demonos-phantomcore-YYYY.MM.DD-x86_64.iso (+ .sha256)
```

Environment knobs:

| Variable | Meaning |
|---|---|
| `DEMONOS_BUILD_DIR` | build directory |
| `DEMONOS_MODEL` | the GGUF to bundle |
| `DEMONOS_MODEL_SHA` | optional sha256 pin for the model |
| `ARCH_CHROOT_SCRIPT` | path to `setup-arch-chroot.sh` |
| `ARCH_ROOT` | path to the chroot root |
| `MIN_START_FREE_MB` | free space needed before the build starts (default 10000) |
| `ABORT_FREE_MB` | the disk watcher kills only the chroot's processes below this (default 1500) |
| `MIN_FREE_MB_ISO` | free space needed before mkarchiso starts (default 9500) |
| `JOBS` | cargo/make parallelism (default 4) |
| `KEEP_RUST=1` | don't uninstall rust from the chroot afterwards |
| `SKIP_PACKAGES=1` | re-run only the art and mkarchiso steps, reusing `/build/repo` |
| `SOURCE_DATE_EPOCH` | defaults to the last commit's time, so the label, version and file times are reproducible |

### Every command the build runs

**Host side: `iso/build-iso.sh`**

```bash
# preflight
df -Pm /workspace/qsec/iso-build                       # need >= MIN_START_FREE_MB
python3 linux/stage-assets.py --check assets/generated # art is real, not LFS pointers
awk ... bundle/llama/llama.lock                        # tag + sha256 must match iso/pkgs/lillith-llama-bin/PKGBUILD
SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)
# stage
sudo rm -rf $BUILD/staging $BUILD/work
cp -a iso/profile $BUILD/staging/profile
ln /workspace/models/qwen2.5-0.5b-instruct-q4_k_m.gguf \
   $BUILD/staging/profile/airootfs/usr/share/lillith/models/
# mounts
/workspace/qsec/scripts/setup-arch-chroot.sh run true  # bind-mounts the chroot root onto itself
sudo mount --bind $BUILD $ROOT/build
sudo mount --bind <checkout> $ROOT/build/src && sudo mount -o remount,bind,ro $ROOT/build/src
# build
/workspace/qsec/scripts/setup-arch-chroot.sh run env SOURCE_DATE_EPOCH=... /build/src/iso/chroot-build.sh
# result
sha256sum out/*.iso > out/*.iso.sha256
sudo umount $ROOT/build/src $ROOT/build; sudo rm -rf $BUILD/work
```

**Chroot side: `iso/chroot-build.sh`**

```bash
pacman -Syu --needed --cachedir /build/pkgcache base-devel rust pkgconf python python-pillow \
       freetype2 grub archiso gcc-libs glibc libxkbcommon wayland dbus libpulse systemd-libs \
       alsa-lib openssl zlib zstd
useradd -m -U builder                                  # makepkg never runs as root
# as builder, with BUILDDIR=/build/makepkg PKGDEST=/build/repo SRCDEST=/build/srcdest
#   LOGDEST=/build/logs CARGO_TARGET_DIR=/build/cargo-target CARGO_HOME=/build/cargo-home
cd /build/src/linux && makepkg -f -c --noconfirm --log           # lillith (repo PKGBUILD)
cd /build/pkgwork/lillith-llama-bin && makepkg -f -c --noconfirm --log
repo-add /build/repo/demonos-local.db.tar.gz /build/repo/*.pkg.tar.zst
rm -rf /build/cargo-target /build/cargo-home /build/makepkg /build/srcdest /build/pkgwork
pacman -Rns rust                                       # unless KEEP_RUST=1
python3 iso/theme/make-theme-assets.py /build/src /build/theme-out
grub-mkfont -s 16 -n "DemonOS Mono Regular 16" -o .../demonos-mono-16.pf2 JetBrainsMono.ttf
grub-mkfont -s 24 -n "DemonOS Mono Regular 24" -o .../demonos-mono-24.pf2 JetBrainsMono.ttf
cp <art> -> staging grub/themes/demonos, syslinux/splash.png, plymouth, sddm, wallpapers
mkarchiso -v -r -w /build/work -o /build/out /build/staging/profile
#   (once mksquashfs starts, /build/pkgcache/*.pkg.tar.* and /build/repo/*.pkg.tar.* are
#    deleted: all packages are installed by then, and this keeps peak disk ~1.5-3 GB lower)
rm -rf /build/work
```

**Disk use.** Peak is the airootfs (about 6 GB) plus the squashfs (about 3.5
GB). After the build only `out/*.iso` (about 3.5 GB), `logs/`, `packages/` (the
small llama package) and `staging/` are left. The staging dir is mostly the
model hardlink.

## Boot test (QEMU, TCG only)

```bash
iso/test-boot.sh /workspace/qsec/iso-build/out/demonos-phantomcore-*.iso both 2400 60
# -> /workspace/qsec/iso-build/boot-test/<ts>/{uefi,bios}/serial.log, shot-*.png, summary.txt
```

**Never use `-accel kvm` on the build box.** Its nested VMX crashes the host
kernel (`kvm_spurious_fault` / `kernel BUG at arch/x86/kvm/x86.c:702`), and
`test-boot.sh` refuses any accelerator other than `tcg`.

Under TCG a Plasma boot takes many minutes, so give it a generous run time.
* **UEFI** uses `OVMF_CODE_4M.fd` with a private copy of `OVMF_VARS_4M.fd`.
* **BIOS** uses SeaBIOS.
* **Display:** `virtio-vga` (`VGA=std` for bochs).
* **Network:** no NIC by default (`NET=1` gives a user-mode NIC).
* **Monitor:** the HMP monitor is on a unix socket, and `screendump FILE -f png` runs every `SHOT_EVERY` seconds.

## Known limits

* `linux/systemd/lillith.service` has `ExecStart=%h/.local/bin/lillith`. That
  path is right for `install.sh` but wrong for the package (`/usr/bin/lillith`).
  * The ISO doesn't use the unit; it starts Lillith from
    `~/.config/autostart/lillith.desktop`.
  * Fix it upstream later with a drop-in or a package-specific unit.
* `llama.cpp` isn't in the Arch repos. That's why `pkgs/lillith-llama-bin`
  repackages the pinned upstream Ubuntu build, which runs fine on Arch: it needs
  glibc, libstdc++, libgomp, openssl, zlib and zstd.
* The ISO is unsigned, and Secure Boot isn't supported, the same as releng.
