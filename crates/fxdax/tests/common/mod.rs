// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What the tests of extent daemons' memory share: extents' bytes, windows, and finding bytes in
//! a daemon's memory.

#![allow(dead_code)]

#[cfg(target_os = "linux")]
pub mod daemon;

/// Makes this process a host of the multi-node system, as an extent daemon's must be: its stores'
/// branches in a bucket in its memory, as of placement 1, unless the test named others.
pub fn host() {
    // The daemon's log, and the VM processes', on standard error, which a test that fails shows.
    let _ = fxdax::extentd::logging::to_stderr();
    for (name, value) in
        [("FXDAX_EXTENTD_REMOTE", "memory:///branches"), ("FXDAX_EXTENTD_TOKEN", "1")]
    {
        if std::env::var_os(name).is_none() {
            // SAFETY: nextest runs each test in a process of its own, whose other threads don't
            // read the environment yet.
            unsafe { std::env::set_var(name, value) };
        }
    }
}

use fxdax::{ExtentId, RemoteExtentStore, VolatileSlice};
use std::os::fd::{AsRawFd, BorrowedFd};

/// An extent's bytes: 64 KiB unlike any other `seed`'s.
pub fn pattern(seed: u64) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    (0..64 << 10)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

/// `pages` pages of anonymous memory, which a store's window takes.
pub fn memory(pages: usize) -> std::ptr::NonNull<u8> {
    let len = pages * fxdax::kernel::page_size() as usize;
    // SAFETY: A new anonymous mapping, which nothing else uses.
    let memory = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
            -1,
            0,
        )
    };
    assert_ne!(memory, libc::MAP_FAILED);
    std::ptr::NonNull::new(memory.cast()).unwrap()
}

/// Reads `len` bytes of a window's memory at `at` as a vCPU's access does: one the window's second
/// stage is changing the memory under fails, with `EFAULT`, rather than resolving wrongly, and is
/// tried again, as fxdax's Firecracker runs the vCPU again (`fxdax::dax`'s uffd docs), where a
/// load of this process's would take `SIGSEGV`. The kernel copies the bytes into a pipe, its fault
/// on them resolved as the guest's would be.
#[cfg(target_os = "linux")]
pub fn read_window(at: *const u8, len: usize) -> Vec<u8> {
    let page = fxdax::kernel::page_size() as usize;
    let mut fds = [0; 2];
    // SAFETY: A new pipe, whose descriptors this function closes.
    assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
    let mut bytes = vec![0; len];
    for offset in (0..len).step_by(page) {
        let n = page.min(len - offset);
        loop {
            // SAFETY: The kernel reads the window's memory, failing rather than faulting.
            let written = unsafe { libc::write(fds[1], at.add(offset).cast(), n) };
            if written == n as isize {
                break;
            }
            let error = std::io::Error::last_os_error();
            assert_eq!(error.raw_os_error(), Some(libc::EFAULT), "copying the window: {error}");
            std::thread::yield_now();
        }
        // SAFETY: Reads back what was just written, into `bytes`.
        let read = unsafe { libc::read(fds[0], bytes[offset..].as_mut_ptr().cast(), n) };
        assert_eq!(read, n as isize);
    }
    // SAFETY: The pipe's descriptors, which nothing else uses.
    unsafe {
        libc::close(fds[0]);
        libc::close(fds[1]);
    }
    bytes
}

/// Writes `bytes` as an extent of `store`, commits, and reads it through a window, which caches
/// its pages in the daemon's memory.
pub fn cache(store: &RemoteExtentStore, bytes: &[u8]) {
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    store.write(extent, 0, &[VolatileSlice::from(bytes)]).unwrap();
    store.commit(None).unwrap();
    let window = memory(bytes.len() / fxdax::kernel::page_size() as usize);
    // SAFETY: The memory is this function's, which it leaks: the window maps it while the
    // process lives.
    unsafe { store.create_dax_window_in_host_memory(window, bytes.len() as u64, None) }.unwrap();
    store.map(extent, 0, bytes.len() as u64, 0, false).unwrap();
    // SAFETY: The window's memory, whose faults the store serves.
    let mapped = unsafe { std::slice::from_raw_parts(window.as_ptr(), bytes.len()) };
    assert!(mapped == bytes, "the window maps other bytes");
}

/// Whether `memory`, a shared memory object, holds `bytes` at a page's start: in the parts of it
/// that hold data.
pub fn holds(memory: BorrowedFd<'_>, bytes: &[u8]) -> bool {
    let page = fxdax::kernel::page_size() as usize;
    let fd = memory.as_raw_fd();
    let mut offset = 0;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        // SAFETY: Plain system calls on the object's descriptor.
        let data = unsafe { libc::lseek(fd, offset, libc::SEEK_DATA) };
        if data < 0 {
            return false;
        }
        // SAFETY: As above.
        let hole = unsafe { libc::lseek(fd, data, libc::SEEK_HOLE) };
        let mut at = data;
        while at < hole {
            let len = (hole - at).min(buf.len() as i64) as usize;
            // SAFETY: Reads into `buf`, of at least `len` bytes.
            let read = unsafe { libc::pread(fd, buf.as_mut_ptr().cast(), len, at) };
            assert!(read > 0, "reading the memory: {}", std::io::Error::last_os_error());
            let read = read as usize;
            if buf[..read].chunks(page).any(|chunk| chunk.starts_with(&bytes[..page])) {
                return true;
            }
            at += read as i64;
        }
        offset = hole;
    }
}

/// How many pages of `memory`, a shared memory object, start as `bytes` does: copies of it.
pub fn copies(memory: BorrowedFd<'_>, bytes: &[u8]) -> usize {
    let page = fxdax::kernel::page_size() as usize;
    let fd = memory.as_raw_fd();
    let mut offset = 0;
    let mut count = 0;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        // SAFETY: Plain system calls on the object's descriptor.
        let data = unsafe { libc::lseek(fd, offset, libc::SEEK_DATA) };
        if data < 0 {
            return count;
        }
        // SAFETY: As above.
        let hole = unsafe { libc::lseek(fd, data, libc::SEEK_HOLE) };
        let mut at = data;
        while at < hole {
            let len = (hole - at).min(buf.len() as i64) as usize;
            // SAFETY: Reads into `buf`, of at least `len` bytes.
            let read = unsafe { libc::pread(fd, buf.as_mut_ptr().cast(), len, at) };
            assert!(read > 0, "reading the memory: {}", std::io::Error::last_os_error());
            let read = read as usize;
            count +=
                buf[..read].chunks(page).filter(|chunk| chunk.starts_with(&bytes[..page])).count();
            at += read as i64;
        }
        offset = hole;
    }
}
