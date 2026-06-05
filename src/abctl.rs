#![allow(dead_code)]
//! A/B slot selection for Rignite, using the shared `runix-bootctl` logic and
//! binary boot-control block - the exact same code + format RUM uses to stage
//! updates. This keeps the bootloader and the updater in lockstep: no divergent
//! reimplementation of the priority / trial-tries / rollback rules.
//!
//! The boot-control block is a fixed 64-byte structure RUM writes (currently
//! `bootctl.bin`). Rignite reads it, picks the slot, accounts for the trial
//! boot, and writes the updated block back before launching the kernel. If a
//! slot's trial tries run out unconfirmed, `select()` returns the previous good
//! slot - automatic rollback.
//!
//! Wiring into the actual partition/subvolume load is pending the A/B on-disk
//! layout decision (two partitions vs two btrfs subvolumes, and where the block
//! lives). `select_and_begin` is the layout-independent core.

use runix_bootctl::{BootControl, Slot, BLOCK_SIZE};

/// Outcome of consulting the boot-control block.
pub struct Decision {
    /// Which slot to boot.
    pub slot: Slot,
    /// The block to persist before booting (the trial try has been accounted).
    pub updated_block: [u8; BLOCK_SIZE],
}

/// Read a boot-control block, select the slot to boot, and account for the
/// trial boot. Returns `None` if the block is invalid or nothing is bootable
/// (the caller should then fall back to its normal single-partition path).
pub fn select_and_begin(block: &[u8]) -> Option<Decision> {
    let mut bc = BootControl::from_bytes(block)?;
    let slot = bc.select()?;
    bc.begin_boot(slot);
    Some(Decision { slot, updated_block: bc.to_bytes() })
}

/// Human-readable slot name for the selected slot ("A"/"B"), e.g. to pick the
/// per-slot partition/subvolume path once the layout is decided.
pub fn slot_dir(slot: Slot) -> &'static str {
    slot.as_str()
}
