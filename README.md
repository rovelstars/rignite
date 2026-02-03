# Rignite
Rignite is a high-performance, optimized UEFI bootloader written in Rust. It is designed to provide a fast and reliable boot process for RunixOS, our upcoming flagship operating system. Rignite leverages Rust's safety and concurrency features to ensure a secure and efficient boot experience.

## Features
- **Fast Boot Times**: Rignite is optimized for speed, reducing the time it takes to boot into RunixOS.
- **UEFI Support**: Fully compatible with UEFI firmware, ensuring broad hardware support.
- **Secure Boot**: Implements secure boot mechanisms to protect against unauthorized code execution during the boot process.
- **Modular Design**: Built with a modular architecture, allowing for easy customization and extension.
- **Rust Safety**: Utilizes Rust's memory safety features to minimize vulnerabilities and enhance reliability.
- **Cross-Platform**: Designed to work across various hardware platforms supported by UEFI. Currently tested on x86_64 & ARM64 architectures.
- **Open Source**: Rignite is open source, encouraging community contributions and collaboration.
- **Graphical Based Bootloader**: Rignite features a graphical user interface (GUI) for a more user-friendly boot experience.
- **Filesystem Support**: Currently supports BTRFS filesystem for loading the OS kernel and initramfs. Looking to add support for more filesystems in the future, as well as boot other OSes.

# FAT BOOT Layout

Rignite follows the UEFI standard specification, which requires a FAT32 formatted boot layout to function correctly. The following structure is be followed:

```/
├── EFI
│   └── BOOT
│       └── BOOTX64.EFI  (Rignite bootloader binary - always named BOOTX64.EFI for x86_64 architecture)
│   └── RovelStars
│       └── RIGNITEX64.EFI  (Alternative location for Rignite bootloader binary, as backup)
│       └── CONF (Configuration file for Rignite - maintains settings and preferences)
```