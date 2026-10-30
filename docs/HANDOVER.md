# Lillith OS + QuantumSec — Phase 5 handover

Branch `quantumsec/integration` holds the finished work: Phases 0–3 plus the DemonOS PhantomCore ISO tooling (Phase 4, PR #13). Everything is **additive** on top of `main`: nothing that existed was removed, hidden or renamed. `main` is untouched; merging it is Ko's call (see the integration → main PR).

Baseline used for the preservation audit: `main` @ `ec00366` (see [`docs/PRESERVATION_MANIFEST.md`](PRESERVATION_MANIFEST.md)).

## 1. Preservation confirmation

Measured with git on the refs named below, not from memory. All counts are identical between the baseline and `quantumsec/integration`.

| What | Baseline `ec00366` | `quantumsec/integration` | Result |
|---|---|---|---|
| Tracked files missing vs baseline | — | **0** | nothing deleted |
| Rust workspace members | 18 | 19 (+ `crates/lillith-quantumsec`) | all 18 kept |
| Icon sets / files (`branding/icons/`) | 15 / 135 | 15 / 135 | unchanged |
| Generated clip folders (`assets/generated/`) | 229 | 229 | unchanged |
| Files under `assets/` (LFS) | 813 | 813 | unchanged |
| Chat/Brain wizard steps | 5: Avatar, Runtime, PickModel, Persona, TestSave | 6: the same 5 + QuantumSec | all 5 kept, 1 added at the end |

### Right-click menu (`crates/lillith-core/src/menu.rs`)

* **Baseline labels (40), all still present:** Poses ▸ (Wave, Gestures, More…), Chat, Avatar ▸ (Lillith, Custom image…), Resize ▸ (Small, Normal, Large, XL, XXL, Bigger, Smaller), Minimize to peek, Mode ▸ (Stay, Wander, Follow), Mood ▸ (Auto, Playful, Happy, Flirty, Mischievous, Alert, Sleepy), Outfit ▸, Mute voice, Pause system monitor, Settings…, Open settings, Icon…, Reset position, About / About Lillith, Close, Off.
* **Baseline-only labels: none.**
* **Added:** a `QuantumSec ▸` submenu (`Status`, `Open panel…`) and a `HOME` label.

### Settings tabs (`crates/lillith-settings-gui/src/tabs/mod.rs`)

* **Baseline:** companion, motion, brain, avatar, icon, voice, build, about, access (sealed I), import (sealed II). All still registered, in the same order.
* **Added:** `quantumsec`, after About and before the sealed add-in slots.

### Also kept

Everything in PRESERVATION_MANIFEST §2–§10 is still there and unchanged except for additions:
* installers (`install.sh`, `linux/install.sh`, PKGBUILD, aio, portable);
* the KWin bridge;
* the Android APK tree;
* CI workflows;
* the Python sidecars;
* the Soul Contract consent gate;
* all docs.

The 31 deleted lines (`main...integration`, without Cargo.lock) are all rewordings or extensions; no feature, item or file was removed. They break down as:
* test counts updated for the new menu items and tab;
* help text and doc comments extended;
* wizard "step N of 5" changed to 6;
* the Aether → Biotek relabels listed in `docs/BIOTEK_TERMINOLOGY_MAP.md`;
* the tray title `Lillith` → `Lillith · DemonOS`;
* the About title `Lillith` kept, with an info rune beside it;
* the consent-card glyphs `⛧ ✕ ✓` changed to `⚠` / plain text, because those glyphs rendered as tofu boxes on the DemonOS font. The buttons and their behaviour are unchanged.

Tests: `cargo test --workspace` with LFS art pulled gives **1397 passed, 0 failed, 2 ignored** (at 20dee27). CI (fmt, clippy `-D warnings`, build, test, all-features clippy, Windows MSVC) is green on integration after PR #14 (formatting only).

### Note on `main` since the baseline

`main` gained 27 commits after `ec00366`: Dream learning (menu: Dream ▸ / Dream learning… / End dream early / Lonely poses) and the Bots settings tab. Merging integration into main auto-merges everything except two doc-comment/help-text conflicts in `lillith-settings-gui` (`bin/lillith-settings.rs`, `tabs/mod.rs`). Each side only added a tab name, so the fix is to keep both (Bots **and** QuantumSec). See the integration → main PR.

## 2. QuantumSec in one paragraph

* `crates/lillith-quantumsec` is the shield. It runs protons (resource load, file integrity, network listeners, …) and writes status to `~/.biotek/quantumsec/status.json`.
* Its other parts:
  * the consent log (`~/.biotek/consent/`);
  * the bot registry;
  * Red/Blue/Purple teams (**red = local simulation only, clearly labelled**);
  * lockdown;
  * the personalization report;
  * systemd user units.
* Every deploy, lockdown, hardening or system change goes through the **blood-seal** dialog. It shows the exact command and needs a clicked Yes. Typed or spoken yes never counts, which is the same rule as the Soul Contract.
* All model servers bind to `127.0.0.1` only and never run as boot services.
* Status levels (`proton.rs`, `poll_resources`):
  * Alert at CPU or memory ≥ 90%, which makes the overall status CRITICAL;
  * Notice at CPU ≥ 70% or memory ≥ 80%, which makes it ELEVATED.
* On an emulated VM (QEMU TCG) the CPU sits at about 100% during login, so `STATUS CRITICAL` there is expected. The thresholds were deliberately left as they are.

## 3. Manifest of changes vs `main`

`git diff --stat main...quantumsec/integration` gives **169 files changed, 7883 insertions(+), 31 deletions(-)**:
* 141 files added;
* 28 files modified, additions plus the rewordings in §1;
* **0 deleted**.

This file, `docs/HANDOVER.md`, was added afterwards and is not counted above.

The diff is measured from the merge-base `ec00366`.

### Added files (`--diff-filter=A`, lines added)

```
    19  crates/lillith-quantumsec/Cargo.toml
   345  crates/lillith-quantumsec/src/consent.rs
    34  crates/lillith-quantumsec/src/lib.rs
   241  crates/lillith-quantumsec/src/lockdown.rs
    81  crates/lillith-quantumsec/src/memory_hook.rs
   333  crates/lillith-quantumsec/src/proton.rs
   216  crates/lillith-quantumsec/src/registry.rs
   220  crates/lillith-quantumsec/src/report.rs
   419  crates/lillith-quantumsec/src/shield.rs
   136  crates/lillith-quantumsec/src/status.rs
   291  crates/lillith-quantumsec/src/teams.rs
   136  crates/lillith-quantumsec/src/units.rs
   590  crates/lillith-settings-gui/src/tabs/quantumsec.rs
   416  crates/lillith-setup/src/biotek.rs
   237  crates/lillith-theme/src/egui_awakening.rs
    91  crates/lillith-theme/src/egui_blood_seal.rs
   116  crates/lillith-theme/src/egui_whisper.rs
   177  docs/BIOTEK_TERMINOLOGY_MAP.md
    47  docs/PHASE1_QUANTUMSEC.md
    31  docs/PHASE2_QUANTUMSEC.md
    27  docs/PHASE3_QUANTUMSEC.md
   142  docs/PRESERVATION_MANIFEST.md
     4  iso/.gitignore
   248  iso/README.md
   103  iso/build-iso.sh
   107  iso/chroot-build.sh
   155  iso/guest-check.py
    36  iso/pkgs/lillith-llama-bin/PKGBUILD
     6  iso/profile/airootfs/etc/group
     6  iso/profile/airootfs/etc/gshadow
     1  iso/profile/airootfs/etc/hostname
     1  iso/profile/airootfs/etc/locale.conf
     1  iso/profile/airootfs/etc/localtime
     3  iso/profile/airootfs/etc/mkinitcpio.conf.d/archiso.conf
     3  iso/profile/airootfs/etc/motd
    13  iso/profile/airootfs/etc/pacman.d/hooks/uncomment-mirrors.hook
    18  iso/profile/airootfs/etc/pacman.d/hooks/zzzz99-remove-custom-hooks-from-airootfs.hook
     2  iso/profile/airootfs/etc/passwd
     4  iso/profile/airootfs/etc/plymouth/plymouthd.conf
     1  iso/profile/airootfs/etc/resolv.conf
    16  iso/profile/airootfs/etc/sddm.conf.d/10-demonos.conf
     2  iso/profile/airootfs/etc/shadow
     8  iso/profile/airootfs/etc/skel/.config/autostart/demonos-awakening.desktop
     7  iso/profile/airootfs/etc/skel/.config/autostart/demonos-firstboot.desktop
     8  iso/profile/airootfs/etc/skel/.config/autostart/lillith.desktop
     3  iso/profile/airootfs/etc/skel/.config/fish/config.fish
     3  iso/profile/airootfs/etc/skel/.config/kscreenlockerrc
    15  iso/profile/airootfs/etc/skel/.config/lillith/config.toml
    27  iso/profile/airootfs/etc/skel/.config/powerdevilrc
     3  iso/profile/airootfs/etc/ssh/sshd_config.d/10-archiso.conf
     4  iso/profile/airootfs/etc/sudoers.d/10-demonos-live
     2  iso/profile/airootfs/etc/systemd/journald.conf.d/volatile-storage.conf
     4  iso/profile/airootfs/etc/systemd/logind.conf.d/do-not-suspend.conf
    22  iso/profile/airootfs/etc/systemd/network/20-ethernet.network
    20  iso/profile/airootfs/etc/systemd/network/20-wlan.network
    19  iso/profile/airootfs/etc/systemd/network/20-wwan.network
     2  iso/profile/airootfs/etc/systemd/networkd.conf.d/ipv6-privacy-extensions.conf
     5  iso/profile/airootfs/etc/systemd/resolved.conf.d/10-demonos-local-only.conf
     4  iso/profile/airootfs/etc/systemd/resolved.conf.d/archiso.conf
     1  iso/profile/airootfs/etc/systemd/system-generators/systemd-gpt-auto-generator
    10  iso/profile/airootfs/etc/systemd/system/choose-mirror.service
     1  iso/profile/airootfs/etc/systemd/system/dbus-org.freedesktop.ModemManager1.service
     1  iso/profile/airootfs/etc/systemd/system/dbus-org.freedesktop.network1.service
     1  iso/profile/airootfs/etc/systemd/system/dbus-org.freedesktop.resolve1.service
     1  iso/profile/airootfs/etc/systemd/system/dbus-org.freedesktop.timesync1.service
     1  iso/profile/airootfs/etc/systemd/system/default.target
     1  iso/profile/airootfs/etc/systemd/system/display-manager.service
     8  iso/profile/airootfs/etc/systemd/system/etc-pacman.d-gnupg.mount
    13  iso/profile/airootfs/etc/systemd/system/livecd-alsa-unmuter.service
    20  iso/profile/airootfs/etc/systemd/system/livecd-talk.service
     1  iso/profile/airootfs/etc/systemd/system/multi-user.target.wants/ModemManager.service
     1  iso/profile/airootfs/etc/systemd/system/multi-user.target.wants/choose-mirror.service
     1  iso/profile/airootfs/etc/systemd/system/multi-user.target.wants/hv_fcopy_daemon.service
     1  iso/profile/airootfs/etc/systemd/system/multi-user.target.wants/hv_kvp_daemon.service
     1  iso/profile/airootfs/etc/systemd/system/multi-user.target.wants/hv_vss_daemon.service
     1  iso/profile/airootfs/etc/systemd/system/multi-user.target.wants/iwd.service
     1  iso/profile/airootfs/etc/systemd/system/multi-user.target.wants/livecd-talk.service
     1  iso/profile/airootfs/etc/systemd/system/multi-user.target.wants/pacman-init.service
     1  iso/profile/airootfs/etc/systemd/system/multi-user.target.wants/systemd-networkd.service
     1  iso/profile/airootfs/etc/systemd/system/multi-user.target.wants/systemd-resolved.service
     1  iso/profile/airootfs/etc/systemd/system/multi-user.target.wants/vboxservice.service
     1  iso/profile/airootfs/etc/systemd/system/multi-user.target.wants/vmtoolsd.service
     1  iso/profile/airootfs/etc/systemd/system/multi-user.target.wants/vmware-vmblock-fuse.service
     1  iso/profile/airootfs/etc/systemd/system/network-online.target.wants/systemd-networkd-wait-online.service
    15  iso/profile/airootfs/etc/systemd/system/pacman-init.service
     1  iso/profile/airootfs/etc/systemd/system/sockets.target.wants/pcscd.socket
     1  iso/profile/airootfs/etc/systemd/system/sockets.target.wants/systemd-networkd.socket
     1  iso/profile/airootfs/etc/systemd/system/sound.target.wants/livecd-alsa-unmuter.service
     1  iso/profile/airootfs/etc/systemd/system/sysinit.target.wants/systemd-time-wait-sync.service
     1  iso/profile/airootfs/etc/systemd/system/sysinit.target.wants/systemd-timesyncd.service
     6  iso/profile/airootfs/etc/systemd/system/systemd-networkd-wait-online.service.d/wait-for-only-one-interface.conf
     6  iso/profile/airootfs/etc/tmpfiles.d/demonos-home.conf
     6  iso/profile/airootfs/etc/xdg/kdeglobals
     3  iso/profile/airootfs/etc/xdg/kscreenlockerrc
    27  iso/profile/airootfs/etc/xdg/powerdevilrc
    43  iso/profile/airootfs/root/.automated_script.sh
     4  iso/profile/airootfs/root/.gnupg/scdaemon.conf
     6  iso/profile/airootfs/root/.zlogin
     5  iso/profile/airootfs/usr/local/bin/Installation_guide
    29  iso/profile/airootfs/usr/local/bin/choose-mirror
     8  iso/profile/airootfs/usr/local/bin/demonos-awakening
     8  iso/profile/airootfs/usr/local/bin/demonos-firstboot
   239  iso/profile/airootfs/usr/local/bin/livecd-sound
     3  iso/profile/airootfs/usr/local/share/livecd-sound/asound.conf.in
   113  iso/profile/airootfs/usr/share/color-schemes/DemonOS.colors
     8  iso/profile/airootfs/usr/share/plasma/shells/org.kde.plasma.desktop/contents/updates/demonos-wallpaper.js
     8  iso/profile/airootfs/usr/share/plymouth/themes/demonos/demonos.plymouth
    56  iso/profile/airootfs/usr/share/plymouth/themes/demonos/demonos.script
    70  iso/profile/airootfs/usr/share/sddm/themes/demonos/Main.qml
    12  iso/profile/airootfs/usr/share/sddm/themes/demonos/metadata.desktop
     2  iso/profile/airootfs/usr/share/sddm/themes/demonos/theme.conf
     8  iso/profile/airootfs/usr/share/wallpapers/DemonOS/metadata.json
     2  iso/profile/bootstrap_packages
     5  iso/profile/efiboot/loader/entries/01-archiso-linux.conf
     5  iso/profile/efiboot/loader/entries/02-archiso-speech-linux.conf
     4  iso/profile/efiboot/loader/entries/03-archiso-memtest86+x64.conf
     3  iso/profile/efiboot/loader/loader.conf
   127  iso/profile/grub/grub.cfg
    85  iso/profile/grub/loopback.cfg
    47  iso/profile/grub/themes/demonos/theme.txt
   119  iso/profile/packages.aarch64
   171  iso/profile/packages.x86_64
   109  iso/profile/pacman.conf
    31  iso/profile/profiledef.sh
    28  iso/profile/syslinux/archiso_head.cfg
    32  iso/profile/syslinux/archiso_pxe-linux.cfg
     5  iso/profile/syslinux/archiso_pxe.cfg
    20  iso/profile/syslinux/archiso_sys-linux.cfg
     8  iso/profile/syslinux/archiso_sys.cfg
    35  iso/profile/syslinux/archiso_tail.cfg
   bin  iso/profile/syslinux/splash.png
    11  iso/profile/syslinux/syslinux.cfg
    80  iso/setup-arch-chroot.sh
   134  iso/test-boot.sh
   176  iso/theme/make-theme-assets.py
    38  linux/fish/biotek_qsec.fish
    11  linux/lillith-quantumsec.desktop
    20  linux/systemd/lillith-quantumsec.service
    20  linux/systemd/lillith.service
     6  linux/systemd/package/lillith-quantumsec.service.d/10-package-path.conf
     8  linux/systemd/package/lillith.service.d/10-package-path.conf
```

### Modified files (`--diff-filter=M`, +added −deleted)

```
+38    -0    Cargo.lock
+3     -0    Cargo.toml
+1     -0    apps/lillith/Cargo.toml
+73    -0    apps/lillith/src/main.rs
+67    -2    crates/lillith-core/src/menu.rs
+12    -0    crates/lillith-memory/src/brain.rs
+15    -0    crates/lillith-memory/src/bridge.rs
+25    -0    crates/lillith-memory/src/bridge_tests.rs
+65    -3    crates/lillith-overlay/src/menu_popup.rs
+5     -0    crates/lillith-overlay/src/run.rs
+1     -0    crates/lillith-settings-gui/Cargo.toml
+3     -1    crates/lillith-settings-gui/src/app.rs
+4     -2    crates/lillith-settings-gui/src/bin/lillith-settings.rs
+8     -1    crates/lillith-settings-gui/src/chrome.rs
+21    -3    crates/lillith-settings-gui/src/tabs/about.rs
+51    -2    crates/lillith-settings-gui/src/tabs/brain.rs
+2     -0    crates/lillith-settings-gui/src/tabs/companion.rs
+6     -3    crates/lillith-settings-gui/src/tabs/mod.rs
+1     -1    crates/lillith-setup/README.md
+2     -1    crates/lillith-setup/src/config.rs
+1     -0    crates/lillith-setup/src/lib.rs
+10    -8    crates/lillith-theme/src/egui_contract.rs
+8     -0    crates/lillith-theme/src/lib.rs
+23    -0    crates/lillith-theme/src/palette.rs
+74    -4    crates/lillith-tray/src/lib.rs
+8     -0    linux/PKGBUILD
+21    -0    linux/install.sh
+2     -0    linux/lillith.install
```

## 4. Launch and test

All commands run from the repo root on Arch, Garuda or Debian with Rust ≥ 1.85.

```sh
git lfs pull                                  # art/clips; needed for the image tests
cargo run -p lillith                          # the avatar (overlay + brain + chat + QuantumSec shield)
cargo run -p lillith -- quantumsec --once     # one shield poll: prints a summary, writes ~/.biotek/quantumsec/status.json
cargo run -p lillith -- quantumsec            # shield poll loop (what the user unit runs); --interval SECS
cargo run -p lillith-settings-gui --bin lillith-settings -- --tab quantumsec          # the QuantumSec settings tab
cargo run -p lillith-settings-gui --bin lillith-settings -- --tab brain --step quantumsec   # wizard step 6
cargo test --workspace                        # 1397 passed, 0 failed, 2 ignored at 20dee27
```

Installed binaries (`linux/install.sh` puts them in `~/.local/bin`; the package puts them in `/usr/bin`):

```sh
lillith quantumsec --once
lillith-settings --tab quantumsec
lillith-settings --tab quantumsec --step awakening   # also: blood-seal | whisper | teams | consent | unit-enable
```

User units are installed but **never enabled automatically**:

```sh
systemctl --user enable --now lillith-quantumsec.service   # shield loop → ~/.biotek/quantumsec/status.json
systemctl --user enable --now lillith.service              # avatar as a unit (instead of the autostart entry)
systemctl --user status lillith-quantumsec.service
systemctl --user disable --now lillith-quantumsec.service  # off again
```

* The shared units in `linux/systemd/` point at `%h/.local/bin/lillith`, which is the portable install.
* The Arch package adds `/usr/lib/systemd/user/<unit>.d/10-package-path.conf` drop-ins (from `linux/systemd/package/`) that change `ExecStart` to `/usr/bin/lillith`.
* Both kinds of install use the same unit names.

## 5. Rebuild the ISO

Run on a Linux host with at least 10 GB free, as root (or with sudo). The build runs inside an Arch bootstrap chroot, so the host doesn't need to be Arch.

```sh
sudo iso/setup-arch-chroot.sh                # one-off: verified Arch bootstrap chroot + archiso + grub
git lfs pull
sudo iso/build-iso.sh                           # makepkg lillith + lillith-llama-bin, then mkarchiso; deletes work/ at the end
#   SKIP_PACKAGES=1 sudo iso/build-iso.sh        # reuse the packages already built in the local repo
iso/test-boot.sh out/demonos-phantomcore-*.iso both   # QEMU TCG (never KVM) UEFI + BIOS boot test, screendumps + in-guest checks
sudo iso/setup-arch-chroot.sh umount
```

The paths used on the build box were:
* chroot in `/workspace/qsec/arch-root` (override with `ARCH_BASE=`, `ARCH_ROOT=`);
* output in `/workspace/qsec/iso-build/{out,boot-test,logs}/`.

`iso/README.md` has every environment knob (`MIN_START_FREE_MB`, `MEM`, `SMP`, `CHECK_AFTER`, …).

## 6. Boot proof (2026-10-30)

* **ISO:** `demonos-phantomcore-2026.10.30-x86_64.iso`
  * 4,161,077,248 bytes (3.9 GiB)
  * sha256 `5ce04e868167a1265f5630f918c6982a259b3c5b82ed8fd3d8c8c2b29a704df3`
  * built 07:56 MDT from `quantumsec/iso` @ `bb50f4f`: lillith = integration `20dee27` + the unit drop-ins
  * on the build box at `/workspace/qsec/iso-build/out/`
* **Previous build:** sha256 `6411be1f…` (06:06 MDT), replaced.
* **Boot tests:** QEMU **TCG** (never KVM), 3 GiB RAM, 4 vCPU, virtio-gpu. Run dir: `/workspace/qsec/iso-build/boot-test/20261030-080005/{uefi,bios}/`.

| Stage | UEFI (OVMF) | BIOS (SeaBIOS) |
|---|---|---|
| Boot menu (GRUB / syslinux, themed) | `uefi/shot-0002-12s.png` | `bios/shot-0001-6s.png` |
| Plymouth "AWAKENING THE CORE" | `uefi/shot-0006-36s.png` | `bios/shot-0006-36s.png` |
| Lillith on the desktop | `uefi/shot-0025-150s.png` | `bios/shot-0022-132s.png` |
| Settings / QuantumSec tab | `uefi/shot-0027-162s.png` | `bios/shot-0028-168s.png` |
| QuantumSec awakening dialog | `uefi/shot-0029-174s.png` | — |
| Desktop still live after 14+ min (no blanking) | `uefi/shot-0041-840s.png`, last shot `shot-0044-1020s.png` = SCREEN-LIVE | last shot `shot-0043-960s.png` = SCREEN-LIVE |

* SDDM is configured, but autologin goes straight to Plasma, so there is no greeter frame.
* In-guest checks (`guest-check.txt` in each run dir), driven over the serial console. **All PASS on both firmwares:**
  * desktop session;
  * lillith running as demon;
  * `~/.biotek`;
  * awakening armed;
  * `lillith quantumsec --once` exit 0;
  * user units present but not enabled;
  * `systemctl --user enable --now lillith-quantumsec.service` → `active`, ExecStart `/usr/bin/lillith quantumsec` (via the package drop-in), `status.json` mtime advancing;
  * powerdevil DPMS/dim off and autolock off;
  * LLMNR off (no :5355);
  * local model (qwen2.5-0.5b, llama-server) answers on `127.0.0.1:8080` and is bound to loopback only.
* The UEFI run used the earlier combined LLMNR+mDNS check, which FAILed on mDNS only. LLMNR showed `-LLMNR` with no :5355 listener. See the limitations below.

## 7. Known limitations

* **mDNS still on in the ISO.** systemd-resolved listens on `:5353` because releng's `resolved.conf.d/archiso.conf` (`MulticastDNS=yes`) sorts after the DemonOS drop-in. LLMNR is off. Fix for the next build: rename the drop-in to `zz-…`.
* **STATUS CRITICAL on VMs.** Under QEMU TCG the CPU is pinned at about 100%, so Resource Load ≥ 90% raises an Alert. This is expected and the thresholds were not changed (§2).
* **"2 missing" under File Integrity** on a fresh live user. Those are watched config files that don't exist yet. It is a notice only.
* **The ISO ships lillith from integration `20dee27`.** It does not include `main`'s newer Dream learning / Bots work. Rebuild after integration → main is merged if the ISO should carry it.
* **No KVM in the tests** (it crashes this host). TCG boots take about 2.5 min to reach the desktop and about 16 min for a full test run.
* **The ISO is not signed** (no Secure Boot shim). It is not uploaded anywhere and lives only on the build box.
* **Red team is local simulation only.** Nothing in the ISO attacks or scans other hosts.
* **CI Windows job** takes about 18 min. The fmt/clippy fixes needed for green CI went in via PR #14 (formatting only).
