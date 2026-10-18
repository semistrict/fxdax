// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TASK-43.3: the extent daemon's control API, gRPC over a Unix socket, as the host's VM manager
//! calls it, from a client in Rust: every call, and the errors a store a VM has open gives.
//!
//! The daemon serves on threads of this process: its control socket, and a VM socket, whose
//! connections, VM processes here, have stores open.

#![cfg(target_os = "linux")]

mod common;

use common::daemon::{CAPACITY, Daemon, failure, write};
use common::pattern;
use fxdax::Errno;
use fxdax::extentd::control::proto::*;
use std::path::Path;
use tonic::Code;

/// A store made, written by a VM, grown, forked, made a public base, synced to object storage and
/// taken from there to a new store, placed, and deleted, through the API.
#[test]
fn stores_through_the_control_api() {
    // SAFETY: nextest runs each test in a process of its own, whose other threads don't read the
    // environment yet.
    unsafe {
        std::env::set_var("FXDAX_EXTENTD_REMOTE", "memory:///branches");
        std::env::set_var("FXDAX_EXTENTD_TOKEN", "1");
    }
    let dir = tempfile::tempdir().unwrap();
    let mut daemon = Daemon::new(dir.path());
    let (base, fork, taken) =
        (dir.path().join("base"), dir.path().join("fork"), dir.path().join("taken"));

    let branch = daemon.create(&base, 0, None).unwrap();
    assert_ne!(branch, 0);
    let vm = daemon.open(&base, CAPACITY);
    assert_eq!(vm.branch(), branch);
    write(&vm, &pattern(1), b"base");
    drop(vm);
    let stat = daemon.stat(&base).unwrap();
    assert_eq!(
        stat,
        StatStoreResponse {
            branch,
            capacity: CAPACITY,
            used: 64 << 10,
            available: CAPACITY - (64 << 10),
            public: false,
            open: false,
        }
    );

    daemon.resize(&base, 2 * CAPACITY).unwrap();
    assert_eq!(daemon.stat(&base).unwrap().capacity, 2 * CAPACITY);
    // Its guest, which would have to agree, isn't there.
    assert_eq!(failure(daemon.resize(&base, CAPACITY)), (Code::FailedPrecondition, Errno::EBUSY));

    let forked = daemon.fork(&base, &fork).unwrap();
    assert_ne!(forked, branch);
    let stat = daemon.stat(&fork).unwrap();
    assert_eq!((stat.branch, stat.used, stat.public), (forked, 64 << 10, false));
    daemon.make_public(&base).unwrap();
    assert!(daemon.stat(&base).unwrap().public);
    assert!(!daemon.stat(&fork).unwrap().public, "a fork of a public base is its own");

    // The fork's commits in object storage, another host takes its branch, placed after this
    // one's: what it holds there.
    daemon.sync(&fork).unwrap();
    assert_eq!(daemon.create(&taken, forked, Some(2)).unwrap(), forked);
    let vm = daemon.open(&taken, CAPACITY);
    assert_eq!(vm.root().unwrap(), b"base");
    drop(vm);

    daemon.place(&base, 3).unwrap();
    daemon.delete(&taken).unwrap();
    assert_eq!(failure(daemon.stat(&taken)), (Code::NotFound, Errno::ENOENT));

    let stats = daemon.stats();
    let object_storage = stats.object_storage.expect("the daemon has a bucket");
    assert!(object_storage.requests["create"] > 0, "{object_storage:?}");
    assert!(object_storage.uploaded_bytes.values().sum::<u64>() >= 64 << 10, "{object_storage:?}");
}

/// A store a VM process has open is its: the API refuses what would change it under its guest,
/// grows it, and forks it live; and calls fail with the errnos the .proto says.
#[test]
fn open_stores_are_their_vms() {
    let dir = tempfile::tempdir().unwrap();
    let mut daemon = Daemon::new(dir.path());
    let (store, fork) = (dir.path().join("store"), dir.path().join("fork"));
    let vm = daemon.open(&store, CAPACITY);
    write(&vm, &pattern(2), b"open");

    assert!(daemon.stat(&store).unwrap().open);
    let busy = (Code::FailedPrecondition, Errno::EBUSY);
    assert_eq!(failure(daemon.delete(&store)), busy);
    assert_eq!(failure(daemon.make_public(&store)), busy);
    assert_eq!(failure(daemon.place(&store, 1)), busy);
    assert_eq!(failure(daemon.create(&store, 0, None)), busy);
    // Grown at once, as its VM process sees.
    daemon.resize(&store, 2 * CAPACITY).unwrap();
    assert_eq!(vm.info().unwrap().capacity, 2 * CAPACITY);
    // Forked live: nothing is open on it.
    let branch = daemon.fork(&store, &fork).unwrap();
    assert_eq!(daemon.stat(&fork).unwrap().branch, branch);

    assert_eq!(failure(daemon.create(&fork, 0, None)), (Code::AlreadyExists, Errno::EEXIST));
    let none = dir.path().join("none");
    assert_eq!(failure(daemon.stat(&none)), (Code::NotFound, Errno::ENOENT));
    assert_eq!(
        failure(daemon.create(Path::new(""), 0, None)),
        (Code::InvalidArgument, Errno::EINVAL)
    );
    // No bucket to sync to.
    assert_eq!(failure(daemon.sync(&store)), (Code::FailedPrecondition, Errno::ENOTSUP));
    assert_eq!(daemon.stats().object_storage, None);

    // Gone with its VM process, it is the API's.
    drop(vm);
    assert!(!daemon.stat(&store).unwrap().open);
    daemon.delete(&store).unwrap();
}

/// A container image imported is a store holding its files.
#[test]
fn imports_images() {
    use fxdax::import::testing::{Entry, Kind, layer};
    let dir = tempfile::tempdir().unwrap();
    let mut daemon = Daemon::new(dir.path());
    let image = dir.path().join("layer.tar");
    let hello = b"hello from the image\n";
    std::fs::write(&image, layer(&[Entry::new("hello", Kind::File(hello))]).unwrap()).unwrap();
    let store = dir.path().join("store");

    let branch = daemon.import(&store, &image).unwrap();
    assert_eq!(daemon.stat(&store).unwrap().branch, branch);
    assert_eq!(failure(daemon.import(&store, &image)), (Code::AlreadyExists, Errno::EEXIST));

    let size = futures::executor::block_on(async {
        let fs = fxdax::Filesystem::mount(fxdax::Storage::open(&store).unwrap()).await.unwrap();
        let attr = fs.lookup(fxdax::Inode::ROOT, b"hello").await.unwrap();
        fs.close().await.unwrap();
        attr.size
    });
    assert_eq!(size, hello.len() as u64);
}
