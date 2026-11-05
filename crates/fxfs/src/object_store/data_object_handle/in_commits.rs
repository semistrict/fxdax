// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own, a child of data_object_handle.rs: operations on a range of however many extents,
//! a clone and a punch, in as many commits as hold them. What splits them across commits is the
//! room their transaction says it has left (`Transaction::room`), each extent costing what its
//! records do, holes nothing: not a count of mutations, nor of bytes.

use super::DataObjectHandle;
use crate::lsm_tree::Query;
use crate::lsm_tree::types::{ItemRef, LayerIterator};
use crate::object_handle::ObjectHandle;
use crate::object_store::extent_record::ExtentValue;
use crate::object_store::journal::mutation_record_bytes;
use crate::object_store::object_record::{AttributeKey, ObjectKey, ObjectKeyData, ObjectValue};
use crate::object_store::transaction::{AllocatorMutation, Mutation, Transaction};
use crate::object_store::{Extent, HandleOwner};
use anyhow::Error;
use std::cmp::{max, min};
use std::ops::Range;
use storage_device::ExtentRange;

impl<S: HandleOwner> DataObjectHandle<S> {
    /// fxdax: `txn_clone_range` of a range of however many extents, in either file, in as many
    /// commits as hold them, from its start, `dst` growing towards `size` with them, and to it as
    /// the last commits: one that fails has cloned what it got to. `transaction` holds the caller's locks, and is left for it to
    /// commit. Each extent costs its record, which holds each of its blocks' checksums, a reference
    /// and a hole's record, and each of `dst`'s a record freeing it; holes nothing, however long:
    /// counted by their bytes, a clone across one of exabytes (generic/303) committed for ever, its
    /// commits outrunning the journal; and a dozen extents of 1 MiB, counted by how many they were,
    /// ran past what a transaction may be.
    pub async fn clone_range_in_commits<'a>(
        &'a self,
        transaction: &mut Transaction<'a>,
        range: Range<u64>,
        dst: &'a DataObjectHandle<S>,
        dst_offset: u64,
        size: u64,
    ) -> Result<(), Error> {
        let store = self.store();
        let (store_id, allocator_id) = (store.store_object_id(), store.allocator().object_id());
        let dst_of = |offset: u64| dst_offset + (offset - range.start);
        let shared = |part: &Range<u64>, blocks: ExtentRange, value: &ExtentValue| {
            let logical = dst_of(part.start)..dst_of(part.end);
            let extent = ObjectKey::extent(dst.object_id(), dst.attribute_id(), logical);
            let hole = ObjectValue::Extent(ExtentValue::deleted_extent());
            let hole = Mutation::merge_object(extent.clone(), hole);
            let extent = Mutation::merge_object(extent, ObjectValue::Extent(value.clone()));
            let reference = Mutation::Allocator(AllocatorMutation::Reference {
                device_range: blocks,
                owner_object_id: store_id,
                delta: 1,
            });
            mutation_record_bytes(store_id, &extent)
                + mutation_record_bytes(store_id, &hole)
                + mutation_record_bytes(allocator_id, &reference)
        };
        let freed = |_: &Range<u64>, blocks, _: &ExtentValue| dst.cost_of_freeing(blocks);
        dst.zero_tail_before_growth(
            transaction,
            size,
            Some(dst_of(range.start)..dst_of(range.end)),
        )
        .await?;
        let mut start = range.start;
        loop {
            // Half of the room for this file's extents, and what they leave for the `dst`'s they
            // replace, which either file's may be most of.
            let room = transaction.room();
            let (end, taken) = self.end_of_room(start..range.end, room / 2, &shared).await?;
            let dst_range = dst_of(start)..dst_of(end);
            let (dst_end, _) = dst.end_of_room(dst_range, room - taken, &freed).await?;
            let end = start + (dst_end - dst_of(start));
            self.txn_clone_range(transaction, start..end, dst, dst_of(start)).await?;
            let reached = if end == range.end { size } else { dst_of(end).min(size) };
            if reached > dst.txn_get_size(transaction) {
                dst.txn_update_size(transaction, reached, None).await?;
            }
            if end == range.end {
                return Ok(());
            }
            transaction.commit_and_continue().await?;
            start = end;
        }
    }

    /// fxdax: `zero` of a range of however many extents, in as many commits as hold them, from its
    /// start: one that fails has zeroed what it got to. As `clone_range_in_commits`, `transaction`
    /// holds the caller's locks, and is left for it to commit. Each extent costs a record, and
    /// holes nothing: a commit a MiB of the range spun for as long as a file may be on
    /// `fallocate(FALLOC_FL_PUNCH_HOLE)`, the inode locked.
    pub async fn zero_in_commits<'a>(
        &'a self,
        transaction: &mut Transaction<'a>,
        range: Range<u64>,
    ) -> Result<(), Error> {
        let mut start = range.start;
        loop {
            let freed = |_: &Range<u64>, blocks, _: &ExtentValue| self.cost_of_freeing(blocks);
            let room = transaction.room();
            let (end, _) = self.end_of_room(start..range.end, room, freed).await?;
            self.zero(transaction, start..end).await?;
            if end == range.end {
                return Ok(());
            }
            transaction.commit_and_continue().await?;
            start = end;
        }
    }

    /// fxdax: what freeing `blocks`, which an extent of this file holds, adds to a transaction.
    pub(super) fn cost_of_freeing(&self, blocks: ExtentRange) -> u64 {
        let store = self.store();
        let (allocator, owner_object_id) = (store.allocator(), store.store_object_id());
        let mutation = AllocatorMutation::Deallocate { device_range: blocks, owner_object_id };
        mutation_record_bytes(allocator.object_id(), &Mutation::Allocator(mutation))
    }

    /// fxdax: how far this file's extents in `range` fill `room` of a transaction, each costing what
    /// `cost` says of the part of it in `range`, the blocks that part is, and the extent's value:
    /// where the last that fits ends, or `range`'s end if they all do, and how much of `room` that
    /// takes. Holes cost nothing, and the first extent fits whatever it costs, so that a caller
    /// always gets somewhere.
    async fn end_of_room(
        &self,
        range: Range<u64>,
        room: u64,
        cost: impl Fn(&Range<u64>, ExtentRange, &ExtentValue) -> u64,
    ) -> Result<(u64, u64), Error> {
        let store = self.store();
        let layer_set = store.tree().layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger
            .query(Query::FullRange(&ObjectKey::attribute(
                self.object_id(),
                self.attribute_id(),
                AttributeKey::Extent(Extent(range.clone()).search_key()),
            )))
            .await?;
        let mut reached = None;
        let mut taken = 0;
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
            if let ExtentValue::Some { extent, extent_offset, .. } = value
                && key.end > range.start
            {
                let part = max(key.start, range.start)..min(key.end, range.end);
                let at = extent_offset + (part.start - key.start);
                let blocks = ExtentRange::new(*extent, at..at + (part.end - part.start));
                let cost = cost(&part, blocks, value);
                if let Some(reached) = reached
                    && taken + cost > room
                {
                    return Ok((reached, taken));
                }
                taken += cost;
                reached = Some(part.end);
            }
            iter.advance().await?;
        }
        Ok((range.end, taken))
    }
}

#[cfg(test)]
mod tests {
    use crate::filesystem::FxFilesystem;
    use crate::fsck::fsck;
    use crate::object_handle::{ObjectHandle, ReadObjectHandle, WriteObjectHandle};
    use crate::object_store::data_object_handle::testing::{
        counted_filesystem, create_file, every,
    };
    use crate::object_store::data_object_handle::tests::{
        TEST_DEVICE_BLOCK_SIZE, create_object_with_key,
    };
    use crate::object_store::transaction::{Options, lock_keys};
    use crate::object_store::{DataObjectHandle, LockKey, ObjectStore};
    use std::ops::Range;
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;

    /// fxdax: a clone of thousands of fragments, one over as many more, a punch of them, and each
    /// across a hole of 2^50 bytes, each on a filesystem of its own: what they take, in commits and
    /// the bytes of their records, no more than the constants that split them took.
    #[fuchsia::test]
    async fn test_what_clones_and_punches_cost() {
        const FRAGMENTS: usize = 2048;
        const FAR: u64 = 1 << 50;
        const BS: usize = 4096;
        const END: u64 = (2 * FRAGMENTS * BS) as u64;

        enum Case {
            Clone,
            CloneOver,
            Punch,
            CloneAcross,
            PunchAcross,
        }
        async fn run(case: Case) -> (usize, u64) {
            let (fs, _, _, commits, hooks) = counted_filesystem(u64::MAX >> 1).await;
            let store = fs.root_store();
            let data = vec![7u8; BS];
            let fragmented = async |first: usize| {
                let file = create_file(&fs).await;
                let writes = every(&data, (first * BS) as u64, 2, FRAGMENTS, BS);
                file.write_many(&writes).await.expect("write_many failed");
                file
            };
            let far = async || {
                let file = create_file(&fs).await;
                file.write_many(&[(0, &data), (FAR, &data)]).await.expect("write_many failed");
                file
            };
            let clone = async |from: &DataObjectHandle<ObjectStore>,
                               to: &DataObjectHandle<ObjectStore>,
                               end: u64| {
                let store_id = store.store_object_id();
                let keys = lock_keys![
                    LockKey::object(store_id, from.object_id()),
                    LockKey::object(store_id, to.object_id()),
                ];
                let options = Options::default();
                let mut transaction =
                    store.new_transaction(keys, options).await.expect("new_transaction");
                commits.take_sum();
                from.clone_range_in_commits(&mut transaction, 0..end, to, 0, end)
                    .await
                    .expect("clone failed");
                to.txn_update_size(&mut transaction, end, None).await.expect("size failed");
                transaction.commit().await.expect("commit failed");
            };
            let punch = async |file: &DataObjectHandle<ObjectStore>, range: Range<u64>| {
                let mut transaction = file.new_transaction().await.expect("new_transaction");
                commits.take_sum();
                file.zero_in_commits(&mut transaction, range).await.expect("punch failed");
                transaction.commit().await.expect("commit failed");
            };
            match case {
                Case::Clone => clone(&fragmented(0).await, &create_file(&fs).await, END).await,
                Case::CloneOver => clone(&fragmented(0).await, &fragmented(1).await, END).await,
                Case::Punch => punch(&fragmented(0).await, 0..END).await,
                Case::CloneAcross => {
                    clone(&far().await, &create_file(&fs).await, FAR + BS as u64).await
                }
                Case::PunchAcross => punch(&far().await, BS as u64..FAR).await,
            }
            let cost = commits.take_sum();
            std::mem::drop(hooks);
            fsck(fs.clone()).await.expect("fsck failed");
            fs.close().await.expect("close failed");
            cost
        }
        let costs = [
            run(Case::Clone).await,
            run(Case::CloneOver).await,
            run(Case::Punch).await,
            run(Case::CloneAcross).await,
            run(Case::PunchAcross).await,
        ];
        // What the constants that split them took: 82 and 84 commits of clones, 11 of the punch.
        let before = [(82, 225_046), (84, 290_495), (11, 67_344), (1, 253), (1, 0)];
        for (cost, before) in costs.into_iter().zip(before) {
            assert!(cost.0 <= before.0 && cost.1 <= before.1, "{cost:?}, of {before:?}");
        }
    }

    /// fxdax: `allocate` over thousands of fragments, whose extents it changes, and whose holes it
    /// fills, in as many transactions as it takes, none more than one may take of the journal.
    #[fuchsia::test]
    async fn test_allocate_fills_transactions() {
        use crate::object_store::transaction::TRANSACTION_MAX_JOURNAL_USAGE;

        const FRAGMENTS: usize = 2048;
        const BS: usize = 4096;
        let (fs, _, _, commits, hooks) = counted_filesystem(u64::MAX >> 1).await;
        let file = create_file(&fs).await;
        let data = vec![7u8; BS];
        file.write_many(&every(&data, 0, 2, FRAGMENTS, BS)).await.expect("write_many failed");
        commits.take();
        file.allocate(0..(2 * FRAGMENTS * BS) as u64).await.expect("allocate failed");
        let taken = commits.take();
        assert!(taken.len() > 1, "{taken:?}");
        assert!(taken.iter().all(|bytes| *bytes <= TRANSACTION_MAX_JOURNAL_USAGE), "{taken:?}");
        std::mem::drop(hooks);
        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("close failed");
    }

    /// fxdax: a truncate trims what it frees in transactions of its own, committed and continued,
    /// which its one metadata reservation must pay for, however many: a file of many extents,
    /// as PostgreSQL's relations written a page at a time are, panicked the guest's kernel with
    /// "Insufficient reservation space" (TASK-58).
    #[fuchsia::test]
    async fn test_truncate_of_many_extents() {
        let device = DeviceHolder::new(FakeDevice::new(65536, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let object = create_object_with_key(fs.clone(), None, false).await;
        let block_size = fs.block_size().get();
        let mut buf = object.allocate_buffer(block_size as usize).await;
        buf.fill(0xaa);
        // Every other block, so that no two extents merge.
        let extents = 2000;
        for i in 0..extents {
            object
                .write_or_append(Some(2 * i * block_size), buf.as_ref())
                .await
                .expect("write failed");
        }
        object.truncate(0).await.expect("truncate failed");
        assert_eq!(object.get_size(), 0);
        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("Close failed");
    }
}
