//! The global allocator: the kernel's kvmalloc, which may sleep.
//!
//! Rust allocates only where the kernel may sleep. Code that runs where it may not, such as a
//! waker called from a timer, does not allocate. Allocations that fail panic, as Rust's do.

use crate::ffi;
use core::alloc::{GlobalAlloc, Layout};

struct Kernel;

// SAFETY: kvmalloc returns memory of at least the size asked for, aligned as `fxfs_rt_alloc`
// promises, or null.
unsafe impl GlobalAlloc for Kernel {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: Allocates; any size and alignment may be asked for.
        unsafe { ffi::fxfs_rt_alloc(layout.size(), layout.align()).cast() }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        // SAFETY: `ptr` came from `alloc`.
        unsafe { ffi::fxfs_rt_free(ptr.cast()) }
    }
}

#[global_allocator]
static KERNEL: Kernel = Kernel;
