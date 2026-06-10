#!/bin/bash
# Boot the REAL rev daemon + an unprivileged WireBus seat client in QEMU with a
# virtio-gpu and keyboard. The orchestrator is PID 1; it starts rev, then the
# client connects over WireBus, gets device fds, and renders a frame.
set -e
cd "$(dirname "$0")"

REV=../../rev/target/release/rev
INIT=target/release/orchestrator
CLIENT=target/release/seat-client
for b in "$REV" "$INIT" "$CLIENT"; do
  [ -x "$b" ] || { echo "missing $b -- build rev (cargo build --release) and this crate first"; exit 1; }
done

KVER=$(uname -r)
MODDIR=/lib/modules/$KVER/kernel
case "$KVER" in
  *cachyos) KERNEL=/boot/vmlinuz-linux-cachyos ;;
  *)        KERNEL=/boot/vmlinuz-linux ;;
esac

STAGE=$(mktemp -d)
INITRD=$(mktemp --suffix=.cpio.gz)
trap 'rm -rf "$STAGE" "$INITRD"' EXIT

mkdir -p "$STAGE"/{proc,sys,dev,run}
cp "$INIT"   "$STAGE/init"
cp "$REV"    "$STAGE/rev"
cp "$CLIENT" "$STAGE/seat-client"
zstd -dqf "$MODDIR/drivers/virtio/virtio_dma_buf.ko.zst"     -o "$STAGE/virtio_dma_buf.ko"
zstd -dqf "$MODDIR/drivers/gpu/drm/virtio/virtio-gpu.ko.zst" -o "$STAGE/virtio-gpu.ko"
zstd -dqf "$MODDIR/drivers/virtio/virtio_input.ko.zst"       -o "$STAGE/virtio_input.ko"

# Bundle every shared lib + the dynamic loader the three binaries need, at their
# real absolute paths so the loader finds them inside the initramfs.
for b in "$STAGE/init" "$STAGE/rev" "$STAGE/seat-client"; do
  ldd "$b" | while read -r line; do
    lib=$(echo "$line" | grep -oE '/[^ ]+\.so[^ ]*' | head -n1)
    [ -n "$lib" ] && [ -e "$lib" ] || continue
    dest="$STAGE$lib"
    mkdir -p "$(dirname "$dest")"
    [ -e "$dest" ] || cp "$lib" "$dest"
  done
done

( cd "$STAGE" && find . -print0 | cpio --null -o -H newc 2>/dev/null | gzip -1 ) > "$INITRD"

echo ">>> kernel: $KERNEL"
echo ">>> booting QEMU (real rev daemon + WireBus seat client)..."
timeout 90 qemu-system-x86_64 \
  -machine accel=kvm:tcg -cpu host -smp 2 -m 1024 \
  -kernel "$KERNEL" -initrd "$INITRD" \
  -append "console=ttyS0 panic=1 quiet" \
  -device virtio-gpu-pci \
  -device virtio-keyboard-pci \
  -nographic -no-reboot 2>&1
