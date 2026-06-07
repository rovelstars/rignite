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

# Init selection:
# - Default: boot the REAL Rev (PID 1) shipped in the base image, which mounts
#   the pseudo-filesystems, runs OOBE if there is no account, then a login
#   session on the console (tty0 / fbcon). Boot the GUI (boot_runix_gui.sh) to
#   interact.
# - DIAG_CMD set: replace rev with a brush script that runs that command and
#   redirects to the serial port, for headless wifi/diag testing on -nographic.
if [ -n "$DIAG_CMD" ]; then
    rm -f "$ROOT/Core/Bin/rev"
    cat > "$ROOT/Core/Bin/rev" <<EOF
#!/Core/Bin/brush
export PATH=/Core/Bin
exec >/dev/ttyS0 2>&1
echo "=== RUNIXOS DIAG INIT ==="
$DIAG_CMD
echo "=== DIAG-DONE ==="
while true; do /Core/Bin/sleep 5; done
EOF
    chmod +x "$ROOT/Core/Bin/rev"
    echo "(diag init: $DIAG_CMD)"
else
    [ -x "$ROOT/Core/Bin/rev" ] || { echo "no real rev binary in base image"; exit 1; }
    echo "(real Rev init: OOBE + login on console)"
fi

rm -f "$IMG"
truncate -s 3G "$IMG"
mkfs.btrfs -q -L RunixOS --rootdir "$ROOT" --subvol rw:Core --shrink "$IMG"
echo "disk.img ready (no-sudo, btrfs RunixOS, subvol Core + kernel + brush-init)."
