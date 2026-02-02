use alloc::format;
use alloc::vec::Vec;
use core::ffi::c_void;
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::block::BlockIO;
use uefi::Identify;

use crate::fs;

#[repr(C)]
struct EfiLoadFile2 {
    load_file: unsafe extern "efiapi" fn(
        this: *mut c_void,
        file_path: *const c_void,
        boot_policy: bool,
        buffer_size: *mut usize,
        buffer: *mut u8,
    ) -> uefi::Status,
}

static mut INITRD_DATA: Option<Vec<u8>> = None;

unsafe extern "efiapi" fn load_file2_initrd(
    _this: *mut c_void,
    _file_path: *const c_void,
    _boot_policy: bool,
    buffer_size: *mut usize,
    buffer: *mut u8,
) -> uefi::Status {
    if buffer_size.is_null() {
        return uefi::Status::INVALID_PARAMETER;
    }

    let data = match unsafe { &*core::ptr::addr_of!(INITRD_DATA) } {
        Some(d) => d,
        None => return uefi::Status::NOT_FOUND,
    };

    let required_size = data.len();
    let available_size = *buffer_size;

    *buffer_size = required_size;

    if buffer.is_null() || available_size < required_size {
        return uefi::Status::BUFFER_TOO_SMALL;
    }

    core::ptr::copy_nonoverlapping(data.as_ptr(), buffer, required_size);
    uefi::Status::SUCCESS
}

pub fn validate_kernel_pe(kernel_data: &[u8]) -> uefi::Result<()> {
    if kernel_data.len() > 0x40 {
        // Check for 'MZ' signature
        if kernel_data[0] != 0x4d || kernel_data[1] != 0x5a {
            crate::error!("Invalid kernel image: Missing 'MZ' signature");
            return Err(uefi::Error::new(uefi::Status::INVALID_PARAMETER, ()));
        }

        // Get offset to PE header
        let pe_offset = u32::from_le_bytes(kernel_data[0x3c..0x40].try_into().unwrap()) as usize;

        if pe_offset + 6 < kernel_data.len() {
            // Check 'PE\0\0' signature
            if kernel_data[pe_offset] != 0x50 || kernel_data[pe_offset + 1] != 0x45 {
                crate::error!("Invalid kernel image: Missing 'PE' signature");
                return Err(uefi::Error::new(uefi::Status::INVALID_PARAMETER, ()));
            }

            // Check Machine Type (Offset +4 after PE signature)
            // 0x8664 = x86_64, 0xAA64 = AArch64
            let machine = u16::from_le_bytes(
                kernel_data[pe_offset + 4..pe_offset + 6]
                    .try_into()
                    .unwrap(),
            );

            #[cfg(target_arch = "x86_64")]
            if machine != 0x8664 {
                crate::error!(
                    "Architecture mismatch: Kernel is {:#x}, expected 0x8664 (x86_64)",
                    machine
                );
                return Err(uefi::Error::new(uefi::Status::INVALID_PARAMETER, ()));
            }

            #[cfg(target_arch = "aarch64")]
            if machine != 0xAA64 {
                crate::error!(
                    "Architecture mismatch: Kernel is {:#x}, expected 0xAA64 (AArch64)",
                    machine
                );
                return Err(uefi::Error::new(uefi::Status::INVALID_PARAMETER, ()));
            }
        }
    }
    Ok(())
}

fn install_initrd_protocol(initrd_data: Vec<u8>) -> uefi::Result<()> {
    unsafe {
        INITRD_DATA = Some(initrd_data);

        // LINUX_EFI_INITRD_MEDIA_GUID
        // 5568e427-68fc-4f3d-ac74-ca555231cc68

        // Construct Device Path
        // Vendor Device Path (Type 4, SubType 3)
        // Header (4) + Guid (16) = 20 bytes
        // End (Type 0x7F, SubType 0xFF, Len 4)
        let device_path_data: [u8; 24] = [
            0x04, 0x03, 0x14, 0x00, // Vendor Path Header
            0x27, 0xe4, 0x68, 0x55, 0xfc, 0x68, 0x3d, 0x4f, // GUID Part 1
            0xac, 0x74, 0xca, 0x55, 0x52, 0x31, 0xcc, 0x68, // GUID Part 2
            0x7f, 0xff, 0x04, 0x00, // End Path
        ];

        // Install protocol
        let load_file2 = EfiLoadFile2 {
            load_file: load_file2_initrd,
        };

        let load_file2_guid = uefi::proto::media::load_file::LoadFile2::GUID;
        let device_path_guid = uefi::proto::device_path::DevicePath::GUID;

        // Allocate DP on heap and leak it
        let dp_ptr = alloc::alloc::alloc(alloc::alloc::Layout::from_size_align(24, 4).unwrap());
        core::ptr::copy_nonoverlapping(device_path_data.as_ptr(), dp_ptr, 24);

        // Create a new handle with DevicePath first
        let new_handle =
            uefi::boot::install_protocol_interface(None, &device_path_guid, dp_ptr as *mut c_void)?;

        // Install LoadFile2 on that handle
        // Allocate protocol struct on heap and leak it
        let lf2_ptr = alloc::alloc::alloc(alloc::alloc::Layout::new::<EfiLoadFile2>());
        core::ptr::write(lf2_ptr as *mut EfiLoadFile2, load_file2);

        uefi::boot::install_protocol_interface(
            Some(new_handle),
            &load_file2_guid,
            lf2_ptr as *mut c_void,
        )?;

        crate::info!("Initrd LoadFile2 protocol installed.");
    }
    Ok(())
}

pub fn boot_from_memory(
    kernel_data: &[u8],
    initrd_data: Option<Vec<u8>>,
    cmdline: &str,
) -> uefi::Result<()> {
    validate_kernel_pe(kernel_data)?;

    if let Some(data) = initrd_data {
        install_initrd_protocol(data)?;
    }

    crate::info!("Loading image from buffer...");
    let handle = match uefi::boot::load_image(
        uefi::boot::image_handle(),
        uefi::boot::LoadImageSource::FromBuffer {
            buffer: kernel_data,
            file_path: None,
        },
    ) {
        Ok(h) => h,
        Err(e) => {
            crate::error!("Failed to load_image: {:?}", e);
            return Err(e);
        }
    };
    crate::info!("Image loaded successfully. Handle: {:?}", handle);

    let mut cmdline_utf16: Vec<u16> = cmdline.encode_utf16().collect();
    cmdline_utf16.push(0); // Null terminate

    let mut loaded_image = uefi::boot::open_protocol_exclusive::<LoadedImage>(handle)?;

    unsafe {
        loaded_image.set_load_options(
            cmdline_utf16.as_ptr() as *const u8,
            (cmdline_utf16.len() * 2) as u32,
        );
    }
    drop(loaded_image);

    crate::info!("Starting image...");
    if let Err(e) = uefi::boot::start_image(handle) {
        crate::error!("Failed to start_image: {:?}", e);
        return Err(e);
    }
    crate::info!("Image returned successfully.");
    Ok(())
}

pub fn boot_linux_from_drive(handle: uefi::Handle) -> uefi::Result<()> {
    let mut block_io = uefi::boot::open_protocol_exclusive::<BlockIO>(handle)?;

    let mut btrfs = match fs::btrfs::Btrfs::new(&mut block_io)? {
        Some(b) => b,
        None => return Err(uefi::Error::new(uefi::Status::UNSUPPORTED, ())),
    };

    crate::info!("Btrfs detected. Searching for /Core/Boot/vmlinuz-linux...");

    let mut current_fs_root = btrfs.get_fs_root()?;
    let mut current_dir_id = 256;

    // 1. Find Core
    let (core_obj, core_type) = btrfs
        .find_file_in_dir(current_fs_root, current_dir_id, "Core")?
        .ok_or(uefi::Error::new(uefi::Status::NOT_FOUND, ()))?;

    // Check if it is a subvolume (Root Item Key = 132)
    if core_type == fs::btrfs::BTRFS_ROOT_ITEM_KEY {
        crate::info!("Entering subvolume Core (ID {})", core_obj);
        current_fs_root = btrfs.get_tree_root(core_obj)?;
        current_dir_id = 256; // Root of new tree
    } else {
        // Just a directory
        current_dir_id = core_obj;
    }

    // 2. Find Boot
    let (boot_obj, _) = btrfs
        .find_file_in_dir(current_fs_root, current_dir_id, "Boot")?
        .ok_or(uefi::Error::new(uefi::Status::NOT_FOUND, ()))?;

    // 3. Find vmlinuz-linux
    let (kernel_obj, _) = btrfs
        .find_file_in_dir(current_fs_root, boot_obj, "vmlinuz-linux")?
        .ok_or(uefi::Error::new(uefi::Status::NOT_FOUND, ()))?;

    // Also try to find initramfs just to check existence
    let initrd_res = btrfs.find_file_in_dir(current_fs_root, boot_obj, "initramfs-linux.img")?;

    let initrd_data = if let Some((initrd_inode, _)) = initrd_res {
        crate::info!("Found initramfs-linux.img, loading...");
        let data = btrfs.read_file(current_fs_root, initrd_inode)?;
        crate::info!("Initrd loaded ({} bytes).", data.len());
        Some(data)
    } else {
        None
    };

    crate::info!("Reading kernel...");
    let kernel_data = btrfs.read_file(current_fs_root, kernel_obj)?;
    crate::info!("Kernel loaded ({} bytes). Starting...", kernel_data.len());

    // Command line: root=UUID=... rw init=/Core/sbin/init console=ttyS0
    let uuid = btrfs.get_uuid();
    // Use /dev/vda fallback for QEMU if UUID resolution fails in initrd (common in minimal dev envs)
    // Also add rootfstype=btrfs to prevent ext4 probe noise
    let cmdline = format!(
        "root=UUID={} root=/dev/vda rw rootfstype=btrfs init=/Core/sbin/init console=ttyS0",
        uuid
    );

    crate::info!("Kernel Command Line: {}", cmdline);

    boot_from_memory(&kernel_data, initrd_data, &cmdline)
}
