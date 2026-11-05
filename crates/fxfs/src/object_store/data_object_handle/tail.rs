// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Of Fuchsia's `src/storage/fxfs/src/object_store/data_object_handle.rs`, a child of it: zeroing
// what a file's last block holds past its end, upstream inline in `grow`, as every way a file
// grows does, and a truncate, which leaves it there (TASK-139, TASK-150.19, TASK-153.4).

use super::DataObjectHandle;
use crate::errors::FxfsError;
use crate::lsm_tree::Query;
use crate::lsm_tree::types::{ItemRef, LayerIterator};
use crate::object_handle::{ObjectHandle, ReadObjectHandle};
use crate::object_store::extent_record::ExtentValue;
use crate::object_store::object_record::{AttributeKey, ObjectKey, ObjectKeyData, ObjectValue};
use crate::object_store::transaction::Transaction;
use crate::object_store::{Extent, HandleOwner};
use anyhow::{Error, ensure};
use std::ops::Range;

impl<S: HandleOwner> DataObjectHandle<S> {
    /// fxdax: zeros what the last block of a file of `size` bytes holds past them, unless it is
    /// zeros already, as it nearly always is: says whether it wrote the block. A growth's zeroing
    /// writes back no block its writes didn't reserve room for, unless a cut left one so (TASK-80,
    /// TASK-153.4). No extent of the file may start past its last block.
    pub(super) async fn txn_zero_tail<'a>(
        &'a self,
        transaction: &mut Transaction<'a>,
        size: u64,
    ) -> Result<bool, Error> {
        let store = self.store();
        let block_size = self.block_size();
        if block_size.is_aligned(size) {
            return Ok(false);
        }
        let layer_set = store.tree.layer_set();
        let mut merger = layer_set.merger();
        let aligned_size = block_size.align_down(size);
        let iter = merger
            .query(Query::FullRange(&ObjectKey::attribute(
                self.object_id(),
                self.attribute_id(),
                AttributeKey::Extent(Extent::search_key_from_offset(aligned_size)),
            )))
            .await?;
        if let Some(ItemRef {
            key:
                ObjectKey {
                    object_id,
                    data: ObjectKeyData::Attribute(attribute_id, AttributeKey::Extent(extent_key)),
                },
            value: ObjectValue::Extent(ExtentValue::Some { extent, extent_offset, key_id, .. }),
            ..
        }) = iter.get()
        {
            if *object_id == self.object_id()
                && *attribute_id == self.attribute_id()
                && extent_key.start <= aligned_size
            {
                let device_offset = extent_offset
                    .checked_add(aligned_size - extent_key.start)
                    .ok_or(FxfsError::Inconsistent)?;
                ensure!(block_size.is_aligned(device_offset), FxfsError::Inconsistent);
                let mut buf = self.allocate_buffer(block_size.get() as usize).await;
                // In the case that this extent is in OverwritePartial mode, there is a
                // possibility that the last block is allocated, but not initialized yet, in
                // which case we don't actually need to bother zeroing out the tail. However,
                // it's not strictly incorrect to change uninitialized data, so we skip the
                // check and blindly do it to keep it simpler here.
                self.read_and_decrypt(*extent, device_offset, aligned_size, buf.as_mut(), *key_id)
                    .await?;
                let mut past = buf.subslice_mut((size % block_size) as usize..buf.len());
                if past.to_vec().iter().all(|&byte| byte == 0) {
                    return Ok(false);
                }
                past.fill(0);
                self.multi_write(
                    transaction,
                    *attribute_id,
                    &[aligned_size..aligned_size + block_size],
                    buf.as_mut(),
                )
                .await?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// fxdax: makes the file `size` bytes long if it is shorter, in `transaction`, which writes
    /// `replaced` of its blocks anew (block aligned): zeroing what its last block holds past its end,
    /// unless `replaced` has that block. What a cut left there is no data, and a cut with no room to
    /// zero it leaves it: the growth zeroes it, as `grow` does, every way a file grows but a DAX
    /// window's, which zeroes it in the window. A write past the end, a clone and an allocation
    /// brought it back (TASK-150.19, TASK-153.4).
    pub(super) async fn txn_grow_size<'a>(
        &'a self,
        transaction: &mut Transaction<'a>,
        size: u64,
        replaced: &[Range<u64>],
    ) -> Result<(), Error> {
        let old = self.txn_get_size(transaction);
        if size <= old {
            return Ok(());
        }
        let last = self.block_size().align_down(old);
        if !replaced.iter().any(|blocks| blocks.start <= last && last < blocks.end) {
            self.txn_zero_tail(transaction, old).await?;
        }
        self.txn_update_size(transaction, size, None).await
    }

    /// fxdax: ahead of a growth to `size` that finds the file's extents before it changes them, a
    /// clone's or an allocation's, zeros what the last block holds past the end, as `txn_grow_size`
    /// does, unless `replaced` has that block, and commits it: in one transaction, the growth would
    /// take the block the zeroing frees for the file's (TASK-153.4).
    pub(super) async fn zero_tail_before_growth<'a>(
        &'a self,
        transaction: &mut Transaction<'a>,
        size: u64,
        replaced: Option<Range<u64>>,
    ) -> Result<(), Error> {
        let old = self.txn_get_size(transaction);
        let last = self.block_size().align_down(old);
        if size <= old
            || self.block_size().is_aligned(old)
            || replaced.is_some_and(|blocks| blocks.start <= last && last < blocks.end)
        {
            return Ok(());
        }
        if self.txn_zero_tail(transaction, old).await? {
            transaction.commit_and_continue().await?;
        }
        Ok(())
    }

    /// fxdax: zeros what the file's last block holds past its end: a truncate leaves it there, for
    /// a write past the end to bring back (xfstests generic/130, TASK-139).
    pub async fn zero_tail(&self) -> Result<(), Error> {
        let size = self.get_size();
        if !self.block_size().is_aligned(size) {
            let mut transaction = self.new_transaction().await?;
            self.txn_zero_tail(&mut transaction, size).await?;
            transaction.commit().await?;
        }
        Ok(())
    }
}
