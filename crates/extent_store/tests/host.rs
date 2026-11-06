// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Stores in directories of the host's: a store a directory, of one branch (TASK-165.7), deleting
//! them, and taking a store's directory for one process at a time (TASK-44.1), and once in it.

use extent_store::{
    Description, Dir, File as _, HostDir, Options, create_host, delete_host, host_branch,
    host_bucket, open_host,
};
use std::path::Path;
use storage_device::ExtentId;

const BLOCK: usize = 4096;
const BRANCH: u64 = 7;
const OPTIONS: Options = Options {
    max_extent: 64 << 20,
    segment_size: 64 << 10,
    segment_age: std::time::Duration::from_secs(3600),
    large_extent: 32 << 10,
    checkpoint_after: 4 << 10,
    open_files: 2,
    view_lag: None,
    view_kept_bytes: None,
};

fn create(path: &Path) -> extent_store::Store<HostDir> {
    std::fs::create_dir(path).unwrap();
    let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: BRANCH };
    create_host(path, description, "memory:///host", false, OPTIONS).unwrap()
}

/// fxdax: a host's store names its branch, and the bucket it is kept in (TASK-165.6), which its
/// description says, opened or not.
#[test]
fn stores_name_their_branch_and_bucket() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let store = create(&path);
    assert_eq!(store.bucket(), Some("memory:///host"));
    assert_eq!(host_bucket(&path).unwrap().as_deref(), Some("memory:///host"));
    assert_eq!(host_branch(&path).unwrap(), BRANCH);
    drop(store);
    let opened = open_host(&path, false, OPTIONS).unwrap();
    assert_eq!((opened.bucket(), opened.description().branch), (Some("memory:///host"), BRANCH));
}

/// A store is opened once in a process, as in all: a second opening, and a deletion, of one open
/// fail at once, rather than wait for the process itself (TASK-165.7).
#[test]
fn stores_open_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let store = create(&path);
    let started = std::time::Instant::now();
    let again = open_host(&path, false, OPTIONS).err().expect("opened twice");
    assert_eq!(
        format!("{again:#}"),
        "taking the store's directory: this process has the store open"
    );
    assert!(delete_host(&path, |_| Ok(())).is_err(), "deleted an open store");
    assert!(started.elapsed() < std::time::Duration::from_secs(1), "waited for itself");
    drop(store);
    drop(open_host(&path, false, OPTIONS).unwrap());
}

/// Deleting a store removes its directory, whole, once `before` deleted its branch elsewhere.
#[test]
fn deleting_stores() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let store = create(&path);
    store.write(ExtentId::new(BRANCH, 1), 0, &[1; BLOCK]).unwrap();
    store.commit(None).unwrap();
    drop(store);
    let deleted = std::cell::Cell::new(None);
    delete_host(&path, |branch| Ok(deleted.set(Some(branch)))).unwrap();
    assert_eq!((deleted.get(), path.exists()), (Some(BRANCH), false));
    assert!(open_host(&path, false, OPTIONS).is_err(), "the deleted store opened");
}

/// A directory is taken by one process at a time: a taking waits for another's to be let go, and
/// takes it then. Another's is a `flock` of the directory here, through a descriptor of its own, as
/// another process's is.
#[test]
fn directories_are_taken_one_at_a_time() {
    use std::os::fd::AsRawFd as _;
    let dir = tempfile::tempdir().unwrap();
    let others = std::fs::File::open(dir.path()).unwrap();
    // SAFETY: Plain system call on the directory's descriptor.
    assert_eq!(unsafe { libc::flock(others.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0);
    let ours = HostDir::new(dir.path(), false).unwrap();
    std::thread::scope(|scope| {
        // Let go once ours is waiting: well within the wait, and after its first try.
        scope.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(200));
            // SAFETY: Plain system call on the directory's descriptor.
            assert_eq!(unsafe { libc::flock(others.as_raw_fd(), libc::LOCK_UN) }, 0);
        });
        let waiting = std::time::Instant::now();
        ours.take().expect("taken once let go");
        assert!(waiting.elapsed() >= std::time::Duration::from_millis(150), "taken while held");
    });
    ours.let_go();
}

/// A file removed is gone.
#[test]
fn removed_files_are_gone() {
    let dir = tempfile::tempdir().unwrap();
    let host = HostDir::new(dir.path(), false).unwrap();
    host.create("file", false).unwrap().write_at(b"data", 0).unwrap();
    assert_eq!(host.list("").unwrap(), ["file"]);
    host.remove("file").unwrap();
    assert_eq!(host.list("").unwrap(), Vec::<String>::new());
}
