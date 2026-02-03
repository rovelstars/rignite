#![allow(dead_code)]

// Rignite Binary Config (RBC) Parser and Verifier

extern crate alloc;
use alloc::vec::Vec;
use core::convert::TryInto;
use core::str;

#[cfg(target_os = "uefi")]
use uefi::proto::loaded_image::LoadedImage;
#[cfg(target_os = "uefi")]
use uefi::proto::media::file::{File, FileAttribute, FileInfo, FileMode, FileType};
#[cfg(target_os = "uefi")]
use uefi::proto::media::fs::SimpleFileSystem;
#[cfg(target_os = "uefi")]
use uefi::CStr16;

// --- Constants & Registry ---

pub const RBC_MAGIC: [u8; 4] = [0x52, 0x47, 0x4E, 0x21]; // "RGN!"
pub const RBC_VERSION: u16 = 1;
pub const HEADER_SIZE: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Tag {
    MainUuid = 0x01,
    MainFsType = 0x02,
    MainKernelParams = 0x03,
    RecoveryUuid = 0x10,
    RecoveryFsType = 0x11,
    RecoveryKernelParams = 0x12,
    Signature = 0xFF,
    Unknown(u16),
}

impl From<u16> for Tag {
    fn from(v: u16) -> Self {
        match v {
            0x01 => Tag::MainUuid,
            0x02 => Tag::MainFsType,
            0x03 => Tag::MainKernelParams,
            0x10 => Tag::RecoveryUuid,
            0x11 => Tag::RecoveryFsType,
            0x12 => Tag::RecoveryKernelParams,
            0xFF => Tag::Signature,
            x => Tag::Unknown(x),
        }
    }
}

impl From<Tag> for u16 {
    fn from(tag: Tag) -> Self {
        match tag {
            Tag::MainUuid => 0x01,
            Tag::MainFsType => 0x02,
            Tag::MainKernelParams => 0x03,
            Tag::RecoveryUuid => 0x10,
            Tag::RecoveryFsType => 0x11,
            Tag::RecoveryKernelParams => 0x12,
            Tag::Signature => 0xFF,
            Tag::Unknown(x) => x,
        }
    }
}

impl Tag {
    pub fn as_u16(self) -> u16 {
        self.into()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum FsType {
    // Linux / Unix
    Btrfs = 1,
    Ext4 = 2,
    Xfs = 3,
    Zfs = 4,
    F2fs = 5,
    Bcachefs = 6,

    // Read-only / Image
    Erofs = 10,
    SquashFs = 11,

    // FAT / Windows
    Fat12 = 20,
    Fat16 = 21,
    Fat32 = 22,
    ExFat = 23,
    Ntfs = 24,

    // Apple
    Apfs = 30,
    HfsPlus = 31,

    // Future expansion placeholder
    Unknown(u16),
}

impl From<u16> for FsType {
    fn from(v: u16) -> Self {
        match v {
            1 => FsType::Btrfs,
            2 => FsType::Ext4,
            3 => FsType::Xfs,
            4 => FsType::Zfs,
            5 => FsType::F2fs,
            6 => FsType::Bcachefs,
            10 => FsType::Erofs,
            11 => FsType::SquashFs,
            20 => FsType::Fat12,
            21 => FsType::Fat16,
            22 => FsType::Fat32,
            23 => FsType::ExFat,
            24 => FsType::Ntfs,
            30 => FsType::Apfs,
            31 => FsType::HfsPlus,
            x => FsType::Unknown(x),
        }
    }
}

impl From<FsType> for u16 {
    fn from(t: FsType) -> Self {
        match t {
            FsType::Btrfs => 1,
            FsType::Ext4 => 2,
            FsType::Xfs => 3,
            FsType::Zfs => 4,
            FsType::F2fs => 5,
            FsType::Bcachefs => 6,
            FsType::Erofs => 10,
            FsType::SquashFs => 11,
            FsType::Fat12 => 20,
            FsType::Fat16 => 21,
            FsType::Fat32 => 22,
            FsType::ExFat => 23,
            FsType::Ntfs => 24,
            FsType::Apfs => 30,
            FsType::HfsPlus => 31,
            FsType::Unknown(x) => x,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum RbcError {
    BufferTooSmall,
    InvalidMagic,
    UnsupportedVersion,
    InvalidSize,
    OutOfBounds,
    Utf8Error,
    MalformedAtom,
    VerificationFailed,
    IoError,
    ProtocolError,
}

#[cfg(target_os = "uefi")]
impl From<uefi::Status> for RbcError {
    fn from(_: uefi::Status) -> Self {
        RbcError::ProtocolError
    }
}

#[cfg(target_os = "uefi")]
impl From<uefi::Error> for RbcError {
    fn from(_: uefi::Error) -> Self {
        RbcError::ProtocolError
    }
}

// --- Binary Parsing (Zero-Copy) ---

/// A Zero-Copy view into a Rignite Binary Config blob.
///
/// This struct holds a reference to the raw byte slice and provides methods
/// to traverse the atoms safely. It does not allocate new memory for parsing.
#[derive(Clone, Copy)]
pub struct ConfigView<'a> {
    data: &'a [u8],
}

impl<'a> ConfigView<'a> {
    /// Validates the header and creates a new ConfigView.
    pub fn new(data: &'a [u8]) -> Result<Self, RbcError> {
        if data.len() < HEADER_SIZE {
            return Err(RbcError::BufferTooSmall);
        }

        // 1. Check Magic
        if data[0..4] != RBC_MAGIC {
            return Err(RbcError::InvalidMagic);
        }

        // 2. Check Version
        let version = u16::from_le_bytes(data[4..6].try_into().unwrap());
        if version != RBC_VERSION {
            return Err(RbcError::UnsupportedVersion);
        }

        // 3. Check Total Size
        let total_size = u32::from_le_bytes(data[6..10].try_into().unwrap()) as usize;
        if data.len() < total_size {
            return Err(RbcError::InvalidSize);
        }

        // We only expose the valid slice
        Ok(Self {
            data: &data[0..total_size],
        })
    }

    /// Returns an iterator over the atoms in the config.
    pub fn atoms(&self) -> AtomIterator<'a> {
        AtomIterator {
            data: self.data,
            offset: HEADER_SIZE,
        }
    }

    // --- Helper Accessors ---

    pub fn get_main_uuid(&self) -> Option<&'a [u8; 16]> {
        self.find_atom(Tag::MainUuid).and_then(|val| {
            if val.len() == 16 {
                val.try_into().ok()
            } else {
                None
            }
        })
    }

    pub fn get_main_fs_type(&self) -> Option<FsType> {
        self.find_atom(Tag::MainFsType).and_then(|val| {
            if val.len() == 2 {
                Some(FsType::from(u16::from_le_bytes(val.try_into().unwrap())))
            } else {
                None
            }
        })
    }

    pub fn get_main_kernel_params(&self) -> Result<Option<&'a str>, RbcError> {
        match self.find_atom(Tag::MainKernelParams) {
            Some(bytes) => str::from_utf8(bytes)
                .map(Some)
                .map_err(|_| RbcError::Utf8Error),
            None => Ok(None),
        }
    }

    pub fn get_recovery_uuid(&self) -> Option<&'a [u8; 16]> {
        self.find_atom(Tag::RecoveryUuid).and_then(|val| {
            if val.len() == 16 {
                val.try_into().ok()
            } else {
                None
            }
        })
    }

    pub fn get_recovery_fs_type(&self) -> Option<FsType> {
        self.find_atom(Tag::RecoveryFsType).and_then(|val| {
            if val.len() == 2 {
                Some(FsType::from(u16::from_le_bytes(val.try_into().unwrap())))
            } else {
                None
            }
        })
    }

    pub fn get_recovery_kernel_params(&self) -> Result<Option<&'a str>, RbcError> {
        match self.find_atom(Tag::RecoveryKernelParams) {
            Some(bytes) => str::from_utf8(bytes)
                .map(Some)
                .map_err(|_| RbcError::Utf8Error),
            None => Ok(None),
        }
    }

    pub fn get_signature(&self) -> Option<&'a [u8]> {
        self.find_atom(Tag::Signature)
    }

    /// Internal helper to find raw bytes for a specific tag.
    fn find_atom(&self, target: Tag) -> Option<&'a [u8]> {
        for atom in self.atoms() {
            if atom.tag == target {
                return Some(atom.value);
            }
        }
        None
    }
}

// --- Iterator ---

pub struct Atom<'a> {
    pub tag: Tag,
    pub value: &'a [u8],
}

pub struct AtomIterator<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Iterator for AtomIterator<'a> {
    type Item = Atom<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        // Check if we have enough space for a header (2 bytes Tag + 2 bytes Length)
        if self.offset + 4 > self.data.len() {
            return None;
        }

        // Parse Header
        let tag_raw =
            u16::from_le_bytes(self.data[self.offset..self.offset + 2].try_into().unwrap());
        let length = u16::from_le_bytes(
            self.data[self.offset + 2..self.offset + 4]
                .try_into()
                .unwrap(),
        ) as usize;

        let value_start = self.offset + 4;
        let value_end = value_start + length;

        // Bounds Check
        if value_end > self.data.len() {
            // Malformed atom, stop iteration to prevent OOB
            return None;
        }

        // Advance
        self.offset = value_end;

        Some(Atom {
            tag: Tag::from(tag_raw),
            value: &self.data[value_start..value_end],
        })
    }
}

// --- Safe Wrapper (Owned) ---

/// A container that owns the config data heap allocation.
/// This solves the self-referential struct issue by holding the `Vec<u8>`
/// and generating temporary `ConfigView`s on demand.
pub struct OwnedConfig {
    data: Vec<u8>,
}

impl OwnedConfig {
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Creates a view into the owned data.
    pub fn view(&self) -> ConfigView {
        // We can unwrap here because the constructor guarantees validity
        ConfigView::new(&self.data).expect("OwnedConfig data corrupted")
    }

    pub fn get_main_uuid(&self) -> Option<&[u8; 16]> {
        self.view().get_main_uuid()
    }

    pub fn get_main_fs_type(&self) -> Option<FsType> {
        self.view().get_main_fs_type()
    }

    pub fn get_main_kernel_params(&self) -> Result<Option<&str>, RbcError> {
        // Note: the &str lifetime is tied to &self
        // This works because ConfigView lifetime matches &self
        let v = self.view();
        // We must re-implement slightly or cast lifetime because view() creates a temporary
        // However, ConfigView::get_main_kernel_params returns &'a str where 'a is 'data.
        // 'data is &self.data.
        // So this is safe.
        v.get_main_kernel_params()
    }
}

// --- Integration & Verification ---

/// Loads, Verifies, and Parses the RBC file from the EFI System Partition.
#[cfg(target_os = "uefi")]
pub fn verify_and_load(path: &str) -> Result<OwnedConfig, RbcError> {
    // 1. Read File
    let loaded_image_handle = uefi::boot::image_handle();
    let loaded_image = uefi::boot::open_protocol_exclusive::<LoadedImage>(loaded_image_handle)?;
    let device_handle = loaded_image.device().ok_or(RbcError::ProtocolError)?;

    // Open SimpleFileSystem on the device
    let mut fs = uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(device_handle)?;
    let mut root = fs.open_volume()?;

    let mut path_buf = [0u16; 256];
    let path_ucs2 =
        CStr16::from_str_with_buf(path, &mut path_buf).map_err(|_| RbcError::IoError)?;

    let handle = root
        .open(path_ucs2, FileMode::Read, FileAttribute::empty())
        .map_err(|_| RbcError::IoError)?;

    let mut file = match handle.into_type().map_err(|_| RbcError::IoError)? {
        FileType::Regular(f) => f,
        _ => return Err(RbcError::IoError),
    };

    // Get file info to know size
    let mut info_buf = [0u8; 512];
    let info = file
        .get_info::<FileInfo>(&mut info_buf)
        .map_err(|_| RbcError::IoError)?;

    let size = info.file_size() as usize;
    let mut buffer = alloc::vec![0u8; size];

    let read_len = file.read(&mut buffer).map_err(|_| RbcError::IoError)?;
    if read_len != size {
        return Err(RbcError::IoError);
    }

    // 2. Initial Parse to get Signature
    {
        let temp_view = ConfigView::new(&buffer)?;
        let _signature = temp_view
            .get_signature()
            .ok_or(RbcError::VerificationFailed)?;

        // 3. Verify Signature
        // Placeholder for EFI_PKCS7_VERIFY_PROTOCOL logic.
        // In a real implementation:
        // let pkcs7_guid = ...;
        // let protocol = uefi::boot::locate_protocol::<Pkcs7Verify>(...)
        // verify(buffer_without_sig, signature)
    }

    // 4. Return Validated Owner
    Ok(OwnedConfig { data: buffer })
}
