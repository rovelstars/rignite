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
use uefi::proto::media::block::BlockIO;
use uefi::proto::media::fs::SimpleFileSystem;
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
mod rbc;

use embedded_graphics::pixelcolor::Rgb888;
use font::FontRenderer;
use graphics::UefiDisplay;
use icons::Icon;
use input::InputHandler;

#[derive(Clone, Debug)]
pub enum MenuItem {
    RunixOS { handle: uefi::Handle },
    MetaEntry, // "[+] Boot from another Drive/Partition..."

    // Secondary
    PhysicalDrive { name: String, handle: uefi::Handle },
    Partition { name: String, handle: uefi::Handle },
    EfiFile { name: String, path: String },

    FirmwareSettings,
    Reboot,
    Shutdown,
    Back,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MenuState {
    Primary,
    DriveList,
    PartitionList(uefi::Handle),
    FileSelection(uefi::Handle),
}

pub enum UIAction {
    BootLinux {
        handle: uefi::Handle,
        params: Option<String>,
    },
    Chainload {
        handle: uefi::Handle,
        path: String,
    },
    FirmwareSettings,
    Reboot,
    Shutdown,
}

// Helper to populate menu items based on state
fn get_menu_items(state: &MenuState, handles: &[uefi::Handle]) -> Vec<MenuItem> {
    let mut items = Vec::new();

    match state {
        MenuState::Primary => {
            // 1. Scan for RunixOS
            for handle in handles {
                let block_io_res = unsafe {
                    uefi::boot::open_protocol::<BlockIO>(
                        uefi::boot::OpenProtocolParams {
                            handle: *handle,
                            agent: uefi::boot::image_handle(),
                            controller: None,
                        },
                        uefi::boot::OpenProtocolAttributes::GetProtocol,
                    )
                };

                if let Ok(block_io) = block_io_res {
                    if let Ok(Some(btrfs)) = fs::btrfs::Btrfs::new(&block_io) {
                        if btrfs.get_label() == "RunixOS" {
                            items.push(MenuItem::RunixOS { handle: *handle });
                        }
                    }
                }
            }

            items.push(MenuItem::MetaEntry);
            items.push(MenuItem::FirmwareSettings);
            items.push(MenuItem::Reboot);
            items.push(MenuItem::Shutdown);
        }
        MenuState::DriveList => {
            items.push(MenuItem::Back);
            for handle in handles {
                let block_io_res = unsafe {
                    uefi::boot::open_protocol::<BlockIO>(
                        uefi::boot::OpenProtocolParams {
                            handle: *handle,
                            agent: uefi::boot::image_handle(),
                            controller: None,
                        },
                        uefi::boot::OpenProtocolAttributes::GetProtocol,
                    )
                };

                if let Ok(block_io) = block_io_res {
                    let media = block_io.media();
                    if !media.is_media_present() {
                        continue;
                    }
                    let size = (media.block_size() as u64).saturating_mul(media.last_block() + 1);
                    if size < 1024 * 1024 {
                        continue;
                    } // Skip small

                    let size_str = if size >= 1024 * 1024 * 1024 {
                        format!("{}GB", size / (1024 * 1024 * 1024))
                    } else {
                        format!("{}MB", size / (1024 * 1024))
                    };

                    let mut name = format!("Drive ({})", size_str);

                    // Btrfs check
                    if let Ok(Some(btrfs)) = fs::btrfs::Btrfs::new(&block_io) {
                        let label = btrfs.get_label();
                        if !label.is_empty() {
                            name = format!("{} ({})", label, size_str);
                        }
                    } else {
                        // FAT/EFI check
                        let fs_check = unsafe {
                            uefi::boot::open_protocol::<SimpleFileSystem>(
                                uefi::boot::OpenProtocolParams {
                                    handle: *handle,
                                    agent: uefi::boot::image_handle(),
                                    controller: None,
                                },
                                uefi::boot::OpenProtocolAttributes::GetProtocol,
                            )
                        };
                        if fs_check.is_ok() {
                            name = format!("EFI/FAT ({})", size_str);
                        }
                    }

                    items.push(MenuItem::PhysicalDrive {
                        name,
                        handle: *handle,
                    });
                }
            }
        }
        MenuState::PartitionList(handle) => {
            items.push(MenuItem::Back);
            // Treat the handle as a partition for now (1:1 mapping)
            items.push(MenuItem::Partition {
                name: "Default Partition".into(),
                handle: *handle,
            });
        }
        MenuState::FileSelection(handle) => {
            items.push(MenuItem::Back);
            let fs_proto = unsafe {
                uefi::boot::open_protocol::<SimpleFileSystem>(
                    uefi::boot::OpenProtocolParams {
                        handle: *handle,
                        agent: uefi::boot::image_handle(),
                        controller: None,
                    },
                    uefi::boot::OpenProtocolAttributes::GetProtocol,
                )
            };

            if let Ok(mut fs) = fs_proto {
                if let Ok(mut root) = fs.open_volume() {
                    let mut apps = Vec::new();
                    let mut quibble = None;
                    boot::scan_dir_recursive(&mut root, "", &mut apps, &mut quibble);

                    for app in apps {
                        items.push(MenuItem::EfiFile {
                            name: app.clone(),
                            path: app,
                        });
                    }
                }
            }

            if items.len() == 1 {
                // Only Back
                items.push(MenuItem::EfiFile {
                    name: "No EFI files found".into(),
                    path: "".into(),
                });
            }
        }
    }
    items
}

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

    // Load Resources
    let font_data = include_bytes!("../assets/font.data");
    let drive_icon_data = include_bytes!("../assets/drive.qoi");
    let firmware_icon_data = include_bytes!("../assets/firmware.qoi");
    let reboot_icon_data = include_bytes!("../assets/reboot.qoi");
    let shutdown_icon_data = include_bytes!("../assets/shutdown.qoi");

    // Load Configuration
    let config = match rbc::verify_and_load("\\EFI\\RovelStars\\CONF\\boot.rbc") {
        Ok(c) => {
            crate::info!("Loaded boot configuration.");
            Some(c)
        }
        Err(e) => {
            crate::warn!("Failed to load boot config: {:?}. Using defaults.", e);
            None
        }
    };

    // Scan for Block Devices
    let handles = uefi::boot::locate_handle_buffer(SearchType::ByProtocol(&BlockIO::GUID))
        .expect("Failed to locate BlockIo handles");

    // Auto-detect RunixOS for auto-boot logic (before menu loop)
    let mut runixos_handle: Option<uefi::Handle> = None;
    for handle in handles.iter() {
        if let Ok(block_io) = unsafe {
            uefi::boot::open_protocol::<BlockIO>(
                uefi::boot::OpenProtocolParams {
                    handle: *handle,
                    agent: uefi::boot::image_handle(),
                    controller: None,
                },
                uefi::boot::OpenProtocolAttributes::GetProtocol,
            )
        } {
            if let Ok(Some(btrfs)) = fs::btrfs::Btrfs::new(&block_io) {
                if btrfs.get_label() == "RunixOS" {
                    runixos_handle = Some(*handle);
                    break;
                }
            }
        }
    }
    let action = core::cell::RefCell::new(None);

    // Phase 1: Splash Screen / Auto-boot Decision
    {
        // Initialize Graphics (Scoped)
        let gop_handle = uefi::boot::get_handle_for_protocol::<GraphicsOutput>().unwrap();
        let mut gop = uefi::boot::open_protocol_exclusive::<GraphicsOutput>(gop_handle).unwrap();
        let mut display = UefiDisplay::new(&mut gop);
        display.set_highest_resolution().ok();

        let (width, height) = (display.width(), display.height());
        let font_renderer = FontRenderer::new(font_data);
        let logo_icon = logo::Logo::new();

        use core::cell::RefCell;
        let display = RefCell::new(display);

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
                            let msg =
                                format!("Press UP & DOWN again ({}) to enter menu...", remaining);
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
                    // Set action to boot
                    let params = config
                        .as_ref()
                        .and_then(|c| c.get_main_kernel_params().ok().flatten())
                        .map(|s| String::from(s));
                    *action.borrow_mut() = Some(UIAction::BootLinux { handle, params });
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
        });
    } // End Phase 1 (GOP dropped)

    // Reconnect Graphics Console for Text Output
    if let Ok(gop_handle) = uefi::boot::get_handle_for_protocol::<GraphicsOutput>() {
        let _ = uefi::boot::connect_controller(gop_handle, None, None, true);
    }

    // Execute Auto-Boot if set
    if let Some(act) = action.into_inner() {
        match act {
            UIAction::BootLinux { handle, params } => {
                crate::info!("Auto-booting RunixOS...");
                if let Err(e) = boot::boot_linux_from_drive(handle, params.as_deref()) {
                    crate::error!("Failed to auto-boot: {:?}", e);
                }
            }
            _ => {}
        }
    }

    use core::cell::RefCell;
    let current_state = RefCell::new(MenuState::Primary);
    let menu_items = RefCell::new(get_menu_items(&current_state.borrow(), &handles));
    let mut selected_index: usize = 0;

    // Main Interactive Loop
    loop {
        // Run UI Session (Acquire GOP)
        let ui_action = {
            let gop_handle = uefi::boot::get_handle_for_protocol::<GraphicsOutput>().unwrap();
            let mut gop =
                uefi::boot::open_protocol_exclusive::<GraphicsOutput>(gop_handle).unwrap();
            let mut display = UefiDisplay::new(&mut gop);
            display.set_highest_resolution().ok();

            let (width, height) = (display.width(), display.height());
            let font_renderer = FontRenderer::new(font_data);
            let drive_icon = Icon::new(drive_icon_data, 196, 196);
            let firmware_icon = Icon::new(firmware_icon_data, 32, 32);
            let reboot_icon = Icon::new(reboot_icon_data, 32, 32);
            let shutdown_icon = Icon::new(shutdown_icon_data, 32, 32);

            let display = RefCell::new(display);
            let selected_index_cell = RefCell::new(selected_index);
            let result_action = RefCell::new(None);

            let ret_idx = uefi::system::with_stdin(|stdin| {
                let mut input_handler = InputHandler::new(stdin);

                // Clear screen
                display
                    .borrow_mut()
                    .clear(Rgb888::new(0, 0, 0))
                    .expect("Failed to clear screen");
                display.borrow_mut().flush();

                let redraw = RefCell::new(true);
                crate::info!("Entering interactive loop...");

                // Initialize scales for animation
                let mut item_scales: Vec<f32> = Vec::new();
                for _ in 0..menu_items.borrow().len() {
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

                        // Map items to names and icons for drawing
                        let main_list: Vec<(String, &Icon)> = menu_items
                            .borrow()
                            .iter()
                            .filter_map(|item: &MenuItem| {
                                match item {
                                    MenuItem::RunixOS { .. } => {
                                        Some(("RunixOS".into(), &drive_icon))
                                    }
                                    MenuItem::MetaEntry => {
                                        Some(("[+] Other...".into(), &drive_icon))
                                    }
                                    MenuItem::PhysicalDrive { name, .. } => {
                                        Some((name.clone(), &drive_icon))
                                    }
                                    MenuItem::Partition { name, .. } => {
                                        Some((name.clone(), &drive_icon))
                                    }
                                    MenuItem::EfiFile { name, .. } => {
                                        Some((name.clone(), &drive_icon))
                                    }
                                    MenuItem::Back => Some(("Back".into(), &drive_icon)), // TODO: Back icon
                                    _ => None, // System items handled separately
                                }
                            })
                            .collect();

                        // Layout: Main List in center
                        let item_width = (icon_size * 2) as i32;
                        let is_vertical = height > width;

                        let title = match *current_state.borrow() {
                            MenuState::Primary => "Rignite Boot Menu",
                            MenuState::DriveList => "Select Drive",
                            MenuState::PartitionList(_) => "Select Partition",
                            MenuState::FileSelection(_) => "Select EFI Application",
                        };

                        let title_size = 28.0;
                        let title_width = font_renderer.get_text_width(title, title_size);
                        let title_x = (width as i32 - title_width) / 2;

                        font_renderer.draw_text(
                            &mut *disp,
                            title,
                            title_x,
                            100,
                            title_size,
                            Rgb888::new(255, 255, 255),
                        );

                        // Draw central items
                        let list_count = main_list.len();
                        let drives_start_x =
                            (width as i32 / 2) - ((list_count as i32 * item_width) / 2);
                        let drives_y = (height as i32 / 2) - (icon_size as i32 / 2);
                        let vertical_item_height = icon_size as i32 + 60;
                        let drives_start_y_vertical =
                            (height as i32 / 2) - ((list_count as i32 * vertical_item_height) / 2);

                        let mut list_idx = 0;

                        for (i, item) in menu_items.borrow().iter().enumerate() {
                            // Check if it's a main list item
                            let (name, icon) = match item {
                                MenuItem::RunixOS { .. } => ("RunixOS", &drive_icon),
                                MenuItem::MetaEntry => ("[+] Other...", &drive_icon),
                                MenuItem::PhysicalDrive { name, .. } => {
                                    (name.as_str(), &drive_icon)
                                }
                                MenuItem::Partition { name, .. } => (name.as_str(), &drive_icon),
                                MenuItem::EfiFile { name, .. } => (name.as_str(), &drive_icon),
                                MenuItem::Back => ("Back", &drive_icon),
                                _ => continue, // Skip system items here
                            };

                            let (x, y) = if is_vertical {
                                (
                                    (width as i32 / 2) - (item_width / 2),
                                    drives_start_y_vertical
                                        + (list_idx as i32 * vertical_item_height),
                                )
                            } else {
                                (drives_start_x + (list_idx as i32 * item_width), drives_y)
                            };

                            // Draw Icon
                            let scale = item_scales[i];
                            let scaled_size = (icon_size as f32 * scale) as u32;

                            let center_x = x + item_width / 2;
                            let center_y = y + icon_size as i32 / 2;
                            let draw_x = center_x - (scaled_size as i32 / 2);
                            let draw_y = center_y - (scaled_size as i32 / 2);

                            icon.draw_scaled(&mut *disp, draw_x, draw_y, scaled_size, 1.0);

                            // Draw Text
                            let label_size = 18.0;
                            let text_width = font_renderer.get_text_width(name, label_size);
                            let text_x = x + (item_width / 2) - (text_width / 2);
                            let text_y = y + icon_size as i32 + 20;

                            let color = if i == *selected_index_cell.borrow() {
                                Rgb888::new(255, 255, 0)
                            } else {
                                Rgb888::new(200, 200, 200)
                            };

                            font_renderer
                                .draw_text(&mut *disp, name, text_x, text_y, label_size, color);

                            list_idx += 1;
                        }

                        // Draw system options at bottom (smaller than drives)
                        let sys_icon_size = (icon_size * 3) / 10; // 30% of drive icon size
                        let sys_options_y = height as i32 - sys_icon_size as i32 - 80; // 80px margin from bottom
                        let sys_option_count = 3; // Firmware, Reboot, Shutdown
                        let sys_item_width = (sys_icon_size * 3) as i32; // Spacing b/w system options
                        let sys_start_x =
                            (width as i32 / 2) - ((sys_option_count * sys_item_width) / 2);

                        let mut sys_idx = 0;
                        for (i, item) in menu_items.borrow().iter().enumerate() {
                            let (icon, name) = match item {
                                MenuItem::FirmwareSettings => (&firmware_icon, "FW"),
                                MenuItem::Reboot => (&reboot_icon, "Reboot"),
                                MenuItem::Shutdown => (&shutdown_icon, "Shutdown"),
                                _ => continue,
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

                            let color = if i == *selected_index_cell.borrow() {
                                crate::debug!(
                                    "System option {} '{}' is SELECTED (yellow)",
                                    i,
                                    name
                                );
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
                        let mut idx = selected_index_cell.borrow_mut();
                        let count = menu_items.borrow().len();
                        match key {
                            Key::Printable(c)
                                if u16::from(c) == 'd' as u16 || u16::from(c) == 's' as u16 =>
                            {
                                *idx = (*idx + 1) % count; // Wraparound
                                *redraw.borrow_mut() = true;
                            }
                            Key::Special(ScanCode::RIGHT) | Key::Special(ScanCode::DOWN) => {
                                *idx = (*idx + 1) % count; // Wraparound
                                *redraw.borrow_mut() = true;
                            }
                            Key::Printable(c)
                                if u16::from(c) == 'a' as u16 || u16::from(c) == 'w' as u16 =>
                            {
                                *idx = if *idx == 0 { count - 1 } else { *idx - 1 }; // Wraparound
                                *redraw.borrow_mut() = true;
                            }
                            Key::Special(ScanCode::LEFT) | Key::Special(ScanCode::UP) => {
                                *idx = if *idx == 0 { count - 1 } else { *idx - 1 }; // Wraparound
                                *redraw.borrow_mut() = true;
                            }
                            Key::Printable(c) if u16::from(c) == '\r' as u16 => {
                                // Clear screen before action
                                {
                                    let mut d = display.borrow_mut();
                                    d.clear(Rgb888::new(0, 0, 0)).ok();
                                    d.flush();
                                }

                                let selected = menu_items.borrow()[*idx].clone();

                                match selected {
                                    MenuItem::RunixOS { handle } => {
                                        crate::info!("Booting RunixOS...");
                                        let params: Option<&str> = config.as_ref().and_then(|c| {
                                            c.get_main_kernel_params().ok().flatten()
                                        });

                                        *result_action.borrow_mut() = Some(UIAction::BootLinux {
                                            handle,
                                            params: params.map(|s| String::from(s)),
                                        });
                                        break;
                                    }
                                    MenuItem::MetaEntry => {
                                        *current_state.borrow_mut() = MenuState::DriveList;
                                        *menu_items.borrow_mut() =
                                            get_menu_items(&current_state.borrow(), &handles);
                                        *idx = 0;
                                        item_scales = vec![1.0; menu_items.borrow().len()];
                                        *redraw.borrow_mut() = true;
                                    }
                                    MenuItem::PhysicalDrive { handle, .. } => {
                                        *current_state.borrow_mut() =
                                            MenuState::PartitionList(handle);
                                        *menu_items.borrow_mut() =
                                            get_menu_items(&current_state.borrow(), &handles);
                                        *idx = 0;
                                        item_scales = vec![1.0; menu_items.borrow().len()];
                                        *redraw.borrow_mut() = true;
                                    }
                                    MenuItem::Partition { handle, .. } => {
                                        *current_state.borrow_mut() =
                                            MenuState::FileSelection(handle);
                                        *menu_items.borrow_mut() =
                                            get_menu_items(&current_state.borrow(), &handles);
                                        *idx = 0;
                                        item_scales = vec![1.0; menu_items.borrow().len()];
                                        *redraw.borrow_mut() = true;
                                    }
                                    MenuItem::EfiFile { path, .. } => {
                                        if path.is_empty() {
                                            // No Op
                                        } else {
                                            if let MenuState::FileSelection(handle) =
                                                *current_state.borrow()
                                            {
                                                *result_action.borrow_mut() =
                                                    Some(UIAction::Chainload {
                                                        handle,
                                                        path: path.clone(),
                                                    });
                                                break;
                                            }
                                        }
                                    }
                                    MenuItem::Back => {
                                        let new_state = match *current_state.borrow() {
                                            MenuState::FileSelection(h) => {
                                                MenuState::PartitionList(h)
                                            }
                                            MenuState::PartitionList(_) => MenuState::DriveList,
                                            MenuState::DriveList => MenuState::Primary,
                                            s => s,
                                        };
                                        *current_state.borrow_mut() = new_state;
                                        *menu_items.borrow_mut() =
                                            get_menu_items(&current_state.borrow(), &handles);
                                        *idx = 0;
                                        item_scales = vec![1.0; menu_items.borrow().len()];
                                        *redraw.borrow_mut() = true;
                                    }
                                    MenuItem::FirmwareSettings => {
                                        *result_action.borrow_mut() =
                                            Some(UIAction::FirmwareSettings);
                                        break;
                                    }
                                    MenuItem::Reboot => {
                                        *result_action.borrow_mut() = Some(UIAction::Reboot);
                                        break;
                                    }
                                    MenuItem::Shutdown => {
                                        *result_action.borrow_mut() = Some(UIAction::Shutdown);
                                        break;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }

                    // Stall to reduce CPU usage and maintain approx 30 FPS
                    uefi::boot::stall(33_000);
                }
                *selected_index_cell.borrow()
            });

            selected_index = ret_idx;
            let res = result_action.borrow_mut().take();
            res
        }; // End UI Scope

        // Reconnect Graphics Console for Text Output
        if let Ok(gop_handle) = uefi::boot::get_handle_for_protocol::<GraphicsOutput>() {
            let _ = uefi::boot::connect_controller(gop_handle, None, None, true);
        }

        if let Some(action) = ui_action {
            match action {
                UIAction::BootLinux { handle, params } => {
                    if let Err(e) = boot::boot_linux_from_drive(handle, params.as_deref()) {
                        crate::error!("Boot failed: {:?}", e);
                        // Continue loop
                    }
                }
                UIAction::Chainload { handle, path } => {
                    if let Err(e) = boot::boot_efi_app(handle, &path) {
                        crate::error!("Chainload failed: {:?}", e);
                    }
                }
                UIAction::FirmwareSettings => {
                    // Set OsIndications variable to boot to firmware UI
                    const EFI_OS_INDICATIONS_BOOT_TO_FW_UI: u64 = 0x0000000000000001;
                    let vendor = VariableVendor::GLOBAL_VARIABLE;
                    let mut buf = [0u16; 32];
                    let var_name = CStr16::from_str_with_buf("OsIndications", &mut buf).unwrap();
                    let value = EFI_OS_INDICATIONS_BOOT_TO_FW_UI.to_le_bytes();
                    let attrs = VariableAttributes::BOOTSERVICE_ACCESS
                        | VariableAttributes::RUNTIME_ACCESS
                        | VariableAttributes::NON_VOLATILE;

                    let _ = uefi::runtime::set_variable(var_name, &vendor, attrs, &value);
                    uefi::runtime::reset(ResetType::COLD, uefi::Status::SUCCESS, None);
                }
                UIAction::Reboot => {
                    uefi::runtime::reset(ResetType::COLD, uefi::Status::SUCCESS, None);
                }
                UIAction::Shutdown => {
                    uefi::runtime::reset(ResetType::SHUTDOWN, uefi::Status::SUCCESS, None);
                }
            }
        }
    } // End Loop
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

use uefi::allocator::Allocator;

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;
