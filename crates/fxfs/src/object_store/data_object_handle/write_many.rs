// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own, a child of data_object_handle.rs: `write_many` writes data scattered across a file
//! in as few transactions, and device writes, as hold it: the guest kernel's writeback, of its page
//! cache and of its DAX window's private pages.

use super::DataObjectHandle;
use crate::checksum::{Checksum, Checksums};
use crate::errors::FxfsError;
use crate::object_handle::{ObjectHandle, ReadObjectHandle};
use crate::object_store::HandleOwner;
use crate::object_store::allocator::Reservation;
use crate::object_store::extent_record::ExtentValue;
use crate::object_store::journal::{checksums_record_bytes, mutation_record_bytes};
use crate::object_store::object_record::{ObjectKey, ObjectValue};
use crate::object_store::transaction::{self, Mutation, Options, Transaction};
use anyhow::{Error, anyhow, ensure};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use storage_device::{ExtentId, ExtentRange, SealedExtent};

/// fxdax: one of `write_many`'s writes: `data`, its bytes, to `range` of the file, which covers
/// `blocks`.
struct BlocksWrite<'d> {
    range: Range<u64>,
    blocks: Range<u64>,
    data: &'d [u8],
}

/// fxdax: fault injection (TASK-164.2): the next this many of [`DataObjectHandle::write_many`]'s
/// transactions fail once their blocks are written, before they commit, leaving the file as a
/// crash between the two would: its old blocks, the new ones named by no record. A guest's
/// copy-on-write writeback commits after its bytes, as the page cache's does, which this checks.
pub static FAIL_COMMITS: AtomicU64 = AtomicU64::new(0);

impl<S: HandleOwner> DataObjectHandle<S> {
    /// fxdax: writes `writes`, each bytes at an offset, as `txn_write` writes one, in as few
    /// transactions as have room for them (`writes_in_room`), each with one `multi_write`: a
    /// transaction's writes have their blocks allocated together, and written with as few of the
    /// device's writes as they take, at once. They are in order of offset, and no two share a
    /// block. This is a page cache's writeback, of pages scattered across a file, with a
    /// transaction, and a round trip to the device, per batch of them rather than per page
    /// (TASK-92.10).
    pub async fn write_many(&self, writes: &[(u64, &[u8])]) -> Result<(), Error> {
        self.write_many_paid(writes, None).await
    }

    /// fxdax: `write_many`, its blocks paid for from `reserved`, which a page cache reserved as it
    /// dirtied them (`DirtyReservation::take`), so that the filesystem filled since doesn't fail
    /// them (TASK-80). Its transactions hold their metadata space in it too, topped up from the
    /// allocator if it is short; or, with no room there either, borrow it, as removing a file
    /// does, which compacting the journal repays: the data was accepted, and must be written.
    pub async fn write_many_reserved(
        &self,
        writes: &[(u64, &[u8])],
        reserved: &Reservation,
    ) -> Result<(), Error> {
        self.write_many_paid(writes, Some(reserved)).await
    }

    /// fxdax: `write_many`, paid for from `reserved` if given (`write_many_reserved`). A
    /// transaction's small writes pack into the staging extent (`txn_write_blocks`), which is open
    /// to every file's: another's write reaching it, and any barrier after, seal it under those
    /// whose blocks are taken but not yet written, which the device then refuses (`SealedExtent`).
    /// No allocation can rule that out, as no allocation holds barriers off, and a packed try again
    /// can lose the same race again, for as long as writes and syncs keep coming (r549). So the
    /// transaction refused is dropped and tried again once, unpacked: in an extent of its own,
    /// minted for it, which no other write reaches before it, and no barrier seals therefore. A
    /// device that refuses that too fails the writes with its error.
    async fn write_many_paid(
        &self,
        writes: &[(u64, &[u8])],
        reserved: Option<&Reservation>,
    ) -> Result<(), Error> {
        let writes = self.blocks_of(writes)?;
        let mut rest = &writes[..];
        while !rest.is_empty() {
            let mut packed = true;
            let (transaction, after) = loop {
                let mut transaction = match reserved {
                    None => self.new_transaction().await?,
                    Some(reserved) => {
                        let blocks = rest.iter().map(|write| write.blocks.clone());
                        let data: u64 =
                            blocks.clone().map(|blocks| blocks.end - blocks.start).sum();
                        // And the last block, which their growth may write again, zeroed past the
                        // end: no page of it was dirtied, nor reserved room for (TASK-149.29).
                        let end = rest.last().map_or(0, |last| last.range.end);
                        let tail = self.growth_zeroes_tail(self.get_size(), end, blocks);
                        let data = data + if tail { self.block_size().get() } else { 0 };
                        let options = self.reserved_transaction_options(reserved, data);
                        self.new_transaction_with_options(options).await?
                    }
                };
                let (writes, after) = rest.split_at(self.writes_in_room(&transaction, rest));
                match self.txn_write_blocks(&mut transaction, writes, packed).await {
                    Ok(()) => break (transaction, after),
                    Err(error)
                        if packed && error.chain().any(|cause| cause.is::<SealedExtent>()) =>
                    {
                        packed = false;
                    }
                    Err(error) => return Err(error),
                }
            };
            // fxdax: fault injection, a crash between the bytes and the commit (`FAIL_COMMITS`).
            let injected = FAIL_COMMITS
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |left| left.checked_sub(1))
                .is_ok();
            if injected {
                return Err(anyhow!(FxfsError::Internal).context("write_many: an injected failure"));
            }
            transaction.commit().await?;
            rest = after;
        }
        Ok(())
    }

    /// fxdax: how many of `writes`, one at least, `transaction` has room for, as written by one
    /// `multi_write`, which has yet to find what they replace, or what it allocates. The most it
    /// can take: for each run of them its extent's record, and a record of the checksums unless
    /// barriers stand for them, and for each block its checksums and the record freeing an extent
    /// of its own. What it can't say is what `TRANSACTION_ROOM` spares: the rest of an allocation
    /// split into many.
    fn writes_in_room(&self, transaction: &Transaction<'_>, writes: &[BlocksWrite<'_>]) -> usize {
        let store = self.store();
        let journaled = !store.filesystem().options().barriers_enabled;
        // Extents' branches and sequences may be as long as they can be, but their offsets don't
        // reach 32 bits.
        let extent = ExtentId::new(u64::MAX, u64::MAX);
        let blocks = ExtentRange::new(extent, u32::MAX as u64..u32::MAX as u64);
        let sum_size = (1 + journaled as u64) * size_of::<Checksum>() as u64;
        let per_block = self.cost_of_freeing(blocks.clone()) + sum_size;
        let run = |write: &BlocksWrite<'_>| {
            let extent =
                ObjectKey::extent(self.object_id(), self.attribute_id(), write.blocks.clone());
            let no_sums = Checksums::fletcher(Vec::new());
            let value = ExtentValue::with_checksum(blocks.extent, blocks.range.start, no_sums, 0);
            let extent = Mutation::merge_object(extent, ObjectValue::Extent(value));
            mutation_record_bytes(store.store_object_id(), &extent)
                + if journaled { checksums_record_bytes(&blocks, &[], true) } else { 0 }
        };
        let block_size = self.block_size();
        let mut room = transaction.room();
        let mut end = None;
        let mut held = 0;
        for write in writes {
            let covered = (write.blocks.end - write.blocks.start) / block_size;
            let run = if end == Some(write.blocks.start) { 0 } else { run(write) };
            let cost = covered * per_block + run;
            if held > 0 && cost > room {
                break;
            }
            room = room.saturating_sub(cost);
            end = Some(write.blocks.end);
            held += 1;
        }
        held
    }

    /// fxdax: the options of a transaction of `write_many_reserved` of writes that have yet to
    /// write `data` bytes of blocks, paid for from `reserved`: holding its metadata space in what
    /// `reserved` has beyond the blocks, topped up from the allocator if it hasn't enough, so that
    /// it never pays for metadata with room for blocks; or, the allocator full, borrowing it, and
    /// as much room for the blocks as the allocator has, if `reserved` hasn't it (some of it owed,
    /// `DirtyReservation::reserve_what_fits`), which then fails it as a write with no reservation
    /// fails.
    fn reserved_transaction_options<'a>(
        &self,
        reserved: &'a Reservation,
        data: u64,
    ) -> Options<'a> {
        let store = self.store();
        let (allocator, owner) = (store.allocator(), Some(store.store_object_id()));
        let short =
            (data + transaction::TRANSACTION_METADATA_MAX_AMOUNT).saturating_sub(reserved.amount());
        let holds = short == 0
            || allocator
                .clone()
                .reserve(owner, short)
                .map(|more| more.move_to(reserved, short))
                .is_some();
        if !holds {
            let short = data.saturating_sub(reserved.amount());
            let more = allocator.reserve_with(owner, |limit| short.min(limit));
            let got = more.amount();
            more.move_to(reserved, got);
        }
        Options {
            borrow_metadata_space: !holds,
            allocator_reservation: Some(reserved),
            ..self.default_transaction_options()
        }
    }

    /// fxdax: `writes`, but those empty, with the blocks each covers, which are in order and apart.
    fn blocks_of<'d>(&self, writes: &[(u64, &'d [u8])]) -> Result<Vec<BlocksWrite<'d>>, Error> {
        let block_size = self.block_size();
        let mut blocks_writes: Vec<BlocksWrite<'_>> = Vec::with_capacity(writes.len());
        for &(offset, data) in writes.iter().filter(|(_, data)| !data.is_empty()) {
            let range = offset..offset.checked_add(data.len() as u64).ok_or(FxfsError::TooBig)?;
            let blocks = block_size.align_range_outwards(range.clone()).ok_or(FxfsError::TooBig)?;
            ensure!(
                blocks_writes.last().is_none_or(|last| last.blocks.end <= blocks.start),
                FxfsError::InvalidArgs
            );
            blocks_writes.push(BlocksWrite { range, blocks, data });
        }
        Ok(blocks_writes)
    }

    /// fxdax: writes `writes`, whose blocks `blocks_of` found, in `transaction`, with one
    /// `multi_write`, and grows the file to the last one's end. If `packed`, small writes (at most
    /// STAGED_MAX bytes in all) take blocks of the staging extent, which other files' small writes
    /// share (`StoreObjectHandle::multi_write_packed`): small files written back pack into one
    /// extent, as a DAX window maps them with one MAP, rather than an extent each.
    async fn txn_write_blocks<'a>(
        &'a self,
        transaction: &mut Transaction<'a>,
        writes: &[BlocksWrite<'_>],
        packed: bool,
    ) -> Result<(), Error> {
        let Some(last) = writes.last() else { return Ok(()) };
        let len = writes.iter().map(|write| write.blocks.end - write.blocks.start).sum::<u64>();
        let mut buf = self.store().device.allocate_buffer(len as usize).await;
        let mut ranges: Vec<Range<u64>> = Vec::new();
        let mut at = 0;
        for BlocksWrite { range, blocks, data } in writes {
            let mut write_buf = buf.subslice_mut(at..at + (blocks.end - blocks.start) as usize);
            at += write_buf.len();
            self.handle
                .read_unaligned_ends(self.attribute_id(), range.clone(), write_buf.reborrow())
                .await?;
            write_buf
                .subslice_mut(
                    (range.start - blocks.start) as usize..(range.end - blocks.start) as usize,
                )
                .copy_from_slice(data);
            match ranges.last_mut() {
                Some(last) if last.end == blocks.start => last.end = blocks.end,
                _ => ranges.push(blocks.clone()),
            }
        }
        let (handle, attribute_id, buf) = (&self.handle, self.attribute_id(), buf.as_mut());
        match packed {
            true => {
                handle.multi_write_packed(transaction, attribute_id, None, &ranges, buf).await?
            }
            false => handle.multi_write(transaction, attribute_id, None, &ranges, buf).await?,
        }
        self.txn_grow_size(transaction, last.range.end, &ranges).await
    }
}

#[cfg(test)]
mod tests {
    use crate::errors::FxfsError;
    use crate::filesystem::{FxFilesystem, SyncOptions};
    use crate::fsck::fsck;
    use crate::object_handle::{ObjectHandle, ReadObjectHandle, WriteObjectHandle};
    use crate::object_store::allocator::STAGED_MAX;
    use crate::object_store::data_object_handle::testing::{
        counted_filesystem, create_file, every, writeback_filesystem,
    };
    use crate::object_store::data_object_handle::tests::{
        TEST_DEVICE_BLOCK_SIZE, create_object_with_key,
    };
    use crate::object_store::directory::{Directory, replace_child};
    use crate::object_store::transaction::{Options, TRANSACTION_ROOM, lock_keys};
    use crate::object_store::{DataObjectHandle, HandleOptions, LockKey, ObjectStore};
    use fuchsia_async as fasync;
    use fuchsia_sync::Mutex;
    use std::ops::Range;
    use std::sync::Arc;
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;
    use storage_device::{ExtentId, ExtentRange, NoExtent, SealedExtent};

    /// fxdax: small files written back (`write_many`) share the staging extent, so that a DAX
    /// window maps them with one MAP rather than one each: their pieces name one extent, in the
    /// order they were written, a transaction's writes together. The extent lives while any file
    /// holds its blocks, and dies once none does and it is the staging extent no more; a large
    /// write takes an extent of its own; and fsck finds nothing wrong.
    #[fuchsia::test]
    async fn test_write_many_packs_small_files() {
        let device = DeviceHolder::new(FakeDevice::new(32768, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let bs = fs.block_size().get();
        let store = fs.root_store();
        let root = Directory::open(&store, store.root_directory_object_id()).await.expect("open");
        let lock = |object_id| {
            lock_keys![
                LockKey::object(store.store_object_id(), root.object_id()),
                LockKey::object(store.store_object_id(), object_id),
            ]
        };
        let create = async |name: &str| {
            let mut transaction = store
                .new_transaction(lock(root.object_id()), Options::default())
                .await
                .expect("new failed");
            let object = root.create_child_file(&mut transaction, name).await.expect("create");
            transaction.commit().await.expect("commit failed");
            object
        };
        let remove = async |name: &str, object_id: u64| {
            let mut transaction =
                store.new_transaction(lock(object_id), Options::default()).await.expect("new");
            replace_child(&mut transaction, None, (&root, name)).await.expect("replace failed");
            transaction.commit().await.expect("commit failed");
            store.tombstone_object(object_id, Options::default(), None).await.expect("tombstone");
        };
        let pieces = async |object: &DataObjectHandle<ObjectStore>, range: Range<u64>| {
            object
                .extents_in(range)
                .await
                .expect("extents_in failed")
                .into_iter()
                .map(|(e, _)| (e.logical_range(), e.device_range().clone()))
                .collect::<Vec<_>>()
        };
        let first = create("first").await;
        let second = create("second").await;
        let third = create("third").await;
        first.write_many(&[(0, b"the first file")]).await.expect("write_many failed");
        second.write_many(&[(0, &[7; 5000])]).await.expect("write_many failed");
        third.write_many(&[(0, &[8; 100]), (2 * bs, &[9; 100])]).await.expect("write_many failed");
        let staged = pieces(&first, 0..bs).await;
        let staging = staged[0].1.extent;
        assert_eq!(staged, vec![(0..bs, ExtentRange::new(staging, 0..bs))]);
        assert_eq!(
            pieces(&second, 0..2 * bs).await,
            vec![(0..2 * bs, ExtentRange::new(staging, bs..3 * bs))]
        );
        assert_eq!(
            pieces(&third, 0..3 * bs).await,
            vec![
                (0..bs, ExtentRange::new(staging, 3 * bs..4 * bs)),
                (2 * bs..3 * bs, ExtentRange::new(staging, 4 * bs..5 * bs)),
            ]
        );
        assert_eq!(&second.contents(usize::MAX).await.expect("contents")[..], &[7; 5000][..]);
        let expected = [vec![8; 100], vec![0; 2 * bs as usize - 100], vec![9; 100]].concat();
        assert_eq!(&third.contents(usize::MAX).await.expect("contents")[..], &expected[..]);

        // A large write takes an extent of its own, from its start.
        let large = create("large").await;
        let len = STAGED_MAX + bs;
        large.write_many(&[(0, &vec![6; len as usize])]).await.expect("write_many failed");
        let own = pieces(&large, 0..len).await;
        assert_eq!(own[0].1.range.start, 0);
        assert!(own.iter().all(|(_, blocks)| blocks.extent != staging), "{own:?}");
        fsck(fs.clone()).await.expect("fsck failed");

        // Any file's blocks keep the extent.
        remove("first", first.object_id()).await;
        fs.sync(SyncOptions { flush_device: true, ..Default::default() })
            .await
            .expect("sync failed");
        assert_eq!(fs.allocator().extents()[&staging], (store.store_object_id(), 4 * bs));
        assert_eq!(&second.contents(usize::MAX).await.expect("contents")[..], &[7; 5000][..]);
        fsck(fs.clone()).await.expect("fsck failed");

        // Without any, it dies once it is no longer the staging extent: here, at a remount.
        remove("second", second.object_id()).await;
        remove("third", third.object_id()).await;
        assert_eq!(fs.allocator().extents()[&staging], (store.store_object_id(), 0));
        fsck(fs.clone()).await.expect("fsck failed");
        drop((first, second, third, large, root, store));
        fs.close().await.expect("close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystem::open(device).await.expect("open failed");
        assert!(!fs.allocator().extents().contains_key(&staging));
        fs.sync(SyncOptions { flush_device: true, ..Default::default() })
            .await
            .expect("sync failed");
        let device = fs.device();
        let mut buf = device.allocate_buffer(bs as usize).await;
        let error = device.read(staging, 0, buf.as_mut()).await.expect_err("read the extent");
        assert_eq!(error.downcast_ref(), Some(&NoExtent(staging)));
        drop(buf);
        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("close failed");
    }

    /// fxdax: a small write back whose blocks of the staging extent the device refuses, as a
    /// barrier sealing it between the allocation and the write has it do, is tried again in a new
    /// transaction, unpacked, which writes it (`write_many_paid`); one the device goes on refusing
    /// fails with the device's error.
    #[fuchsia::test]
    async fn test_write_many_tries_again_after_a_seal() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use storage_device::fake_device::Op;
        let refusals = Arc::new(AtomicUsize::new(0));
        let mut device = FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE);
        device.set_op_callback({
            let refusals = refusals.clone();
            move |op| {
                let refuse = matches!(op, Op::Write)
                    && refusals
                        .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| left.checked_sub(1))
                        .is_ok();
                if refuse { Err(SealedExtent(ExtentId::new(0, 1)).into()) } else { Ok(()) }
            }
        });
        let fs = FxFilesystem::new_empty(DeviceHolder::new(device)).await.expect("new_empty");
        let object = create_object_with_key(fs.clone(), None, false).await;

        refusals.store(1, Ordering::SeqCst);
        object.write_many(&[(0, b"a small file")]).await.expect("write_many failed");
        assert_eq!(refusals.load(Ordering::SeqCst), 0, "the write wasn't refused");
        // The test's object holds more than the write, which leaves the rest as it was.
        let small = b"a small file";
        assert_eq!(&object.contents(usize::MAX).await.expect("contents")[..small.len()], small);

        refusals.store(usize::MAX, Ordering::SeqCst);
        let error = object.write_many(&[(0, b"refused")]).await.expect_err("write_many wrote");
        refusals.store(0, Ordering::SeqCst);
        assert!(error.chain().any(|cause| cause.is::<SealedExtent>()), "{error:#}");
        assert_eq!(&object.contents(usize::MAX).await.expect("contents")[..small.len()], small);
        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("close failed");
    }

    /// fxdax: small writes back to files of their own, packed into the staging extent, with their
    /// own syncs and another's coming between them all the time, as a guest's writers fdatasync
    /// while another fsyncs (r549's log_writes): each seals the staging extent, the device's
    /// barrier sealing it for real rather than a callback refusing writes, and each write is
    /// written, and reads back, whatever the seals came between.
    #[fuchsia::test(threads = 4)]
    async fn test_write_many_across_real_seals() {
        use std::sync::atomic::{AtomicBool, Ordering};
        const WRITERS: usize = 8;
        const ROUNDS: u64 = 64;
        const LEN: u64 = 8192;
        let device = DeviceHolder::new(FakeDevice::new(65536, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let mut objects = Vec::new();
        for _ in 0..WRITERS {
            let store = fs.root_store();
            let mut transaction = store
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            let object = ObjectStore::create_object(
                &store,
                &mut transaction,
                HandleOptions::default(),
                None,
            )
            .await
            .expect("create_object failed");
            transaction.commit().await.expect("commit failed");
            objects.push(Arc::new(object));
        }
        let sync = || SyncOptions { flush_device: true, ..Default::default() };
        let done = Arc::new(AtomicBool::new(false));
        let syncer = fasync::Task::spawn({
            let (fs, done) = (fs.clone(), done.clone());
            async move {
                while !done.load(Ordering::SeqCst) {
                    fs.sync(sync()).await.expect("sync failed");
                    // A sync with nothing to flush doesn't wait: the writers' syncs take turns.
                    fasync::yield_now().await;
                }
            }
        });
        let writers = objects.iter().enumerate().map(|(i, object)| {
            let (fs, object) = (fs.clone(), object.clone());
            fasync::Task::spawn(async move {
                for round in 0..ROUNDS {
                    let data = vec![(i as u64 * ROUNDS + round) as u8; LEN as usize];
                    object
                        .write_many(&[(round * LEN, &data)])
                        .await
                        .unwrap_or_else(|error| panic!("writer {i}, round {round}: {error:#}"));
                    fs.sync(sync()).await.expect("sync failed");
                }
            })
        });
        futures::future::join_all(writers).await;
        done.store(true, Ordering::SeqCst);
        syncer.await;
        for (i, object) in objects.iter().enumerate() {
            let contents = object.contents(usize::MAX).await.expect("contents failed");
            let expected: Vec<u8> = (0..ROUNDS)
                .flat_map(|round| vec![(i as u64 * ROUNDS + round) as u8; LEN as usize])
                .collect();
            assert!(contents[..] == expected[..], "writer {i}'s file doesn't read back");
        }
        drop(objects);
        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("close failed");
    }

    /// fxdax: the staging extent, emptied, takes another small write, which a crash loses: replay
    /// discards that write's transaction, after the deallocation that emptied the extent, so the
    /// extent dies there. A mount freed it at once, and a cut before the journal recorded the
    /// discard had the next replay read the write again, and verify it in an extent the device no
    /// longer held: a filesystem that mounted no more (CI run 41876989384). Every cut of the mount
    /// mounts.
    #[fuchsia::test]
    async fn test_every_cut_of_a_mount_discarding_a_write_to_the_staging_extent_emptied() {
        use storage_device::fake_device::Crashes;
        let crashes = Crashes::new(0, u32::MAX);
        let device = FakeDevice::new(32768, TEST_DEVICE_BLOCK_SIZE).record_crashes(crashes.clone());
        let fs = FxFilesystem::new_empty(DeviceHolder::new(device)).await.expect("new_empty");
        let bs = fs.block_size().get();
        let first = create_object_with_key(fs.clone(), None, false).await;
        let second = create_object_with_key(fs.clone(), None, false).await;
        fs.sync(SyncOptions { flush_device: true, ..Default::default() }).await.expect("sync");
        let staged = async |object: &DataObjectHandle<ObjectStore>| {
            let extents = object.extents_in(0..bs).await.expect("extents_in failed");
            let [(extent, _)] = &extents[..] else { panic!("not one extent: {extents:?}") };
            extent.device_range().clone()
        };
        first.write_many(&[(0, &vec![1; bs as usize])]).await.expect("write_many failed");
        let staging = staged(&first).await.extent;
        first.truncate(0).await.expect("truncate failed");
        second.write_many(&[(0, &vec![2; bs as usize])]).await.expect("write_many failed");
        let lost = staged(&second).await;
        assert_eq!(lost, ExtentRange::new(staging, bs..2 * bs));

        // The journal is written, with no barrier after, so that replay verifies the write.
        let journal = fs.journal();
        let (checkpoint, _) = journal.sync(SyncOptions::default()).await.expect("sync").unwrap();
        journal.wait_for_flush(checkpoint.file_offset).await.expect("written");
        crashes.mark(0);
        crashes.last();
        fs.device().flush().await.expect("flush failed");
        drop((first, second));
        fs.close().await.expect("close failed");
        let (_, image) = crashes.take().next().expect("what the device held");
        let device = image.recovered(0);
        device.corrupt(|extents, _| {
            let data = extents.get_mut(&staging).expect("the staging extent");
            data[lost.range.start as usize..lost.range.end as usize].fill(0xaf);
        });

        let cuts = Crashes::every(0, 8);
        cuts.mark(0);
        let device = DeviceHolder::new(device.record_crashes(cuts.clone()));
        let fs = FxFilesystem::open(device).await.expect("open failed");
        // What goes on after the mount: a transaction, and a sync.
        drop(create_object_with_key(fs.clone(), None, false).await);
        fs.sync(SyncOptions { flush_device: true, ..Default::default() }).await.expect("sync");
        cuts.stop();
        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("close failed");
        let mut count = 0;
        for (_, device) in cuts.take() {
            count += 1;
            let fs = FxFilesystem::open(DeviceHolder::new(device))
                .await
                .unwrap_or_else(|error| panic!("cut {count} of the mount: {error:#}"));
            fsck(fs.clone()).await.expect("fsck failed");
            fs.close().await.expect("close failed");
        }
        assert!(count > 0, "no cut of the mount");
    }

    /// fxdax: an extent minted for a write whose transaction hasn't committed as the allocator's
    /// tree is compacted is recorded dying, and its allocation commits after, which a crash loses
    /// the write of: replay discards its transaction, and the mount finds the extent dead. It freed
    /// it at once, and a cut before the journal recorded the discard had the next replay read the
    /// write again, and verify it in an extent the device no longer held: a filesystem that
    /// mounted no more (CI run 42196106048, TASK-210). Every cut of the mount mounts.
    #[fuchsia::test]
    async fn test_every_cut_of_a_mount_discarding_a_write_to_an_extent_minted_as_it_compacted() {
        use storage_device::fake_device::Crashes;
        let crashes = Crashes::new(0, u32::MAX);
        let device = FakeDevice::new(32768, TEST_DEVICE_BLOCK_SIZE).record_crashes(crashes.clone());
        let fs = FxFilesystem::new_empty(DeviceHolder::new(device)).await.expect("new_empty");
        let bs = fs.block_size().get();
        let object = create_object_with_key(fs.clone(), None, false).await;
        fs.sync(SyncOptions { flush_device: true, ..Default::default() }).await.expect("sync");
        let mut transaction = object.new_transaction().await.expect("new_transaction failed");
        let mut buffer = object.allocate_buffer(bs as usize).await;
        buffer.as_mut().fill(1);
        object.txn_write(&mut transaction, 0, buffer.as_ref()).await.expect("write failed");
        fs.journal().force_compact().await.expect("compaction failed");
        transaction.commit().await.expect("commit failed");
        let extents = object.extents_in(0..bs).await.expect("extents_in failed");
        let [(extent, _)] = &extents[..] else { panic!("not one extent: {extents:?}") };
        let lost = extent.device_range().clone();

        // The journal is written, with no barrier after, so that replay verifies the write.
        let journal = fs.journal();
        let (checkpoint, _) = journal.sync(SyncOptions::default()).await.expect("sync").unwrap();
        journal.wait_for_flush(checkpoint.file_offset).await.expect("written");
        crashes.mark(0);
        crashes.last();
        fs.device().flush().await.expect("flush failed");
        drop(buffer);
        drop(object);
        fs.close().await.expect("close failed");
        let (_, image) = crashes.take().next().expect("what the device held");
        let device = image.recovered(0);
        device.corrupt(|extents, _| {
            let data = extents.get_mut(&lost.extent).expect("the extent written");
            data[lost.range.start as usize..lost.range.end as usize].fill(0xaf);
        });

        let cuts = Crashes::every(0, 8);
        cuts.mark(0);
        let device = DeviceHolder::new(device.record_crashes(cuts.clone()));
        let fs = FxFilesystem::open(device).await.expect("open failed");
        // What goes on after the mount: a transaction, and a sync.
        drop(create_object_with_key(fs.clone(), None, false).await);
        fs.sync(SyncOptions { flush_device: true, ..Default::default() }).await.expect("sync");
        cuts.stop();
        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("close failed");
        let mut count = 0;
        for (_, device) in cuts.take() {
            count += 1;
            let fs = FxFilesystem::open(DeviceHolder::new(device))
                .await
                .unwrap_or_else(|error| panic!("cut {count} of the mount: {error:#}"));
            fsck(fs.clone()).await.expect("fsck failed");
            fs.close().await.expect("close failed");
        }
        assert!(count > 0, "no cut of the mount");
    }

    /// fxdax: a small write back whose blocks of the staging extent another file's small write
    /// reaches, and a sync then seals, each time it is about to write them, as a guest's writers
    /// racing syncs can have it as often as they like (r549): the device refuses the packed write
    /// for real, and its one try again takes an extent of its own, which no other write reaches,
    /// nor the seals therefore, and writes it. A try again packed would take a new staging extent,
    /// for the next interloper and seal to take from under it in turn, until the tries ran out.
    #[fuchsia::test]
    async fn test_write_many_outlives_seals_of_the_staging_extent() {
        use crate::object_store::allocator::STAGING_SIZE;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        type Target = (Arc<FxFilesystem>, Arc<DataObjectHandle<ObjectStore>>);
        // The filesystem and the interloper's file, once they are made.
        let target: Arc<Mutex<Option<Target>>> = Arc::default();
        let interloping = Arc::new(AtomicBool::new(false));
        let interlopes = Arc::new(AtomicUsize::new(0));
        let mut device = FakeDevice::new(16384, TEST_DEVICE_BLOCK_SIZE);
        device.set_write_hook({
            let (target, interloping, interlopes) =
                (target.clone(), interloping.clone(), interlopes.clone());
            move |extent| {
                let target = target.lock().clone();
                let (interloping, interlopes) = (interloping.clone(), interlopes.clone());
                Box::pin(async move {
                    let Some((fs, interloper)) = target else { return };
                    if fs.allocator().extent_size(extent) != Some(STAGING_SIZE)
                        || interloping.swap(true, Ordering::SeqCst)
                    {
                        return;
                    }
                    interloper.write_many(&[(0, b"an interloper")]).await.expect("interloper");
                    fs.sync(SyncOptions { flush_device: true, ..Default::default() })
                        .await
                        .expect("sync failed");
                    interlopes.fetch_add(1, Ordering::SeqCst);
                    interloping.store(false, Ordering::SeqCst);
                })
            }
        });
        let fs = FxFilesystem::new_empty(DeviceHolder::new(device)).await.expect("new_empty");
        let store = fs.root_store();
        let mut objects = Vec::new();
        for _ in 0..2 {
            let mut transaction = store
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            let object = ObjectStore::create_object(
                &store,
                &mut transaction,
                HandleOptions::default(),
                None,
            )
            .await
            .expect("create_object failed");
            transaction.commit().await.expect("commit failed");
            objects.push(Arc::new(object));
        }
        let (object, interloper) = (objects[0].clone(), objects[1].clone());
        *target.lock() = Some((fs.clone(), interloper.clone()));

        object.write_many(&[(0, b"a small file")]).await.expect("write_many failed");
        *target.lock() = None;
        assert_eq!(interlopes.load(Ordering::SeqCst), 1);
        assert_eq!(&object.contents(usize::MAX).await.expect("contents")[..], b"a small file");
        assert_eq!(&interloper.contents(usize::MAX).await.expect("contents")[..], b"an interloper");
        let blocks = object.extents_in(0..fs.block_size().get()).await.expect("extents_in");
        let staged = interloper.extents_in(0..fs.block_size().get()).await.expect("extents_in");
        assert_ne!(blocks[0].0.device_range().extent, staged[0].0.device_range().extent);
        drop((object, interloper, objects, store));
        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("close failed");
    }

    /// fxdax: a page cache's writeback of 64 pages scattered across a file, every other block of
    /// it, the last one up to the file's new end, short of its block: through `write_many`, one
    /// transaction, whose blocks one device write writes, at once. One write each, as the guest
    /// wrote back each run of pages, took a transaction and a round trip to the device each
    /// (TASK-92.10).
    #[fuchsia::test]
    async fn test_writeback_of_scattered_pages() {
        let (fs, trips, _) =
            writeback_filesystem(storage_device::fake_device::DEFAULT_BRANCH).await;
        let bs = fs.block_size().get() as usize;
        let pages: Vec<Vec<u8>> = (1..=64).map(|page| vec![page as u8; bs]).collect();
        let mut writes: Vec<(u64, &[u8])> = pages
            .iter()
            .enumerate()
            .map(|(page, data)| ((2 * page * bs) as u64, &data[..]))
            .collect();
        writes.last_mut().unwrap().1 = &pages[63][..100];
        let mut expected = vec![0; 126 * bs + 100];
        for (offset, data) in &writes {
            expected[*offset as usize..*offset as usize + data.len()].copy_from_slice(data);
        }
        let device_writes = || (trips.writes.count(), trips.writes.trips());

        let one_each = create_file(&fs).await;
        let before = device_writes();
        for (offset, data) in &writes {
            let mut buf = one_each.allocate_buffer(data.len()).await;
            buf.as_mut().copy_from_slice(data);
            one_each.write_or_append(Some(*offset), buf.as_ref()).await.expect("write failed");
        }
        let after = device_writes();
        let one_each_took = (after.0 - before.0, after.1 - before.1);
        assert_eq!(one_each.contents(usize::MAX).await.expect("contents")[..], expected[..]);

        let at_once = create_file(&fs).await;
        let before = device_writes();
        at_once.write_many(&writes).await.expect("write_many failed");
        let after = device_writes();
        let at_once_took = (after.0 - before.0, after.1 - before.1);
        assert_eq!(at_once.contents(usize::MAX).await.expect("contents")[..], expected[..]);
        assert_eq!(at_once.get_size(), expected.len() as u64);

        assert_eq!((one_each_took, at_once_took), ((64, 64), (1, 1)));
        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("close failed");
    }

    /// fxdax: `write_many` writes each write as `write_or_append` does: parts of blocks keep what
    /// the file holds around them, empty writes write nothing, and the file grows to the last
    /// one's end. Writes out of order, or sharing a block, are refused.
    #[fuchsia::test]
    async fn test_write_many_writes_as_each_write_would() {
        let (fs, _, _) = writeback_filesystem(storage_device::fake_device::DEFAULT_BRANCH).await;
        let bs = fs.block_size().get() as usize;
        let old = vec![1u8; 4 * bs];
        let (head, span, tail) = (vec![2u8; 50], vec![3u8; bs], vec![4u8; 9]);
        let writes = [
            (100, &head[..]),
            (bs as u64, &[][..]),
            ((bs + 10) as u64, &span[..]),
            ((5 * bs + 7) as u64, &tail[..]),
        ];
        let (one_each, at_once) = (create_file(&fs).await, create_file(&fs).await);
        for file in [&one_each, &at_once] {
            file.write_many(&[(0, &old[..])]).await.expect("write_many failed");
        }
        for (offset, data) in writes {
            let mut buf = one_each.allocate_buffer(data.len()).await;
            buf.as_mut().copy_from_slice(data);
            one_each.write_or_append(Some(offset), buf.as_ref()).await.expect("write failed");
        }
        at_once.write_many(&writes).await.expect("write_many failed");
        let mut expected = [old, vec![0; bs + 16]].concat();
        for (offset, data) in writes {
            expected[offset as usize..offset as usize + data.len()].copy_from_slice(data);
        }
        for file in [&one_each, &at_once] {
            assert_eq!(file.contents(usize::MAX).await.expect("contents")[..], expected[..]);
        }

        for refused in
            [[(bs as u64, &head[..]), (0, &head[..])], [(0, &head[..]), (100, &tail[..])]]
        {
            let error = at_once.write_many(&refused).await.expect_err("write_many wrote");
            assert!(FxfsError::InvalidArgs.matches(&error), "{error:?}");
        }
        assert_eq!(at_once.contents(usize::MAX).await.expect("contents")[..], expected[..]);
        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("close failed");
    }

    /// fxdax: `write_many`'s transactions hold the writes they have room for, by what they could
    /// cost, unless one alone costs more: each a device write here.
    #[fuchsia::test]
    async fn test_write_many_splits_its_transactions() {
        const LARGE: usize = 1 << 20;
        let (fs, trips, _) =
            writeback_filesystem(storage_device::fake_device::DEFAULT_BRANCH).await;
        let bs = fs.block_size().get() as usize;
        let data = vec![5u8; 4 * LARGE];
        let file = create_file(&fs).await;
        let mut expected = Vec::new();
        let mut writes = every(&data[..bs], 0, 2, 300, bs);
        // Two writes of more blocks than a transaction has room for together, and one alone.
        let half = 3 * LARGE / 4;
        for (len, at) in [(half, 600), (half, 1000), (4 * LARGE, 1400)] {
            writes.push(((at * bs) as u64, &data[..len]));
        }
        // Two writes each from a block's last byte, whose blocks are more than their bytes fill,
        // which is what they are costed by.
        writes.extend(every(&data[..3 * LARGE / 4 - 2 * bs], (2600 * bs - 1) as u64, 300, 2, bs));
        for (offset, data) in &writes {
            expected.resize(*offset as usize, 0);
            expected.extend_from_slice(data);
        }
        let before = trips.writes.count();
        file.write_many(&writes).await.expect("write_many failed");
        // Transactions of 179 writes, the rest of the 300, the two large ones, the one that costs
        // more alone, in the three of the most an allocation may be, and the last two.
        assert_eq!(trips.writes.count() - before, 7);
        assert_eq!(file.contents(usize::MAX).await.expect("contents")[..], expected[..]);
        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("close failed");
    }

    /// fxdax: `write_many`'s transactions, of as many writes as they have room for, that overwrite
    /// as many records as they can, take no more than the room it counted on, which leaves what
    /// else they take within what a transaction may take of the journal, their records' integers
    /// as long as a branch's and a large file's make them; and one holds the
    /// BLOCKS_PER_TRANSACTION writes of a block that dirty data pays its metadata for.
    #[fuchsia::test]
    async fn test_write_many_transactions_fit_the_journal() {
        use crate::object_store::dirty_reservation::BLOCKS_PER_TRANSACTION;
        use crate::object_store::transaction::TRANSACTION_MAX_JOURNAL_USAGE;
        use std::cell::RefCell;

        const WRITES: usize = BLOCKS_PER_TRANSACTION as usize;
        let (fs, _, journal, commits, _hooks) = counted_filesystem(u64::MAX >> 1).await;
        let bs = fs.block_size().get() as usize;
        let data = vec![7u8; 512 * bs];
        const AT: u64 = 1 << 40;
        // A file of `blocks` blocks, written a block at a time, or 256 at a time: an extent each.
        let file = async |blocks: usize, at_a_time: usize| {
            let file = create_file(&fs).await;
            for write in every(&data[..at_a_time * bs], AT, at_a_time, blocks / at_a_time, bs) {
                file.write_many(&[write]).await.expect("write_many failed");
            }
            file
        };
        // What each of the transactions `write_many` makes of `writes` takes of the journal.
        let records = RefCell::new(Vec::new());
        let journal_usages = async |file: &DataObjectHandle<ObjectStore>,
                                    writes: &[(u64, &[u8])]| {
            journal.take();
            commits.take();
            file.write_many(writes).await.expect("write_many failed");
            records.borrow_mut().extend(commits.take());
            journal.take()
        };
        let mut usages = Vec::new();
        // Writes of a block, each with a block between, over extents of a block, or each splitting
        // an extent, as many as dirty data pays for in a transaction.
        for at_a_time in [1, 256] {
            let file = file(768, at_a_time).await;
            let usage = journal_usages(&file, &every(&data[..bs], AT, 2, WRITES, bs)).await;
            assert_eq!(usage.len(), 1, "{usage:?}");
            usages.push(usage);
        }
        // Twice that, of a block or more each, so that each transaction fills up to its room.
        for (len, at_a_time) in [(1, 1), (2, 1), (3, 1), (8, 1), (32, 1), (1, 256), (8, 256)] {
            let writes = 2 * WRITES.min(512 / (len + 1));
            let file = file((len + 1) * writes, at_a_time).await;
            let writes = every(&data[..len * bs], AT, len + 1, writes, bs);
            usages.push(journal_usages(&file, &writes).await);
        }
        // 127 writes of a block, and one of the rest of 1 MiB, over extents of a block.
        let blocks = file(512, 1).await;
        let mut writes = every(&data[..bs], AT, 2, WRITES - 1, bs);
        writes.push((AT + (254 * bs) as u64, &data[..129 * bs]));
        usages.push(journal_usages(&blocks, &writes).await);
        // Writes of two blocks and two bytes, each from a block's last byte, so covering four
        // blocks, with a block between, over extents of a block.
        let blocks = file(640, 1).await;
        let writes = every(&data[..2 * bs + 2], AT + bs as u64 - 1, 5, WRITES, bs);
        usages.push(journal_usages(&blocks, &writes).await);
        assert!(
            usages.iter().flatten().all(|usage| *usage <= TRANSACTION_MAX_JOURNAL_USAGE),
            "{usages:?} of the journal, of {TRANSACTION_MAX_JOURNAL_USAGE}"
        );
        let records = records.into_inner();
        assert!(records.iter().all(|bytes| *bytes <= TRANSACTION_ROOM), "{records:?}");
        fs.close().await.expect("close failed");
    }
}
