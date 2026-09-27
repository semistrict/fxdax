//! The entry point of hardware page faults into the VM.
//!
//! Ported from `zircon/kernel/vm/vmm.cc`.
//!
//! Deviations:
//! - No page fault timing or tracing.
//! - Threads are neither suspended nor killed while blocked in a fault, so there are no statuses
//!   of those to translate (`TranslatePageFaultStatus`).

use crate::kernel::thread::Current;
use crate::vm::fault::flag;
use zx_status::Status;

/// Resolves a page fault the hardware took at `addr`, described by `flags`.
pub fn vmm_page_fault_handler(addr: usize, mut flags: u32) -> Result<(), Status> {
    // hardware fault, mark it as such
    flags |= flag::HW_FAULT;

    // page fault it
    Current::page_fault(addr, flags)
}
