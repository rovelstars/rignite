#!/bin/bash
# Headless QEMU boot of the RunixOS disk via Rignite (serial console).
# Runs from anywhere: all paths are resolved relative to this script's dir.
cd "$(dirname "$0")" || exit 1

for f in bios/OVMF_CODE.4m.fd bios/OVMF_VARS.4m.fd disk.img target/uefi/esp/EFI/BOOT/BOOTX64.EFI; do
  [ -e "$f" ] || { echo "missing: $f (build it first)"; exit 1; }
done

echo ">>> booting RunixOS in QEMU (serial console; Ctrl-A X to quit)..."
timeout "${1:-0}" qemu-system-x86_64 -machine accel=kvm:tcg -smp 2 -m 1536 \
  -drive if=pflash,format=raw,readonly=on,file=bios/OVMF_CODE.4m.fd \
  -drive if=pflash,format=raw,file=bios/OVMF_VARS.4m.fd \
  -drive format=raw,file=fat:rw:target/uefi/esp \
  -drive file=disk.img,if=none,id=drive0,format=raw -device virtio-blk-pci,drive=drive0 \
  -nographic -no-reboot
