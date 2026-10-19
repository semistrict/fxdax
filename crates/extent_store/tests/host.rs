// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Stores in directories of the host's: their forks' handles, deleting them, and taking a store's
//! directory for one process at a time (TASK-44.1).

use extent_store::{
    Description, Dir, File as _, HostDir, Options, create_host, delete_host, fork_host, open_host,
    resolve_host,
};
use std::path::Path;
use storage_device::ExtentId;

const BLOCK: usize = 4096;
const BRANCH: u64 = 7;
const OPTIONS: Options = Options {
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
    create_host(path, description, false, OPTIONS).unwrap()
}

/// A store's directory resolves to it and its first branch; a fork's handle, to the store's
/// directory and the fork; relative handles too.
#[test]
fn handles_resolve_to_their_store_and_branch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let store = create(&path);
    let canonical = path.canonicalize().unwrap();
    assert_eq!(resolve_host(&path).unwrap(), (canonical.clone(), BRANCH));
    fork_host(&store, &dir.path().join("fork"), BRANCH + 1).unwrap();
    assert_eq!(resolve_host(&dir.path().join("fork")).unwrap(), (canonical.clone(), BRANCH + 1));
    // In the working directory, its parent named by no path.
    std::env::set_current_dir(dir.path()).unwrap();
    fork_host(&store, Path::new("relative"), BRANCH + 2).unwrap();
    assert_eq!(resolve_host(Path::new("relative")).unwrap(), (canonical, BRANCH + 2));
}

/// Deleting a fork removes its handle; the store, its directory, once its last branch goes.
#[test]
fn deleting_forks_and_stores() {
    let dir = tempfile::tempdir().unwrap();
    let (path, fork) = (dir.path().join("store"), dir.path().join("fork"));
    let store = create(&path);
    store.write(ExtentId::new(BRANCH, 1), 0, &[1; BLOCK]).unwrap();
    store.commit(None).unwrap();
    fork_host(&store, &fork, BRANCH + 1).unwrap();
    drop(store);
    delete_host(&path, OPTIONS).unwrap();
    assert!(path.exists(), "the fork holds what it shares of it");
    assert!(open_host(&path, false, OPTIONS).is_err(), "the deleted store opened");
    let forked = open_host(&fork, false, OPTIONS).unwrap();
    let mut data = vec![0; BLOCK];
    forked.read(ExtentId::new(BRANCH, 1), 0, &mut data).unwrap();
    assert_eq!(data, [1; BLOCK]);
    drop(forked);
    delete_host(&fork, OPTIONS).unwrap();
    assert!(!fork.exists() && !path.exists());
}

/// A directory is taken by one opening of it at a time: another waits for it to be let go, and
/// takes it then.
#[test]
fn directories_are_taken_one_at_a_time() {
    let dir = tempfile::tempdir().unwrap();
    let first = HostDir::new(dir.path(), false).unwrap();
    let second = HostDir::new(dir.path(), false).unwrap();
    first.take().unwrap();
    std::thread::scope(|scope| {
        // Let go once the second is waiting: well within the wait, and after its first try.
        scope.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(200));
            first.let_go();
        });
        let waiting = std::time::Instant::now();
        second.take().expect("taken once let go");
        assert!(waiting.elapsed() >= std::time::Duration::from_millis(150), "taken while held");
    });
    second.let_go();
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
