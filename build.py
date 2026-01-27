import os
import subprocess
import shutil
import sys

TARGET_ARCH = {
    "x86_64": "x86_64-unknown-uefi",
    "aarch64": "aarch64-unknown-uefi"
}

ASSETS_DIR = "assets"
MEDIA_DIR = "media"
OUTPUT_DIR = "target/uefi"

def run_command(cmd):
    print(f"Running: {cmd}")
    subprocess.check_call(cmd, shell=True)

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
        ("drive-harddisk-root.svg", "drive.raw"),
        ("firmware.svg", "firmware.raw"),
        ("reboot.svg", "reboot.raw"),
        ("shutdown.svg", "shutdown.raw"),
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
                run_command(f"convert -background none -resize 512x512 {svg_src} {png_path}")
            except:
                print(f"Error: Could not convert {svg_name}")
                continue
        
        if os.path.exists(png_path):
            # Convert PNG to Raw RGBA
            from PIL import Image
            try:
                img = Image.open(png_path).convert("RGBA")
                data = img.tobytes() # RGBA format
                with open(raw_dst, "wb") as f:
                    f.write(data)
                print(f"Generated {raw_name}: {len(data)} bytes")
            except ImportError:
                 print("Error: PIL/Pillow not installed. Cannot convert to raw.")
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
