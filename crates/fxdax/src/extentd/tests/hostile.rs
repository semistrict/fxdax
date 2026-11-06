// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TASK-149.17: what a compromised VM process does to the daemon, checked against what must stay
//! true of the tenants beside it.

use super::super::client::RemoteExtentStore;
use super::super::server::{Tenant, serve_tenant};
use super::{BLOCK, host, read, write};
use crate::ExtentId;
use std::os::fd::AsRawFd as _;
use std::os::unix::net::UnixListener;

/// TASK-149.17: a VM process that shrinks the memory it is given as its store opens crashes
/// nothing: not its tenant's arena, which it may write, whose pages past a new end would fault the
/// daemon's own accesses (`SIGBUS`), and every tenant's VMs with it; nor the public one, which it
/// may only read. It may grow its arena, as the daemon goes on growing it. The memory it reads
/// alone is its owner's alone to open anew: a VM process, of another user, can't reopen it to
/// write through `/proc/self/fd`.
#[test]
fn vm_processes_shrinking_their_memory_crash_nothing() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let vms = dir.path().join("vms");
    std::fs::create_dir(&vms).unwrap();
    let socket = dir.path().join("vms.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let tenant = Tenant::new("vms", &vms).unwrap();
    std::thread::spawn(move || serve_tenant(listener, tenant));
    let store = RemoteExtentStore::connect(&socket, vms.join("store"), 1 << 26).unwrap();
    let extent = |sequence| ExtentId { branch: store.branch(), sequence };
    write(&store, extent(1), 0, &super::pattern(1, BLOCK)).unwrap();
    store.commit(None).unwrap();
    // In the extent cache, the arena's pages.
    assert_eq!(read(&store, extent(1), 0, BLOCK), Ok(super::pattern(1, BLOCK)));

    let (own, public) = (store.physical_memory().as_raw_fd(), store.public_memory().as_raw_fd());
    let truncated = |fd, len: i64| {
        // SAFETY: Sizes an object the test holds a descriptor of.
        match unsafe { libc::ftruncate(fd, len) } {
            0 => Ok(()),
            _ => Err(std::io::Error::last_os_error().raw_os_error()),
        }
    };
    let shrunk = (truncated(own, 0), truncated(public, 0));
    // What the daemon reads of the arena's pages, which it would fault on, had the VM process
    // shrunk it.
    assert_eq!(read(&store, extent(1), 0, BLOCK), Ok(super::pattern(1, BLOCK)));
    assert_eq!(shrunk, (Err(Some(libc::EPERM)), Err(Some(libc::EINVAL))));
    // Grown, by a tebibyte, as the daemon grows it further, as its extents take more chunks.
    // SAFETY: A stat buffer the call fills, of a descriptor the test holds.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::fstat(own, &mut stat) }, 0);
    assert_eq!(truncated(own, stat.st_size + (1 << 40)), Ok(()));
    let extents = 2 * physmem::CHUNK_SIZE / (8 * BLOCK);
    for sequence in 2..2 + extents as u64 {
        write(&store, extent(sequence), 0, &super::pattern(sequence as u8, 8 * BLOCK)).unwrap();
    }
    store.commit(None).unwrap();
    for sequence in 1..2 + extents as u64 {
        let len = if sequence == 1 { BLOCK } else { 8 * BLOCK };
        assert_eq!(read(&store, extent(sequence), 0, len), Ok(super::pattern(sequence as u8, len)));
    }
    // SAFETY: As above.
    assert_eq!(unsafe { libc::fstat(public, &mut stat) }, 0);
    assert_eq!(stat.st_mode & 0o777, 0o600);
    drop(store);
}
