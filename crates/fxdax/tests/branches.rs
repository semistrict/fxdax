// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdaxctl over a real S3 server: `scripts/test-rustfs.sh` starts RustFS and runs this with
//! `FXDAX_S3_URL` naming a bucket of it, and the `AWS_*` variables to reach it.

use extent_remote::store::Store as Bucket;
use fxdax::{ExtentId, ExtentStore, VolatileSlice, VolatileSliceMut};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

fn pattern(seed: u8, len: usize) -> Vec<u8> {
    (0..len).map(|i| seed ^ (i % 251) as u8).collect()
}

/// Runs fxdaxctl with `args`, the bucket at `url` the stores' remote, as a host whose placement's
/// token is 1: what it prints.
fn branch(url: &str, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_fxdaxctl"))
        .args(args)
        .env("FXDAX_EXTENTD_REMOTE", url)
        .env("FXDAX_EXTENTD_TOKEN", "1")
        .env_remove("FXDAX_EXTENTD_LOCAL_ONLY")
        .env_remove("FXDAX_EXTENTD_GRPC")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "fxdaxctl {args:?}: {stderr}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// The store at `path`, keeping its branch in `bucket`.
fn attached(path: &Path, bucket: &Arc<Bucket>) -> ExtentStore {
    let store = ExtentStore::open(path).unwrap();
    store.attach_remote(bucket.clone(), rand::random(), Default::default()).unwrap();
    store
}

fn read(store: &ExtentStore, extent: ExtentId, len: usize) -> Vec<u8> {
    let mut data = vec![0; len];
    store.read(extent, 0, &mut [VolatileSliceMut::from(&mut data[..])]).unwrap();
    data
}

/// A store forked with fxdaxctl is a fork of its branch in the bucket, and a branch taken with
/// it onto another host's empty store is read there from the bucket; deleted with it, the fork
/// goes, and then its parent.
#[test]
#[ignore]
fn rustfs_forks_and_takes_branches() {
    let url = std::env::var("FXDAX_S3_URL").expect("FXDAX_S3_URL names the bucket to use");
    let bucket = Arc::new(Bucket::from_url(&url).unwrap());
    let dir = tempfile::tempdir().unwrap();
    let path = |name: &str| dir.path().join(name);

    let parent = ExtentStore::create(path("parent"), 1 << 30).unwrap();
    parent.attach_remote(bucket.clone(), rand::random(), Default::default()).unwrap();
    let data = pattern(7, 5 << 20);
    let extent = ExtentId { branch: parent.branch(), sequence: 1 };
    parent.write(extent, 0, &[VolatileSlice::from(&data[..])]).unwrap();
    parent.commit(Some(b"root")).unwrap();
    let parent_branch = parent.branch();
    drop(parent);

    let child =
        branch(&url, &["fork", path("parent").to_str().unwrap(), path("child").to_str().unwrap()]);
    let child_branch = u64::from_str_radix(&child, 16).unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let record = runtime.block_on(extent_remote::branch::record(&*bucket, child_branch));
    assert_eq!(record.unwrap().unwrap().origin.map(|origin| origin.parent), Some(parent_branch));
    let forked = attached(&path("child"), &bucket);
    assert_eq!(forked.branch(), child_branch);
    assert_eq!(read(&forked, extent, data.len()), data);
    drop(forked);

    branch(&url, &["take", path("elsewhere").to_str().unwrap(), &(1u64 << 30).to_string(), &child]);
    let elsewhere = attached(&path("elsewhere"), &bucket);
    assert_eq!(elsewhere.root().unwrap(), b"root");
    assert_eq!(read(&elsewhere, extent, data.len()), data);

    // The fork deleted, then its parent's store.
    branch(&url, &["delete", path("child").to_str().unwrap()]);
    assert!(!path("child").exists());
    branch(&url, &["delete", path("parent").to_str().unwrap()]);
    assert!(!path("parent").exists());
}

/// Runs fxdaxctl with `args`, the bucket at `url` the stores' remote, expecting it to fail: what it
/// says.
fn branch_fails(url: &str, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_fxdaxctl"))
        .args(args)
        .env("FXDAX_EXTENTD_REMOTE", url)
        .env("FXDAX_EXTENTD_TOKEN", "1")
        .env_remove("FXDAX_EXTENTD_LOCAL_ONLY")
        .env_remove("FXDAX_EXTENTD_GRPC")
        .output()
        .unwrap();
    assert!(!output.status.success(), "fxdaxctl {args:?} succeeded");
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// TASK-39.7: `fxdaxctl sync` returns once a store's commits are in the bucket; a store's copy
/// placed earlier than the placement that took its branch last takes it no more, and placed later,
/// does.
#[test]
#[ignore]
fn rustfs_places_and_syncs_stores() {
    let url = std::env::var("FXDAX_S3_URL").expect("FXDAX_S3_URL names the bucket to use");
    let bucket = Arc::new(Bucket::from_url(&url).unwrap());
    let dir = tempfile::tempdir().unwrap();
    let path = |name: &str| dir.path().join(name).to_str().unwrap().to_owned();

    let store = ExtentStore::create(path("first"), 1 << 30).unwrap();
    let branch_id = store.branch();
    let extent = ExtentId { branch: branch_id, sequence: 1 };
    store.write(extent, 0, &[VolatileSlice::from(&pattern(3, 1 << 20)[..])]).unwrap();
    store.commit(Some(b"synced")).unwrap();
    drop(store);
    branch(&url, &["place", &path("first"), "5"]);
    branch(&url, &["sync", &path("first")]);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let state = runtime.block_on(extent_remote::recover(&*bucket, branch_id, None)).unwrap().state;
    assert_eq!(state.root, Some(b"synced".to_vec()));

    let capacity = (1u64 << 30).to_string();
    let hex = format!("{branch_id:x}");
    branch(&url, &["take", &path("earlier"), &capacity, &hex, "3"]);
    let stale = branch_fails(&url, &["sync", &path("earlier")]);
    assert!(stale.contains("a later placement holds the branch"), "{stale}");
    branch(&url, &["place", &path("earlier"), "6"]);
    branch(&url, &["sync", &path("earlier")]);
    let later = attached(Path::new(&path("earlier")), &bucket);
    assert_eq!(later.root().unwrap(), b"synced");
    assert_eq!(read(&later, extent, 1 << 20), pattern(3, 1 << 20));
}
