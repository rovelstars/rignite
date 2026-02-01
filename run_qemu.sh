#!/bin/bash
ARCH=${1:-x86_64}
ESP_DIR="target/uefi/esp"
OVMF_CODE="bios/OVMF_CODE.4m.fd"
OVMF_VARS="bios/OVMF_VARS.4m.fd"
AAVMF_CODE="bios/AAVMF_CODE.fd"
AAVMF_VARS="bios/AAVMF_VARS.fd"

if [ "$ARCH" == "x86_64" ]; then
    qemu-system-x86_64 \
        -enable-kvm \
        -m 4G -smp 4 \
        -drive if=pflash,format=raw,readonly=on,file=$OVMF_CODE \
        -drive if=pflash,format=raw,file=$OVMF_VARS \
        -drive format=raw,file=fat:rw:$ESP_DIR \
        -drive file=disk.img,if=none,id=drive0,format=raw \
        -device virtio-blk-pci,drive=drive0 \
        -vga std \
        -device qemu-xhci -device usb-tablet -device usb-kbd \
        -display gtk,gl=on -serial stdio \
        -boot menu=on,splash-time=0
elif [ "$ARCH" == "aarch64" ]; then
qemu-system-aarch64 \
    -M virt,highmem=on \
    -accel tcg,thread=multi,tb-size=1024 \
    -cpu max \
    -smp 4 -m 4G \
    -drive if=pflash,format=raw,readonly=on,file=$AAVMF_CODE \
    -drive if=pflash,format=raw,file=$AAVMF_VARS \
    -drive format=raw,file=fat:rw:$ESP_DIR \
    -drive file=disk.img,if=none,id=drive0,format=raw \
    -device virtio-blk-pci,drive=drive0 \
    -vga none -device ramfb \
    -device qemu-xhci -device usb-tablet -device usb-kbd \
    -display gtk,gl=on -serial stdio \
    -boot menu=on,splash-time=0
else
    echo "Unknown arch: $ARCH"
fi
