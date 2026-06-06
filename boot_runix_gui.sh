#!/bin/bash
# GUI boot: QEMU opens a window showing the RunixOS console directly. The kernel
# now has a framebuffer console (simpledrm/efifb + fbcon) and boots with
# console=tty0, so the shell renders in the window with a working keyboard,
# arrows, history and Ctrl-C. A copy of the kernel log also goes to ttyS0
# (/tmp/runix-boot.log) for debugging.
cd "$(dirname "$0")" || exit 1

for f in bios/OVMF_CODE.4m.fd bios/OVMF_VARS.4m.fd disk.img target/uefi/esp/EFI/BOOT/BOOTX64.EFI; do
  [ -e "$f" ] || { echo "missing: $f (build it first)"; exit 1; }
done

echo ">>> opening QEMU window. Click into it; the RunixOS shell is on screen."
echo ">>> kernel log copy: /tmp/runix-boot.log   (quit: close window or Ctrl-C here)"
exec qemu-system-x86_64 -machine accel=kvm:tcg -smp 2 -m 1536 \
  -drive if=pflash,format=raw,readonly=on,file=bios/OVMF_CODE.4m.fd \
  -drive if=pflash,format=raw,file=bios/OVMF_VARS.4m.fd \
  -drive format=raw,file=fat:rw:target/uefi/esp \
  -drive file=disk.img,if=none,id=drive0,format=raw -device virtio-blk-pci,drive=drive0 \
  -vga virtio -display gtk -serial file:/tmp/runix-boot.log -no-reboot
