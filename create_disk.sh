#!/bin/bash

# create a raw disk image of 2G, format it with btrfs filesystem.
# mount it to temporary directory and add Linux boot files.
# This script requires sudo privileges to run.

IMAGE="disk.img"
MOUNT_DIR="mnt_btrfs"

if [ "$EUID" -ne 0 ]; then
  echo "Please run as root (sudo ./create_disk.sh)"
  exit 1
fi

# Clean up previous run
if [ -d "$MOUNT_DIR" ]; then
    umount "$MOUNT_DIR" 2>/dev/null
    rm -rf "$MOUNT_DIR"
fi
rm -f "$IMAGE"

echo "Creating 2G raw disk image..."
qemu-img create -f raw "$IMAGE" 2G

echo "Formatting as Btrfs..."
mkfs.btrfs "$IMAGE"

echo "Mounting image..."
mkdir -p "$MOUNT_DIR"
mount -o loop "$IMAGE" "$MOUNT_DIR"

echo "Creating /Core/Boot structure..."
mkdir -p "$MOUNT_DIR/Core/Boot"

# Define paths to copy from
KERNEL_SRC="/boot/vmlinuz-linux"
INITRD_SRC="/boot/initramfs-linux.img"

# Copy Kernel
if [ -f "$KERNEL_SRC" ]; then
    echo "Copying kernel from $KERNEL_SRC..."
    cp "$KERNEL_SRC" "$MOUNT_DIR/Core/Boot/vmlinuz-linux"
else
    # Fallback for systems that might name it differently (e.g. Ubuntu/Debian)
    if [ -f "/boot/vmlinuz" ]; then
        echo "Copying kernel from /boot/vmlinuz..."
        cp "/boot/vmlinuz" "$MOUNT_DIR/Core/Boot/vmlinuz-linux"
    else
        echo "WARNING: Kernel not found! Creating dummy file for testing."
        echo "DUMMY KERNEL" > "$MOUNT_DIR/Core/Boot/vmlinuz-linux"
    fi
fi

# Copy Initramfs
if [ -f "$INITRD_SRC" ]; then
    echo "Copying initramfs from $INITRD_SRC..."
    cp "$INITRD_SRC" "$MOUNT_DIR/Core/Boot/initramfs-linux.img"
else
    # Fallback
    if [ -f "/boot/initrd.img" ]; then
        echo "Copying initramfs from /boot/initrd.img..."
        cp "/boot/initrd.img" "$MOUNT_DIR/Core/Boot/initramfs-linux.img"
    else
        echo "WARNING: Initramfs not found! Creating dummy file for testing."
        echo "DUMMY INITRAMFS" > "$MOUNT_DIR/Core/Boot/initramfs-linux.img"
    fi
fi

echo "Listing contents:"
ls -R "$MOUNT_DIR"

echo "Unmounting..."
umount "$MOUNT_DIR"
rm -rf "$MOUNT_DIR"

# Fix permissions so regular user can read it for QEMU
chmod 666 "$IMAGE"

echo "Done. Created $IMAGE with Btrfs and Linux boot files."
