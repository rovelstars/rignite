// src/fs/btrfs.rs
#![allow(dead_code)]

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::mem;
use core::slice;
use uefi::proto::media::block::BlockIO;
use uefi::{Error, Result, Status};

// -----------------------------------------------------------------------------
// Constants & Types
// -----------------------------------------------------------------------------

pub type BtrfsChecksum = [u8; 32];
pub type BtrfsUuid = [u8; 16];

pub const BTRFS_SIGNATURE: &[u8; 8] = b"_BHRfS_M";
pub const BTRFS_MAX_NUM_DEVICES: usize = 256;
pub const BTRFS_SUPER_INFO_OFFSET: u64 = 64 * 1024;

// Object IDs
pub const BTRFS_ROOT_TREE_OBJECTID: u64 = 1;
pub const BTRFS_EXTENT_TREE_OBJECTID: u64 = 2;
pub const BTRFS_CHUNK_TREE_OBJECTID: u64 = 3;
pub const BTRFS_DEV_TREE_OBJECTID: u64 = 4;
pub const BTRFS_FS_TREE_OBJECTID: u64 = 5;
pub const BTRFS_ROOT_TREE_DIR_OBJECTID: u64 = 6;
pub const BTRFS_FIRST_CHUNK_TREE_OBJECTID: u64 = 256;

// Key Types
pub const BTRFS_INODE_ITEM_KEY: u8 = 1;
pub const BTRFS_INODE_REF_KEY: u8 = 12;
pub const BTRFS_INODE_EXTREF_KEY: u8 = 13;
pub const BTRFS_XATTR_ITEM_KEY: u8 = 24;
pub const BTRFS_ORPHAN_ITEM_KEY: u8 = 48;
pub const BTRFS_DIR_LOG_ITEM_KEY: u8 = 60;
pub const BTRFS_DIR_LOG_INDEX_KEY: u8 = 72;
pub const BTRFS_DIR_ITEM_KEY: u8 = 84;
pub const BTRFS_DIR_INDEX_KEY: u8 = 96;
pub const BTRFS_EXTENT_DATA_KEY: u8 = 108;
pub const BTRFS_EXTENT_CSUM_KEY: u8 = 128;
pub const BTRFS_ROOT_ITEM_KEY: u8 = 132;
pub const BTRFS_ROOT_BACKREF_KEY: u8 = 144;
pub const BTRFS_ROOT_REF_KEY: u8 = 156;
pub const BTRFS_EXTENT_ITEM_KEY: u8 = 168;
pub const BTRFS_METADATA_ITEM_KEY: u8 = 169;
pub const BTRFS_TREE_BLOCK_REF_KEY: u8 = 176;
pub const BTRFS_EXTENT_DATA_REF_KEY: u8 = 178;
pub const BTRFS_SHARED_BLOCK_REF_KEY: u8 = 180;
pub const BTRFS_SHARED_DATA_REF_KEY: u8 = 182;
pub const BTRFS_BLOCK_GROUP_ITEM_KEY: u8 = 192;
pub const BTRFS_DEV_EXTENT_KEY: u8 = 204;
pub const BTRFS_DEV_ITEM_KEY: u8 = 216;
pub const BTRFS_CHUNK_ITEM_KEY: u8 = 228;

// Chunk Types
pub const BTRFS_BLOCK_GROUP_DATA: u64 = 1 << 0;
pub const BTRFS_BLOCK_GROUP_SYSTEM: u64 = 1 << 1;
pub const BTRFS_BLOCK_GROUP_METADATA: u64 = 1 << 2;
pub const BTRFS_BLOCK_GROUP_RAID0: u64 = 1 << 3;
pub const BTRFS_BLOCK_GROUP_RAID1: u64 = 1 << 4;
pub const BTRFS_BLOCK_GROUP_DUP: u64 = 1 << 5;
pub const BTRFS_BLOCK_GROUP_RAID10: u64 = 1 << 6;
pub const BTRFS_BLOCK_GROUP_RAID5: u64 = 1 << 7;
pub const BTRFS_BLOCK_GROUP_RAID6: u64 = 1 << 8;

// File Types (Directory Entry)
pub const BTRFS_FT_UNKNOWN: u8 = 0;
pub const BTRFS_FT_REG_FILE: u8 = 1;
pub const BTRFS_FT_DIR: u8 = 2;
pub const BTRFS_FT_CHRDEV: u8 = 3;
pub const BTRFS_FT_BLKDEV: u8 = 4;
pub const BTRFS_FT_FIFO: u8 = 5;
pub const BTRFS_FT_SOCK: u8 = 6;
pub const BTRFS_FT_SYMLINK: u8 = 7;
pub const BTRFS_FT_XATTR: u8 = 8;

pub const BTRFS_FILE_EXTENT_INLINE: u8 = 0;
pub const BTRFS_FILE_EXTENT_REG: u8 = 1;
pub const BTRFS_FILE_EXTENT_PREALLOC: u8 = 2;

// -----------------------------------------------------------------------------
// On-Disk Structures
// -----------------------------------------------------------------------------

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct BtrfsKey {
    pub objectid: u64,
    pub type_: u8,
    pub offset: u64,
}

impl BtrfsKey {
    pub fn new(objectid: u64, type_: u8, offset: u64) -> Self {
        Self {
            objectid,
            type_,
            offset,
        }
    }

    pub fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        let s_obj = self.objectid;
        let o_obj = other.objectid;
        if s_obj != o_obj {
            return s_obj.cmp(&o_obj);
        }
        let s_type = self.type_;
        let o_type = other.type_;
        if s_type != o_type {
            return s_type.cmp(&o_type);
        }
        let s_off = self.offset;
        let o_off = other.offset;
        s_off.cmp(&o_off)
    }
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct BtrfsHeader {
    pub csum: BtrfsChecksum,
    pub fsid: BtrfsUuid,
    pub bytenr: u64,
    pub flags: u64,
    pub chunk_tree_uuid: BtrfsUuid,
    pub generation: u64,
    pub owner: u64,
    pub nritems: u32,
    pub level: u8,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct BtrfsKeyPtr {
    pub key: BtrfsKey,
    pub blockptr: u64,
    pub generation: u64,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct BtrfsItem {
    pub key: BtrfsKey,
    pub offset: u32,
    pub size: u32,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct BtrfsDevItem {
    pub devid: u64,
    pub total_bytes: u64,
    pub bytes_used: u64,
    pub io_align: u32,
    pub io_width: u32,
    pub sector_size: u32,
    pub type_: u64,
    pub generation: u64,
    pub start_offset: u64,
    pub dev_group: u32,
    pub seek_speed: u8,
    pub bandwidth: u8,
    pub uuid: BtrfsUuid,
    pub fsid: BtrfsUuid,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct BtrfsSuperBlock {
    pub csum: BtrfsChecksum,
    pub fsid: BtrfsUuid,
    pub bytenr: u64,
    pub flags: u64,
    pub magic: [u8; 8],
    pub generation: u64,
    pub root: u64,
    pub chunk_root: u64,
    pub log_root: u64,
    pub log_root_transid: u64,
    pub total_bytes: u64,
    pub bytes_used: u64,
    pub root_dir_objectid: u64,
    pub num_devices: u64,
    pub sectorsize: u32,
    pub nodesize: u32,
    pub leafsize: u32,
    pub stripesize: u32,
    pub sys_chunk_array_size: u32,
    pub chunk_root_generation: u64,
    pub compat_flags: u64,
    pub compat_ro_flags: u64,
    pub incompat_flags: u64,
    pub csum_type: u16,
    pub root_level: u8,
    pub chunk_root_level: u8,
    pub log_root_level: u8,
    pub dev_item: BtrfsDevItem,
    pub label: [u8; 256],
    pub cache_generation: u64,
    pub uuid_tree_generation: u64,
    pub reserved: [u8; 30 * 8],
    pub sys_chunk_array: [u8; 2048],
    pub super_roots: [BtrfsKeyPtr; 4],
    pub unused: [u8; 565],
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct BtrfsStripe {
    pub devid: u64,
    pub offset: u64,
    pub dev_uuid: BtrfsUuid,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct BtrfsChunk {
    pub length: u64,
    pub owner: u64,
    pub stripe_len: u64,
    pub type_: u64,
    pub io_align: u32,
    pub io_width: u32,
    pub sector_size: u32,
    pub num_stripes: u16,
    pub sub_stripes: u16,
    // followed by num_stripes BtrfsStripe
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct BtrfsDirItem {
    pub location: BtrfsKey,
    pub transid: u64,
    pub data_len: u16,
    pub name_len: u16,
    pub type_: u8,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct BtrfsRootItem {
    pub invoice: [u8; 160], // Simplification for BtrfsInodeItem
    pub generation: u64,
    pub root_dirid: u64,
    pub bytenr: u64,
    pub byte_limit: u64,
    pub bytes_used: u64,
    pub last_snapshot: u64,
    pub flags: u64,
    pub refs: u32,
    pub drop_on_cache: BtrfsKey,
    pub drop_progress: u8,
    pub level: u8,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct BtrfsFileExtentItem {
    pub generation: u64,
    pub ram_bytes: u64,
    pub compression: u8,
    pub encryption: u8,
    pub other_encoding: u16,
    pub type_: u8,
}

// -----------------------------------------------------------------------------
// Logic
// -----------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ChunkMap {
    pub logical: u64,
    pub length: u64,
    pub physical: u64, // Simplified: assuming 1 device mapping for now
}

pub struct Btrfs<'a> {
    block_io: &'a BlockIO,
    pub sb: BtrfsSuperBlock,
    pub chunks: Vec<ChunkMap>,
}

impl<'a> Btrfs<'a> {
    pub fn new(block_io: &'a BlockIO) -> Result<Option<Self>> {
        // Allocate aligned buffer (4096 bytes)
        // Some UEFI implementations (and QEMU virtio) require IO buffers to be aligned.
        let mut raw_buffer = vec![0u8; 4096 + 4096];
        let align_offset = raw_buffer.as_ptr().align_offset(4096);
        let buffer = &mut raw_buffer[align_offset..align_offset + 4096];

        // Read Superblock at 64KB
        block_io.read_blocks(
            block_io.media().media_id(),
            BTRFS_SUPER_INFO_OFFSET / block_io.media().block_size() as u64,
            buffer,
        )?;

        let sb = unsafe {
            let ptr = buffer.as_ptr() as *const BtrfsSuperBlock;
            ptr.read_unaligned()
        };

        if &sb.magic != BTRFS_SIGNATURE {
            if sb.magic != [0; 8] {
                crate::warn!(
                    "Btrfs: Invalid magic bytes at 64KB offset: {:02x?}. Expected: {:02x?}",
                    &sb.magic,
                    BTRFS_SIGNATURE
                );
            }
            return Ok(None);
        }

        let mut btrfs = Btrfs {
            block_io,
            sb,
            chunks: Vec::new(),
        };

        btrfs.load_sys_chunks();
        crate::debug!(
            "Btrfs: Loaded {} chunks from sys_chunk_array",
            btrfs.chunks.len()
        );
        for (i, c) in btrfs.chunks.iter().enumerate() {
            crate::debug!(
                "  Chunk {}: logical={:#x}, len={:#x}, phys={:#x}",
                i,
                c.logical,
                c.length,
                c.physical
            );
        }

        // Scan the Chunk Tree to load data/metadata chunks
        if let Err(e) = btrfs.load_chunk_tree() {
            crate::warn!("Btrfs: Failed to load Chunk Tree: {:?}", e);
        }

        Ok(Some(btrfs))
    }

    pub fn get_label(&self) -> String {
        let label_slice = &self.sb.label;
        let end = label_slice
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(label_slice.len());
        String::from_utf8_lossy(&label_slice[..end]).into_owned()
    }

    pub fn get_uuid(&self) -> String {
        let u = self.sb.fsid;
        format!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            u[0], u[1], u[2], u[3], u[4], u[5], u[6], u[7], u[8], u[9], u[10], u[11], u[12], u[13], u[14], u[15]
        )
    }

    fn load_chunk_tree(&mut self) -> Result<()> {
        let chunk_root_logical = self.sb.chunk_root;
        let node_size = self.sb.nodesize as usize;
        let mut node_buf = vec![0u8; node_size];

        crate::debug!(
            "Btrfs: Reading Chunk Tree Root at {:#x}",
            chunk_root_logical
        );

        // Read the chunk tree root
        // Note: usage of read_logical here relies on sys_chunks being loaded
        if let Err(e) = self.read_logical(chunk_root_logical, node_size, &mut node_buf) {
            crate::debug!("Btrfs: Failed to read Chunk Tree root: {:?}", e);
            // This is critical if sys_chunks doesn't cover everything
            return Err(e);
        }

        let header = unsafe { (node_buf.as_ptr() as *const BtrfsHeader).read_unaligned() };

        if header.level != 0 {
            crate::debug!(
                "Btrfs: Chunk Tree is too deep (level {}), simplify impl supports leaf only.",
                header.level
            );
            return Ok(());
        }

        let items_start = mem::size_of::<BtrfsHeader>();
        let items_ptr = unsafe { node_buf.as_ptr().add(items_start) };

        for i in 0..header.nritems {
            let item_ptr = unsafe { (items_ptr as *const BtrfsItem).add(i as usize) };
            let item = unsafe { item_ptr.read_unaligned() };

            if item.key.type_ == BTRFS_CHUNK_ITEM_KEY {
                let data_ptr = node_buf.as_ptr() as usize
                    + mem::size_of::<BtrfsHeader>()
                    + item.offset as usize;
                let chunk = unsafe { (data_ptr as *const BtrfsChunk).read_unaligned() };

                // Read first stripe (Simplified: RAID0/Linear/Single support only)
                let stripe_ptr =
                    unsafe { (data_ptr as *const u8).add(mem::size_of::<BtrfsChunk>()) };
                let stripe = unsafe { (stripe_ptr as *const BtrfsStripe).read_unaligned() };

                let k_off = item.key.offset;
                let c_len = chunk.length;
                let s_off = stripe.offset;
                crate::debug!(
                    "Btrfs: Found Chunk logical={:#x} len={:#x} physical={:#x}",
                    k_off,
                    c_len,
                    s_off
                );

                self.chunks.push(ChunkMap {
                    logical: item.key.offset,
                    length: chunk.length,
                    physical: stripe.offset,
                });
            }
        }

        Ok(())
    }

    fn load_sys_chunks(&mut self) {
        let ptr = self.sb.sys_chunk_array.as_ptr();
        let limit = self.sb.sys_chunk_array_size as usize;
        let mut offset = 0;

        while offset < limit {
            unsafe {
                let key_ptr = ptr.add(offset) as *const BtrfsKey;
                let key = key_ptr.read_unaligned();
                offset += mem::size_of::<BtrfsKey>();

                if key.type_ != BTRFS_CHUNK_ITEM_KEY {
                    // Should not happen in sys_chunk_array
                    break;
                }

                let chunk_ptr = ptr.add(offset) as *const BtrfsChunk;
                let chunk = chunk_ptr.read_unaligned();
                offset += mem::size_of::<BtrfsChunk>();

                // Parse Stripes
                let stripe_ptr = ptr.add(offset) as *const BtrfsStripe;
                // Just take the first stripe for now (Simplified)
                let stripe = stripe_ptr.read_unaligned();

                self.chunks.push(ChunkMap {
                    logical: key.offset,
                    length: chunk.length,
                    physical: stripe.offset,
                });

                offset += (chunk.num_stripes as usize) * mem::size_of::<BtrfsStripe>();
            }
        }
    }

    pub fn logical_to_physical(&self, logical: u64) -> Option<u64> {
        for chunk in &self.chunks {
            if logical >= chunk.logical && logical < chunk.logical + chunk.length {
                let offset = logical - chunk.logical;
                return Some(chunk.physical + offset);
            }
        }
        crate::debug!("Btrfs: logical_to_physical failed for {:#x}", logical);
        None
    }

    pub fn read_logical(&mut self, logical: u64, length: usize, buffer: &mut [u8]) -> Result<()> {
        if let Some(physical) = self.logical_to_physical(logical) {
            let block_size = self.block_io.media().block_size() as u64;
            let lba = physical / block_size;
            let offset_in_block = (physical % block_size) as usize;

            // Allocate a temp buffer to handle alignment if needed, or non-block-aligned reads
            // For simplicity, reading aligned blocks is best.
            // Assuming 4096 node size usually

            // Quick hack: Read aligned blocks around the target
            let start_block = lba;
            let end_block = (physical + length as u64 + block_size - 1) / block_size;
            let blocks_to_read = (end_block - start_block) as usize;

            let mut temp_buf = vec![0u8; blocks_to_read * block_size as usize];
            self.block_io.read_blocks(
                self.block_io.media().media_id(),
                start_block,
                &mut temp_buf,
            )?;

            // Copy out relevant part
            let slice = &temp_buf[offset_in_block..offset_in_block + length];
            buffer.copy_from_slice(slice);

            Ok(())
        } else {
            // Chunk not found in cache. In full impl, read Chunk Tree here.
            // For now, fail silent or panic in logs
            crate::debug!(
                "Btrfs: read_logical failed - no chunk map for {:#x}",
                logical
            );
            Err(Error::new(Status::DEVICE_ERROR, ()))
        }
    }

    // A simplified tree search that assumes we are looking for a leaf
    // Returns the leaf content and the offset to the item
    pub fn search_slot(
        &mut self,
        root_logical: u64,
        key: &BtrfsKey,
    ) -> Result<Option<(Vec<u8>, BtrfsItem)>> {
        let node_size = self.sb.nodesize as usize;
        let mut node_buf = vec![0u8; node_size];

        let mut cur_logical = root_logical;

        loop {
            crate::debug!("Btrfs: search_slot reading node at {:#x}", cur_logical);
            self.read_logical(cur_logical, node_size, &mut node_buf)?;

            let header = unsafe { (node_buf.as_ptr() as *const BtrfsHeader).read_unaligned() };
            let h_lvl = header.level;
            let h_nr = header.nritems;
            crate::debug!("Btrfs: Node level={}, nritems={}", h_lvl, h_nr);

            if header.level == 0 {
                // We are at a leaf
                let items_start = mem::size_of::<BtrfsHeader>();
                let items_ptr = unsafe { node_buf.as_ptr().add(items_start) };

                for i in 0..header.nritems {
                    let item_ptr = unsafe { (items_ptr as *const BtrfsItem).add(i as usize) };
                    let item = unsafe { item_ptr.read_unaligned() };

                    // Exact match check
                    if item.key.objectid == key.objectid && item.key.type_ == key.type_ {
                        // For directory listings (indexes), we might want range matches, but this is search_slot for specific key
                        if item.key.offset == key.offset {
                            return Ok(Some((node_buf, item)));
                        }
                    }

                    // Since items are sorted, if we passed it, it's not here
                    let item_key = item.key;
                    if item_key.cmp(key) == core::cmp::Ordering::Greater {
                        break;
                    }
                }
                crate::debug!("Btrfs: Key not found in leaf");
                return Ok(None);
            } else {
                // Internal node
                let ptrs_start = mem::size_of::<BtrfsHeader>();
                let ptrs_ptr = unsafe { node_buf.as_ptr().add(ptrs_start) };

                let mut next_logical = 0;
                let mut found = false;

                for i in 0..header.nritems {
                    let ptr_ptr = unsafe { (ptrs_ptr as *const BtrfsKeyPtr).add(i as usize) };
                    let kp = unsafe { ptr_ptr.read_unaligned() };

                    if kp.key.cmp(key) == core::cmp::Ordering::Greater {
                        // The key is in the previous child
                        if i > 0 {
                            let prev_ptr =
                                unsafe { (ptrs_ptr as *const BtrfsKeyPtr).add(i as usize - 1) };
                            next_logical = unsafe { prev_ptr.read_unaligned().blockptr };
                            found = true;
                        } else {
                            // Key is smaller than smallest in node, go left
                            next_logical = kp.blockptr;
                            found = true;
                        }
                        break;
                    }
                }

                if !found {
                    // It must be in the last child
                    let last_ptr = unsafe {
                        (ptrs_ptr as *const BtrfsKeyPtr).add(header.nritems as usize - 1)
                    };
                    next_logical = unsafe { last_ptr.read_unaligned().blockptr };
                }

                cur_logical = next_logical;
            }
        }
    }

    pub fn get_tree_root(&mut self, tree_id: u64) -> Result<u64> {
        let key = BtrfsKey::new(tree_id, BTRFS_ROOT_ITEM_KEY, 0);
        if let Some((leaf, item)) = self.search_slot(self.sb.root, &key)? {
            let data_ptr =
                leaf.as_ptr() as usize + mem::size_of::<BtrfsHeader>() + item.offset as usize;
            let root_item = unsafe { (data_ptr as *const BtrfsRootItem).read_unaligned() };
            Ok(root_item.bytenr)
        } else {
            Err(Error::new(Status::NOT_FOUND, ()))
        }
    }

    // Get the FS Tree Root
    pub fn get_fs_root(&mut self) -> Result<u64> {
        self.get_tree_root(BTRFS_FS_TREE_OBJECTID)
    }

    pub fn read_file(&mut self, fs_root_logical: u64, inode: u64) -> Result<Vec<u8>> {
        let key = BtrfsKey::new(inode, BTRFS_EXTENT_DATA_KEY, 0);
        if let Some((leaf, item)) = self.search_slot(fs_root_logical, &key)? {
            let data_ptr =
                leaf.as_ptr() as usize + mem::size_of::<BtrfsHeader>() + item.offset as usize;
            let extent = unsafe { (data_ptr as *const BtrfsFileExtentItem).read_unaligned() };

            if extent.compression != 0 {
                crate::error!(
                    "Btrfs: Compressed file detected (algo {}). Only uncompressed files supported.",
                    extent.compression
                );
                return Err(Error::new(Status::UNSUPPORTED, ()));
            }

            if extent.type_ == BTRFS_FILE_EXTENT_INLINE {
                let inline_data_ptr = data_ptr + mem::size_of::<BtrfsFileExtentItem>();
                let inline_len = item.size as usize - mem::size_of::<BtrfsFileExtentItem>();
                let slice =
                    unsafe { slice::from_raw_parts(inline_data_ptr as *const u8, inline_len) };
                return Ok(slice.to_vec());
            } else if extent.type_ == BTRFS_FILE_EXTENT_REG {
                // Regular extent: header + disk_bytenr(8) + disk_num_bytes(8) + offset(8) + num_bytes(8)
                let reg_ptr = data_ptr + mem::size_of::<BtrfsFileExtentItem>();
                let disk_bytenr_ptr = reg_ptr as *const u64;
                let disk_bytenr = unsafe { disk_bytenr_ptr.read_unaligned() };

                // offset 24 bytes from disk_bytenr_ptr for num_bytes
                let num_bytes_ptr = unsafe { disk_bytenr_ptr.add(3) };
                let num_bytes = unsafe { num_bytes_ptr.read_unaligned() };

                if disk_bytenr == 0 {
                    return Ok(Vec::new());
                }

                // Read full extent (assuming contiguous for this simplified driver)
                let read_size = num_bytes as usize;
                let mut buf = vec![0u8; read_size];

                self.read_logical(disk_bytenr, read_size, &mut buf)?;
                return Ok(buf);
            }
        }
        Err(Error::new(Status::NOT_FOUND, ()))
    }

    pub fn find_file_in_dir(
        &mut self,
        fs_root_logical: u64,
        dir_objectid: u64,
        name_to_find: &str,
    ) -> Result<Option<(u64, u8)>> {
        let node_size = self.sb.nodesize as usize;
        let mut node_buf = vec![0u8; node_size];

        // Start at FS root
        self.read_logical(fs_root_logical, node_size, &mut node_buf)?;
        let mut header = unsafe { (node_buf.as_ptr() as *const BtrfsHeader).read_unaligned() };

        // Drill down to the left-most leaf for now (Simplified).
        let mut leaf_buf = node_buf;

        while header.level > 0 {
            let ptrs_start = mem::size_of::<BtrfsHeader>();
            let ptr_ptr = unsafe { leaf_buf.as_ptr().add(ptrs_start) };
            let kp = unsafe { (ptr_ptr as *const BtrfsKeyPtr).read_unaligned() };

            self.read_logical(kp.blockptr, node_size, &mut leaf_buf)?;
            header = unsafe { (leaf_buf.as_ptr() as *const BtrfsHeader).read_unaligned() };
        }

        // Iterate items in leaf
        let items_start = mem::size_of::<BtrfsHeader>();
        for i in 0..header.nritems {
            let item_offset = items_start + i as usize * mem::size_of::<BtrfsItem>();
            let item = unsafe {
                (leaf_buf.as_ptr().add(item_offset) as *const BtrfsItem).read_unaligned()
            };

            if item.key.objectid == dir_objectid
                && (item.key.type_ == BTRFS_DIR_INDEX_KEY || item.key.type_ == BTRFS_DIR_ITEM_KEY)
            {
                let data_ptr = leaf_buf.as_ptr() as usize
                    + mem::size_of::<BtrfsHeader>()
                    + item.offset as usize;
                let dir_item = unsafe { (data_ptr as *const BtrfsDirItem).read_unaligned() };

                let name_len = dir_item.name_len as usize;
                let name_ptr = data_ptr + mem::size_of::<BtrfsDirItem>();

                if name_ptr + name_len <= leaf_buf.as_ptr() as usize + node_size {
                    let name_slice =
                        unsafe { slice::from_raw_parts(name_ptr as *const u8, name_len) };

                    if name_slice == name_to_find.as_bytes() {
                        return Ok(Some((dir_item.location.objectid, dir_item.location.type_)));
                    }
                }
            }
        }

        Ok(None)
    }

    pub fn list_root_files(&mut self) -> Result<Vec<String>> {
        let fs_root_logical = match self.get_fs_root() {
            Ok(loc) => loc,
            Err(e) => {
                // Fallback for debugging: assume simple setup where FS tree might be pointed differently or rely on defaults
                // If we can't find the FS Root, we can't list files.
                return Ok(vec![format!("ERR: FS Root Not Found: {:?}", e)]);
            }
        };

        // We want to list the root directory. Root Dir Object ID is usually 256.
        // Scan for Key(256, DIR_INDEX, ...).
        let dir_objectid = 256;

        // Manual iteration of the leaf (simplified - assumes root dir fits in one leaf)
        let node_size = self.sb.nodesize as usize;
        let mut node_buf = vec![0u8; node_size];

        // We start searching for the first DIR_INDEX for object 256
        // Key(256, DIR_INDEX, 0)
        let _search_key = BtrfsKey::new(dir_objectid, BTRFS_DIR_INDEX_KEY, 0);

        // Find the leaf containing this key (or the one after it)
        // Since search_slot does exact or prev, let's implement a "lower_bound" style read
        // Re-using logic: read root node of FS tree

        let mut files = Vec::new();

        // Traverse to leaf for the root dir
        // This is a simplified "read the leaf at logical address" assuming we know where it is
        // We actually need to traverse the FS Tree.
        // Using a hack: Assume root node is a leaf for small filesystems

        self.read_logical(fs_root_logical, node_size, &mut node_buf)?;
        let header = unsafe { (node_buf.as_ptr() as *const BtrfsHeader).read_unaligned() };

        // If the root is not a leaf, we need to drill down.
        // For this demo, let's just inspect the root node. If it's a leaf, great.
        // If it's a node, we just look at the first child for now (DFS-ish).

        let mut leaf_buf = node_buf;
        let mut cur_header = header;

        while cur_header.level > 0 {
            let ptrs_start = mem::size_of::<BtrfsHeader>();
            let ptr_ptr = unsafe { leaf_buf.as_ptr().add(ptrs_start) }; // First child
            let kp = unsafe { (ptr_ptr as *const BtrfsKeyPtr).read_unaligned() };

            self.read_logical(kp.blockptr, node_size, &mut leaf_buf)?;
            cur_header = unsafe { (leaf_buf.as_ptr() as *const BtrfsHeader).read_unaligned() };
        }

        // Now we have a leaf. Iterate items.
        let items_start = mem::size_of::<BtrfsHeader>();
        for i in 0..cur_header.nritems {
            let item_offset = items_start + i as usize * mem::size_of::<BtrfsItem>();
            let item = unsafe {
                (leaf_buf.as_ptr().add(item_offset) as *const BtrfsItem).read_unaligned()
            };

            if item.key.type_ == BTRFS_DIR_INDEX_KEY {
                let data_ptr = leaf_buf.as_ptr() as usize
                    + mem::size_of::<BtrfsHeader>()
                    + item.offset as usize;
                let dir_item = unsafe { (data_ptr as *const BtrfsDirItem).read_unaligned() };

                let name_len = dir_item.name_len as usize;
                let name_ptr = data_ptr + mem::size_of::<BtrfsDirItem>();

                // Safety check
                if name_ptr + name_len <= leaf_buf.as_ptr() as usize + node_size {
                    let name_slice =
                        unsafe { slice::from_raw_parts(name_ptr as *const u8, name_len) };
                    let name = String::from_utf8_lossy(name_slice).into_owned();

                    let type_str = match dir_item.type_ {
                        BTRFS_FT_DIR => "/",
                        _ => "",
                    };
                    files.push(format!("{}{}", name, type_str));
                }
            }
        }

        if files.is_empty() {
            files.push(String::from("<Empty or Nav Failed>"));
        }

        Ok(files)
    }
}
