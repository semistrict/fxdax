// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// fxdax: Fuchsia's fake device is a range of memory. This one is an extent store in memory
// (backlog doc-4): it holds extents by ID, each as long as its furthest write, written once (until
// the next barrier seals them) and only by their branch, frees them, and simulates a crash by undoing any of what happened
// since the last barrier (writes and frees). It can record such crashes at random barriers
// (`Crashes`), and count the round trips its reads would take to a store answering from afar
// (`RoundTrips`). Unless told to be calm (`FakeDevice::calm`), it is as awkward as a store
// (`Awkward`): its numbers start large and its floor jumps, and its requests take turns of the
// executor to answer, in no order but what barriers keep.

use crate::buffer::{BufferFuture, BufferRef, MutableBufferRef};
use crate::buffer_allocator::{BufferAllocator, BufferSource};
use crate::{Device, DeviceHolder, ExtentId, NoExtent, OpenExtents, OtherBranch, SealedExtent};
use anyhow::{Error, ensure};
use async_trait::async_trait;
use block_protocol::{ReadOptions, WriteFlags, WriteOptions};
use fuchsia_sync::Mutex;
use rand::rngs::StdRng;
use rand::{RngExt as _, SeedableRng as _};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
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
    /// fxdax: those that were open as the seal in flight began (`Awkward::seal`): the seal's own
    /// write may yet write them, and no one may take them for open.
    sealing: HashSet<ExtentId>,
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
        self.sealing.clear();
        self.since_barrier.clear();
    }

    fn free(&mut self, extent: ExtentId) {
        if let Some(data) = self.extents.remove(&extent) {
            self.open.remove(&extent);
            self.sealing.remove(&extent);
            self.stored -= data.len() as u64;
            self.since_barrier.push(Change::Free { extent, data });
        }
    }

    /// Undoes each write of a block and each free since the last barrier, or not, at random: a
    /// block undone holds garbage, and an extent whose free is undone holds what it did. Returns
    /// what it undid.
    fn discard_random(&mut self, rng: &mut impl rand::Rng, block_size: usize) -> Vec<String> {
        let mut discarded = Vec::new();
        for change in std::mem::take(&mut self.since_barrier) {
            if rng.random::<bool>() {
                discarded.extend(self.undo(change, block_size));
            }
        }
        discarded
    }

    /// Undoes `change`, saying so if there was anything to undo.
    fn undo(&mut self, change: Change, block_size: usize) -> Option<String> {
        match change {
            Change::Write { extent, offset } => {
                let data = self.extents.get_mut(&extent)?;
                data[offset..offset + block_size].fill(0xaf);
                Some(format!("write of {extent:?} at {offset}"))
            }
            Change::Free { extent, data } => {
                if self.extents.contains_key(&extent) {
                    return None;
                }
                self.stored += data.len() as u64;
                self.extents.insert(extent, data);
                Some(format!("free of {extent:?}"))
            }
        }
    }

    /// fxdax: the changes since the last barrier that a crash loses together or not at all: a free,
    /// or the writes to an extent: of those, by their indices in `since_barrier`.
    fn lost_together(&self) -> Vec<Vec<usize>> {
        let mut together = Vec::new();
        let mut written = HashMap::new();
        for (index, change) in self.since_barrier.iter().enumerate() {
            match change {
                Change::Write { extent, .. } => {
                    let group = *written.entry(*extent).or_insert_with(|| {
                        together.push(Vec::new());
                        together.len() - 1
                    });
                    together[group].push(index);
                }
                Change::Free { .. } => together.push(vec![index]),
            }
        }
        together
    }

    /// fxdax: undoes the changes at `lost` since the last barrier.
    fn undo_lost(&mut self, lost: &[usize], block_size: usize) {
        let changes = std::mem::take(&mut self.since_barrier);
        let mut lost = lost.iter().copied().peekable();
        for (index, change) in changes.into_iter().enumerate() {
            if lost.next_if_eq(&index).is_some() {
                self.undo(change, block_size);
            }
        }
    }
}

/// Crashes of a device, recorded at random barriers once marked (`Crashes::mark`): each is what the
/// device would hold had it crashed just before the barrier, which is any of what happened since
/// the barrier before (`FakeDevice::discard_random_since_last_flush`). Or every crash, up to a bound
/// (`Crashes::every`), as ALICE and CrashMonkey have it.
pub struct Crashes(Mutex<CrashesInner>);

struct CrashesInner {
    rng: StdRng,
    /// What's recorded at a barrier.
    recording: Recording,
    /// The mark that crashes recorded from now on carry, if any are recorded.
    mark: Option<u64>,
    /// fxdax: whether the next barrier is recorded both ways (`Crashes::last`).
    last: bool,
    recorded: Vec<Crash>,
    /// fxdax: what `recording` got to.
    counts: CrashCounts,
}

enum Recording {
    /// A crash at one barrier in `one_in`, on average.
    Random { one_in: u32 },
    /// Every crash at each barrier: every subset, where a barrier has at most `most` things to lose
    /// (`Inner::lost_together`), and otherwise losing nothing, all, each alone, or all but each.
    Every { most: usize },
}

/// fxdax: what `Crashes::every` went through, which its bound is honest about.
#[derive(Clone, Copy, Debug, Default)]
pub struct CrashCounts {
    /// The barrier with anything to lose.
    pub barriers: u64,
    /// The crashes recorded.
    pub crashes: u64,
    /// The barriers with more to lose than every subset of it was tried for.
    pub beyond_the_bound: u64,
}

struct Crash {
    mark: u64,
    /// What the device held. Those of a barrier share it.
    inner: Arc<Inner>,
    /// The changes since the last barrier, by index, that this crash loses, if `inner` still holds
    /// them.
    lost: Vec<usize>,
    capacity: u64,
    block_size: u32,
    branch: u64,
    /// fxdax: the seed of the device that recovers, if it is awkward, and the floor before.
    awkward: Option<(u64, u64)>,
}

impl Crash {
    /// The device that recovered from this crash, what it held sealed.
    fn recovered(self) -> (u64, FakeDevice) {
        let mut inner = Arc::unwrap_or_clone(self.inner);
        inner.undo_lost(&self.lost, self.block_size as usize);
        inner.barrier();
        let device =
            FakeDevice::with_inner(inner, self.capacity, self.block_size).with_branch(self.branch);
        let device = match self.awkward {
            Some((seed, floor)) => device.recovered_awkward(seed, floor),
            None => device,
        };
        (self.mark, device)
    }
}

/// Every way to lose some of `together`: all of them, if there are at most `most`.
fn ways_to_lose(together: &[Vec<usize>], most: usize) -> Vec<Vec<usize>> {
    let chosen = |chosen: &mut dyn FnMut(usize) -> bool| {
        let groups = together.iter().enumerate().filter(|(index, _)| chosen(*index));
        let mut lost: Vec<usize> = groups.flat_map(|(_, group)| group).copied().collect();
        lost.sort_unstable();
        lost
    };
    let count = together.len();
    if count <= most {
        return (0..1u64 << count)
            .map(|mask| chosen(&mut |index| mask >> index & 1 == 1))
            .collect();
    }
    let alone = (0..count).map(|one| chosen(&mut |index| index == one));
    let but = (0..count).map(|one| chosen(&mut |index| index != one));
    let ends = [chosen(&mut |_| false), chosen(&mut |_| true)];
    ends.into_iter().chain(alone).chain(but).collect()
}

impl Crashes {
    fn with(seed: u64, recording: Recording) -> Arc<Self> {
        Arc::new(Self(Mutex::new(CrashesInner {
            rng: StdRng::seed_from_u64(seed),
            recording,
            mark: None,
            last: false,
            recorded: Vec::new(),
            counts: CrashCounts::default(),
        })))
    }

    /// Crashes recorded at one barrier in `one_in`, chosen, as is what each crash undoes, by an RNG
    /// seeded with `seed`.
    pub fn new(seed: u64, one_in: u32) -> Arc<Self> {
        Self::with(seed, Recording::Random { one_in })
    }

    /// fxdax: every crash at every barrier: every subset of what a crash may lose there, where that
    /// is at most `most` things, a free or an extent's writes each; and otherwise losing nothing,
    /// all, each alone, or all but each, which the counts say of how many barriers (`counts`). What
    /// recovers from each follows `seed`, if it is awkward.
    pub fn every(seed: u64, most: usize) -> Arc<Self> {
        Self::with(seed, Recording::Every { most })
    }

    /// Crashes recorded from now on carry `mark`, which tells a test what had happened before them:
    /// a crash carries the latest mark set before the barrier it was recorded at.
    pub fn mark(&self, mark: u64) {
        self.0.lock().mark = Some(mark);
    }

    /// Crashes stop being recorded, until the next mark.
    pub fn stop(&self) {
        self.0.lock().mark = None;
    }

    /// fxdax: the next barrier records a crash whatever else it would, and one that loses nothing:
    /// for a test that wants what a device holds, at last, as a crash leaves it, and as it stands.
    pub fn last(&self) {
        self.0.lock().last = true;
    }

    /// fxdax: what was gone through so far.
    pub fn counts(&self) -> CrashCounts {
        self.0.lock().counts
    }

    /// The crashes recorded, in order, as devices that recovered from them, with their marks: which
    /// may record crashes of their own (`FakeDevice::record_crashes`). Each is made as it is asked
    /// for, as a sequence may have thousands.
    pub fn take(&self) -> impl Iterator<Item = (u64, FakeDevice)> + use<> {
        std::mem::take(&mut self.0.lock().recorded).into_iter().map(Crash::recovered)
    }

    /// At random, or always, records `device` crashing before a barrier, as it holds `inner`.
    fn record(&self, device: &FakeDevice, inner: &Inner) {
        let mut guard = self.0.lock();
        let crashes = &mut *guard;
        let Some(mark) = crashes.mark else { return };
        let last = std::mem::take(&mut crashes.last);
        if last {
            crashes.record_crash(device, mark, Arc::new(inner.clone()), Vec::new());
        }
        match crashes.recording {
            Recording::Random { one_in } => {
                if !last && !crashes.rng.random_ratio(1, one_in) {
                    return;
                }
                let mut crashed = inner.clone();
                crashed.discard_random(&mut crashes.rng, device.allocator.block_size());
                crashes.record_crash(device, mark, Arc::new(crashed), Vec::new());
            }
            Recording::Every { most } => {
                let together = inner.lost_together();
                if together.is_empty() {
                    return;
                }
                crashes.counts.barriers += 1;
                crashes.counts.beyond_the_bound += (together.len() > most) as u64;
                let shared = Arc::new(inner.clone());
                for lost in ways_to_lose(&together, most) {
                    crashes.record_crash(device, mark, shared.clone(), lost);
                }
            }
        }
    }
}

impl CrashesInner {
    fn record_crash(
        &mut self,
        device: &FakeDevice,
        mark: u64,
        inner: Arc<Inner>,
        lost: Vec<usize>,
    ) {
        let awkward = (device.awkward.as_ref())
            .map(|_| (self.rng.random(), device.sequence_floor.load(Ordering::Relaxed)));
        self.counts.crashes += 1;
        self.recorded.push(Crash {
            mark,
            inner,
            lost,
            capacity: device.capacity,
            block_size: device.block_size(),
            branch: device.branch,
            awkward,
        });
    }
}

/// Waits for a turn of the executor, whatever else it has to do.
async fn turn() {
    let mut turned = false;
    std::future::poll_fn(|cx| {
        if std::mem::replace(&mut turned, true) {
            Poll::Ready(())
        } else {
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await;
}

/// fxdax: the most turns of the executor an awkward device's request takes to answer.
const MOST_TURNS: u32 = 16;

/// fxdax: a device as awkward as a store (backlog doc-4), which memory, answering at once and from
/// sequence number 0, is not: a search from 0 was tiny on it, and took a guest's allocator 2 GiB.
/// All of it follows a seed.
///
/// - A branch's numbers start large: what the branch wrote before this device, as a fork has, up to
///   its `next_sequence`, and half of branches, kept in a bucket, have a floor past that, which
///   jumps as another writer takes them over: at a crash, or a remount, of those a lease moved.
/// - Reads, writes, frees and barriers take turns of the executor to answer, at random, so that
///   those in flight together finish in any order. As a guest's driver holds them (`Seal` in
///   guest/rust/fxfs-linux/src/device.rs), a barrier waits for writes in flight and keeps others
///   out until it answers, and no one is told an extent is open from when it starts: the store
///   would have those fall on either side of it, a write's requests on both.
struct Awkward {
    rng: Mutex<StdRng>,
    /// Held by the seal in flight.
    sealing: futures::lock::Mutex<()>,
    /// Whether a seal keeps writes out.
    sealed: AtomicBool,
    /// The writes that seal nothing in flight.
    writes: AtomicUsize,
}

impl Awkward {
    fn new(seed: u64) -> Self {
        Self {
            rng: Mutex::new(StdRng::seed_from_u64(seed)),
            sealing: futures::lock::Mutex::new(()),
            sealed: AtomicBool::new(false),
            writes: AtomicUsize::new(0),
        }
    }

    /// Where `inner`'s branch, which has written nothing here, starts, and its `floor`.
    fn start(&self, inner: &mut Inner, floor: &AtomicU64) {
        let mut rng = self.rng.lock();
        inner.next_sequence = rng.random_range(1 << 20..1 << 40);
        let first_epoch = rng.random_range(1..1 << 16);
        floor.store(if rng.random() { first_epoch << EPOCH_SHIFT } else { 0 }, Ordering::Relaxed);
    }

    /// Moves a floor on, if a lease moved: a new writer's range starts past every number of the
    /// last's. A branch that has none, kept on the host alone, has no writer but the host's.
    fn take_over(&self, floor: &AtomicU64) {
        let mut rng = self.rng.lock();
        if floor.load(Ordering::Relaxed) != 0 && rng.random() {
            let epochs: u64 = rng.random_range(1..4);
            floor.fetch_add(epochs << EPOCH_SHIFT, Ordering::Relaxed);
        }
    }

    /// Takes the turns a request takes to answer: often none.
    async fn answer(&self) {
        let turns = {
            let mut rng = self.rng.lock();
            if rng.random() { rng.random_range(1..=MOST_TURNS) } else { 0 }
        };
        for _ in 0..turns {
            turn().await;
        }
    }

    /// A seal's way in: waits for another seal to end and for writes in flight, and keeps new ones
    /// out until what this returns is dropped.
    async fn seal(&self) -> Passage<'_> {
        let one = self.sealing.lock().await;
        self.sealed.store(true, Ordering::SeqCst);
        let passage = Passage { awkward: self, sealing: Some(one) };
        while self.writes.load(Ordering::SeqCst) > 0 {
            turn().await;
        }
        passage
    }

    /// A write's way in: waits for a seal to end, and holds others off until what this returns is
    /// dropped.
    async fn write(&self) -> Passage<'_> {
        loop {
            self.writes.fetch_add(1, Ordering::SeqCst);
            let passage = Passage { awkward: self, sealing: None };
            if !self.sealed.load(Ordering::SeqCst) {
                return passage;
            }
            drop(passage);
            turn().await;
        }
    }
}

/// A write, or a seal, in flight on an awkward device (`Awkward::write`, `Awkward::seal`).
struct Passage<'a> {
    awkward: &'a Awkward,
    /// The hold of a seal.
    sealing: Option<futures::lock::MutexGuard<'a, ()>>,
}

impl Drop for Passage<'_> {
    fn drop(&mut self) {
        match self.sealing {
            Some(_) => self.awkward.sealed.store(false, Ordering::SeqCst),
            None => drop(self.awkward.writes.fetch_sub(1, Ordering::SeqCst)),
        }
    }
}

/// fxdax: a store's writers' ranges are this far apart (`EPOCH_SHIFT` in crates/extent_remote).
const EPOCH_SHIFT: u32 = 40;

/// fxdax: the seed of the next device that isn't given one: FXDAX_DEVICE_SEED, or 0, then the next
/// for each device a process makes, so that a test run alone, as nextest runs each, sees the same
/// device, and a run with another seed other devices.
fn next_seed() -> u64 {
    static FIRST: OnceLock<u64> = OnceLock::new();
    static DEVICES: AtomicU64 = AtomicU64::new(0);
    let first = FIRST.get_or_init(|| {
        std::env::var("FXDAX_DEVICE_SEED").map_or(0, |seed| seed.parse().expect("a seed"))
    });
    first.wrapping_add(DEVICES.fetch_add(1, Ordering::Relaxed))
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
        turn().await;
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
    /// fxdax: how it is as awkward as a store, unless it is calm (`FakeDevice::calm`).
    awkward: Option<Awkward>,
    /// fxdax: what the filesystem may take for open, as a guest's device has it, which `Inner`'s
    /// `open`, what this device would refuse, isn't: a write opens nothing here.
    known_open: OpenExtents,
    /// fxdax: the devices reads of extents this one doesn't hold fall through to, which forks of it
    /// fall through to too (`FakeDevice::attach`), each with what counts the reads that reach it.
    attached: Vec<(Arc<DeviceHolder>, Arc<AtomicU64>)>,
}

const TRANSFER_HEAP_SIZE: usize = 64 * 1024 * 1024;

impl FakeDevice {
    /// A device holding at most `block_count` blocks of `block_size` bytes, as awkward as a store,
    /// as FXDAX_DEVICE_SEED says (`FakeDevice::awkward`).
    pub fn new(block_count: u64, block_size: u32) -> Self {
        Self::with_inner(Inner::default(), block_count * block_size as u64, block_size)
            .awkward(next_seed())
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
            awkward: None,
            known_open: OpenExtents::default(),
            attached: Vec::new(),
        }
    }

    /// fxdax: makes the device, new, as awkward as a store (`Awkward`), as `seed` says.
    pub fn awkward(mut self, seed: u64) -> Self {
        let awkward = Awkward::new(seed);
        awkward.start(self.inner.get_mut(), &self.sequence_floor);
        self.awkward = Some(awkward);
        self
    }

    /// fxdax: makes the device, new, answer at once, in order, with numbers from 0 and no floor: a
    /// test's counts and a benchmark's times depend on it.
    pub fn calm(mut self) -> Self {
        self.awkward = None;
        self.inner.get_mut().next_sequence = 0;
        self.sequence_floor.store(0, Ordering::Relaxed);
        self
    }

    /// fxdax: makes the device, what a crash left of an awkward one, awkward as `seed` says: its
    /// floor was `floor`, until someone else took over.
    fn recovered_awkward(mut self, seed: u64, floor: u64) -> Self {
        let awkward = Awkward::new(seed);
        self.sequence_floor.store(floor, Ordering::Relaxed);
        awkward.take_over(&self.sequence_floor);
        self.awkward = Some(awkward);
        self
    }

    /// Makes `branch` the branch of the device's extents.
    pub fn with_branch(mut self, branch: u64) -> Self {
        self.branch = branch;
        self
    }

    /// fxdax: makes reads of extents the device doesn't hold fall through to `device`, as a VM's
    /// reads do to a fork attached to it (backlog doc-10). Returns what counts those that reach it.
    pub fn attach(&mut self, device: DeviceHolder) -> Arc<AtomicU64> {
        let reads = Arc::new(AtomicU64::new(0));
        self.attached.push((Arc::new(device), reads.clone()));
        reads
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

    /// fxdax: a calm device of `branch` holding what this one does, sealed.
    fn copy_of_branch(&self, branch: u64) -> Self {
        let mut inner = self.inner.lock().clone();
        inner.open.clear();
        inner.sealing.clear();
        if branch != self.branch {
            // The fork's branch has written nothing.
            inner.next_sequence = 0;
        }
        Self::with_inner(inner, self.capacity, self.block_size()).with_branch(branch)
    }

    /// fxdax: a copy of the device, as if it crashed losing nothing and recovered, awkward as
    /// `seed` says, if it is: one image, many times, as `corrupt` leaves each.
    pub fn recovered(&self, seed: u64) -> Self {
        let copy = self.copy_of_branch(self.branch);
        copy.inner.lock().barrier();
        match self.awkward {
            Some(_) => copy.recovered_awkward(seed, self.sequence_floor.load(Ordering::Relaxed)),
            None => copy,
        }
    }

    /// fxdax: what `extent` holds, if it exists, as `corrupt` finds it.
    pub fn held(&self, extent: ExtentId) -> Option<Vec<u8>> {
        self.inner.lock().extents.get(&extent).cloned()
    }

    /// fxdax: changes what the device holds, its extents and its root, as no request may: as a host
    /// failure or a bad upload does.
    pub fn corrupt(&self, corrupt: impl FnOnce(&mut HashMap<ExtentId, Vec<u8>>, &mut Vec<u8>)) {
        let mut inner = self.inner.lock();
        let inner = &mut *inner;
        corrupt(&mut inner.extents, &mut inner.root);
        inner.stored = inner.extents.values().map(|data| data.len() as u64).sum();
    }

    /// What a free checks before it frees anything.
    fn free_begins(&self) -> Result<(), Error> {
        ensure!(!self.closed.load(Ordering::Relaxed));
        ensure!(!self.read_only.load(Ordering::Relaxed));
        (self.operation_closure)(Op::Free)
    }

    /// fxdax: takes the turns an awkward device's read or free takes to answer.
    async fn answer(&self) {
        if let Some(awkward) = &self.awkward {
            awkward.answer().await;
        }
    }

    /// fxdax: waits for the place of a write, or of a request that `seals`, among seals, on an
    /// awkward device, then the turns it takes to answer.
    async fn passage(&self, seals: bool) -> Option<Passage<'_>> {
        let awkward = self.awkward.as_ref()?;
        let passage = if seals {
            let passage = awkward.seal().await;
            let mut inner = self.inner.lock();
            let open = std::mem::take(&mut inner.open);
            inner.sealing.extend(open);
            self.known_open.seal();
            passage
        } else {
            awkward.write().await
        };
        awkward.answer().await;
        Some(passage)
    }

    /// A barrier: what happened before it survives a crash, and the extents written are sealed.
    fn barrier(&self, inner: &mut Inner) {
        if let Some(crashes) = &self.crashes {
            crashes.record(self, inner);
        }
        self.known_open.seal();
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
        self.answer().await;
        {
            let inner = self.inner.lock();
            if let Some(data) = inner.extents.get(&extent) {
                let offset = offset as usize;
                let held = data.len().saturating_sub(offset).min(buffer.len());
                let (mut held_part, mut rest) = buffer.reborrow().split_at_mut(held);
                if held > 0 {
                    held_part.copy_from_slice(&data[offset..offset + held]);
                }
                rest.fill(0);
                return Ok(());
            }
        }
        for (device, reads) in &self.attached {
            match device.read(extent, offset, buffer.reborrow()).await {
                Err(error) if error.is::<NoExtent>() => {}
                result => {
                    reads.fetch_add(1, Ordering::Relaxed);
                    return result;
                }
            }
        }
        Err(NoExtent(extent).into())
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
        let seals = write_opts.flags.intersects(WriteFlags::PRE_BARRIER | WriteFlags::POST_BARRIER);
        let _passage = self.passage(seals).await;
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
        if inner.extents.contains_key(&extent)
            && !inner.open.contains(&extent)
            && !inner.sealing.contains(&extent)
        {
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
        self.free_extents(&[extent]).await.pop().expect("a result")
    }

    /// fxdax: as one request, as a guest's `FREE_MANY`, which waits and answers once.
    async fn free_extents(&self, extents: &[ExtentId]) -> Vec<Result<(), Error>> {
        let mut results = Vec::with_capacity(extents.len());
        for _ in extents {
            results.push(self.free_begins());
        }
        if let Some(held) = &self.frees_held_by {
            drop(held.lock().await);
        }
        self.answer().await;
        // fxdax: what reads fall through to is a filesystem's never to free.
        for &extent in extents {
            let holds = self.inner.lock().extents.contains_key(&extent);
            if !holds {
                for (device, _) in &self.attached {
                    let mut buffer = self.allocate_buffer(self.block_size() as usize).await;
                    let read = device.read(extent, 0, buffer.as_mut()).await;
                    assert!(read.is_err(), "freeing {extent:?}, which is attached");
                }
            }
        }
        let mut inner = self.inner.lock();
        for (extent, result) in extents.iter().zip(&results) {
            if result.is_ok() {
                inner.free(*extent);
            }
        }
        results
    }

    fn is_open(&self, extent: ExtentId) -> bool {
        self.known_open.contains(extent)
    }

    fn opened(&self, extent: ExtentId) {
        self.known_open.minted(extent);
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
        let _passage = self.passage(true).await;
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
        let _passage = self.passage(true).await;
        // fxdax: first: a flush that fails seals nothing, as a commit that fails doesn't.
        (self.operation_closure)(Op::Flush)?;
        self.barrier(&mut self.inner.lock());
        Ok(())
    }

    fn reopen(&self, read_only: bool) {
        // Whatever was written before is sealed.
        self.inner.lock().open.clear();
        self.known_open.seal();
        if let Some(awkward) = &self.awkward {
            awkward.take_over(&self.sequence_floor);
        }
        self.closed.store(false, Ordering::Relaxed);
        self.read_only.store(read_only, Ordering::Relaxed);
    }

    fn is_read_only(&self) -> bool {
        self.read_only.load(Ordering::Relaxed)
    }

    fn extents(&self) -> Vec<ExtentId> {
        let mut extents: Vec<_> = self.inner.lock().extents.keys().copied().collect();
        extents.sort();
        extents
    }

    fn snapshot(&self) -> Result<DeviceHolder, Error> {
        self.fork(self.branch)
    }

    /// The snapshot is of a device that crashed, and recovered with what it held sealed.
    fn fork(&self, branch: u64) -> Result<DeviceHolder, Error> {
        let mut fork = self.copy_of_branch(branch);
        fork.attached = self.attached.clone();
        let fork = match &self.awkward {
            Some(awkward) => {
                let seed = awkward.rng.lock().random();
                if branch == self.branch {
                    fork.recovered_awkward(seed, self.sequence_floor.load(Ordering::Relaxed))
                } else {
                    fork.awkward(seed)
                }
            }
            None => fork,
        };
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
    use super::{Crashes, EPOCH_SHIFT, FakeDevice, RoundTrips};
    use crate::Device;
    use crate::{ExtentId, OtherBranch, SealedExtent};
    use block_protocol::{WriteFlags, WriteOptions};
    use fuchsia_sync::Mutex;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

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
        let device = FakeDevice::new(8, BS as u32).calm().with_branch(7);
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
        // An extent can be written until the next barrier, and no write says it is open.
        write(&device, a, 0, 1, BS).await;
        write(&device, a, BS as u64, 2, BS).await;
        assert!(!device.is_open(a));
        device.flush().await.expect("flush failed");
        let buffer = device.allocate_buffer(BS).await;
        let error = device.write(a, 2 * BS as u64, buffer.as_ref()).await.expect_err("rewrote");
        assert_eq!(error.downcast_ref::<SealedExtent>(), Some(&SealedExtent(a)));

        // A write with a barrier first seals what was written before it.
        device.opened(b);
        assert!(device.is_open(b));
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
        assert!(device.is_open(a));
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

    /// A write reaching an extent a barrier came before opens nothing: the device takes it, the
    /// barrier having sealed nothing of the extent, which goes on reading as sealed, as it is once
    /// the next barrier comes (TASK-153.3).
    #[fuchsia::test]
    async fn test_writes_open_nothing() {
        const BS: usize = TEST_DEVICE_BLOCK_SIZE;
        let device = FakeDevice::new(8, BS as u32);
        let extent = ExtentId::new(1, 1);
        device.opened(extent);
        device.flush().await.expect("flush failed");
        assert!(!device.is_open(extent));

        write(&device, extent, 0, 1, BS).await;
        assert!(!device.is_open(extent));
        write(&device, extent, BS as u64, 2, BS).await;
        assert!(!device.is_open(extent));

        device.flush().await.expect("flush failed");
        let buffer = device.allocate_buffer(BS).await;
        let error = device.write(extent, 0, buffer.as_ref()).await.expect_err("rewrote");
        assert_eq!(error.downcast_ref::<SealedExtent>(), Some(&SealedExtent(extent)));
        assert!(!device.is_open(extent));
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
        let recorded: Vec<_> =
            recorded.map(|(mark, device)| (mark, crate::DeviceHolder::new(device))).collect();
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
        assert!(crashes.take().next().is_none());
    }

    /// What a crash left of `extents`' first blocks, each `Some` of what it held.
    async fn held(device: &FakeDevice, extents: &[ExtentId]) -> Vec<Option<u8>> {
        let mut held = Vec::new();
        for extent in extents {
            let mut buffer = device.allocate_buffer(TEST_DEVICE_BLOCK_SIZE).await;
            let read = device.read(*extent, 0, buffer.as_mut()).await;
            held.push(read.is_ok().then(|| buffer.to_vec()[0]));
        }
        held
    }

    /// fxdax: every crash at a barrier is every subset of what it may lose: writes to an extent
    /// lost together, frees alone. A barrier with nothing to lose has no crash.
    #[fuchsia::test]
    async fn test_every_crash() {
        const BS: usize = TEST_DEVICE_BLOCK_SIZE;
        let crashes = Crashes::every(1, 3);
        let device = FakeDevice::new(8, BS as u32).calm().record_crashes(crashes.clone());
        let extents = [ExtentId::new(1, 1), ExtentId::new(1, 2), ExtentId::new(1, 3)];
        write(&device, extents[0], 0, 1, BS).await;
        device.flush().await.expect("flush failed");
        crashes.mark(1);
        write(&device, extents[1], 0, 2, BS).await;
        write(&device, extents[1], BS as u64, 2, BS).await;
        write(&device, extents[2], 0, 3, BS).await;
        device.free(extents[0]).await.expect("free failed");
        device.flush().await.expect("flush failed");
        device.flush().await.expect("flush failed");

        let mut left = Vec::new();
        for (mark, recovered) in crashes.take() {
            assert_eq!(mark, 1);
            left.push(held(&recovered, &extents).await);
        }
        const LOST: u8 = 0xaf;
        let mut expected = Vec::new();
        for b in [2, LOST] {
            for c in [3, LOST] {
                for freed in [None, Some(1)] {
                    expected.push(vec![freed, Some(b), Some(c)]);
                }
            }
        }
        left.sort();
        expected.sort();
        assert_eq!(left, expected);
        let counts = crashes.counts();
        assert_eq!((counts.barriers, counts.crashes, counts.beyond_the_bound), (1, 8, 0));
    }

    /// fxdax: past its bound, a barrier's crashes are losing nothing, all, each alone, or all but
    /// each, which the counts own up to.
    #[fuchsia::test]
    async fn test_every_crash_past_the_bound() {
        const BS: usize = TEST_DEVICE_BLOCK_SIZE;
        let crashes = Crashes::every(1, 2);
        let device = FakeDevice::new(8, BS as u32).calm().record_crashes(crashes.clone());
        let extents: Vec<_> = (1..=4).map(|sequence| ExtentId::new(1, sequence)).collect();
        crashes.mark(1);
        for extent in &extents {
            write(&device, *extent, 0, 1, BS).await;
        }
        device.flush().await.expect("flush failed");

        let mut lost = Vec::new();
        for (_, recovered) in crashes.take() {
            let left = held(&recovered, &extents).await;
            lost.push(left.iter().filter(|block| **block != Some(1)).count());
        }
        lost.sort();
        assert_eq!(lost, [0, 1, 1, 1, 1, 3, 3, 3, 3, 4]);
        let counts = crashes.counts();
        assert_eq!((counts.barriers, counts.crashes, counts.beyond_the_bound), (1, 10, 1));
    }

    /// fxdax: a free waits while the test holds the frees, and is made once it lets go.
    #[fuchsia::test]
    async fn test_frees_held() {
        const BS: usize = TEST_DEVICE_BLOCK_SIZE;
        let held = Arc::new(futures::lock::Mutex::new(()));
        let device = FakeDevice::new(8, BS as u32).calm().with_frees_held_by(held.clone());
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
        let device = FakeDevice::new(128, BS as u32).calm().with_round_trips(trips.clone());
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

    /// fxdax: an awkward device's numbers start large, as its seed says, some past a floor, and
    /// only those floors jump, as another writer takes over: all of it as its seed says.
    #[fuchsia::test]
    async fn test_awkward_numbers() {
        let numbers = async |seed| {
            let device = FakeDevice::new(8, 512).awkward(seed);
            let next = device.next_sequence().await.unwrap();
            assert!(next >= 1 << 20, "started from {next}");
            let floor = device.sequence_floor().await.unwrap();
            let mut floors = vec![floor];
            for _ in 0..16 {
                device.reopen(false);
                floors.push(device.sequence_floor().await.unwrap());
            }
            (next, floors)
        };
        let mut floorless = 0;
        for seed in 0..32 {
            let (next, floors) = numbers(seed).await;
            assert_eq!((next, floors.clone()), numbers(seed).await, "seed {seed}");
            if floors[0] == 0 {
                floorless += 1;
                assert_eq!(floors, vec![0; floors.len()]);
            } else {
                assert!(floors[0] > next, "{next} isn't below its floor, {}", floors[0]);
                assert!(floors.is_sorted());
                assert!(floors[16] > floors[0], "seed {seed}: its floor never jumped");
                assert_eq!(floors[16] % (1 << EPOCH_SHIFT), 0);
            }
        }
        assert!((1..32).contains(&floorless), "{floorless} of 32 had no floor");
    }

    /// fxdax: what a crash leaves of an awkward device is awkward: a floor as it was, or past it,
    /// and numbers as they were.
    #[fuchsia::test]
    async fn test_awkward_crashes() {
        const BS: usize = TEST_DEVICE_BLOCK_SIZE;
        let seed = (0..).find(|seed| {
            let device = FakeDevice::new(8, BS as u32).awkward(*seed);
            device.sequence_floor.load(Ordering::Relaxed) != 0
        });
        let crashes = Crashes::new(1, 1);
        let device =
            FakeDevice::new(8, BS as u32).awkward(seed.unwrap()).record_crashes(crashes.clone());
        crashes.mark(1);
        let floor = device.sequence_floor().await.unwrap();
        let extent = ExtentId::new(1, floor);
        write(&device, extent, 0, 1, BS).await;
        device.flush().await.expect("flush failed");
        let (_, recovered) = crashes.take().last().expect("a crash");
        assert_eq!(recovered.next_sequence().await.unwrap(), floor + 1);
        assert!(recovered.sequence_floor().await.unwrap() >= floor);
        assert!(recovered.awkward.is_some());
    }

    /// fxdax: reads, writes and frees issued together on an awkward device end in another order
    /// than they were issued, as its seed says.
    #[fuchsia::test]
    async fn test_awkward_requests_finish_out_of_order() {
        const BS: usize = TEST_DEVICE_BLOCK_SIZE;
        async fn order(seed: u64) -> Vec<usize> {
            let device = FakeDevice::new(64, BS as u32).awkward(seed);
            let extent = ExtentId::new(1, 1 << 50);
            let finished = Mutex::new(Vec::new());
            let writes = (0..32).map(|block| {
                let (device, finished) = (&device, &finished);
                async move {
                    write(device, extent, block as u64 * BS as u64, block as u8, BS).await;
                    finished.lock().push(block);
                }
            });
            futures::future::join_all(writes).await;
            let reads = (0..32).map(|block| read(&device, extent, block as u64 * BS as u64, BS));
            let read = futures::future::join_all(reads).await;
            assert_eq!(read, (0..32).map(|block| vec![block as u8; BS]).collect::<Vec<_>>());
            finished.into_inner()
        }
        let (first, again, other) = (order(1).await, order(1).await, order(2).await);
        assert_ne!(first, (0..32).collect::<Vec<_>>());
        assert_eq!(first, again);
        assert_ne!(first, other);
    }

    /// fxdax: a seal on an awkward device, as a guest's: it waits for writes in flight and keeps
    /// new ones out until it answers, and no one takes an extent for open from when it starts.
    #[fuchsia::test]
    async fn test_awkward_seals() {
        const BS: usize = TEST_DEVICE_BLOCK_SIZE;
        let device = FakeDevice::new(8, BS as u32).awkward(3);
        let (open, other) = (ExtentId::new(1, 1 << 50), ExtentId::new(1, (1 << 50) + 1));
        let awkward = device.awkward.as_ref().unwrap();
        device.opened(open);
        write(&device, open, 0, 1, BS).await;

        let writing = awkward.write().await;
        let mut seal = std::pin::pin!(device.flush());
        assert!(futures::poll!(seal.as_mut()).is_pending());
        assert!(device.is_open(open), "a seal waiting for a write has started");
        drop(writing);
        while !awkward.sealed.load(Ordering::SeqCst) || device.is_open(open) {
            assert!(futures::poll!(seal.as_mut()).is_pending());
        }
        let mut late = std::pin::pin!(write(&device, other, 0, 2, BS));
        assert!(futures::poll!(late.as_mut()).is_pending());
        seal.await.expect("flush failed");
        late.await;
        assert!(!device.is_open(open));
        // Written before the seal, it would be sealed.
        let buffer = device.allocate_buffer(BS).await;
        device.write(other, BS as u64, buffer.as_ref()).await.expect("the write came before");
    }

    /// fxdax: a seal's own write may write what was open when it started.
    #[fuchsia::test]
    async fn test_awkward_commit_writes_what_was_open() {
        const BS: usize = TEST_DEVICE_BLOCK_SIZE;
        let device = FakeDevice::new(8, BS as u32).awkward(3);
        let extent = ExtentId::new(1, 1 << 50);
        device.opened(extent);
        write(&device, extent, 0, 1, BS).await;
        let buffer = device.allocate_buffer(BS).await;
        let options = WriteOptions { flags: WriteFlags::POST_BARRIER, ..Default::default() };
        device
            .write_with_opts(extent, BS as u64, buffer.as_ref(), options)
            .await
            .expect("write failed");
        assert!(!device.is_open(extent));
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
