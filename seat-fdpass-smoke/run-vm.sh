#!/bin/bash
# Build a throwaway initramfs around the smoke binary and boot it in QEMU with a
# virtio-gpu, root, and a serial console. The smoke binary is PID 1: it loads
# virtio-gpu, runs the fd-pass / DRM-master validation, prints, and powers off.
set -e
cd "$(dirname "$0")"

BIN=target/x86_64-unknown-linux-gnu/release/seat-fdpass-smoke
[ -x "$BIN" ] || { echo "build first: RUSTFLAGS='-C target-feature=+crt-static' cargo build --release --target x86_64-unknown-linux-gnu"; exit 1; }

KVER=$(uname -r)
MODDIR=/lib/modules/$KVER/kernel
# Kernel image must match the running modules' vermagic, else finit_module
# rejects them with ENOEXEC. uname -r picks the live kernel; map it to /boot.
case "$KVER" in
  *cachyos) KERNEL=/boot/vmlinuz-linux-cachyos ;;
  *)        KERNEL=/boot/vmlinuz-linux ;;
esac

STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE"/{proc,sys,dev}
cp "$BIN" "$STAGE/init"
zstd -dqf "$MODDIR/drivers/virtio/virtio_dma_buf.ko.zst"   -o "$STAGE/virtio_dma_buf.ko"
zstd -dqf "$MODDIR/drivers/gpu/drm/virtio/virtio-gpu.ko.zst" -o "$STAGE/virtio-gpu.ko"

INITRD=$(mktemp --suffix=.cpio.gz)
trap 'rm -rf "$STAGE" "$INITRD"' EXIT
( cd "$STAGE" && find . -print0 | cpio --null -o -H newc 2>/dev/null | gzip -1 ) > "$INITRD"

echo ">>> kernel: $KERNEL"
echo ">>> booting QEMU (virtio-gpu, root, serial)..."
timeout 60 qemu-system-x86_64 \
  -machine accel=kvm:tcg -cpu host -smp 2 -m 1024 \
  -kernel "$KERNEL" -initrd "$INITRD" \
  -append "console=ttyS0 panic=1 quiet" \
  -device virtio-gpu-pci \
  -nographic -no-reboot 2>&1
