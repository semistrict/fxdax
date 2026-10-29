// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Tests of physical memory through its API, as its users allocate it, and of what they cannot
//! see, which the tests reach inside for: which chunk a page is of, which arena has a chunk.
//!
//! Physical memory, relief and quarantine are the process's: nextest runs each test in a process
//! of its own, which configures them as it needs.

use crate::CHUNK_SIZE;
#[cfg(not(feature = "shuttle"))]
use crate::{ArenaId, Page, page_bytes};
#[cfg(not(feature = "shuttle"))]
use std::os::fd::AsRawFd;

#[cfg(not(feature = "shuttle"))]
mod arenas;
#[cfg(not(feature = "shuttle"))]
mod config;
#[cfg(not(feature = "shuttle"))]
mod memory_out;
#[cfg(not(feature = "shuttle"))]
mod quarantine;
#[cfg(feature = "shuttle")]
mod shuttle;

/// The pages of a chunk.
#[cfg(not(feature = "shuttle"))]
fn chunk_pages() -> usize {
    CHUNK_SIZE / page_bytes()
}

/// The chunk `page` is of.
#[cfg(not(feature = "shuttle"))]
fn chunk_of(page: &Page) -> usize {
    crate::pmm::node().chunk_of(page.frame())
}

/// The arena that has chunk `chunk`, if any.
#[cfg(not(feature = "shuttle"))]
fn chunk_arena(chunk: usize) -> Option<ArenaId> {
    crate::pmm::node().phys_memory().chunk_arena(chunk)
}

/// Physical memory of `chunks` chunks, for a test whose process has not used it yet.
fn configure_chunks(chunks: usize) {
    crate::configure((chunks * CHUNK_SIZE) as u64).expect("physical memory is not in use yet");
}

/// The page at `offset` in arena `arena`'s shared memory object, as another process mapping the
/// object sees it.
#[cfg(not(feature = "shuttle"))]
fn read_arena(arena: ArenaId, offset: u64) -> Vec<u8> {
    let fd = crate::arena_fd(arena).unwrap();
    // SAFETY: Maps a page of the object, read-only, which is unmapped below.
    let addr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            page_bytes(),
            libc::PROT_READ,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            offset as libc::off_t,
        )
    };
    assert_ne!(addr, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
    // SAFETY: The mapping is a page long.
    let page = unsafe { std::slice::from_raw_parts(addr.cast::<u8>(), page_bytes()) }.to_vec();
    // SAFETY: Unmaps the mapping made above.
    unsafe { libc::munmap(addr, page_bytes()) };
    page
}

/// Fills `page` with `byte`.
#[cfg(not(feature = "shuttle"))]
fn fill(page: &Page, byte: u8) {
    // SAFETY: The page is the test's, a page long.
    unsafe { std::ptr::write_bytes(page.as_ptr(), byte, page_bytes()) };
}

/// The first byte of `page`.
#[cfg(not(feature = "shuttle"))]
fn first_byte(page: &Page) -> u8 {
    // SAFETY: The page is the test's.
    unsafe { page.as_ptr().read() }
}
