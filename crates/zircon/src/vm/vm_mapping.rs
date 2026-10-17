// Copyright 2016 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Mappings of VMOs into address spaces.
//!
//! Ported from `zircon/kernel/vm/vm_mapping.cc` and the `VmMapping` parts of
//! `zircon/kernel/vm/include/vm/vm_address_region.h`.
//!
//! A mapping's pages are mapped lazily, on faults, and the VMO updates its mappings when its
//! content changes (`RangeChangeListener`).
//!
//! Deviations:
//! - The fields Zircon guards with the aspace lock and the VMO lock are kept in a lock of the
//!   mapping's own, which is always taken last.
//! - Only paged VMOs, and no private clones, memory priorities, attribution or merging of
//!   neighboring mappings.
//! - `map_range` hands the arch aspace up to a page table's worth of pages at a time
//!   (`MAP_RANGE_MAX_PAGES`), rather than 16: each call costs the host system calls, which map
//!   runs of consecutive physical pages together.

use crate::ensure;
use crate::kernel::types::PAddr;
use crate::vm::arch_vm_aspace::*;
use crate::vm::fault::flag;
use crate::vm::page_source::MultiPageRequest;
use crate::vm::vm_address_region::{
    LifeCycleState, VMAR_FLAG_CAN_MAP_WRITE, VmAddressRegion, VmAddressRegionOrMapping,
    get_intersect, is_valid_mapping_flags,
};
use crate::vm::vm_aspace::{AspaceGuard, VmAspace};
use crate::vm::vm_cow_pages::{
    DeferredOps, RangeChangeListener, RangeChangeOp, RequireResult, VmCowPagesLocked, VmCowRange,
};
use crate::vm::vm_object_paged::VmObjectPaged;
use parking_lot::{Mutex, MutexGuard};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use zx_status::Status;

const PAGE_SIZE: usize = page::SIZE;

/// Pages mapped by one page table, whose boundary opportunistic faulting does not cross.
const PAGE_TABLE_SPAN: usize = PAGE_SIZE * (PAGE_SIZE / 8);

/// The maximum number of pages a fault maps, including opportunistic pages.
pub const PAGE_FAULT_MAX_OPTIMISTIC_PAGES: usize = 16;

/// The maximum number of pages `map_range` maps with one call to the arch aspace.
const MAP_RANGE_MAX_PAGES: usize = PAGE_TABLE_SPAN / PAGE_SIZE;

#[derive(Clone, Copy, PartialEq, Eq)]
enum UnmapOptions {
    None,
    OnlyHasZeroPages,
    Harvest,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CurrentlyFaultingState {
    NoUnmapNeeded,
    UnmapSkipped,
    Completed,
}

/// Tracks an in progress fault, so that unmaps of the zero page it is about to replace can be
/// skipped. The faulting thread either updates the mapping or unmaps the range itself.
struct CurrentlyFaulting {
    /// The offset, in object space, of the page fault.
    object_offset: u64,
    len: u64,
    state: CurrentlyFaultingState,
}

impl CurrentlyFaulting {
    /// Called to say that the given range needs to be unmapped. This returns true if updating the
    /// range will be handled by the faulting thread and that the unmap can therefore be skipped.
    fn unmap_range(&mut self, object_offset: u64, len: u64) -> bool {
        debug_assert!(self.state != CurrentlyFaultingState::Completed);
        let intersects = object_offset < self.object_offset + self.len
            && self.object_offset < object_offset + len;
        if intersects {
            self.state = CurrentlyFaultingState::UnmapSkipped;
        }
        intersects
    }
}

/// The mapping's mutable state.
struct MappingState {
    life: LifeCycleState,
    parent: Option<Weak<VmAddressRegion>>,
    object: Option<Arc<VmObjectPaged>>,
    /// The protection of the first page, and of each page until the first of
    /// `rest_protection_ranges`, which records each address at which the protection changes.
    first_region_arch_mmu_flags: ArchMmuFlags,
    rest_protection_ranges: BTreeMap<usize, ArchMmuFlags>,
    currently_faulting: Option<CurrentlyFaulting>,
}

struct FlagsRange {
    mmu_flags: ArchMmuFlags,
    region_top: usize,
}

pub struct VmMapping {
    base: usize,
    size: usize,
    flags: u32,
    object_offset: u64,
    aspace: Arc<VmAspace>,
    this: Weak<VmMapping>,
    /// Set once the object is detached on destruction, so faults racing with it can retry.
    object_reset: AtomicBool,
    state: Mutex<MappingState>,
}

/// Batches up to `N` pages to map with the same flags into one call to the arch aspace.
struct VmMappingCoalescer<'a, const N: usize> {
    mapping: &'a VmMapping,
    base: usize,
    phys: [PAddr; N],
    count: usize,
    total_mapped: usize,
    mmu_flags: ArchMmuFlags,
    existing_entry_action: ExistingEntryAction,
}

impl<'a, const N: usize> VmMappingCoalescer<'a, N> {
    fn new(
        mapping: &'a VmMapping,
        base: usize,
        mmu_flags: ArchMmuFlags,
        existing_entry_action: ExistingEntryAction,
    ) -> Self {
        debug_assert!(mmu_flags & ARCH_MMU_FLAG_PERM_RWX_MASK != 0);
        Self {
            mapping,
            base,
            phys: [PAddr(0); N],
            count: 0,
            total_mapped: 0,
            mmu_flags,
            existing_entry_action,
        }
    }

    fn can_append(&self, vaddr: usize) -> bool {
        self.count < self.phys.len() && vaddr == self.base + self.count * PAGE_SIZE
    }

    fn append(&mut self, vaddr: usize, paddr: PAddr) -> Result<(), Status> {
        if !self.can_append(vaddr) {
            self.flush()?;
            self.base = vaddr;
        }
        self.phys[self.count] = paddr;
        self.count += 1;
        Ok(())
    }

    fn append_or_adjust_mapping(
        &mut self,
        vaddr: usize,
        paddr: PAddr,
        mmu_flags: ArchMmuFlags,
    ) -> Result<(), Status> {
        if !self.can_append(vaddr) || mmu_flags != self.mmu_flags {
            self.flush()?;
            self.base = vaddr;
            self.mmu_flags = mmu_flags;
        }
        self.phys[self.count] = paddr;
        self.count += 1;
        Ok(())
    }

    /// How many more pages could be appended from `vaddr` without a flush.
    fn extra_page_capacity_from(&self, vaddr: usize) -> usize {
        if self.can_append(vaddr) && self.count != 0 { self.phys.len() - self.count } else { 0 }
    }

    fn flush(&mut self) -> Result<(), Status> {
        if self.count == 0 {
            return Ok(());
        }
        let status = self.mapping.aspace.arch_aspace().map(
            self.base,
            &self.phys[..self.count],
            self.mmu_flags,
            self.existing_entry_action,
        );
        if status.is_ok() {
            self.total_mapped += self.count;
        }
        self.count = 0;
        status
    }
}

impl VmMapping {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        parent: &Arc<VmAddressRegion>,
        base: usize,
        size: usize,
        vmar_flags: u32,
        vmo: Arc<VmObjectPaged>,
        vmo_offset: u64,
        first_mmu_flags: ArchMmuFlags,
        ranges: BTreeMap<usize, ArchMmuFlags>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|this| Self {
            base,
            size,
            flags: vmar_flags,
            object_offset: vmo_offset,
            aspace: parent.aspace().clone(),
            this: this.clone(),
            object_reset: AtomicBool::new(false),
            state: Mutex::new(MappingState {
                life: LifeCycleState::NotReady,
                parent: Some(Arc::downgrade(parent)),
                object: Some(vmo),
                first_region_arch_mmu_flags: first_mmu_flags,
                rest_protection_ranges: ranges,
                currently_faulting: None,
            }),
        })
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

    pub fn object_offset(&self) -> u64 {
        self.object_offset
    }

    pub fn aspace(&self) -> &Arc<VmAspace> {
        &self.aspace
    }

    fn arc(&self) -> Arc<VmMapping> {
        self.this.upgrade().expect("mapping is referenced")
    }

    fn listener(&self) -> Arc<dyn RangeChangeListener> {
        self.arc()
    }

    pub fn is_in_range(&self, base: usize, size: usize) -> bool {
        let offset = base.wrapping_sub(self.base);
        base >= self.base && offset < self.size && self.size - offset >= size
    }

    /// The mapped VMO, unless the mapping was destroyed.
    pub fn vmo_locked(&self, _guard: &AspaceGuard<'_>) -> Option<Arc<VmObjectPaged>> {
        self.state.lock().object.clone()
    }

    /// The mapped VMO, unless the mapping was destroyed.
    pub fn vmo(&self) -> Option<Arc<VmObjectPaged>> {
        let guard = self.aspace.lock();
        self.vmo_locked(&guard)
    }

    /// Returns the protection at `va` and the end of the range with that protection.
    fn flags_range_at_addr(&self, state: &MappingState, va: usize) -> FlagsRange {
        let top = state
            .rest_protection_ranges
            .range(va + 1..)
            .next()
            .map_or(self.base + self.size, |(addr, _)| *addr);
        let mmu_flags = state
            .rest_protection_ranges
            .range(..=va)
            .next_back()
            .map_or(state.first_region_arch_mmu_flags, |(_, flags)| *flags);
        FlagsRange { mmu_flags, region_top: top }
    }

    /// Calls `func` with each range of pages of [base, base + size) with the same protection.
    /// `func` returns whether to continue.
    fn enumerate_protection_ranges_locked(
        &self,
        state: &MappingState,
        base: usize,
        size: usize,
        mut func: impl FnMut(usize, usize, ArchMmuFlags) -> Result<bool, Status>,
    ) -> Result<(), Status> {
        debug_assert!(self.is_in_range(base, size));
        if state.life != LifeCycleState::Alive {
            return Ok(());
        }
        let end = base + size;
        let mut range_start = base;
        let mut flags = state
            .rest_protection_ranges
            .range(..=base)
            .next_back()
            .map_or(state.first_region_arch_mmu_flags, |(_, flags)| *flags);
        let mut transitions =
            state.rest_protection_ranges.range(base + 1..).map(|(a, f)| (*a, *f)).peekable();
        loop {
            let next = transitions.peek().copied().filter(|(addr, _)| *addr < end);
            let range_end = next.map_or(end, |(addr, _)| addr);
            debug_assert!(range_start < range_end);
            if !func(range_start, range_end - range_start, flags)? {
                return Ok(());
            }
            let Some((addr, next_flags)) = next else { return Ok(()) };
            transitions.next();
            range_start = addr;
            flags = next_flags;
        }
    }

    /// Calls `func` with each range of pages of [base, base + size) with the same protection.
    pub(crate) fn enumerate_protection_ranges(
        &self,
        base: usize,
        size: usize,
        func: impl FnMut(usize, usize, ArchMmuFlags) -> Result<bool, Status>,
    ) -> Result<(), Status> {
        let state = self.state.lock();
        self.enumerate_protection_ranges_locked(&state, base, size, func)
    }

    /// Protects the pages, unmapping them rather than removing all permissions. Cannot add write
    /// permission.
    fn protect_or_unmap(
        aspace: &VmAspace,
        base: usize,
        size: usize,
        new_arch_mmu_flags: ArchMmuFlags,
    ) -> Result<(), Status> {
        // This can never be used to set a WRITE permission since it does not ask the underlying
        // VMO to perform the copy-on-write step. The underlying VMO might also support dirty
        // tracking, which requires write permission faults in order to track pages as dirty when
        // written.
        assert_eq!(new_arch_mmu_flags & ARCH_MMU_FLAG_PERM_WRITE, 0);
        // If not removing all permissions do the protect, otherwise skip straight to unmapping the
        // entire region.
        if new_arch_mmu_flags & ARCH_MMU_FLAG_PERM_RWX_MASK != 0 {
            let status = aspace.arch_aspace().protect(
                base,
                size / PAGE_SIZE,
                new_arch_mmu_flags,
                aspace.enlarge_arch_unmap(),
            );
            if status.is_ok() || aspace.enlarge_arch_unmap() == ARCH_UNMAP_OPTION_NONE {
                return status;
            }
        }
        aspace.arch_aspace().unmap(base, size / PAGE_SIZE, aspace.enlarge_arch_unmap())
    }

    /// Changes the protection of [base, base + size).
    pub(crate) fn protect_locked(
        &self,
        base: usize,
        size: usize,
        new_arch_mmu_flags: ArchMmuFlags,
    ) -> Result<(), Status> {
        // Assert a few things that should already have been checked by the caller.
        debug_assert!(size != 0 && page::is_aligned(base) && page::is_aligned(size));
        debug_assert_eq!(new_arch_mmu_flags & ARCH_MMU_FLAG_CACHE_MASK, 0);
        debug_assert!(is_valid_mapping_flags(self.flags, new_arch_mmu_flags));
        let object = self.state.lock().object.clone().expect("alive mapping has an object");
        // grab the lock for the vmo
        let _object_guard = object.cow_pages().lock();
        let mut state = self.state.lock();

        // Persist our current caching mode. Every protect region will have the same caching mode
        // so we can acquire this from any region.
        let new_arch_mmu_flags =
            new_arch_mmu_flags | (state.first_region_arch_mmu_flags & ARCH_MMU_FLAG_CACHE_MASK);

        // Efficiently calculate the flags before, at, and after the requested range.
        let flags_at = |state: &MappingState, addr: usize| {
            state
                .rest_protection_ranges
                .range(..=addr)
                .next_back()
                .map_or(state.first_region_arch_mmu_flags, |(_, flags)| *flags)
        };
        let base_exists = state.rest_protection_ranges.contains_key(&base);
        let flags_at_base = flags_at(&state, base);
        let flags_before =
            if base > self.base { flags_at(&state, base - 1) } else { flags_at_base };
        let end = base + size;
        let end_exists = state.rest_protection_ranges.contains_key(&end);
        let flags_end = flags_at(&state, end);

        // We only need a boundary node at 'base' if it's not the start of the mapping and the
        // preceding flags are different from the new ones.
        let need_base = base > self.base && flags_before != new_arch_mmu_flags;
        // Similarly, we need a boundary node at 'base + size' if it's not the end of the mapping
        // and the new flags for the range are different from the existing flags that follow it.
        let need_end = end < self.base + self.size && new_arch_mmu_flags != flags_end;

        // Perform any required insertions first, with the *old* flags, as a structural change
        // only.
        if need_base && !base_exists {
            state.rest_protection_ranges.insert(base, flags_at_base);
        }
        if need_end && !end_exists {
            state.rest_protection_ranges.insert(end, flags_end);
        }

        // Now that all structural nodes are guaranteed to exist, perform the architectural
        // protection changes.
        let aspace = &self.aspace;
        self.enumerate_protection_ranges_locked(&state, base, size, |base, size, old_flags| {
            // Perform an early return if the new and old flags are the same, as there's nothing
            // to be done.
            if new_arch_mmu_flags == old_flags {
                return Ok(true);
            }
            let mut flags = new_arch_mmu_flags;
            // Check if the new flags have the write permission. This is problematic as we cannot
            // just change any existing hardware mappings to have the write permission, as any
            // individual mapping may be the result of a read fault and still need to have a
            // copy-on-write step performed. This could also map a dirty tracked VMO which requires
            // write permission faults to track pages as dirty when written.
            if new_arch_mmu_flags & ARCH_MMU_FLAG_PERM_WRITE != 0 {
                // Whatever happens, we're not going to be protecting the arch aspace to have write
                // mappings, so this has to be a user aspace so that we can lazily take write faults
                // in the future.
                assert!(aspace.is_user() || aspace.is_guest_physical());
                flags &= !ARCH_MMU_FLAG_PERM_WRITE;
                // If the new flags without write permission are the same as the old flags, then
                // skip the protect step since it will be a no-op.
                if flags == old_flags {
                    return Ok(true);
                }
            }
            // If the protect failed then we do not have sufficient information left to rollback in
            // order to return an error, nor can we claim success, so require the protect to have
            // succeeded to continue.
            Self::protect_or_unmap(aspace, base, size, flags).expect("protect");
            Ok(true)
        })?;

        // Update the protection flags for the start of the range.
        if base == self.base {
            state.first_region_arch_mmu_flags = new_arch_mmu_flags;
        } else if need_base {
            state.rest_protection_ranges.insert(base, new_arch_mmu_flags);
        } else {
            // If the boundary node is no longer needed (e.g., flags now match), remove it.
            state.rest_protection_ranges.remove(&base);
        }
        // If a boundary node at 'base + size' is not needed, remove it to keep the tree minimal.
        if !need_end {
            state.rest_protection_ranges.remove(&end);
        }
        // Finally, erase any internal boundary nodes that were entirely within the protected
        // range, as the entire range [base, base + size) now has uniform protections.
        let internal: Vec<usize> =
            state.rest_protection_ranges.range(base + 1..end).map(|(addr, _)| *addr).collect();
        for addr in internal {
            state.rest_protection_ranges.remove(&addr);
        }
        Ok(())
    }

    /// Unmaps [base, base + size), which must not be the whole mapping, splitting it into the
    /// mappings either side.
    pub(crate) fn unmap_locked(
        &self,
        guard: &mut AspaceGuard<'_>,
        base: usize,
        size: usize,
    ) -> Result<(), Status> {
        debug_assert!(size != 0 && page::is_aligned(size) && page::is_aligned(base));
        debug_assert!(base >= self.base && base - self.base < self.size);
        debug_assert!(self.size - (base - self.base) >= size);
        // Should never be unmapping everything, otherwise should destroy.
        debug_assert!(base != self.base || size != self.size);

        let (parent, object) = {
            let state = self.state.lock();
            ensure!(state.life == LifeCycleState::Alive, Status::BAD_STATE);
            (
                state.parent.as_ref().and_then(Weak::upgrade).expect("alive mapping has a parent"),
                state.object.clone().expect("alive mapping has an object"),
            )
        };

        // First create any new mapping. One or two might be required depending on whether
        // unmapping from an end or the middle.
        let left = (self.base != base).then(|| {
            VmMapping::new(
                &parent,
                self.base,
                base - self.base,
                self.flags,
                object.clone(),
                self.object_offset,
                0,
                BTreeMap::new(),
            )
        });
        let right = (base + size != self.base + self.size).then(|| {
            let offset = base + size - self.base;
            VmMapping::new(
                &parent,
                self.base + offset,
                self.size - offset,
                self.flags,
                object.clone(),
                self.object_offset + offset as u64,
                0,
                BTreeMap::new(),
            )
        });

        // Grab the lock for the vmo. This is acquired here so that it is held continuously over
        // both the architectural unmap and removing the current mapping from the VMO.
        let mut object_guard = object.cow_pages().lock();

        // First install any right mapping, then if there is a left mapping replace the current
        // mapping. As the locks are held throughout, the intermediate states are never visible.
        if let Some(right) = &right {
            parent
                .state()
                .subregions
                .insert_region(VmAddressRegionOrMapping::Mapping(right.clone()));
            object_guard.add_mapping(right.clone());
        }
        if let Some(left) = &left {
            object_guard.add_mapping(left.clone());
            parent
                .state()
                .subregions
                .replace_region(VmAddressRegionOrMapping::Mapping(left.clone()));
        } else {
            parent.state().subregions.remove_region(self.base);
        }

        self.aspace
            .arch_aspace()
            .unmap(base, size / PAGE_SIZE, self.aspace.enlarge_arch_unmap())
            .expect("unmap");

        // Split the protection ranges from this mapping into the new mapping(s).
        {
            let state = self.state.lock();
            if let Some(right) = &right {
                let split = base + size;
                let first = state
                    .rest_protection_ranges
                    .range(..=split)
                    .next_back()
                    .map_or(state.first_region_arch_mmu_flags, |(_, flags)| *flags);
                let ranges: BTreeMap<_, _> = state
                    .rest_protection_ranges
                    .range(split + 1..)
                    .map(|(addr, flags)| (*addr, *flags))
                    .collect();
                let mut right_state = right.state.lock();
                right_state.first_region_arch_mmu_flags = first;
                right_state.rest_protection_ranges = ranges;
            }
            if let Some(left) = &left {
                let ranges: BTreeMap<_, _> = state
                    .rest_protection_ranges
                    .range(..base)
                    .map(|(addr, flags)| (*addr, *flags))
                    .collect();
                let mut left_state = left.state.lock();
                left_state.first_region_arch_mmu_flags = state.first_region_arch_mmu_flags;
                left_state.rest_protection_ranges = ranges;
            }
        }

        // Now finish destroying this mapping. We already updated the subregion list in the parent.
        self.destroy_locked_object(guard, &mut object_guard, false, false)?;

        // Install the new mappings. We already updated the parent region list and object mapping
        // list.
        for mapping in left.iter().chain(right.iter()) {
            mapping.state.lock().life = LifeCycleState::Alive;
        }
        Ok(())
    }

    /// Adds this new mapping to its parent and its VMO.
    pub(crate) fn activate(&self, _guard: &mut AspaceGuard<'_>) {
        let (parent, object) = {
            let state = self.state.lock();
            debug_assert_eq!(state.life, LifeCycleState::NotReady);
            (
                state.parent.as_ref().and_then(Weak::upgrade).expect("parent"),
                state.object.clone().expect("object"),
            )
        };
        let mut object_guard = object.cow_pages().lock();
        parent.state().subregions.insert_region(VmAddressRegionOrMapping::Mapping(self.arc()));
        object_guard.add_mapping(self.listener());
        // Mappings of paged VMOs are always cached, which is what an unset cache policy means.
        self.state.lock().life = LifeCycleState::Alive;
    }

    /// Unmaps and destroys this mapping.
    pub(crate) fn destroy_locked(&self, guard: &mut AspaceGuard<'_>) -> Result<(), Status> {
        // Keep a reference to the object so we know our lock remains valid.
        let object = self.state.lock().object.clone().ok_or(Status::BAD_STATE)?;
        let mut object_guard = object.cow_pages().lock();
        self.destroy_locked_object(guard, &mut object_guard, true, true)
    }

    fn destroy_locked_object(
        &self,
        guard: &mut AspaceGuard<'_>,
        object_guard: &mut MutexGuard<'_, VmCowPagesLocked>,
        unmap: bool,
        remove_from_parent: bool,
    ) -> Result<(), Status> {
        // Take a reference to ourself, so that we do not get destructed after dropping our last
        // reference in this method.
        let this = self.arc();
        // If this is the last fault then clear it before removing from the VMAR tree.
        guard.clear_last_fault(self);
        if unmap {
            self.aspace.arch_aspace().unmap(
                self.base,
                self.size / PAGE_SIZE,
                self.aspace.enlarge_arch_unmap(),
            )?;
        }
        object_guard.remove_mapping(&(this.clone() as Arc<dyn RangeChangeListener>));
        let mut state = self.state.lock();
        state.rest_protection_ranges.clear();
        // Detach the region from the parent.
        let parent = state.parent.take().and_then(|p| p.upgrade());
        if let Some(parent) = parent
            && remove_from_parent
        {
            parent.state().subregions.remove_region(self.base);
        }
        // detach from any object we have mapped.
        state.object = None;
        self.object_reset.store(true, Ordering::Relaxed);
        // mark ourself as dead
        state.life = LifeCycleState::Dead;
        Ok(())
    }

    /// Returns the part of [offset, offset + len) of the object that this maps, as a range of
    /// addresses.
    fn object_range_to_vaddr_range(&self, offset: u64, len: u64) -> Option<(usize, usize)> {
        debug_assert!(page::is_aligned(offset as usize) && page::is_aligned(len as usize));
        // Zero sized ranges are considered to have no overlap.
        if len == 0 {
            return None;
        }
        // compute the intersection of the passed in vmo range and our mapping
        let (offset_new, virtual_len) = get_intersect(
            self.object_offset as usize,
            self.size,
            offset as usize,
            len.min((usize::MAX - offset as usize) as u64) as usize,
        )?;
        let base = self.base + (offset_new - self.object_offset as usize);
        debug_assert!(base + virtual_len <= self.base + self.size);
        Some((base, virtual_len))
    }

    fn aspace_unmap_locked_object(&self, offset: u64, len: u64, options: UnmapOptions) {
        let mut state = self.state.lock();
        debug_assert_eq!(state.life, LifeCycleState::Alive);
        // In the case of unmapping known instances of the zero page check if this range intersects
        // with an in progress fault. If it does we can skip the unmap with the knowledge that the
        // mapping will be updated later.
        if options == UnmapOptions::OnlyHasZeroPages
            && let Some(currently_faulting) = &mut state.currently_faulting
            && currently_faulting.unmap_range(offset, len)
        {
            return;
        }
        drop(state);
        // See if there's an intersect.
        let Some((base, new_len)) = self.object_range_to_vaddr_range(offset, len) else { return };
        let mut aspace_op = self.aspace.enlarge_arch_unmap();
        if options == UnmapOptions::Harvest {
            aspace_op |= ARCH_UNMAP_OPTION_HARVEST;
        }
        self.aspace.arch_aspace().unmap(base, new_len / PAGE_SIZE, aspace_op).expect("unmap");
    }

    fn aspace_remove_write_locked_object(&self, offset: u64, len: u64) {
        // If this doesn't support writing then nothing to be done, as we know we have no write
        // mappings.
        if self.flags & VMAR_FLAG_CAN_MAP_WRITE == 0 {
            return;
        }
        // See if there's an intersect.
        let Some((base, new_len)) = self.object_range_to_vaddr_range(offset, len) else { return };
        let state = self.state.lock();
        debug_assert_eq!(state.life, LifeCycleState::Alive);
        let aspace = &self.aspace;
        self.enumerate_protection_ranges_locked(
            &state,
            base,
            new_len,
            |region_base, region_len, mmu_flags| {
                // If this range doesn't currently support being writable then we can skip.
                if mmu_flags & ARCH_MMU_FLAG_PERM_WRITE == 0 {
                    return Ok(true);
                }
                // Build new mmu flags without writing.
                Self::protect_or_unmap(
                    aspace,
                    region_base,
                    region_len,
                    mmu_flags & !ARCH_MMU_FLAG_PERM_WRITE,
                )?;
                Ok(true)
            },
        )
        .expect("remove write");
    }

    /// Maps the pages of [offset, offset + len) of the mapping that are present, or with `commit`
    /// commits them first.
    pub fn map_range(
        &self,
        offset: usize,
        len: usize,
        commit: bool,
        ignore_existing: bool,
    ) -> Result<(), Status> {
        let guard = self.aspace.lock();
        let len = len.checked_next_multiple_of(PAGE_SIZE).ok_or(Status::INVALID_ARGS)?;
        ensure!(len != 0, Status::INVALID_ARGS);
        let object = {
            let state = self.state.lock();
            ensure!(state.life == LifeCycleState::Alive, Status::BAD_STATE);
            state.object.clone().expect("alive mapping has an object")
        };
        ensure!(
            page::is_aligned(offset) && self.is_in_range(self.base + offset, len),
            Status::INVALID_ARGS
        );
        drop(guard);
        // Cache whether the object is dirty tracked, we need to know this when computing mmu flags
        // later.
        let dirty_tracked = object.is_dirty_tracked();
        // The region to map could have multiple different current arch mmu flags, so we need to
        // iterate over them to ensure we install mappings with the correct permissions.
        let ranges = {
            let state = self.state.lock();
            let mut ranges = Vec::new();
            self.enumerate_protection_ranges_locked(
                &state,
                self.base + offset,
                len,
                |base, len, flags| {
                    ranges.push((base, len, flags));
                    Ok(true)
                },
            )?;
            ranges
        };
        for (base, len, mut mmu_flags) in ranges {
            // Remove the write permission if this maps a vmo that supports dirty tracking, in order
            // to trigger write permission faults when writes occur, enabling us to track when pages
            // are dirtied.
            if dirty_tracked {
                mmu_flags &= !ARCH_MMU_FLAG_PERM_WRITE;
            }
            // If there are no access permissions on this region then mapping has no effect, so
            // skip.
            if mmu_flags & ARCH_MMU_FLAG_PERM_RWX_MASK == 0 {
                continue;
            }
            let mut page_request = MultiPageRequest::default();
            let map_offset = base - self.base;
            let vmo_offset = self.object_offset + map_offset as u64;
            // grab the lock for the vmo
            let mut deferred = DeferredOps::new(object.cow_pages());
            let mut locked = object.cow_pages().lock();
            // Trim our range to the current VMO size. Our mapping might exceed the VMO in the case
            // where the VMO is resizable, and this should not be considered an error.
            let len = (len as u64).min(locked.size().saturating_sub(vmo_offset)) as usize;
            if len == 0 {
                break;
            }
            let mut coalescer = VmMappingCoalescer::<MAP_RANGE_MAX_PAGES>::new(
                self,
                base,
                mmu_flags,
                if ignore_existing {
                    ExistingEntryAction::Skip
                } else {
                    ExistingEntryAction::Error
                },
            );
            let writing = mmu_flags & ARCH_MMU_FLAG_PERM_WRITE != 0;
            let mut cursor = object.get_lookup_cursor_locked(&locked, vmo_offset, len as u64)?;
            // Do not consider pages touched when mapping in, if they are actually touched they will
            // get an accessed bit set in the hardware.
            cursor.disable_mark_accessed();
            let mut off = 0;
            while off < len {
                let page = if commit {
                    match cursor.require_owned_page(
                        object.cow_pages(),
                        &mut locked,
                        writing,
                        1,
                        &mut deferred,
                        &mut page_request,
                    ) {
                        Ok(result) => Some(result.page),
                        Err(status) => {
                            // There should never be commit + pager backed VMO and so we should
                            // never end up with a PageRequest needing to be waited on.
                            assert_ne!(status, Status::SHOULD_WAIT);
                            return Err(status);
                        }
                    }
                } else {
                    // Not committing so get a page if one exists.
                    let page = cursor.maybe_page(&mut locked, writing);
                    // Efficiently skip a run of absent pages.
                    if page.is_none() && off + PAGE_SIZE < len {
                        off += cursor.skip_missing_pages(&mut locked) as usize * PAGE_SIZE;
                    }
                    page
                };
                if let Some(page) = page {
                    coalescer.append(base + off, page.paddr())?;
                }
                off += PAGE_SIZE;
            }
            coalescer.flush()?;
        }
        Ok(())
    }

    /// Decommits [offset, offset + len) of the mapping.
    pub fn decommit_range(&self, offset: usize, len: usize) -> Result<(), Status> {
        let object = {
            let _guard = self.aspace.lock();
            let state = self.state.lock();
            ensure!(state.life == LifeCycleState::Alive, Status::BAD_STATE);
            state.object.clone().expect("alive mapping has an object")
        };
        ensure!(offset.checked_add(len).is_some_and(|end| end <= self.size), Status::OUT_OF_RANGE);
        object.decommit_range(self.object_offset + offset as u64, len as u64)
    }

    /// Resolves a fault at `va` by mapping the pages there, and up to `additional_pages` more.
    /// Returns the status and how many pages were mapped.
    pub(crate) fn page_fault(
        &self,
        va: usize,
        pf_flags: u32,
        additional_pages: usize,
        object: &Arc<VmObjectPaged>,
        page_request: &mut MultiPageRequest,
    ) -> (Result<(), Status>, usize) {
        debug_assert!(page::is_aligned(va));
        let mut deferred = DeferredOps::new(object.cow_pages());
        let mut locked = object.cow_pages().lock();
        if self.object_reset.load(Ordering::Relaxed) {
            return (Err(Status::UNAVAILABLE), 0);
        }
        self.page_fault_locked_object(
            va,
            pf_flags,
            additional_pages,
            object,
            &mut locked,
            &mut deferred,
            page_request,
        )
    }

    /// Completes an in progress fault, unmapping the range if the fault did not update it and an
    /// unmap was skipped.
    fn finish_currently_faulting(&self, mapping_updated: bool) {
        let mut state = self.state.lock();
        let currently_faulting = state.currently_faulting.take().expect("fault in progress");
        drop(state);
        if !mapping_updated && currently_faulting.state == CurrentlyFaultingState::UnmapSkipped {
            let (base, new_len) = self
                .object_range_to_vaddr_range(
                    currently_faulting.object_offset,
                    currently_faulting.len,
                )
                .expect("faulting range is mapped");
            assert_eq!(new_len as u64, currently_faulting.len);
            self.aspace
                .arch_aspace()
                .unmap(base, new_len / PAGE_SIZE, self.aspace.enlarge_arch_unmap())
                .expect("unmap");
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn page_fault_locked_object(
        &self,
        va: usize,
        pf_flags: u32,
        additional_pages: usize,
        object: &VmObjectPaged,
        locked: &mut VmCowPagesLocked,
        deferred: &mut DeferredOps,
        page_request: &mut MultiPageRequest,
    ) -> (Result<(), Status>, usize) {
        let vmo_offset = (va - self.base) as u64 + self.object_offset;

        // Need to look up the mmu flags for this virtual address, as well as how large a region
        // those flags are for so we can cap the extra mappings we create.
        let range = self.flags_range_at_addr(&self.state.lock(), va);

        // Build the mmu flags we need to have based on the page fault.
        let mut needed_mmu_flags = 0;
        if pf_flags & flag::USER != 0 {
            needed_mmu_flags |= ARCH_MMU_FLAG_PERM_USER;
        }
        let write = pf_flags & flag::WRITE != 0;
        if write {
            needed_mmu_flags |= ARCH_MMU_FLAG_PERM_WRITE;
        } else {
            needed_mmu_flags |= ARCH_MMU_FLAG_PERM_READ;
        }
        if pf_flags & flag::INSTRUCTION != 0 {
            needed_mmu_flags |= ARCH_MMU_FLAG_PERM_EXECUTE;
        }
        // Check that all the needed flags are present.
        if range.mmu_flags & needed_mmu_flags != needed_mmu_flags {
            return (Err(Status::ACCESS_DENIED), 0);
        }

        // Calculate the number of pages from va until the end of the protection range.
        let num_protection_range_pages = (range.region_top - va) / PAGE_SIZE;
        let vmo_size = locked.size();
        if vmo_offset >= vmo_size {
            return (Err(Status::OUT_OF_RANGE), 0);
        }
        // Calculate the maximum number of pages we can legally look at, i.e. are valid, in the vmo
        // taking into account the protection range, which is implicitly taking into account the
        // mapping size.
        let num_vmo_pages = ((vmo_size - vmo_offset) / PAGE_SIZE as u64) as usize;
        let num_valid_pages = num_protection_range_pages.min(num_vmo_pages);
        // Number of requested pages, trimmed to protection range & VMO.
        let num_required_pages = num_valid_pages.min(additional_pages + 1);
        debug_assert!(num_required_pages > 0);
        // Number of pages we're aiming to fault. If a range > 1 page is supplied, it is assumed
        // the user knows the appropriate range, so opportunistic pages will not be added.
        let num_pt_pages = ((va | (PAGE_TABLE_SPAN - 1)) + 1 - va) / PAGE_SIZE;
        let num_fault_pages = if additional_pages == 0 {
            PAGE_FAULT_MAX_OPTIMISTIC_PAGES.min(num_valid_pages).min(num_pt_pages)
        } else {
            num_required_pages
        };

        // Opportunistic pages are not considered in currently_faulting optimisation, as it is not
        // guaranteed the mappings will be updated.
        {
            let mut state = self.state.lock();
            debug_assert!(state.currently_faulting.is_none());
            state.currently_faulting = Some(CurrentlyFaulting {
                object_offset: vmo_offset,
                len: (num_required_pages * PAGE_SIZE) as u64,
                state: CurrentlyFaultingState::NoUnmapNeeded,
            });
        }
        let mut coalescer = VmMappingCoalescer::<PAGE_FAULT_MAX_OPTIMISTIC_PAGES>::new(
            self,
            va,
            range.mmu_flags,
            ExistingEntryAction::Upgrade,
        );
        let result = (|| {
            // fault in or grab existing pages.
            let mut cursor = object.get_lookup_cursor_locked(
                locked,
                vmo_offset,
                (num_fault_pages * PAGE_SIZE) as u64,
            )?;
            // Do not consider pages touched when mapping in, if they are actually touched they will
            // get an accessed bit set in the hardware.
            cursor.disable_mark_accessed();

            // Fault requested pages.
            for i in 0..num_required_pages {
                let offset = i * PAGE_SIZE;
                let mut curr_mmu_flags = range.mmu_flags;
                let num_curr_pages = (num_required_pages - i) as u64;
                let result = cursor.require_page(
                    object.cow_pages(),
                    locked,
                    write,
                    num_curr_pages,
                    deferred,
                    page_request,
                );
                let RequireResult { page, writable } = match result {
                    Ok(result) => result,
                    Err(status) => {
                        // Map any pages already looked up.
                        let _ = coalescer.flush();
                        return Err(status);
                    }
                };
                debug_assert!(!write || writable);
                // We looked up in order to write. Mark as modified. Only need to do this once.
                if write && offset == 0 {
                    object.cow_pages().mark_modified_locked(locked);
                }
                // If we read faulted, and lookup didn't say that this is always writable, then we
                // map or modify the page without any write permissions. This ensures we will fault
                // again if a write is attempted so we can potentially replace this page with a
                // copy or a new one, or update the page's dirty state.
                if !write && !writable {
                    // we read faulted, so only map with read permissions
                    curr_mmu_flags &= !ARCH_MMU_FLAG_PERM_WRITE;
                }
                if let Err(status) =
                    coalescer.append_or_adjust_mapping(va + offset, page.paddr(), curr_mmu_flags)
                {
                    // Flush any existing pages in the coalescer.
                    let _ = coalescer.flush();
                    return Err(status);
                }
            }

            // Fault opportunistic pages. If a range is supplied, it is assumed the user knows the
            // appropriate range, so opportunistic pages will not be fault.
            if additional_pages == 0 {
                // Check how much space the coalescer has for faulting additional pages.
                let extra_pages =
                    coalescer.extra_page_capacity_from(va + PAGE_SIZE).min(num_fault_pages - 1);
                // Acquire any additional pages, but only if they already exist as the user has not
                // attempted to use these pages yet.
                if extra_pages > 0 {
                    let writable = coalescer.mmu_flags & ARCH_MMU_FLAG_PERM_WRITE != 0;
                    let count = coalescer.count;
                    let num_extra_pages = cursor.if_exist_pages(
                        locked,
                        writable,
                        extra_pages as u64,
                        &mut coalescer.phys[count..count + extra_pages],
                    ) as usize;
                    coalescer.count += num_extra_pages;
                }
            }
            coalescer.flush()
        })();
        // If the mapping has been successfully updated by us, the faulting helper knows not to
        // unmap the range instead.
        self.finish_currently_faulting(result.is_ok());
        (result, coalescer.total_mapped)
    }
}

impl RangeChangeListener for VmMapping {
    fn range_change_update(&self, range: VmCowRange, op: RangeChangeOp) {
        match op {
            RangeChangeOp::Unmap => {
                self.aspace_unmap_locked_object(range.offset, range.len, UnmapOptions::None)
            }
            RangeChangeOp::UnmapZeroPage => self.aspace_unmap_locked_object(
                range.offset,
                range.len,
                UnmapOptions::OnlyHasZeroPages,
            ),
            RangeChangeOp::UnmapAndHarvest => {
                self.aspace_unmap_locked_object(range.offset, range.len, UnmapOptions::Harvest)
            }
            RangeChangeOp::RemoveWrite => {
                self.aspace_remove_write_locked_object(range.offset, range.len)
            }
            // Pinned pages are never unmapped here, so there is nothing to check.
            RangeChangeOp::DebugUnpin => {}
        }
    }
}
