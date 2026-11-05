// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own, a child of data_object_handle.rs: a reflink (FICLONE, FICLONERANGE,
//! copy_file_range) shares a file's extents with another file's range, copying no data.

use super::DataObjectHandle;
use crate::errors::FxfsError;
use crate::lsm_tree::Query;
use crate::lsm_tree::types::{ItemRef, LayerIterator};
use crate::object_handle::ObjectHandle;
use crate::object_store::extent_record::{ExtentMode, ExtentValue};
use crate::object_store::object_record::{AttributeKey, ObjectKey, ObjectKeyData, ObjectValue};
use crate::object_store::transaction::{Mutation, Transaction};
use crate::object_store::{Extent, HandleOwner};
use anyhow::{Error, anyhow, ensure};
use std::cmp::{max, min};
use std::ops::Range;
use storage_device::ExtentRange;

impl<S: HandleOwner> DataObjectHandle<S> {
    /// fxdax: makes `dst`'s bytes from `dst_offset` share the extents holding this file's bytes
    /// `range` (a reflink: FICLONE, FICLONERANGE, copy_file_range), copying no data: `dst`'s
    /// extents there go, and this file's take their place, each referenced once more
    /// (`Allocator::reference`), its holes holes there. Writes to either file afterwards move the
    /// blocks they change (`Allocator::is_shared`). Both files are this store's, unencrypted and
    /// not verified, and the caller locks them; `range` and `dst_offset` are block aligned, and if
    /// `dst` is this file, the ranges don't overlap. The caller updates `dst`'s size. Extents
    /// written in place (overwrite modes), which can't be shared, fail with NotSupported.
    pub async fn txn_clone_range(
        &self,
        transaction: &mut Transaction<'_>,
        range: Range<u64>,
        dst: &DataObjectHandle<S>,
        dst_offset: u64,
    ) -> Result<(), Error> {
        ensure!(range.start <= range.end, FxfsError::InvalidArgs);
        let len = range.end - range.start;
        let dst_range = dst_offset..dst_offset.checked_add(len).ok_or(FxfsError::TooBig)?;
        let block_size = self.block_size();
        ensure!(
            block_size.is_aligned(&range) && block_size.is_aligned(&dst_range),
            FxfsError::InvalidArgs
        );
        ensure!(
            self.store().store_object_id() == dst.store().store_object_id(),
            FxfsError::InvalidArgs
        );
        ensure!(
            dst.object_id() != self.object_id()
                || dst_range.end <= range.start
                || range.end <= dst_range.start,
            FxfsError::InvalidArgs
        );
        for file in [self, dst] {
            ensure!(
                !file.handle.is_encrypted() && !file.is_verified_file(),
                FxfsError::NotSupported
            );
        }
        if len == 0 {
            return Ok(());
        }
        let store = self.store();

        // This file's extents in the range, as they are before `dst` changes, which may be it.
        let mut parts = Vec::new();
        {
            let layer_set = store.tree().layer_set();
            let mut merger = layer_set.merger();
            let mut iter = merger
                .query(Query::FullRange(&ObjectKey::attribute(
                    self.object_id(),
                    self.attribute_id(),
                    AttributeKey::Extent(Extent(range.clone()).search_key()),
                )))
                .await?;
            while let Some(ItemRef {
                key:
                    ObjectKey {
                        object_id,
                        data: ObjectKeyData::Attribute(attribute_id, AttributeKey::Extent(key)),
                    },
                value: ObjectValue::Extent(value),
                ..
            }) = iter.get()
                && *object_id == self.object_id()
                && *attribute_id == self.attribute_id()
                && key.start < range.end
            {
                if let ExtentValue::Some { mode, .. } = value
                    && key.end > range.start
                {
                    ensure!(
                        matches!(mode, ExtentMode::Raw | ExtentMode::Cow(_)),
                        anyhow!(FxfsError::NotSupported)
                            .context("sharing an extent written in place")
                    );
                    let (start, end) = (max(key.start, range.start), min(key.end, range.end));
                    let extent_len = key.end - key.start;
                    let value = value
                        .offset_by(start - key.start, extent_len)
                        .shrunk(key.end - start, end - start);
                    parts.push((start..end, value));
                }
                iter.advance().await?;
            }
        }

        let deallocated = dst
            .handle
            .deallocate_old_extents(transaction, dst.attribute_id(), dst_range.clone())
            .await?;
        let allocator = store.allocator();
        let mut allocated = 0;
        // `dst`'s range, in order: holes where this file's has none, and its extents.
        let mut offset = dst_range.start;
        let record = |transaction: &mut Transaction<'_>, logical: Range<u64>, value| {
            transaction.add(
                store.store_object_id(),
                Mutation::merge_object(
                    ObjectKey::extent(dst.object_id(), dst.attribute_id(), logical),
                    ObjectValue::Extent(value),
                ),
            );
        };
        for (part, value) in parts {
            let at = dst_offset + (part.start - range.start);
            let part_len = part.end - part.start;
            if offset < at {
                record(transaction, offset..at, ExtentValue::deleted_extent());
            }
            let ExtentValue::Some { extent, extent_offset, .. } = &value else {
                unreachable!("extents alone are kept");
            };
            let device_range = ExtentRange::new(*extent, *extent_offset..*extent_offset + part_len);
            allocator.reference(transaction, store.store_object_id(), device_range);
            record(transaction, at..at + part_len, value);
            allocated += part_len;
            offset = at + part_len;
        }
        if offset < dst_range.end {
            record(transaction, offset..dst_range.end, ExtentValue::deleted_extent());
        }
        dst.update_allocated_size(transaction, allocated, deallocated).await
    }
}

#[cfg(test)]
mod tests {
    use crate::filesystem::{FxFilesystem, OpenFxFilesystem};
    use crate::fsck::fsck;
    use crate::object_handle::{ObjectHandle, ReadObjectHandle, WriteObjectHandle};
    use crate::object_store::data_object_handle::tests::test_filesystem;
    use crate::object_store::directory::{Directory, replace_child};
    use crate::object_store::transaction::{Options, lock_keys};
    use crate::object_store::{DataObjectHandle, HandleOptions, LockKey, ObjectStore};
    use std::ops::Range;
    use std::sync::Arc;

    /// fxdax: files to clone between, in the root store's root directory.
    struct Clones {
        fs: OpenFxFilesystem,
        store: Arc<ObjectStore>,
        root: Directory<ObjectStore>,
    }

    impl Clones {
        async fn new() -> Self {
            let fs = test_filesystem().await;
            let store = fs.root_store();
            let root =
                Directory::open(&store, store.root_directory_object_id()).await.expect("open");
            Self { fs, store, root }
        }

        async fn create(&self, name: &str) -> DataObjectHandle<ObjectStore> {
            let lock = LockKey::object(self.store.store_object_id(), self.root.object_id());
            let mut transaction = self
                .store
                .new_transaction(lock_keys![lock], Options::default())
                .await
                .expect("new_transaction failed");
            let object = self.root.create_child_file(&mut transaction, name).await.expect("create");
            transaction.commit().await.expect("commit failed");
            object
        }

        async fn remove(&self, name: &str, object_id: u64) {
            let store_id = self.store.store_object_id();
            let keys = lock_keys![
                LockKey::object(store_id, self.root.object_id()),
                LockKey::object(store_id, object_id),
            ];
            let mut transaction = self
                .store
                .new_transaction(keys, Options::default())
                .await
                .expect("new_transaction failed");
            replace_child(&mut transaction, None, (&self.root, name)).await.expect("replace");
            transaction.commit().await.expect("commit failed");
            self.store
                .tombstone_object(object_id, Options::default(), None)
                .await
                .expect("tombstone failed");
        }

        async fn write(&self, object: &DataObjectHandle<ObjectStore>, offset: u64, data: &[u8]) {
            let mut buf = object.allocate_buffer(data.len()).await;
            buf.subslice_mut(0..data.len()).copy_from_slice(data);
            object.write_or_append(Some(offset), buf.as_ref()).await.expect("write failed");
        }

        /// Clones `source`'s `range` to `dst` at `dst_offset`, growing `dst` to hold it.
        async fn clone(
            &self,
            source: &DataObjectHandle<ObjectStore>,
            range: Range<u64>,
            dst: &DataObjectHandle<ObjectStore>,
            dst_offset: u64,
        ) {
            let store_id = self.store.store_object_id();
            let mut keys = lock_keys![LockKey::object(store_id, source.object_id())];
            if dst.object_id() != source.object_id() {
                keys.push(LockKey::object(store_id, dst.object_id()));
            }
            let mut transaction = self
                .store
                .new_transaction(keys, Options::default())
                .await
                .expect("new_transaction failed");
            let end = dst_offset + range.end - range.start;
            source.txn_clone_range(&mut transaction, range, dst, dst_offset).await.expect("clone");
            if end > dst.txn_get_size(&transaction) {
                dst.txn_update_size(&mut transaction, end, None).await.expect("size failed");
            }
            transaction.commit().await.expect("commit failed");
        }
    }

    /// fxdax: a clone across a hole takes no more commits however long the hole is: counted as
    /// bytes to clone, one of exabytes (xfstests' generic/303) committed for ever, and the journal,
    /// which couldn't keep up, panicked the guest with a write larger than its transfer buffer.
    #[fuchsia::test]
    async fn test_clone_range_crosses_holes_at_once() {
        const FAR: u64 = 1 << 50;
        let clones = Clones::new().await;
        let bs = clones.fs.block_size().get();
        let data = vec![7; bs as usize];
        let source = clones.create("source").await;
        let target = clones.create("target").await;
        clones.write(&source, 0, &data).await;
        clones.write(&source, FAR, &data).await;
        let store_id = clones.store.store_object_id();
        let keys = lock_keys![
            LockKey::object(store_id, source.object_id()),
            LockKey::object(store_id, target.object_id()),
        ];
        let mut transaction = clones
            .store
            .new_transaction(keys, Options::default())
            .await
            .expect("new_transaction failed");
        let end = FAR + bs;
        let before = clones.fs.journal().file_offset();
        source
            .clone_range_in_commits(&mut transaction, 0..end, &target, 0, end)
            .await
            .expect("clone failed");
        target.txn_update_size(&mut transaction, end, None).await.expect("size failed");
        transaction.commit().await.expect("commit failed");
        let journaled = clones.fs.journal().file_offset() - before;
        assert!(journaled < 64 << 10, "{journaled} bytes of journal to cross a hole");
        assert_eq!(target.get_size(), end);
        let mut buffer = target.allocate_buffer(bs as usize).await;
        target.read_aligned(FAR, buffer.as_mut()).await.expect("read failed");
        assert_eq!(buffer.to_vec(), data);
    }

    /// fxdax: a clone of a file's range, holes and all, shares its extents, copying nothing, in
    /// place of what the clone held there: each file reads as it should, before and after either is
    /// written, which moves the blocks it changes; fsck finds nothing wrong, after a remount too;
    /// and the extents are freed once neither file holds them.
    #[fuchsia::test]
    async fn test_clone_range() {
        let clones = Clones::new().await;
        let bs = clones.fs.block_size().get();
        let allocator = clones.fs.allocator();
        let empty = allocator.get_allocated_bytes();
        let source = clones.create("source").await;
        let data: Vec<u8> = (0..4 * bs).map(|i| (i % 251) as u8).collect();
        clones.write(&source, 0, &data).await;
        // A hole, past the data.
        source.truncate(5 * bs).await.expect("truncate failed");
        let clone = clones.create("clone").await;
        clones.write(&clone, 0, &vec![9; 2 * bs as usize]).await;
        let written = allocator.get_allocated_bytes();

        clones.clone(&source, 0..5 * bs, &clone, 0).await;
        let mut expected = data.clone();
        expected.resize(5 * bs as usize, 0);
        assert_eq!(&clone.contents(usize::MAX).await.expect("contents")[..], &expected[..]);
        assert_eq!(allocator.get_allocated_bytes(), written - 2 * bs, "nothing copied");

        clones.write(&source, bs, &vec![1; bs as usize]).await;
        clones.write(&clone, 2 * bs, &vec![2; bs as usize]).await;
        let mut source_expected = expected.clone();
        source_expected[bs as usize..2 * bs as usize].fill(1);
        let mut clone_expected = expected.clone();
        clone_expected[2 * bs as usize..3 * bs as usize].fill(2);
        let check = async |source: &DataObjectHandle<ObjectStore>,
                           clone: &DataObjectHandle<ObjectStore>| {
            assert_eq!(
                &source.contents(usize::MAX).await.expect("contents")[..],
                &source_expected[..]
            );
            assert_eq!(
                &clone.contents(usize::MAX).await.expect("contents")[..],
                &clone_expected[..]
            );
        };
        check(&source, &clone).await;
        fsck(clones.fs.clone()).await.expect("fsck failed");

        let (source_id, clone_id) = (source.object_id(), clone.object_id());
        clones.fs.close().await.expect("close failed");
        let device = clones.fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");
        fsck(fs.clone()).await.expect("fsck failed");
        let store = fs.root_store();
        let root = Directory::open(&store, store.root_directory_object_id()).await.expect("open");
        let open = async |store: &Arc<ObjectStore>, object_id| {
            ObjectStore::open_object(store, object_id, HandleOptions::default(), None)
                .await
                .expect("open_object failed")
        };
        let (source, clone) = (open(&store, source_id).await, open(&store, clone_id).await);
        check(&source, &clone).await;
        drop((source, clone));

        let clones = Clones { fs, store, root };
        clones.remove("source", source_id).await;
        let clone = open(&clones.store, clone_id).await;
        assert_eq!(&clone.contents(usize::MAX).await.expect("contents")[..], &clone_expected[..]);
        drop(clone);
        clones.remove("clone", clone_id).await;
        assert_eq!(clones.fs.allocator().get_allocated_bytes(), empty);
        fsck(clones.fs.clone()).await.expect("fsck failed");
        clones.fs.close().await.expect("close failed");
    }

    /// fxdax: a file can share its extents with itself, three times, and give up every reference
    /// in one truncation, which frees them; fsck finds nothing wrong between. The references are
    /// read back from the allocator's layers, the journal compacted into them, after a remount
    /// (`test_clone_range` replays them from the journal).
    #[fuchsia::test]
    async fn test_clone_range_within_a_file() {
        let clones = Clones::new().await;
        let bs = clones.fs.block_size().get();
        let allocator = clones.fs.allocator();
        let file = clones.create("file").await;
        let empty = allocator.get_allocated_bytes();
        let data: Vec<u8> = (0..2 * bs).map(|i| (i % 253) as u8).collect();
        clones.write(&file, 0, &data).await;
        clones.clone(&file, 0..2 * bs, &file, 2 * bs).await;
        clones.clone(&file, 0..2 * bs, &file, 4 * bs).await;
        let expected = [data.clone(), data.clone(), data.clone()].concat();
        assert_eq!(&file.contents(usize::MAX).await.expect("contents")[..], &expected[..]);
        assert_eq!(allocator.get_allocated_bytes(), empty + 2 * bs);
        fsck(clones.fs.clone()).await.expect("fsck failed");

        let file_id = file.object_id();
        drop(file);
        clones.fs.journal().force_compact().await.expect("compact failed");
        clones.fs.close().await.expect("close failed");
        let device = clones.fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");
        fsck(fs.clone()).await.expect("fsck failed");
        let file =
            ObjectStore::open_object(&fs.root_store(), file_id, HandleOptions::default(), None)
                .await
                .expect("open_object failed");
        assert_eq!(&file.contents(usize::MAX).await.expect("contents")[..], &expected[..]);
        // The metadata's own allocations are as compaction left them.
        let allocated = fs.allocator().get_allocated_bytes();
        file.truncate(0).await.expect("truncate failed");
        assert_eq!(fs.allocator().get_allocated_bytes(), allocated - 2 * bs);
        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("close failed");
    }
}
