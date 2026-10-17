// Copyright 2016 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
// Copyright (c) 2008-2015 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The current thread's faults.
//!
//! Ported from `Thread::Current`'s fault functions in `zircon/kernel/kernel/thread.cc`.
//!
//! Deviation: threads are host threads, which all share the host process's address spaces, so a
//! fault is resolved in the host address space containing it (`VmAspace::find_host_aspace`),
//! standing in for the thread's process's `aspace_at`. Faulting addresses are host addresses,
//! which are translated to the address space's own.

use crate::vm::vm_aspace::VmAspace;
use zx_status::Status;

/// The functions of `Thread::Current` that resolve faults.
pub struct Current;

impl Current {
    fn fault(
        va: usize,
        resolve_fault: impl FnOnce(&VmAspace, usize) -> Result<(), Status>,
    ) -> Result<(), Status> {
        // Addresses outside every address space are not the process's to fault on, as kernel
        // addresses are not.
        let Some(containing_aspace) = VmAspace::find_host_aspace(va) else {
            return Err(Status::NOT_FOUND);
        };

        // Call the appropriate fault function on the containing address space.
        resolve_fault(&containing_aspace, containing_aspace.vaddr_of_host(va))
    }

    pub fn page_fault(va: usize, flags: u32) -> Result<(), Status> {
        Self::fault(va, |aspace, va| aspace.page_fault(va, flags))
    }

    pub fn soft_fault(va: usize, flags: u32) -> Result<(), Status> {
        Self::fault(va, |aspace, va| aspace.soft_fault(va, flags))
    }

    pub fn soft_fault_in_range(va: usize, flags: u32, len: usize) -> Result<(), Status> {
        Self::fault(va, |aspace, va| aspace.soft_fault_in_range(va, flags, len))
    }

    /// Faults in the range as `soft_fault_in_range` does, as a future that waits on page requests
    /// rather than blocking the calling thread.
    pub async fn soft_fault_in_range_async(
        va: usize,
        flags: u32,
        len: usize,
    ) -> Result<(), Status> {
        let Some(containing_aspace) = VmAspace::find_host_aspace(va) else {
            return Err(Status::NOT_FOUND);
        };
        let va = containing_aspace.vaddr_of_host(va);
        containing_aspace.soft_fault_in_range_async(va, flags, len).await
    }

    pub fn accessed_fault(va: usize) -> Result<(), Status> {
        Self::fault(va, |aspace, va| aspace.accessed_fault(va))
    }
}
