// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Host memory standing in for physical memory.
//!
//! Zircon maps all of physical memory into the kernel (the physmap). Here the
//! PMM's memory is a host shared memory object, a physical address is an
//! offset into it from `PHYS_BASE`, and the physmap is a mapping of all of it.
//! Being a shared memory object, a page can also be mapped wherever a VMO
//! containing it is mapped, as a physical page would be.

use crate::kernel::types::PAddr;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::ptr::NonNull;

/// The physical address of the first page. Nonzero so that a zero physical
/// address is never valid.
pub const PHYS_BASE: PAddr = PAddr(0x4000_0000);

/// Host memory backing the PMM.
pub struct PhysMemory {
    fd: OwnedFd,
    base: NonNull<u8>,
    size: usize,
}

// SAFETY: The mapping is plain memory owned for the life of the kernel.
unsafe impl Send for PhysMemory {}
unsafe impl Sync for PhysMemory {}

/// Creates an anonymous shared memory object.
#[cfg(target_os = "linux")]
fn create_shared_memory() -> OwnedFd {
    // SAFETY: The name is a valid C string.
    let fd = unsafe { libc::memfd_create(c"fxdax-physical-memory".as_ptr(), libc::MFD_CLOEXEC) };
    assert!(fd >= 0, "memfd_create: {}", std::io::Error::last_os_error());
    // SAFETY: `fd` was just created and is owned here.
    unsafe { OwnedFd::from_raw_fd(fd) }
}

/// Creates an anonymous shared memory object.
#[cfg(target_os = "macos")]
fn create_shared_memory() -> OwnedFd {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    // Names are limited to 31 bytes. The object is unlinked as soon as it is open.
    let name = format!("/fxdax-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    let name = std::ffi::CString::new(name).unwrap();
    // SAFETY: The name is a valid C string.
    let fd = unsafe {
        libc::shm_open(
            name.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
            0o600 as libc::c_uint,
        )
    };
    assert!(fd >= 0, "shm_open: {}", std::io::Error::last_os_error());
    // SAFETY: As above.
    unsafe { libc::shm_unlink(name.as_ptr()) };
    // SAFETY: `fd` was just created and is owned here.
    unsafe { OwnedFd::from_raw_fd(fd) }
}

impl PhysMemory {
    /// Creates `size` bytes of physical memory. Pages are only committed by
    /// the host when first touched.
    pub fn new(size: usize) -> Self {
        let fd = create_shared_memory();
        // SAFETY: `fd` is a shared memory object.
        let status = unsafe { libc::ftruncate(fd.as_raw_fd(), size as libc::off_t) };
        assert_eq!(status, 0, "ftruncate physical memory: {}", std::io::Error::last_os_error());
        // SAFETY: Maps the whole object, with no address hint.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        assert_ne!(
            base,
            libc::MAP_FAILED,
            "mmap physical memory: {}",
            std::io::Error::last_os_error()
        );
        Self { fd, base: NonNull::new(base.cast()).unwrap(), size }
    }

    pub fn size(&self) -> usize {
        self.size
    }

    /// The shared memory object, for mapping pages elsewhere.
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }

    /// Returns the offset of physical address `paddr` in the shared memory
    /// object.
    pub fn offset_of(&self, paddr: PAddr) -> usize {
        let offset = paddr.0 - PHYS_BASE.0;
        assert!(offset < self.size, "physical address {:#x} out of range", paddr.0);
        offset
    }

    /// Returns the host address of physical address `paddr`.
    pub fn paddr_to_physmap(&self, paddr: PAddr) -> *mut u8 {
        // SAFETY: The offset is within the mapping.
        unsafe { self.base.as_ptr().add(self.offset_of(paddr)) }
    }
}

/// Returns the host address of physical address `paddr`.
pub fn paddr_to_physmap(paddr: PAddr) -> *mut u8 {
    crate::vm::pmm::node().phys_memory().paddr_to_physmap(paddr)
}
