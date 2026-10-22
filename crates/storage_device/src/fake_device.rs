// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// fxdax: Fuchsia's fake device is a range of memory. This one is an extent store in memory
// (backlog doc-4): it holds extents by ID, each as long as its furthest write, written once (until
// the next barrier seals them) and only by their branch, frees them, and simulates a crash by undoing any of what happened
// since the last barrier (writes and frees). It can record such crashes at random barriers
// (`Crashes`), and count the round trips its reads would take to a store answering from afar
// (`RoundTrips`).

use crate::buffer::{BufferFuture, BufferRef, MutableBufferRef};
use crate::buffer_allocator::{BufferAllocator, BufferSource};
use crate::{Device, DeviceHolder, ExtentId, NoExtent, OtherBranch, SealedExtent};
use anyhow::{Error, ensure};
use async_trait::async_trait;
use block_protocol::{ReadOptions, WriteFlags, WriteOptions};
use fuchsia_sync::Mutex;
use rand::rngs::StdRng;
use rand::{RngExt as _, SeedableRng as _};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::Poll;

pub enum Op {
    Read,
    Write,
    Flush,
    Free,
    /// fxdax: the barrier after a write with `POST_BARRIER`, which a flush would otherwise be.
    PostBarrier,
}

pub trait Observer: Send + Sync {
    fn barrier(&self) {}
}

/// The branch of a fake device's extents, unless it is given another.
pub const DEFAULT_BRANCH: u64 = 1;

#[derive(Debug, Default, Clone)]
struct Inner {
    /// The extents, by ID, each as long as its furthest write.
    extents: HashMap<ExtentId, Vec<u8>>,
    /// The bytes the extents hold.
    stored: u64,
    /// One more than the highest sequence number written in the device's branch.
    next_sequence: u64,
    /// The extents written since the last barrier, which are open: the others are sealed.
    open: HashSet<ExtentId>,
    /// The root.
    root: Vec<u8>,
    /// What happened since the last barrier, which a simulated crash may undo.
    since_barrier: Vec<Change>,
}

#[derive(Debug, Clone)]
enum Change {
    /// A block written at `offset` in `extent`.
    Write { extent: ExtentId, offset: usize },
    /// `extent` freed, with what it held.
    Free { extent: ExtentId, data: Vec<u8> },
}

impl Inner {
    fn barrier(&mut self) {
        self.open.clear();
        self.since_barrier.clear();
    }

    /// Undoes each write of a block and each free since the last barrier, or not, at random: a
    /// block undone holds garbage, and an extent whose free is undone holds what it did. Returns
    /// what it undid.
    fn discard_random(&mut self, rng: &mut impl rand::Rng, block_size: usize) -> Vec<String> {
        let mut discarded = Vec::new();
        for change in std::mem::take(&mut self.since_barrier) {
            if !rng.random::<bool>() {
                continue;
            }
            match change {
                Change::Write { extent, offset } => {
                    if let Some(data) = self.extents.get_mut(&extent) {
                        data[offset..offset + block_size].fill(0xaf);
                        discarded.push(format!("write of {extent:?} at {offset}"));
                    }
                }
                Change::Free { extent, data } => {
                    if !self.extents.contains_key(&extent) {
                        self.stored += data.len() as u64;
                        self.extents.insert(extent, data);
                        discarded.push(format!("free of {extent:?}"));
                    }
                }
            }
        }
        discarded
    }
}

/// Crashes of a device, recorded at random barriers once marked (`Crashes::mark`): each is what the
/// device would hold had it crashed just before the barrier, which is any of what happened since
/// the barrier before (`FakeDevice::discard_random_since_last_flush`).
pub struct Crashes(Mutex<CrashesInner>);

struct CrashesInner {
    rng: StdRng,
    /// A crash is recorded at one barrier in `one_in`, on average.
    one_in: u32,
    /// The mark that crashes recorded from now on carry, if any are recorded.
    mark: Option<u64>,
    recorded: Vec<Crash>,
}

struct Crash {
    mark: u64,
    inner: Inner,
    capacity: u64,
    block_size: u32,
    branch: u64,
}

impl Crashes {
    /// Crashes recorded at one barrier in `one_in`, chosen, as is what each crash undoes, by an RNG
    /// seeded with `seed`.
    pub fn new(seed: u64, one_in: u32) -> Arc<Self> {
        Arc::new(Self(Mutex::new(CrashesInner {
            rng: StdRng::seed_from_u64(seed),
            one_in,
            mark: None,
            recorded: Vec::new(),
        })))
    }

    /// Crashes recorded from now on carry `mark`, which tells a test what had happened before them:
    /// a crash carries the latest mark set before the barrier it was recorded at.
    pub fn mark(&self, mark: u64) {
        self.0.lock().mark = Some(mark);
    }

    /// The crashes recorded, in order, as devices that recovered from them, with their marks.
    pub fn take(&self) -> Vec<(u64, DeviceHolder)> {
        std::mem::take(&mut self.0.lock().recorded)
            .into_iter()
            .map(|crash| {
                let device = FakeDevice::with_inner(crash.inner, crash.capacity, crash.block_size)
                    .with_branch(crash.branch);
                (crash.mark, DeviceHolder::new(device))
            })
            .collect()
    }

    /// At random, records `device` crashing before a barrier, as it holds `inner`.
    fn maybe_record(&self, device: &FakeDevice, inner: &Inner) {
        let mut guard = self.0.lock();
        let crashes = &mut *guard;
        let Some(mark) = crashes.mark else { return };
        if !crashes.rng.random_ratio(1, crashes.one_in) {
            return;
        }
        let mut crashed = inner.clone();
        crashed.discard_random(&mut crashes.rng, device.allocator.block_size());
        // What the device holds after recovering is sealed.
        crashed.barrier();
        crashes.recorded.push(Crash {
            mark,
            inner: crashed,
            capacity: device.capacity,
            block_size: device.block_size(),
            branch: device.branch,
        });
    }
}

/// fxdax: the round trips a device's reads and writes take (`FakeDevice::with_round_trips`), were
/// each a request to a store that answers a while after, as an extent store over a VM's queue
/// does. What a caller waits for, reading or writing, is a round trip each, whatever their size.
#[derive(Debug, Default)]
pub struct RoundTrips {
    pub reads: Requests,
    pub writes: Requests,
}

/// fxdax: requests of one kind, and their round trips (`RoundTrips`): those in flight together
/// share one, and one issued once none is in flight takes one more.
#[derive(Debug, Default)]
pub struct Requests(Mutex<RequestsInner>);

#[derive(Debug, Default)]
struct RequestsInner {
    count: u64,
    trips: u64,
    in_flight: usize,
}

impl Requests {
    /// The requests so far.
    pub fn count(&self) -> u64 {
        self.0.lock().count
    }

    /// The round trips the requests so far took.
    pub fn trips(&self) -> u64 {
        self.0.lock().trips
    }

    /// Issues a request, in flight until what this returns is dropped, which this returns once its
    /// caller polls it again: the requests a caller issues together, as `join_all` does, are all in
    /// flight before the first is answered, and those it awaits one after another never are.
    async fn issue(&self) -> InFlight<'_> {
        {
            let mut inner = self.0.lock();
            if inner.in_flight == 0 {
                inner.trips += 1;
            }
            inner.in_flight += 1;
            inner.count += 1;
        }
        let in_flight = InFlight(self);
        let mut sent = false;
        std::future::poll_fn(|cx| {
            if std::mem::replace(&mut sent, true) {
                Poll::Ready(())
            } else {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
        .await;
        in_flight
    }
}

/// A request in flight (`Requests::issue`).
struct InFlight<'a>(&'a Requests);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.0.lock().in_flight -= 1;
    }
}

/// A Device backed by memory.
pub struct FakeDevice {
    allocator: BufferAllocator,
    inner: Mutex<Inner>,
    /// The most bytes the extents may hold.
    capacity: u64,
    branch: u64,
    closed: AtomicBool,
    operation_closure: Box<dyn Fn(Op) -> Result<(), Error> + Send + Sync>,
    read_only: AtomicBool,
    poisoned: AtomicBool,
    observer: Option<Box<dyn Observer>>,
    crashes: Option<Arc<Crashes>>,
    /// fxdax: what `Device::sequence_floor` says, which its test may change.
    sequence_floor: Arc<AtomicU64>,
    /// fxdax: what each free waits for, if anything (`FakeDevice::with_frees_held_by`).
    frees_held_by: Option<Arc<futures::lock::Mutex<()>>>,
    /// fxdax: what counts the reads' round trips, if anything (`FakeDevice::with_round_trips`).
    round_trips: Option<Arc<RoundTrips>>,
}

const TRANSFER_HEAP_SIZE: usize = 64 * 1024 * 1024;

impl FakeDevice {
    /// A device holding at most `block_count` blocks of `block_size` bytes.
    pub fn new(block_count: u64, block_size: u32) -> Self {
        Self::with_inner(Inner::default(), block_count * block_size as u64, block_size)
    }

    fn with_inner(inner: Inner, capacity: u64, block_size: u32) -> Self {
        let allocator =
            BufferAllocator::new(block_size as usize, BufferSource::new(TRANSFER_HEAP_SIZE));
        Self {
            allocator,
            inner: Mutex::new(inner),
            capacity,
            branch: DEFAULT_BRANCH,
            closed: AtomicBool::new(false),
            operation_closure: Box::new(|_: Op| Ok(())),
            read_only: AtomicBool::new(false),
            poisoned: AtomicBool::new(false),
            observer: None,
            crashes: None,
            sequence_floor: Arc::default(),
            frees_held_by: None,
            round_trips: None,
        }
    }

    /// Makes `branch` the branch of the device's extents.
    pub fn with_branch(mut self, branch: u64) -> Self {
        self.branch = branch;
        self
    }

    /// fxdax: makes `floor` the device's sequence floor (`Device::sequence_floor`), as it is when
    /// the test sets it.
    pub fn with_sequence_floor(mut self, floor: Arc<AtomicU64>) -> Self {
        self.sequence_floor = floor;
        self
    }

    /// fxdax: makes each free wait for `held`'s lock, after the op callback is told of it, before
    /// it frees anything: a test holding the lock holds the frees in flight, as an extent store's
    /// are over their round trips, until it lets go.
    pub fn with_frees_held_by(mut self, held: Arc<futures::lock::Mutex<()>>) -> Self {
        self.frees_held_by = Some(held);
        self
    }

    /// fxdax: counts the round trips of the device's reads and writes in `trips`.
    pub fn with_round_trips(mut self, trips: Arc<RoundTrips>) -> Self {
        self.round_trips = Some(trips);
        self
    }

    /// Records crashes at random barriers in `crashes`.
    pub fn record_crashes(mut self, crashes: Arc<Crashes>) -> Self {
        self.crashes = Some(crashes);
        self
    }

    pub fn set_observer(&mut self, observer: Box<dyn Observer>) {
        self.observer = Some(observer);
    }

    /// Sets a callback that will run at the beginning of read, write, flush and free which will
    /// forward any errors, and proceed on Ok().
    pub fn set_op_callback(
        &mut self,
        cb: impl Fn(Op) -> Result<(), Error> + Send + Sync + 'static,
    ) {
        self.operation_closure = Box::new(cb);
    }

    /// Creates a fake device from an image (which can be anything that implements
    /// std::io::Read), which becomes the contents of extent 0 of branch 0. The device holds as
    /// much as is read.
    pub fn from_image(
        mut reader: impl std::io::Read,
        block_size: u32,
    ) -> Result<Self, std::io::Error> {
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;
        Ok(Self::from_vec(data, block_size))
    }

    /// Creates a fake device from a `Vec`, which becomes the contents of extent 0 of branch 0. The
    /// device holds as much as the `Vec`.
    pub fn from_vec(data: Vec<u8>, block_size: u32) -> Self {
        let capacity = data.len() as u64;
        let inner = Inner {
            extents: HashMap::from([(ExtentId::default(), data)]),
            stored: capacity,
            ..Default::default()
        };
        Self::with_inner(inner, capacity, block_size)
    }

    /// The IDs of the extents the device holds, in order.
    pub fn extents(&self) -> Vec<ExtentId> {
        let mut extents: Vec<_> = self.inner.lock().extents.keys().copied().collect();
        extents.sort();
        extents
    }

    /// A barrier: what happened before it survives a crash, and the extents written are sealed.
    fn barrier(&self, inner: &mut Inner) {
        if let Some(crashes) = &self.crashes {
            crashes.maybe_record(self, inner);
        }
        inner.barrier();
    }

    fn check_alignment(&self, offset: u64, len: usize) -> Result<(), Error> {
        let block_size = self.allocator.block_size() as u64;
        ensure!(
            offset % block_size == 0 && len as u64 % block_size == 0,
            "unaligned I/O: {len} bytes at {offset}"
        );
        Ok(())
    }
}

#[async_trait]
impl Device for FakeDevice {
    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        assert!(!self.closed.load(Ordering::Relaxed));
        self.allocator.allocate_buffer(size)
    }

    fn block_size(&self) -> u32 {
        self.allocator.block_size() as u32
    }

    fn block_count(&self) -> u64 {
        self.capacity / self.block_size() as u64
    }

    /// Blocks within the extent never written, and past its end, read as zeros.
    async fn read_with_opts(
        &self,
        extent: ExtentId,
        offset: u64,
        mut buffer: MutableBufferRef<'_>,
        _read_opts: ReadOptions,
    ) -> Result<(), Error> {
        ensure!(!self.closed.load(Ordering::Relaxed));
        (self.operation_closure)(Op::Read)?;
        self.check_alignment(offset, buffer.len())?;
        let _in_flight = match &self.round_trips {
            Some(trips) => Some(trips.reads.issue().await),
            None => None,
        };
        let inner = self.inner.lock();
        let Some(data) = inner.extents.get(&extent) else {
            return Err(NoExtent(extent).into());
        };
        let offset = offset as usize;
        let held = data.len().saturating_sub(offset).min(buffer.len());
        let (mut held_part, mut rest) = buffer.reborrow().split_at_mut(held);
        if held > 0 {
            held_part.copy_from_slice(&data[offset..offset + held]);
        }
        rest.fill(0);
        Ok(())
    }

    async fn write_with_opts(
        &self,
        extent: ExtentId,
        offset: u64,
        buffer: BufferRef<'_>,
        write_opts: WriteOptions,
    ) -> Result<(), Error> {
        ensure!(!self.closed.load(Ordering::Relaxed));
        ensure!(!self.read_only.load(Ordering::Relaxed));
        self.check_alignment(offset, buffer.len())?;
        let _in_flight = match &self.round_trips {
            Some(trips) => Some(trips.writes.issue().await),
            None => None,
        };
        let mut inner = self.inner.lock();

        if write_opts.flags.contains(WriteFlags::PRE_BARRIER) {
            if let Some(observer) = &self.observer {
                observer.barrier();
            }
            self.barrier(&mut inner);
        }

        (self.operation_closure)(Op::Write)?;
        // As doc-4 has it, only the device's branch writes, and an extent sealed by a barrier can't
        // be written.
        if extent.branch != self.branch {
            return Err(OtherBranch(extent).into());
        }
        if inner.extents.contains_key(&extent) && !inner.open.contains(&extent) {
            return Err(SealedExtent(extent).into());
        }
        let offset = offset as usize;
        let end = offset + buffer.len();
        let held = inner.extents.get(&extent).map_or(0, |data| data.len());
        let grows = end.saturating_sub(held) as u64;
        ensure!(
            inner.stored + grows <= self.capacity,
            "the device is full: {} of {} bytes held, writing {} more",
            inner.stored,
            self.capacity,
            grows
        );
        inner.stored += grows;
        inner.next_sequence = inner.next_sequence.max(extent.sequence + 1);
        inner.open.insert(extent);
        let data = inner.extents.entry(extent).or_default();
        if data.len() < end {
            data.resize(end, 0);
        }
        buffer.copy_to_slice(&mut data[offset..end]);
        let block_size = self.allocator.block_size();
        for offset in (offset..end).step_by(block_size) {
            inner.since_barrier.push(Change::Write { extent, offset });
        }
        if write_opts.flags.contains(WriteFlags::POST_BARRIER) {
            // As a flush after the write; failing, as a full store's commit, it seals nothing.
            (self.operation_closure)(Op::PostBarrier)?;
            self.barrier(&mut inner);
        }
        Ok(())
    }

    fn branch(&self) -> u64 {
        self.branch
    }

    async fn next_sequence(&self) -> Result<u64, Error> {
        ensure!(!self.closed.load(Ordering::Relaxed));
        Ok(self.inner.lock().next_sequence)
    }

    async fn sequence_floor(&self) -> Result<u64, Error> {
        ensure!(!self.closed.load(Ordering::Relaxed));
        Ok(self.sequence_floor.load(Ordering::Relaxed))
    }

    async fn free(&self, extent: ExtentId) -> Result<(), Error> {
        ensure!(!self.closed.load(Ordering::Relaxed));
        ensure!(!self.read_only.load(Ordering::Relaxed));
        (self.operation_closure)(Op::Free)?;
        if let Some(held) = &self.frees_held_by {
            drop(held.lock().await);
        }
        let mut inner = self.inner.lock();
        if let Some(data) = inner.extents.remove(&extent) {
            inner.open.remove(&extent);
            inner.stored -= data.len() as u64;
            inner.since_barrier.push(Change::Free { extent, data });
        }
        Ok(())
    }

    fn is_open(&self, extent: ExtentId) -> bool {
        self.inner.lock().open.contains(&extent)
    }

    fn opened(&self, extent: ExtentId) {
        let mut inner = self.inner.lock();
        if !inner.extents.contains_key(&extent) {
            inner.open.insert(extent);
        }
    }

    async fn root(&self) -> Result<Vec<u8>, Error> {
        ensure!(!self.closed.load(Ordering::Relaxed));
        (self.operation_closure)(Op::Read)?;
        Ok(self.inner.lock().root.clone())
    }

    async fn commit_root(&self, root: &[u8]) -> Result<(), Error> {
        ensure!(!self.closed.load(Ordering::Relaxed));
        ensure!(!self.read_only.load(Ordering::Relaxed));
        (self.operation_closure)(Op::Flush)?;
        let mut inner = self.inner.lock();
        self.barrier(&mut inner);
        inner.root = root.to_vec();
        Ok(())
    }

    async fn close(&self) -> Result<(), Error> {
        self.closed.store(true, Ordering::Relaxed);
        Ok(())
    }

    async fn flush(&self) -> Result<(), Error> {
        self.barrier(&mut self.inner.lock());
        (self.operation_closure)(Op::Flush)
    }

    fn reopen(&self, read_only: bool) {
        // Whatever was written before is sealed.
        self.inner.lock().open.clear();
        self.closed.store(false, Ordering::Relaxed);
        self.read_only.store(read_only, Ordering::Relaxed);
    }

    fn is_read_only(&self) -> bool {
        self.read_only.load(Ordering::Relaxed)
    }

    fn snapshot(&self) -> Result<DeviceHolder, Error> {
        self.fork(self.branch)
    }

    /// The snapshot is of a device that crashed, and recovered with what it held sealed.
    fn fork(&self, branch: u64) -> Result<DeviceHolder, Error> {
        let mut inner = self.inner.lock().clone();
        inner.open.clear();
        if branch != self.branch {
            // The fork's branch has written nothing.
            inner.next_sequence = 0;
        }
        let fork = Self::with_inner(inner, self.capacity, self.block_size()).with_branch(branch);
        Ok(DeviceHolder::new(fork))
    }

    /// Undoes each write of a block and each free since the last barrier, or not, at random: a
    /// block undone holds garbage, and an extent whose free is undone holds what it did.
    fn discard_random_since_last_flush(&self) -> Result<(), Error> {
        let mut inner = self.inner.lock();
        log::info!("Discarding from {} changes", inner.since_barrier.len());
        let discarded = inner.discard_random(&mut rand::rng(), self.allocator.block_size());
        log::info!("Discarded {discarded:?}");
        Ok(())
    }

    /// Sets the poisoned state for the device. A poisoned device will panic the thread that
    /// performs Drop on it.
    fn poison(&self) -> Result<(), Error> {
        self.poisoned.store(true, Ordering::Relaxed);
        Ok(())
    }
}

impl Drop for FakeDevice {
    fn drop(&mut self) {
        if self.poisoned.load(Ordering::Relaxed) {
            panic!("This device was poisoned to crash whomever is holding a reference here.");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Crashes, FakeDevice, RoundTrips};
    use crate::Device;
    use crate::{ExtentId, OtherBranch, SealedExtent};
    use block_protocol::{WriteFlags, WriteOptions};
    use std::sync::Arc;

    const TEST_DEVICE_BLOCK_SIZE: usize = 512;

    async fn write(device: &dyn Device, extent: ExtentId, offset: u64, byte: u8, len: usize) {
        let mut buffer = device.allocate_buffer(len).await;
        buffer.fill(byte);
        device.write(extent, offset, buffer.as_ref()).await.expect("write failed");
    }

    async fn read(device: &dyn Device, extent: ExtentId, offset: u64, len: usize) -> Vec<u8> {
        let mut buffer = device.allocate_buffer(len).await;
        device.read(extent, offset, buffer.as_mut()).await.expect("read failed");
        buffer.to_vec()
    }

    #[fuchsia::test]
    async fn test_extents() {
        const BS: usize = TEST_DEVICE_BLOCK_SIZE;
        let device = FakeDevice::new(8, BS as u32).with_branch(7);
        assert_eq!(device.branch(), 7);
        assert_eq!(device.next_sequence().await.unwrap(), 0);
        let (a, b) = (ExtentId::new(7, 1), ExtentId::new(7, 4));
        write(&device, a, BS as u64, 1, BS).await;
        write(&device, b, 0, 2, 2 * BS).await;
        assert_eq!(device.extents(), vec![a, b]);
        assert_eq!(device.next_sequence().await.unwrap(), 5);
        // Blocks never written read as zeros, within the extent and past its end.
        assert_eq!(
            read(&device, a, 0, 3 * BS).await,
            [vec![0; BS], vec![1; BS], vec![0; BS]].concat()
        );
        assert_eq!(read(&device, b, 0, 2 * BS).await, vec![2; 2 * BS]);
        assert_eq!(read(&device, a, 4 * BS as u64, BS).await, vec![0; BS]);

        // The device holds 8 blocks, of which the extents hold 4.
        let mut buffer = device.allocate_buffer(5 * BS).await;
        assert!(device.write(ExtentId::new(7, 9), 0, buffer.as_ref()).await.is_err());
        buffer.fill(3);
        device
            .write(ExtentId::new(7, 9), 0, buffer.subslice(..4 * BS))
            .await
            .expect("write failed");

        device.free(a).await.expect("free failed");
        device.free(a).await.expect("freeing a freed extent failed");
        let mut buffer = device.allocate_buffer(BS).await;
        assert!(device.read(a, 0, buffer.as_mut()).await.is_err());
        assert_eq!(device.extents(), vec![b, ExtentId::new(7, 9)]);
        // Freeing doesn't make sequence numbers available again.
        assert_eq!(device.next_sequence().await.unwrap(), 10);
        // Only the device's branch writes.
        let other = ExtentId::new(8, 20);
        let error = device.write(other, 0, buffer.as_ref()).await.expect_err("wrote");
        assert_eq!(error.downcast_ref::<OtherBranch>(), Some(&OtherBranch(other)));
    }

    #[fuchsia::test]
    async fn test_write_once() {
        const BS: usize = TEST_DEVICE_BLOCK_SIZE;
        let device = FakeDevice::new(8, BS as u32);
        let (a, b) = (ExtentId::new(1, 1), ExtentId::new(1, 2));
        // An extent is open until the next barrier.
        assert!(!device.is_open(a));
        write(&device, a, 0, 1, BS).await;
        write(&device, a, BS as u64, 2, BS).await;
        assert!(device.is_open(a));
        device.flush().await.expect("flush failed");
        assert!(!device.is_open(a));
        let buffer = device.allocate_buffer(BS).await;
        let error = device.write(a, 2 * BS as u64, buffer.as_ref()).await.expect_err("rewrote");
        assert_eq!(error.downcast_ref::<SealedExtent>(), Some(&SealedExtent(a)));

        // A write with a barrier first seals what was written before it.
        write(&device, b, 0, 3, BS).await;
        let options = WriteOptions { flags: WriteFlags::PRE_BARRIER, ..Default::default() };
        device
            .write_with_opts(ExtentId::new(1, 3), 0, buffer.as_ref(), options)
            .await
            .expect("write failed");
        assert!(!device.is_open(b));
        assert!(device.write(b, BS as u64, buffer.as_ref()).await.is_err());
        device.write(ExtentId::new(1, 3), BS as u64, buffer.as_ref()).await.expect("write failed");

        // What was written before a rewrite failed is as it was.
        assert_eq!(read(&device, a, 0, 2 * BS).await, [vec![1; BS], vec![2; BS]].concat());
    }

    /// An extent the filesystem minted is open once it says so, until the next barrier; one sealed
    /// stays sealed.
    #[fuchsia::test]
    async fn test_opened() {
        const BS: usize = TEST_DEVICE_BLOCK_SIZE;
        let device = FakeDevice::new(8, BS as u32);
        let (a, b) = (ExtentId::new(1, 1), ExtentId::new(1, 2));
        device.opened(a);
        assert!(device.is_open(a));
        write(&device, a, 0, 1, BS).await;
        device.flush().await.expect("flush failed");
        assert!(!device.is_open(a));
        device.opened(a);
        assert!(!device.is_open(a));
        // Opened and sealed before anything wrote it, it doesn't exist.
        device.opened(b);
        device.flush().await.expect("flush failed");
        assert!(!device.is_open(b));
        assert_eq!(device.extents(), vec![a]);
    }

    #[fuchsia::test]
    async fn test_root() {
        const BS: usize = TEST_DEVICE_BLOCK_SIZE;
        let device = FakeDevice::new(8, BS as u32);
        assert_eq!(device.root().await.unwrap(), Vec::<u8>::new());
        let extent = ExtentId::new(1, 1);
        write(&device, extent, 0, 1, BS).await;
        device.commit_root(b"one").await.expect("commit failed");
        assert_eq!(device.root().await.unwrap(), b"one");
        // Committing is a barrier: a crash keeps the root, and what was written before it.
        let snapshot = device.snapshot().expect("snapshot failed");
        snapshot.discard_random_since_last_flush().expect("discard failed");
        assert_eq!(snapshot.root().await.unwrap(), b"one");
        assert_eq!(read(&**snapshot, extent, 0, BS).await, vec![1; BS]);
    }

    #[fuchsia::test]
    async fn test_crashes() {
        const BS: usize = TEST_DEVICE_BLOCK_SIZE;
        let crashes = Crashes::new(1, 1);
        let device = FakeDevice::new(8, BS as u32).record_crashes(crashes.clone());
        let (a, b) = (ExtentId::new(1, 1), ExtentId::new(1, 2));
        // Nothing is recorded before the first mark.
        write(&device, a, 0, 1, BS).await;
        device.flush().await.expect("flush failed");
        crashes.mark(1);
        write(&device, b, 0, 2, BS).await;
        device.flush().await.expect("flush failed");
        crashes.mark(2);
        device.commit_root(b"one").await.expect("commit failed");

        let recorded = crashes.take();
        assert_eq!(recorded.iter().map(|(mark, _)| *mark).collect::<Vec<_>>(), vec![1, 2]);
        // A crash before a flush keeps what the flush before sealed.
        let (_, before_flush) = &recorded[0];
        assert_eq!(read(&***before_flush, a, 0, BS).await, vec![1; BS]);
        // A crash before a commit keeps the root before it, and what was flushed.
        let (_, before_commit) = &recorded[1];
        assert_eq!(before_commit.root().await.unwrap(), Vec::<u8>::new());
        assert_eq!(read(&***before_commit, b, 0, BS).await, vec![2; BS]);
        // A device that recovered holds its extents sealed.
        let buffer = before_commit.allocate_buffer(BS).await;
        let error = before_commit.write(b, BS as u64, buffer.as_ref()).await.expect_err("rewrote");
        assert_eq!(error.downcast_ref::<SealedExtent>(), Some(&SealedExtent(b)));
        assert!(crashes.take().is_empty());
    }

    /// fxdax: a free waits while the test holds the frees, and is made once it lets go.
    #[fuchsia::test]
    async fn test_frees_held() {
        const BS: usize = TEST_DEVICE_BLOCK_SIZE;
        let held = Arc::new(futures::lock::Mutex::new(()));
        let device = FakeDevice::new(8, BS as u32).with_frees_held_by(held.clone());
        let extent = ExtentId::new(1, 1);
        write(&device, extent, 0, 1, BS).await;
        let holding = held.lock().await;
        let mut free = std::pin::pin!(device.free(extent));
        assert!(futures::poll!(free.as_mut()).is_pending());
        assert_eq!(device.extents(), vec![extent]);
        drop(holding);
        free.await.expect("free failed");
        assert_eq!(device.extents(), vec![]);
    }

    /// fxdax: reads, or writes, awaited one after another take a round trip each; those issued
    /// together, few or many (which `join_all` polls otherwise), share one.
    #[fuchsia::test]
    async fn test_round_trips() {
        const BS: usize = TEST_DEVICE_BLOCK_SIZE;
        let trips = Arc::new(RoundTrips::default());
        let device = FakeDevice::new(128, BS as u32).with_round_trips(trips.clone());
        let extent = ExtentId::new(1, 1);
        write(&device, extent, 0, 1, 40 * BS).await;
        for block in 0..3 {
            assert_eq!(read(&device, extent, block * BS as u64, BS).await, vec![1; BS]);
        }
        assert_eq!((trips.reads.count(), trips.reads.trips()), (3, 3));
        for together in [2, 40] {
            let reads = (0..together).map(|block| read(&device, extent, block * BS as u64, BS));
            assert_eq!(
                futures::future::join_all(reads).await,
                vec![vec![1; BS]; together as usize]
            );
        }
        assert_eq!((trips.reads.count(), trips.reads.trips()), (45, 5));

        for block in 0..2 {
            write(&device, ExtentId::new(1, 2), block * BS as u64, 2, BS).await;
        }
        let writes =
            (0..40).map(|block| write(&device, ExtentId::new(1, 3), block * BS as u64, 3, BS));
        futures::future::join_all(writes).await;
        assert_eq!((trips.writes.count(), trips.writes.trips()), (43, 4));
        assert_eq!(read(&device, ExtentId::new(1, 3), 0, 40 * BS).await, vec![3; 40 * BS]);
    }

    #[fuchsia::test]
    async fn test_discard_random_undoes_frees() {
        const BS: usize = TEST_DEVICE_BLOCK_SIZE;
        let device = FakeDevice::new(8, BS as u32);
        let extent = ExtentId::new(1, 1);
        write(&device, extent, 0, 1, BS).await;
        device.flush().await.expect("flush failed");
        device.free(extent).await.expect("free failed");
        let (mut kept, mut undone) = (false, false);
        while !(kept && undone) {
            let snapshot = device.snapshot().expect("snapshot failed");
            snapshot.discard_random_since_last_flush().expect("discard failed");
            let mut buffer = snapshot.allocate_buffer(BS).await;
            match snapshot.read(extent, 0, buffer.as_mut()).await {
                Ok(()) => {
                    assert_eq!(buffer.to_vec(), vec![1; BS]);
                    undone = true;
                }
                Err(_) => kept = true,
            }
        }
    }

    #[fuchsia::test(threads = 10)]
    async fn test_discard_random_with_barriers() {
        let device = FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE as u32);
        // Loop 100 times to catch errors.
        for iteration in 0..1000 {
            // fxdax: extents are written once, before the next barrier, so each epoch's writes are
            // to an extent of their own: writes 0-1, 2-4 and 5-6.
            let extent = |i: usize| {
                let epoch = if i < 2 {
                    0
                } else if i < 5 {
                    1
                } else {
                    2
                };
                ExtentId::new(1, iteration * 3 + epoch)
            };
            let mut data = vec![0; 7 * TEST_DEVICE_BLOCK_SIZE];
            rand::fill(&mut data[..]);
            // Ensure that barriers work with overwrites.
            let indices = [1, 2, 3, 4, 3, 5, 6];
            for i in 0..indices.len() {
                let mut buffer = device.allocate_buffer(TEST_DEVICE_BLOCK_SIZE).await;
                if i == 2 || i == 5 {
                    buffer.copy_from_slice(
                        &data[indices[i] * TEST_DEVICE_BLOCK_SIZE
                            ..indices[i] * TEST_DEVICE_BLOCK_SIZE + TEST_DEVICE_BLOCK_SIZE],
                    );
                    device
                        .write_with_opts(
                            extent(i),
                            i as u64 * TEST_DEVICE_BLOCK_SIZE as u64,
                            buffer.as_ref(),
                            WriteOptions { flags: WriteFlags::PRE_BARRIER, ..Default::default() },
                        )
                        .await
                        .expect("Failed to write to FakeDevice");
                } else {
                    buffer.copy_from_slice(
                        &data[indices[i] * TEST_DEVICE_BLOCK_SIZE
                            ..indices[i] * TEST_DEVICE_BLOCK_SIZE + TEST_DEVICE_BLOCK_SIZE],
                    );
                    device
                        .write_with_opts(
                            extent(i),
                            i as u64 * TEST_DEVICE_BLOCK_SIZE as u64,
                            buffer.as_ref(),
                            WriteOptions::default(),
                        )
                        .await
                        .expect("Failed to write to FakeDevice");
                }
            }
            device.discard_random_since_last_flush().expect("failed to randomly discard writes");
            let mut discard = false;
            let mut discard_2 = false;
            for i in 0..7 {
                let mut read_buffer = device.allocate_buffer(TEST_DEVICE_BLOCK_SIZE).await;
                device
                    .read(extent(i), i as u64 * TEST_DEVICE_BLOCK_SIZE as u64, read_buffer.as_mut())
                    .await
                    .expect("failed to read from FakeDevice");
                let mut read_data = vec![0u8; TEST_DEVICE_BLOCK_SIZE];
                read_buffer.copy_to_slice(&mut read_data);
                let expected_data = &data[indices[i] * TEST_DEVICE_BLOCK_SIZE
                    ..indices[i] * TEST_DEVICE_BLOCK_SIZE + TEST_DEVICE_BLOCK_SIZE];
                if i < 2 {
                    if expected_data != &read_data[..] {
                        discard = true;
                    }
                } else if i < 5 {
                    if discard == true {
                        assert_ne!(expected_data, &read_data[..]);
                        discard_2 = true;
                    } else if expected_data != &read_data[..] {
                        discard_2 = true;
                    }
                } else if discard_2 == true {
                    assert_ne!(expected_data, &read_data[..]);
                }
            }
            for i in [0, 2, 5] {
                device.free(extent(i)).await.expect("free failed");
            }
        }
    }
}
