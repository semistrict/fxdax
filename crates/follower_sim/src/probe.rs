// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fxfs under Shuttle at all: a writer on an extent store, and a follower that mounts it
//! read-only through a view, which reads what the view's commit held while the writer goes on,
//! until it follows the writer.

use crate::device::{StoreDevice, ViewDevice};
use extent_store::{Description, MemDir, Options as StoreOptions, Store};
use fuchsia_async as fasync;
use fxfs::filesystem::{FxFilesystem, FxFilesystemBuilder, OpenFxFilesystem, SyncOptions};
use fxfs::object_handle::{ObjectHandle, WriteObjectHandle};
use fxfs::object_store::directory::Directory;
use fxfs::object_store::transaction::{LockKey, Options, lock_keys};
use fxfs::object_store::{HandleOptions, ObjectStore};
use shuttle::scheduler::RandomScheduler;
use shuttle::{Config, Runner};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use storage_device::DeviceHolder;

const BLOCK: usize = 4096;

fn config() -> Config {
    let mut config = Config::new();
    config.stack_size = 16 << 20;
    config
}

/// `file`'s first block, in the root directory of `fs`'s root store.
async fn read(fs: &OpenFxFilesystem, file: &str) -> Vec<u8> {
    let root_store = fs.root_store();
    let root = Directory::open(&root_store, root_store.root_directory_object_id())
        .await
        .expect("open failed");
    let (object_id, ..) = root.lookup(file).await.expect("lookup failed").expect("no file");
    let handle = ObjectStore::open_object(&root_store, object_id, HandleOptions::default(), None)
        .await
        .expect("open_object failed");
    let mut buf = handle.allocate_buffer(BLOCK).await;
    assert_eq!(handle.read_aligned(0, buf.as_mut()).await.expect("read failed"), BLOCK);
    buf.to_vec()
}

async fn write_and_follow() {
    let description = Description { block_size: BLOCK as u32, capacity: 64 << 20, branch: 1 };
    let store =
        Arc::new(Store::create(MemDir::new(), description, StoreOptions::default()).unwrap());
    let device = DeviceHolder::new(StoreDevice::new(store.clone()));
    let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
    let root_store = fs.root_store();
    let root = Directory::open(&root_store, root_store.root_directory_object_id())
        .await
        .expect("open failed");
    let mut transaction = root_store
        .new_transaction(
            lock_keys![LockKey::object(root_store.store_object_id(), root.object_id())],
            Options::default(),
        )
        .await
        .expect("new_transaction failed");
    let handle = root.create_child_file(&mut transaction, "file").await.expect("create failed");
    transaction.commit().await.expect("commit failed");
    let mut buf = handle.allocate_buffer(BLOCK).await;
    buf.fill(7);
    handle.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
    fs.sync(SyncOptions { flush_device: true, ..SyncOptions::default() })
        .await
        .expect("sync failed");

    // A follower as of now, which goes on reading it as the writer changes it.
    let view = DeviceHolder::new(ViewDevice::new(store.clone(), store.view()));
    let follower =
        FxFilesystemBuilder::new().read_only(true).open(view).await.expect("follower failed");
    assert_eq!(read(&follower, "file").await, vec![7; BLOCK]);
    buf.fill(8);
    handle.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
    fs.sync(SyncOptions { flush_device: true, ..SyncOptions::default() })
        .await
        .expect("sync failed");
    assert_eq!(read(&follower, "file").await, vec![7; BLOCK]);

    // A follower as of then reads the new.
    let view = DeviceHolder::new(ViewDevice::new(store.clone(), store.view()));
    let later = FxFilesystemBuilder::new().read_only(true).open(view).await.expect("later failed");
    assert_eq!(read(&later, "file").await, vec![8; BLOCK]);
    later.close().await.expect("close failed");

    // The follower, once it follows, reads the new too.
    follower.follow().await.expect("follow failed");
    assert_eq!(read(&follower, "file").await, vec![8; BLOCK]);
    follower.close().await.expect("close failed");
    fs.close().await.expect("close failed");
    DONE.fetch_add(1, Ordering::SeqCst);
}

/// The runs that got to the end.
static DONE: AtomicUsize = AtomicUsize::new(0);

#[test]
fn a_follower_reads_its_views_commit() {
    Runner::new(RandomScheduler::new(50), config()).run(|| {
        fasync::LocalExecutor::new().run_singlethreaded(write_and_follow());
    });
    assert_eq!(DONE.load(Ordering::SeqCst), 50);
}
