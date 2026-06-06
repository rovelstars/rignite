#!/bin/bash
# Build a Rignite-bootable RunixOS disk the tested way (kernel-written btrfs +
# subvol Core), populated from the base-image rootfs. Run with sudo.
set -e
ROOT="${1:-/home/ren/coding/rovelos/Rocket/output-clean/base-image}"
IMG=disk.img; MNT=mnt_btrfs
[ "$EUID" -eq 0 ] || { echo "run as root: sudo ./build_runix_disk.sh"; exit 1; }
umount "$MNT" 2>/dev/null || true; rm -rf "$MNT" "$IMG"
truncate -s 3G "$IMG"
mkfs.btrfs -L RunixOS "$IMG"
mkdir -p "$MNT"; mount -o loop,compress=zstd:1 "$IMG" "$MNT"
btrfs subvolume create "$MNT/Core"
cp -a "$ROOT/Core/." "$MNT/Core/"
for d in Vault Space Transit Construct dev proc sys; do
    [ -d "$ROOT/$d" ] && cp -a "$ROOT/$d" "$MNT/" || mkdir -p "$MNT/$d"
done
# kernel + init for the boot test
cp /home/ren/ROS/Core/Startup/vmlinuz-7.0.11-runixos-26.2 "$MNT/Core/Startup/vmlinuz-runixos"
ln -sf brush "$MNT/Core/Bin/rev"
sync; umount "$MNT"; rmdir "$MNT"
# Hand the image back to the invoking user so QEMU (run unprivileged) can open it.
[ -n "$SUDO_USER" ] && chown "$SUDO_USER":"$(id -gn "$SUDO_USER")" "$IMG"
echo "disk.img ready (btrfs RunixOS, subvol Core + kernel + init)."
