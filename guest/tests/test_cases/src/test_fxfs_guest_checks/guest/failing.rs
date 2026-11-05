// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the checks made to fail, which the overrun suite runs before the page cache's checks
//! ([`Suites::Failing`]): one crashes, one hangs in the kernel. Each fails alone, by name, what it
//! left of the guest put back, and the checks after it pass, as they would after a check that
//! crashed or hung by mistake (TASK-160.30).

use super::*;

/// Crashes, as a store past a mapped file's end does (SIGBUS), with one of fxfs's fault
/// injections set, which no guard sets back: the agent does.
pub(super) fn test_crashes() {
    const PAGE: usize = 4096;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(path("crashes"))
        .unwrap();
    file.set_len(PAGE as u64).unwrap();
    let map = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            2 * PAGE,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
    };
    assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
    let _fail = Param::set("dax_fail_maps", "1000");
    unsafe { map.cast::<u8>().add(PAGE).write_volatile(1) };
    panic!("a store past the file's end went in");
}

/// Hangs in the kernel: the filesystem frozen (FIFREEZE), a write waits in it to thaw, which no
/// one does but the agent, once the check runs on past its budget and it has killed it. The
/// write, uninterruptible, keeps the process from ending until then.
pub(super) fn test_hangs() {
    const FIFREEZE: libc::c_ulong = 0xc0045877;
    let dir = fs::File::open(MNT).unwrap();
    let ret = unsafe { libc::ioctl(dir.as_raw_fd(), FIFREEZE as _, 0) };
    assert_eq!(ret, 0, "FIFREEZE: {}", std::io::Error::last_os_error());
    std::thread::spawn(|| fs::write(path("hangs"), b"written once thawed").unwrap());
    loop {
        std::thread::park();
    }
}
