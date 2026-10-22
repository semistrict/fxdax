// Copyright 2016 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Address spaces.
//!
//! Ported from `zircon/kernel/vm/vm_aspace.cc` and `zircon/kernel/vm/include/vm/vm_aspace.h`.
//!
//! An address space is a tree of VMARs and mappings rooted at its root VMAR, together with the
//! page tables (`ArchVmAspace`) that hold the mappings' pages.
//!
//! Deviations:
//! - One lock protects the region tree, standing in for both the aspace lock and the region lock.
//!   Each region's own state has a lock nested inside it; see `vm_address_region`.
//! - User address spaces are ranges of the host process's address space (`arch::host::mmu`), so
//!   their base is wherever the host reserved them. Guest physical address spaces are windows,
//!   whose addresses start at 0: of the host process's address space too, or, given a
//!   hypervisor's second stage, of that alone.
//! - No kernel address space, ASLR, vDSO, memory priorities, attribution or page table
//!   reclamation. The address space list is the list of address spaces in host memory, which the
//!   host process's faults are in.

use crate::arch::host::mmu::{HostArchVmAspace, SecondStage};
use crate::vm::arch_vm_aspace::{
    ARCH_UNMAP_OPTION_ENLARGE, ARCH_UNMAP_OPTION_NONE, ArchUnmapOptions, ArchVmAspace,
    NonTerminalAction, TerminalAction,
};
use crate::vm::fault::flag;
use crate::vm::page_source::MultiPageRequest;
use crate::vm::vm_address_region::{
    VMAR_FLAG_CAN_MAP_SPECIFIC, VmAddressRegion, VmAddressRegionOrMapping,
};
use crate::vm::vm_mapping::VmMapping;
use parking_lot::{Mutex, MutexGuard};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Weak};
use zx_status::Status;
use zx_types::ZX_MAX_NAME_LEN;

const PAGE_SIZE: usize = page::SIZE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    User,
    Kernel,
    LowKernel,
    GuestPhysical,
}

/// State protected by the aspace lock.
pub struct AspaceLocked {
    aspace_destroyed: bool,
    root_vmar: Option<Arc<VmAddressRegion>>,
    /// The mapping of the most recent fault, to short-circuit the vmar walk.
    last_fault: Option<Arc<VmMapping>>,
}

impl AspaceLocked {
    pub(crate) fn clear_last_fault(&mut self, mapping: &VmMapping) {
        if self.last_fault.as_ref().is_some_and(|m| std::ptr::eq(Arc::as_ptr(m), mapping)) {
            self.last_fault = None;
        }
    }
}

/// The lock held over changes to an aspace's regions.
pub type AspaceGuard<'a> = MutexGuard<'a, AspaceLocked>;

pub struct VmAspace {
    base: usize,
    size: usize,
    type_: Type,
    name: Mutex<[u8; ZX_MAX_NAME_LEN]>,
    arch_aspace: HostArchVmAspace,
    lock: Mutex<AspaceLocked>,
    /// How many page table harvests have happened since the aspace was last active, guarded by
    /// the aspace list lock.
    pt_harvest_since_active: AtomicU32,
}

/// The aspaces in host memory (`host_base`): the address space list, for finding the aspace a
/// host fault is in. An aspace whose pages are mapped in a hypervisor's second stage is not in
/// it, as the host process never accesses its pages.
static HOST_ASPACES: Mutex<Vec<Weak<VmAspace>>> = Mutex::new(Vec::new());

impl VmAspace {
    /// Creates a user address space of `size` bytes mapped into the host process.
    pub fn create_host(size: usize, name: &str) -> Option<Arc<VmAspace>> {
        Some(Self::create_in(Type::User, HostArchVmAspace::new(size), name))
    }

    /// Creates a guest physical address space of `size` bytes, whose pages are mapped in the
    /// hypervisor's `second_stage`, or, without one, in a window of the host process's address
    /// space (see `host_base`).
    pub fn create_guest_physical(
        size: usize,
        name: &str,
        second_stage: Option<Arc<dyn SecondStage>>,
    ) -> Option<Arc<VmAspace>> {
        let arch = HostArchVmAspace::new_window(0, size, second_stage);
        Some(Self::create_in(Type::GuestPhysical, arch, name))
    }

    fn create_in(type_: Type, arch: HostArchVmAspace, name: &str) -> Arc<VmAspace> {
        let in_host_memory = arch.host_base().is_some();
        let aspace = Arc::new(VmAspace {
            base: arch.base(),
            size: arch.size(),
            type_,
            name: Mutex::new(Self::make_name(name)),
            arch_aspace: arch,
            lock: Mutex::new(AspaceLocked {
                aspace_destroyed: false,
                root_vmar: None,
                last_fault: None,
            }),
            pt_harvest_since_active: AtomicU32::new(0),
        });
        aspace.init();
        crate::arch::host::exceptions::install();
        if in_host_memory {
            let mut aspaces = HOST_ASPACES.lock();
            aspaces.retain(|a| a.strong_count() > 0);
            aspaces.push(Arc::downgrade(&aspace));
        }
        aspace
    }

    /// The host address the aspace's memory starts at, unless its pages are mapped in a
    /// hypervisor's second stage.
    pub fn host_base(&self) -> Option<usize> {
        self.arch_aspace.host_base()
    }

    /// Returns the address in the aspace of host address `host_addr`, which must be in the
    /// aspace's memory.
    pub fn vaddr_of_host(&self, host_addr: usize) -> usize {
        self.arch_aspace.vaddr_of_host(host_addr)
    }

    fn make_name(name: &str) -> [u8; ZX_MAX_NAME_LEN] {
        let mut stored = [0; ZX_MAX_NAME_LEN];
        let len = name.len().min(ZX_MAX_NAME_LEN - 1);
        stored[..len].copy_from_slice(&name.as_bytes()[..len]);
        stored
    }

    fn init(self: &Arc<Self>) {
        let mut guard = self.lock.lock();
        if guard.root_vmar.is_none() {
            guard.root_vmar =
                Some(VmAddressRegion::create_root_locked(self, VMAR_FLAG_CAN_MAP_SPECIFIC));
        }
    }

    /// Harvests the accessed information of all the user address spaces into the page queues.
    pub fn harvest_all_user_accessed_bits(
        non_terminal_action: NonTerminalAction,
        terminal_action: TerminalAction,
    ) {
        let aspaces = HOST_ASPACES.lock();

        for a in aspaces.iter().filter_map(Weak::upgrade) {
            if a.is_user() && a.size() > 0 {
                // The arch_aspace is only destroyed in the VmAspace destructor. Even if the actual
                // VmAspace has been destroyed, it is still completely safe to walk to the hardware
                // page tables, there just will not be anything there.
                // First we always check accessed_since_last_check (even if we could separately
                // infer that we have to do a harvest) in order to clear the state from it.
                let mut harvest = true;
                // `pt_harvest_since_active` is only accessed with the aspace list lock held.
                let pt_harvest_since_active = &a.pt_harvest_since_active;
                if a.arch_aspace().accessed_since_last_check(
                    terminal_action == TerminalAction::UpdateAgeAndHarvest,
                ) {
                    // The aspace has been accessed since some kind of harvest last happened, so we
                    // must do a new one. Reset our counter of how many pt reclamations we've done
                    // based on what kind scan this is.
                    if non_terminal_action == NonTerminalAction::FreeUnaccessed {
                        // This is set to one since we haven't yet performed the harvest, and so if
                        // next time the call to accessed_since_last_check() returns false, then it
                        // will be true that one harvest has been done since last active.
                        // Alternative if next time accessed_since_last_check() returns true, then
                        // we'll just re-set this back to 1 again.
                        pt_harvest_since_active.store(1, Ordering::Relaxed);
                    } else {
                        pt_harvest_since_active.store(0, Ordering::Relaxed);
                    }
                } else if non_terminal_action == NonTerminalAction::FreeUnaccessed
                    && pt_harvest_since_active.load(Ordering::Relaxed) < 2
                {
                    // The aspace hasn't been active, but we haven't yet performed two successive
                    // pt reclamations. Since the first pt reclamation only removes accessed
                    // information, the second is needed to actually do the reclamation.
                    pt_harvest_since_active.fetch_add(1, Ordering::Relaxed);
                } else {
                    // Either this is not a request to harvest pt information, or enough pt
                    // harvesting has been done, and so we can skip as the aspace should now be at
                    // a fixed point with no new information.
                    harvest = false;
                }
                if harvest {
                    let result = a.arch_aspace().harvest_accessed(
                        a.base(),
                        a.size() / PAGE_SIZE,
                        non_terminal_action,
                        terminal_action,
                    );
                    debug_assert!(result.is_ok());
                }
            }
        }
    }

    /// Returns the host aspace whose memory contains host address `addr`.
    pub fn find_host_aspace(addr: usize) -> Option<Arc<VmAspace>> {
        HOST_ASPACES.lock().iter().filter_map(Weak::upgrade).find(|aspace| {
            (aspace.host_base())
                .is_some_and(|host_base| addr >= host_base && addr - host_base < aspace.size)
        })
    }

    pub fn rename(&self, name: &str) {
        *self.name.lock() = Self::make_name(name);
    }

    pub fn base(&self) -> usize {
        self.base
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn name(&self) -> [u8; ZX_MAX_NAME_LEN] {
        *self.name.lock()
    }

    pub fn arch_aspace(&self) -> &dyn ArchVmAspace {
        &self.arch_aspace
    }

    pub fn is_user(&self) -> bool {
        self.type_ == Type::User
    }

    pub fn is_guest_physical(&self) -> bool {
        self.type_ == Type::GuestPhysical
    }

    fn can_enlarge_arch_unmap(&self) -> bool {
        self.is_user() || self.is_guest_physical()
    }

    pub(crate) fn enlarge_arch_unmap(&self) -> ArchUnmapOptions {
        if self.can_enlarge_arch_unmap() {
            ARCH_UNMAP_OPTION_ENLARGE
        } else {
            ARCH_UNMAP_OPTION_NONE
        }
    }

    /// Acquires the aspace lock, which protects the region tree.
    pub(crate) fn lock(&self) -> AspaceGuard<'_> {
        self.lock.lock()
    }

    pub fn root_vmar(&self) -> Option<Arc<VmAddressRegion>> {
        self.lock.lock().root_vmar.clone()
    }

    pub(crate) fn root_vmar_locked(&self, guard: &AspaceGuard<'_>) -> Option<Arc<VmAddressRegion>> {
        guard.root_vmar.clone()
    }

    /// Destroys all the regions of the aspace.
    pub fn destroy(&self) -> Result<(), Status> {
        let mut guard = self.lock.lock();
        // tear down and free all of the regions in our address space
        if let Some(root_vmar) = guard.root_vmar.clone() {
            match root_vmar.destroy_locked(&mut guard) {
                Ok(()) | Err(Status::BAD_STATE) => {}
                Err(status) => return Err(status),
            }
        }
        guard.aspace_destroyed = true;
        guard.root_vmar = None;
        guard.last_fault = None;
        // Now that we've removed all mappings we can put the arch aspace into a sort of read-only
        // mode.
        self.arch_aspace().disable_updates();
        Ok(())
    }

    /// Returns the innermost region containing `va`.
    pub fn find_region(&self, va: usize) -> Option<VmAddressRegionOrMapping> {
        let mut vmar = self.root_vmar()?;
        loop {
            match vmar.find_region(va) {
                None => return Some(VmAddressRegionOrMapping::Vmar(vmar)),
                Some(VmAddressRegionOrMapping::Vmar(next)) => vmar = next,
                Some(mapping) => return Some(mapping),
            }
        }
    }

    /// Handles a page fault at `va` described by `flags`.
    pub fn page_fault(&self, va: usize, flags: u32) -> Result<(), Status> {
        // If the fault was actually an access fault, handle that and return.
        if flags & flag::ACCESS != 0 {
            // Assert that the translation bit is not set.
            debug_assert_eq!(flags & flag::NOT_PRESENT, 0);
            return self.accessed_fault(va);
        }
        // With the original va logged in the traces can now convert to a page aligned address
        // suitable for passing to PageFaultLocked.
        self.page_fault_internal(page::round_down(va), flags, 0)
    }

    fn page_fault_internal(
        &self,
        va: usize,
        flags: u32,
        additional_pages: usize,
    ) -> Result<(), Status> {
        let mut fault = PageFault::new(self, va, flags, additional_pages);
        loop {
            match self.page_fault_step(&mut fault) {
                PageFaultStep::Done(result) => return result,
                PageFaultStep::Wait => {
                    let user = fault.user();
                    let waited = fault.page_request.wait(user);
                    fault.waited(waited)?
                }
            }
        }
    }

    /// Resolves a fault, as `page_fault_internal` does, as a future that waits on page requests
    /// rather than blocking the calling thread.
    async fn page_fault_internal_async(
        &self,
        va: usize,
        flags: u32,
        additional_pages: usize,
    ) -> Result<(), Status> {
        let mut fault = PageFault::new(self, va, flags, additional_pages);
        loop {
            match self.page_fault_step(&mut fault) {
                PageFaultStep::Done(result) => return result,
                PageFaultStep::Wait => {
                    let user = fault.user();
                    let waited = fault.page_request.wait_async(user).await;
                    fault.waited(waited)?
                }
            }
        }
    }

    /// Attempts the fault `fault` until it is resolved or fails, or it must wait on its page
    /// request.
    fn page_fault_step(&self, fault: &mut PageFault) -> PageFaultStep {
        loop {
            let (object, mapping) = {
                let mut guard = self.lock.lock();
                debug_assert!(!guard.aspace_destroyed);
                // First check if we're faulting on the same mapping as last time to short-circuit
                // the vmar walk.
                let found = guard.last_fault.as_ref().is_some_and(|m| m.is_in_range(fault.va, 1));
                if !found {
                    let Some(root_vmar) = guard.root_vmar.clone() else {
                        return PageFaultStep::Done(Err(Status::NOT_FOUND));
                    };
                    // Stash the mapping we found as the most recent fault.
                    guard.last_fault = root_vmar.find_mapping_locked(&guard, fault.va);
                }
                let Some(mapping) = guard.last_fault.clone() else {
                    return PageFaultStep::Done(Err(Status::NOT_FOUND));
                };
                // As we just found this mapping in the VMAR tree we know it's alive, and so has
                // an object.
                (mapping.vmo_locked(&guard).expect("alive mapping has an object"), mapping)
            };
            let (status, mapped) = mapping.page_fault(
                fault.va,
                fault.flags,
                fault.additional_pages,
                &object,
                &mut fault.page_request,
            );
            match status {
                Err(Status::SHOULD_WAIT) => {
                    fault.mapped = mapped;
                    return PageFaultStep::Wait;
                }
                // Need to retry the fault if our previous attempt at resolving the fault raced with
                // the mapping being deleted.
                Err(Status::UNAVAILABLE) => {}
                status => return PageFaultStep::Done(status),
            }
        }
    }

    /// Faults in the page at `va` as if accessed, without a hardware fault.
    pub fn soft_fault(&self, va: usize, flags: u32) -> Result<(), Status> {
        // With the current implementation we can just reuse the internal PageFault mechanism.
        self.page_fault(va, flags | flag::SW_FAULT)
    }

    /// Faults in the pages of [va, va + len) as if accessed.
    pub fn soft_fault_in_range(&self, va: usize, flags: u32, len: usize) -> Result<(), Status> {
        // If the fault was actually an access fault, handle that and return.
        if flags & flag::ACCESS != 0 {
            debug_assert_eq!(flags & flag::NOT_PRESENT, 0);
            return self.accessed_fault(va);
        }
        debug_assert!(len > 0);
        let range_end = va.checked_add(len - 1).ok_or(Status::OUT_OF_RANGE)?;
        let va_page_base = page::round_down(va);
        let last_page_base = page::round_down(range_end);
        let extra_pages = (last_page_base - va_page_base) / PAGE_SIZE;
        self.page_fault_internal(va_page_base, flags | flag::SW_FAULT, extra_pages)
    }

    /// Faults in the pages of [va, va + len) as if accessed, as `soft_fault_in_range` does, as a
    /// future that waits on page requests rather than blocking the calling thread.
    pub async fn soft_fault_in_range_async(
        &self,
        va: usize,
        flags: u32,
        len: usize,
    ) -> Result<(), Status> {
        // If the fault was actually an access fault, handle that and return.
        if flags & flag::ACCESS != 0 {
            debug_assert_eq!(flags & flag::NOT_PRESENT, 0);
            return self.accessed_fault(va);
        }
        debug_assert!(len > 0);
        let range_end = va.checked_add(len - 1).ok_or(Status::OUT_OF_RANGE)?;
        let va_page_base = page::round_down(va);
        let last_page_base = page::round_down(range_end);
        let extra_pages = (last_page_base - va_page_base) / PAGE_SIZE;
        self.page_fault_internal_async(va_page_base, flags | flag::SW_FAULT, extra_pages).await
    }

    /// Handles a fault on a page whose accessed flag was harvested.
    pub fn accessed_fault(&self, va: usize) -> Result<(), Status> {
        // There are no permissions etc associated with accessed bits so we can skip any vmar
        // walking and just let the hardware aspace walk for the virtual address.
        self.arch_aspace().mark_accessed(page::round_down(va), 1)
    }

    /// Returns the flags of a page fault the host process took at `va`, other than whether it was
    /// a user access, as the architecture's exception handler decodes them from the fault's
    /// syndrome.
    pub fn host_fault_flags(&self, host_addr: usize, write: bool) -> u32 {
        let host = &self.arch_aspace;
        let va = host.vaddr_of_host(host_addr);
        let mut flags = 0;
        if write {
            flags |= flag::WRITE;
        }
        if host.is_accessed_fault(va) {
            flags |= flag::ACCESS;
        } else if !host.is_mapped(va) {
            flags |= flag::NOT_PRESENT;
        }
        flags
    }
}

impl Drop for VmAspace {
    fn drop(&mut self) {
        // we have to have already been destroyed before freeing
        debug_assert!(self.lock.get_mut().aspace_destroyed);
        let status = self.arch_aspace().destroy();
        debug_assert!(status.is_ok());
    }
}

/// A page fault being resolved, which `page_fault_step` attempts.
struct PageFault {
    va: usize,
    flags: u32,
    additional_pages: usize,
    page_request: MultiPageRequest,
    /// How many pages the last attempt mapped before it had to wait.
    mapped: usize,
}

impl PageFault {
    fn new(aspace: &VmAspace, va: usize, mut flags: u32, additional_pages: usize) -> Self {
        debug_assert_eq!(flags & flag::ACCESS, 0);
        if aspace.type_ == Type::GuestPhysical {
            flags &= !flag::USER;
            flags |= flag::GUEST;
        }
        Self { va, flags, additional_pages, page_request: MultiPageRequest::default(), mapped: 0 }
    }

    /// Whether the fault is a user's, whose waits may be suspended.
    fn user(&self) -> bool {
        self.flags & flag::USER != 0
    }

    /// Accounts for the wait on the page request, whose result is `waited`, before retrying.
    fn waited(&mut self, waited: Result<(), Status>) -> Result<(), Status> {
        waited?;
        // Before retrying the page fault, take into account how many pages got mapped on the
        // previous attempt (if any).
        if self.mapped > 0 {
            self.va += PAGE_SIZE * self.mapped;
            debug_assert!(self.mapped <= self.additional_pages);
            self.additional_pages -= self.mapped;
            self.mapped = 0;
        }
        Ok(())
    }
}

/// What a fault needs once `page_fault_step` returns.
enum PageFaultStep {
    /// Nothing: it is resolved, or failed.
    Done(Result<(), Status>),
    /// A wait on its page request.
    Wait,
}
