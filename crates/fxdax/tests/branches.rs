//! fxdax-branch over a real S3 server: `scripts/test-rustfs.sh` starts RustFS and runs this with
//! `FXDAX_S3_URL` naming a bucket of it, and the `AWS_*` variables to reach it.

use extent_remote::store::Store as Bucket;
use fxdax::{ExtentId, ExtentStore, VolatileSlice, VolatileSliceMut};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

fn pattern(seed: u8, len: usize) -> Vec<u8> {
    (0..len).map(|i| seed ^ (i % 251) as u8).collect()
}

/// Runs fxdax-branch with `args`, the bucket at `url` the stores' remote: what it prints.
fn branch(url: &str, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_fxdax-branch"))
        .args(args)
        .env("FXDAX_EXTENTD_REMOTE", url)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "fxdax-branch {args:?}: {stderr}");
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

/// A store forked with fxdax-branch is a fork of its branch in the bucket, and a branch taken with
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
