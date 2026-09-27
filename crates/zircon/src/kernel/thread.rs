//! The current thread's faults.
//!
//! Ported from `Thread::Current`'s fault functions in `zircon/kernel/kernel/thread.cc`.
//!
//! Deviation: threads are host threads, which all share the host process's address spaces, so a
//! fault is resolved in the host address space containing it (`VmAspace::find_host_aspace`),
//! standing in for the thread's process's `aspace_at`.

use crate::vm::vm_aspace::VmAspace;
use zx_status::Status;

/// The functions of `Thread::Current` that resolve faults.
pub struct Current;

impl Current {
    fn fault(
        va: usize,
        resolve_fault: impl FnOnce(&VmAspace) -> Result<(), Status>,
    ) -> Result<(), Status> {
        // Addresses outside every address space are not the process's to fault on, as kernel
        // addresses are not.
        let Some(containing_aspace) = VmAspace::find_host_aspace(va) else {
            return Err(Status::NOT_FOUND);
        };

        // Call the appropriate fault function on the containing address space.
        resolve_fault(&containing_aspace)
    }

    pub fn page_fault(va: usize, flags: u32) -> Result<(), Status> {
        Self::fault(va, |aspace| aspace.page_fault(va, flags))
    }

    pub fn soft_fault(va: usize, flags: u32) -> Result<(), Status> {
        Self::fault(va, |aspace| aspace.soft_fault(va, flags))
    }

    pub fn soft_fault_in_range(va: usize, flags: u32, len: usize) -> Result<(), Status> {
        Self::fault(va, |aspace| aspace.soft_fault_in_range(va, flags, len))
    }

    pub fn accessed_fault(va: usize) -> Result<(), Status> {
        Self::fault(va, |aspace| aspace.accessed_fault(va))
    }
}
