// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TASK-39.2: a VM process reaches the memory of its extent daemon alone, which caches the
//! extents of the stores the daemon serves: a daemon is a trust domain, a tenant's, and another's
//! cached pages are not in what a VM process of this one can map.
//!
//! Two daemons, each its own process with its own kernel, serve a store each. Each store's extent,
//! written, committed and read through a window, is in its daemon's physical memory, which the
//! VM process of that store holds, and not in the other's.

#![cfg(target_os = "linux")]

use fxdax::{ExtentId, RemoteExtentStore, VolatileSlice};
use std::os::fd::{AsRawFd, BorrowedFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// An extent's bytes: 64 KiB unlike any other `seed`'s.
fn pattern(seed: u64) -> Vec<u8> {
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

/// A daemon of its own, serving at a socket in `dir`, once it does.
fn daemon(dir: &Path, name: &str) -> (PathBuf, KillOnDrop) {
    let socket = dir.join(format!("{name}.sock"));
    let child = Command::new(env!("CARGO_BIN_EXE_fxdax-extentd")).arg(&socket).spawn().unwrap();
    let child = KillOnDrop(child);
    let deadline = Instant::now() + Duration::from_secs(30);
    while fxdax::extentd::stats(&socket).is_err() {
        assert!(Instant::now() < deadline, "the daemon {name} never served");
        std::thread::sleep(Duration::from_millis(20));
    }
    (socket, child)
}

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `pages` pages of anonymous memory, which a store's window takes.
fn memory(pages: usize) -> std::ptr::NonNull<u8> {
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

/// Writes `bytes` as an extent of `store`, commits, and reads it through a window, which caches
/// its pages in the daemon's memory.
fn cache(store: &RemoteExtentStore, bytes: &[u8]) {
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
fn holds(memory: BorrowedFd<'_>, bytes: &[u8]) -> bool {
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

#[test]
fn vm_processes_reach_their_daemons_memory_alone() {
    let dir = tempfile::tempdir().unwrap();
    let (one, _one) = daemon(dir.path(), "one");
    let (two, _two) = daemon(dir.path(), "two");
    let ours = RemoteExtentStore::connect(&one, dir.path().join("ours"), 1 << 24).unwrap();
    let theirs = RemoteExtentStore::connect(&two, dir.path().join("theirs"), 1 << 24).unwrap();
    let (our_bytes, their_bytes) = (pattern(1), pattern(2));
    cache(&ours, &our_bytes);
    cache(&theirs, &their_bytes);

    assert!(holds(ours.physical_memory(), &our_bytes), "our pages are not in our memory");
    assert!(!holds(ours.physical_memory(), &their_bytes), "their pages are in our memory");
    assert!(holds(theirs.physical_memory(), &their_bytes), "their pages are not in theirs");
    assert!(!holds(theirs.physical_memory(), &our_bytes), "our pages are in theirs");
}
