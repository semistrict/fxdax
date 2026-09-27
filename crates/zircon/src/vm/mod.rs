//! The virtual memory subsystem, ported from Zircon's `kernel/vm`.

pub mod anonymous_page_request;
pub mod arch_vm_aspace;
pub mod evictor;
pub mod fault;
pub mod page;
pub mod page_queues;
pub mod page_source;
pub mod page_state;
pub mod physmap;
pub mod pmm;
pub mod scanner;
#[cfg(test)]
mod unittests;
pub mod vm_address_region;
pub mod vm_aspace;
pub mod vm_constants;
pub mod vm_cow_pages;
pub mod vm_mapping;
pub mod vm_object_paged;
pub mod vm_page_list;
mod vm_page_list_btree;
pub mod vmm;

use crate::vm::page::VmPagePtr;
use crate::vm::page_state::VmPageState;
use std::sync::OnceLock;

/// Returns the page of zeroes that is mapped wherever content is known to be zero, allocating it
/// on first use as `vm_init` does.
pub fn vm_get_zero_page() -> VmPagePtr {
    static ZERO_PAGE: OnceLock<VmPagePtr> = OnceLock::new();
    *ZERO_PAGE.get_or_init(|| {
        let (page, _) = pmm::alloc_page(0).expect("allocate the zero page");
        // consider the zero page a wired page part of the kernel.
        // SAFETY: The page was just allocated.
        unsafe { page.set_state(VmPageState::Wired) };
        vm_cow_pages::zero_page(page);
        page
    })
}
