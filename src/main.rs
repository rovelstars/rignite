#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use uefi::boot::SearchType;
use uefi::proto::console::gop::GraphicsOutput;
use uefi::proto::console::text::{Key, ScanCode};
use uefi::proto::media::block::BlockIO;
use uefi::runtime::{ResetType, VariableAttributes, VariableVendor};
use uefi::CStr16;
use uefi::Identify;

mod font;
mod graphics;
mod icons;
mod input;
mod logger;

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
    let logo_icon_data = include_bytes!("../assets/logo.qoi");

    // Init Renderers
    let font_renderer = FontRenderer::new(font_data);
    let drive_icon = Icon::new(drive_icon_data, 512, 512);
    let firmware_icon = Icon::new(firmware_icon_data, 512, 512);
    let reboot_icon = Icon::new(reboot_icon_data, 512, 512);
    let shutdown_icon = Icon::new(shutdown_icon_data, 512, 512);
    let logo_icon = Icon::new(logo_icon_data, 512, 512);

    // Logo Animation
    display
        .clear(Rgb888::new(0, 0, 0))
        .expect("Failed to clear screen");

    let logo_size = 64;
    let center_x = (width as i32 / 2) - (logo_size as i32 / 2);
    let center_y = (height as i32 / 2) - (logo_size as i32 / 2);

    // Fade In
    crate::debug!("Starting logo fade-in...");
    for i in 0..=60 {
        display.clear(Rgb888::new(0, 0, 0)).ok();
        let opacity = i as f32 / 60.0;
        logo_icon.draw_scaled(&mut display, center_x, center_y, logo_size, opacity);
        display.flush();
        uefi::boot::stall(16_670);
    }

    uefi::boot::stall(2000_000); // Hold

    // Fade Out
    crate::debug!("Starting logo fade-out...");
    for i in (0..=60).rev() {
        display.clear(Rgb888::new(0, 0, 0)).ok();
        let opacity = i as f32 / 60.0;
        logo_icon.draw_scaled(&mut display, center_x, center_y, logo_size, opacity);
        display.flush();
        uefi::boot::stall(16_670);
    }

    // Clear screen for menu
    display
        .clear(Rgb888::new(0, 0, 0))
        .expect("Failed to clear screen");
    display.flush();

    // Scan for Block Devices
    let handles = uefi::boot::locate_handle_buffer(SearchType::ByProtocol(&BlockIO::GUID))
        .expect("Failed to locate BlockIo handles");

    #[derive(Clone)]
    enum MenuItem {
        Drive { name: String },
        FirmwareSettings,
        Reboot,
        Shutdown,
    }

    let mut menu_items = Vec::new();

    // Add drives
    for handle in handles.iter() {
        if let Ok(block_io) = uefi::boot::open_protocol_exclusive::<BlockIO>(*handle) {
            let media = block_io.media();
            if media.is_media_present() {
                let size_gb = media.block_size() as u64 * media.last_block() / (1024 * 1024 * 1024);
                let name = if media.is_removable_media() {
                    format!("Removable {}GB", size_gb)
                } else {
                    format!("HDD {}GB", size_gb)
                };
                menu_items.push(MenuItem::Drive { name });
            }
        }
    }

    if menu_items.is_empty() {
        menu_items.push(MenuItem::Drive {
            name: String::from("No Drives Found"),
        });
    }

    // Add system options
    menu_items.push(MenuItem::FirmwareSettings);
    menu_items.push(MenuItem::Reboot);
    menu_items.push(MenuItem::Shutdown);

    use core::cell::RefCell;

    let selected_index = RefCell::new(0);
    let redraw = RefCell::new(true);
    let display = RefCell::new(display);

    crate::info!("Entering interactive loop...");

    // Use with_stdin to access input safely
    uefi::system::with_stdin(|stdin| {
        // Initialize scales for animation
        let mut item_scales: Vec<f32> = Vec::new();
        for _ in 0..menu_items.len() {
            item_scales.push(1.0);
        }

        let mut input_handler = InputHandler::new(stdin);

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
                crate::debug!("Drawing title at x={}, y={}", (width as i32 / 2) - 100, 100);
                font_renderer.draw_text(
                    &mut *disp,
                    title,
                    (width as i32 / 2) - 100,
                    100,
                    32.0,
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
                    if let MenuItem::Drive { name } = item {
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
                        let text_x = x + (item_width / 2) - (name.len() as i32 * 4);
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
                        font_renderer.draw_text(&mut *disp, name, text_x, text_y, 20.0, color);

                        drive_idx += 1;
                    }
                }

                // Draw system options at bottom (smaller than drives)
                let sys_icon_size = (icon_size * 6) / 10; // 60% of drive icon size
                let sys_options_y = height as i32 - sys_icon_size as i32 - 80;
                let sys_option_count = 3; // Firmware, Reboot, Shutdown
                let sys_item_width = (sys_icon_size * 2) as i32;
                let sys_start_x = (width as i32 / 2) - ((sys_option_count * sys_item_width) / 2);

                let mut sys_idx = 0;
                for (i, item) in menu_items.iter().enumerate() {
                    let (icon, name) = match item {
                        MenuItem::FirmwareSettings => (&firmware_icon, "Firmware Settings"),
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
                    let text_x = x + (sys_item_width / 2) - (name.len() as i32 * 3);
                    let text_y = y + sys_icon_size as i32 + 15;

                    let color = if i == *selected_index.borrow() {
                        crate::debug!("System option {} '{}' is SELECTED (yellow)", i, name);
                        Rgb888::new(255, 255, 0)
                    } else {
                        Rgb888::new(200, 200, 200)
                    };

                    font_renderer.draw_text(&mut *disp, name, text_x, text_y, 16.0, color); // Smaller font
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
                            MenuItem::Drive { name } => {
                                crate::debug!("Selected drive: {}", name);
                                break;
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

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

use uefi::allocator::Allocator;

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;
