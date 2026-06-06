#!/bin/bash
# Build a Rignite-bootable RunixOS disk WITHOUT root, using mkfs.btrfs --rootdir
# + --subvol (btrfs-progs >= 6.x). No loop mount, so no sudo. Produces the same
# on-disk layout as build_runix_disk.sh: a Core subvolume populated from the
# base-image, the kernel at Core/Startup/vmlinuz-runixos, and rev -> brush for a
# direct shell on boot.
set -e
ROOT="${1:-/home/ren/coding/rovelos/Rocket/output-clean/base-image}"
KERNEL="${2:-/home/ren/coding/KernelFactory/output/linux-7.0.11/arch/x86/boot/bzImage}"
IMG="${3:-disk.img}"
cd "$(dirname "$0")"

[ -d "$ROOT/Core" ] || { echo "no Core in $ROOT"; exit 1; }
[ -f "$KERNEL" ] || { echo "no kernel at $KERNEL"; exit 1; }

# Stage the boot bits into the rootdir (base-image is regenerated each build).
mkdir -p "$ROOT/Core/Startup"
cp -f "$KERNEL" "$ROOT/Core/Startup/vmlinuz-runixos"

# Init: kernel /dev/console resolves to tty0 (fbcon) with the baked cmdline, so
# a headless serial boot sees nothing. Make PID 1 a brush script that runs the
# WPA diag and redirects all output to the serial port, so -nographic captures
# it. (rm the real rev binary first; ln -sf onto an existing file would nest.)
DIAGCMD="${DIAG_CMD:-/Core/Bin/aetherctl wifi diag RunixOS password123}"
rm -f "$ROOT/Core/Bin/rev"
cat > "$ROOT/Core/Bin/rev" <<EOF
#!/Core/Bin/brush
export PATH=/Core/Bin
exec >/dev/ttyS0 2>&1
echo "=== RUNIXOS DIAG INIT ==="
$DIAGCMD
echo "=== DIAG-DONE ==="
while true; do /Core/Bin/sleep 5; done
EOF
chmod +x "$ROOT/Core/Bin/rev"

rm -f "$IMG"
truncate -s 3G "$IMG"
mkfs.btrfs -q -L RunixOS --rootdir "$ROOT" --subvol rw:Core --shrink "$IMG"
echo "disk.img ready (no-sudo, btrfs RunixOS, subvol Core + kernel + brush-init)."
