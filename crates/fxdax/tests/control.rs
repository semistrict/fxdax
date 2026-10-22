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

use common::daemon::{CAPACITY, Daemon, failure, name, write};
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
            own: 64 << 10,
            inherited: 0,
            committed: 1,
            // Closed, it uploads nothing now.
            uploading: false,
            lease_token: 0,
            lease_epoch: 0,
            lease_taken_unix_ms: 0,
            uploaded: 0,
            pending_upload_bytes: 0,
            oldest_pending_ms: 0,
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
    // TASK-49: all it holds, it inherited.
    assert_eq!((stat.own, stat.inherited), (0, 64 << 10));
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

/// TASK-50.5: the stores VM processes have open are listed, with their branches' leases, as this
/// host's placement took them, and their uploads: synced, nothing waits.
#[test]
fn open_stores_are_listed_with_their_leases_and_uploads() {
    let dir = tempfile::tempdir().unwrap();
    let mut daemon = Daemon::new(dir.path());
    let (first, second) = (dir.path().join("first"), dir.path().join("second"));
    let before = std::time::SystemTime::now();
    let one = daemon.open(&first, CAPACITY);
    write(&one, &pattern(1), b"one");
    let two = daemon.open(&second, CAPACITY);
    daemon.sync(&first).unwrap();
    daemon.sync(&second).unwrap();
    let after = std::time::SystemTime::now();

    let listed = daemon.list();
    let paths: Vec<_> = listed.iter().map(|(path, _)| path.clone()).collect();
    let canonical = |path: &Path| path.canonicalize().unwrap();
    assert_eq!(paths, [canonical(&first), canonical(&second)]);
    let millis = |time: std::time::SystemTime| {
        time.duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
    };
    for ((_, stat), branch) in listed.iter().zip([one.branch(), two.branch()]) {
        assert_eq!((stat.branch, stat.open, stat.uploading), (branch, true, true));
        assert_eq!((stat.lease_token, stat.lease_epoch), (1, 1));
        assert!((millis(before)..=millis(after)).contains(&stat.lease_taken_unix_ms), "{stat:?}");
        assert_eq!(stat.uploaded, stat.committed, "{stat:?}");
        assert_eq!((stat.pending_upload_bytes, stat.oldest_pending_ms), (0, 0));
    }
    assert_eq!(daemon.stat(&first).unwrap(), listed[0].1);
    drop((one, two));
}

/// A store a VM process has open is its: the API refuses what would change it under its guest,
/// grows it, forks it live, and syncs it; and calls fail with the errnos the .proto says.
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
    // Synced as its VM process goes on.
    daemon.sync(&store).unwrap();

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
        let (_, attr) = fs.root().lookup(b"hello").await.unwrap();
        fs.close().await.unwrap();
        attr.size
    });
    assert_eq!(size, hello.len() as u64);
}

/// A fork of a branch this host never had: the branch's store taken from object storage
/// (CreateStore of its branch, placed after the host that wrote it), then forked (ForkStore), whose
/// VM reads what the branch held, its data fetched as read.
#[test]
fn forks_of_branches_never_had_here() {
    // SAFETY: as in stores_through_the_control_api.
    unsafe {
        std::env::set_var("FXDAX_EXTENTD_REMOTE", "memory:///elsewhere");
        std::env::set_var("FXDAX_EXTENTD_TOKEN", "1");
    }
    let dir = tempfile::tempdir().unwrap();
    let mut daemon = Daemon::new(dir.path());
    let (written, taken, fork) =
        (dir.path().join("written"), dir.path().join("taken"), dir.path().join("fork"));
    // Another host's: written, synced, and gone from here.
    let vm = daemon.open(&written, CAPACITY);
    let branch = vm.branch();
    write(&vm, &pattern(7), b"elsewhere");
    drop(vm);
    daemon.sync(&written).unwrap();
    std::fs::remove_dir_all(&written).unwrap();

    assert_eq!(daemon.create(&taken, branch, Some(2)).unwrap(), branch);
    let forked = daemon.fork(&taken, &fork).unwrap();
    let vm = daemon.open(&fork, CAPACITY);
    assert_eq!(vm.branch(), forked);
    assert_eq!(vm.root().unwrap(), b"elsewhere");
    let extent = fxdax::ExtentId { branch, sequence: 1 };
    let mut read = vec![0; 64 << 10];
    vm.read(extent, 0, &mut [fxdax::VolatileSliceMut::from(&mut read[..])]).unwrap();
    assert!(read == pattern(7), "the fork reads other bytes");
}

/// TASK-43.6: calls waiting hold no thread: shrinks waiting for guests that don't answer, more of
/// them than the threads the calls' I/O runs on, leave another call to complete at once, and end
/// as the guests' time to answer does.
#[test]
fn shrinks_waiting_for_their_guests_hold_no_threads() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = tempfile::tempdir().unwrap();
    let mut daemon = Daemon::new(dir.path());
    let idle = dir.path().join("idle");
    daemon.create(&idle, 0, None).unwrap();
    let count = fxdax::extentd::control::BLOCKING_THREADS + 16;
    let asked = Arc::new(AtomicUsize::new(0));
    let (vms, paths): (Vec<_>, Vec<_>) = (0..count)
        .map(|vm| {
            let path = dir.path().join(format!("vm-{vm}"));
            let store = daemon.open(&path, CAPACITY);
            // A guest that never answers.
            let asked = asked.clone();
            store.on_capacity(move |_, ask| {
                if ask {
                    asked.fetch_add(1, Ordering::Relaxed);
                }
            });
            (store, path)
        })
        .collect();

    let shrinks: Vec<_> = (paths.iter())
        .map(|path| {
            let mut client = daemon.client.clone();
            let request = ResizeStoreRequest { path: name(path), capacity: CAPACITY / 2 };
            daemon.runtime.spawn(async move { client.resize_store(request).await })
        })
        .collect();
    daemon.runtime.block_on(async {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while asked.load(Ordering::Relaxed) < count {
            assert!(tokio::time::Instant::now() < deadline, "the guests weren't all asked");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    });
    let start = std::time::Instant::now();
    assert_eq!(daemon.stat(&idle).unwrap().capacity, CAPACITY);
    assert!(start.elapsed() < std::time::Duration::from_secs(2), "{:?}", start.elapsed());

    for shrink in shrinks {
        let shrunk = daemon.runtime.block_on(shrink).unwrap();
        assert_eq!(failure(shrunk), (Code::DeadlineExceeded, Errno::ETIMEDOUT));
    }
    drop(vms);
}

/// TASK-48: the control socket serves the standard gRPC health service: NOT_SERVING until the
/// daemon accepts VM processes, SERVING while it does, NOT_SERVING once it stops; Watch streams
/// each change, for the whole server and the control API alike.
#[test]
fn health_follows_whether_the_daemon_serves() {
    use tonic_health::pb::HealthCheckRequest;
    use tonic_health::pb::health_check_response::ServingStatus;
    use tonic_health::pb::health_client::HealthClient;
    // SAFETY: As above.
    unsafe {
        std::env::set_var("FXDAX_EXTENTD_REMOTE", "memory:///branches");
        std::env::set_var("FXDAX_EXTENTD_TOKEN", "1");
    }
    let dir = tempfile::tempdir().unwrap();
    let control = dir.path().join("control.sock");
    let listener = std::os::unix::net::UnixListener::bind(&control).unwrap();
    let tenants = dir.path().join("tenants");
    std::thread::spawn(move || fxdax::extentd::serve_control(listener, tenants));
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let channel = fxdax::extentd::control::channel(&control).await.unwrap();
        let health = HealthClient::new(channel);
        let mut watcher = health.clone();
        let status = |service: &str| {
            let mut health = health.clone();
            let request = HealthCheckRequest { service: service.to_owned() };
            async move { health.check(request).await.unwrap().into_inner().status() }
        };
        let api = "fxdax.extentd.v1.Extentd";
        assert_eq!(status("").await, ServingStatus::NotServing);
        assert_eq!(status(api).await, ServingStatus::NotServing);
        let request = HealthCheckRequest { service: String::new() };
        let mut watch = watcher.watch(request).await.unwrap().into_inner();
        let mut next = async || watch.message().await.unwrap().unwrap().status();
        assert_eq!(next().await, ServingStatus::NotServing);

        let vms = std::os::unix::net::UnixListener::bind(dir.path().join("vms.sock")).unwrap();
        std::thread::spawn(move || fxdax::extentd::serve(vms));
        assert_eq!(next().await, ServingStatus::Serving);
        assert_eq!(status(api).await, ServingStatus::Serving);

        tokio::task::spawn_blocking(fxdax::extentd::stop_serving).await.unwrap();
        assert_eq!(next().await, ServingStatus::NotServing);
        assert_eq!(status(api).await, ServingStatus::NotServing);
    });
}
