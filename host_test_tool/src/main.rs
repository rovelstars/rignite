use anyhow::{Context, Result};
use rusb::{Device, DeviceDescriptor, DeviceHandle, Direction, GlobalContext, TransferType};
use sha2::{Digest, Sha256};
use std::time::Duration;
use std::{thread, time};

// AOA Constants
const AOA_GET_PROTOCOL: u8 = 51;
const AOA_SEND_STRING: u8 = 52;
const AOA_START: u8 = 53;

// Rignite Constants
const MANUFACTURER: &str = "Rignite";
const MODEL: &str = "RDF";
const DESCRIPTION: &str = "Rignite Device Flasher";
const VERSION: &str = "1.0";
const URI: &str = "https://rignite.io";
const SERIAL: &str = "1234567890";

const GOOGLE_VID: u16 = 0x18D1;
const ACCESSORY_PIDS: &[u16] = &[0x2D00, 0x2D01];

fn main() -> Result<()> {
    println!("=== RDF Host Test Tool ===");
    println!("Waiting for device...");

    loop {
        if let Ok(Some((device, desc))) = find_target_device() {
            println!(
                "Found device: VID={:04x} PID={:04x}",
                desc.vendor_id(),
                desc.product_id()
            );

            if desc.vendor_id() == GOOGLE_VID && ACCESSORY_PIDS.contains(&desc.product_id()) {
                println!("Device is in Accessory Mode! Ready to receive.");
                if let Err(e) = handle_accessory_mode(device) {
                    match e.downcast_ref::<rusb::Error>() {
                        Some(rusb::Error::NoDevice)
                        | Some(rusb::Error::Io)
                        | Some(rusb::Error::NotFound) => {
                            println!("Device disconnected.");
                        }
                        _ => eprintln!("Error handling accessory: {:?}", e),
                    }
                }
            } else {
                println!("Device in standard mode. Attempting AOA handshake...");
                if let Err(e) = perform_aoa_handshake(device) {
                    eprintln!("Handshake failed: {:?}", e);
                } else {
                    println!("Handshake complete. Waiting for re-enumeration...");
                    thread::sleep(time::Duration::from_secs(5));
                }
            }
        }
        thread::sleep(time::Duration::from_secs(1));
    }
}

fn find_target_device() -> Result<Option<(Device<GlobalContext>, DeviceDescriptor)>> {
    for device in rusb::devices()?.iter() {
        let desc = device.device_descriptor()?;

        // Target OnePlus (0x22d9) or Google Accessory (0x18d1)
        if desc.vendor_id() == 0x22d9 || desc.vendor_id() == GOOGLE_VID {
            return Ok(Some((device, desc)));
        }
    }
    Ok(None)
}

fn perform_aoa_handshake(device: Device<GlobalContext>) -> Result<()> {
    let mut handle = device.open()?;

    // 1. Get Protocol
    let mut buf = [0u8; 2];
    let len = handle.read_control(
        0xC0,             // Dir=IN, Type=Vendor, Recipient=Device
        AOA_GET_PROTOCOL, // Request
        0,                // Value
        0,                // Index
        &mut buf,
        Duration::from_secs(1),
    )?;

    if len != 2 {
        anyhow::bail!("Failed to read protocol version");
    }

    let protocol_ver = u16::from_le_bytes(buf);
    println!("AOA Protocol Version: {}", protocol_ver);

    if protocol_ver < 1 {
        anyhow::bail!("Device does not support AOA 1.0");
    }

    // 2. Send Strings
    send_string(&mut handle, 0, MANUFACTURER)?;
    send_string(&mut handle, 1, MODEL)?;
    send_string(&mut handle, 2, DESCRIPTION)?;
    send_string(&mut handle, 3, VERSION)?;
    send_string(&mut handle, 4, URI)?;
    send_string(&mut handle, 5, SERIAL)?;

    println!("Strings sent. Sending START command...");

    // 3. Start
    handle.write_control(
        0x40,      // Dir=OUT, Type=Vendor, Recipient=Device
        AOA_START, // Request
        0,
        0,
        &[],
        Duration::from_secs(1),
    )?;

    Ok(())
}

fn send_string(handle: &mut DeviceHandle<GlobalContext>, index: u16, s: &str) -> Result<()> {
    let mut data = s.as_bytes().to_vec();
    data.push(0); // Null terminator

    handle.write_control(
        0x40,            // Dir=OUT, Type=Vendor, Recipient=Device
        AOA_SEND_STRING, // Request
        0,
        index,
        &data,
        Duration::from_secs(1),
    )?;
    Ok(())
}

fn handle_accessory_mode(device: Device<GlobalContext>) -> Result<()> {
    let handle = device.open()?;

    // Find Bulk IN endpoint
    let config = device.active_config_descriptor()?;
    let interface = config.interfaces().next().context("No interfaces found")?;
    let interface_desc = interface.descriptors().next().context("No descriptor")?;

    let endpoint = interface_desc
        .endpoint_descriptors()
        .find(|ep| ep.direction() == Direction::In && ep.transfer_type() == TransferType::Bulk)
        .context("No Bulk IN endpoint found")?;

    let endpoint_address = endpoint.address();
    println!("Found Bulk IN Endpoint: 0x{:02x}", endpoint_address);

    handle.claim_interface(interface.number())?;

    println!("Waiting for RDF Header (Scanning)...");

    let mut buffer = [0u8; 65536]; // 64KB buffer
                                   // Scan until we find the magic bytes
    let (packet_len, magic_offset) = loop {
        match handle.read_bulk(endpoint_address, &mut buffer, Duration::from_secs(1)) {
            Ok(len) => {
                if len == 0 {
                    continue;
                }

                // Search for magic bytes: RDF!
                if let Some(offset) = buffer[0..len]
                    .windows(4)
                    .position(|w| w == [0x52, 0x44, 0x46, 0x21])
                {
                    println!("Found Magic Bytes at offset {}", offset);
                    break (len, offset);
                } else {
                    println!("Skipping {} bytes of non-header data...", len);
                }
            }
            Err(rusb::Error::Timeout) => continue, // Keep waiting
            Err(e) => return Err(e.into()),
        }
    };

    // Check if we have enough data for the header
    if packet_len - magic_offset < 128 {
        anyhow::bail!(
            "Header split across packets. Available: {}",
            packet_len - magic_offset
        );
    }

    let header_slice = &buffer[magic_offset..magic_offset + 128];
    // Magic is already validated by the search.

    let size_bytes: [u8; 8] = header_slice[4..12].try_into()?;
    let image_size = u64::from_le_bytes(size_bytes);

    let expected_checksum: [u8; 32] = header_slice[12..44].try_into()?;

    println!("Header Received!");
    println!("  Magic: Valid");
    println!("  Image Size: {} bytes", image_size);

    let mut hasher = Sha256::new();

    println!("Receiving Data...");
    let mut total_received: u64 = 0;

    // Account for any payload bytes received in the first packet
    let header_end = magic_offset + 128;
    if packet_len > header_end {
        let payload_bytes = (packet_len - header_end) as u64;
        total_received += payload_bytes;
        println!(
            "  Initial packet contained {} bytes of payload",
            payload_bytes
        );
        hasher.update(&buffer[header_end..packet_len]);
    }

    let start_time = time::Instant::now();

    while total_received < image_size {
        let len = handle.read_bulk(endpoint_address, &mut buffer, Duration::from_secs(5))?;
        if len == 0 {
            continue;
        }
        total_received += len as u64;
        hasher.update(&buffer[0..len]);

        // Print progress every MB
        if total_received % (1024 * 1024) < 65536 {
            print!(
                "\rProgress: {} / {} bytes ({:.1}%)",
                total_received,
                image_size,
                (total_received as f64 / image_size as f64) * 100.0
            );
        }
    }

    let duration = start_time.elapsed();
    println!("\nTransfer Complete!");
    println!("  Time: {:.2}s", duration.as_secs_f64());
    println!(
        "  Speed: {:.2} MB/s",
        (total_received as f64 / 1024.0 / 1024.0) / duration.as_secs_f64()
    );

    let calculated_checksum = hasher.finalize();
    if calculated_checksum.as_slice() != expected_checksum {
        eprintln!("Checksum mismatch!");
        eprintln!("Expected: {:02x?}", expected_checksum);
        eprintln!("Actual:   {:02x?}", calculated_checksum);
        anyhow::bail!("Checksum verification failed");
    }
    println!("Checksum verified.");

    Ok(())
}
