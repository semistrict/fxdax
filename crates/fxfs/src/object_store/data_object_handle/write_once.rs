// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own, a child of data_object_handle.rs: data lives in extents of an extent store, written
//! once (backlog doc-4). `overwrite` writes blocks in place only as `in_place` says, which refuses
//! those a snapshot or a clone shares; `reallocate_range` moves a range written in place already,
//! the journal's, to new extents.

use super::DataObjectHandle;
use crate::errors::FxfsError;
use crate::object_handle::ObjectHandle;
use crate::object_store::HandleOwner;
use crate::object_store::extent_record::{ExtentMode, ExtentValue};
use crate::object_store::object_record::{ObjectKey, ObjectValue};
use crate::object_store::transaction::{Mutation, Transaction};
use anyhow::{Context, Error, ensure};
use std::cmp::min;
use std::ops::Range;
use storage_device::ExtentRange;

/// fxdax: whether a file's blocks may be written in place, and if not, why: the rule
/// ([`DataObjectHandle::in_place`]) for `overwrite`, which writes them through the device
/// (TASK-160.22).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum InPlace {
    /// They may: their record is raw, and theirs alone.
    Writable,
    /// Their record keeps something of what they hold, which a write in place would leave stale:
    /// checksums (`ExtentMode::Cow`), which a replay of the journal checks a transaction's blocks
    /// against (barriers off), stopping where they don't match; or that they are an allocation's
    /// (`ExtentMode::Overwrite`), and which of them were written (`OverwritePartial`), the others
    /// reading as zeros. Fxfs's own writes of them go to new blocks (`multi_overwrite`).
    Recorded,
    /// Another record shares them: a clone's or a snapshot's (`Allocator::is_shared`).
    Shared,
}

impl<S: HandleOwner> DataObjectHandle<S> {
    /// fxdax: whether `blocks` of the file, which a record of `mode` holds, may be written in place
    /// (`InPlace`), as `overwrite` writes them: whether their extent is open isn't asked, as the
    /// device takes or refuses its writes itself.
    pub(super) async fn in_place(
        &self,
        mode: &ExtentMode,
        blocks: &ExtentRange,
    ) -> Result<InPlace, Error> {
        Ok(if !matches!(mode, ExtentMode::Raw) {
            InPlace::Recorded
        } else if self.store().allocator().is_shared(blocks.clone()).await? {
            InPlace::Shared
        } else {
            InPlace::Writable
        })
    }

    /// fxdax: replaces the extents in `file_range` with new ones, preallocated, of at most
    /// `max_extent_len` bytes each, freeing those it had: for a file written in place, such as
    /// the journal, whose extents there may have been written already, and can't be again (backlog
    /// doc-4). The range must be within the file.
    pub async fn reallocate_range<'a>(
        &'a self,
        transaction: &mut Transaction<'a>,
        file_range: Range<u64>,
        max_extent_len: u64,
    ) -> Result<Vec<ExtentRange>, Error> {
        ensure!(self.block_size().is_aligned(&file_range), FxfsError::InvalidArgs);
        ensure!(!self.handle.is_encrypted(), FxfsError::NotSupported);
        let deallocated = self
            .handle
            .deallocate_old_extents(transaction, self.attribute_id(), file_range.clone())
            .await?;
        let key_id = self.get_key(None).await?.0;
        let mut ranges = Vec::new();
        let mut allocated = 0;
        let mut offset = file_range.start;
        while offset < file_range.end {
            let device_range = self
                .store()
                .allocator()
                .allocate(
                    transaction,
                    self.store().store_object_id(),
                    min(file_range.end - offset, max_extent_len),
                )
                .await
                .context("Allocation failed")?;
            allocated += device_range.len();
            transaction.add(
                self.store().store_object_id,
                Mutation::merge_object(
                    ObjectKey::extent(
                        self.object_id(),
                        self.attribute_id(),
                        offset..offset + device_range.len(),
                    ),
                    ObjectValue::Extent(ExtentValue::new_raw(
                        device_range.extent,
                        device_range.range.start,
                        key_id,
                    )),
                ),
            );
            offset += device_range.len();
            ranges.push(device_range);
        }
        self.update_allocated_size(transaction, allocated, deallocated).await?;
        Ok(ranges)
    }
}

#[cfg(test)]
mod tests {
    use crate::filesystem::FxFilesystem;
    use crate::fsck::fsck;
    use crate::object_handle::{ObjectHandle, WriteObjectHandle};
    use crate::object_store::AttributeId;
    use crate::object_store::data_object_handle::tests::{
        TEST_DEVICE_BLOCK_SIZE, create_object_with_key,
    };
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;

    /// fxdax: random single-block overwrites of a large allocated file, each in a transaction of
    /// its own (as fxfs-platform flushes overwrite ranges), read back, with the journal compacted.
    /// Each overwrite writes new blocks, and the old ones' extents live on while any of their
    /// blocks does.
    #[fuchsia::test]
    async fn test_random_overwrites_of_an_allocated_file() {
        const FILE_SIZE: u64 = 128 << 20;
        const WRITES: usize = 8192;
        let device = DeviceHolder::new(FakeDevice::new(1 << 20, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let object = create_object_with_key(fs.clone(), None, false).await;
        object.truncate(FILE_SIZE).await.expect("truncate failed");
        object.allocate(0..FILE_SIZE).await.expect("allocate failed");
        let bs = fs.block_size().get();
        let mut buf = object.allocate_buffer(bs as usize).await;
        buf.as_mut().fill(7);
        // A fixed pseudo-random sequence of blocks.
        let mut seed: u64 = 0x2545f4914f6cdd1d;
        let mut written = std::collections::HashSet::new();
        for _ in 0..WRITES {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let offset = (seed % (FILE_SIZE / bs)) * bs;
            let mut transaction = object.new_transaction().await.expect("new_transaction failed");
            object
                .multi_overwrite(
                    &mut transaction,
                    AttributeId::DATA,
                    &[offset..offset + bs],
                    buf.as_mut(),
                )
                .await
                .expect("multi_overwrite failed");
            transaction.commit().await.expect("commit failed");
            written.insert(offset);
        }
        fs.journal().force_compact().await.expect("compact failed");
        let mut read = object.allocate_buffer(bs as usize).await;
        for offset in (0..FILE_SIZE).step_by(bs as usize) {
            object.read_aligned(offset, read.as_mut()).await.expect("read failed");
            let byte = if written.contains(&offset) { 7 } else { 0 };
            assert_eq!(read.to_vec(), vec![byte; bs as usize], "block at {offset}");
        }
        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("close failed");
    }
}
