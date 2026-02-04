use alloc::format;
use alloc::string::ToString;
use alloc::vec::Vec;
use core::ffi::c_void;
use uefi::proto::device_path::{DevicePath, FfiDevicePath};
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::block::BlockIO;
use uefi::proto::media::file::{File, FileAttribute, FileInfo, FileMode, FileType};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::CStr16;
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

pub fn scan_dir_recursive(
    dir: &mut uefi::proto::media::file::Directory,
    path_prefix: &str,
    apps: &mut Vec<alloc::string::String>,
    quibble_path: &mut Option<alloc::string::String>,
) {
    let mut buf = [0u8; 512];
    loop {
        match dir.read_entry(&mut buf) {
            Ok(Some(entry)) => {
                let name = entry.file_name().to_string();
                if name == "." || name == ".." {
                    continue;
                }

                let full_path = format!("{}\\{}", path_prefix, name);
                let is_dir = entry
                    .attribute()
                    .contains(uefi::proto::media::file::FileAttribute::DIRECTORY);

                if is_dir {
                    // Recurse (Limited depth implicitly by stack/structure)
                    if let Ok(handle) =
                        dir.open(entry.file_name(), FileMode::Read, FileAttribute::empty())
                    {
                        if let Ok(FileType::Dir(mut subdir)) = handle.into_type() {
                            scan_dir_recursive(&mut subdir, &full_path, apps, quibble_path);
                        }
                    }
                } else {
                    let name_lower = name.to_lowercase();
                    if name_lower.ends_with(".efi") {
                        apps.push(full_path.clone());
                        if name_lower.contains("quibble.efi") {
                            *quibble_path = Some(full_path);
                        }
                    }
                }
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
}

pub fn boot_efi_app(handle: uefi::Handle, path_str: &str) -> uefi::Result<()> {
    crate::info!("Chainloading: {}", path_str);

    // 1. Get Volume Device Path
    let volume_dp = unsafe {
        uefi::boot::open_protocol::<DevicePath>(
            uefi::boot::OpenProtocolParams {
                handle,
                agent: uefi::boot::image_handle(),
                controller: None,
            },
            uefi::boot::OpenProtocolAttributes::GetProtocol,
        )
        .map_err(|e| uefi::Error::new(e.status(), ()))?
    };

    // 2. Construct Full Device Path (Volume + File Path + End)
    let mut dp_data = Vec::new();
    let mut src_ptr = volume_dp.as_ffi_ptr() as *const u8;

    // Copy Volume DP nodes
    loop {
        let type_ = unsafe { *src_ptr };
        let subtype = unsafe { *src_ptr.add(1) };
        if type_ == 0x7f && subtype == 0xff {
            break; // Stop before End Node
        }
        let len_lo = unsafe { *src_ptr.add(2) };
        let len_hi = unsafe { *src_ptr.add(3) };
        let len = (len_hi as usize) << 8 | (len_lo as usize);

        let node_slice = unsafe { core::slice::from_raw_parts(src_ptr, len) };
        dp_data.extend_from_slice(node_slice);

        src_ptr = unsafe { src_ptr.add(len) };
    }

    // Create File Path Node
    // Normalization
    let clean_path = path_str.replace('/', "\\");
    let clean_path = if !clean_path.starts_with('\\') {
        format!("\\{}", clean_path)
    } else {
        clean_path
    };

    let path_u16: Vec<u16> = clean_path
        .encode_utf16()
        .chain(core::iter::once(0))
        .collect();
    let path_size_bytes = path_u16.len() * 2;
    let node_size = 4 + path_size_bytes;

    dp_data.push(0x04); // Type: Media
    dp_data.push(0x04); // SubType: File Path
    dp_data.push(node_size as u8);
    dp_data.push((node_size >> 8) as u8);

    let path_bytes =
        unsafe { core::slice::from_raw_parts(path_u16.as_ptr() as *const u8, path_size_bytes) };
    dp_data.extend_from_slice(path_bytes);

    // Append End Node (Type 0x7f, SubType 0xff, Len 4)
    dp_data.extend_from_slice(&[0x7f, 0xff, 0x04, 0x00]);

    // Create DevicePath reference from buffer
    let file_dp = unsafe { DevicePath::from_ffi_ptr(dp_data.as_ptr() as *const FfiDevicePath) };

    // 3. Read file for PE Validation
    let mut fs_proto = unsafe {
        uefi::boot::open_protocol::<SimpleFileSystem>(
            uefi::boot::OpenProtocolParams {
                handle,
                agent: uefi::boot::image_handle(),
                controller: None,
            },
            uefi::boot::OpenProtocolAttributes::GetProtocol,
        )
        .map_err(|e| uefi::Error::new(e.status(), ()))?
    };
    let fs = &mut *fs_proto;
    let mut root = fs.open_volume()?;

    let mut path_buf = [0u16; 256];
    let clean_path_open = path_str.trim_start_matches('\\');
    let path_cstr = CStr16::from_str_with_buf(clean_path_open, &mut path_buf)
        .map_err(|_| uefi::Status::INVALID_PARAMETER)?;

    let file_handle = root.open(path_cstr, FileMode::Read, FileAttribute::empty())?;
    let mut file = match file_handle.into_type()? {
        FileType::Regular(f) => f,
        _ => return Err(uefi::Status::UNSUPPORTED.into()),
    };

    let mut info_buf = [0u8; 256];
    let info = file
        .get_info::<FileInfo>(&mut info_buf)
        .map_err(|e| uefi::Error::new(e.status(), ()))?;
    let size = info.file_size() as usize;

    let mut code_buf = vec![0u8; size];
    file.read(&mut code_buf)
        .map_err(|e| uefi::Error::new(e.status(), ()))?;

    // Validate PE
    validate_kernel_pe(&code_buf)?;

    // Load & Start using Buffer AND DevicePath
    let image_handle = uefi::boot::load_image(
        uefi::boot::image_handle(),
        uefi::boot::LoadImageSource::FromBuffer {
            buffer: &code_buf,
            file_path: Some(file_dp),
        },
    )?;

    crate::info!("Starting Chainloaded App...");

    // Reset Console Output to Text Mode
    uefi::system::with_stdout(|stdout| {
        let _ = stdout.reset(false);
        if let Some(mode) = stdout.modes().next() {
            let _ = stdout.set_mode(mode);
        }
        let _ = stdout.clear();
    });

    uefi::boot::start_image(image_handle)?;
    crate::info!("Chainloaded App Returned.");

    Ok(())
}

fn try_boot_fat_fallback(handle: uefi::Handle) -> uefi::Result<()> {
    crate::warn!("Btrfs failed. Attempting FAT/EFI Chainload Fallback...");

    // Open FS non-exclusively (GetProtocol)
    let mut fs_proto = unsafe {
        uefi::boot::open_protocol::<SimpleFileSystem>(
            uefi::boot::OpenProtocolParams {
                handle,
                agent: uefi::boot::image_handle(),
                controller: None,
            },
            uefi::boot::OpenProtocolAttributes::GetProtocol,
        )
        .map_err(|_| {
            crate::error!("No Filesystem found on drive.");
            uefi::Error::from(uefi::Status::UNSUPPORTED)
        })?
    };

    // ScopedProtocol derefs to SimpleFileSystem.
    let fs = &mut *fs_proto;
    let mut root = fs.open_volume()?;
    let mut apps = Vec::new();
    let mut quibble_path = None;

    scan_dir_recursive(&mut root, "", &mut apps, &mut quibble_path);

    if apps.is_empty() {
        crate::warn!("No EFI applications found.");
        return Err(uefi::Error::new(uefi::Status::NOT_FOUND, ()));
    }

    crate::info!("Found EFI Applications:");
    for app in &apps {
        crate::info!(" - {}", app);
    }

    let target = quibble_path
        .or_else(|| {
            apps.iter()
                .find(|a| {
                    let low = a.to_lowercase();
                    low.contains("bootx64.efi") || low.contains("bootmgfw.efi")
                })
                .cloned()
        })
        .or_else(|| apps.first().cloned());

    if let Some(path_str) = target {
        boot_efi_app(handle, &path_str)?;
    }

    Ok(())
}

pub fn boot_linux_from_drive(
    handle: uefi::Handle,
    cmdline_override: Option<&str>,
) -> uefi::Result<()> {
    // 1. Try BlockIO + Btrfs
    // Use GetProtocol to avoid disconnecting SimpleFileSystem driver which we might need for fallback
    let block_io = match unsafe {
        uefi::boot::open_protocol::<BlockIO>(
            uefi::boot::OpenProtocolParams {
                handle,
                agent: uefi::boot::image_handle(),
                controller: None,
            },
            uefi::boot::OpenProtocolAttributes::GetProtocol,
        )
    } {
        Ok(io) => io,
        Err(_) => return try_boot_fat_fallback(handle),
    };

    let mut btrfs = match fs::btrfs::Btrfs::new(&block_io) {
        Ok(Some(b)) => b,
        _ => {
            // Drop block_io to release handle (though GetProtocol doesn't lock it, scoping is good)
            drop(block_io);
            return try_boot_fat_fallback(handle);
        }
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
    let default_cmdline = format!(
        "root=UUID={} root=/dev/vda rw rootfstype=btrfs init=/Core/sbin/init console=ttyS0",
        uuid
    );

    let cmdline = cmdline_override.unwrap_or(&default_cmdline);

    crate::info!("Kernel Command Line: {}", cmdline);

    boot_from_memory(&kernel_data, initrd_data, cmdline)
}
