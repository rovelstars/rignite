# Rignite

Rignite is a UEFI bootloader written in Rust, designed to provide a fast and reliable boot process for RunixOS. It runs in a `#![no_std]` environment, targeting the `x86_64-unknown-uefi` and `aarch64-unknown-uefi` bare-metal targets.

The final release binary is under 620 KB and includes a graphical boot menu, font rendering, icon display, Btrfs filesystem parsing, and a custom binary configuration format.

---

## Features

- **Graphical Boot Menu**: A GPU-accelerated UI rendered through the UEFI Graphics Output Protocol (GOP). The display pipeline uses a backbuffer that is flushed to the framebuffer each frame, supporting both direct framebuffer write and `BltBufferToVideo` pixel formats.
- **Smooth Animations**: Logo fade-in and fade-out animations at approximately 60 FPS using `uefi::boot::stall`. Menu item selection uses smooth scale interpolation.
- **Auto-Boot**: On startup, Rignite scans all `BlockIO` handles for a Btrfs partition labeled `RunixOS`. If found, it auto-boots after a 2-second splash timeout without entering the menu.
- **Interactive Splash**: During the splash screen, pressing Up and Down simultaneously enters a confirmation state. A second chord press within 5 seconds opens the boot menu.
- **Hierarchical Boot Menu**: The menu has four states: primary drive list, physical drive selection, partition selection, and EFI file selection. Navigation supports arrow keys and WASD.
- **Linux Boot**: Loads a Linux kernel and initramfs directly from a Btrfs partition using the `EFI_LOAD_FILE2_PROTOCOL` for initrd handoff and `EFI_LOADED_IMAGE_PROTOCOL` for kernel chainloading.
- **EFI Chainloading**: Can boot arbitrary EFI applications from any FAT-formatted partition.
- **Btrfs Filesystem Driver**: A from-scratch Btrfs reader that parses the superblock, chunk tree, and B-tree nodes to locate and read files. Used to load the kernel and initramfs from the root partition.
- **RBC Configuration**: A custom binary configuration format (Rignite Binary Config) stored at `\EFI\RovelStars\CONF\boot.rbc` on the ESP. Encodes partition UUIDs, filesystem types, and kernel command-line parameters for both the main and recovery partitions using a TLV (tag-length-value) atom structure.
- **Firmware Settings**: Sets the `OsIndications` UEFI runtime variable to trigger a reboot into the platform firmware setup UI.
- **Procedurally Rendered Logo**: The Rignite logo is rendered entirely from polygon data using a ray-casting point-in-polygon algorithm with 2x2 supersampling for anti-aliasing. No image assets are needed for the logo.
- **Custom Logger**: A minimal in-memory ring buffer logger (up to 50 entries) that also forwards messages to the UEFI `ConOut`. Replaces the `log` crate entirely to reduce binary size.

---

## Binary Size Optimization

Getting the release binary to under 620 KB in a `no_std` Rust environment required a series of deliberate decisions.

### Release Profile

The `Cargo.toml` release profile uses:

```Rignite/Cargo.toml#L17-22
[profile.release]
panic = "abort"
lto = true
opt-level = "z"
codegen-units = 1
strip = true
```

- `opt-level = "z"` optimizes for binary size rather than speed.
- `lto = true` enables link-time optimization, allowing the linker to eliminate dead code across crate boundaries.
- `codegen-units = 1` compiles everything in a single unit, giving LTO the most visibility.
- `strip = true` removes debug symbols from the final EFI binary.
- `panic = "abort"` removes the unwinding machinery, which is substantial in size.

These settings together brought the binary from an initial ~844 KB (after switching to `fontdue`) back down to that range and eventually lower.

### Removing the `log` Crate

The `log` crate adds overhead through its abstractions and dispatch mechanism. It was replaced with four simple macros (`debug!`, `info!`, `warn!`, `error!`) that call directly into a minimal in-memory logger in `src/logger.rs`. This saved approximately 20 KB.

### Font Subsetting

The project uses JetBrains Mono as its UI font. The full TTF is 268 KB. It is subsetted to only the printable ASCII range (U+0020-U+007E), with hinting tables, OpenType substitution/positioning tables, and other unnecessary tables stripped:

```/dev/null/subset.sh#L1-5
pyftsubset JBMR.ttf \
    --unicodes="U+0020-007E" \
    --no-hinting \
    --desubroutinize \
    --drop-tables+=DSIG,GPOS,GSUB,gasp,hdmx,LTSH,VDMX \
    --output-file=JBMR_ultra.ttf
```

This produces a 12 KB font file, down from 268 KB. The font is embedded into the binary at compile time via `include_bytes!`.

### QOI Icons Instead of PNG

Icons (drive, firmware, reboot, shutdown) are SVGs converted to PNG using `rsvg-convert`, then encoded to QOI format by the `build.py` script. QOI compresses better than PNG for pixel-art-style icons and is significantly faster to decode with no heap allocations beyond the output buffer. The `qoi` crate is included with `default-features = false`.

### Procedural Logo Rendering

The Rignite logo SVG was simplified to a set of three polygons that can be described by normalized vertex coordinates. Instead of bundling any image asset for the logo, it is rendered at runtime by rasterizing those polygons directly to the framebuffer. This eliminated all logo image data from the binary entirely.

### Font Rendering: `fontdue` over `ab_glyph`

The font renderer uses `fontdue` with `default-features = false`. Compared to `ab_glyph`, `fontdue` offers faster rasterization and lower runtime memory usage. The initial switch added about 30 KB due to hashmap internals, which was more than recovered by the release profile settings above.

---

## Project Structure

```Rignite/src/main.rs#L19-27
mod boot;
mod font;
mod fs;
mod graphics;
mod icons;
mod input;
mod logger;
mod logo;
mod rbc;
```

| Module | Description |
|---|---|
| `main.rs` | Entry point (`efi_main`), splash screen, boot menu state machine, and action dispatch |
| `boot/mod.rs` | Linux kernel loading, initrd protocol installation, EFI chainloading, and Btrfs-based boot |
| `fs/btrfs.rs` | Btrfs superblock, chunk tree, and B-tree parser for reading files off a raw block device |
| `graphics.rs` | `UefiDisplay` wrapping GOP with a backbuffer and `embedded-graphics` `DrawTarget` integration |
| `font.rs` | `FontRenderer` using `fontdue` for glyph rasterization with sub-pixel positioning |
| `icons.rs` | QOI icon decoder and Catmull-Rom bicubic scaled drawing with alpha blending |
| `logo.rs` | Procedural polygon rasterizer for the Rignite logo with gradient fill and 2x2 supersampling |
| `input.rs` | Thin wrapper around the UEFI `Input` protocol |
| `logger.rs` | In-memory ring buffer logger with `debug!`, `info!`, `warn!`, `error!` macros |
| `rbc.rs` | RBC binary config parser, zero-copy `ConfigView`, and UEFI file loader |

### Supporting Tools

- **`rbc_cli/`**: A host-side Rust CLI that reads a `config.toml` and produces the binary `boot.rbc` file to be placed on the ESP. Run automatically by `build.py` during builds.
- **`host_test_tool/`**: A host-side test harness for exercising filesystem parsing logic outside of UEFI.
- **`build.py`**: Orchestrates asset preparation (font copy, SVG-to-QOI conversion), `cargo build`, EFI binary placement, and RBC config generation.

---

## RBC Configuration Format

RBC (Rignite Binary Config) is a compact TLV binary format. The file is located at `\EFI\RovelStars\CONF\boot.rbc` on the EFI System Partition.

**Header** (16 bytes):

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | Magic: `RGN!` (0x52 0x47 0x4E 0x21) |
| 4 | 2 | Version (little-endian, currently 1) |
| 6 | 4 | Total file size (little-endian) |
| 10 | 6 | Reserved |

Each atom after the header is a 4-byte header (2-byte tag + 2-byte length) followed by the value bytes.

Defined tags include partition UUID (`0x01`, `0x10`), filesystem type (`0x02`, `0x11`), kernel parameters (`0x03`, `0x12`), and an optional PKCS7 signature atom (`0xFF`).

The source configuration is a TOML file:

```Rignite/rbc_cli/config.toml#L1-24
# Rignite Binary Config (RBC) Source Configuration
# Used by rbc_cli to generate the binary boot.rbc blob

[main]
# UUID of the partition containing the root filesystem
uuid = "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11"

# File System Type ID:
# 1 = Btrfs
# 2 = Ext4
# 3 = XFS
fs_type = 1

# Linux Kernel Command Line Parameters
kernel_params = "root=UUID=a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11 root=/dev/vda rw rootfstype=btrfs init=/Core/sbin/init console=ttyS0 quiet splash loglevel=3 systemd.show_status=false"

[recovery]
uuid = "b1ffcd88-8d0a-3de7-aa5c-5cc8ac270b22"

# File System Type ID:
# 10 = EROFS
# 11 = SquashFS
fs_type = 11

kernel_params = "root=UUID=b1ffcd88-8d0a-3de7-aa5c-5cc8ac270b22 ro init=/init rescue"
```

---

## FAT Boot Layout

Rignite follows the UEFI specification and requires a FAT32-formatted EFI System Partition with the following layout:

```/dev/null/layout.txt#L1-9
EFI/
  BOOT/
    BOOTX64.EFI          # Primary UEFI boot entry (x86_64)
    BOOTAA64.EFI         # Primary UEFI boot entry (aarch64)
  RovelStars/
    RIGNITEX64.EFI       # Vendor-specific boot entry (optional fallback)
    CONF/
      boot.rbc           # Rignite Binary Config
```

---

## Building

Prerequisites: Rust with the `x86_64-unknown-uefi` or `aarch64-unknown-uefi` target installed, `rsvg-convert`, Python 3 with `Pillow`, and `pyftsubset` (from `fonttools`).

```/dev/null/build.sh#L1-4
# Build for x86_64 (default)
python3 build.py

# Build for aarch64
python3 build.py aarch64
```

The script:
1. Subsets and copies the font to `assets/font.data`.
2. Converts SVG icons to QOI and writes them to `assets/`.
3. Runs `cargo build --target x86_64-unknown-uefi --release`.
4. Copies the EFI binary to `target/uefi/esp/EFI/BOOT/BOOTX64.EFI`.
5. Builds and runs `rbc_cli` to generate `boot.rbc` and places it in the ESP tree.

---

## Dependencies

| Crate | Purpose |
|---|---|
| `uefi` | UEFI protocol bindings, boot services, runtime services |
| `uefi-raw` | Raw UEFI system table pointer access |
| `embedded-graphics` | 2D drawing primitives and `DrawTarget` trait |
| `fontdue` | No-std font rasterizer (used with `default-features = false`) |
| `qoi` | QOI image decoder (used with `default-features = false`) |
| `sha2` | SHA-256 for future signature verification (used with `default-features = false`) |
```

Now let me apply this to the actual file: