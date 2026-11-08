// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own, a child of volume.rs: the tests of lazy volumes, which `RootVolume::volume` opens
//! as they are asked for (`Options::lazy_volumes`, `ObjectManager::open_volume`).

use crate::errors::FxfsError;
use crate::filesystem::{FxFilesystem, FxFilesystemBuilder, SyncOptions};
use crate::object_handle::ObjectHandle;
use crate::object_store::transaction::{Options, lock_keys};
use crate::object_store::volume::root_volume;
use crate::object_store::volume::testing::{lazily, open_store, read_file, write_file};
use crate::object_store::volume::tests::do_fsck;
use crate::object_store::{
    HandleOptions, NewChildStoreOptions, ObjectKey, ObjectStore, StoreOptions,
};
use std::sync::Arc;
use storage_device::DeviceHolder;
use storage_device::fake_device::FakeDevice;

/// Makes volumes "a" and "b", each with a file, flushed, so that the journal holds nothing of
/// them, and returns the filesystem, open still.
async fn two_flushed_volumes() -> crate::filesystem::OpenFxFilesystem {
    let device = DeviceHolder::new(FakeDevice::new(16384, 512));
    let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
    let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
    for (name, byte) in [("a", 0xaa), ("b", 0xbb)] {
        let store =
            root.new_volume(name, NewChildStoreOptions::default()).await.expect("new_volume");
        write_file(&filesystem, &store, "file", &[byte; 8192]).await;
    }
    filesystem.journal().force_compact().await.expect("compact failed");
    filesystem.sync(SyncOptions { flush_device: true, ..Default::default() }).await.unwrap();
    filesystem
}

/// fxdax: volumes open as they are asked for, and only then, with what they require of the
/// metadata reservation paid for.
#[fuchsia::test]
async fn test_lazy_volumes() {
    let filesystem = two_flushed_volumes().await;
    filesystem.close().await.expect("Close failed");
    let device = filesystem.take_device().await;
    device.reopen(false);
    let filesystem = lazily(device).await;
    let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
    assert!(!open_store(&filesystem, &root, "a").await);
    assert!(!open_store(&filesystem, &root, "b").await);

    let a = root.volume("a", StoreOptions::default()).await.expect("volume failed");
    assert!(open_store(&filesystem, &root, "a").await);
    assert!(!open_store(&filesystem, &root, "b").await);
    assert_eq!(read_file(&a, "file").await, vec![0xaa; 8192]);
    // A second ask finds it.
    let again = root.volume("a", StoreOptions::default()).await.expect("volume failed");
    assert!(Arc::ptr_eq(&a, &again));

    let objects = filesystem.object_manager();
    assert_eq!(
        objects.metadata_reservation().amount() + objects.borrowed_metadata_space(),
        objects.required_reservation()
    );
    write_file(&filesystem, &a, "more", &[0xcc; 4096]).await;
    std::mem::drop((a, again, root));
    filesystem.close().await.expect("Close failed");

    // Opened eagerly, as fsck needs, the stores are as they should be.
    let device = filesystem.take_device().await;
    device.reopen(false);
    let filesystem = FxFilesystem::open(device).await.expect("open failed");
    do_fsck(&filesystem, Some("a"), None).await;
    do_fsck(&filesystem, Some("b"), None).await;
    filesystem.close().await.expect("Close failed");
}

/// fxdax: replay opens a volume the journal holds something of, and leaves the others closed.
#[fuchsia::test]
async fn test_lazy_volumes_replay() {
    let filesystem = two_flushed_volumes().await;
    let crashed = {
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let b = root.volume("b", StoreOptions::default()).await.expect("volume failed");
        write_file(&filesystem, &b, "in-the-journal", &[0xcc; 4096]).await;
        filesystem
            .sync(SyncOptions { flush_device: true, ..Default::default() })
            .await
            .expect("sync failed");
        filesystem.device().snapshot().expect("device snapshot failed")
    };
    filesystem.close().await.expect("Close failed");

    let filesystem = lazily(crashed).await;
    let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
    assert!(!open_store(&filesystem, &root, "a").await);
    assert!(open_store(&filesystem, &root, "b").await);
    let b = root.volume("b", StoreOptions::default()).await.expect("volume failed");
    assert_eq!(read_file(&b, "in-the-journal").await, vec![0xcc; 4096]);
    filesystem.close().await.expect("Close failed");
}

/// fxdax: a volume closed through a mount is reaped as it opens: what a crash left in its
/// graveyard goes then.
#[fuchsia::test]
async fn test_lazy_volumes_reap_as_they_open() {
    let device = DeviceHolder::new(FakeDevice::new(16384, 512));
    let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
    let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
    let store = root.new_volume("a", NewChildStoreOptions::default()).await.expect("new_volume");
    let mut transaction = filesystem
        .root_store()
        .new_transaction(lock_keys![], Options::default())
        .await
        .expect("new transaction failed");
    let handle =
        ObjectStore::create_object(&store, &mut transaction, HandleOptions::default(), None)
            .await
            .expect("create_object failed");
    let object_id = handle.object_id();
    store.add_to_graveyard(&mut transaction, object_id);
    transaction.commit().await.expect("commit failed");
    drop(handle);
    filesystem.journal().force_compact().await.expect("compact failed");
    filesystem.sync(SyncOptions { flush_device: true, ..Default::default() }).await.unwrap();
    let crashed = filesystem.device().snapshot().expect("device snapshot failed");
    filesystem.close().await.expect("Close failed");

    let filesystem = lazily(crashed).await;
    let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
    assert!(!open_store(&filesystem, &root, "a").await);
    let store = root.volume("a", StoreOptions::default()).await.expect("volume failed");
    filesystem.graveyard().flush().await;
    let found = store.tree().find(&ObjectKey::object(object_id)).await.expect("find failed");
    assert_eq!(found.map(|item| item.value), None);
    filesystem.close().await.expect("Close failed");
}

/// [`two_flushed_volumes`], opened again with volumes lazily.
async fn two_flushed_volumes_lazily() -> crate::filesystem::OpenFxFilesystem {
    let filesystem = two_flushed_volumes().await;
    filesystem.close().await.expect("Close failed");
    let device = filesystem.take_device().await;
    device.reopen(false);
    lazily(device).await
}

/// The object ID of the volume `name`.
async fn volume_id(root: &crate::object_store::volume::RootVolume, name: &str) -> u64 {
    root.volume_directory().lookup(name).await.unwrap().unwrap().0
}

/// fxdax: a volume closed once nothing has it open lets go of its store, what the journal held of
/// it flushed into its layers, and opens again as asked, with what was written to it (TASK-136.6).
#[fuchsia::test]
async fn test_closed_volumes_open_again() {
    let filesystem = two_flushed_volumes_lazily().await;
    let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
    let objects = filesystem.object_manager();
    let a = root.volume("a", StoreOptions::default()).await.expect("volume failed");
    write_file(&filesystem, &a, "written", &[0xcc; 4096]).await;
    let id = volume_id(&root, "a").await;
    assert!(objects.needs_flush(id));
    assert_eq!(objects.open_volumes(), 1);
    drop(a);

    objects.close_volume(id).await.expect("close_volume failed");
    assert!(!open_store(&filesystem, &root, "a").await);
    assert!(!objects.needs_flush(id));
    assert_eq!(objects.open_volumes(), 0);
    // Closed again, nothing happens.
    objects.close_volume(id).await.expect("close_volume failed");

    let a = root.volume("a", StoreOptions::default()).await.expect("volume failed");
    assert_eq!(read_file(&a, "written").await, vec![0xcc; 4096]);
    assert_eq!(read_file(&a, "file").await, vec![0xaa; 8192]);
    drop((a, root));
    filesystem.close().await.expect("Close failed");
    let device = filesystem.take_device().await;
    device.reopen(false);
    let filesystem = FxFilesystem::open(device).await.expect("open failed");
    do_fsck(&filesystem, Some("a"), None).await;
    filesystem.close().await.expect("Close failed");
}

/// fxdax: what was written to a volume before it closed is in its layers: a crash after loses
/// none of it, and once the journal no longer names the volume, a replay leaves it closed
/// (TASK-136.6).
#[fuchsia::test]
async fn test_closed_volumes_keep_what_was_written_through_a_crash() {
    let filesystem = two_flushed_volumes_lazily().await;
    let crashed = |compacted: bool| {
        let filesystem = filesystem.clone();
        async move {
            let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let b = root.volume("b", StoreOptions::default()).await.expect("volume failed");
            let name = format!("before-closing-{compacted}");
            write_file(&filesystem, &b, &name, &[0xdd; 4096]).await;
            drop(b);
            let id = volume_id(&root, "b").await;
            filesystem.object_manager().close_volume(id).await.expect("close_volume failed");
            if compacted {
                filesystem.journal().force_compact().await.expect("compact failed");
            }
            filesystem
                .sync(SyncOptions { flush_device: true, ..Default::default() })
                .await
                .expect("sync failed");
            filesystem.device().snapshot().expect("device snapshot failed")
        }
    };
    let (named, compacted) = (crashed(false).await, crashed(true).await);
    filesystem.close().await.expect("Close failed");

    for (device, compacted) in [(named, false), (compacted, true)] {
        let filesystem = lazily(device).await;
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        assert_eq!(open_store(&filesystem, &root, "b").await, !compacted);
        let b = root.volume("b", StoreOptions::default()).await.expect("volume failed");
        let name = format!("before-closing-{compacted}");
        assert_eq!(read_file(&b, &name).await, vec![0xdd; 4096]);
        drop((b, root));
        filesystem.close().await.expect("Close failed");
    }
}

/// fxdax: a volume whose store anything else holds stays open, as does one the journal holds
/// something of on a read-only filesystem, which can't flush it; one it holds nothing of closes
/// there too (TASK-136.6).
#[fuchsia::test]
async fn test_volumes_in_use_stay_open() {
    let filesystem = two_flushed_volumes_lazily().await;
    let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
    let a = root.volume("a", StoreOptions::default()).await.expect("volume failed");
    let id = volume_id(&root, "a").await;
    let objects = filesystem.object_manager();
    let error = objects.close_volume(id).await.expect_err("closed a volume in use");
    assert!(FxfsError::Unavailable.matches(&error), "{error:#}");
    assert!(open_store(&filesystem, &root, "a").await);
    assert_eq!(read_file(&a, "file").await, vec![0xaa; 8192]);
    write_file(&filesystem, &a, "in-the-journal", &[0xee; 4096]).await;
    drop((a, root));
    filesystem.sync(SyncOptions { flush_device: true, ..Default::default() }).await.unwrap();
    let crashed = filesystem.device().snapshot().expect("device snapshot failed");
    filesystem.close().await.expect("Close failed");

    let filesystem = FxFilesystemBuilder::new()
        .read_only(true)
        .lazy_volumes(true)
        .open(crashed)
        .await
        .expect("open failed");
    let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
    let objects = filesystem.object_manager();
    let error = objects.close_volume(id).await.expect_err("closed a volume the journal holds");
    assert!(FxfsError::ReadOnlyFilesystem.matches(&error), "{error:#}");
    assert!(open_store(&filesystem, &root, "a").await);
    let b = root.volume("b", StoreOptions::default()).await.expect("volume failed");
    drop(b);
    objects.close_volume(volume_id(&root, "b").await).await.expect("close_volume failed");
    assert!(!open_store(&filesystem, &root, "b").await);
    filesystem.close().await.expect("Close failed");
}
