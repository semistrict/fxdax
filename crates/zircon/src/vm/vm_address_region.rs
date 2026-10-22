// Copyright 2016 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Virtual memory address regions (VMARs): the tree of regions and mappings of an address space.
//!
//! Ported from `zircon/kernel/vm/vm_address_region.cc`,
//! `zircon/kernel/vm/vm_address_region_or_mapping.cc` and
//! `zircon/kernel/vm/include/vm/vm_address_region.h`.
//!
//! Deviations:
//! - `VmAddressRegionOrMapping` is an enum of the two kinds of region.
//! - A region's mutable state, which Zircon guards with the aspace lock, is that lock's, kept with
//!   the region (`sync::LockedBy`). The children are a `BTreeMap` keyed by base.
//! - No ASLR: space is allocated at the lowest address that fits, as Zircon does with ASLR
//!   disabled.
//! - No memory priorities, attribution, enumerators, reservations or mapping merging. Operations
//!   over a range of mappings find the mappings first, rather than enumerating them as they go.

use crate::ensure;
use crate::kernel::range_check::in_range;
use crate::sync::LockedBy;
use crate::vm::arch_vm_aspace::*;
use crate::vm::vm_aspace::{AspaceGuard, AspaceLocked, VmAspace};
use crate::vm::vm_mapping::VmMapping;
use crate::vm::vm_object_paged::{EvictionHint, VmObjectPaged};
use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::{Arc, Weak};
use zx_status::Status;
use zx_types::ZX_MAX_NAME_LEN;

const PAGE_SIZE: usize = page::SIZE;
const PAGE_SHIFT: u8 = page::SHIFT as u8;

// flags for VMARs, mappings, and aspaces
pub const VMAR_FLAG_COMPACT: u32 = 1 << 0;
pub const VMAR_FLAG_SPECIFIC: u32 = 1 << 1;
pub const VMAR_FLAG_SPECIFIC_OVERWRITE: u32 = 1 << 2;
pub const VMAR_FLAG_CAN_MAP_SPECIFIC: u32 = 1 << 3;
pub const VMAR_FLAG_CAN_MAP_READ: u32 = 1 << 4;
pub const VMAR_FLAG_CAN_MAP_WRITE: u32 = 1 << 5;
pub const VMAR_FLAG_CAN_MAP_EXECUTE: u32 = 1 << 6;
pub const VMAR_FLAG_REQUIRE_NON_RESIZABLE: u32 = 1 << 7;
pub const VMAR_FLAG_ALLOW_FAULTS: u32 = 1 << 8;
pub const VMAR_FLAG_OFFSET_IS_UPPER_LIMIT: u32 = 1 << 9;
pub const VMAR_FLAG_DEBUG_DYNAMIC_KERNEL_MAPPING: u32 = 1 << 10;
pub const VMAR_FLAG_FAULT_BEYOND_STREAM_SIZE: u32 = 1 << 11;

pub const VMAR_CAN_RWX_FLAGS: u32 =
    VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE | VMAR_FLAG_CAN_MAP_EXECUTE;

/// Operations on the mappings of a range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeOpType {
    Commit,
    Decommit,
    MapRange,
    Zero,
    DontNeed,
    AlwaysNeed,
    Prefetch,
}

/// Whether an operation applies to child regions as well.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VmAddressRegionOpChildren {
    Yes,
    No,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LifeCycleState {
    /// Initial state: if we haven't been added to the parent yet.
    NotReady,
    /// Usual state: we're in the parent and alive.
    Alive,
    /// We've been removed from the parent and destroyed.
    Dead,
}

/// A VMAR or a mapping.
#[derive(Clone)]
pub enum VmAddressRegionOrMapping {
    Vmar(Arc<VmAddressRegion>),
    Mapping(Arc<VmMapping>),
}

impl VmAddressRegionOrMapping {
    pub fn base(&self) -> usize {
        match self {
            Self::Vmar(vmar) => vmar.base(),
            Self::Mapping(mapping) => mapping.base(),
        }
    }

    pub fn size(&self) -> usize {
        match self {
            Self::Vmar(vmar) => vmar.size(),
            Self::Mapping(mapping) => mapping.size(),
        }
    }

    pub fn flags(&self) -> u32 {
        match self {
            Self::Vmar(vmar) => vmar.flags(),
            Self::Mapping(mapping) => mapping.flags(),
        }
    }

    pub fn is_mapping(&self) -> bool {
        matches!(self, Self::Mapping(_))
    }

    pub fn as_vm_address_region(&self) -> Option<&Arc<VmAddressRegion>> {
        match self {
            Self::Vmar(vmar) => Some(vmar),
            Self::Mapping(_) => None,
        }
    }

    pub fn as_vm_mapping(&self) -> Option<&Arc<VmMapping>> {
        match self {
            Self::Vmar(_) => None,
            Self::Mapping(mapping) => Some(mapping),
        }
    }

    fn last_byte(&self) -> usize {
        self.base() + (self.size() - 1)
    }

    pub(crate) fn destroy_locked(&self, guard: &mut AspaceGuard<'_>) -> Result<(), Status> {
        match self {
            Self::Vmar(vmar) => vmar.destroy_locked(guard),
            Self::Mapping(mapping) => mapping.destroy_locked(guard),
        }
    }

    /// Destroys the region, unmapping and removing it and everything within it.
    pub fn destroy(&self) -> Result<(), Status> {
        let aspace = match self {
            Self::Vmar(vmar) => vmar.aspace.clone(),
            Self::Mapping(mapping) => mapping.aspace().clone(),
        };
        let mut guard = aspace.lock();
        self.destroy_locked(&mut guard)
    }
}

/// The children of a VMAR, keyed by base.
#[derive(Default)]
pub(crate) struct RegionList {
    regions: BTreeMap<usize, VmAddressRegionOrMapping>,
}

impl RegionList {
    pub(crate) fn insert_region(&mut self, region: VmAddressRegionOrMapping) {
        let previous = self.regions.insert(region.base(), region);
        debug_assert!(previous.is_none());
    }

    pub(crate) fn remove_region(&mut self, base: usize) {
        let removed = self.regions.remove(&base);
        assert!(removed.is_some());
    }

    pub(crate) fn replace_region(&mut self, region: VmAddressRegionOrMapping) {
        let slot = self.regions.get_mut(&region.base()).expect("replaced region exists");
        *slot = region;
    }

    fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    fn front(&self) -> Option<&VmAddressRegionOrMapping> {
        self.regions.values().next()
    }

    /// Returns the region containing `addr`.
    fn find_region(&self, addr: usize) -> Option<&VmAddressRegionOrMapping> {
        let (_, region) = self.regions.range(..=addr).next_back()?;
        debug_assert!(region.size() > 0);
        (addr <= region.last_byte()).then_some(region)
    }

    /// Returns the regions that include `base` or are above it, in order.
    fn include_or_higher(&self, base: usize) -> impl Iterator<Item = &VmAddressRegionOrMapping> {
        let start = match self.find_region(base) {
            Some(region) => region.base(),
            None => base,
        };
        self.regions.range(start..).map(|(_, region)| region)
    }

    /// Returns whether [base, base + size) overlaps no region.
    fn is_range_available(&self, base: usize, size: usize) -> bool {
        debug_assert!(size > 0);
        let Some(last_byte) = base.checked_add(size - 1) else { return false };
        if let Some((_, prev)) = self.regions.range(..=base).next_back()
            && prev.last_byte() >= base
        {
            return false;
        }
        self.regions
            .range((Bound::Excluded(base), Bound::Unbounded))
            .next()
            .is_none_or(|(_, next)| next.base() > last_byte)
    }

    /// Finds the lowest spot aligned to `1 << align_pow2` where `size` bytes fit within
    /// [parent_base, parent_base + parent_size) below `upper_limit`.
    fn get_alloc_spot(
        &self,
        align_pow2: u8,
        size: usize,
        parent_base: usize,
        parent_size: usize,
        upper_limit: usize,
    ) -> Result<usize, Status> {
        let align = 1usize << align_pow2;
        let parent_end = parent_base.saturating_add(parent_size).min(upper_limit);
        let fits = |gap_base: usize, gap_end: usize| {
            let spot = gap_base.checked_next_multiple_of(align)?;
            (spot.checked_add(size)? <= gap_end.min(parent_end)).then_some(spot)
        };
        let mut gap_base = parent_base;
        for region in self.regions.values() {
            if let Some(spot) = fits(gap_base, region.base()) {
                return Ok(spot);
            }
            gap_base = region.base() + region.size();
        }
        fits(gap_base, parent_end).ok_or(Status::NO_RESOURCES)
    }
}

/// State of a VMAR protected by the aspace lock.
pub(crate) struct VmarState {
    life: LifeCycleState,
    parent: Option<Weak<VmAddressRegion>>,
    pub(crate) subregions: RegionList,
}

pub struct VmAddressRegion {
    base: usize,
    size: usize,
    flags: u32,
    aspace: Arc<VmAspace>,
    name: [u8; ZX_MAX_NAME_LEN],
    this: Weak<VmAddressRegion>,
    state: LockedBy<VmarState, AspaceLocked>,
}

fn make_name(name: &str) -> [u8; ZX_MAX_NAME_LEN] {
    let mut stored = [0; ZX_MAX_NAME_LEN];
    let len = name.len().min(ZX_MAX_NAME_LEN - 1);
    stored[..len].copy_from_slice(&name.as_bytes()[..len]);
    stored
}

/// Returns whether `arch_mmu_flags` needs only the permissions `vmar_flags` allows.
pub(crate) fn is_valid_mapping_flags(vmar_flags: u32, arch_mmu_flags: ArchMmuFlags) -> bool {
    let mut needed = 0;
    if arch_mmu_flags & ARCH_MMU_FLAG_PERM_READ != 0 {
        needed |= VMAR_FLAG_CAN_MAP_READ;
    }
    if arch_mmu_flags & ARCH_MMU_FLAG_PERM_WRITE != 0 {
        needed |= VMAR_FLAG_CAN_MAP_WRITE;
    }
    if arch_mmu_flags & ARCH_MMU_FLAG_PERM_EXECUTE != 0 {
        needed |= VMAR_FLAG_CAN_MAP_EXECUTE;
    }
    let actual = vmar_flags & VMAR_CAN_RWX_FLAGS;
    (needed & actual) == needed
}

/// Returns the intersection of [offset1, offset1 + len1) and [offset2, offset2 + len2).
pub(crate) fn get_intersect(
    offset1: usize,
    len1: usize,
    offset2: usize,
    len2: usize,
) -> Option<(usize, usize)> {
    let start = offset1.max(offset2);
    let end = (offset1 + len1).min(offset2 + len2);
    (start < end).then(|| (start, end - start))
}

impl VmAddressRegion {
    fn new(
        aspace: &Arc<VmAspace>,
        parent: Option<&Arc<VmAddressRegion>>,
        base: usize,
        size: usize,
        flags: u32,
        name: &str,
    ) -> Arc<Self> {
        Arc::new_cyclic(|this| Self {
            base,
            size,
            flags,
            aspace: aspace.clone(),
            name: make_name(name),
            this: this.clone(),
            // The region holds its aspace, and so the aspace lock, which must outlive every access
            // to the state it guards (`LockId`).
            state: LockedBy::new(
                aspace.lock_id(),
                VmarState {
                    life: LifeCycleState::NotReady,
                    parent: parent.map(Arc::downgrade),
                    subregions: RegionList::default(),
                },
            ),
        })
    }

    /// Creates the root VMAR of `aspace`, covering all of it.
    pub(crate) fn create_root_locked(
        aspace: &Arc<VmAspace>,
        guard: &mut AspaceGuard<'_>,
        vmar_flags: u32,
    ) -> Arc<Self> {
        // We add in CAN_RWX_FLAGS, since an address space can't usefully contain a process without
        // all of these.
        let vmar = Self::new(
            aspace,
            None,
            aspace.base(),
            aspace.size(),
            vmar_flags | VMAR_CAN_RWX_FLAGS,
            "root",
        );
        vmar.state_mut(guard).life = LifeCycleState::Alive;
        vmar
    }

    pub fn base(&self) -> usize {
        self.base
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn flags(&self) -> u32 {
        self.flags
    }

    pub fn aspace(&self) -> &Arc<VmAspace> {
        &self.aspace
    }

    pub fn name(&self) -> [u8; ZX_MAX_NAME_LEN] {
        self.name
    }

    fn arc(&self) -> Arc<VmAddressRegion> {
        self.this.upgrade().expect("VMAR is referenced")
    }

    /// The VMAR's state, to the aspace lock's holder, whose state `guard` is.
    pub(crate) fn state<'a>(&'a self, guard: &'a AspaceLocked) -> &'a VmarState {
        self.state.get(guard)
    }

    /// The VMAR's state, to change, to the aspace lock's holder, whose state `guard` is.
    pub(crate) fn state_mut<'a>(&'a self, guard: &'a mut AspaceLocked) -> &'a mut VmarState {
        self.state.get_mut(guard)
    }

    pub fn is_in_range(&self, base: usize, size: usize) -> bool {
        let offset = base.wrapping_sub(self.base);
        base >= self.base && offset < self.size && self.size - offset >= size
    }

    pub fn has_parent(&self) -> bool {
        let guard = self.aspace.lock();
        self.state(&guard).parent.is_some()
    }

    /// Creates a child VMAR.
    pub fn create_sub_vmar(
        &self,
        offset: usize,
        size: usize,
        align_pow2: u8,
        vmar_flags: u32,
        name: &str,
    ) -> Result<Arc<VmAddressRegion>, Status> {
        ensure!(page::is_aligned(size), Status::INVALID_ARGS);
        // Check that only allowed flags have been set
        if vmar_flags
            & !(VMAR_FLAG_SPECIFIC
                | VMAR_FLAG_CAN_MAP_SPECIFIC
                | VMAR_FLAG_COMPACT
                | VMAR_CAN_RWX_FLAGS
                | VMAR_FLAG_OFFSET_IS_UPPER_LIMIT)
            != 0
        {
            return Err(Status::INVALID_ARGS);
        }
        let (region, _) = self.create_sub_vmar_internal(
            offset,
            size,
            align_pow2,
            vmar_flags,
            None,
            0,
            ARCH_MMU_FLAG_INVALID,
            name,
        )?;
        Ok(region.as_vm_address_region().unwrap().clone())
    }

    /// Creates a mapping of `vmo` at `vmo_offset`, returning the mapping and its base.
    #[allow(clippy::too_many_arguments)]
    pub fn create_vm_mapping(
        &self,
        mapping_offset: usize,
        size: usize,
        align_pow2: u8,
        mut vmar_flags: u32,
        vmo: Arc<VmObjectPaged>,
        vmo_offset: u64,
        arch_mmu_flags: ArchMmuFlags,
        name: &str,
    ) -> Result<(Arc<VmMapping>, usize), Status> {
        // Check that only allowed flags have been set
        if vmar_flags
            & !(VMAR_FLAG_SPECIFIC
                | VMAR_FLAG_SPECIFIC_OVERWRITE
                | VMAR_CAN_RWX_FLAGS
                | VMAR_FLAG_OFFSET_IS_UPPER_LIMIT
                | VMAR_FLAG_DEBUG_DYNAMIC_KERNEL_MAPPING
                | VMAR_FLAG_FAULT_BEYOND_STREAM_SIZE)
            != 0
        {
            return Err(Status::INVALID_ARGS);
        }
        // Validate that arch_mmu_flags does not contain any prohibited flags
        ensure!(is_valid_mapping_flags(self.flags, arch_mmu_flags), Status::ACCESS_DENIED);
        ensure!(page::is_aligned(vmo_offset as usize), Status::INVALID_ARGS);
        // Make sure that rounding up the page size did not overflow.
        let mapping_size = size.checked_next_multiple_of(PAGE_SIZE).ok_or(Status::OUT_OF_RANGE)?;
        // Make sure that a mapping of this size wouldn't overflow the vmo offset.
        ensure!(vmo_offset.checked_add(mapping_size as u64).is_some(), Status::OUT_OF_RANGE);
        // If we're mapping it with a specific permission, we should allow future Protect() calls
        // on the mapping to keep that permission.
        if arch_mmu_flags & ARCH_MMU_FLAG_PERM_READ != 0 {
            vmar_flags |= VMAR_FLAG_CAN_MAP_READ;
        }
        if arch_mmu_flags & ARCH_MMU_FLAG_PERM_WRITE != 0 {
            vmar_flags |= VMAR_FLAG_CAN_MAP_WRITE;
        }
        if arch_mmu_flags & ARCH_MMU_FLAG_PERM_EXECUTE != 0 {
            vmar_flags |= VMAR_FLAG_CAN_MAP_EXECUTE;
        }
        let (region, base) = self.create_sub_vmar_internal(
            mapping_offset,
            mapping_size,
            align_pow2,
            vmar_flags,
            Some(vmo),
            vmo_offset,
            arch_mmu_flags,
            name,
        )?;
        Ok((region.as_vm_mapping().unwrap().clone(), base))
    }

    #[allow(clippy::too_many_arguments)]
    fn create_sub_vmar_internal(
        &self,
        offset: usize,
        size: usize,
        align_pow2: u8,
        mut vmar_flags: u32,
        vmo: Option<Arc<VmObjectPaged>>,
        vmo_offset: u64,
        arch_mmu_flags: ArchMmuFlags,
        name: &str,
    ) -> Result<(VmAddressRegionOrMapping, usize), Status> {
        let mut guard = self.aspace.lock();
        ensure!(self.state(&guard).life == LifeCycleState::Alive, Status::BAD_STATE);
        ensure!(size != 0, Status::INVALID_ARGS);
        // Check if there are any RWX privileges that the child would have that the parent does
        // not.
        ensure!(vmar_flags & !self.flags & VMAR_CAN_RWX_FLAGS == 0, Status::ACCESS_DENIED);
        // Extract the creation only flags out of vmar_flags, leaving the remaining to be stored
        // permanently in the object.
        let is_specific_overwrite = vmar_flags & VMAR_FLAG_SPECIFIC_OVERWRITE != 0;
        let is_specific = vmar_flags & VMAR_FLAG_SPECIFIC != 0 || is_specific_overwrite;
        let is_upper_bound = vmar_flags & VMAR_FLAG_OFFSET_IS_UPPER_LIMIT != 0;
        vmar_flags &=
            !(VMAR_FLAG_SPECIFIC_OVERWRITE | VMAR_FLAG_SPECIFIC | VMAR_FLAG_OFFSET_IS_UPPER_LIMIT);
        ensure!(!is_specific || !is_upper_bound, Status::INVALID_ARGS);
        ensure!(is_specific || is_upper_bound || offset == 0, Status::INVALID_ARGS);
        ensure!(page::is_aligned(offset), Status::INVALID_ARGS);
        // Check that we have the required privileges if we want a SPECIFIC or UPPER_LIMIT
        // mapping.
        ensure!(
            (!is_specific && !is_upper_bound) || self.flags & VMAR_FLAG_CAN_MAP_SPECIFIC != 0,
            Status::ACCESS_DENIED
        );
        ensure!(
            is_upper_bound || (offset < self.size && size <= self.size - offset),
            Status::INVALID_ARGS
        );
        ensure!(
            !is_upper_bound || (offset <= self.size && size <= self.size && size <= offset),
            Status::INVALID_ARGS
        );

        let new_base = if is_specific {
            // This would not overflow because offset <= size_ - 1, base_ + offset <= base_ + size_ - 1.
            let new_base = self.base + offset;
            ensure!(
                align_pow2 == 0 || new_base & ((1usize << align_pow2) - 1) == 0,
                Status::INVALID_ARGS
            );
            if !self.state(&guard).subregions.is_range_available(new_base, size) {
                if is_specific_overwrite {
                    let vmo = vmo.ok_or(Status::INVALID_ARGS)?;
                    let mapping = self.overwrite_vm_mapping_locked(
                        &mut guard,
                        new_base,
                        size,
                        vmar_flags,
                        vmo,
                        vmo_offset,
                        arch_mmu_flags,
                    )?;
                    return Ok((VmAddressRegionOrMapping::Mapping(mapping), new_base));
                }
                return Err(Status::ALREADY_EXISTS);
            }
            new_base
        } else {
            // If we're not mapping to a specific place, search for an opening.
            let upper_bound = if is_upper_bound { self.base + offset } else { usize::MAX };
            self.alloc_spot_locked(&guard, size, align_pow2, arch_mmu_flags, upper_bound)?
        };

        let region = match vmo {
            Some(vmo) => VmAddressRegionOrMapping::Mapping(VmMapping::new(
                &self.arc(),
                new_base,
                size,
                vmar_flags,
                vmo,
                if is_upper_bound { 0 } else { vmo_offset },
                arch_mmu_flags,
                BTreeMap::new(),
            )),
            None => VmAddressRegionOrMapping::Vmar(VmAddressRegion::new(
                &self.aspace,
                Some(&self.arc()),
                new_base,
                size,
                vmar_flags,
                name,
            )),
        };
        match &region {
            VmAddressRegionOrMapping::Vmar(vmar) => vmar.activate(&mut guard),
            VmAddressRegionOrMapping::Mapping(mapping) => mapping.activate(&mut guard),
        }
        Ok((region, new_base))
    }

    #[allow(clippy::too_many_arguments)]
    fn overwrite_vm_mapping_locked(
        &self,
        guard: &mut AspaceGuard<'_>,
        base: usize,
        size: usize,
        vmar_flags: u32,
        vmo: Arc<VmObjectPaged>,
        vmo_offset: u64,
        arch_mmu_flags: ArchMmuFlags,
    ) -> Result<Arc<VmMapping>, Status> {
        let mapping = VmMapping::new(
            &self.arc(),
            base,
            size,
            vmar_flags,
            vmo,
            vmo_offset,
            arch_mmu_flags,
            BTreeMap::new(),
        );
        self.unmap_internal_locked(guard, base, size, false)?;
        mapping.activate(guard);
        Ok(mapping)
    }

    /// Adds this new VMAR to its parent.
    fn activate(&self, guard: &mut AspaceGuard<'_>) {
        let state = self.state(guard);
        let parent = state.parent.as_ref().and_then(Weak::upgrade).expect("parent");
        debug_assert_eq!(state.life, LifeCycleState::NotReady);
        // Validate we are a correct child of our parent.
        debug_assert!(parent.is_in_range(self.base, self.size));
        let parent_state = parent.state_mut(guard);
        // Look for a region in the parent starting from our desired base. If any region is found,
        // make sure we do not intersect with it.
        if let Some(candidate) = parent_state.subregions.include_or_higher(self.base).next() {
            assert!(candidate.base() >= self.base + self.size);
        }
        parent_state.subregions.insert_region(VmAddressRegionOrMapping::Vmar(self.arc()));
        self.state_mut(guard).life = LifeCycleState::Alive;
    }

    /// Destroys this VMAR and everything within it.
    pub(crate) fn destroy_locked(&self, guard: &mut AspaceGuard<'_>) -> Result<(), Status> {
        // The cur reference prevents regions from being destructed after dropping the last
        // reference to them when removing from their parent.
        let mut cur = Some(self.arc());
        while let Some(vmar) = cur {
            // Iterate through children destroying mappings. If we find a subregion, stop so we can
            // traverse down.
            let mut child_region = None;
            while child_region.is_none() {
                let child = vmar.state(guard).subregions.front().cloned();
                match child {
                    None => break,
                    Some(VmAddressRegionOrMapping::Mapping(mapping)) => {
                        // destroy_locked should remove this child from our list on success.
                        mapping.destroy_locked(guard)?;
                    }
                    Some(VmAddressRegionOrMapping::Vmar(child)) => child_region = Some(child),
                }
            }
            if let Some(child) = child_region {
                // If we found a child region, traverse down the tree.
                cur = Some(child);
            } else {
                // All children are destroyed, so now destroy the current node.
                let parent = vmar.state_mut(guard).parent.take().and_then(|p| p.upgrade());
                if let Some(parent) = &parent {
                    parent.state_mut(guard).subregions.remove_region(vmar.base);
                }
                vmar.state_mut(guard).life = LifeCycleState::Dead;
                // If we destroyed the original node, stop. Otherwise traverse up the tree and keep
                // destroying.
                cur = if std::ptr::eq(Arc::as_ptr(&vmar), self) { None } else { parent };
            }
        }
        Ok(())
    }

    /// Destroys this VMAR and everything within it.
    pub fn destroy(&self) -> Result<(), Status> {
        let mut guard = self.aspace.lock();
        self.destroy_locked(&mut guard)
    }

    /// Returns the child region containing `addr`.
    pub fn find_region(&self, addr: usize) -> Option<VmAddressRegionOrMapping> {
        let guard = self.aspace.lock();
        let state = self.state(&guard);
        if state.life != LifeCycleState::Alive {
            return None;
        }
        state.subregions.find_region(addr).cloned()
    }

    /// Returns the mapping containing `va` anywhere below this VMAR.
    pub(crate) fn find_mapping_locked(
        &self,
        guard: &AspaceGuard<'_>,
        va: usize,
    ) -> Option<Arc<VmMapping>> {
        let mut next = self.state(guard).subregions.find_region(va).cloned();
        loop {
            match next? {
                VmAddressRegionOrMapping::Mapping(mapping) => return Some(mapping),
                VmAddressRegionOrMapping::Vmar(vmar) => {
                    next = vmar.state(guard).subregions.find_region(va).cloned();
                }
            }
        }
    }

    /// Unmaps [base, base + size), destroying the mappings (and, with `op_children`, the VMARs)
    /// entirely within it and splitting any that partly overlap it.
    pub fn unmap(
        &self,
        base: usize,
        size: usize,
        op_children: VmAddressRegionOpChildren,
    ) -> Result<(), Status> {
        let size = size.checked_next_multiple_of(PAGE_SIZE).ok_or(Status::INVALID_ARGS)?;
        ensure!(size != 0 && page::is_aligned(base), Status::INVALID_ARGS);
        let mut guard = self.aspace.lock();
        ensure!(self.state(&guard).life == LifeCycleState::Alive, Status::BAD_STATE);
        self.unmap_internal_locked(
            &mut guard,
            base,
            size,
            op_children == VmAddressRegionOpChildren::Yes,
        )
    }

    fn unmap_internal_locked(
        &self,
        guard: &mut AspaceGuard<'_>,
        base: usize,
        size: usize,
        can_destroy_regions: bool,
    ) -> Result<(), Status> {
        ensure!(self.is_in_range(base, size), Status::INVALID_ARGS);
        if self.state(guard).subregions.is_empty() {
            return Ok(());
        }
        debug_assert!(size > 0);
        let end_addr_byte = base.checked_add(size - 1).expect("range is in the vmar");

        // Check if we're partially spanning a subregion, or aren't allowed to destroy regions and
        // are spanning a region, and bail if we are.
        for region in self.state(guard).subregions.include_or_higher(base) {
            if region.base() > end_addr_byte {
                break;
            }
            if !region.is_mapping()
                && (!can_destroy_regions
                    || region.base() < base
                    || region.last_byte() > end_addr_byte)
            {
                return Err(Status::INVALID_ARGS);
            }
        }

        loop {
            // Every deletion invalidates our iterator, hence we have to lookup a new one every time.
            let curr_region = self.state(guard).subregions.include_or_higher(base).next().cloned();
            let Some(curr_region) = curr_region.filter(|r| r.base() <= end_addr_byte) else {
                break;
            };
            let (unmap_base, unmap_size) =
                get_intersect(base, size, curr_region.base(), curr_region.size())
                    .expect("region intersects the range");
            if unmap_base == curr_region.base() && unmap_size == curr_region.size() {
                curr_region.destroy_locked(guard)?;
            } else {
                // Already validated in the loop above that the only kind of region we can
                // partially span is a mapping.
                let mapping = curr_region.as_vm_mapping().expect("partially spanned mapping");
                mapping.unmap_locked(guard, unmap_base, unmap_size)?;
            }
        }
        Ok(())
    }

    /// Collects the mappings intersecting [base, last_byte] below this VMAR, in address order, with
    /// whether they are direct children.
    fn mappings_in_range(
        &self,
        guard: &AspaceGuard<'_>,
        base: usize,
        last_byte: usize,
    ) -> Vec<(Arc<VmMapping>, bool)> {
        fn collect(
            vmar: &VmAddressRegion,
            guard: &AspaceGuard<'_>,
            base: usize,
            last_byte: usize,
            direct: bool,
            out: &mut Vec<(Arc<VmMapping>, bool)>,
        ) {
            let children = vmar.state(guard).subregions.include_or_higher(base);
            for child in children.take_while(|r| r.base() <= last_byte) {
                match child {
                    VmAddressRegionOrMapping::Mapping(mapping) => {
                        out.push((mapping.clone(), direct));
                    }
                    VmAddressRegionOrMapping::Vmar(child) => {
                        collect(child, guard, base, last_byte, false, out);
                    }
                }
            }
        }
        let mut out = Vec::new();
        collect(self, guard, base, last_byte, true, &mut out);
        out
    }

    /// Changes the permissions of the mappings in [base, base + size), which must all be mapped.
    pub fn protect(
        &self,
        base: usize,
        size: usize,
        new_arch_mmu_flags: ArchMmuFlags,
        op_children: VmAddressRegionOpChildren,
    ) -> Result<(), Status> {
        let size = size.checked_next_multiple_of(PAGE_SIZE).ok_or(Status::INVALID_ARGS)?;
        ensure!(size != 0 && page::is_aligned(base), Status::INVALID_ARGS);
        let mut guard = self.aspace.lock();
        ensure!(self.state(&guard).life == LifeCycleState::Alive, Status::BAD_STATE);
        ensure!(self.is_in_range(base, size), Status::INVALID_ARGS);
        // Do not allow changing caching.
        ensure!(new_arch_mmu_flags & ARCH_MMU_FLAG_CACHE_MASK == 0, Status::INVALID_ARGS);
        // The last byte of the range.
        let end_addr_byte = base + (size - 1);
        let mappings = self.mappings_in_range(&guard, base, end_addr_byte);

        // Check part of the range is not mapped, or the new permissions are invalid for some
        // mapping in the range.
        let mut expected = base;
        for (mapping, direct) in &mappings {
            ensure!(mapping.base() <= expected, Status::NOT_FOUND);
            let end = mapping.base() + mapping.size();
            ensure!(
                is_valid_mapping_flags(mapping.flags(), new_arch_mmu_flags),
                Status::ACCESS_DENIED
            );
            if !direct {
                ensure!(op_children != VmAddressRegionOpChildren::No, Status::INVALID_ARGS);
                // As this is a sub-region we cannot increase its mapping flags, even if they might
                // otherwise be permissible. A mapping might have multiple different protect regions
                // so need to check all of them within the protection range.
                let len = end_addr_byte.min(end - 1) - expected + 1;
                mapping.enumerate_protection_ranges_locked(
                    &guard,
                    expected,
                    len,
                    |_, _, flags| {
                        ensure!(
                            flags & new_arch_mmu_flags == new_arch_mmu_flags,
                            Status::ACCESS_DENIED
                        );
                        Ok(true)
                    },
                )?;
            }
            expected = end;
        }
        ensure!(expected >= end_addr_byte, Status::NOT_FOUND);

        for (mapping, _) in mappings {
            let protect_base = mapping.base().max(base);
            let protect_end_byte = (mapping.base() + (mapping.size() - 1)).min(end_addr_byte);
            let protect_size = protect_end_byte - protect_base + 1;
            mapping.protect_locked(&mut guard, protect_base, protect_size, new_arch_mmu_flags)?;
        }
        Ok(())
    }

    /// Performs `op` on the mappings covering [base, base + len), which must have no gaps.
    pub fn range_op(
        &self,
        op: RangeOpType,
        base: usize,
        len: usize,
        op_children: VmAddressRegionOpChildren,
    ) -> Result<(), Status> {
        let len = len.checked_next_multiple_of(PAGE_SIZE).ok_or(Status::INVALID_ARGS)?;
        ensure!(len != 0 && page::is_aligned(base), Status::INVALID_ARGS);
        ensure!(self.is_in_range(base, len), Status::OUT_OF_RANGE);
        let last_addr = base + len;
        // Capture the validation that we need to do whenever the lock is acquired.
        let validate = |guard: &AspaceGuard<'_>| {
            ensure!(self.state(guard).life == LifeCycleState::Alive, Status::BAD_STATE);
            Ok(())
        };
        let mappings = {
            let guard = self.aspace.lock();
            validate(&guard)?;
            self.mappings_in_range(&guard, base, last_addr - 1)
        };
        let mut expected = base;
        for (mapping, direct) in mappings {
            // It's possible base is less than expected if the first mapping is not precisely
            // aligned to the start of our range. After that base should always be expected, and if
            // it's greater then there is a gap and this is considered an error.
            ensure!(mapping.base() <= expected, Status::BAD_STATE);
            debug_assert!(mapping.is_in_range(expected, 1));
            let mapping_offset = expected - mapping.base();
            let vmo_offset = mapping.object_offset() + mapping_offset as u64;
            let total_remain = last_addr - expected;
            let max_in_mapping = mapping.size() - mapping_offset;
            let size = total_remain.min(max_in_mapping);
            // The commit, decommit and prefetch ops check the maximal permissions of the mapping
            // and can be thought of as acting as if they perform a protect to add read or write
            // permissions. Since protect to add permissions through a parent VMAR is not valid we
            // similarly forbid this notional protect by not allowing these operations if acting
            // through a sub-vmar, regardless of whether op_children is otherwise allowed.
            if (matches!(op, RangeOpType::Commit | RangeOpType::Decommit | RangeOpType::Prefetch)
                || op_children == VmAddressRegionOpChildren::No)
                && !direct
            {
                return Err(Status::INVALID_ARGS);
            }
            let vmo = mapping.vmo().ok_or(Status::BAD_STATE)?;
            let can_write = is_valid_mapping_flags(mapping.flags(), ARCH_MMU_FLAG_PERM_WRITE);
            let size64 = size as u64;
            let result = match op {
                RangeOpType::Commit => {
                    if !can_write {
                        Err(Status::ACCESS_DENIED)
                    } else {
                        vmo.commit_range(vmo_offset, size64)
                            .and_then(|()| mapping.map_range(mapping_offset, size, false, true))
                    }
                }
                // Decommit zeroes pages of the VMO, equivalent to writing to it.
                RangeOpType::Decommit => {
                    if !can_write {
                        Err(Status::ACCESS_DENIED)
                    } else {
                        vmo.decommit_range(vmo_offset, size64)
                    }
                }
                RangeOpType::MapRange => mapping.map_range(mapping_offset, size, false, true),
                RangeOpType::Zero => {
                    if !can_write {
                        Err(Status::ACCESS_DENIED)
                    } else {
                        vmo.zero_range(vmo_offset, size64)
                    }
                }
                RangeOpType::AlwaysNeed => vmo
                    .hint_range(vmo_offset, size64, EvictionHint::AlwaysNeed)
                    .and_then(|()| mapping.map_range(mapping_offset, size, false, true)),
                RangeOpType::DontNeed => vmo.hint_range(vmo_offset, size64, EvictionHint::DontNeed),
                RangeOpType::Prefetch => {
                    if !is_valid_mapping_flags(mapping.flags(), ARCH_MMU_FLAG_PERM_READ) {
                        Err(Status::ACCESS_DENIED)
                    } else {
                        vmo.prefetch_range(vmo_offset, size64)
                            .and_then(|()| mapping.map_range(mapping_offset, size, false, true))
                    }
                }
            };
            // Since the lock was dropped we must re-validate before doing anything else.
            validate(&self.aspace.lock())?;
            result?;
            expected += size;
        }
        // Check if there was a gap right at the end of the range.
        ensure!(expected >= last_addr, Status::BAD_STATE);
        Ok(())
    }

    /// Allocates a spot for a region of `size` bytes, below `upper_limit`.
    fn alloc_spot_locked(
        &self,
        guard: &AspaceGuard<'_>,
        size: usize,
        align_pow2: u8,
        arch_mmu_flags: ArchMmuFlags,
        upper_limit: usize,
    ) -> Result<usize, Status> {
        debug_assert!(size > 0 && page::is_aligned(size));
        let align_pow2 = align_pow2.max(PAGE_SHIFT);
        let spot = self.state(guard).subregions.get_alloc_spot(
            align_pow2,
            size,
            self.base,
            self.size,
            upper_limit,
        )?;
        let va = self.aspace.arch_aspace().pick_spot(
            spot,
            spot + (size - 1),
            1 << align_pow2,
            size,
            arch_mmu_flags,
        );
        debug_assert_eq!(va, spot);
        Ok(va)
    }

    /// Whether a range of bytes is within this VMAR and valid for an operation.
    pub fn check_range(&self, base: usize, len: usize) -> bool {
        in_range(base.wrapping_sub(self.base) as u64, len as u64, self.size as u64)
            && base >= self.base
    }
}
