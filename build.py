import os
import shutil
import subprocess
import sys

TARGET_ARCH = {"x86_64": "x86_64-unknown-uefi", "aarch64": "aarch64-unknown-uefi"}

ASSETS_DIR = "assets"
MEDIA_DIR = "media"
OUTPUT_DIR = "target/uefi"


def run_command(cmd):
    print(f"Running: {cmd}")
    subprocess.check_call(cmd, shell=True)


def qoi_encode(rgba, width, height):
    import struct

    out = bytearray(struct.pack(">4sIIBB", b"qoif", width, height, 4, 1))

    pixels = rgba
    prev = (0, 0, 0, 255)
    index = [(0, 0, 0, 0)] * 64
    run = 0

    # pixels is bytes/bytearray

    for i in range(0, len(pixels), 4):
        px = tuple(pixels[i : i + 4])

        if px == prev:
            run += 1
            if run == 62:
                out.append(0xC0 | (run - 1))
                run = 0
            continue

        if run > 0:
            out.append(0xC0 | (run - 1))
            run = 0

        index_pos = (px[0] * 3 + px[1] * 5 + px[2] * 7 + px[3] * 11) % 64

        if index[index_pos] == px:
            out.append(0x00 | index_pos)
        elif px[3] == prev[3]:
            vr = (px[0] - prev[0]) & 0xFF
            if vr > 127:
                vr -= 256
            vg = (px[1] - prev[1]) & 0xFF
            if vg > 127:
                vg -= 256
            vb = (px[2] - prev[2]) & 0xFF
            if vb > 127:
                vb -= 256

            vg_r = vr - vg
            vb_g = vb - vg

            if -2 <= vr <= 1 and -2 <= vg <= 1 and -2 <= vb <= 1:
                out.append(0x40 | ((vr + 2) << 4) | ((vg + 2) << 2) | (vb + 2))
            elif -32 <= vg <= 31 and -8 <= vg_r <= 7 and -8 <= vb_g <= 7:
                out.append(0x80 | (vg + 32))
                out.append(((vg_r + 8) << 4) | (vb_g + 8))
            else:
                out.extend(b"\xfe" + bytes(px[:3]))
        else:
            out.extend(b"\xff" + bytes(px))

        prev = px
        index[index_pos] = px

    if run > 0:
        out.append(0xC0 | (run - 1))

    out.extend(b"\x00\x00\x00\x00\x00\x00\x00\x01")
    return out


def prepare_assets():
    if not os.path.exists(ASSETS_DIR):
        os.makedirs(ASSETS_DIR)

    # 1. Process Font
    # Just copy the TTF for inclusion
    ttf_src = os.path.join(MEDIA_DIR, "JetBrainsMonoNerdFont-Regular.ttf")
    if os.path.exists(ttf_src):
        shutil.copy(ttf_src, os.path.join(ASSETS_DIR, "font.data"))
    else:
        print("Warning: Font file not found!")

    # 2. Process Icons (multiple icons now)
    icons = [
        ("drive-harddisk-root.svg", "drive.qoi"),
        ("firmware.svg", "firmware.qoi"),
        ("reboot.svg", "reboot.qoi"),
        ("shutdown.svg", "shutdown.qoi"),
        ("logo.svg", "logo.qoi"),
    ]

    for svg_name, raw_name in icons:
        svg_src = os.path.join(MEDIA_DIR, svg_name)
        raw_dst = os.path.join(ASSETS_DIR, raw_name)

        if not os.path.exists(svg_src):
            print(f"Warning: {svg_name} not found, skipping...")
            continue

        png_path = os.path.join(ASSETS_DIR, "temp.png")

        try:
            # Try rsvg-convert for 512x512 (ultra high quality)
            run_command(f"rsvg-convert -w 512 -h 512 -f png -o {png_path} {svg_src}")
        except subprocess.CalledProcessError:
            try:
                # Try convert
                run_command(
                    f"convert -background none -resize 512x512 {svg_src} {png_path}"
                )
            except:
                print(f"Error: Could not convert {svg_name}")
                continue

        if os.path.exists(png_path):
            # Convert PNG to QOI
            from PIL import Image

            try:
                img = Image.open(png_path).convert("RGBA")
                data = img.tobytes()  # RGBA format
                width, height = img.size

                encoded_data = qoi_encode(data, width, height)

                with open(raw_dst, "wb") as f:
                    f.write(encoded_data)
                print(
                    f"Generated {raw_name}: {len(encoded_data)} bytes (Raw: {len(data)})"
                )
            except ImportError:
                print("Error: PIL/Pillow not installed. Cannot convert to QOI.")
        else:
            print(f"Warning: PNG generation failed for {svg_name}")


def build_uefi(arch="x86_64"):
    prepare_assets()
    target = TARGET_ARCH.get(arch)
    if not target:
        print(f"Unknown arch: {arch}")
        return

    # Build
    run_command(f"cargo build --target {target} --release")

    # Create EFI Structure
    efi_boot_dir = os.path.join(OUTPUT_DIR, "esp", "EFI", "BOOT")
    os.makedirs(efi_boot_dir, exist_ok=True)

    src_efi = os.path.join("target", target, "release", "rignite.efi")

    # EFI Naming convention
    if arch == "x86_64":
        dst_efi = os.path.join(efi_boot_dir, "BOOTX64.EFI")
    elif arch == "aarch64":
        dst_efi = os.path.join(efi_boot_dir, "BOOTAA64.EFI")

    shutil.copy(src_efi, dst_efi)
    print(f"Build complete. ESP at {os.path.join(OUTPUT_DIR, 'esp')}")


if __name__ == "__main__":
    if len(sys.argv) > 1:
        build_uefi(sys.argv[1])
    else:
        build_uefi("x86_64")
