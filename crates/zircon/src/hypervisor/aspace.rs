// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! A guest's physical address space, into which VMOs are mapped as the guest's memory.
//!
//! Ported from `zircon/kernel/hypervisor/aspace.cc` and
//! `zircon/kernel/hypervisor/include/hypervisor/aspace.h`.
//!
//! The address space is a window of the host process's address space (`VmAspace::host_base`),
//! which the VMM gives the guest as memory, and whose pages are mapped as the guest faults on
//! them.
//!
//! Deviations:
//! - The address space's size is given, rather than the architecture's.
//! - The hypervisor's second stage translation is given (`create_with_second_stage`), as the host
//!   MMU makes the address space's mappings to it (see `arch::host::mmu`).
//! - A fault maps the page with the access's permission, rather than the mapping's maximum, so
//!   that reading a page does not dirty it, as dirty tracking of guest mappings needs.
//! - No interrupt controller mappings or guest pointers, as there are no physical VMOs or kernel
//!   address space.
//! - A fault may be resolved by a future (`page_fault_async`), which waits on page requests rather
//!   than blocking its thread (see `kernel::wakers`).

pub use crate::arch::host::mmu::SecondStage;
use crate::object::handle::KernelHandle;
use crate::object::vm_address_region_dispatcher::VmAddressRegionDispatcher;
use crate::vm::arch_vm_aspace::ArchVmAspace;
use crate::vm::fault::flag;
use crate::vm::page_source::MultiPageRequest;
use crate::vm::vm_address_region::{VmAddressRegion, VmAddressRegionOpChildren};
use crate::vm::vm_aspace::{AspaceGuard, VmAspace};
use crate::vm::vm_mapping::VmMapping;
use std::sync::Arc;
use zx_status::Status;
use zx_types::{zx_gpaddr_t, zx_rights_t};

pub struct GuestPhysicalAspace {
    physical_aspace: Arc<VmAspace>,
}

impl GuestPhysicalAspace {
    pub fn create(size: usize) -> Result<Self, Status> {
        Self::create_in(size, None)
    }

    /// Creates an address space whose mappings are also made to the hypervisor's `second_stage`.
    pub fn create_with_second_stage(
        size: usize,
        second_stage: Arc<dyn SecondStage>,
    ) -> Result<Self, Status> {
        Self::create_in(size, Some(second_stage))
    }

    fn create_in(size: usize, second_stage: Option<Arc<dyn SecondStage>>) -> Result<Self, Status> {
        let physical_aspace = VmAspace::create_guest_physical(size, "guest_physical", second_stage)
            .ok_or(Status::NO_MEMORY)?;
        Ok(Self { physical_aspace })
    }

    pub fn size(&self) -> usize {
        self.physical_aspace.size()
    }

    pub fn arch_aspace(&self) -> &dyn ArchVmAspace {
        self.physical_aspace.arch_aspace()
    }

    /// The host address of the guest's physical memory, which the VMM gives the guest.
    pub fn host_base(&self) -> usize {
        self.physical_aspace.host_base()
    }

    pub fn root_vmar(&self) -> Arc<VmAddressRegion> {
        self.physical_aspace.root_vmar().expect("guest physical aspace has a root vmar")
    }

    /// Creates a VMAR object of the root VMAR, for mapping VMOs into the guest's physical memory
    /// with the VMAR syscalls, as `zx_guest_create` does.
    pub fn create_vmar_dispatcher(
        &self,
    ) -> Result<(KernelHandle<VmAddressRegionDispatcher>, zx_rights_t), Status> {
        VmAddressRegionDispatcher::create(self.root_vmar(), 0)
    }

    pub fn is_mapped(&self, guest_paddr: zx_gpaddr_t) -> bool {
        let guard = self.physical_aspace.lock();
        self.find_mapping(&guard, guest_paddr).is_some()
    }

    pub fn unmap_range(&self, guest_paddr: zx_gpaddr_t, len: usize) -> Result<(), Status> {
        // Walk from the root down through any child VMARs for this range so we can do an unmap.
        let mut vmar = self.root_vmar();
        loop {
            match vmar.find_region(guest_paddr) {
                Some(next) if !next.is_mapping() => {
                    vmar = next.as_vm_address_region().unwrap().clone();
                }
                _ => break,
            }
        }
        vmar.unmap(guest_paddr, len, VmAddressRegionOpChildren::No)
    }

    /// Resolves the guest's `write` or read fault at `guest_paddr`.
    pub fn page_fault(&self, guest_paddr: zx_gpaddr_t, write: bool) -> Result<(), Status> {
        let mut page_request = MultiPageRequest::default();
        let guest_paddr = page::round_down(guest_paddr);
        while self.page_fault_attempt(guest_paddr, write, &mut page_request)? {
            page_request.wait(true)?;
        }
        Ok(())
    }

    /// Resolves the guest's `write` or read fault at `guest_paddr`, as `page_fault` does, as a
    /// future that waits on page requests rather than blocking the calling thread.
    pub async fn page_fault_async(
        &self,
        guest_paddr: zx_gpaddr_t,
        write: bool,
    ) -> Result<(), Status> {
        let mut page_request = MultiPageRequest::default();
        let guest_paddr = page::round_down(guest_paddr);
        while self.page_fault_attempt(guest_paddr, write, &mut page_request)? {
            page_request.wait_async(true).await?;
        }
        Ok(())
    }

    /// Attempts to resolve the fault at the page `guest_paddr`, returning whether it must wait on
    /// `page_request` and then be attempted again.
    fn page_fault_attempt(
        &self,
        guest_paddr: zx_gpaddr_t,
        write: bool,
        page_request: &mut MultiPageRequest,
    ) -> Result<bool, Status> {
        loop {
            let (object, mapping, pf_flags) = {
                let guard = self.physical_aspace.lock();
                let mapping = self.find_mapping(&guard, guest_paddr).ok_or(Status::NOT_FOUND)?;
                let mut pf_flags = flag::GUEST | flag::HW_FAULT;
                if write {
                    pf_flags |= flag::WRITE;
                }
                // As we just found this mapping in the VMAR tree we know it's alive, and so has an
                // object.
                let object = mapping.vmo_locked(&guard).expect("alive mapping has an object");
                (object, mapping, pf_flags)
            };
            let (status, _) = mapping.page_fault(guest_paddr, pf_flags, 0, &object, page_request);

            match status {
                Err(Status::SHOULD_WAIT) => return Ok(true),
                // Retry if the fault raced with the mapping being deleted.
                Err(Status::UNAVAILABLE) => {}
                status => return status.map(|()| false),
            }
        }
    }

    fn find_mapping(&self, guard: &AspaceGuard<'_>, guest_paddr: usize) -> Option<Arc<VmMapping>> {
        self.physical_aspace.root_vmar_locked(guard)?.find_mapping_locked(guard, guest_paddr)
    }
}

impl Drop for GuestPhysicalAspace {
    fn drop(&mut self) {
        // VmAspace maintains a circular reference with it's root VMAR. We need to destroy the
        // VmAspace in order to break that reference and allow the VmAspace to be destructed.
        let status = self.physical_aspace.destroy();
        debug_assert!(status.is_ok());
    }
}
