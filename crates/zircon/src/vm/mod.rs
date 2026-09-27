//! The virtual memory subsystem, ported from `zircon/kernel/vm`.

pub mod anonymous_page_request;
pub mod page;
pub mod page_source;
pub mod page_state;
pub mod physmap;
pub mod pmm;
pub mod vm_constants;
pub mod vm_page_list;
mod vm_page_list_btree;
#[cfg(test)]
mod unittests;
