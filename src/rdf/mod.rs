use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;
use sha2::{Digest, Sha256};
use uefi::boot::{
    locate_handle_buffer, open_protocol, open_protocol_exclusive, OpenProtocolAttributes,
    OpenProtocolParams, SearchType,
};
use uefi::proto::Protocol;
use uefi::{Guid, Identify, Status};

#[repr(C)]
pub struct PciIo {
    pub _pad: [usize; 6],
    pub pci: PciIoAccess,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PciIoAccess {
    pub read: unsafe extern "efiapi" fn(
        this: *mut PciIo,
        width: u32,
        offset: u32,
        count: usize,
        buffer: *mut c_void,
    ) -> Status,
    pub write: usize,
}

unsafe impl Identify for PciIo {
    const GUID: Guid = Guid::parse_or_panic("4cf5b200-68b8-4ca5-9eec-b23e3f50029a");
}
impl Protocol for PciIo {}

struct Usb2Hc;
unsafe impl Identify for Usb2Hc {
    const GUID: Guid = Guid::parse_or_panic("3e745226-9818-45b6-a2ac-d7cd0e8ba2bc");
}
impl Protocol for Usb2Hc {}

// --- RDF Header Definition ---

// Magic Bytes: 0x52 0x44 0x46 0x21 ("RDF!")
pub const RDF_MAGIC: [u8; 4] = [0x52, 0x44, 0x46, 0x21];

// AOA Constants
const AOA_GET_PROTOCOL: u8 = 51;
const AOA_SEND_STRING: u8 = 52;
const AOA_START: u8 = 53;

/// The RDF Header (128 bytes)
/// Sent by the host before the image stream to verify protocol and integrity.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct RdfHeader {
    pub magic: [u8; 4],
    pub image_size: u64,
    pub checksum: [u8; 32], // Blake3 or Sha256 of the incoming blob
    pub target_subvolume: [u8; 64],
    pub reserved: [u8; 20],
}

impl Default for RdfHeader {
    fn default() -> Self {
        Self {
            magic: [0; 4],
            image_size: 0,
            checksum: [0; 32],
            target_subvolume: [0; 64],
            reserved: [0; 20],
        }
    }
}

impl RdfHeader {
    pub fn is_valid(&self) -> bool {
        self.magic == RDF_MAGIC
    }
}

// --- Mock Data Source ---

pub struct MockDataSource {
    data: Vec<u8>,
    cursor: usize,
}

impl MockDataSource {
    pub fn new() -> Self {
        let mut data = Vec::new();
        // Magic
        data.extend_from_slice(&RDF_MAGIC);
        // Size: 1MB payload
        let payload_size = 1024 * 1024;
        data.extend_from_slice(&(payload_size as u64).to_le_bytes());
        // Checksum (dummy)
        data.extend_from_slice(&[0xAA; 32]);
        // Target: @core
        let mut target = [0u8; 64];
        let s = b"@core";
        target[0..s.len()].copy_from_slice(s);
        data.extend_from_slice(&target);
        // Reserved
        data.extend_from_slice(&[0u8; 20]);
        // Payload (Pattern)
        for i in 0..payload_size {
            data.push((i % 255) as u8);
        }

        Self { data, cursor: 0 }
    }

    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let remaining = self.data.len() - self.cursor;
        let count = remaining.min(buf.len());
        if count == 0 {
            return 0;
        }

        buf[0..count].copy_from_slice(&self.data[self.cursor..self.cursor + count]);
        self.cursor += count;
        count
    }
}

// --- USB Definitions ---

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct UsbDeviceRequest {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbDataDirection {
    NoData = 0x00,
    DataIn = 0x01,
    DataOut = 0x02,
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct UsbDeviceDescriptor {
    pub length: u8,
    pub descriptor_type: u8,
    pub bcd_usb: u16,
    pub device_class: u8,
    pub device_sub_class: u8,
    pub device_protocol: u8,
    pub max_packet_size0: u8,
    pub id_vendor: u16,
    pub id_product: u16,
    pub bcd_device: u16,
    pub i_manufacturer: u8,
    pub i_product: u8,
    pub i_serial_number: u8,
    pub num_configurations: u8,
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct UsbInterfaceDescriptor {
    pub length: u8,
    pub descriptor_type: u8,
    pub interface_number: u8,
    pub alternate_setting: u8,
    pub num_endpoints: u8,
    pub interface_class: u8,
    pub interface_sub_class: u8,
    pub interface_protocol: u8,
    pub i_interface: u8,
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct UsbEndpointDescriptor {
    pub length: u8,
    pub descriptor_type: u8,
    pub endpoint_address: u8,
    pub attributes: u8,
    pub max_packet_size: u16,
    pub interval: u8,
}

#[repr(C)]
pub struct UsbIo {
    pub control_transfer: unsafe extern "efiapi" fn(
        this: *mut UsbIo,
        request: *mut UsbDeviceRequest,
        direction: u32,
        timeout: u32,
        data: *mut u8,
        data_length: *mut usize,
        status: *mut u32,
    ) -> Status,

    pub bulk_transfer: unsafe extern "efiapi" fn(
        this: *mut UsbIo,
        device_endpoint: u8,
        data: *mut u8,
        data_length: *mut usize,
        timeout: u32,
        status: *mut u32,
    ) -> Status,

    // AsyncInterrupt, SyncInterrupt, Isochronous, AsyncIsochronous
    _pad: [usize; 4],

    pub get_device_descriptor: unsafe extern "efiapi" fn(
        this: *mut UsbIo,
        device_descriptor: *mut UsbDeviceDescriptor,
    ) -> Status,

    pub get_config_descriptor: unsafe extern "efiapi" fn(
        this: *mut UsbIo,
        config_descriptor: *mut u8, // Placeholder
    ) -> Status,

    pub get_interface_descriptor: unsafe extern "efiapi" fn(
        this: *mut UsbIo,
        interface_descriptor: *mut UsbInterfaceDescriptor,
    ) -> Status,

    pub get_endpoint_descriptor: unsafe extern "efiapi" fn(
        this: *mut UsbIo,
        endpoint_index: u8,
        endpoint_descriptor: *mut UsbEndpointDescriptor,
    ) -> Status,
}

unsafe impl Identify for UsbIo {
    // EFI_USB_IO_PROTOCOL_GUID: 2B2F68D6-0CD2-44cf-823B-9DA9DD869684
    const GUID: Guid = Guid::parse_or_panic("2B2F68D6-0CD2-44cf-823B-9DA9DD869684");
}

impl Protocol for UsbIo {}

impl UsbIo {
    pub fn get_device_descriptor(&mut self) -> Result<UsbDeviceDescriptor, Status> {
        let mut descriptor = UsbDeviceDescriptor::default();
        let status = unsafe { (self.get_device_descriptor)(self as *mut _, &mut descriptor) };

        if status.is_success() {
            Ok(descriptor)
        } else {
            Err(status)
        }
    }

    pub fn get_interface_descriptor(&mut self) -> Result<UsbInterfaceDescriptor, Status> {
        let mut descriptor = UsbInterfaceDescriptor::default();
        let status = unsafe { (self.get_interface_descriptor)(self as *mut _, &mut descriptor) };

        if status.is_success() {
            Ok(descriptor)
        } else {
            Err(status)
        }
    }

    pub fn get_endpoint_descriptor(&mut self, index: u8) -> Result<UsbEndpointDescriptor, Status> {
        let mut descriptor = UsbEndpointDescriptor::default();
        let status =
            unsafe { (self.get_endpoint_descriptor)(self as *mut _, index, &mut descriptor) };

        if status.is_success() {
            Ok(descriptor)
        } else {
            Err(status)
        }
    }

    pub fn aoa_validate_protocol(&mut self) -> Result<u16, Status> {
        let mut req = UsbDeviceRequest {
            request_type: 0xC0, // Dir=IN, Type=Vendor, Recipient=Device
            request: AOA_GET_PROTOCOL,
            value: 0,
            index: 0,
            length: 2,
        };
        let mut data = [0u8; 2];
        self.control_transfer(&mut req, UsbDataDirection::DataIn, 2000, &mut data)?;
        Ok(u16::from_le_bytes(data))
    }

    pub fn aoa_send_string(&mut self, index: u16, s: &str) -> Result<(), Status> {
        let bytes = s.as_bytes();
        let mut req = UsbDeviceRequest {
            request_type: 0x40, // Dir=OUT, Type=Vendor, Recipient=Device
            request: AOA_SEND_STRING,
            value: 0,
            index,
            length: 0, // Set after buffer creation
        };
        // We need a mutable buffer for control_transfer
        let mut buffer = Vec::from(bytes);
        buffer.push(0); // Null terminate
        req.length = buffer.len() as u16;

        self.control_transfer(&mut req, UsbDataDirection::DataOut, 2000, &mut buffer)?;
        Ok(())
    }

    pub fn aoa_start(&mut self) -> Result<(), Status> {
        let mut req = UsbDeviceRequest {
            request_type: 0x40,
            request: AOA_START,
            value: 0,
            index: 0,
            length: 0,
        };
        let mut data = [];
        self.control_transfer(&mut req, UsbDataDirection::DataOut, 2000, &mut data)?;
        Ok(())
    }

    pub fn control_transfer(
        &mut self,
        request: &mut UsbDeviceRequest,
        direction: UsbDataDirection,
        timeout: u32,
        data: &mut [u8],
    ) -> Result<usize, Status> {
        let mut len = data.len();
        let mut status_code = 0;
        let status = unsafe {
            (self.control_transfer)(
                self as *mut _,
                request as *mut _,
                direction as u32,
                timeout,
                data.as_mut_ptr(),
                &mut len,
                &mut status_code,
            )
        };

        if status.is_success() {
            Ok(len)
        } else {
            Err(status)
        }
    }

    pub fn bulk_transfer(
        &mut self,
        endpoint: u8,
        data: &mut [u8],
        timeout: u32,
    ) -> Result<usize, Status> {
        let mut len = data.len();
        let mut status_code = 0;
        let status = unsafe {
            (self.bulk_transfer)(
                self as *mut _,
                endpoint,
                data.as_mut_ptr(),
                &mut len,
                timeout,
                &mut status_code,
            )
        };

        if status.is_success() {
            Ok(len)
        } else {
            Err(status)
        }
    }
}

// --- Discovery Manager ---

#[derive(Debug)]
pub struct UsbDevice {
    pub handle: uefi::Handle,
    pub vid: u16,
    pub pid: u16,
}

pub struct RdfManager;

impl RdfManager {
    fn kickstart_usb() {
        crate::debug!("Kickstarting USB controllers...");
        // Locate all PCI handles (which includes USB Host Controllers) and force connection
        match locate_handle_buffer(SearchType::ByProtocol(&PciIo::GUID)) {
            Ok(handles) => {
                crate::debug!(
                    "Found {} PCI handles. Attempting to connect drivers...",
                    handles.len()
                );
                for (i, handle) in handles.iter().enumerate() {
                    let mut pci_info = String::new();

                    unsafe {
                        if let Ok(pci_io) = open_protocol::<PciIo>(
                            OpenProtocolParams {
                                handle: *handle,
                                agent: uefi::boot::image_handle(),
                                controller: None,
                            },
                            OpenProtocolAttributes::GetProtocol,
                        ) {
                            let pci_io_ptr = &*pci_io as *const PciIo as *mut PciIo;
                            let mut buffer = 0u32;
                            let status = ((*pci_io_ptr).pci.read)(
                                pci_io_ptr,
                                2,    // Uint32
                                0x08, // Offset for RevID/ProgIF/SubClass/Class
                                1,    // Count
                                &mut buffer as *mut _ as *mut c_void,
                            );

                            if status.is_success() {
                                let class = (buffer >> 24) as u8;
                                let subclass = (buffer >> 16) as u8;
                                let prog_if = (buffer >> 8) as u8;

                                // USB Controller is Class 0x0C, Subclass 0x03
                                let is_usb = class == 0x0C && subclass == 0x03;
                                let usb_desc = if is_usb {
                                    match prog_if {
                                        0x00 => " (USB UHCI)",
                                        0x10 => " (USB OHCI)",
                                        0x20 => " (USB EHCI)",
                                        0x30 => " (USB XHCI)",
                                        _ => " (USB Unknown)",
                                    }
                                } else {
                                    ""
                                };

                                pci_info = alloc::format!(
                                    " [Class: {:02x} Sub: {:02x} Prog: {:02x}{}]",
                                    class,
                                    subclass,
                                    prog_if,
                                    usb_desc
                                );
                            }
                        }

                        // Recursive connect
                        match uefi::boot::connect_controller(*handle, None, None, true) {
                            Ok(_) => {
                                crate::debug!("PCI Handle #{}: Driver connected.{}", i, pci_info);
                            }
                            Err(e) => {
                                crate::debug!(
                                    "PCI Handle #{}: Connect failed: {:?}{}",
                                    i,
                                    e.status(),
                                    pci_info
                                );
                            }
                        }

                        // Check if USB HC Protocol is present now
                        if open_protocol::<Usb2Hc>(
                            OpenProtocolParams {
                                handle: *handle,
                                agent: uefi::boot::image_handle(),
                                controller: None,
                            },
                            OpenProtocolAttributes::GetProtocol,
                        )
                        .is_ok()
                        {
                            crate::debug!(
                                "  -> Handle #{} supports EFI_USB2_HC_PROTOCOL (Host Controller Active)",
                                i
                            );
                        }
                    }
                }
            }
            Err(e) => {
                crate::error!("Failed to locate PCI handles: {:?}", e);
            }
        }

        // Explicitly connect handles with EFI_USB2_HC_PROTOCOL
        // This forces the USB Bus driver to load if it wasn't automatically triggered by the PCI connect.
        match locate_handle_buffer(SearchType::ByProtocol(&Usb2Hc::GUID)) {
            Ok(handles) => {
                for (i, handle) in handles.iter().enumerate() {
                    match uefi::boot::connect_controller(*handle, None, None, true) {
                        Ok(_) => {
                            crate::debug!(
                                "Usb2Hc Handle #{}: Connected (Bus Driver should be active)",
                                i
                            );
                        }
                        Err(e) => {
                            crate::debug!("Usb2Hc Handle #{}: Connect failed: {:?}", i, e.status());
                        }
                    }
                }
            }
            Err(e) => {
                if e.status() != Status::NOT_FOUND {
                    crate::debug!("Failed to locate Usb2Hc handles: {:?}", e);
                }
            }
        }
    }

    /// List all connected USB devices by scanning handles supporting `UsbIo`.
    pub fn list_devices() -> uefi::Result<Vec<UsbDevice>> {
        // Force driver connection on controllers to ensure child devices are enumerated
        Self::kickstart_usb();

        crate::info!("Scanning for USB devices...");
        let mut devices = Vec::new();

        // Add Mock Device (VID=0xDEAD, PID=0xBEEF)
        // We use a dummy pointer for the handle.
        // WARNING: Do not try to open protocols on this handle!
        static mut MOCK_TARGET: u8 = 0;
        if let Some(mock_handle) =
            unsafe { uefi::Handle::from_ptr(core::ptr::addr_of_mut!(MOCK_TARGET) as *mut _) }
        {
            devices.push(UsbDevice {
                handle: mock_handle,
                vid: 0xDEAD,
                pid: 0xBEEF,
            });
        }

        // Find all handles supporting UsbIo
        let handles = match locate_handle_buffer(SearchType::ByProtocol(&UsbIo::GUID)) {
            Ok(h) => h,
            Err(e) => {
                if e.status() == Status::NOT_FOUND {
                    crate::warn!("No USB handles found via LocateHandleBuffer.");
                    return Ok(devices);
                }
                crate::error!("Failed to locate USB handles: {:?}", e);
                return Err(e);
            }
        };

        crate::info!("Found {} USB handles.", handles.len());
        for (i, handle) in handles.iter().enumerate() {
            // We use GetProtocol to peek at the device without disconnecting drivers
            let res = unsafe {
                open_protocol::<UsbIo>(
                    OpenProtocolParams {
                        handle: *handle,
                        agent: uefi::boot::image_handle(),
                        controller: None,
                    },
                    OpenProtocolAttributes::GetProtocol,
                )
            };

            match res {
                Ok(usb_io) => {
                    // Use raw pointer to avoid UB from casting &T to &mut T
                    let usb_io_ptr = &*usb_io as *const UsbIo as *mut UsbIo;
                    let mut descriptor = UsbDeviceDescriptor::default();
                    let status = unsafe {
                        ((*usb_io_ptr).get_device_descriptor)(usb_io_ptr, &mut descriptor)
                    };

                    if status.is_success() {
                        crate::info!(
                            "USB Device Found (Handle #{:?}): VID={:#x} PID={:#x}",
                            handle,
                            descriptor.id_vendor,
                            descriptor.id_product
                        );
                        devices.push(UsbDevice {
                            handle: *handle,
                            vid: descriptor.id_vendor,
                            pid: descriptor.id_product,
                        });
                    } else {
                        crate::error!("Handle #{}: Failed to get descriptor: {:?}", i, status);
                    }
                }
                Err(e) => {
                    crate::warn!("Handle #{}: Open (GetProtocol) failed: {:?}", i, e);
                }
            }
        }

        Ok(devices)
    }

    /// Connect to the device, handshake, and download the OS image.
    pub fn download_image<F>(device: &UsbDevice, progress_callback: F) -> uefi::Result<Vec<u8>>
    where
        F: Fn(usize, usize),
    {
        // --- Mock Logic ---
        if device.vid == 0xDEAD && device.pid == 0xBEEF {
            crate::info!("Starting Mock Download...");
            let mut mock_source = MockDataSource::new();
            let mut buffer = [0u8; 1024]; // 1KB chunks
            let mut total_read = 0;
            let mut downloaded_image = Vec::new();

            // Read Header
            let mut header_bytes = [0u8; 128];
            mock_source.read(&mut header_bytes);

            // Validate Magic
            if header_bytes[0..4] != RDF_MAGIC {
                return Err(uefi::Error::new(Status::LOAD_ERROR, ()));
            }

            // Get Size
            let image_size = u64::from_le_bytes(header_bytes[4..12].try_into().unwrap()) as usize;

            // Stream Data
            loop {
                let read = mock_source.read(&mut buffer);
                if read == 0 {
                    break;
                }
                downloaded_image.extend_from_slice(&buffer[0..read]);
                total_read += read;

                progress_callback(total_read, image_size);

                // Simulate latency
                uefi::boot::stall(5_000);
            }

            return Ok(downloaded_image);
        }

        // --- Real USB Logic ---
        crate::info!("Connecting to USB Device (Handle: {:?})...", device.handle);
        let mut usb_io = open_protocol_exclusive::<UsbIo>(device.handle)?;

        // Check for AOA (Google VID + Accessory PID)
        let is_accessory = device.vid == 0x18D1 && (device.pid == 0x2D00 || device.pid == 0x2D01);

        if !is_accessory {
            crate::info!(
                "Device not in Accessory Mode (VID={:#x}). Attempting handshake...",
                device.vid
            );

            // 1. Get Protocol
            match usb_io.aoa_validate_protocol() {
                Ok(proto) if proto >= 1 => {
                    crate::info!("AOA Protocol v{} detected.", proto);

                    // 2. Send Strings
                    usb_io.aoa_send_string(0, "Rignite")?;
                    usb_io.aoa_send_string(1, "RDF")?;
                    usb_io.aoa_send_string(2, "Rignite Device Flasher")?;
                    usb_io.aoa_send_string(3, "1.0")?;
                    usb_io.aoa_send_string(4, "https://rignite.io")?;
                    usb_io.aoa_send_string(5, "1234567890")?;

                    // 3. Start
                    crate::info!("Sending AOA Start command...");
                    usb_io.aoa_start()?;

                    // Device will disconnect and re-enumerate
                    crate::info!("Accessory mode requested. Device should re-enumerate.");
                    return Err(uefi::Error::new(Status::TIMEOUT, ()));
                }
                Ok(_) => {
                    crate::warn!("AOA not supported (ver < 1).");
                }
                Err(e) => {
                    crate::warn!("Failed to query AOA protocol: {:?}", e);
                }
            }
        } else {
            crate::info!("Device is already in Accessory Mode.");
        }

        // 1. Find Bulk IN Endpoint
        let interface_desc = usb_io.get_interface_descriptor()?;
        let mut endpoint_in = None;

        for i in 0..interface_desc.num_endpoints {
            if let Ok(ep_desc) = usb_io.get_endpoint_descriptor(i) {
                // Bit 7: Direction (1 = Device to Host = IN)
                // Bits 1..0: Transfer Type (2 = Bulk)
                let is_in = (ep_desc.endpoint_address & 0x80) != 0;
                let is_bulk = (ep_desc.attributes & 0x03) == 2;

                if is_in && is_bulk {
                    endpoint_in = Some(ep_desc.endpoint_address);
                    crate::debug!("Found Bulk IN Endpoint: {:#x}", ep_desc.endpoint_address);
                    break;
                }
            }
        }

        let ep_addr = endpoint_in.ok_or(uefi::Error::new(Status::NOT_FOUND, ()))?;

        // 2. Read RDF Header (Scanning)
        crate::info!("Waiting for RDF Header (Scanning)...");
        let mut scan_buffer = Vec::new();
        scan_buffer.resize(65536, 0); // 64KB buffer
        let (magic_offset, packet_len) = loop {
            // 1s timeout for scanning, loop until found
            match usb_io.bulk_transfer(ep_addr, &mut scan_buffer, 1_000) {
                Ok(len) => {
                    if len == 0 {
                        continue;
                    }

                    // Search for magic bytes: RDF!
                    if let Some(offset) =
                        scan_buffer[0..len].windows(4).position(|w| w == RDF_MAGIC)
                    {
                        crate::info!("Found Magic Bytes at offset {}", offset);
                        break (offset, len);
                    } else {
                        crate::debug!("Skipping {} bytes of non-header data...", len);
                    }
                }
                Err(e) if e == Status::TIMEOUT => {
                    continue;
                }
                Err(e) => return Err(uefi::Error::new(e, ())),
            }
        };

        if packet_len - magic_offset < 128 {
            crate::error!(
                "Header split across packets. Available: {}",
                packet_len - magic_offset
            );
            return Err(uefi::Error::new(Status::PROTOCOL_ERROR, ()));
        }

        // 3. Parse Header
        let header_slice = &scan_buffer[magic_offset..magic_offset + 128];
        // Magic is checked by search logic above

        let image_size = u64::from_le_bytes(header_slice[4..12].try_into().unwrap()) as usize;
        let expected_checksum: [u8; 32] = header_slice[12..44].try_into().unwrap();

        crate::info!(
            "Header Valid. Image Size: {} bytes. Downloading...",
            image_size
        );

        // 4. Download Image
        let mut image_data = Vec::with_capacity(image_size);
        let mut hasher = Sha256::new();

        // Handle payload bytes that came with the header packet
        let header_end = magic_offset + 128;
        if packet_len > header_end {
            let payload_bytes = packet_len - header_end;
            crate::info!(
                "Initial packet contained {} bytes of payload",
                payload_bytes
            );
            let data_slice = &scan_buffer[header_end..packet_len];
            image_data.extend_from_slice(data_slice);
            hasher.update(data_slice);
        }

        // 64KB Transfer chunks
        let chunk_size = 64 * 1024;
        let mut buffer = Vec::new();
        buffer.resize(chunk_size, 0);

        let mut total_read = image_data.len();
        progress_callback(total_read, image_size);

        while total_read < image_size {
            let remaining = image_size - total_read;
            let to_read = remaining.min(chunk_size);

            let mut retries = 0;
            const MAX_RETRIES: usize = 5;
            let mut chunk_success = false;

            while retries < MAX_RETRIES {
                // 5s timeout per chunk
                match usb_io.bulk_transfer(ep_addr, &mut buffer[0..to_read], 5_000) {
                    Ok(read) => {
                        if read == 0 {
                            // ZLP handling - treated as success (no data)
                            chunk_success = true;
                            break;
                        }

                        let data_slice = &buffer[0..read];
                        image_data.extend_from_slice(data_slice);
                        hasher.update(data_slice);
                        total_read += read;

                        progress_callback(total_read, image_size);
                        chunk_success = true;
                        break;
                    }
                    Err(e) => {
                        retries += 1;
                        crate::warn!(
                            "Bulk transfer failed (chunk start: {}): {:?}. Retry {}/{}",
                            total_read,
                            e,
                            retries,
                            MAX_RETRIES
                        );

                        if retries >= MAX_RETRIES {
                            return Err(uefi::Error::new(e, ()));
                        }

                        // Stall 50ms before retry
                        uefi::boot::stall(50_000);
                    }
                }
            }

            if !chunk_success {
                return Err(uefi::Error::new(Status::DEVICE_ERROR, ()));
            }
        }

        if total_read < image_size {
            crate::error!("Download incomplete: {}/{} bytes", total_read, image_size);
            return Err(uefi::Error::new(Status::END_OF_FILE, ()));
        }

        // Verify Checksum
        let calculated_checksum = hasher.finalize();
        if calculated_checksum.as_slice() != expected_checksum {
            crate::error!("Checksum mismatch!");
            crate::error!("Expected: {:02x?}", expected_checksum);
            crate::error!("Actual:   {:02x?}", calculated_checksum);
            return Err(uefi::Error::new(Status::CRC_ERROR, ()));
        }
        crate::info!("Checksum verified.");

        crate::info!("Download complete.");
        Ok(image_data)
    }
}
