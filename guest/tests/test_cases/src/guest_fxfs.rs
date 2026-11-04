// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: what the guest halves of the tests of the guest's Fxfs share (fxfs-guest's,
//! fxfs-guest-checks'): evicting a file's inode, whether a file is DAX, what a mount counts, and,
//! copying on write (doc-13), whether the window does, and files read in from sealed extents, as a
//! base's are, whose writes are the window's.

use nix::libc;
use std::ffi::CString;
use std::fs;
use std::os::fd::AsRawFd;

/// Evicts `path`'s inode, for it to be read in again: written back, and its pages dropped, as an
/// inode that has any isn't evicted (dropping every cache would drop the filesystem's metadata's
/// too).
pub fn evict(path: &str) {
    let file = fs::File::open(path).unwrap();
    file.sync_all().unwrap();
    let fd = file.as_raw_fd();
    assert_eq!(unsafe { libc::posix_fadvise(fd, 0, 0, libc::POSIX_FADV_DONTNEED) }, 0);
    drop(file);
    fs::write("/proc/sys/vm/drop_caches", b"2").unwrap();
}

/// Whether the kernel accesses `path` through the DAX window: STATX_ATTR_DAX.
pub fn is_dax(path: &str) -> bool {
    const STATX_ATTR_DAX: u64 = 0x200000;
    // struct statx: stx_attributes at byte 8, stx_attributes_mask at 56.
    let mut buf = [0u64; 32];
    let c_path = CString::new(path).unwrap();
    let ret = unsafe {
        libc::syscall(
            libc::SYS_statx,
            libc::AT_FDCWD,
            c_path.as_ptr(),
            0,
            0x7ff, // STATX_BASIC_STATS
            buf.as_mut_ptr(),
        )
    };
    assert_eq!(ret, 0, "statx: {}", std::io::Error::last_os_error());
    let (attributes, mask) = (buf[1], buf[7]);
    assert_ne!(mask & STATX_ATTR_DAX, 0, "the kernel does not report DAX");
    attributes & STATX_ATTR_DAX != 0
}

/// What the Fxfs mounted from the device tagged `tag` counts, /proc/fs/fxfs/<tag> (TASK-91.7), by
/// name: a histogram (NAME_hist, its buckets' counts comma-separated, TASK-111.6) as how many it
/// counts.
pub fn stats(tag: &str) -> std::collections::BTreeMap<String, u64> {
    let text = fs::read_to_string(format!("/proc/fs/fxfs/{tag}")).unwrap();
    (text.lines())
        .map(|line| {
            let (name, value) = line.split_once(' ').unwrap();
            let counts = value.split(',').map(|count| count.parse::<u64>().unwrap());
            (name.to_owned(), counts.sum())
        })
        .collect()
}

/// Whether the window of the device tagged `tag` copies on write (fxfs.dax_cow, doc-13): the device
/// took DAX_COW, as the mount's counts say (dax_cow), not merely that it was asked for. Turned on
/// for a whole run by FXDAX_GUEST_CMDLINE=fxfs.dax_cow=1, as the kernel's command line takes it.
pub fn dax_cow(tag: &str) -> bool {
    stats(tag).get("dax_cow").is_some_and(|cow| *cow == 1)
}

/// Has the file `path`, written here, read in again once its writes are sealed: evicted, which
/// writes it back, and a barrier seals. Copying on write, a file created in the mount is a
/// page-cache inode until then, its writes the page cache's (TASK-164.3); read in again, it is DAX,
/// mapped from sealed extents, its writes the window's, as a base's files are.
pub fn read_in_sealed(path: &str) {
    evict(path);
    assert!(is_dax(path), "{path}, read in again once sealed, isn't DAX");
}

/// A file of `len` bytes of `byte`'s pattern, written, and read in again once sealed
/// ([`read_in_sealed`]): copying on write, a DAX file, as a base's are. Its bytes.
pub fn cow_base_file(path: &str, len: usize, byte: u8) -> Vec<u8> {
    let data: Vec<u8> = (0..len).map(|i| byte ^ (i % 251) as u8).collect();
    fs::write(path, &data).unwrap();
    read_in_sealed(path);
    data
}
