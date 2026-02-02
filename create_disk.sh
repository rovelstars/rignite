#!/bin/bash

# create a raw disk image of 2G, format it with btrfs filesystem.
# Setup subvolumes @core and @home.
# Enable ZSTD compression globally.
# Disable compression/CoW for Boot directory to ensure compatibility with simple bootloaders.
# This script requires sudo privileges to run.

IMAGE="disk.img"
MOUNT_DIR="mnt_btrfs"

if [ "$EUID" -ne 0 ]; then
  echo "Please run as root (sudo ./create_disk.sh)"
  exit 1
fi

# Clean up previous run
if [ -d "$MOUNT_DIR" ]; then
    if mountpoint -q "$MOUNT_DIR"; then
        umount "$MOUNT_DIR"
    fi
    rm -rf "$MOUNT_DIR"
fi
rm -f "$IMAGE"

echo "Creating 200MB raw disk image..."
qemu-img create -f raw "$IMAGE" 200M

echo "Formatting as Btrfs (Label: RunixOS)..."
mkfs.btrfs --label "RunixOS" "$IMAGE"

echo "Mounting image with zstd compression..."
mkdir -p "$MOUNT_DIR"
# Mount with compress=zstd:1. All new files will be compressed unless configured otherwise.
mount -o loop,compress=zstd:1 "$IMAGE" "$MOUNT_DIR"

echo "Creating Btrfs Subvolumes..."
btrfs subvolume create "$MOUNT_DIR/Core"
btrfs subvolume create "$MOUNT_DIR/Home"

echo "Setting up Boot directory with NoCOW (+C)..."
# Create Boot directory inside Core
mkdir -p "$MOUNT_DIR/Core/Boot"

# Apply NoCOW attribute (+C).
# This disables Copy-on-Write and Compression for this directory and new files inside it.
# Essential for bootloaders that don't support Btrfs compression/encryption.
chattr +C "$MOUNT_DIR/Core/Boot"

echo "Verifying attributes on Boot folder:"
lsattr -d "$MOUNT_DIR/Core/Boot"

echo "Populating Core/Boot..."

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

# echo "Copying ROS Environment..."
# ROS_SRC="/home/ren/ROS"
# if [ -d "$ROS_SRC" ]; then
#     echo "Copying contents of $ROS_SRC to $MOUNT_DIR..."
#     cp -a "$ROS_SRC/." "$MOUNT_DIR/"
#     # Ensure root ownership for system files
#     chown -R root:root "$MOUNT_DIR"
# else
#     echo "WARNING: $ROS_SRC not found. Skipping copy."
# fi

# Ensure standard directories exist if ROS didn't provide them
mkdir -p "$MOUNT_DIR/Core/"{etc,usr,var,bin,sbin,lib64}
mkdir -p "$MOUNT_DIR/Home/user"

install_binary() {
    local bin_name=$1
    local bin_path=$(command -v "$bin_name")

    if [ -z "$bin_path" ]; then
        echo "WARNING: $bin_name not found on host. Skipping."
        return
    fi

    echo "Installing $bin_name from $bin_path..."
    cp "$bin_path" "$MOUNT_DIR/Core/bin/$bin_name"

    # Copy dependencies
    ldd "$bin_path" | grep -o '/[^ ]*' | while read -r lib; do
        # Determine destination dir inside Core, preserving path structure
        # e.g. /usr/lib/libc.so.6 -> $MOUNT_DIR/Core/usr/lib/libc.so.6
        dest_dir="$MOUNT_DIR/Core$(dirname "$lib")"
        mkdir -p "$dest_dir"
        cp "$lib" "$dest_dir/"
    done
}

# Install bash (required)
if ! command -v bash >/dev/null; then
    echo "Error: bash not found on host."
    exit 1
fi
install_binary bash

# Install busybox and binutils (temporarily)
install_binary busybox
install_binary ld
install_binary as
install_binary objdump

# Create the init script
echo "Creating bash init script..."
cat <<EOF > "$MOUNT_DIR/Core/sbin/init"
#!/bin/bash
export PATH=/bin:/sbin:/usr/bin:/usr/sbin:/Core/Bin
echo "Successfully booted into RunixOS (Bash Mode)!"
echo "Dropping to shell..."
exec /bin/bash --login
EOF
chmod +x "$MOUNT_DIR/Core/sbin/init"

# Create root symlinks to point into Core
# This ensures that paths like /bin/bash (used in shebang) resolve to /Core/bin/bash
ln -sf Core/sbin "$MOUNT_DIR/sbin"
ln -sf Core/bin "$MOUNT_DIR/bin"
ln -sf Core/lib "$MOUNT_DIR/lib"
ln -sf Core/lib64 "$MOUNT_DIR/lib64"
ln -sf Core/usr "$MOUNT_DIR/usr"
ln -sf Core/etc "$MOUNT_DIR/etc"

# Add a dummy fstab
echo "# /etc/fstab: static file system information." > "$MOUNT_DIR/Core/etc/fstab"
echo "LABEL=RunixOS / btrfs rw,relatime,compress=zstd:1 0 0" >> "$MOUNT_DIR/Core/etc/fstab"

# Add a dummy hostname
echo "runixos-desktop" > "$MOUNT_DIR/Core/etc/hostname"

# Add some compressed data to verify zstd is working elsewhere
echo "This is some user data that should be compressed by ZSTD." > "$MOUNT_DIR/Home/user/document.txt"
# Create a larger file to ensure compression triggers
for i in {1..1000}; do echo "Repeated text for compression testing $i"; done >> "$MOUNT_DIR/Home/user/large_log.txt"

echo "Listing contents of root (subvolumes are visible as directories):"
ls -F "$MOUNT_DIR"
echo "Listing contents of Core/Boot:"
ls -lh "$MOUNT_DIR/Core/Boot"

echo "Unmounting..."
umount "$MOUNT_DIR"
rm -rf "$MOUNT_DIR"

# Fix permissions so regular user can read it for QEMU
chmod 666 "$IMAGE"

echo "Done. Created $IMAGE with:"
echo "  - Btrfs filesystem (Label: RunixOS)"
echo "  - ZSTD:1 compression enabled globally"
echo "  - Subvolumes: Core, Home"
echo "  - NoCOW/Uncompressed directory: Core/Boot"
echo "  - Linux boot files installed"
