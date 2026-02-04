#!/bin/bash
ARCH=${1:-x86_64}
ESP_DIR="target/uefi/esp"
OVMF_CODE="bios/OVMF_CODE.4m.fd"
OVMF_VARS="bios/OVMF_VARS.4m.fd"
AAVMF_CODE="bios/AAVMF_CODE.fd"
AAVMF_VARS="bios/AAVMF_VARS.fd"

# Grant permissions to USB device (OnePlus)
# Find any device with Vendor ID 22d9 (OnePlus)
USB_INFO=$(lsusb -d 22d9:)
if [ ! -z "$USB_INFO" ]; then
    # Extract PID (e.g., from "ID 22d9:2769", take 2769)
    USB_PID=$(echo "$USB_INFO" | head -n1 | awk '{print $6}' | cut -d: -f2)
    # Extract Device Path for chmod
    USB_DEV=$(echo "$USB_INFO" | head -n1 | awk '{print "/dev/bus/usb/"$2"/"$4}' | tr -d ':')

    echo "Found OnePlus Device: PID=0x$USB_PID at $USB_DEV"
    echo "Granting permissions..."
    sudo chmod 666 "$USB_DEV"
else
    echo "Warning: OnePlus device not found. USB Passthrough might fail."
    USB_PID="2769" # Fallback
fi

if [ "$ARCH" == "x86_64" ]; then
    qemu-system-x86_64 \
        -enable-kvm \
        -smp 4 -m 1G \
        -drive if=pflash,format=raw,readonly=on,file=$OVMF_CODE \
        -drive if=pflash,format=raw,file=$OVMF_VARS \
        -drive format=raw,file=fat:rw:$ESP_DIR \
        -drive file=disk.img,if=none,id=drive0,format=raw \
        -device virtio-blk-pci,drive=drive0 \
        -vga virtio \
        -device qemu-xhci -device usb-tablet -device usb-kbd \
        -device usb-host,vendorid=0x22d9,productid=0x$USB_PID,guest-reset=false \
        -device usb-host,vendorid=0x18d1,guest-reset=false \
        -monitor unix:qemu-monitor.sock,server,nowait \
        -display gtk,gl=on -serial stdio \
        -boot menu=on,splash-time=0
elif [ "$ARCH" == "aarch64" ]; then
qemu-system-aarch64 \
    -M virt,highmem=on \
    -accel tcg,thread=multi,tb-size=1024 \
    -cpu max \
    -smp 4 -m 1G \
    -drive if=pflash,format=raw,readonly=on,file=$AAVMF_CODE \
    -drive if=pflash,format=raw,file=$AAVMF_VARS \
    -drive format=raw,file=fat:rw:$ESP_DIR,if=none,id=esp \
    -device virtio-blk-pci,drive=esp,bootindex=0 \
    -drive file=disk.img,if=none,id=drive0,format=raw \
    -device virtio-blk-pci,drive=drive0 \
    -vga none -device virtio-gpu-pci \
    -device qemu-xhci -device usb-tablet -device usb-kbd \
    -device usb-host,vendorid=0x22d9,productid=0x$USB_PID,guest-reset=false \
    -device usb-host,vendorid=0x18d1,guest-reset=false \
    -monitor unix:qemu-monitor.sock,server,nowait \
    -display gtk,gl=off -serial stdio \
    -boot menu=on,splash-time=0
else
    echo "Unknown arch: $ARCH"
fi
