// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Of Fuchsia's `src/storage/fxfs/src/object_store/data_object_handle.rs`, a child of it: zeroing
// what a file's last block holds past its end, upstream inline in `grow`, as every way a file
// grows does, and a truncate, which leaves it there (TASK-139, TASK-150.19, TASK-153.4). The block
// is read as every read reads, not from its extent with upstream's `read_and_decrypt`, gone with
// it (TASK-149.36).

use super::DataObjectHandle;
use crate::object_handle::{ObjectHandle, ReadObjectHandle};
use crate::object_store::HandleOwner;
use crate::object_store::transaction::Transaction;
use anyhow::Error;
use std::ops::Range;

impl<S: HandleOwner> DataObjectHandle<S> {
    /// fxdax: zeros what the last block of a file of `size` bytes holds past them, unless it is
    /// zeros already, as it nearly always is: says whether it wrote the block. A growth's zeroing
    /// writes back no block its writes didn't reserve room for but one a cut left so, whose room a
    /// writeback paid with its dirty pages' counts (`growth_zeroes_tail`; TASK-80, TASK-153.4,
    /// TASK-149.29). No extent of the file may start past its last block.
    pub(super) async fn txn_zero_tail<'a>(
        &'a self,
        transaction: &mut Transaction<'a>,
        size: u64,
    ) -> Result<bool, Error> {
        let block_size = self.block_size();
        if block_size.is_aligned(size) {
            return Ok(false);
        }
        let last = block_size.align_down(size);
        let mut buf = self.allocate_buffer(block_size.get() as usize).await;
        // As every read reads it: a hole, and a block allocated and never written, which the
        // device doesn't hold, as zeros (TASK-149.36).
        self.handle.read_aligned_locked(self.attribute_id(), last, buf.as_mut()).await?;
        let mut past = buf.subslice_mut((size % block_size) as usize..buf.len());
        if past.to_vec().iter().all(|&byte| byte == 0) {
            return Ok(false);
        }
        past.fill(0);
        self.multi_write(
            transaction,
            self.attribute_id(),
            &[last..last + block_size],
            buf.as_mut(),
        )
        .await?;
        Ok(true)
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
        if self.growth_zeroes_tail(old, size, replaced.iter().cloned()) {
            self.txn_zero_tail(transaction, old).await?;
        }
        self.txn_update_size(transaction, size, None).await
    }

    /// fxdax: whether a growth of the file from `old` bytes to `size` zeros its last block past
    /// `old`: if `old` is within it, unless `replaced`, the blocks it writes anew, has it. Where the
    /// block holds zeros there, as it nearly always does, the zeroing writes nothing
    /// (`txn_zero_tail`); where a cut left what it held (SET_KEEP_TAIL, a DAX file's), the block is
    /// written again, which room reserved for the growth's own blocks alone didn't pay for: a
    /// writeback paid with its dirty pages' room failed NoSpace (TASK-149.29). What decides whether
    /// a growth zeroes, and what its room counts (`write_many_paid`).
    pub(super) fn growth_zeroes_tail(
        &self,
        old: u64,
        size: u64,
        mut replaced: impl Iterator<Item = Range<u64>>,
    ) -> bool {
        let block_size = self.block_size();
        let last = block_size.align_down(old);
        size > old
            && !block_size.is_aligned(old)
            && !replaced.any(|blocks| blocks.start <= last && last < blocks.end)
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
        if !self.growth_zeroes_tail(old, size, replaced.into_iter()) {
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

#[cfg(test)]
mod tests {
    use crate::object_handle::WriteObjectHandle;
    use crate::object_store::data_object_handle::tests::test_filesystem_and_object_with_key;

    /// fxdax: a file allocated part of a block grows: the block, which nothing wrote, isn't on the
    /// device, and reads as zeros, so the growth has nothing to zero. Read from its extent, the
    /// growth failed, "no extent" (TASK-149.36).
    #[fuchsia::test]
    async fn test_a_file_allocated_part_of_a_block_grows() {
        let (fs, object) = test_filesystem_and_object_with_key(None, false).await;
        // Past what small files share of an extent (the staging extent), so that the allocation
        // takes extents of its own, which nothing wrote.
        let end = (1 << 20) + 1;
        object.allocate(0..end).await.expect("allocate failed");
        object.truncate(end + 1).await.expect("truncate failed");
        let contents = object.contents(usize::MAX).await.expect("contents failed");
        assert_eq!(contents[..], vec![0; end as usize + 1][..]);
        fs.close().await.expect("close failed");
    }
}
