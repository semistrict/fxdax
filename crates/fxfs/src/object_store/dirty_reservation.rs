// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: room for the data a page cache holds dirty, reserved as it dirties it, so that a write
//! the filesystem has no room for fails then, with ENOSPC, and its writeback, later, never finds
//! the filesystem full (TASK-80): the guest kernel's buffered writes reserved nothing, and their
//! writeback, failing, dropped the data. As Fuchsia's pager-backed files reserve as their pages
//! are marked dirty (`PagedObjectHandle::mark_dirty`): each block, and its share of the metadata
//! space of the transactions that write it back, of WRITE_MANY_WRITES blocks; and for each file
//! with dirty data a spare, a transaction's metadata space, for its writeback's last transaction,
//! which may write a block alone. A writeback's blocks are paid for from what it takes
//! (`DataObjectHandle::write_many_reserved`), which never pays for metadata with them: what
//! metadata space a writeback takes beyond its share, its file's next writes reserve again,
//! which may then find no room.

use crate::errors::FxfsError;
use crate::object_store::ObjectStore;
use crate::object_store::allocator::{Allocator, Reservation};
use crate::object_store::data_object_handle::WRITE_MANY_WRITES;
use crate::object_store::transaction::TRANSACTION_METADATA_MAX_AMOUNT;
use anyhow::Error;
use fuchsia_sync::Mutex;
use rustc_hash::FxHashMap;
use std::sync::Arc;

/// The metadata space reserved for each block of dirty data: a transaction's, shared by the
/// WRITE_MANY_WRITES writes of a block it may hold.
const METADATA_PER_BLOCK: u64 = TRANSACTION_METADATA_MAX_AMOUNT / WRITE_MANY_WRITES as u64;

/// The metadata space reserved for each file with dirty data: a transaction's.
const SPARE: u64 = TRANSACTION_METADATA_MAX_AMOUNT;

/// The room reserved for a store's files' dirty data, by file. Their writeback takes the room of
/// what it writes (`take`), for `DataObjectHandle::write_many_reserved`.
pub struct DirtyReservation {
    allocator: Arc<Allocator>,
    store_object_id: u64,
    block_size: u64,
    /// The room reserved: every file's dirty data's, and its spare, but what is owed.
    reserved: Reservation,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    files: FxHashMap<u64, File>,
    /// What of the room of data dirtied without room for it is not reserved
    /// (`reserve_what_fits`).
    owed: u64,
}

/// A file with dirty data, or a writeback that has its spare.
#[derive(Default)]
struct File {
    /// How many bytes of it are dirty.
    bytes: u64,
    /// Its spare, as much of it as is reserved.
    spare: u64,
    /// Whether a writeback has its spare, which it gives back as it ends (`written`).
    lent: bool,
}

/// The room of dirty data a writeback took (`DirtyReservation::take`), which its writes are
/// paid for from. What is left of it is released as it is dropped.
pub struct Taken {
    pub reservation: Reservation,
    ino: u64,
    lent: bool,
}

impl DirtyReservation {
    /// Room for `store`'s files' dirty data, none reserved yet.
    pub fn new(store: &ObjectStore) -> Self {
        let allocator = store.allocator();
        let store_object_id = store.store_object_id();
        Self {
            reserved: allocator.clone().reserve_with(Some(store_object_id), |_| 0),
            allocator,
            store_object_id,
            block_size: store.block_size().get().into(),
            inner: Mutex::new(Inner::default()),
        }
    }

    /// What `bytes` of dirty data, whole blocks, take reserved: the blocks, and their share of
    /// metadata space.
    pub fn cost(&self, bytes: u64) -> u64 {
        bytes + bytes.div_ceil(self.block_size) * METADATA_PER_BLOCK
    }

    /// Reserves room for `bytes` more of the file `ino`'s data, which are dirtied, and its spare,
    /// as much of it as isn't: NoSpace if the filesystem hasn't room for them.
    pub fn reserve(&self, ino: u64, bytes: u64) -> Result<(), Error> {
        let mut inner = self.inner.lock();
        let file = inner.files.entry(ino).or_default();
        let spare = if file.lent { 0 } else { SPARE - file.spare };
        let cost = self.cost(bytes) + spare;
        match self.allocator.clone().reserve(Some(self.store_object_id), cost) {
            Some(room) => room.move_to(&self.reserved, cost),
            None => {
                if file.bytes == 0 && file.spare == 0 && !file.lent {
                    inner.files.remove(&ino);
                }
                return Err(FxfsError::NoSpace.into());
            }
        }
        file.bytes += bytes;
        file.spare += spare;
        Ok(())
    }

    /// Reserves what the filesystem has room for of `bytes` more of the file `ino`'s data, which
    /// are dirtied whether it has or not, owing the rest: their writeback then finds room for
    /// them as a write with no reservation does, if it can.
    pub fn reserve_what_fits(&self, ino: u64, bytes: u64) {
        let mut inner = self.inner.lock();
        let file = inner.files.entry(ino).or_default();
        let (cost, spare) = (self.cost(bytes), if file.lent { 0 } else { SPARE - file.spare });
        let room = self
            .allocator
            .clone()
            .reserve_with(Some(self.store_object_id), |limit| (cost + spare).min(limit));
        let got = room.amount();
        room.move_to(&self.reserved, got);
        file.bytes += bytes;
        file.spare += got.saturating_sub(cost);
        inner.owed += cost.saturating_sub(got);
    }

    /// Releases the room of `bytes` of the file `ino`'s dirty data that were dropped, unwritten:
    /// truncated, or the file removed; and its spare, with no dirty data left.
    pub fn release(&self, ino: u64, bytes: u64) {
        let mut inner = self.inner.lock();
        let file = Self::file(&mut inner, ino, bytes);
        let mut freed = self.cost(bytes);
        if file.bytes == 0 && !file.lent {
            freed += file.spare;
            inner.files.remove(&ino);
        }
        let paid = Self::settle(&mut inner, &self.reserved, freed);
        self.reserved.give_back(paid);
    }

    /// Takes the room of `bytes` of the file `ino`'s dirty data a writeback writes, and its
    /// spare, unless another writeback has it, until the writeback is `written`.
    pub fn take(&self, ino: u64, bytes: u64) -> Taken {
        let reservation = self.allocator.clone().reserve_with(Some(self.store_object_id), |_| 0);
        let mut inner = self.inner.lock();
        let file = Self::file(&mut inner, ino, bytes);
        let mut taken = self.cost(bytes);
        let lent = !file.lent;
        if lent {
            taken += std::mem::take(&mut file.spare);
            file.lent = true;
        }
        let paid = Self::settle(&mut inner, &self.reserved, taken);
        self.reserved.move_to(&reservation, paid);
        Taken { reservation, ino, lent }
    }

    /// Ends the writeback that `taken` paid for: its file's spare, if it lent it, held again if
    /// the file has dirty data still, from what the writeback left, or the allocator, as much as
    /// they have: what they haven't, the file's next writes reserve. The rest is released as
    /// `taken` is dropped.
    pub fn written(&self, taken: Taken) {
        if !taken.lent {
            return;
        }
        let mut inner = self.inner.lock();
        let file = inner.files.get_mut(&taken.ino).expect("a file whose spare is lent");
        file.lent = false;
        if file.bytes == 0 {
            inner.files.remove(&taken.ino);
            return;
        }
        let left = taken.reservation.amount().min(SPARE);
        taken.reservation.move_to(&self.reserved, left);
        let room = self
            .allocator
            .clone()
            .reserve_with(Some(self.store_object_id), |limit| (SPARE - left).min(limit));
        let got = room.amount();
        room.move_to(&self.reserved, got);
        file.spare = left + got;
    }

    /// The bytes of dirty data the file `ino` has.
    pub fn dirty_bytes(&self, ino: u64) -> u64 {
        self.inner.lock().files.get(&ino).map_or(0, |file| file.bytes)
    }

    /// The file `ino`, `bytes` of its dirty data no longer dirty.
    fn file(inner: &mut Inner, ino: u64, bytes: u64) -> &mut File {
        let file = inner.files.get_mut(&ino).expect("a file with dirty data");
        assert!(file.bytes >= bytes, "{bytes} bytes of {} dirty", file.bytes);
        file.bytes -= bytes;
        file
    }

    /// What of `cost`, the room of dirty data no longer dirty, the reservation has: as little as
    /// it has, the rest of it owed.
    fn settle(inner: &mut Inner, reserved: &Reservation, cost: u64) -> u64 {
        let paid = cost.min(reserved.amount());
        inner.owed -= cost - paid;
        paid
    }
}

#[cfg(test)]
mod tests {
    use super::{DirtyReservation, SPARE};
    use crate::errors::FxfsError;
    use crate::filesystem::{FxFilesystem, FxFilesystemBuilder, OpenFxFilesystem};
    use crate::fsck::fsck;
    use crate::object_handle::ObjectHandle;
    use crate::object_store::transaction::{Options, lock_keys};
    use crate::object_store::{DataObjectHandle, HandleOptions, ObjectStore};
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;

    const BLOCK: u64 = 4096;

    async fn filesystem() -> OpenFxFilesystem {
        let device = DeviceHolder::new(FakeDevice::new(8192, BLOCK as u32));
        FxFilesystemBuilder::new().format(true).open(device).await.expect("open failed")
    }

    async fn create_file(fs: &FxFilesystem) -> DataObjectHandle<ObjectStore> {
        let store = fs.root_store();
        let mut transaction = store
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let file =
            ObjectStore::create_object(&store, &mut transaction, HandleOptions::default(), None)
                .await
                .expect("create_object failed");
        transaction.commit().await.expect("commit failed");
        file
    }

    /// Every other block of a file, dirtied a block at a time until the filesystem has no room
    /// for more, then written back `at_a_time` blocks at a time, as a page cache's writeback
    /// writes them, the filesystem full: each written, and nothing is left reserved.
    async fn fill_and_write_back(at_a_time: usize) {
        let fs = filesystem().await;
        let allocator = fs.allocator();
        let dirty = DirtyReservation::new(&fs.root_store());
        let (file, other) = (create_file(&fs).await, create_file(&fs).await);
        let ino = file.object_id();
        let mut blocks = 0;
        while dirty.reserve(ino, BLOCK).is_ok() {
            blocks += 1;
        }
        assert!(blocks > 4000, "{blocks} blocks reserved of 8192");
        let data: Vec<Vec<u8>> =
            (0..blocks).map(|block| vec![block as u8; BLOCK as usize]).collect();
        let writes: Vec<(u64, &[u8])> = data
            .iter()
            .enumerate()
            .map(|(block, data)| (2 * block as u64 * BLOCK, &data[..]))
            .collect();
        let error = other.write_many(&[(0, &data[0][..])]).await.expect_err("wrote, full");
        assert!(FxfsError::NoSpace.matches(&error), "{error:?}");
        // What room writing back leaves, or compacting frees, as soon as it is free.
        for writes in writes.chunks(at_a_time) {
            let _full = allocator.clone().reserve_with(None, |limit| limit);
            let taken = dirty.take(ino, writes.len() as u64 * BLOCK);
            file.write_many_reserved(writes, &taken.reservation).await.expect("write_many failed");
            dirty.written(taken);
        }
        assert_eq!(dirty.dirty_bytes(ino), 0);
        assert_eq!((dirty.reserved.amount(), dirty.inner.lock().owed), (0, 0));
        assert!(dirty.inner.lock().files.is_empty());
        let contents = file.contents(usize::MAX).await.expect("contents failed");
        for (offset, data) in writes {
            assert!(contents[offset as usize..][..data.len()] == *data, "block at {offset}");
        }
        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("close failed");
    }

    /// A writeback of the most writes a transaction holds at a time.
    #[fuchsia::test]
    async fn test_dirty_data_reserved_is_written_however_full() {
        fill_and_write_back(128).await;
    }

    /// A writeback of a block at a time, whose transactions take more metadata space than their
    /// share, and their file's spare once: the rest they borrow.
    #[fuchsia::test]
    async fn test_dirty_data_reserved_is_written_a_block_at_a_time_however_full() {
        fill_and_write_back(1).await;
    }

    /// What files' dirty data reserves, its blocks, their share of metadata space and a spare
    /// for each file, is released as the data is dropped, or written back, and a file's spare,
    /// lent to its writeback, held again while it has dirty data.
    #[fuchsia::test]
    async fn test_dirty_data_releases_what_it_reserved() {
        let fs = filesystem().await;
        let allocator = fs.allocator();
        let dirty = DirtyReservation::new(&fs.root_store());
        let before = allocator.get_used_bytes().0;
        let reserved = || dirty.reserved.amount();
        dirty.reserve(1, 10 * BLOCK).expect("reserve failed");
        dirty.reserve(2, 4 * BLOCK).expect("reserve failed");
        assert_eq!(reserved(), dirty.cost(14 * BLOCK) + 2 * SPARE);
        assert_eq!(allocator.get_used_bytes().0, before + reserved());
        dirty.release(1, 4 * BLOCK);
        assert_eq!(reserved(), dirty.cost(10 * BLOCK) + 2 * SPARE);
        dirty.release(1, 6 * BLOCK);
        assert_eq!(reserved(), dirty.cost(4 * BLOCK) + SPARE);

        let taken = dirty.take(2, 2 * BLOCK);
        assert_eq!(taken.reservation.amount(), dirty.cost(2 * BLOCK) + SPARE);
        assert_eq!(reserved(), dirty.cost(2 * BLOCK));
        // A writeback has the spare: another reserves none, nor takes it.
        dirty.reserve(2, BLOCK).expect("reserve failed");
        let also = dirty.take(2, BLOCK);
        assert_eq!(also.reservation.amount(), dirty.cost(BLOCK));
        dirty.written(also);
        dirty.written(taken);
        assert_eq!(reserved(), dirty.cost(2 * BLOCK) + SPARE);
        assert_eq!(dirty.dirty_bytes(2), 2 * BLOCK);
        dirty.release(2, 2 * BLOCK);
        assert_eq!(reserved(), 0);
        assert!(dirty.inner.lock().files.is_empty());
        assert_eq!(allocator.get_used_bytes().0, before);
        fs.close().await.expect("close failed");
    }

    /// Dirty data the filesystem had no room for is owed: its writeback is paid for from the
    /// room of other dirty data, and the owed settled as that is released, nothing left reserved
    /// or owed.
    #[fuchsia::test]
    async fn test_dirty_data_with_no_room_is_owed() {
        let fs = filesystem().await;
        let dirty = DirtyReservation::new(&fs.root_store());
        let file = create_file(&fs).await;
        let ino = file.object_id();
        let mut blocks = 0;
        while dirty.reserve(ino, BLOCK).is_ok() {
            blocks += 1;
        }
        dirty.reserve_what_fits(ino, 2 * BLOCK);
        let owed = dirty.inner.lock().owed;
        assert!(owed > dirty.cost(BLOCK), "{owed} owed");
        let data = vec![1u8; BLOCK as usize];
        let taken = dirty.take(ino, 2 * BLOCK);
        assert_eq!(taken.reservation.amount(), dirty.cost(2 * BLOCK) + SPARE);
        let writes = [(0, &data[..]), (2 * BLOCK, &data[..])];
        file.write_many_reserved(&writes, &taken.reservation).await.expect("write_many failed");
        dirty.written(taken);
        assert_eq!(dirty.inner.lock().owed, owed);
        dirty.release(ino, blocks * BLOCK);
        assert_eq!((dirty.reserved.amount(), dirty.inner.lock().owed), (0, 0));
        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("close failed");
    }
}
