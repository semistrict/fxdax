// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own, a child of volume.rs: volume snapshots, which share their source's layers, by
//! reference count, and its extents, which the allocator freezes (`Allocator::snapshot`).

use super::RootVolume;
use crate::errors::FxfsError;
use crate::filesystem::{FlushReason, ForceMajor};
use crate::object_handle::ObjectHandle;
use crate::object_store::transaction::{LockKeys, Options, lock_keys};
use crate::object_store::{HandleOptions, LockKey, ObjectDescriptor, ObjectStore};
use crate::serialized_types::VersionedLatest;
use anyhow::{Context, Error, anyhow, bail, ensure};
use std::sync::Arc;

impl RootVolume {
    /// fxdax: snapshots the volume `source_name`, which must be unencrypted, into the new volume
    /// `snapshot_name`, which holds what the source held when it was flushed here, and is a volume
    /// of its own from then on.
    ///
    /// The snapshot shares the source's persistent layers, by reference count, and its extents,
    /// which the allocator freezes (see `Allocator::snapshot`), so it takes time and space
    /// independent of the source's size. Writes to the source that are not flushed when this
    /// flushes it, such as a file's dirty pages, are not in the snapshot.
    ///
    /// It is atomic with the source's commits (TASK-241): it holds them back from before its flush
    /// until it commits (`LockKey::Commits`), so that none comes between the layers it shares and
    /// the freezing of the source's data owner. One built before the freeze and committed after
    /// it frees nothing frozen: the allocator leaves frozen extents out as it applies it.
    pub async fn snapshot_volume(
        &self,
        source_name: &str,
        snapshot_name: &str,
    ) -> Result<Arc<ObjectStore>, Error> {
        let filesystem = &self.filesystem;
        let root_store = filesystem.root_store();
        let source_object_id = match self.volume_directory().lookup(source_name).await? {
            Some((object_id, ObjectDescriptor::Volume, _)) => object_id,
            Some(_) => bail!(anyhow!(FxfsError::Inconsistent).context("Expected volume")),
            None => bail!(FxfsError::NotFound),
        };
        let source = filesystem
            .object_manager()
            .store(source_object_id)
            .ok_or(FxfsError::NotFound)
            .context("Missing volume store")?;

        // fxdax: the source's commits held back until the snapshot commits, taken before the flush
        // lock: a commit in flight may compact the journal, which flushes the source (TASK-241).
        let _commits = filesystem
            .lock_manager()
            .write_lock(lock_keys![LockKey::commits(source_object_id)])
            .await;
        // Flush the source, and keep its layers as they are until the snapshot shares them.
        let _flush_guard = filesystem
            .lock_manager()
            .write_lock(lock_keys![LockKey::flush(source_object_id)])
            .await;
        source.flush_guarded_with_reason(FlushReason::Journal(ForceMajor::False)).await?;
        let info = source.snapshot_store_info()?;

        let root_store_object_id = root_store.store_object_id();
        let mut keys =
            vec![LockKey::object(root_store_object_id, self.volume_directory().object_id())];
        keys.extend(info.layers.iter().map(|layer| LockKey::object(root_store_object_id, *layer)));
        // Declared first, as the transaction writes it.
        let handle;
        let mut transaction =
            root_store.new_transaction(LockKeys::Vec(keys), Options::default()).await?;
        ensure!(
            matches!(self.volume_directory().lookup(snapshot_name).await?, None),
            FxfsError::AlreadyExists
        );

        // The snapshot's store, whose information is the source's, sharing its layers.
        handle = ObjectStore::create_object(
            &root_store,
            &mut transaction,
            HandleOptions::default(),
            None,
        )
        .await?;
        let snapshot_object_id = handle.object_id();
        for layer in &info.layers {
            root_store.adjust_refs(&mut transaction, *layer, 1).await?;
        }
        let mut serialized_info = Vec::new();
        info.serialize_with_version(&mut serialized_info)?;
        let mut buf = handle.allocate_buffer(serialized_info.len()).await;
        buf.copy_from_slice(&serialized_info);
        handle.txn_write(&mut transaction, 0u64, buf.as_ref()).await?;

        // From here on, the source allocates under an owner of its own, never used before.
        let new_owner_object_id = root_store.get_next_object_id().await?.release().get();
        filesystem.allocator().snapshot(
            &mut transaction,
            source_object_id,
            snapshot_object_id,
            new_owner_object_id,
        );
        self.volume_directory()
            .add_child_volume(&mut transaction, snapshot_name, snapshot_object_id)
            .await?;
        transaction.commit().await?;

        let store = ObjectStore::open(
            &root_store,
            snapshot_object_id,
            Some(Box::new(filesystem.new_tree_cache())),
        )
        .await?;
        store.set_trace(filesystem.trace());
        filesystem.object_manager().add_store(store.clone());
        Ok(store)
    }
}

#[cfg(test)]
mod tests {
    use crate::filesystem::{FxFilesystem, SyncOptions};
    use crate::object_handle::{ObjectHandle, WriteObjectHandle};
    use crate::object_store::directory::Directory;
    use crate::object_store::volume::root_volume;
    use crate::object_store::volume::testing::{delete_volume, open_file, read_file, write_file};
    use crate::object_store::volume::tests::do_fsck;
    use crate::object_store::{HandleOptions, NewChildStoreOptions, ObjectStore, StoreOptions};
    use std::sync::Arc;
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;
    use test_case::test_case;

    /// fxdax: a snapshot holds what its source held, and each changes on its own after it.
    #[fuchsia::test]
    async fn test_snapshot_volume() {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let source = root
            .new_volume("source", NewChildStoreOptions::default())
            .await
            .expect("new_volume failed");
        write_file(&filesystem, &source, "file", &[0xaa; 8192]).await;
        write_file(&filesystem, &source, "kept", &[0x11; 4096]).await;
        let snapshot =
            root.snapshot_volume("source", "snapshot").await.ok().expect("snapshot failed");
        assert_ne!(snapshot.store_object_id(), source.store_object_id());
        assert_eq!(read_file(&snapshot, "file").await, vec![0xaa; 8192]);

        // Writes to either don't show in the other.
        write_file(&filesystem, &source, "file", &[0xbb; 8192]).await;
        write_file(&filesystem, &snapshot, "kept", &[0x22; 4096]).await;
        assert_eq!(read_file(&source, "file").await, vec![0xbb; 8192]);
        assert_eq!(read_file(&snapshot, "file").await, vec![0xaa; 8192]);
        assert_eq!(read_file(&source, "kept").await, vec![0x11; 4096]);
        assert_eq!(read_file(&snapshot, "kept").await, vec![0x22; 4096]);
        assert!(
            root.snapshot_volume("source", "snapshot").await.is_err(),
            "snapshot over a volume"
        );
        filesystem.sync(SyncOptions::default()).await.expect("sync failed");
        drop((root, source, snapshot));

        // So after a remount.
        filesystem.close().await.expect("Close failed");
        let device = filesystem.take_device().await;
        device.reopen(false);
        let filesystem = FxFilesystem::open(device).await.expect("open failed");
        do_fsck(&filesystem, Some("source"), None).await;
        do_fsck(&filesystem, Some("snapshot"), None).await;
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let source = root.volume("source", StoreOptions::default()).await.expect("volume failed");
        let snapshot =
            root.volume("snapshot", StoreOptions::default()).await.expect("volume failed");
        assert_eq!(read_file(&source, "file").await, vec![0xbb; 8192]);
        assert_eq!(read_file(&snapshot, "file").await, vec![0xaa; 8192]);
        assert_eq!(read_file(&source, "kept").await, vec![0x11; 4096]);
        assert_eq!(read_file(&snapshot, "kept").await, vec![0x22; 4096]);
        drop((root, source, snapshot));
        filesystem.close().await.expect("Close failed");
    }

    /// fxdax: the pages of a file that a snapshot shares are frozen, until written.
    #[fuchsia::test]
    async fn test_snapshot_frozen_pages() {
        const PAGE: u64 = 4096;
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
        let source = root
            .new_volume("source", NewChildStoreOptions::default())
            .await
            .expect("new_volume failed");
        write_file(&filesystem, &source, "file", &[0xaa; 4 * PAGE as usize]).await;
        let frozen_pages = async |store: &Arc<ObjectStore>| {
            open_file(store, "file").await.frozen_pages(0..4 * PAGE, PAGE).await.unwrap()
        };
        assert_eq!(frozen_pages(&source).await, vec![]);

        let snapshot =
            root.snapshot_volume("source", "snapshot").await.ok().expect("snapshot failed");
        let owner = filesystem.allocator().shared_owners(source.store_object_id())[0];
        let pages = frozen_pages(&snapshot).await;
        assert_eq!(pages.len(), 4);
        for (i, page) in pages.iter().enumerate() {
            assert_eq!(page.offset, i as u64 * PAGE);
            assert_eq!(page.extent_offset, pages[0].extent_offset + i as u64 * PAGE);
            assert_eq!(page.owner_object_id, owner);
        }
        assert_eq!(frozen_pages(&source).await, pages);

        // A page written is the source's own.
        write_file(&filesystem, &source, "file", &[0xbb; PAGE as usize]).await;
        assert_eq!(frozen_pages(&source).await, pages[1..]);
        assert_eq!(frozen_pages(&snapshot).await, pages);

        let file = open_file(&source, "file").await;
        assert_eq!(file.frozen_pages(PAGE..3 * PAGE, PAGE).await.unwrap(), pages[1..3]);
        // Pages larger than blocks are frozen if all their blocks are, in one extent.
        assert_eq!(file.frozen_pages(0..4 * PAGE, 2 * PAGE).await.unwrap(), [pages[2]]);
        assert!(file.frozen_pages(1..PAGE, PAGE).await.is_err());
        drop((file, root, source, snapshot));
        filesystem.close().await.expect("Close failed");
    }

    /// fxdax: a snapshot replays from the journal.
    #[fuchsia::test]
    async fn test_snapshot_volume_replays() {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let crashed = {
            let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let source = root
                .new_volume("source", NewChildStoreOptions::default())
                .await
                .expect("new_volume failed");
            write_file(&filesystem, &source, "file", &[0xaa; 8192]).await;
            let snapshot =
                root.snapshot_volume("source", "snapshot").await.ok().expect("snapshot failed");
            write_file(&filesystem, &source, "file", &[0xbb; 8192]).await;
            write_file(&filesystem, &snapshot, "new", &[0xcc; 4096]).await;
            filesystem
                .sync(SyncOptions { flush_device: true, ..Default::default() })
                .await
                .expect("sync failed");
            filesystem.device().snapshot().expect("device snapshot failed")
        };
        filesystem.close().await.expect("Close failed");

        let filesystem = FxFilesystem::open(crashed).await.expect("open failed");
        do_fsck(&filesystem, Some("source"), None).await;
        do_fsck(&filesystem, Some("snapshot"), None).await;
        {
            let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let source =
                root.volume("source", StoreOptions::default()).await.expect("volume failed");
            let snapshot =
                root.volume("snapshot", StoreOptions::default()).await.expect("volume failed");
            assert_eq!(read_file(&source, "file").await, vec![0xbb; 8192]);
            assert_eq!(read_file(&snapshot, "file").await, vec![0xaa; 8192]);
            assert_eq!(read_file(&snapshot, "new").await, vec![0xcc; 4096]);
            // A new file in the source doesn't take an object id the snapshot's owner has.
            write_file(&filesystem, &source, "after", &[0xdd; 4096]).await;
        }
        do_fsck(&filesystem, Some("source"), None).await;
        filesystem.close().await.expect("Close failed");
    }

    /// fxdax: a snapshot is atomic with its source's commits (TASK-241): writes of the source
    /// racing it, each overwriting a file whole, which frees the extents it overwrote, are in it
    /// or not, each file whole, and free nothing it shares; the snapshot reads back, passes fsck,
    /// and so after a remount. A commit between the snapshot's flush and its own freed extents
    /// the snapshot shared.
    #[fuchsia::test]
    async fn test_snapshot_volume_racing_writes() {
        const FILES: u8 = 8;
        const LEN: usize = 8192;
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let name = |file: u8| format!("file-{file}");
        {
            let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let source = root
                .new_volume("source", NewChildStoreOptions::default())
                .await
                .expect("new_volume failed");
            for file in 0..FILES {
                write_file(&filesystem, &source, &name(file), &[0; LEN]).await;
            }
            let writes = async {
                for round in 1..=4u8 {
                    for file in 0..FILES {
                        write_file(&filesystem, &source, &name(file), &[round; LEN]).await;
                    }
                }
            };
            let (snapshot, ()) = futures::join!(root.snapshot_volume("source", "snapshot"), writes);
            let snapshot = snapshot.expect("snapshot failed");
            for file in 0..FILES {
                let held = read_file(&snapshot, &name(file)).await;
                assert!(held.len() == LEN && held.iter().all(|byte| *byte == held[0]), "torn");
                assert_eq!(read_file(&source, &name(file)).await, vec![4; LEN]);
            }
            filesystem.sync(SyncOptions::default()).await.expect("sync failed");
        }
        do_fsck(&filesystem, Some("source"), None).await;
        do_fsck(&filesystem, Some("snapshot"), None).await;
        filesystem.close().await.expect("Close failed");
        let device = filesystem.take_device().await;
        device.reopen(false);
        let filesystem = FxFilesystem::open(device).await.expect("open failed");
        do_fsck(&filesystem, Some("snapshot"), None).await;
        {
            let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let snapshot =
                root.volume("snapshot", StoreOptions::default()).await.expect("volume failed");
            for file in 0..FILES {
                assert_eq!(read_file(&snapshot, &name(file)).await.len(), LEN);
            }
        }
        filesystem.close().await.expect("Close failed");
    }

    /// fxdax: what a snapshot shares is freed once neither it nor its source references it, in
    /// either order of deletion.
    #[test_case(true; "source first")]
    #[test_case(false; "snapshot first")]
    #[fuchsia::test]
    async fn test_snapshot_volume_deletion_frees_space(source_first: bool) {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let allocator = filesystem.allocator();
        let baseline = allocator.get_owner_allocated_bytes();
        {
            let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let source = root
                .new_volume("source", NewChildStoreOptions::default())
                .await
                .expect("new_volume failed");
            write_file(&filesystem, &source, "shared", &[0xaa; 65536]).await;
            let snapshot =
                root.snapshot_volume("source", "snapshot").await.ok().expect("snapshot failed");
            write_file(&filesystem, &source, "own", &[0xbb; 16384]).await;
            write_file(&filesystem, &snapshot, "own", &[0xcc; 16384]).await;
            // Overwriting shared data leaves it shared: the other still references it.
            write_file(&filesystem, &source, "shared", &[0xdd; 65536]).await;
            assert_eq!(read_file(&snapshot, "shared").await, vec![0xaa; 65536]);
        }
        filesystem.sync(SyncOptions::default()).await.expect("sync failed");

        let (first, second) =
            if source_first { ("source", "snapshot") } else { ("snapshot", "source") };
        delete_volume(&filesystem, first).await;
        do_fsck(&filesystem, Some(second), None).await;
        {
            let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let left = root.volume(second, StoreOptions::default()).await.expect("volume failed");
            let shared = read_file(&left, "shared").await;
            assert_eq!(shared, vec![if source_first { 0xaa } else { 0xdd }; 65536]);
        }
        assert_eq!(allocator.frozen_owners().len(), 1, "the shared data has a sharer left");

        delete_volume(&filesystem, second).await;
        do_fsck(&filesystem, None, None).await;
        assert!(allocator.frozen_owners().is_empty());
        assert_eq!(allocator.get_owner_allocated_bytes(), baseline);

        // And on disk.
        filesystem.close().await.expect("Close failed");
        let device = filesystem.take_device().await;
        device.reopen(false);
        let filesystem = FxFilesystem::open(device).await.expect("open failed");
        do_fsck(&filesystem, None, None).await;
        assert_eq!(filesystem.allocator().get_owner_allocated_bytes(), baseline);
        filesystem.close().await.expect("Close failed");
    }

    /// fxdax: overwriting a preallocated file's blocks that a snapshot shares writes new blocks,
    /// leaving the snapshot's as they were.
    #[fuchsia::test]
    async fn test_snapshot_volume_overwrite_extents() {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let block = filesystem.block_size().get() as usize;
        {
            let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let source = root
                .new_volume("source", NewChildStoreOptions::default())
                .await
                .expect("new_volume failed");
            write_file(&filesystem, &source, "file", &[]).await;
            let root_directory = Directory::open(&source, source.root_directory_object_id())
                .await
                .expect("open failed");
            let (object_id, _, _) =
                root_directory.lookup("file").await.expect("lookup failed").expect("not found");
            let handle =
                ObjectStore::open_object(&source, object_id, HandleOptions::default(), None)
                    .await
                    .expect("open_object failed");
            handle.truncate(10 * block as u64).await.expect("truncate failed");
            handle.allocate(0..10 * block as u64).await.expect("allocate failed");
            let overwrite = |range: std::ops::Range<usize>, byte: u8| {
                let handle = &handle;
                async move {
                    let mut buf = handle.allocate_buffer(range.end - range.start).await;
                    buf.as_mut().fill(byte);
                    let mut transaction = handle.new_transaction().await.expect("transaction");
                    handle
                        .multi_overwrite(
                            &mut transaction,
                            crate::object_store::AttributeId::DATA,
                            &[range.start as u64..range.end as u64],
                            buf.as_mut(),
                        )
                        .await
                        .expect("multi_overwrite failed");
                    transaction.commit().await.expect("commit failed");
                }
            };
            overwrite(0..2 * block, 0xaa).await;
            let snapshot =
                root.snapshot_volume("source", "snapshot").await.ok().expect("snapshot failed");
            // A file open across the snapshot, and an unwritten block.
            overwrite(0..block, 0xbb).await;
            overwrite(5 * block..6 * block, 0xcc).await;

            let mut expected_snapshot = vec![0; 10 * block];
            expected_snapshot[..2 * block].fill(0xaa);
            let mut expected_source = expected_snapshot.clone();
            expected_source[..block].fill(0xbb);
            expected_source[5 * block..6 * block].fill(0xcc);
            assert_eq!(read_file(&snapshot, "file").await, expected_snapshot);
            assert_eq!(read_file(&source, "file").await, expected_source);
        }
        filesystem.sync(SyncOptions::default()).await.expect("sync failed");
        do_fsck(&filesystem, Some("source"), None).await;
        do_fsck(&filesystem, Some("snapshot"), None).await;
        delete_volume(&filesystem, "snapshot").await;
        do_fsck(&filesystem, Some("source"), None).await;
        {
            let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let source =
                root.volume("source", StoreOptions::default()).await.expect("volume failed");
            let contents = read_file(&source, "file").await;
            assert_eq!(&contents[..block], &vec![0xbb; block][..]);
            assert_eq!(&contents[block..2 * block], &vec![0xaa; block][..]);
            assert_eq!(&contents[5 * block..6 * block], &vec![0xcc; block][..]);
        }
        filesystem.close().await.expect("Close failed");
    }

    /// fxdax: snapshots of snapshots share what each was made from, and deleting them in any order
    /// frees everything once the last is gone.
    #[fuchsia::test]
    async fn test_snapshot_volume_chain() {
        let device = DeviceHolder::new(FakeDevice::new(16384, 512));
        let filesystem = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let allocator = filesystem.allocator();
        let baseline = allocator.get_owner_allocated_bytes();
        {
            let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let source = root
                .new_volume("source", NewChildStoreOptions::default())
                .await
                .expect("new_volume failed");
            write_file(&filesystem, &source, "a", &[1; 16384]).await;
            let first =
                root.snapshot_volume("source", "first").await.ok().expect("snapshot failed");
            write_file(&filesystem, &first, "b", &[2; 16384]).await;
            let second =
                root.snapshot_volume("first", "second").await.ok().expect("snapshot failed");
            write_file(&filesystem, &second, "a", &[3; 16384]).await;
            write_file(&filesystem, &first, "b", &[4; 16384]).await;
            assert_eq!(read_file(&source, "a").await, vec![1; 16384]);
            assert_eq!(read_file(&first, "a").await, vec![1; 16384]);
            assert_eq!(read_file(&first, "b").await, vec![4; 16384]);
            assert_eq!(read_file(&second, "a").await, vec![3; 16384]);
            assert_eq!(read_file(&second, "b").await, vec![2; 16384]);
        }
        filesystem.sync(SyncOptions::default()).await.expect("sync failed");
        for volume in ["source", "first", "second"] {
            do_fsck(&filesystem, Some(volume), None).await;
        }
        delete_volume(&filesystem, "first").await;
        do_fsck(&filesystem, Some("source"), None).await;
        do_fsck(&filesystem, Some("second"), None).await;
        {
            let root = root_volume(filesystem.clone()).await.expect("root_volume failed");
            let second =
                root.volume("second", StoreOptions::default()).await.expect("volume failed");
            assert_eq!(read_file(&second, "b").await, vec![2; 16384]);
        }
        delete_volume(&filesystem, "source").await;
        do_fsck(&filesystem, Some("second"), None).await;
        delete_volume(&filesystem, "second").await;
        do_fsck(&filesystem, None, None).await;
        assert!(allocator.frozen_owners().is_empty());
        assert_eq!(allocator.get_owner_allocated_bytes(), baseline);
        filesystem.close().await.expect("Close failed");
    }
}
