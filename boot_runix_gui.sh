#!/bin/bash
# GUI boot: QEMU opens its own window and renders the RunixOS serial console
# inside it (-serial vc). The kernel still talks ttyS0; QEMU draws that serial
# in a virtual-console tab, so this does NOT depend on the host terminal at all.
# The kernel has no framebuffer console (no efifb/fbcon), so the VGA tab stays
# blank under UEFI - use the serial0 tab.
cd "$(dirname "$0")" || exit 1

for f in bios/OVMF_CODE.4m.fd bios/OVMF_VARS.4m.fd disk.img target/uefi/esp/EFI/BOOT/BOOTX64.EFI; do
  [ -e "$f" ] || { echo "missing: $f (build it first)"; exit 1; }
done

echo ">>> opening QEMU window. In it: View menu -> serial0 to see the RunixOS console (the VGA tab stays blank: no fbcon)."
exec qemu-system-x86_64 -machine accel=kvm:tcg -smp 2 -m 1536 \
  -drive if=pflash,format=raw,readonly=on,file=bios/OVMF_CODE.4m.fd \
  -drive if=pflash,format=raw,file=bios/OVMF_VARS.4m.fd \
  -drive format=raw,file=fat:rw:target/uefi/esp \
  -drive file=disk.img,if=none,id=drive0,format=raw -device virtio-blk-pci,drive=drive0 \
  -vga std -display gtk -serial vc -no-reboot
