// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The architecture specific part of an address space: its page tables.
//!
//! The flags and options are copied from `zircon/kernel/vm/arch_vm_aspace.rs`, and the interface
//! is ported from `zircon/kernel/vm/include/vm/arch_vm_aspace.h`.
//!
//! Deviations:
//! - `ArchVmAspace` is a trait, since the kernel has an implementation per kind of address space
//!   rather than per architecture: host process mappings (`arch::host::mmu`), and later guest
//!   physical address spaces.
//! - No shared, restricted or unified address spaces, and no page table reclamation.

use crate::kernel::types::PAddr;
use zx_status::Status;

// TODO(https://fxbug.dev/529507187): Use bitflags! or equivalent once available.
pub type ArchMmuFlags = u8;

#[allow(clippy::identity_op)]
pub const ARCH_MMU_FLAG_CACHED: ArchMmuFlags = 0 << 0;
pub const ARCH_MMU_FLAG_UNCACHED: ArchMmuFlags = 1 << 0;
#[allow(clippy::identity_op)]
pub const ARCH_MMU_FLAG_UNCACHED_DEVICE: ArchMmuFlags = 2 << 0;
#[allow(clippy::identity_op)]
pub const ARCH_MMU_FLAG_WRITE_COMBINING: ArchMmuFlags = 3 << 0;
#[allow(clippy::identity_op)]
pub const ARCH_MMU_FLAG_CACHE_MASK: ArchMmuFlags = 3 << 0;
pub const ARCH_MMU_FLAG_PERM_USER: ArchMmuFlags = 1 << 2;
pub const ARCH_MMU_FLAG_PERM_READ: ArchMmuFlags = 1 << 3;
pub const ARCH_MMU_FLAG_PERM_WRITE: ArchMmuFlags = 1 << 4;
pub const ARCH_MMU_FLAG_PERM_EXECUTE: ArchMmuFlags = 1 << 5;
pub const ARCH_MMU_FLAG_PERM_RWX_MASK: ArchMmuFlags =
    ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE | ARCH_MMU_FLAG_PERM_EXECUTE;
pub const ARCH_MMU_FLAG_NS: ArchMmuFlags = 1 << 6;
pub const ARCH_MMU_FLAG_INVALID: ArchMmuFlags = 1 << 7;

// TODO(https://fxbug.dev/529507187): Use bitflags! or equivalent once available.
pub type ArchAspaceFlags = u8;

pub const ARCH_ASPACE_FLAG_KERNEL: ArchAspaceFlags = 1 << 0;
pub const ARCH_ASPACE_FLAG_GUEST: ArchAspaceFlags = 1 << 1;

// TODO(https://fxbug.dev/529507187): Use bitflags! or equivalent once available.
// Options for unmapping the given virtual address range.
pub type ArchUnmapOptions = u8;

pub const ARCH_UNMAP_OPTION_NONE: ArchUnmapOptions = 0;
// Controls whether the unmap region can be extended to be larger, or if only the exact region may
// be unmapped. The unmap region might be extended, even if only temporarily, if large pages need to
// be split.
pub const ARCH_UNMAP_OPTION_ENLARGE: ArchUnmapOptions = 1 << 0;
// Requests that the accessed bit be harvested, and the page queues updated.
pub const ARCH_UNMAP_OPTION_HARVEST: ArchUnmapOptions = 1 << 1;

/// Returns true if the MMU flags specify an uncached memory type.
#[inline]
pub const fn arch_mmu_flags_uncached(mmu_flags: ArchMmuFlags) -> bool {
    (mmu_flags & (ARCH_MMU_FLAG_UNCACHED | ARCH_MMU_FLAG_UNCACHED_DEVICE)) != 0
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistingEntryAction {
    Skip = 0,
    Error = 1,
    Upgrade = 2,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonTerminalAction {
    /// If a non-terminal entry has no accessed information, unmap and free it. If it has accessed
    /// information, just remove the flag.
    FreeUnaccessed = 0,
    /// Retain both the non-terminal mappings and any accessed information.
    Retain = 1,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalAction {
    /// If the page is accessed update its age in the page queues, and remove the accessed flag.
    UpdateAgeAndHarvest = 0,
    /// If the page is accessed update its age in the page queues, but do not clear the flag.
    UpdateAge = 1,
}

/// The page tables of an address space.
pub trait ArchVmAspace: Send + Sync {
    /// Puts the aspace into a mode where changes to the mappings are not allowed. Used once all
    /// the mappings of a destroyed aspace have been removed.
    fn disable_updates(&self);

    /// Destroys the aspace. All mappings must have been removed.
    fn destroy(&self) -> Result<(), Status>;

    /// Maps the pages `phys` at `vaddr` with `mmu_flags`. `existing_action` controls what happens
    /// to any existing mapping of a page in the range.
    fn map(
        &self,
        vaddr: usize,
        phys: &[PAddr],
        mmu_flags: ArchMmuFlags,
        existing_action: ExistingEntryAction,
    ) -> Result<(), Status>;

    /// Unmaps `count` pages at `vaddr`.
    fn unmap(&self, vaddr: usize, count: usize, options: ArchUnmapOptions) -> Result<(), Status>;

    /// Changes the permissions of `count` pages at `vaddr` to `mmu_flags`.
    fn protect(
        &self,
        vaddr: usize,
        count: usize,
        mmu_flags: ArchMmuFlags,
        options: ArchUnmapOptions,
    ) -> Result<(), Status>;

    /// Returns the physical address and flags a page is mapped with.
    fn query(&self, vaddr: usize) -> Result<(PAddr, ArchMmuFlags), Status>;

    /// Picks a spot of `size` bytes in [base, end] aligned to `align`.
    fn pick_spot(
        &self,
        base: usize,
        _end: usize,
        align: usize,
        _size: usize,
        _mmu_flags: ArchMmuFlags,
    ) -> usize {
        base.next_multiple_of(align)
    }

    /// Harvests the accessed flags of the pages in the range into the page queues.
    fn harvest_accessed(
        &self,
        vaddr: usize,
        count: usize,
        non_terminal_action: NonTerminalAction,
        terminal_action: TerminalAction,
    ) -> Result<(), Status>;

    /// Marks the pages in the range as accessed, as for a fault on a page whose accessed flag was
    /// harvested.
    fn mark_accessed(&self, vaddr: usize, count: usize) -> Result<(), Status>;

    /// Returns whether or not this aspace might have additional accessed information since the
    /// last time this method was called with clear=true. If this returns `false` then, modulo
    /// races, `harvest_accessed` is defined to not find any set bits and not call
    /// `PageQueues::mark_accessed`.
    ///
    /// This is intended for use by the harvester to avoid scanning for any accessed or dirty bits
    /// if the aspace has not been accessed at all.
    ///
    /// The `clear` flag controls whether the aspace having been accessed should be cleared or not.
    fn accessed_since_last_check(&self, clear: bool) -> bool;
}
