#![no_std]
#![no_main]

#[macro_use]
extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use uefi::boot::SearchType;
use uefi::proto::console::gop::GraphicsOutput;
use uefi::proto::console::text::{Key, ScanCode};
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::block::BlockIO;
use uefi::runtime::{ResetType, VariableAttributes, VariableVendor};
use uefi::CStr16;
use uefi::Identify;

mod font;
mod fs;
mod graphics;
mod icons;
mod input;
mod logger;
mod logo;

use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::RgbColor;
use font::FontRenderer;
use graphics::UefiDisplay;
use icons::Icon;
use input::InputHandler;

#[no_mangle]
pub extern "C" fn efi_main(
    image_handle: uefi::Handle,
    system_table: *mut uefi_raw::table::system::SystemTable,
) -> uefi::Status {
    unsafe {
        uefi::boot::set_image_handle(image_handle);
        uefi::table::set_system_table(system_table);
    }

    if let Err(_) = uefi::helpers::init() {
        return uefi::Status::ABORTED;
    }

    crate::info!("Rignite Bootloader Started");

    // Initialize Graphics
    let gop_handle = match uefi::boot::get_handle_for_protocol::<GraphicsOutput>() {
        Ok(h) => h,
        Err(e) => {
            crate::error!("Failed to get GOP handle: {:?}", e);
            return e.status();
        }
    };

    let mut gop = match uefi::boot::open_protocol_exclusive::<GraphicsOutput>(gop_handle) {
        Ok(g) => g,
        Err(e) => {
            crate::error!("Failed to open GOP: {:?}", e);
            return e.status();
        }
    };

    // Create Display Wrapper
    let mut display = UefiDisplay::new(&mut gop);

    // High Resolution
    if let Err(e) = display.set_highest_resolution() {
        crate::warn!("Failed to set resolution: {:?}", e);
    }

    let (width, height) = (display.width(), display.height());
    crate::info!("Resolution: {}x{}", width, height);

    // Load Resources
    let font_data = include_bytes!("../assets/font.data");
    let drive_icon_data = include_bytes!("../assets/drive.qoi");
    let firmware_icon_data = include_bytes!("../assets/firmware.qoi");
    let reboot_icon_data = include_bytes!("../assets/reboot.qoi");
    let shutdown_icon_data = include_bytes!("../assets/shutdown.qoi");

    // Init Renderers
    let font_renderer = FontRenderer::new(font_data);
    let drive_icon = Icon::new(drive_icon_data, 196, 196);
    let firmware_icon = Icon::new(firmware_icon_data, 32, 32);
    let reboot_icon = Icon::new(reboot_icon_data, 32, 32);
    let shutdown_icon = Icon::new(shutdown_icon_data, 32, 32);
    let logo_icon = logo::Logo::new();

    // Scan for Block Devices
    let handles = uefi::boot::locate_handle_buffer(SearchType::ByProtocol(&BlockIO::GUID))
        .expect("Failed to locate BlockIo handles");

    #[derive(Clone)]
    enum MenuItem {
        Drive {
            name: String,
            handle: Option<uefi::Handle>,
        },
        FirmwareSettings,
        Reboot,
        Shutdown,
    }

    let mut menu_items = Vec::new();
    let mut runixos_handle: Option<uefi::Handle> = None;

    // Add drives
    for handle in handles.iter() {
        if let Ok(mut block_io) = uefi::boot::open_protocol_exclusive::<BlockIO>(*handle) {
            let (is_present, size_gb, is_removable) = {
                let media = block_io.media();
                (
                    media.is_media_present(),
                    media.block_size() as u64 * media.last_block() / (1024 * 1024 * 1024),
                    media.is_removable_media(),
                )
            };
            // empty drives are skipped
            if size_gb == 0 {
                continue;
            }
            if is_present {
                let mut label_name = None;

                // Try to read Btrfs label
                if let Ok(Some(btrfs)) = fs::btrfs::Btrfs::new(&mut block_io) {
                    let label = btrfs.get_label();
                    if !label.is_empty() {
                        label_name = Some(format!("{} ({}GB)", label, size_gb));
                        if label == "RunixOS" {
                            runixos_handle = Some(*handle);
                        }
                    }
                }

                let name = if let Some(l) = label_name {
                    l
                } else if is_removable {
                    format!("Removable {}GB", size_gb)
                } else {
                    format!("HDD {}GB", size_gb)
                };

                menu_items.push(MenuItem::Drive {
                    name,
                    handle: Some(*handle),
                });
            }
        }
    }

    if menu_items.is_empty() {
        menu_items.push(MenuItem::Drive {
            name: String::from("No Drives Found"),
            handle: None,
        });
    }

    // Add system options
    menu_items.push(MenuItem::FirmwareSettings);
    menu_items.push(MenuItem::Reboot);
    menu_items.push(MenuItem::Shutdown);

    use core::cell::RefCell;
    let display = RefCell::new(display);

    // Use with_stdin to access input safely
    uefi::system::with_stdin(|stdin| {
        let mut input_handler = InputHandler::new(stdin);

        // Logo Animation
        display
            .borrow_mut()
            .clear(Rgb888::new(0, 0, 0))
            .expect("Failed to clear screen");

        let logo_size = 64;
        let center_x = (width as i32 / 2) - (logo_size as i32 / 2);
        let center_y = (height as i32 / 2) - (logo_size as i32 / 2);

        // Fade In
        crate::debug!("Starting logo fade-in...");
        for i in 0..=60 {
            let mut disp = display.borrow_mut();
            disp.clear(Rgb888::new(0, 0, 0)).ok();
            let opacity = i as f32 / 60.0;
            logo_icon.draw(&mut *disp, center_x, center_y, logo_size, opacity);
            disp.flush();
            uefi::boot::stall(16_670);
        }

        // Interactive Double-Chord
        let mut enter_menu = false;
        let mut up_window = 0;
        let mut down_window = 0;
        let mut cooldown = 0;

        let fps = 60;
        let idle_timeout = 2 * fps; // 2 seconds initial wait
        let confirm_timeout = 5 * fps; // 5 seconds to confirm

        enum SplashState {
            Idle,
            Confirming,
        }
        let mut state = SplashState::Idle;
        let mut state_timer = 0;

        loop {
            // Read input (Window of 15 frames = ~250ms for simultaneous press)
            while let Some(key) = input_handler.read_key() {
                match key {
                    Key::Special(ScanCode::UP) => up_window = 15,
                    Key::Special(ScanCode::DOWN) => down_window = 15,
                    _ => {}
                }
            }

            if up_window > 0 {
                up_window -= 1;
            }
            if down_window > 0 {
                down_window -= 1;
            }
            if cooldown > 0 {
                cooldown -= 1;
            }

            let chord_trigger = up_window > 0 && down_window > 0;

            {
                let mut disp = display.borrow_mut();
                disp.clear(Rgb888::new(0, 0, 0)).ok();
                logo_icon.draw(&mut *disp, center_x, center_y, logo_size, 1.0);

                match state {
                    SplashState::Idle => {
                        if chord_trigger && cooldown == 0 {
                            state = SplashState::Confirming;
                            state_timer = 0;
                            cooldown = 30; // 0.5s debounce
                        } else if state_timer >= idle_timeout {
                            break; // Timeout, proceed to auto-boot
                        }
                    }
                    SplashState::Confirming => {
                        // Fade in text
                        let text_opacity = (state_timer as f32 / 30.0).min(1.0);
                        let c = (255.0 * text_opacity) as u8;
                        let color = Rgb888::new(c, c, c);

                        let remaining = 5 - (state_timer / 60);
                        let msg = format!("Press UP & DOWN again ({}) to enter menu...", remaining);
                        let msg_width = font_renderer.get_text_width(&msg, 16.0);
                        font_renderer.draw_text(
                            &mut *disp,
                            &msg,
                            (width as i32 - msg_width) / 2,
                            height as i32 - 40,
                            16.0,
                            color,
                        );

                        if chord_trigger && cooldown == 0 {
                            enter_menu = true;
                            break;
                        } else if state_timer >= confirm_timeout {
                            // Fade out text
                            for i in (0..=30).rev() {
                                disp.clear(Rgb888::new(0, 0, 0)).ok();
                                logo_icon.draw(&mut *disp, center_x, center_y, logo_size, 1.0);

                                let fade_opacity = i as f32 / 30.0;
                                let c = (255.0 * fade_opacity) as u8;
                                let color = Rgb888::new(c, c, c);

                                font_renderer.draw_text(
                                    &mut *disp,
                                    &msg,
                                    (width as i32 - msg_width) / 2,
                                    height as i32 - 40,
                                    16.0,
                                    color,
                                );
                                disp.flush();
                                uefi::boot::stall(16_670);
                            }
                            break; // Timeout, proceed to auto-boot
                        }
                    }
                }

                disp.flush();
            }

            uefi::boot::stall(1000_000 / fps as usize);
            state_timer += 1;
        }

        // Auto-boot Logic
        if !enter_menu {
            if let Some(handle) = runixos_handle {
                crate::info!("Auto-booting RunixOS...");
                if let Err(e) = boot_linux_from_drive(handle) {
                    crate::error!("Failed to auto-boot: {:?}", e);
                    enter_menu = true;
                } else {
                    // Boot returned (kernel exit?), show menu
                    enter_menu = true;
                }
            } else {
                crate::warn!("No default drive 'RunixOS' found.");
                enter_menu = true;
            }
        }

        // Fade Out (only if entering menu)
        if enter_menu {
            crate::debug!("Starting logo fade-out...");
            for i in (0..=60).rev() {
                let mut disp = display.borrow_mut();
                disp.clear(Rgb888::new(0, 0, 0)).ok();
                let opacity = i as f32 / 60.0;
                logo_icon.draw(&mut *disp, center_x, center_y, logo_size, opacity);
                disp.flush();
                uefi::boot::stall(16_670);
            }
        }

        // Clear screen for menu
        display
            .borrow_mut()
            .clear(Rgb888::new(0, 0, 0))
            .expect("Failed to clear screen");
        display.borrow_mut().flush();

        use core::cell::RefCell;

        let selected_index = RefCell::new(0);
        let redraw = RefCell::new(true);
        // let display = RefCell::new(display);

        crate::info!("Entering interactive loop...");

        // Initialize scales for animation
        let mut item_scales: Vec<f32> = Vec::new();
        for _ in 0..menu_items.len() {
            item_scales.push(1.0);
        }

        loop {
            // Update animations
            let mut animating = false;
            let selected = *selected_index.borrow();
            for (i, scale) in item_scales.iter_mut().enumerate() {
                let target = if i == selected { 1.2 } else { 1.0 };
                let diff = target - *scale;
                if diff.abs() > 0.01 {
                    *scale += diff * 0.2; // Smooth transition
                    animating = true;
                } else {
                    *scale = target;
                }
            }

            if animating {
                *redraw.borrow_mut() = true;
            }

            if *redraw.borrow() {
                // crate::debug!("Redrawing UI...");
                let mut disp = display.borrow_mut();

                // Clear screen to avoid artifacts during animation
                disp.clear(Rgb888::new(0, 0, 0)).ok();

                // Dynamic sizing: 15% of screen height
                let icon_size = (height as u32 * 15) / 100;
                let icon_size = icon_size.max(64).min(256); // Clamp size

                // Calculate drive count
                let drive_count = menu_items
                    .iter()
                    .filter(|item| matches!(item, MenuItem::Drive { .. }))
                    .count();

                // Layout: Drives in center, System options at bottom
                let item_width = (icon_size * 2) as i32;
                let is_vertical = height > width;

                let title = "Select Boot Device";
                let title_size = 28.0;
                let title_width = font_renderer.get_text_width(title, title_size);
                let title_x = (width as i32 - title_width) / 2;

                crate::debug!("Drawing title at x={}, y={}", title_x, 100);
                font_renderer.draw_text(
                    &mut *disp,
                    title,
                    title_x,
                    100,
                    title_size,
                    Rgb888::new(255, 255, 255),
                );

                crate::debug!(
                    "Drawing {} total menu items, selected_index={}",
                    menu_items.len(),
                    *selected_index.borrow()
                );

                // Draw drives in center
                let drives_start_x = (width as i32 / 2) - ((drive_count as i32 * item_width) / 2);
                let drives_y = (height as i32 / 2) - (icon_size as i32 / 2);

                // Vertical layout calculations
                let vertical_item_height = icon_size as i32 + 60;
                let drives_start_y_vertical =
                    (height as i32 / 2) - ((drive_count as i32 * vertical_item_height) / 2);

                let mut drive_idx = 0;
                for (i, item) in menu_items.iter().enumerate() {
                    if let MenuItem::Drive { name, .. } = item {
                        let (x, y) = if is_vertical {
                            (
                                (width as i32 / 2) - (item_width / 2),
                                drives_start_y_vertical + (drive_idx as i32 * vertical_item_height),
                            )
                        } else {
                            (drives_start_x + (drive_idx as i32 * item_width), drives_y)
                        };

                        // Draw Icon
                        let scale = item_scales[i];
                        let scaled_size = (icon_size as f32 * scale) as u32;

                        let center_x = x + item_width / 2;
                        let center_y = y + icon_size as i32 / 2;
                        let draw_x = center_x - (scaled_size as i32 / 2);
                        let draw_y = center_y - (scaled_size as i32 / 2);

                        drive_icon.draw_scaled(&mut *disp, draw_x, draw_y, scaled_size, 1.0);

                        // Draw Text
                        let label_size = 18.0;
                        let text_width = font_renderer.get_text_width(name, label_size);
                        let text_x = x + (item_width / 2) - (text_width / 2);
                        let text_y = y + icon_size as i32 + 20;

                        let color = if i == *selected_index.borrow() {
                            crate::debug!("Menu item {} '{}' is SELECTED (yellow)", i, name);
                            Rgb888::new(255, 255, 0)
                        } else {
                            Rgb888::new(200, 200, 200)
                        };

                        crate::debug!(
                            "Drawing text '{}' at x={}, y={}, color=({},{},{})",
                            name,
                            text_x,
                            text_y,
                            color.r(),
                            color.g(),
                            color.b()
                        );
                        font_renderer
                            .draw_text(&mut *disp, name, text_x, text_y, label_size, color);

                        drive_idx += 1;
                    }
                }

                // Draw system options at bottom (smaller than drives)
                let sys_icon_size = (icon_size * 3) / 10; // 30% of drive icon size
                let sys_options_y = height as i32 - sys_icon_size as i32 - 80;
                let sys_option_count = 3; // Firmware, Reboot, Shutdown
                let sys_item_width = (sys_icon_size * 2) as i32;
                let sys_start_x = (width as i32 / 2) - ((sys_option_count * sys_item_width) / 2);

                let mut sys_idx = 0;
                for (i, item) in menu_items.iter().enumerate() {
                    let (icon, name) = match item {
                        MenuItem::FirmwareSettings => (&firmware_icon, "FW"),
                        MenuItem::Reboot => (&reboot_icon, "Reboot"),
                        MenuItem::Shutdown => (&shutdown_icon, "Shutdown"),
                        MenuItem::Drive { .. } => continue,
                    };

                    let x = sys_start_x + (sys_idx * sys_item_width);
                    let y = sys_options_y;

                    // Draw Icon (smaller)
                    let scale = item_scales[i];
                    let scaled_size = (sys_icon_size as f32 * scale) as u32;

                    let center_x = x + sys_item_width / 2;
                    let center_y = y + sys_icon_size as i32 / 2;
                    let draw_x = center_x - (scaled_size as i32 / 2);
                    let draw_y = center_y - (scaled_size as i32 / 2);

                    icon.draw_scaled(&mut *disp, draw_x, draw_y, scaled_size, 1.0);

                    // Draw Text (smaller font)
                    let sys_label_size = 14.0;
                    let text_width = font_renderer.get_text_width(name, sys_label_size);
                    let text_x = x + (sys_item_width / 2) - (text_width / 2);
                    let text_y = y + sys_icon_size as i32 + 15;

                    let color = if i == *selected_index.borrow() {
                        crate::debug!("System option {} '{}' is SELECTED (yellow)", i, name);
                        Rgb888::new(255, 255, 0)
                    } else {
                        Rgb888::new(200, 200, 200)
                    };

                    font_renderer.draw_text(
                        &mut *disp,
                        name,
                        text_x,
                        text_y,
                        sys_label_size,
                        color,
                    ); // Smaller font
                    sys_idx += 1;
                }

                disp.flush();
                *redraw.borrow_mut() = false;
                crate::debug!("Redraw complete.");
            }

            if let Some(key) = input_handler.read_key() {
                let mut idx = selected_index.borrow_mut();
                match key {
                    Key::Printable(c)
                        if u16::from(c) == 'd' as u16 || u16::from(c) == 's' as u16 =>
                    {
                        *idx = (*idx + 1) % menu_items.len(); // Wraparound
                        *redraw.borrow_mut() = true;
                    }
                    Key::Special(ScanCode::RIGHT) | Key::Special(ScanCode::DOWN) => {
                        *idx = (*idx + 1) % menu_items.len(); // Wraparound
                        *redraw.borrow_mut() = true;
                    }
                    Key::Printable(c)
                        if u16::from(c) == 'a' as u16 || u16::from(c) == 'w' as u16 =>
                    {
                        *idx = if *idx == 0 {
                            menu_items.len() - 1
                        } else {
                            *idx - 1
                        }; // Wraparound
                        *redraw.borrow_mut() = true;
                    }
                    Key::Special(ScanCode::LEFT) | Key::Special(ScanCode::UP) => {
                        *idx = if *idx == 0 {
                            menu_items.len() - 1
                        } else {
                            *idx - 1
                        }; // Wraparound
                        *redraw.borrow_mut() = true;
                    }
                    Key::Printable(c) if u16::from(c) == '\r' as u16 => {
                        let selected = &menu_items[*idx];
                        match selected {
                            MenuItem::Drive { name, handle } => {
                                crate::info!("Booting from drive: {}", name);
                                if let Some(h) = handle {
                                    if let Err(e) = boot_linux_from_drive(*h) {
                                        crate::error!("Boot failed: {:?}", e);
                                    }
                                }
                            }
                            MenuItem::FirmwareSettings => {
                                crate::debug!("Rebooting to firmware settings...");

                                // Set OsIndications variable to boot to firmware UI
                                const EFI_OS_INDICATIONS_BOOT_TO_FW_UI: u64 = 0x0000000000000001;

                                // Use EFI_GLOBAL_VARIABLE vendor
                                let vendor = VariableVendor::GLOBAL_VARIABLE;

                                let mut buf = [0u16; 32];
                                let var_name = CStr16::from_str_with_buf("OsIndications", &mut buf)
                                    .expect("Failed to create variable name");

                                let value = EFI_OS_INDICATIONS_BOOT_TO_FW_UI.to_le_bytes();

                                let attrs = VariableAttributes::BOOTSERVICE_ACCESS
                                    | VariableAttributes::RUNTIME_ACCESS
                                    | VariableAttributes::NON_VOLATILE;

                                // Set the variable
                                match uefi::runtime::set_variable(var_name, &vendor, attrs, &value)
                                {
                                    Ok(_) => {
                                        crate::debug!("OsIndications variable set successfully");
                                        uefi::runtime::reset(
                                            ResetType::COLD,
                                            uefi::Status::SUCCESS,
                                            None,
                                        );
                                    }
                                    Err(e) => {
                                        crate::warn!("Failed to set OsIndications: {:?}, doing normal reboot", e);
                                        uefi::runtime::reset(
                                            ResetType::COLD,
                                            uefi::Status::SUCCESS,
                                            None,
                                        );
                                    }
                                }
                            }
                            MenuItem::Reboot => {
                                crate::debug!("Rebooting...");
                                uefi::runtime::reset(ResetType::COLD, uefi::Status::SUCCESS, None);
                            }
                            MenuItem::Shutdown => {
                                crate::debug!("Shutting down...");
                                uefi::runtime::reset(
                                    ResetType::SHUTDOWN,
                                    uefi::Status::SUCCESS,
                                    None,
                                );
                            }
                        }
                    }
                    _ => {}
                }
            }

            // Stall to reduce CPU usage and maintain approx 30 FPS
            uefi::boot::stall(33_000);
        }
    });

    uefi::Status::SUCCESS
}

#[repr(C)]
struct EfiLoadFile2 {
    load_file: unsafe extern "efiapi" fn(
        this: *mut core::ffi::c_void,
        file_path: *const core::ffi::c_void,
        boot_policy: bool,
        buffer_size: *mut usize,
        buffer: *mut u8,
    ) -> uefi::Status,
}

fn boot_linux_from_drive(handle: uefi::Handle) -> uefi::Result<()> {
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

    if let Some((initrd_inode, _)) = initrd_res {
        crate::info!("Found initramfs-linux.img, loading...");
        let initrd_data = btrfs.read_file(current_fs_root, initrd_inode)?;
        crate::info!("Initrd loaded ({} bytes).", initrd_data.len());

        // Setup LoadFile2 for Initrd
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
            let new_handle = uefi::boot::install_protocol_interface(
                None,
                &device_path_guid,
                dp_ptr as *mut core::ffi::c_void,
            )?;

            // Install LoadFile2 on that handle
            // Allocate protocol struct on heap and leak it
            let lf2_ptr = alloc::alloc::alloc(alloc::alloc::Layout::new::<EfiLoadFile2>());
            core::ptr::write(lf2_ptr as *mut EfiLoadFile2, load_file2);

            uefi::boot::install_protocol_interface(
                Some(new_handle),
                &load_file2_guid,
                lf2_ptr as *mut core::ffi::c_void,
            )?;

            crate::info!("Initrd LoadFile2 protocol installed.");
        }
    }

    crate::info!("Reading kernel...");
    let kernel_data = btrfs.read_file(current_fs_root, kernel_obj)?;
    crate::info!("Kernel loaded ({} bytes). Starting...", kernel_data.len());

    let handle = uefi::boot::load_image(
        uefi::boot::image_handle(),
        uefi::boot::LoadImageSource::FromBuffer {
            buffer: &kernel_data,
            file_path: None,
        },
    )?;

    // Command line: root=UUID=... rw rootflags=compress=zstd:1 init=/Core/sbin/init console=ttyS0
    let uuid = btrfs.get_uuid();
    let cmdline = format!(
        "root=UUID={} rw rootflags=compress=zstd:1 init=/Core/sbin/init console=ttyS0",
        uuid
    );
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

    uefi::boot::start_image(handle)?;
    Ok(())
}

static mut INITRD_DATA: Option<Vec<u8>> = None;

unsafe extern "efiapi" fn load_file2_initrd(
    _this: *mut core::ffi::c_void,
    _file_path: *const core::ffi::c_void,
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

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

use uefi::allocator::Allocator;

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;
