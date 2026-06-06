#!/bin/bash
# Headless QEMU boot of the RunixOS disk via Rignite (serial console).
timeout "${1:-120}" qemu-system-x86_64 -machine accel=kvm:tcg -smp 2 -m 1536 \
  -drive if=pflash,format=raw,readonly=on,file=bios/OVMF_CODE.4m.fd \
  -drive if=pflash,format=raw,file=bios/OVMF_VARS.4m.fd \
  -drive format=raw,file=fat:rw:target/uefi/esp \
  -drive file=disk.img,if=none,id=drive0,format=raw -device virtio-blk-pci,drive=drive0 \
  -vga virtio -display none -serial stdio -no-reboot
