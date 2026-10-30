#!/usr/bin/env bash
# Boot-test the DemonOS ISO in QEMU with pure software emulation (TCG).
#
#   iso/test-boot.sh ISO [uefi|bios|both] [SECONDS] [SHOT_EVERY]
#   e.g. iso/test-boot.sh /workspace/qsec/iso-build/out/demonos-phantomcore-*.iso both 2400 60
#
# KVM IS NEVER USED: on this box nested VMX crashes the host kernel
# (kvm_spurious_fault). The script refuses ACCEL other than tcg.
# Per run it writes $OUT/<mode>/: serial.log (kernel + systemd on ttyS0),
# shot-NNNN-<secs>s.png (QEMU screendump every SHOT_EVERY s), qemu.log, summary.txt.
set -euo pipefail

ISO="${1:?usage: test-boot.sh ISO [uefi|bios|both] [SECONDS] [SHOT_EVERY]}"
MODE="${2:-both}"
SECS="${3:-2400}"
EVERY="${4:-60}"
ACCEL="${ACCEL:-tcg}"
MEM="${MEM:-4096}"
SMP="${SMP:-4}"
VGA="${VGA:-virtio}"                  # virtio (virtio-gpu KMS) or std (bochs-drm)
OVMF_CODE="${OVMF_CODE:-/usr/share/OVMF/OVMF_CODE_4M.fd}"
OVMF_VARS="${OVMF_VARS:-/usr/share/OVMF/OVMF_VARS_4M.fd}"
OUT="${OUT:-/workspace/qsec/iso-build/boot-test/$(date +%Y%m%d-%H%M%S)}"
NET="${NET:-0}"                       # 1 = user-mode NIC (no host ports opened)

[[ "$ACCEL" == tcg ]] || { echo "refusing ACCEL=$ACCEL: only TCG is allowed (KVM crashes this host)" >&2; exit 2; }
[[ -f "$ISO" ]] || { echo "no such ISO: $ISO" >&2; exit 2; }

mon(){ # $1 = socket, rest = HMP command
  python3 - "$@" <<'PY'
import socket, sys, time
s = socket.socket(socket.AF_UNIX); s.settimeout(20)
s.connect(sys.argv[1]); time.sleep(0.2)
try: s.recv(65536)
except Exception: pass
s.sendall((" ".join(sys.argv[2:]) + "\n").encode()); time.sleep(1.5)
try: print(s.recv(65536).decode(errors="replace"), end="")
except Exception: pass
PY
}

run_one(){ # $1 = uefi|bios
  local m="$1" d="$OUT/$1"; mkdir -p "$d"
  local fw=()
  if [[ "$m" == uefi ]]; then
    cp "$OVMF_VARS" "$d/OVMF_VARS.fd"
    fw=(-drive "if=pflash,format=raw,unit=0,readonly=on,file=$OVMF_CODE"
        -drive "if=pflash,format=raw,unit=1,file=$d/OVMF_VARS.fd")
  fi
  local net=(-nic none); [[ "$NET" == 1 ]] && net=(-nic user,model=virtio-net-pci)
  local vga=(-vga std); [[ "$VGA" == virtio ]] && vga=(-vga none -device virtio-vga)
  echo "[$m] booting $ISO (TCG, ${MEM}M, ${SMP} vCPU, $VGA) for ${SECS}s -> $d"
  qemu-system-x86_64 \
    -accel tcg,thread=multi -cpu max -machine q35 -smp "$SMP" -m "$MEM" \
    "${fw[@]}" \
    -drive "file=$ISO,media=cdrom,readonly=on,if=none,id=cd0" -device ide-cd,drive=cd0,bootindex=0 \
    "${vga[@]}" -display none \
    -device qemu-xhci -device usb-tablet \
    "${net[@]}" \
    -serial "file:$d/serial.log" \
    -monitor "unix:$d/mon.sock,server,nowait" \
    -no-reboot -name "demonos-$m" \
    >"$d/qemu.log" 2>&1 &
  local qpid=$! t=0 n=0
  echo "$qpid" > "$d/qemu.pid"
  sleep 3
  while (( t < SECS )) && kill -0 "$qpid" 2>/dev/null; do
    sleep "$EVERY"; t=$((t + EVERY)); n=$((n + 1))
    mon "$d/mon.sock" screendump "$d/shot-$(printf %04d "$n")-${t}s.png" -f png >/dev/null 2>&1 || true
    printf '[%s] t=%ss shots=%s serial=%sB\n' "$m" "$t" "$n" "$(stat -c %s "$d/serial.log" 2>/dev/null || echo 0)"
  done
  mon "$d/mon.sock" quit >/dev/null 2>&1 || true
  sleep 2; kill "$qpid" 2>/dev/null || true; wait "$qpid" 2>/dev/null || true
  {
    echo "mode=$m iso=$ISO secs=$SECS vga=$VGA mem=$MEM smp=$SMP accel=tcg"
    for pat in 'Linux version' 'archiso' 'Reached target.*Graphical' 'Simple Desktop Display Manager|sddm' \
               'plymouth' 'Failed to start' 'Kernel panic' 'emergency' 'login:'; do
      printf '%-55s %s\n' "$pat" "$(grep -Eci "$pat" "$d/serial.log" 2>/dev/null || true)"
    done
    echo "screenshots: $(ls "$d"/shot-*.png 2>/dev/null | wc -l)"
  } | tee "$d/summary.txt"
}

case "$MODE" in
  uefi|bios) run_one "$MODE" ;;
  both) run_one uefi; run_one bios ;;
  *) echo "mode must be uefi, bios or both" >&2; exit 2 ;;
esac
echo "results in $OUT"
