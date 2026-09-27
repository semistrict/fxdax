//! Host memory standing in for physical memory.
//!
//! Zircon maps all of physical memory into the kernel (the physmap). Here the
//! PMM's memory is one host mapping, a physical address is an offset into it
//! from `PHYS_BASE`, and the physmap is that mapping.

use crate::kernel::types::PAddr;
use std::ptr::NonNull;

/// The physical address of the first page. Nonzero so that a zero physical
/// address is never valid.
pub const PHYS_BASE: PAddr = PAddr(0x4000_0000);

/// Host memory backing the PMM.
pub struct PhysMemory {
    base: NonNull<u8>,
    size: usize,
}

// SAFETY: The mapping is plain memory owned for the life of the kernel.
unsafe impl Send for PhysMemory {}
unsafe impl Sync for PhysMemory {}

impl PhysMemory {
    /// Reserves `size` bytes. Pages are only committed by the host when first
    /// touched.
    pub fn new(size: usize) -> Self {
        // SAFETY: Anonymous mapping with no address hint.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_NORESERVE,
                -1,
                0,
            )
        };
        assert_ne!(
            base,
            libc::MAP_FAILED,
            "mmap physical memory: {}",
            std::io::Error::last_os_error()
        );
        Self { base: NonNull::new(base.cast()).unwrap(), size }
    }

    pub fn size(&self) -> usize {
        self.size
    }

    /// Returns the host address of physical address `paddr`.
    pub fn paddr_to_physmap(&self, paddr: PAddr) -> *mut u8 {
        let offset = paddr.0 - PHYS_BASE.0;
        assert!(offset < self.size, "physical address {:#x} out of range", paddr.0);
        // SAFETY: `offset` is within the mapping.
        unsafe { self.base.as_ptr().add(offset) }
    }
}

/// Returns the host address of physical address `paddr`.
pub fn paddr_to_physmap(paddr: PAddr) -> *mut u8 {
    crate::vm::pmm::node().phys_memory().paddr_to_physmap(paddr)
}
