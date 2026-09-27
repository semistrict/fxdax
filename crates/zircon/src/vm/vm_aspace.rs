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
//!   their base is wherever the host reserved them.
//! - No kernel address space, ASLR, vDSO, memory priorities, attribution or page table harvesting.

use crate::arch::host::mmu::HostArchVmAspace;
use crate::vm::arch_vm_aspace::{
    ARCH_UNMAP_OPTION_ENLARGE, ARCH_UNMAP_OPTION_NONE, ArchUnmapOptions, ArchVmAspace,
};
use crate::vm::fault::flag;
use crate::vm::page_source::MultiPageRequest;
use crate::vm::vm_address_region::{
    VMAR_FLAG_CAN_MAP_SPECIFIC, VmAddressRegion, VmAddressRegionOrMapping,
};
use crate::vm::vm_mapping::VmMapping;
use parking_lot::{Mutex, MutexGuard};
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

/// The page tables of an aspace.
enum Arch {
    Host(HostArchVmAspace),
}

impl Arch {
    fn get(&self) -> &dyn ArchVmAspace {
        match self {
            Arch::Host(host) => host,
        }
    }
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
    arch_aspace: Arch,
    lock: Mutex<AspaceLocked>,
}

/// Aspaces mapped into the host process, for finding the aspace a host fault is in.
static HOST_ASPACES: Mutex<Vec<Weak<VmAspace>>> = Mutex::new(Vec::new());

impl VmAspace {
    /// Creates a user address space of `size` bytes mapped into the host process.
    pub fn create_host(size: usize, name: &str) -> Option<Arc<VmAspace>> {
        let arch = HostArchVmAspace::new(size);
        let aspace = Arc::new(VmAspace {
            base: arch.base(),
            size,
            type_: Type::User,
            name: Mutex::new(Self::make_name(name)),
            arch_aspace: Arch::Host(arch),
            lock: Mutex::new(AspaceLocked {
                aspace_destroyed: false,
                root_vmar: None,
                last_fault: None,
            }),
        });
        aspace.init();
        crate::arch::host::exceptions::install();
        let mut aspaces = HOST_ASPACES.lock();
        aspaces.retain(|a| a.strong_count() > 0);
        aspaces.push(Arc::downgrade(&aspace));
        Some(aspace)
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

    /// Returns the host aspace containing host address `addr`.
    pub fn find_host_aspace(addr: usize) -> Option<Arc<VmAspace>> {
        HOST_ASPACES
            .lock()
            .iter()
            .filter_map(Weak::upgrade)
            .find(|aspace| addr >= aspace.base && addr - aspace.base < aspace.size)
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
        self.arch_aspace.get()
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

    pub fn is_destroyed(&self) -> bool {
        self.lock.lock().aspace_destroyed
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
        mut va: usize,
        mut flags: u32,
        mut additional_pages: usize,
    ) -> Result<(), Status> {
        debug_assert_eq!(flags & flag::ACCESS, 0);
        if self.type_ == Type::GuestPhysical {
            flags &= !flag::USER;
            flags |= flag::GUEST;
        }
        let mut page_request = MultiPageRequest::default();
        loop {
            let (object, mapping) = {
                let mut guard = self.lock.lock();
                debug_assert!(!guard.aspace_destroyed);
                // First check if we're faulting on the same mapping as last time to short-circuit
                // the vmar walk.
                let found = guard.last_fault.as_ref().is_some_and(|m| m.is_in_range(va, 1));
                if !found {
                    let root_vmar = guard.root_vmar.clone().ok_or(Status::NOT_FOUND)?;
                    // Stash the mapping we found as the most recent fault.
                    guard.last_fault = root_vmar.find_mapping_locked(&guard, va);
                }
                let mapping = guard.last_fault.clone().ok_or(Status::NOT_FOUND)?;
                // As we just found this mapping in the VMAR tree we know it's alive, and so has
                // an object.
                (mapping.vmo_locked(&guard).expect("alive mapping has an object"), mapping)
            };
            let (status, mapped) =
                mapping.page_fault(va, flags, additional_pages, &object, &mut page_request);
            match status {
                Err(Status::SHOULD_WAIT) => {
                    page_request.wait(flags & flag::USER != 0)?;
                    // Before retrying the page fault, take into account how many pages got mapped
                    // on the previous attempt (if any).
                    if mapped > 0 {
                        va += PAGE_SIZE * mapped;
                        debug_assert!(mapped <= additional_pages);
                        additional_pages -= mapped;
                    }
                }
                // Need to retry the fault if our previous attempt at resolving the fault raced with
                // the mapping being deleted.
                Err(Status::UNAVAILABLE) => {}
                status => return status,
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

    /// Handles a fault on a page whose accessed flag was harvested.
    pub fn accessed_fault(&self, va: usize) -> Result<(), Status> {
        // There are no permissions etc associated with accessed bits so we can skip any vmar
        // walking and just let the hardware aspace walk for the virtual address.
        self.arch_aspace().mark_accessed(page::round_down(va), 1)
    }

    /// Handles a fault taken by the host process at `va`, as the architecture's exception handler
    /// does for a hardware fault.
    pub fn host_fault(&self, va: usize, write: bool) -> Result<(), Status> {
        let Arch::Host(host) = &self.arch_aspace;
        if host.is_accessed_fault(va) {
            return self.page_fault(va, flag::ACCESS);
        }
        let mut flags = flag::USER | flag::HW_FAULT;
        if write {
            flags |= flag::WRITE;
        }
        if !host.is_mapped(va) {
            flags |= flag::NOT_PRESENT;
        }
        self.page_fault(va, flags)
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
