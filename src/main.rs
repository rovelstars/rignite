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

mod boot;
mod font;
mod fs;
mod graphics;
mod icons;
mod input;
mod logger;
mod logo;
mod rdf;

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

    logger::init();

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
        Recovery,
    }

    let mut menu_items = Vec::new();
    let mut runixos_handle: Option<uefi::Handle> = None;

    // Add drives
    for handle in handles.iter() {
        if let Ok(mut block_io) = uefi::boot::open_protocol_exclusive::<BlockIO>(*handle) {
            let (is_present, size_bytes, is_removable) = {
                let media = block_io.media();
                (
                    media.is_media_present(),
                    (media.block_size() as u64).saturating_mul(media.last_block() + 1),
                    media.is_removable_media(),
                )
            };

            // Skip extremely small drives (e.g. < 1MB) to filter noise
            if size_bytes < 1024 * 1024 {
                continue;
            }

            let size_str = if size_bytes >= 1024 * 1024 * 1024 {
                format!("{}GB", size_bytes / (1024 * 1024 * 1024))
            } else {
                format!("{}MB", size_bytes / (1024 * 1024))
            };

            if is_present {
                let mut label_name = None;

                // Try to read Btrfs label
                match fs::btrfs::Btrfs::new(&mut block_io) {
                    Ok(Some(btrfs)) => {
                        let label = btrfs.get_label();
                        if !label.is_empty() {
                            label_name = Some(format!("{} ({})", label, size_str));
                            if label == "RunixOS" {
                                runixos_handle = Some(*handle);
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        crate::warn!("Btrfs probe failed on handle {:?}: {:?}", handle, e);
                    }
                }

                let name = if let Some(l) = label_name {
                    l
                } else if is_removable {
                    format!("Removable {}", size_str)
                } else {
                    format!("HDD {}", size_str)
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
    menu_items.push(MenuItem::Recovery);

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
                if let Err(e) = boot::boot_linux_from_drive(handle) {
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
            for (_, scale) in item_scales.iter_mut().enumerate() {
                let target = 1.0;
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
                let sys_option_count = 4; // Firmware, Reboot, Shutdown, Recovery (RFU)
                let sys_item_width = (sys_icon_size * 2) as i32;
                let sys_start_x = (width as i32 / 2) - ((sys_option_count * sys_item_width) / 2);

                let mut sys_idx = 0;
                for (i, item) in menu_items.iter().enumerate() {
                    let (icon, name) = match item {
                        MenuItem::FirmwareSettings => (&firmware_icon, "FW"),
                        MenuItem::Reboot => (&reboot_icon, "Reboot"),
                        MenuItem::Shutdown => (&shutdown_icon, "Shutdown"),
                        MenuItem::Recovery => (&firmware_icon, "RFU"),
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
                                    if let Err(e) = boot::boot_linux_from_drive(*h) {
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
                            MenuItem::Recovery => {
                                crate::info!("Entering Recovery Mode...");
                                let mut recovery_selected_idx = 0;
                                let mut devices = Vec::new();
                                let mut scan_counter = 10; // Force initial scan

                                loop {
                                    // Poll devices every 1s (10 * 100ms)
                                    if scan_counter >= 10 {
                                        scan_counter = 0;
                                        match rdf::RdfManager::list_devices() {
                                            Ok(d) => devices = d,
                                            Err(e) => {
                                                crate::warn!("Error listing devices: {:?}", e);
                                            }
                                        };
                                    }
                                    scan_counter += 1;

                                    let mut disp = display.borrow_mut();
                                    disp.clear(Rgb888::new(0, 0, 0)).ok();
                                    font_renderer.draw_text(
                                        &mut *disp,
                                        "Recovery Mode - Select Device:",
                                        20,
                                        20,
                                        20.0,
                                        Rgb888::new(255, 255, 255),
                                    );

                                    // Draw Debug Console (Last 10 lines)
                                    let logs = logger::get_logs();
                                    for (i, entry) in logs.iter().rev().take(10).enumerate() {
                                        let color = match entry.level {
                                            "ERR" => Rgb888::new(255, 100, 100),
                                            "WRN" => Rgb888::new(255, 255, 0),
                                            "DBG" => Rgb888::new(100, 100, 100),
                                            _ => Rgb888::new(200, 200, 200),
                                        };
                                        // Draw from bottom up
                                        font_renderer.draw_text(
                                            &mut *disp,
                                            &format!("[{}] {}", entry.level, entry.message),
                                            20,
                                            height as i32 - 20 - (i as i32 * 16),
                                            14.0,
                                            color,
                                        );
                                    }

                                    if devices.is_empty() {
                                        font_renderer.draw_text(
                                            &mut *disp,
                                            "No USB IO devices found.",
                                            20,
                                            60,
                                            16.0,
                                            Rgb888::new(200, 200, 200),
                                        );
                                    } else {
                                        if recovery_selected_idx >= devices.len() {
                                            recovery_selected_idx = 0;
                                        }

                                        for (i, dev) in devices.iter().enumerate() {
                                            let is_selected = i == recovery_selected_idx;
                                            let prefix = if is_selected { "> " } else { "  " };
                                            let info = format!(
                                                "{}VID: {:#06x} PID: {:#06x}",
                                                prefix, dev.vid, dev.pid
                                            );

                                            let color = if is_selected {
                                                Rgb888::new(255, 255, 0)
                                            } else {
                                                Rgb888::new(200, 255, 200)
                                            };

                                            font_renderer.draw_text(
                                                &mut *disp,
                                                &info,
                                                20,
                                                60 + (i as i32 * 20),
                                                16.0,
                                                color,
                                            );
                                        }
                                    }

                                    font_renderer.draw_text(
                                        &mut *disp,
                                        "Press ENTER to flash, ESC to return...",
                                        20,
                                        height as i32 - 200,
                                        16.0,
                                        Rgb888::new(150, 150, 150),
                                    );

                                    disp.flush();
                                    uefi::boot::stall(100_000); // 100ms

                                    if let Some(key) = input_handler.read_key() {
                                        match key {
                                            Key::Special(ScanCode::ESCAPE) => break,
                                            Key::Special(ScanCode::UP) => {
                                                if recovery_selected_idx > 0 {
                                                    recovery_selected_idx -= 1;
                                                }
                                            }
                                            Key::Special(ScanCode::DOWN) => {
                                                if !devices.is_empty()
                                                    && recovery_selected_idx < devices.len() - 1
                                                {
                                                    recovery_selected_idx += 1;
                                                }
                                            }
                                            Key::Printable(c) if u16::from(c) == '\r' as u16 => {
                                                if !devices.is_empty() {
                                                    let dev = &devices[recovery_selected_idx];

                                                    // Drop display lock to allow callback to use it
                                                    drop(disp);

                                                    let res = rdf::RdfManager::download_image(
                                                        dev,
                                                        |curr, total| {
                                                            let mut d = display.borrow_mut();
                                                            d.clear(Rgb888::new(0, 0, 20)).ok();

                                                            let progress = if total > 0 {
                                                                (curr as f32 / total as f32 * 100.0)
                                                                    as u32
                                                            } else {
                                                                0
                                                            };

                                                            font_renderer.draw_text(
                                                                &mut *d,
                                                                "Flashing in progress...",
                                                                20,
                                                                20,
                                                                20.0,
                                                                Rgb888::new(255, 255, 255),
                                                            );

                                                            let status = format!(
                                                                "Received: {} / {} bytes",
                                                                curr, total
                                                            );
                                                            font_renderer.draw_text(
                                                                &mut *d,
                                                                &status,
                                                                20,
                                                                60,
                                                                16.0,
                                                                Rgb888::new(200, 200, 200),
                                                            );

                                                            // Draw simple bar
                                                            let filled_len =
                                                                (progress as usize / 5).min(20);
                                                            let empty_len = 20 - filled_len;
                                                            let mut bar_str = String::from("[");
                                                            for _ in 0..filled_len {
                                                                bar_str.push('=');
                                                            }
                                                            for _ in 0..empty_len {
                                                                bar_str.push(' ');
                                                            }
                                                            bar_str.push(']');
                                                            let bar_display = format!(
                                                                "{} {}%",
                                                                bar_str, progress
                                                            );

                                                            font_renderer.draw_text(
                                                                &mut *d,
                                                                &bar_display,
                                                                20,
                                                                100,
                                                                16.0,
                                                                Rgb888::new(0, 255, 0),
                                                            );
                                                            d.flush();
                                                        },
                                                    );

                                                    match res {
                                                        Ok(downloaded_image) => {
                                                            crate::info!("Flashing Complete!");
                                                            uefi::boot::stall(1_000_000);

                                                            crate::info!(
                                                                "Attempting to boot from RAM..."
                                                            );
                                                            let cmdline = "console=ttyS0";
                                                            // Note: boot_from_memory might not return if successful
                                                            if let Err(e) = boot::boot_from_memory(
                                                                &downloaded_image,
                                                                None,
                                                                cmdline,
                                                            ) {
                                                                crate::error!(
                                                                    "RAM Boot failed: {:?}",
                                                                    e
                                                                );
                                                                // Need to re-borrow display to show error
                                                                let mut d = display.borrow_mut();
                                                                let err_msg =
                                                                    format!("Boot Error: {:?}", e);
                                                                font_renderer.draw_text(
                                                                    &mut *d,
                                                                    &err_msg,
                                                                    20,
                                                                    140,
                                                                    16.0,
                                                                    Rgb888::new(255, 0, 0),
                                                                );
                                                                d.flush();
                                                                uefi::boot::stall(5_000_000);
                                                            }
                                                            break;
                                                        }
                                                        Err(e) => {
                                                            crate::error!(
                                                                "Download failed: {:?}",
                                                                e
                                                            );
                                                            let mut d = display.borrow_mut();
                                                            let err_msg =
                                                                format!("Download Error: {:?}", e);
                                                            font_renderer.draw_text(
                                                                &mut *d,
                                                                &err_msg,
                                                                20,
                                                                140,
                                                                16.0,
                                                                Rgb888::new(255, 0, 0),
                                                            );
                                                            d.flush();
                                                            uefi::boot::stall(3_000_000);
                                                        }
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                *redraw.borrow_mut() = true;
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

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

use uefi::allocator::Allocator;

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;
