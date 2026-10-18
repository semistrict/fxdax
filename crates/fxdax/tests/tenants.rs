// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TASK-43: one extent daemon serves every tenant, each in an arena of its kernel's physical memory
//! of its own: a tenant's VM processes hold their tenant's arena, and the default one, whose pages
//! are public bases', read-only, and nothing of another tenant's. This is defence in depth, for a
//! VM process escaped from its guest; guest isolation is the security boundary.
//!
//! The daemon serves on threads of this process: a listener for each tenant.

#![cfg(target_os = "linux")]

mod common;

use common::{cache, copies, holds, memory, pattern};
use fxdax::extentd::{Tenant, serve, serve_tenant};
use fxdax::{Errno, ExtentId, ExtentStore, RemoteExtentStore};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A tenant named `name`, whose stores are in the directory `name` in `dir`.
fn tenant(dir: &Path, name: &str) -> Arc<Tenant> {
    std::fs::create_dir(dir.join(name)).unwrap();
    Tenant::new(name, dir.join(name)).unwrap()
}

/// A daemon's listener at `name` in `dir`, serving `tenant`'s VM processes, or those of none.
fn listen(dir: &Path, name: &str, tenant: Option<Arc<Tenant>>) -> PathBuf {
    common::host();
    let socket = dir.join(format!("{name}.sock"));
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || match tenant {
        Some(tenant) => serve_tenant(listener, tenant),
        None => serve(listener),
    });
    socket
}

/// Two tenants' VM processes each hold their own tenant's cached pages, and not the other's.
#[test]
fn tenants_hold_their_own_pages() {
    let dir = tempfile::tempdir().unwrap();
    let one = listen(dir.path(), "one", Some(tenant(dir.path(), "one")));
    let two = listen(dir.path(), "two", Some(tenant(dir.path(), "two")));
    let ours = RemoteExtentStore::connect(&one, dir.path().join("one/ours"), 1 << 24).unwrap();
    let theirs = RemoteExtentStore::connect(&two, dir.path().join("two/theirs"), 1 << 24).unwrap();
    let (our_bytes, their_bytes) = (pattern(1), pattern(2));
    cache(&ours, &our_bytes);
    cache(&theirs, &their_bytes);

    assert_eq!(copies(ours.physical_memory(), &our_bytes), 1, "our pages, once, in ours");
    assert!(!holds(ours.physical_memory(), &their_bytes), "their pages are in our memory");
    assert_eq!(copies(theirs.physical_memory(), &their_bytes), 1, "theirs, once, in theirs");
    assert!(!holds(theirs.physical_memory(), &our_bytes), "our pages are in theirs");
    // Nor in the default arena, which both hold.
    let public = ours.public_memory().expect("a tenant's VM process holds the default arena");
    assert!(!holds(public, &our_bytes) && !holds(public, &their_bytes));
}

/// Forks of a public base in two tenants read its pages from the default arena, one copy, which
/// neither VM process can write, and neither opens the base itself.
#[test]
fn public_bases_are_shared_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let one = listen(dir.path(), "one", Some(tenant(dir.path(), "one")));
    let two = listen(dir.path(), "two", Some(tenant(dir.path(), "two")));
    let base_path = dir.path().join("base");
    let base = ExtentStore::create(&base_path, 1 << 24).unwrap();
    let base_bytes = pattern(3);
    let extent = ExtentId { branch: base.branch(), sequence: 1 };
    base.write(extent, 0, &[fxdax::VolatileSlice::from(&base_bytes[..])]).unwrap();
    base.commit(None).unwrap();
    base.make_public().unwrap();
    drop(base);
    let forks = [dir.path().join("one/fork"), dir.path().join("two/fork")];
    let base = ExtentStore::open(&base_path).unwrap();
    for (fork, branch) in forks.iter().zip(1..) {
        base.fork(fork, base.branch() + branch).unwrap();
    }
    drop(base);

    // Not in its directory, nor a tenant's to open were it there.
    let refused = RemoteExtentStore::connect(&one, &base_path, 1 << 24);
    assert_eq!(refused.err().and_then(|error| error.errno()), Some(Errno::EACCES));
    let base_in_one = dir.path().join("one/base");
    ExtentStore::create(&base_in_one, 1 << 24).unwrap().make_public().unwrap();
    let refused = RemoteExtentStore::connect(&one, &base_in_one, 1 << 24);
    assert_eq!(refused.err().and_then(|error| error.errno()), Some(Errno::EPERM));

    let mut stores = Vec::new();
    for (socket, fork) in [(&one, &forks[0]), (&two, &forks[1])] {
        let store = RemoteExtentStore::connect(socket, fork, 1 << 24).unwrap();
        let pages = base_bytes.len() / fxdax::kernel::page_size() as usize;
        let window = memory(pages);
        // SAFETY: The memory is this test's, which it leaks: the window maps it while the process
        // lives.
        unsafe { store.create_dax_window_in_host_memory(window, base_bytes.len() as u64, None) }
            .unwrap();
        store.map(extent, 0, base_bytes.len() as u64, 0, false).unwrap();
        // SAFETY: The window's memory, whose faults the store serves.
        let mapped = unsafe { std::slice::from_raw_parts(window.as_ptr(), base_bytes.len()) };
        assert!(mapped == base_bytes, "the fork maps other bytes");
        assert!(!holds(store.physical_memory(), &base_bytes), "the base's pages are a tenant's");
        stores.push(store);
    }
    let public = stores[0].public_memory().unwrap();
    assert_eq!(copies(public, &base_bytes), 1, "the base's pages, once, in the default arena");

    // Neither tenant's VM process can map the default arena writably.
    let size = fxdax::kernel::page_size() as usize;
    // SAFETY: An attempt at a shared, writable mapping of the read-only descriptor.
    let mapped = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            public.as_raw_fd(),
            0,
        )
    };
    assert_eq!(mapped, libc::MAP_FAILED);
    assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EACCES));
}
