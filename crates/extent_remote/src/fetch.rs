// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A host's reads of extents from a bucket (doc-7, Reading): by whole [`PART`]s of objects, each
//! a GET of its stored bytes, which the object's table says where are, checked against its CRC
//! and decompressed; one GET for a part however many reads want it at once; GETs in flight
//! bounded in number and bytes, by a [`Budget`] a host's fetchers share, where reads waiting for
//! their data go before read-ahead. An extent the collector moved is read where it went. Every
//! read of an extent's data is a fetcher's: a host's, or one of its own ([`read`]).
//!
//! A host's fetcher keeps the parts it fetches in the host's cache, as stored, and those it
//! fetched last in memory too, as fetched, which the first read that wants one unpacks: a part
//! read ahead is unpacked once, by the read it was read ahead of, not read back from the cache and
//! unpacked again (TASK-92.21). What its fetchers keep in memory, those parts and objects' tails,
//! the host's fetchers share ([`Memory`]), as they share its budget and its cache: they keep each
//! once, however many of the host's stores read it, as the forks of one base read its parts, and
//! within bounds that hold for the host, however many stores it serves.
//!
//! A checkpoint's extents are looked up a block at a time ([`Fetcher::lookup`]): its tail holds its
//! table, which says where the block that would hold an extent's entry is, read by the parts it is
//! in, as extents' data is (doc-7, Checkpoints).
//!
//! An extent is read where the footer of the object at its key says it is, not where the reader's
//! index does: keys of segments name no writer, so a writer taken over, its create landing late,
//! can create an object at a key the collector deleted, holding extents of its own. Neither CRCs
//! nor keys tell such an object from the one deleted; its footer, which names what it holds, does,
//! and an object that holds no longer what a reader's index says is read as one gone: where its
//! moves say. The footer and the table are tied to one version of an object by their CRCs, which
//! the trailer and the table hold, and the parts by theirs, which the table holds.

use crate::Error;
use crate::bucket::{self, Bucket, ByteRange};
use crate::cache::Cache;
use crate::checkpoint::Table;
use crate::layout::{self, Footer, Kind, Location, Moves, PART};
use crate::moves::MOVES;
use crate::object::{self, TAIL};
use futures::channel::oneshot;
use futures::future::{BoxFuture, FutureExt as _, Shared};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ops::Range;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use storage_device::ExtentId;

/// Whether a read waits for its data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    Demand,
    Ahead,
}

/// How many requests may be in flight, of how many bytes in all. One goes whatever its size.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub requests: usize,
    pub bytes: u64,
}

impl Limits {
    /// No bound: every request goes at once.
    pub const UNBOUNDED: Self = Self { requests: usize::MAX, bytes: u64::MAX };
}

/// The bytes of `extent`, at `location` in `bucket` or where the collector moved it from there,
/// read once, by a fetcher of its own that keeps nothing and bounds no GET: a ranged read of its
/// object's tail, and of each part it is in, checked and decompressed. A host reads through its
/// [`Fetcher`], which keeps the parts it fetches, under its budget.
pub async fn read<B: Bucket>(
    bucket: &Arc<B>,
    extent: ExtentId,
    location: &Location,
) -> Result<Vec<u8>, Error> {
    let fetcher = Fetcher::new(bucket.clone(), Budget::new(Limits::UNBOUNDED), Memory::new(), None);
    fetcher.read(extent, location, 0, location.len, Priority::Demand).await
}

/// Requests in flight, and those waiting to go, demand first, each in the order it came: a host's
/// GETs, which hold the parts they fetch, or the PUTs its writers upload objects with, which hold
/// the objects' bytes ([`Writer::uploading_within`](crate::Writer::uploading_within)).
pub struct Budget {
    limits: Limits,
    queues: Mutex<Queues>,
}

#[derive(Default)]
struct Queues {
    requests: usize,
    bytes: u64,
    tickets: u64,
    /// By [`Priority`].
    waiting: [VecDeque<Waiter>; 2],
}

struct Waiter {
    ticket: u64,
    bytes: u64,
    go: oneshot::Sender<Grant>,
}

/// A request's place in its budget, given up when it is dropped.
pub(crate) struct Grant {
    budget: Option<Arc<Budget>>,
    bytes: u64,
}

impl Drop for Grant {
    fn drop(&mut self) {
        if let Some(budget) = self.budget.take() {
            let mut queues = budget.queues.lock().unwrap();
            queues.requests -= 1;
            queues.bytes -= self.bytes;
            budget.admit(&mut queues);
        }
    }
}

impl Budget {
    pub fn new(limits: Limits) -> Arc<Self> {
        Arc::new(Self { limits, queues: Mutex::default() })
    }

    /// A place for a request of `bytes`: its ticket, by which [`Budget::promote`] finds it, and the
    /// wait for it.
    pub(crate) fn enter(
        self: &Arc<Self>,
        priority: Priority,
        bytes: u64,
    ) -> (u64, impl Future<Output = Grant> + Send + use<>) {
        let (go, granted) = oneshot::channel();
        let mut queues = self.queues.lock().unwrap();
        queues.tickets += 1;
        let ticket = queues.tickets;
        queues.waiting[priority as usize].push_back(Waiter { ticket, bytes, go });
        self.admit(&mut queues);
        // Its sender goes only with a grant, and the budget, which holds it, outlives this.
        (ticket, async move { granted.await.expect("a waiter is granted its place") })
    }

    /// Moves the request of `ticket`, if it waits behind demand, up to it.
    fn promote(self: &Arc<Self>, ticket: u64) {
        let mut queues = self.queues.lock().unwrap();
        let ahead = &mut queues.waiting[Priority::Ahead as usize];
        if let Some(at) = ahead.iter().position(|waiter| waiter.ticket == ticket) {
            let waiter = ahead.remove(at).unwrap();
            queues.waiting[Priority::Demand as usize].push_back(waiter);
            self.admit(&mut queues);
        }
    }

    /// Lets the requests waiting first go, as far as the limits allow.
    fn admit(self: &Arc<Self>, queues: &mut Queues) {
        while let Some(queue) = queues.waiting.iter_mut().find(|queue| !queue.is_empty()) {
            let bytes = queue[0].bytes;
            let fits =
                queues.requests < self.limits.requests && queues.bytes + bytes <= self.limits.bytes;
            if queues.requests > 0 && !fits {
                return;
            }
            let waiter = queue.pop_front().unwrap();
            queues.requests += 1;
            queues.bytes += bytes;
            let grant = Grant { budget: Some(self.clone()), bytes };
            // Its reader gave up: its place goes to the next.
            if let Err(mut grant) = waiter.go.send(grant) {
                grant.budget = None;
                queues.requests -= 1;
                queues.bytes -= bytes;
            }
        }
    }
}

/// An object's frame, where its parts are stored, and where in its data each extent it holds is,
/// as its footer says, or, a checkpoint's, its table: its tail.
#[derive(Debug)]
struct Tail {
    parts: extent_frame::Parts,
    /// Each extent's offset in its data, and its length.
    extents: HashMap<ExtentId, (u64, u64)>,
    /// A checkpoint's table, which its footer is.
    table: Option<Table>,
}

impl Tail {
    /// The tail of the object at `key`, as read, `tail`: a segment's footer names the extents
    /// sealed in it, and moves' those the collector moved there; an extent stored on its own is
    /// its object's whole data, which its key names; a checkpoint's footer is its table. Other
    /// objects hold no extents.
    fn of(key: &str, tail: object::Tail) -> Result<Self, Error> {
        let (footer, parts) = (tail.footer(), &tail.parts);
        let (extents, table) = match layout::kind(key) {
            Kind::Segment { .. } => (
                (Footer::decode(footer)?.extents.into_iter())
                    .map(|(extent, offset, len)| (extent, (offset, len)))
                    .collect(),
                None,
            ),
            Kind::Moves { .. } => (
                (layout::decode::<Moves>(footer)?.extents.into_iter())
                    .map(|(extent, to)| (extent, (to.offset, to.len)))
                    .collect(),
                None,
            ),
            Kind::Extent { branch, sequence, .. } => (
                HashMap::from([(ExtentId::new(branch, sequence), (0, parts.frame.data_len))]),
                None,
            ),
            Kind::Checkpoint { .. } => (HashMap::new(), Some(Table::decode(footer)?)),
            _ => (HashMap::new(), None),
        };
        Ok(Self { parts: tail.parts, extents, table })
    }

    /// Where part `part` of the object at `key` is stored, and how long it is unpacked.
    fn stored(&self, key: &str, part: u64) -> Result<(extent_frame::Stored, u64), Error> {
        let (place, stored) = self
            .parts
            .part(part)
            .map_err(|_| Error::Corrupt(format!("part {part} of {key}, past its end")))?;
        Ok((stored, place.end - place.start))
    }

    /// About the bytes it takes in memory: where its parts are stored, where its extents are, and
    /// a checkpoint's table, which takes about what its footer does as read.
    fn size(&self) -> u64 {
        use std::mem::size_of;
        // A map's entries, and a byte of control each, in a table an eighth larger than they fill.
        let extents = self.extents.capacity() * (size_of::<(ExtentId, (u64, u64))>() + 1) * 8 / 7;
        let parts = self.parts.stored.capacity() * size_of::<extent_frame::Stored>();
        let table = self.table.as_ref().map_or(0, |_| self.parts.frame.footer_len as usize);
        (size_of::<Self>() + extents + parts + table) as u64
    }
}

/// What a host's fetchers keep in memory, which they share, as they share its budget and its
/// cache: objects' tails, and the parts fetched last, each kept once for the host however many of
/// its stores read it, within bounds that hold for the host, not for each store (TASK-92.21,
/// TASK-92.22). The host's fetchers read one bucket, as they share one cache: what is kept is kept
/// by key, as the cache keeps it; a part is taken only by a tail that says it is stored as it was,
/// and a tail goes once a part fails the CRC it gives.
#[derive(Default)]
pub struct Memory {
    tails: Mutex<Tails>,
    recent: Mutex<Recent>,
}

impl Memory {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }
}

/// How many bytes of objects' tails a host's fetchers keep in memory, those used least recently
/// going first: and kept in the cache, if they keep one, from which they are read again
/// (TASK-92.22).
const TAILS: u64 = 64 << 20;

/// Objects' tails the host's fetchers keep in memory, or the reads of them in flight, by key, least
/// recently used first, and the bytes they take ([`Tail::size`]): [`TAIL`] for a read in flight,
/// which reads as many at first. None is the tail of an object gone.
#[derive(Default)]
struct Tails {
    kept: HashMap<String, KeptTail>,
    by_use: BTreeMap<u64, String>,
    clock: u64,
    bytes: u64,
}

struct KeptTail {
    /// When it was used last: its place in `by_use`.
    used: u64,
    /// Which read of it it is.
    read: u64,
    size: u64,
    flight: Flight<Option<Arc<Tail>>>,
}

impl Tails {
    /// The read of the tail of the object at `key`, kept or in flight, if there is one: used now.
    fn get(&mut self, key: &str) -> Option<Flight<Option<Arc<Tail>>>> {
        let kept = self.kept.get_mut(key)?;
        self.clock += 1;
        self.by_use.remove(&kept.used);
        kept.used = self.clock;
        self.by_use.insert(self.clock, key.to_owned());
        Some(kept.flight.clone())
    }

    /// The number of the next read of a tail.
    fn next_read(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// Keeps `flight`, read `read` of the tail of the object at `key`, now going.
    fn insert(&mut self, key: &str, read: u64, flight: Flight<Option<Arc<Tail>>>) {
        self.remove(key, None);
        self.kept.insert(key.to_owned(), KeptTail { used: read, read, size: 0, flight });
        self.by_use.insert(read, key.to_owned());
        self.settle(key, read, TAIL);
    }

    /// Read `read` of the tail of the object at `key` takes `size` bytes, unless it was dropped
    /// since: those used least recently go while they all take more than [`TAILS`].
    fn settle(&mut self, key: &str, read: u64, size: u64) {
        let Some(kept) = self.kept.get_mut(key).filter(|kept| kept.read == read) else { return };
        self.bytes = self.bytes - kept.size + size;
        kept.size = size;
        while self.bytes > TAILS
            && let Some((_, oldest)) = self.by_use.pop_first()
        {
            let dropped = self.kept.remove(&oldest).expect("a tail kept");
            self.bytes -= dropped.size;
        }
    }

    /// Drops the tail of the object at `key`: if it is read `read`, if one is given.
    fn remove(&mut self, key: &str, read: Option<u64>) {
        if let Some(kept) = self.kept.get(key)
            && read.is_none_or(|read| read == kept.read)
        {
            let kept = self.kept.remove(key).unwrap();
            self.by_use.remove(&kept.used);
            self.bytes -= kept.size;
        }
    }
}

/// A GET, or what it found, which reads that come while it is in flight share.
type Flight<T> = Shared<BoxFuture<'static, Result<T, Error>>>;

/// A part of an object, read by its tail.
#[derive(Clone)]
enum Part {
    /// Its bytes.
    Data(Arc<Fetched>),
    /// The object is gone.
    Gone,
    /// Its stored bytes fail the CRC its tail gives, or are past the object's end: the object is no
    /// longer the one the tail is of, another created at its key since, or they are corrupt.
    Changed,
}

/// A part's stored bytes, checked against its CRC, unpacked once a read wants them: a read ahead
/// never does, so that a part is unpacked once, by the read that wants it (TASK-92.21).
struct Fetched {
    stored: extent_frame::Stored,
    /// How long it is unpacked.
    len: u64,
    bytes: Arc<[u8]>,
    unpacked: OnceLock<Result<Arc<[u8]>, Error>>,
}

impl Fetched {
    /// Part `part` of the object at `key`, of `bytes`, which are as `stored` says, checked.
    fn new(stored: extent_frame::Stored, len: u64, bytes: Vec<u8>) -> Self {
        Self { stored, len, bytes: bytes.into(), unpacked: OnceLock::new() }
    }

    /// Its bytes, unpacked, those of part `part` of the object at `key`.
    fn unpacked(&self, key: &str, part: u64) -> Result<Arc<[u8]>, Error> {
        let unpacked = self.unpacked.get_or_init(|| match self.stored.encoding {
            extent_frame::Encoding::Raw => Ok(self.bytes.clone()),
            extent_frame::Encoding::Zstd => match self.stored.unpack(&self.bytes, self.len) {
                Ok(unpacked) => Ok(unpacked.into()),
                Err(err) => Err(Error::Corrupt(format!("part {part} of {key}: {err}"))),
            },
        });
        unpacked.clone()
    }

    /// The bytes it takes, unpacked too: a raw part's unpacked bytes are its stored bytes.
    fn size(&self) -> u64 {
        match self.stored.encoding {
            extent_frame::Encoding::Raw => self.len,
            extent_frame::Encoding::Zstd => self.bytes.len() as u64 + self.len,
        }
    }
}

/// How many bytes of the parts fetched last a host's fetchers keep in memory, for the reads after:
/// a read ahead's, until the read that wants it, a part a reader reads a little of at a time, and
/// one a fork's store reads after its base's did.
const RECENT: u64 = 64 << 20;

/// The parts fetched last, least recently used first, by their objects' keys and numbers, and the
/// bytes they take.
#[derive(Default)]
struct Recent {
    parts: HashMap<(String, u64), (u64, Arc<Fetched>)>,
    by_use: BTreeMap<u64, (String, u64)>,
    clock: u64,
    bytes: u64,
}

impl Recent {
    /// Part `part` of the object at `key`, if kept, as `stored` says it is stored.
    fn get(&mut self, key: &str, part: u64, stored: &extent_frame::Stored) -> Option<Arc<Fetched>> {
        let id = (key.to_owned(), part);
        let (used, fetched) = self.parts.get_mut(&id)?;
        if fetched.stored != *stored {
            return None;
        }
        self.clock += 1;
        self.by_use.remove(used);
        *used = self.clock;
        self.by_use.insert(self.clock, id);
        Some(fetched.clone())
    }

    /// Keeps `fetched`, part `part` of the object at `key`, dropping those used least recently
    /// past [`RECENT`] bytes.
    fn put(&mut self, key: &str, part: u64, fetched: Arc<Fetched>) {
        let id = (key.to_owned(), part);
        self.clock += 1;
        self.bytes += fetched.size();
        if let Some((used, replaced)) = self.parts.insert(id.clone(), (self.clock, fetched)) {
            self.by_use.remove(&used);
            self.bytes -= replaced.size();
        }
        self.by_use.insert(self.clock, id);
        while self.bytes > RECENT
            && let Some((_, oldest)) = self.by_use.pop_first()
        {
            let (_, dropped) = self.parts.remove(&oldest).expect("a part kept");
            self.bytes -= dropped.size();
        }
    }
}

/// What a read of a range of an object's data found in it.
#[derive(Debug, PartialEq, Eq)]
enum Found {
    Bytes(Vec<u8>),
    Gone,
    /// The part of the object of this number was changed ([`Part::Changed`]).
    Changed(u64),
}

/// The GETs of parts that reads wait for, each until its last reader leaves it.
#[derive(Default)]
struct Parts {
    flights: HashMap<(String, u64), Waited>,
}

struct Waited {
    /// Its place in the budget.
    ticket: u64,
    readers: usize,
    flight: Flight<Part>,
}

/// Reads extents from a bucket by parts, under a budget.
pub struct Fetcher<B: Bucket> {
    shared: Arc<Fetching<B>>,
    parts: Arc<Mutex<Parts>>,
    /// Where reads of each object stopped, and how far ahead of them parts are read.
    streams: Arc<Mutex<HashMap<String, Stream>>>,
    /// Runs read-ahead, which no read waits for.
    spawn: Option<Spawn>,
}

/// Runs a future to its end, no one waiting for it.
pub type Spawn = Arc<dyn Fn(BoxFuture<'static, ()>) + Send + Sync>;

impl<B: Bucket> Clone for Fetcher<B> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
            parts: self.parts.clone(),
            streams: self.streams.clone(),
            spawn: self.spawn.clone(),
        }
    }
}

/// Reads of an object, as read-ahead follows them.
struct Stream {
    /// The part after the last one read.
    next: u64,
    /// How many parts are read ahead of the next.
    window: u64,
    /// The part after the last one read ahead.
    ahead: u64,
}

/// The most parts read ahead of a read.
const AHEAD: u64 = 16;

/// How many objects' reads read-ahead follows.
const STREAMS: usize = 1024;

/// What a part's GET needs.
struct Fetching<B: Bucket> {
    bucket: Arc<B>,
    budget: Arc<Budget>,
    /// Objects' tails, or the reads of them, and, if it keeps the parts it fetches, those it
    /// fetched last, as fetched.
    memory: Arc<Memory>,
    /// Where it keeps the parts it fetches, as stored, and their objects' tails, if it does.
    cache: Option<Arc<Cache>>,
}

impl<B: Bucket> Fetcher<B> {
    /// Reads `bucket` under `budget`, keeping objects' tails in `memory`; and, with a `cache`, the
    /// parts it fetches there, those it fetched last in `memory` too.
    pub fn new(
        bucket: Arc<B>,
        budget: Arc<Budget>,
        memory: Arc<Memory>,
        cache: Option<Arc<Cache>>,
    ) -> Self {
        let shared = Arc::new(Fetching { bucket, budget, memory, cache });
        Self { shared, parts: Arc::default(), streams: Arc::default(), spawn: None }
    }

    /// Reads ahead of reads that wait for their data, by `spawn`, into the cache: with none, it
    /// reads nothing ahead.
    pub fn reading_ahead(self, spawn: Spawn) -> Self {
        Self { spawn: Some(spawn), ..self }
    }

    /// `len` bytes at `offset` in `extent`, at `location` or where the collector moved it from
    /// there: as many as it holds. The object at a key holds the extent where its footer says, if
    /// it names it; one that names it not, or is gone, the reader follows to its moves. One whose
    /// parts fail the CRCs its tail, kept, gives is read again, once, by its tail fetched anew.
    pub async fn read(
        &self,
        extent: ExtentId,
        location: &Location,
        offset: u64,
        len: u64,
        priority: Priority,
    ) -> Result<Vec<u8>, Error> {
        let mut key = location.key.clone();
        let mut again = true;
        for _ in 0..MOVES {
            if let Some(tail) = self.shared.tail(&key).await?
                && let Some(&(at, held)) = tail.extents.get(&extent)
            {
                let range = at + offset.min(held)..at + offset.saturating_add(len).min(held);
                if priority == Priority::Demand && !range.is_empty() {
                    self.read_ahead(&key, range.start / PART, (range.end - 1) / PART);
                }
                match self.data(&key, &tail, range, priority).await? {
                    Found::Bytes(bytes) => return Ok(bytes),
                    Found::Gone => {}
                    Found::Changed(part) => {
                        self.changed(&key, &tail, part, &mut again)?;
                        continue;
                    }
                }
            }
            let moved = self.moved(extent, &key).await?;
            key = moved.ok_or_else(|| Error::Lost(format!("{extent:?} in {key}")))?;
        }
        Err(Error::Corrupt(format!("{extent:?} moved too often")))
    }

    /// Where `extent` is as of the checkpoint at `key`, if it holds it, looked up a block at a time
    /// (doc-7, Checkpoints), rather than read whole: the checkpoint's tail, kept as other objects'
    /// are, holds its table, which says where the block that would hold the extent's entry is; the
    /// block is read by the parts it is in, as extents' data is, within the budget, and kept in the
    /// cache. One whose parts fail the CRCs its tail, kept, gives is read again, once, by its tail
    /// fetched anew. A checkpoint gone is lost.
    pub async fn lookup(&self, key: &str, extent: ExtentId) -> Result<Option<Location>, Error> {
        let mut again = true;
        loop {
            let tail = self.shared.tail(key).await?.ok_or_else(|| Error::Lost(key.to_owned()))?;
            let Some(table) = &tail.table else {
                return Err(Error::Corrupt(format!("{key} holds no checkpoint")));
            };
            let Some(block) = table.block(extent) else { return Ok(None) };
            let range = block.offset..block.offset + u64::from(block.len);
            match self.data(key, &tail, range, Priority::Demand).await? {
                Found::Bytes(bytes) => return table.find(&bytes, block, extent),
                Found::Gone => return Err(Error::Lost(key.to_owned())),
                Found::Changed(part) => self.changed(key, &tail, part, &mut again)?,
            }
        }
    }

    /// The bytes `range` of the data of the object at `key`, whose tail is `tail`: read by the
    /// parts that hold them.
    async fn data(
        &self,
        key: &str,
        tail: &Arc<Tail>,
        range: Range<u64>,
        priority: Priority,
    ) -> Result<Found, Error> {
        if range.start > range.end || range.end > tail.parts.frame.data_len {
            return Err(Error::Corrupt(format!("{range:?} past the end of {key}")));
        }
        let numbers = tail.parts.frame.parts_of(range.clone());
        let parts = numbers.clone().map(|part| self.part(key, part, tail, priority));
        let parts = futures::future::try_join_all(parts).await?;
        let Range { start, end } = range;
        let mut bytes = Vec::with_capacity((end - start) as usize);
        for (number, part) in numbers.zip(parts) {
            let part = match part {
                Part::Data(fetched) => fetched.unpacked(key, number)?,
                Part::Gone => return Ok(Found::Gone),
                Part::Changed => return Ok(Found::Changed(number)),
            };
            let at = start + bytes.len() as u64;
            let from = ((at % PART) as usize).min(part.len());
            let to = (from + (end - at) as usize).min(part.len());
            bytes.extend_from_slice(&part[from..to]);
        }
        Ok(Found::Bytes(bytes))
    }

    /// Forgets `tail`, kept as the tail of the object at `key`, whose part `part` was changed, so
    /// that the object is read once more by its tail fetched anew, unless it was `again`: then it
    /// is corrupt.
    fn changed(
        &self,
        key: &str,
        tail: &Arc<Tail>,
        part: u64,
        again: &mut bool,
    ) -> Result<(), Error> {
        if !std::mem::take(again) {
            return Err(Error::Corrupt(format!("part {part} of {key}, by its CRC, twice")));
        }
        self.shared.forget(key, tail);
        Ok(())
    }

    /// The key of the moves of the object at `key`, if the collector moved `extent` there when it
    /// compacted the object: their footer names it.
    async fn moved(&self, extent: ExtentId, key: &str) -> Result<Option<String>, Error> {
        let moves = layout::moves(key);
        let tail = self.shared.tail(&moves).await?;
        Ok(tail.is_some_and(|tail| tail.extents.contains_key(&extent)).then_some(moves))
    }

    /// Reads ahead of a read of parts `first..=last` of the object at `key`: the part after them,
    /// and twice as many as the time before, up to [`AHEAD`], while reads go on where the last
    /// stopped, so that a sequential reader finds its parts kept, and a random one their neighbours.
    fn read_ahead(&self, key: &str, first: u64, last: u64) {
        let Some(spawn) = self.spawn.as_ref().filter(|_| self.shared.cache.is_some()) else {
            return;
        };
        let parts = {
            let mut streams = self.streams.lock().unwrap();
            if streams.len() >= STREAMS && !streams.contains_key(key) {
                streams.clear();
            }
            let stream = (streams.entry(key.to_owned())).or_insert(Stream {
                next: u64::MAX,
                window: 0,
                ahead: 0,
            });
            let sequential = first <= stream.next && stream.next <= last + 1;
            if sequential {
                stream.window = (stream.window * 2).clamp(1, AHEAD);
            } else {
                stream.window = 1;
                stream.ahead = 0;
            }
            let parts = stream.ahead.max(last + 1)..last + 1 + stream.window;
            stream.next = last + 1;
            stream.ahead = stream.ahead.max(parts.end);
            parts
        };
        if parts.is_empty() {
            return;
        }
        let (this, key) = (self.clone(), key.to_owned());
        spawn(Box::pin(async move {
            // None past the object's end.
            let Ok(Some(tail)) = this.shared.tail(&key).await else { return };
            let parts = parts.start..parts.end.min(tail.parts.frame.parts());
            let reads = parts.map(|part| this.part(&key, part, &tail, Priority::Ahead));
            for read in futures::future::join_all(reads).await {
                if let Err(err) = read {
                    log::debug!("fxdax: reading {key} ahead: {err}");
                }
            }
        }));
    }

    /// Part `part` of the object at `key`, whose tail is `tail`: the GET for it in flight, or a new
    /// one. The GET goes, and gives up its place in the budget, once no read waits for it.
    async fn part(
        &self,
        key: &str,
        part: u64,
        tail: &Arc<Tail>,
        priority: Priority,
    ) -> Result<Part, Error> {
        let id = (key.to_owned(), part);
        let flight = {
            let mut parts = self.parts.lock().unwrap();
            let parts = &mut *parts;
            let waited = parts.flights.entry(id.clone()).or_insert_with(|| {
                let (ticket, granted) = self.shared.budget.enter(priority, PART);
                let (shared, tail) = (self.shared.clone(), tail.clone());
                let flight = async move { shared.part(&id.0, id.1, &tail, granted).await };
                Waited { ticket, readers: 0, flight: flight.boxed().shared() }
            });
            if priority == Priority::Demand {
                self.shared.budget.promote(waited.ticket);
            }
            waited.readers += 1;
            waited.flight.clone()
        };
        let _reading = Reading { parts: &self.parts, id: (key.to_owned(), part) };
        flight.await
    }
}

/// Runs `io`, which blocks on files, as a task of a multi-threaded runtime may: in
/// `block_in_place`, which hands the runtime's other tasks to another of its threads meanwhile, as
/// those of the runtime a host's stores share need, which their fetches and uploads run on
/// (TASK-92.21). On an executor of another kind, or none, it runs as it is.
fn blocking<T>(io: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current().map(|runtime| runtime.runtime_flavor()) {
        Ok(tokio::runtime::RuntimeFlavor::MultiThread) => tokio::task::block_in_place(io),
        _ => io(),
    }
}

/// A read waiting for a part's flight, or done with it: one of the readers its flight counts, which
/// goes when they all have.
struct Reading<'a> {
    parts: &'a Mutex<Parts>,
    id: (String, u64),
}

impl Drop for Reading<'_> {
    fn drop(&mut self) {
        let mut parts = self.parts.lock().unwrap();
        let waited = parts.flights.get_mut(&self.id).expect("a flight its readers keep");
        waited.readers -= 1;
        if waited.readers == 0 {
            parts.flights.remove(&self.id);
        }
    }
}

impl<B: Bucket> Fetching<B> {
    /// Part `part` of the object at `key`, whose tail is `tail`, checked against the CRC the tail
    /// gives: kept, in memory or in the cache, or fetched once `granted` lets its GET go, and kept
    /// then; unpacked only once a read wants it ([`Fetched`]).
    async fn part(
        self: &Arc<Self>,
        key: &str,
        part: u64,
        tail: &Tail,
        granted: impl Future<Output = Grant>,
    ) -> Result<Part, Error> {
        let (stored, len) = tail.stored(key, part)?;
        if let Some(fetched) = self.recent().and_then(|mut recent| recent.get(key, part, &stored)) {
            return Ok(Part::Data(fetched));
        }
        let bytes = match self.kept(key, part, &stored) {
            Some(bytes) => bytes,
            None => {
                let grant = granted.await;
                let range = ByteRange::Bounded(stored.range());
                let fetched = bucket::get_range(&*self.bucket, key, range).await;
                drop(grant);
                let fetched = match fetched {
                    Ok(Some(fetched)) => fetched,
                    Ok(None) => return Ok(Part::Gone),
                    Err(bucket::Past { .. }) => return Ok(Part::Changed),
                };
                if stored.check(&fetched).is_err() {
                    return Ok(Part::Changed);
                }
                self.keep(key, tail, part, &fetched);
                fetched
            }
        };
        let fetched = Arc::new(Fetched::new(stored, len, bytes));
        if let Some(mut recent) = self.recent() {
            recent.put(key, part, fetched.clone());
        }
        Ok(Part::Data(fetched))
    }

    /// The parts fetched last, in the host's memory, if it keeps the parts it fetches.
    fn recent(&self) -> Option<MutexGuard<'_, Recent>> {
        self.cache.as_ref()?;
        Some(self.memory.recent.lock().unwrap())
    }

    /// What `io` does with the files of its cache, if it keeps one: as a task that blocks may,
    /// holding up none of the runtime's others ([`blocking`]).
    fn cached<T>(&self, io: impl FnOnce(&Cache) -> T) -> Option<T> {
        let cache = self.cache.as_deref()?;
        Some(blocking(|| io(cache)))
    }

    /// The stored bytes of part `part` of the object at `key`, stored as `stored` says, if the
    /// cache has them and they pass its check: those that fail are dropped from the cache.
    fn kept(&self, key: &str, part: u64, stored: &extent_frame::Stored) -> Option<Vec<u8>> {
        let bytes = match self.cached(|cache| cache.get(key, part))? {
            Ok(bytes) => bytes?,
            Err(err) => {
                log::warn!("fxdax: reading part {part} of {key} from the cache: {err}");
                return None;
            }
        };
        if stored.check(&bytes).is_ok() {
            return Some(bytes);
        }
        log::warn!("fxdax: part {part} of {key} in the cache failed its CRC");
        if let Some(Err(err)) = self.cached(|cache| cache.forget(key, part)) {
            log::warn!("fxdax: dropping part {part} of {key} from the cache: {err}");
        }
        None
    }

    /// Keeps `bytes`, part `part`'s stored bytes, checked, in the cache.
    fn keep(&self, key: &str, tail: &Tail, part: u64, bytes: &[u8]) {
        let size = tail.parts.frame.size();
        let offset = tail.parts.stored[part as usize].offset;
        if let Some(Err(err)) = self.cached(|cache| cache.put(key, size, part, offset, bytes)) {
            log::warn!("fxdax: keeping part {part} of {key} in the cache: {err}");
        }
    }

    /// The tail of the object at `key`: kept, or in flight, or fetched now, under a place in the
    /// budget.
    fn tail(self: &Arc<Self>, key: &str) -> Flight<Option<Arc<Tail>>> {
        let mut tails = self.memory.tails.lock().unwrap();
        if let Some(tail) = tails.get(key) {
            return tail;
        }
        let (shared, key_owned, read) = (self.clone(), key.to_owned(), tails.next_read());
        let tail = async move {
            let fetched = shared.read_tail(&key_owned).await;
            let mut tails = shared.memory.tails.lock().unwrap();
            match &fetched {
                Ok(Some((_, size))) => tails.settle(&key_owned, read, *size),
                // What failed, or found the object gone, is not kept.
                _ => tails.remove(&key_owned, Some(read)),
            }
            fetched.map(|fetched| fetched.map(|(tail, _)| tail))
        };
        let tail = tail.boxed().shared();
        tails.insert(key, read, tail.clone());
        tail
    }

    /// The tail of the object at `key`, if there is one, and the bytes it takes in memory: the
    /// cache's copy of it, or one read from the bucket, under a place in the budget, and kept in
    /// the cache then.
    async fn read_tail(&self, key: &str) -> Result<Option<(Arc<Tail>, u64)>, Error> {
        let tail = match self.kept_tail(key) {
            Some(tail) => tail,
            None => {
                let grant = self.budget.enter(Priority::Demand, TAIL).1.await;
                let read = object::tail(&*self.bucket, key).await;
                drop(grant);
                let Some(tail) = read? else { return Ok(None) };
                self.keep_tail(key, &tail);
                tail
            }
        };
        let tail = Tail::of(key, tail)?;
        let size = tail.size();
        Ok(Some((Arc::new(tail), size)))
    }

    /// The tail of the object at `key`, if the cache has it and it passes its checks: one that
    /// fails is dropped from the cache.
    fn kept_tail(&self, key: &str) -> Option<object::Tail> {
        let bytes = match self.cached(|cache| cache.get(key, Cache::TAIL))? {
            Ok(bytes) => bytes?,
            Err(err) => {
                log::warn!("fxdax: reading the tail of {key} from the cache: {err}");
                return None;
            }
        };
        match object::Tail::of(key, bytes) {
            Ok(tail) => Some(tail),
            Err(err) => {
                log::warn!("fxdax: the tail of {key} in the cache: {err}");
                if let Some(Err(err)) = self.cached(|cache| cache.forget(key, Cache::TAIL)) {
                    log::warn!("fxdax: dropping the tail of {key} from the cache: {err}");
                }
                None
            }
        }
    }

    /// Keeps `tail`, the tail of the object at `key`, as read, in the cache.
    fn keep_tail(&self, key: &str, tail: &object::Tail) {
        let size = tail.parts.frame.size();
        let offset = size - tail.bytes.len() as u64;
        let kept = self.cached(|cache| cache.put(key, size, Cache::TAIL, offset, &tail.bytes));
        if let Some(Err(err)) = kept {
            log::warn!("fxdax: keeping the tail of {key} in the cache: {err}");
        }
    }

    /// Forgets `tail`, kept as the tail of the object at `key`, if it still is: the object is read
    /// by its tail fetched anew. The cache's copy goes first, so that no read after finds it there.
    fn forget(&self, key: &str, tail: &Arc<Tail>) {
        if let Some(Err(err)) = self.cached(|cache| cache.forget(key, Cache::TAIL)) {
            log::warn!("fxdax: dropping the tail of {key} from the cache: {err}");
        }
        let mut tails = self.memory.tails.lock().unwrap();
        let kept = tails.kept.get(key).map(|kept| (kept.read, kept.flight.peek()));
        if let Some((read, Some(Ok(Some(kept))))) = kept
            && Arc::ptr_eq(kept, tail)
        {
            tails.remove(key, Some(read));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bucket::PutMode;
    use crate::layout::{self, Frame, Moves};
    use crate::object::TAIL;
    use crate::sim::bucket::{Direct, Fault, SimBucket};
    use futures::executor::{LocalPool, block_on};
    use futures::task::LocalSpawnExt as _;
    use std::cell::RefCell;
    use std::rc::Rc;

    const KEY: &str = "segments/0000000000000001/0000000000000000";

    /// Bytes that compress.
    fn pattern(len: u64) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8 ^ (i / PART) as u8).collect()
    }

    /// Bytes that don't, from a xorshift generator: their objects' parts are stored raw, each at
    /// its offset in the data.
    fn noise(len: u64) -> Vec<u8> {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect()
    }

    /// A segment's object of `data`, whose footer names `extents` in it, each at an offset, of a
    /// length.
    fn segment(data: &[u8], extents: Vec<(ExtentId, u64, u64)>) -> Vec<u8> {
        let contents = layout::Contents { extents, ..layout::Contents::default() };
        let footer = layout::Chain::new(1, 1, 0, None, 0).close(contents);
        layout::Segment { data: data.to_vec(), footer }.encode()
    }

    /// A segment's object of `data`, all of which is [`EXTENT`].
    fn whole(data: &[u8]) -> Vec<u8> {
        segment(data, vec![(EXTENT, 0, data.len() as u64)])
    }

    /// A bucket holding `bytes` at [`KEY`], its requests completed by the test.
    fn holding(bytes: Vec<u8>) -> Arc<SimBucket> {
        let bucket = SimBucket::new();
        block_on(Direct(bucket.clone()).put(KEY, bytes, PutMode::Create)).unwrap();
        bucket
    }

    /// A bucket holding a segment of `data` at [`KEY`], all of which is [`EXTENT`].
    fn bucket(data: &[u8]) -> Arc<SimBucket> {
        holding(whole(data))
    }

    /// How long the tail of the segment of `data` at [`KEY`] is: what a cache keeps of it but its
    /// parts.
    fn tail_len(data: &[u8]) -> u64 {
        let tail = Frame::of(&whole(data)).unwrap().tail();
        tail.end - tail.start
    }

    fn location(len: u64) -> Location {
        Location { key: KEY.into(), offset: 0, len }
    }

    const EXTENT: ExtentId = ExtentId::new(1, 1);

    type Read = Result<Vec<u8>, Error>;

    /// Reads, each spawned, whose results land in the returned cells.
    struct Reads {
        pool: LocalPool,
        results: Vec<Rc<RefCell<Option<Read>>>>,
        aborts: Vec<futures::future::AbortHandle>,
        /// What fetchers spawned, to be run.
        spawned: Arc<Mutex<Vec<BoxFuture<'static, ()>>>>,
    }

    impl Reads {
        fn new() -> Self {
            let spawned = Arc::default();
            Self { pool: LocalPool::new(), results: Vec::new(), aborts: Vec::new(), spawned }
        }

        /// A spawner for fetchers' read-ahead, which runs with the reads.
        fn spawn(&self) -> Spawn {
            let spawned = self.spawned.clone();
            Arc::new(move |future| spawned.lock().unwrap().push(future))
        }

        /// Runs the reads, and what fetchers spawned, until they all wait.
        fn run(&mut self) {
            loop {
                self.pool.run_until_stalled();
                let spawned = std::mem::take(&mut *self.spawned.lock().unwrap());
                if spawned.is_empty() {
                    return;
                }
                for future in spawned {
                    self.pool.spawner().spawn_local(future).unwrap();
                }
            }
        }

        fn read(
            &mut self,
            fetcher: &Rc<Fetcher<SimBucket>>,
            location: Location,
            offset: u64,
            len: u64,
            priority: Priority,
        ) -> usize {
            let result = Rc::new(RefCell::new(None));
            let (fetcher, into) = (fetcher.clone(), result.clone());
            let read = async move {
                let read = fetcher.read(EXTENT, &location, offset, len, priority).await;
                *into.borrow_mut() = Some(read);
            };
            let (read, abort) = futures::future::abortable(read);
            self.pool.spawner().spawn_local(read.map(drop)).unwrap();
            self.results.push(result);
            self.aborts.push(abort);
            self.run();
            self.results.len() - 1
        }

        /// Abandons read `read`.
        fn abandon(&mut self, read: usize) {
            self.aborts[read].abort();
            self.run();
        }

        fn result(&self, read: usize) -> Option<Read> {
            self.results[read].borrow().clone()
        }
    }

    /// Completes the waiting requests, each as it comes, until none waits.
    fn complete_all(bucket: &SimBucket, reads: &mut Reads) -> Vec<String> {
        let mut completed = Vec::new();
        while bucket.waiting() > 0 {
            completed.push(bucket.describe(0));
            bucket.complete(0, Fault::None);
            reads.run();
        }
        completed
    }

    fn limits(requests: usize) -> Arc<Budget> {
        Budget::new(Limits { requests, bytes: u64::MAX })
    }

    /// Enters a GET of `bytes` on demand into `budget`: its grant, if it went at once, or the wait
    /// for it.
    fn enter(budget: &Arc<Budget>, bytes: u64) -> Result<Grant, BoxFuture<'static, Grant>> {
        let (_, granted) = budget.enter(Priority::Demand, bytes);
        let mut granted = granted.boxed();
        match (&mut granted).now_or_never() {
            Some(grant) => Ok(grant),
            None => Err(granted),
        }
    }

    /// The GETs and bytes `budget` counts in flight.
    fn in_flight(budget: &Budget) -> (usize, u64) {
        let queues = budget.queues.lock().unwrap();
        (queues.requests, queues.bytes)
    }

    /// GETs go while both their number and their bytes are within the limits.
    #[test]
    fn gets_go_within_both_limits() {
        let budget = Budget::new(Limits { requests: 2, bytes: 10 });
        let first = enter(&budget, 1).ok().unwrap();
        let _second = enter(&budget, 1).ok().unwrap();
        let third = enter(&budget, 1).err().unwrap();
        drop(first);
        assert!(third.now_or_never().is_some());

        let budget = Budget::new(Limits { requests: 4, bytes: 10 });
        let _six = enter(&budget, 6).ok().unwrap();
        let _four = enter(&budget, 4).ok().unwrap();
        assert!(enter(&budget, 1).is_err());
    }

    /// A GET larger than the bytes allowed goes alone.
    #[test]
    fn a_get_over_the_bytes_goes_alone() {
        let budget = Budget::new(Limits { requests: 4, bytes: 10 });
        let large = enter(&budget, 20).ok().unwrap();
        let after = enter(&budget, 1).err().unwrap();
        drop(large);
        assert!(after.now_or_never().is_some());
    }

    /// GETs done give back their places, and their bytes.
    #[test]
    fn gets_done_give_back_their_places() {
        let budget = Budget::new(Limits { requests: 4, bytes: 100 });
        let three = enter(&budget, 3).ok().unwrap();
        let two = enter(&budget, 2).ok().unwrap();
        drop(three);
        assert_eq!(in_flight(&budget), (1, 2));
        drop(two);
        assert_eq!(in_flight(&budget), (0, 0));
    }

    /// A GET whose read gave up waiting gives its place, once granted it, to the next.
    #[test]
    fn gets_given_up_give_back_their_places() {
        let budget = Budget::new(Limits { requests: 1, bytes: 100 });
        let three = enter(&budget, 3).ok().unwrap();
        drop(enter(&budget, 2).err().unwrap());
        drop(three);
        assert_eq!(in_flight(&budget), (0, 0));
    }

    /// Reads fetch the object's tail, once, then the parts that hold what they want, whole.
    #[test]
    fn reads_fetch_whole_parts() {
        let data = noise(2 * PART + 300);
        let bucket = bucket(&data);
        let fetcher = Rc::new(Fetcher::new(bucket.clone(), limits(8), Memory::new(), None));
        let mut reads = Reads::new();
        let location = location(data.len() as u64);
        let read = reads.read(&fetcher, location.clone(), PART - 10, 20, Priority::Demand);
        let requests = complete_all(&bucket, &mut reads);
        let parts = [format!("get {KEY} 0..{PART}"), format!("get {KEY} {PART}..{}", 2 * PART)];
        assert_eq!(
            requests,
            [format!("get {KEY} last {TAIL}"), parts[0].clone(), parts[1].clone()]
        );
        let want = data[(PART - 10) as usize..(PART + 10) as usize].to_vec();
        assert_eq!(reads.result(read), Some(Ok(want)));

        // The tail is kept; past the extent's end, a read reads short.
        let read = reads.read(&fetcher, location, 2 * PART + 200, 1000, Priority::Demand);
        let requests = complete_all(&bucket, &mut reads);
        assert_eq!(requests, [format!("get {KEY} {}..{}", 2 * PART, 2 * PART + 300)]);
        assert_eq!(reads.result(read), Some(Ok(data[(2 * PART + 200) as usize..].to_vec())));
        assert!(fetcher.parts.lock().unwrap().flights.is_empty());
    }

    /// Reads of one part while its GET is in flight share it.
    #[test]
    fn reads_of_a_part_share_its_get() {
        let data = noise(PART);
        let bucket = bucket(&data);
        let fetcher = Rc::new(Fetcher::new(bucket.clone(), limits(8), Memory::new(), None));
        let mut reads = Reads::new();
        let location = location(PART);
        let all: Vec<_> = (0..5)
            .map(|read| reads.read(&fetcher, location.clone(), read * 100, 100, Priority::Demand))
            .collect();
        assert_eq!(complete_all(&bucket, &mut reads).len(), 2);
        for read in all {
            let at = read * 100;
            assert_eq!(reads.result(read), Some(Ok(data[at..at + 100].to_vec())));
        }
    }

    /// No more GETs than the budget allows are in flight; reads waiting for their data go before
    /// read-ahead, even read-ahead of a part they then want.
    #[test]
    fn demand_goes_before_read_ahead_within_the_budget() {
        let data = noise(6 * PART);
        let bucket = bucket(&data);
        let fetcher = Rc::new(Fetcher::new(bucket.clone(), limits(1), Memory::new(), None));
        let mut reads = Reads::new();
        let location = location(6 * PART);
        let first = reads.read(&fetcher, location.clone(), 0, 1, Priority::Demand);
        // Its tail's GET goes, under its part's place, then its part's.
        assert_eq!(bucket.waiting(), 1);
        assert_eq!(complete_all(&bucket, &mut reads).len(), 2);
        assert_eq!(reads.result(first), Some(Ok(data[..1].to_vec())));

        let ahead: Vec<_> = (1..4)
            .map(|part| reads.read(&fetcher, location.clone(), part * PART, 1, Priority::Ahead))
            .collect();
        let demand = reads.read(&fetcher, location.clone(), 5 * PART, 1, Priority::Demand);
        let wanted = reads.read(&fetcher, location.clone(), 3 * PART, 1, Priority::Demand);
        assert_eq!(bucket.waiting(), 1);
        let order = complete_all(&bucket, &mut reads);
        let get = |part: u64| format!("get {KEY} {}..{}", part * PART, (part + 1) * PART);
        assert_eq!(order, [get(1), get(5), get(3), get(2)]);
        for (read, part) in ahead.into_iter().chain([demand, wanted]).zip([1, 2, 3, 5, 3]) {
            let at = (part * PART) as usize;
            assert_eq!(reads.result(read), Some(Ok(data[at..at + 1].to_vec())));
        }
    }

    /// A GET no read waits for any longer gives up its place in the budget, and is forgotten.
    #[test]
    fn gets_no_read_waits_for_go() {
        let data = noise(3 * PART);
        let bucket = bucket(&data);
        let fetcher = Rc::new(Fetcher::new(bucket.clone(), limits(1), Memory::new(), None));
        let mut reads = Reads::new();
        let location = location(3 * PART);
        reads.read(&fetcher, location.clone(), 0, 1, Priority::Demand);
        complete_all(&bucket, &mut reads);

        let first = reads.read(&fetcher, location.clone(), 0, 1, Priority::Demand);
        let waiting = reads.read(&fetcher, location.clone(), PART, 1, Priority::Ahead);
        let granted = reads.read(&fetcher, location.clone(), 2 * PART, 1, Priority::Ahead);
        reads.abandon(waiting);
        assert_eq!(fetcher.parts.lock().unwrap().flights.len(), 2);
        // The first's GET went, and its answer comes to none; its place goes to the next.
        reads.abandon(first);
        assert_eq!(fetcher.parts.lock().unwrap().flights.len(), 1);
        let order = complete_all(&bucket, &mut reads);
        let get = |part: u64| format!("get {KEY} {}..{}", part * PART, (part + 1) * PART);
        assert_eq!(order, [get(0), get(2)]);
        assert_eq!(reads.result(granted), Some(Ok(data[2 * PART as usize..][..1].to_vec())));
        assert!(fetcher.parts.lock().unwrap().flights.is_empty());
    }

    /// A part that fails its CRC is fetched again, once, by its object's tail fetched anew;
    /// failing again, the read fails.
    #[test]
    fn parts_failing_their_crcs_are_fetched_again_once() {
        let data = noise(PART + 100);
        let bucket = bucket(&data);
        let fetcher = Rc::new(Fetcher::new(bucket.clone(), limits(8), Memory::new(), None));
        let mut reads = Reads::new();
        let location = location(PART + 100);
        let good = whole(&data);
        let mut flipped = good.clone();
        flipped[PART as usize + 7] ^= 1;
        let put = |bytes: &[u8]| {
            block_on(Direct(bucket.clone()).put(KEY, bytes.to_vec(), PutMode::Overwrite)).unwrap();
        };
        let (tail, part) =
            (format!("get {KEY} last {TAIL}"), format!("get {KEY} {PART}..{}", PART + 100));

        // Corrupt when first read, whole when read again.
        put(&flipped);
        let read = reads.read(&fetcher, location.clone(), PART, 100, Priority::Demand);
        bucket.complete(bucket.position("last").unwrap(), Fault::None);
        reads.run();
        bucket.complete(0, Fault::None);
        reads.run();
        put(&good);
        assert_eq!(complete_all(&bucket, &mut reads), [tail.clone(), part.clone()]);
        assert_eq!(reads.result(read), Some(Ok(data[PART as usize..].to_vec())));

        // Corrupt both times.
        put(&flipped);
        let read = reads.read(&fetcher, location, PART, 100, Priority::Demand);
        assert_eq!(complete_all(&bucket, &mut reads), [part.clone(), tail, part]);
        let corrupt = format!("part 1 of {KEY}, by its CRC, twice");
        assert_eq!(reads.result(read), Some(Err(Error::Corrupt(corrupt))));
    }

    /// Parts fetched are kept, and their objects' tails, and read kept, with no GET.
    #[test]
    fn reads_keep_what_they_fetch() {
        let data = noise(2 * PART);
        let bucket = bucket(&data);
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path(), 8 * PART, 0).unwrap());
        let fetcher =
            Rc::new(Fetcher::new(bucket.clone(), limits(8), Memory::new(), Some(cache.clone())));
        let mut reads = Reads::new();
        let read = reads.read(&fetcher, location(2 * PART), PART + 5, 10, Priority::Demand);
        assert_eq!(complete_all(&bucket, &mut reads).len(), 2);
        let want = Some(Ok(data[(PART + 5) as usize..][..10].to_vec()));
        assert_eq!(reads.result(read), want);
        assert_eq!(cache.bytes(), PART + tail_len(&data));

        let read = reads.read(&fetcher, location(2 * PART), PART + 5, 10, Priority::Demand);
        assert_eq!(bucket.waiting(), 0);
        assert_eq!(reads.result(read), want);
    }

    /// A part kept that fails its CRC is dropped and fetched; fetched twice failing it, the second
    /// time by its object's tail fetched anew, the read fails. Read by a fetcher with a memory of
    /// its own each time, which holds none of it: the part, and its object's tail, are in the
    /// cache only.
    #[test]
    fn parts_kept_failing_their_crcs_are_fetched() {
        let data = noise(PART);
        let bucket = bucket(&data);
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path(), 8 * PART, 0).unwrap());
        let fetcher =
            || Rc::new(Fetcher::new(bucket.clone(), limits(8), Memory::new(), Some(cache.clone())));
        let mut reads = Reads::new();
        reads.read(&fetcher(), location(PART), 0, 10, Priority::Demand);
        complete_all(&bucket, &mut reads);
        let flip = |bytes: &mut Vec<u8>| bytes[3] ^= 1;
        let mut kept = std::fs::read(cache.path(KEY)).unwrap();
        flip(&mut kept);
        std::fs::write(cache.path(KEY), kept).unwrap();

        // The bucket's is whole.
        let read = reads.read(&fetcher(), location(PART), 0, 10, Priority::Demand);
        let part = format!("get {KEY} 0..{PART}");
        let tail = format!("get {KEY} last {TAIL}");
        assert_eq!(complete_all(&bucket, &mut reads), std::slice::from_ref(&part));
        assert_eq!(reads.result(read), Some(Ok(data[..10].to_vec())));
        assert_eq!(cache.get(KEY, 0).unwrap(), Some(data.clone()));

        // Neither is.
        let mut kept = std::fs::read(cache.path(KEY)).unwrap();
        flip(&mut kept);
        std::fs::write(cache.path(KEY), kept).unwrap();
        let mut object = whole(&data);
        flip(&mut object);
        block_on(Direct(bucket.clone()).put(KEY, object, PutMode::Overwrite)).unwrap();
        let read = reads.read(&fetcher(), location(PART), 0, 10, Priority::Demand);
        assert_eq!(complete_all(&bucket, &mut reads), [part.clone(), tail, part]);
        let corrupt = format!("part 0 of {KEY}, by its CRC, twice");
        assert_eq!(reads.result(read), Some(Err(Error::Corrupt(corrupt))));
        assert_eq!(cache.get(KEY, 0).unwrap(), None);
    }

    /// Reads read ahead the part after them, and twice as many each time they go on where the
    /// last stopped; a read elsewhere reads only the part after it ahead again.
    #[test]
    fn read_ahead_grows_along_sequential_reads() {
        let data = noise(12 * PART);
        let bucket = bucket(&data);
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path(), 64 * PART, 0).unwrap());
        let mut reads = Reads::new();
        let fetcher = Fetcher::new(bucket.clone(), limits(8), Memory::new(), Some(cache.clone()));
        let fetcher = Rc::new(fetcher.reading_ahead(reads.spawn()));
        let location = location(12 * PART);
        let get = |part: u64| format!("get {KEY} {}..{}", part * PART, (part + 1) * PART);
        let mut read_part = |part: u64| {
            let read = reads.read(&fetcher, location.clone(), part * PART, 10, Priority::Demand);
            let mut requests = complete_all(&bucket, &mut reads);
            requests.sort();
            let at = (part * PART) as usize;
            assert_eq!(reads.result(read), Some(Ok(data[at..at + 10].to_vec())));
            requests
        };
        assert_eq!(read_part(0), [get(0), get(1), format!("get {KEY} last {TAIL}")]);
        assert_eq!(read_part(1), [get(2), get(3)]);
        assert_eq!(read_part(2), [get(4), get(5), get(6)]);
        // Not past the object's end.
        assert_eq!(read_part(3), [get(10), get(11), get(7), get(8), get(9)]);
        assert_eq!(read_part(7), Vec::<String>::new());
        assert_eq!(cache.bytes(), 12 * PART + tail_len(&data));
    }

    /// A part stored compressed is fetched with one ranged GET of its stored bytes, checked and
    /// decompressed, and kept compressed.
    #[test]
    fn compressed_parts_are_fetched_and_kept_so() {
        let data = pattern(2 * PART);
        let bucket = bucket(&data);
        let parts = extent_frame::Parts::of(&whole(&data)).unwrap();
        let stored = parts.stored[1];
        assert_eq!(stored.encoding, extent_frame::Encoding::Zstd);
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path(), 8 * PART, 0).unwrap());
        let fetcher =
            Rc::new(Fetcher::new(bucket.clone(), limits(8), Memory::new(), Some(cache.clone())));
        let mut reads = Reads::new();
        let read = reads.read(&fetcher, location(2 * PART), PART + 5, 10, Priority::Demand);
        let range = stored.range();
        assert_eq!(
            complete_all(&bucket, &mut reads),
            [format!("get {KEY} last {TAIL}"), format!("get {KEY} {}..{}", range.start, range.end)]
        );
        let want = Some(Ok(data[(PART + 5) as usize..][..10].to_vec()));
        assert_eq!(reads.result(read), want);
        assert_eq!(cache.bytes(), stored.len + tail_len(&data));
        assert!(stored.len < PART / 100, "{} bytes stored", stored.len);

        let read = reads.read(&fetcher, location(2 * PART), PART + 5, 10, Priority::Demand);
        assert_eq!(bucket.waiting(), 0);
        assert_eq!(reads.result(read), want);
    }

    /// Where [`EXTENT`] is in the segment at [`KEY`] that the collector compacts in the tests of
    /// moves: at this offset, of this length.
    const MOVED: (u64, u64) = (1000, 1000);

    /// A bucket holding a segment of `data` at [`KEY`], whose footer names [`EXTENT`] at
    /// [`MOVED`] in it.
    fn holding_moved(data: &[u8]) -> Arc<SimBucket> {
        holding(segment(data, vec![(EXTENT, MOVED.0, MOVED.1)]))
    }

    /// The collector moves [`EXTENT`] from the segment of `data` at [`KEY`] in `bucket`, compacting
    /// it, then deletes it.
    fn compacted(bucket: &Arc<SimBucket>, data: &[u8]) {
        let direct = Direct(bucket.clone());
        let to = Location { key: layout::moves(KEY), offset: 10, len: MOVED.1 };
        let mut moved = vec![0; 10];
        moved.extend_from_slice(&data[MOVED.0 as usize..][..MOVED.1 as usize]);
        let moves = layout::with_footer(&moved, &Moves { extents: vec![(EXTENT, to)] });
        block_on(direct.put(&layout::moves(KEY), moves, PutMode::Create)).unwrap();
        block_on(direct.delete(KEY)).unwrap();
    }

    /// A read of an extent whose object the collector compacted reads it where it moved.
    #[test]
    fn reads_follow_moves() {
        let data = pattern(3000);
        let bucket = holding_moved(&data);
        compacted(&bucket, &data);
        let fetcher = Rc::new(Fetcher::new(bucket.clone(), limits(8), Memory::new(), None));
        assert_eq!(read_moved(&bucket, &fetcher), Some(Ok(data[1100..1150].to_vec())));
    }

    /// The collector compacts the segment of `data` at [`KEY`] in `bucket` ([`compacted`]); and a
    /// writer taken over, its create landing late, creates another segment at its key, as long,
    /// whose footer names another extent where [`EXTENT`] was.
    fn compacted_and_recreated(bucket: &Arc<SimBucket>, data: &[u8]) {
        compacted(bucket, data);
        let direct = Direct(bucket.clone());
        let other: Vec<u8> = data.iter().map(|byte| byte ^ 0x5a).collect();
        let stale = segment(&other, vec![(ExtentId::new(1, 9), MOVED.0, MOVED.1)]);
        block_on(direct.put(KEY, stale, PutMode::Create)).unwrap();
    }

    /// Reads 50 bytes at 100 in [`EXTENT`], at [`MOVED`] in the object at [`KEY`], with `fetcher`.
    fn read_moved(bucket: &SimBucket, fetcher: &Rc<Fetcher<SimBucket>>) -> Option<Read> {
        let mut reads = Reads::new();
        let at = Location { key: KEY.into(), offset: MOVED.0, len: MOVED.1 };
        let read = reads.read(fetcher, at, 100, 50, Priority::Demand);
        complete_all(bucket, &mut reads);
        reads.result(read)
    }

    /// TASK-79.18: a read of an extent at a key where a writer taken over recreated an object the
    /// collector deleted, whose footer names other extents, reads the extent where the collector
    /// moved it, not the bytes the object holds there.
    #[test]
    fn reads_follow_moves_past_objects_recreated_at_their_keys() {
        let data = noise(3000);
        let bucket = holding_moved(&data);
        compacted_and_recreated(&bucket, &data);
        let fetcher = Rc::new(Fetcher::new(bucket.clone(), limits(8), Memory::new(), None));
        assert_eq!(read_moved(&bucket, &fetcher), Some(Ok(data[1100..1150].to_vec())));
    }

    /// TASK-79.18: so does a read by a fetcher that kept the tail of the object deleted, whose
    /// parts the one recreated fails the CRCs of.
    #[test]
    fn reads_by_tails_kept_follow_moves_past_objects_recreated_at_their_keys() {
        let data = noise(3000);
        let bucket = holding_moved(&data);
        let fetcher = Rc::new(Fetcher::new(bucket.clone(), limits(8), Memory::new(), None));
        assert_eq!(read_moved(&bucket, &fetcher), Some(Ok(data[1100..1150].to_vec())));
        compacted_and_recreated(&bucket, &data);
        assert_eq!(read_moved(&bucket, &fetcher), Some(Ok(data[1100..1150].to_vec())));
    }

    /// TASK-92.22: so does a read by a fetcher whose memory holds none of the object, as once the
    /// host's let it go, which finds the tail of the object deleted in the cache, but not the part
    /// holding the extent: the part of the object recreated fails the tail's CRC, and the tail,
    /// forgotten in the cache too, is read anew, from the bucket.
    #[test]
    fn reads_by_tails_kept_in_the_cache_follow_moves_past_objects_recreated_at_their_keys() {
        let data = noise(3000);
        let bucket = holding_moved(&data);
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path(), 8 * PART, 0).unwrap());
        let fetcher =
            || Rc::new(Fetcher::new(bucket.clone(), limits(8), Memory::new(), Some(cache.clone())));
        assert_eq!(read_moved(&bucket, &fetcher()), Some(Ok(data[1100..1150].to_vec())));
        cache.forget(KEY, 0).unwrap();
        compacted_and_recreated(&bucket, &data);
        let fetcher = fetcher();
        let mut reads = Reads::new();
        let at = Location { key: KEY.into(), offset: MOVED.0, len: MOVED.1 };
        let read = reads.read(&fetcher, at, 100, 50, Priority::Demand);
        let moves = layout::moves(KEY);
        let moved =
            extent_frame::Parts::of(&block_on(Direct(bucket.clone()).get(&moves)).unwrap().bytes);
        assert_eq!(
            complete_all(&bucket, &mut reads),
            [
                format!("get {KEY} 0..3000"),
                format!("get {KEY} last {TAIL}"),
                format!("get {moves} last {TAIL}"),
                format!("get {moves} {:?}", moved.unwrap().stored[0].range()),
            ]
        );
        assert_eq!(reads.result(read), Some(Ok(data[1100..1150].to_vec())));
    }

    /// TASK-79.40: so does a read by a fetcher that kept the tail of the object deleted, the part
    /// holding the extent past the end of the one recreated, shorter: the read of the part, which
    /// object storage refuses, is not retried for ever, but shows the object changed.
    #[test]
    fn reads_by_tails_kept_past_the_end_of_objects_recreated_follow_moves() {
        let data = noise(PART + 3000);
        let at = (PART + 1000, 1000);
        let bucket = holding(segment(&data, vec![(EXTENT, at.0, at.1)]));
        let fetcher = Rc::new(Fetcher::new(bucket.clone(), limits(8), Memory::new(), None));
        let location = Location { key: KEY.into(), offset: at.0, len: at.1 };
        let read = || {
            let mut reads = Reads::new();
            let read = reads.read(&fetcher, location.clone(), 100, 50, Priority::Demand);
            let asked = complete_all(&bucket, &mut reads);
            (reads.result(read), asked)
        };
        let want = data[(at.0 + 100) as usize..][..50].to_vec();
        assert_eq!(read().0, Some(Ok(want.clone())));
        // Compacted, then a segment far shorter recreated at its key.
        let direct = Direct(bucket.clone());
        let to = Location { key: layout::moves(KEY), offset: 0, len: at.1 };
        let moved = data[at.0 as usize..][..at.1 as usize].to_vec();
        let moves = layout::with_footer(&moved, &Moves { extents: vec![(EXTENT, to)] });
        block_on(direct.put(&layout::moves(KEY), moves, PutMode::Create)).unwrap();
        block_on(direct.delete(KEY)).unwrap();
        let stale = segment(&noise(100), vec![(ExtentId::new(1, 9), 0, 100)]);
        block_on(direct.put(KEY, stale, PutMode::Create)).unwrap();

        let (read, asked) = read();
        assert_eq!(read, Some(Ok(want)));
        let moves = layout::moves(KEY);
        assert_eq!(
            asked,
            [
                format!("get {KEY} {}..{}", PART, PART + 3000),
                format!("head {KEY}"),
                format!("get {KEY} last {TAIL}"),
                format!("get {moves} last {TAIL}"),
                format!("get {moves} 0..{}", at.1),
            ]
        );
    }

    /// A read of an extent by a fetcher of its own ([`read`]) asks for its object's tail, then for
    /// the parts the extent is in; with the object gone, moved nowhere, the extent is lost.
    #[test]
    fn reads_on_their_own_fetch_the_tail_then_the_parts() {
        let data = noise(PART + 300);
        let location = Location { key: KEY.into(), offset: PART - 100, len: 300 };
        let bucket = holding(segment(&data, vec![(EXTENT, location.offset, location.len)]));
        let (read_back, asked) = bucket.served(read(&bucket, EXTENT, &location));
        assert_eq!(read_back, Ok(data[(PART - 100) as usize..(PART + 200) as usize].to_vec()));
        let parts = [format!("get {KEY} 0..{PART}"), format!("get {KEY} {PART}..{}", PART + 300)];
        assert_eq!(asked, [format!("get {KEY} last {TAIL}"), parts[0].clone(), parts[1].clone()]);

        block_on(Direct(bucket.clone()).delete(KEY)).unwrap();
        let (lost, asked) = bucket.served(read(&bucket, EXTENT, &location));
        assert_eq!(lost, Err(Error::Lost(format!("{EXTENT:?} in {KEY}"))));
        let moves = format!("get {} last {TAIL}", layout::moves(KEY));
        assert_eq!(asked, [format!("get {KEY} last {TAIL}"), moves]);
    }

    /// A range of an object's data is read from the parts it touches; an empty one reads none; one
    /// reversed, or past the data's end, is corrupt, and read not.
    #[test]
    fn data_is_read_by_the_parts_a_range_touches() {
        let data = pattern(2 * PART + 100);
        let bucket = bucket(&data);
        let fetcher = Fetcher::new(bucket.clone(), limits(8), Memory::new(), None);
        let tail = bucket.served(fetcher.shared.tail(KEY)).0.unwrap().unwrap();
        let read = |range| bucket.served(fetcher.data(KEY, &tail, range, Priority::Demand));

        let (read_back, asked) = read(PART - 5..PART + 5);
        let want = data[(PART - 5) as usize..(PART + 5) as usize].to_vec();
        assert_eq!((read_back, asked.len()), (Ok(Found::Bytes(want)), 2));
        assert_eq!(read(7..7), (Ok(Found::Bytes(Vec::new())), Vec::new()));
        let end = data.len() as u64;
        for range in [Range { start: 8, end: 7 }, end - 1..end + 1] {
            let corrupt = format!("{range:?} past the end of {KEY}");
            assert_eq!(read(range), (Err(Error::Corrupt(corrupt)), Vec::new()));
        }
    }

    /// TASK-90: an extent is looked up in a checkpoint by the block of its table that would hold
    /// its entry, read by its parts, which are kept, as the checkpoint's tail is; before the
    /// table's first entry, no block is read. A checkpoint gone is lost; an object that is none
    /// holds no table.
    #[test]
    fn lookups_read_blocks_of_checkpoints_by_their_parts() {
        let mut state = crate::recovery::State::new(1);
        let location = Location { key: KEY.into(), offset: 7, len: 9 };
        state.extents.insert(EXTENT, location.clone());
        let checkpoint = layout::checkpoint(1, 3);
        let bytes = crate::checkpoint::encode(&state);
        let bucket = SimBucket::new();
        block_on(Direct(bucket.clone()).put(&checkpoint, bytes.clone(), PutMode::Create)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path(), 8 * PART, 0).unwrap());
        let fetcher = Fetcher::new(bucket.clone(), limits(8), Memory::new(), Some(cache));
        let lookup = |key: &str, extent| bucket.served(fetcher.lookup(key, extent));

        let stored = extent_frame::Parts::of(&bytes).unwrap().stored[0].range();
        let read =
            [format!("get {checkpoint} last {TAIL}"), format!("get {checkpoint} {stored:?}")];
        assert_eq!(lookup(&checkpoint, EXTENT), (Ok(Some(location.clone())), read.to_vec()));
        assert_eq!(lookup(&checkpoint, EXTENT), (Ok(Some(location)), Vec::new()));
        assert_eq!(lookup(&checkpoint, ExtentId::new(1, 0)), (Ok(None), Vec::new()));

        block_on(Direct(bucket.clone()).delete(&checkpoint)).unwrap();
        let fetcher = Fetcher::new(bucket.clone(), limits(8), Memory::new(), None);
        let (gone, _) = bucket.served(fetcher.lookup(&checkpoint, EXTENT));
        assert_eq!(gone, Err(Error::Lost(checkpoint)));
        block_on(Direct(bucket.clone()).put(KEY, whole(&noise(100)), PutMode::Create)).unwrap();
        let (none, _) = bucket.served(fetcher.lookup(KEY, EXTENT));
        assert_eq!(none, Err(Error::Corrupt(format!("{KEY} holds no checkpoint"))));
    }

    /// A read that ends where a part does fetches no part after it.
    #[test]
    fn reads_ending_with_a_part_fetch_none_after_it() {
        let data = noise(2 * PART + 300);
        let bucket = bucket(&data);
        let fetcher = Rc::new(Fetcher::new(bucket.clone(), limits(8), Memory::new(), None));
        let mut reads = Reads::new();
        let read = reads.read(&fetcher, location(2 * PART + 300), PART - 10, 10, Priority::Demand);
        assert_eq!(
            complete_all(&bucket, &mut reads),
            [format!("get {KEY} last {TAIL}"), format!("get {KEY} 0..{PART}")]
        );
        assert_eq!(
            reads.result(read),
            Some(Ok(data[(PART - 10) as usize..PART as usize].to_vec()))
        );
    }

    /// TASK-79.18: a read of an extent in a segment naming a thousand extents, as one of 32 MiB of
    /// extents of 32 KiB does, asks for its object's tail once, then for its part: the footer,
    /// which says where the extent is, comes with the first read of the tail.
    #[test]
    fn reads_of_segments_naming_many_extents_read_their_tails_at_once() {
        let data = noise(PART);
        let others = (1..1024).map(|sequence| (ExtentId::new(2, sequence), sequence * 1024, 1024));
        let bucket =
            holding(segment(&data, [(EXTENT, 0, 1024)].into_iter().chain(others).collect()));
        let fetcher = Rc::new(Fetcher::new(bucket.clone(), limits(8), Memory::new(), None));
        let mut reads = Reads::new();
        let read = reads.read(&fetcher, location(1024), 0, 10, Priority::Demand);
        assert_eq!(
            complete_all(&bucket, &mut reads),
            [format!("get {KEY} last {TAIL}"), format!("get {KEY} 0..{PART}")]
        );
        assert_eq!(reads.result(read), Some(Ok(data[..10].to_vec())));
    }

    /// An object whose tail is longer than a tail's first read, its footer naming more extents than
    /// fit, is read with a second, of the whole of its footer and table.
    #[test]
    fn long_tails_are_read_whole() {
        let data = noise(PART);
        let named = TAIL / 32;
        let others = (1..=named).map(|sequence| (ExtentId::new(2, sequence), sequence, 1));
        let bytes = segment(&data, [(EXTENT, 0, PART)].into_iter().chain(others).collect());
        let frame = Frame::of(&bytes).unwrap();
        let tail = frame.tail();
        assert!(tail.end - tail.start > TAIL);
        let bucket = holding(bytes);
        let fetcher = Rc::new(Fetcher::new(bucket.clone(), limits(8), Memory::new(), None));
        let mut reads = Reads::new();
        let read = reads.read(&fetcher, location(PART), PART - 10, 10, Priority::Demand);
        assert_eq!(
            complete_all(&bucket, &mut reads),
            [
                format!("get {KEY} last {TAIL}"),
                format!("get {KEY} {tail:?}"),
                format!("get {KEY} 0..{PART}")
            ]
        );
        assert_eq!(reads.result(read), Some(Ok(data[(PART - 10) as usize..].to_vec())));
    }

    /// An object shorter than a tail's read whose trailer says another length is corrupt.
    #[test]
    fn short_objects_whose_trailers_lie_are_corrupt() {
        let data = noise(100);
        let bucket = bucket(&data);
        let mut longer = vec![0; 10];
        longer.extend_from_slice(&whole(&data));
        block_on(Direct(bucket.clone()).put(KEY, longer, PutMode::Overwrite)).unwrap();
        let fetcher = Rc::new(Fetcher::new(bucket.clone(), limits(8), Memory::new(), None));
        let mut reads = Reads::new();
        let read = reads.read(&fetcher, location(100), 0, 10, Priority::Demand);
        complete_all(&bucket, &mut reads);
        assert_eq!(reads.result(read), Some(Err(Error::Corrupt(format!("{KEY}, by its trailer")))));
    }

    /// Objects' tails are kept, each, while reads go from one object to another; past [`TAILS`]
    /// bytes of them, those used least recently are forgotten, one at a time, a read in flight
    /// counting [`TAIL`] bytes, and read anew.
    #[test]
    fn tails_are_kept_for_each_object() {
        let data = noise(100);
        let bucket = bucket(&data);
        let other = "segments/0000000000000001/0000000000000001";
        block_on(Direct(bucket.clone()).put(other, whole(&data), PutMode::Create)).unwrap();
        let fetcher = Rc::new(Fetcher::new(bucket.clone(), limits(8), Memory::new(), None));
        let mut reads = Reads::new();
        let mut read = |key: &str| {
            let location = Location { key: key.into(), offset: 0, len: 100 };
            let read = reads.read(&fetcher, location, 0, 10, Priority::Demand);
            let requests = complete_all(&bucket, &mut reads);
            assert_eq!(reads.result(read), Some(Ok(data[..10].to_vec())));
            requests
        };
        assert_eq!(read(KEY), [format!("get {KEY} last {TAIL}"), format!("get {KEY} 0..100")]);
        assert_eq!(
            read(other),
            [format!("get {other} last {TAIL}"), format!("get {other} 0..100")]
        );
        assert_eq!(read(other), [format!("get {other} 0..100")]);
        assert_eq!(read(KEY), [format!("get {KEY} 0..100")]);

        // Reads in flight of nearly as many bytes as are kept; this one's tail read again, used
        // last; then another read in flight: the other's tail, used least recently, goes, then
        // the first read in flight.
        let kept = |key: &str| fetcher.shared.memory.tails.lock().unwrap().kept.contains_key(key);
        let key = |number: u64| format!("segments/0000000000000002/{number:016x}");
        let reading = TAILS / TAIL;
        for number in 0..reading - 1 {
            drop(fetcher.shared.tail(&key(number)));
        }
        assert!(kept(KEY) && kept(other));
        assert_eq!(read(KEY), [format!("get {KEY} 0..100")]);
        drop(fetcher.shared.tail(&key(reading - 1)));
        let kept: Vec<_> = [other, KEY, &key(0), &key(1)].map(kept).into();
        assert_eq!(kept, [false, true, false, true]);
        assert_eq!(
            read(other),
            [format!("get {other} last {TAIL}"), format!("get {other} 0..100")]
        );
    }

    /// TASK-92.21: a demand read of a part read ahead takes it from what the read ahead fetched:
    /// neither from the bucket again nor from the cache's copy of it, removed here, so that
    /// reading it there would show as a GET.
    #[test]
    fn demand_reads_of_parts_read_ahead_take_what_was_fetched() {
        let data = pattern(4 * PART);
        let bucket = bucket(&data);
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path(), 64 * PART, 0).unwrap());
        let mut reads = Reads::new();
        let fetcher = Fetcher::new(bucket.clone(), limits(8), Memory::new(), Some(cache.clone()));
        let fetcher = Rc::new(fetcher.reading_ahead(reads.spawn()));
        let location = location(4 * PART);
        let parts = extent_frame::Parts::of(&whole(&data)).unwrap();
        let get = |part: usize| format!("get {KEY} {:?}", parts.stored[part].range());
        let mut read_at = |at: u64| {
            let read = reads.read(&fetcher, location.clone(), at, 10, Priority::Demand);
            let mut asked = complete_all(&bucket, &mut reads);
            asked.sort();
            assert_eq!(reads.result(read), Some(Ok(data[at as usize..][..10].to_vec())));
            asked
        };
        let mut first = vec![get(0), get(1), format!("get {KEY} last {TAIL}")];
        first.sort();
        assert_eq!(read_at(0), first);
        std::fs::remove_file(cache.path(KEY)).unwrap();
        let mut ahead = vec![get(2), get(3)];
        ahead.sort();
        assert_eq!(read_at(PART), ahead);
    }

    /// TASK-92.21: a benchmark of reads of an object's data from the bucket, one after another,
    /// through a fetcher reading ahead into a cache: of 64 KiB each, and of a part each, as a
    /// pager's page-ins are; how long they take, the data compressing as an image's might, at
    /// about a third. `cargo nextest run -p extent_remote --run-ignored only
    /// bench_sequential_reads --no-capture`.
    #[test]
    #[ignore]
    fn bench_sequential_reads() {
        const LEN: u64 = 64 * PART;
        // Each 1 KiB a run of a byte, then noise: about a third of it compresses away.
        let noise = noise(LEN);
        let data: Vec<u8> = (0..LEN as usize)
            .map(|i| if i % 1024 < 340 { (i / 1024) as u8 } else { noise[i] })
            .collect();
        for read_len in [64 << 10, PART] {
            let bucket = bucket(&data);
            let dir = tempfile::tempdir().unwrap();
            let cache = Arc::new(Cache::new(dir.path(), 2 * LEN, 0).unwrap());
            let mut reads = Reads::new();
            let fetcher =
                Fetcher::new(bucket.clone(), limits(64), Memory::new(), Some(cache.clone()));
            let fetcher = Rc::new(fetcher.reading_ahead(reads.spawn()));
            let location = location(LEN);
            let started = std::time::Instant::now();
            for at in (0..LEN).step_by(read_len as usize) {
                let read = reads.read(&fetcher, location.clone(), at, read_len, Priority::Demand);
                complete_all(&bucket, &mut reads);
                let read = reads.result(read).unwrap().unwrap();
                assert_eq!(read, data[at as usize..(at + read_len) as usize]);
            }
            let took = started.elapsed();
            println!("{} reads of {read_len} bytes, one after another: {took:?}", LEN / read_len);
        }
    }

    /// TASK-92.22: a fetcher reading parts another kept in the cache they share, which its memory
    /// holds not, as once the host's memory let them go, reads them, and the tails of their
    /// objects, from the cache: it asks the bucket for nothing.
    #[test]
    fn fetchers_sharing_a_cache_read_the_tails_kept_there() {
        let data = noise(2 * PART);
        let bucket = bucket(&data);
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path(), 8 * PART, 0).unwrap());
        let fetcher =
            || Rc::new(Fetcher::new(bucket.clone(), limits(8), Memory::new(), Some(cache.clone())));
        let mut reads = Reads::new();
        let want = Some(Ok(data[(PART + 5) as usize..][..10].to_vec()));
        let read = reads.read(&fetcher(), location(2 * PART), PART + 5, 10, Priority::Demand);
        let part = format!("get {KEY} {PART}..{}", 2 * PART);
        assert_eq!(complete_all(&bucket, &mut reads), [format!("get {KEY} last {TAIL}"), part]);
        assert_eq!(reads.result(read), want);

        let read = reads.read(&fetcher(), location(2 * PART), PART + 5, 10, Priority::Demand);
        assert_eq!(complete_all(&bucket, &mut reads), Vec::<String>::new());
        assert_eq!(reads.result(read), want);
    }

    /// TASK-92.22: reads of more objects, one after another, than a fetcher once kept the tails
    /// of, 4,096, then of them all again, find each object's tail, and part, kept: the second time
    /// they ask the bucket for nothing.
    #[test]
    fn reads_of_many_objects_find_their_tails_kept() {
        const OBJECTS: u64 = 4097;
        let data = noise(100);
        let bucket = SimBucket::new();
        let key = |object: u64| format!("segments/0000000000000001/{object:016x}");
        for object in 0..OBJECTS {
            block_on(Direct(bucket.clone()).put(&key(object), whole(&data), PutMode::Create))
                .unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path(), 64 * PART, 0).unwrap());
        let fetcher =
            Rc::new(Fetcher::new(bucket.clone(), limits(8), Memory::new(), Some(cache.clone())));
        let mut reads = Reads::new();
        let mut read_all = || -> usize {
            let mut asked = 0;
            for object in 0..OBJECTS {
                let location = Location { key: key(object), offset: 0, len: 100 };
                let read = reads.read(&fetcher, location, 0, 10, Priority::Demand);
                asked += complete_all(&bucket, &mut reads).len();
                assert_eq!(reads.result(read), Some(Ok(data[..10].to_vec())));
            }
            asked
        };
        assert_eq!(read_all(), 2 * OBJECTS as usize);
        assert_eq!(read_all(), 0);
    }

    /// The bytes of parts, and of tails, `fetchers` hold in memory, each memory they keep them in
    /// counted once.
    fn held(fetchers: &[Rc<Fetcher<SimBucket>>]) -> (u64, u64) {
        let mut counted = std::collections::HashSet::new();
        let memories = fetchers.iter().map(|fetcher| &fetcher.shared.memory);
        let memories = memories.filter(|memory| counted.insert(Arc::as_ptr(memory)));
        memories.fold((0, 0), |(parts, tails), memory| {
            let held = (memory.recent.lock().unwrap().bytes, memory.tails.lock().unwrap().bytes);
            (parts + held.0, tails + held.1)
        })
    }

    /// `count` fetchers of one host, as its stores' are, sharing its memory and `cache`.
    fn host(
        bucket: &Arc<SimBucket>,
        cache: &Arc<Cache>,
        count: usize,
    ) -> Vec<Rc<Fetcher<SimBucket>>> {
        let memory = Memory::new();
        let fetcher =
            || Fetcher::new(bucket.clone(), limits(8), memory.clone(), Some(cache.clone()));
        (0..count).map(|_| Rc::new(fetcher())).collect()
    }

    /// TASK-92.21: a host's fetchers, one for each of its stores, keep no more than [`RECENT`]
    /// bytes of parts in memory in all, however many of them there are: here four, each reading
    /// 24 MiB of an object of its own.
    #[test]
    fn fetchers_of_a_host_hold_no_more_parts_in_memory_than_its_bound() {
        const PARTS: u64 = 24;
        let data = noise(PARTS * PART);
        let bucket = SimBucket::new();
        let key = |object: u64| format!("segments/0000000000000001/{object:016x}");
        for object in 0..4 {
            block_on(Direct(bucket.clone()).put(&key(object), whole(&data), PutMode::Create))
                .unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path(), 8 * PART, 0).unwrap());
        let fetchers = host(&bucket, &cache, 4);
        let mut reads = Reads::new();
        for (object, fetcher) in (0..).zip(&fetchers) {
            let location = Location { key: key(object), offset: 0, len: PARTS * PART };
            for part in 0..PARTS {
                let read = reads.read(fetcher, location.clone(), part * PART, 10, Priority::Demand);
                complete_all(&bucket, &mut reads);
                let at = (part * PART) as usize;
                assert_eq!(reads.result(read), Some(Ok(data[at..at + 10].to_vec())));
            }
        }
        let (parts, _) = held(&fetchers);
        assert!(parts <= RECENT, "{} MiB of parts held, past {} MiB", parts >> 20, RECENT >> 20);
    }

    /// TASK-92.22: a host's fetchers keep no more than [`TAILS`] bytes of tails in memory in all,
    /// however many of them there are: here two, each reading [`TAILS`] bytes of them, as reads in
    /// flight, which count the bytes they read at first.
    #[test]
    fn fetchers_of_a_host_hold_no_more_tails_in_memory_than_its_bound() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path(), 8 * PART, 0).unwrap());
        let fetchers = host(&SimBucket::new(), &cache, 2);
        for (fetcher, branch) in fetchers.iter().zip(2..) {
            for number in 0..TAILS / TAIL {
                drop(fetcher.shared.tail(&format!("segments/{branch:016x}/{number:016x}")));
            }
        }
        let (_, tails) = held(&fetchers);
        assert!(tails <= TAILS, "{} MiB of tails held, past {} MiB", tails >> 20, TAILS >> 20);
    }

    /// TASK-92.21, TASK-92.22: a fetcher of another of the host's stores, as a fork's store's of
    /// its base's, reads what one read last from the host's memory: the parts, and their object's
    /// tail, the cache's copies of them removed here, so that reading them anew would show as GETs.
    #[test]
    fn fetchers_of_a_host_share_what_they_hold_in_memory() {
        let data = pattern(2 * PART);
        let bucket = bucket(&data);
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path(), 8 * PART, 0).unwrap());
        let fetchers = host(&bucket, &cache, 2);
        let (base, fork) = (&fetchers[0], &fetchers[1]);
        let mut reads = Reads::new();
        let want = Some(Ok(data[(PART - 5) as usize..][..10].to_vec()));
        let read = reads.read(base, location(2 * PART), PART - 5, 10, Priority::Demand);
        assert_eq!(complete_all(&bucket, &mut reads).len(), 3);
        assert_eq!(reads.result(read), want);
        std::fs::remove_file(cache.path(KEY)).unwrap();

        let read = reads.read(fork, location(2 * PART), PART - 5, 10, Priority::Demand);
        assert_eq!(complete_all(&bucket, &mut reads), Vec::<String>::new());
        assert_eq!(reads.result(read), want);
        // One copy of the parts, each as stored and unpacked.
        let stored = extent_frame::Parts::of(&whole(&data)).unwrap().stored;
        assert_eq!(held(&fetchers).0, stored.iter().map(|stored| stored.len + PART).sum::<u64>());
    }

    /// Reads within the part the last read ended in go on from it, and read further ahead; a read
    /// elsewhere, after it or before, reads only the part after it ahead.
    #[test]
    fn read_ahead_follows_reads_within_a_part_and_restarts_elsewhere() {
        let data = noise(12 * PART);
        let bucket = bucket(&data);
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path(), 64 * PART, 0).unwrap());
        let mut reads = Reads::new();
        let fetcher = Fetcher::new(bucket.clone(), limits(8), Memory::new(), Some(cache));
        let fetcher = Rc::new(fetcher.reading_ahead(reads.spawn()));
        let location = location(12 * PART);
        let get = |part: u64| format!("get {KEY} {}..{}", part * PART, (part + 1) * PART);
        let mut read_at = |at: u64| {
            let read = reads.read(&fetcher, location.clone(), at, 10, Priority::Demand);
            let mut requests = complete_all(&bucket, &mut reads);
            requests.sort();
            assert_eq!(reads.result(read), Some(Ok(data[at as usize..][..10].to_vec())));
            requests
        };
        assert_eq!(read_at(0), [get(0), get(1), format!("get {KEY} last {TAIL}")]);
        assert_eq!(read_at(100), [get(2)]);
        assert_eq!(read_at(8 * PART), [get(8), get(9)]);
        assert_eq!(read_at(5 * PART), [get(5), get(6)]);
    }

    /// TASK-92.21: a read of the cache, which blocks on its files, holds up no other task of the
    /// runtime the host's fetches run on, here one of a single thread: the file the cache keeps
    /// the read's part and its object's tail in is a pipe, which the read waits to open, as for a
    /// disk slow to answer, until the runtime's other task opens it to write. What the read finds
    /// in it then is no part, and it reads the bucket.
    #[test]
    fn reads_of_the_cache_hold_up_no_other_task_of_their_runtime() {
        use std::os::unix::fs::OpenOptionsExt as _;
        use std::sync::atomic::{AtomicBool, Ordering};
        let data = noise(PART);
        let bucket = bucket(&data);
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path(), 8 * PART, 0).unwrap());
        let mut reads = Reads::new();
        let keeping = Fetcher::new(bucket.clone(), limits(8), Memory::new(), Some(cache.clone()));
        reads.read(&Rc::new(keeping), location(PART), 0, 10, Priority::Demand);
        complete_all(&bucket, &mut reads);
        let pipe = cache.path(KEY);
        std::fs::remove_file(&pipe).unwrap();
        let path = std::ffi::CString::new(pipe.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a mkfifo of a C string.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        // Opening the pipe to write, which lets a read waiting to open it go on: none waiting,
        // it fails.
        let open = |pipe: &std::path::Path| {
            drop(std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(pipe))
        };

        let runtime =
            tokio::runtime::Builder::new_multi_thread().worker_threads(1).build().unwrap();
        let fetcher = Fetcher::new(Arc::new(Direct(bucket)), limits(8), Memory::new(), Some(cache));
        let done = Arc::new(AtomicBool::new(false));
        let (finished, read) = std::sync::mpsc::channel();
        let reading = done.clone();
        runtime.spawn(async move {
            let read = fetcher.read(EXTENT, &location(PART), 0, 10, Priority::Demand).await;
            reading.store(true, Ordering::Relaxed);
            finished.send(read).unwrap();
        });
        let (opening, at) = (done.clone(), pipe.clone());
        runtime.spawn(async move {
            while !opening.load(Ordering::Relaxed) {
                open(&at);
                tokio::task::yield_now().await;
            }
        });
        // Had the read held the runtime's one thread, the other task would not run, nor the read
        // end: once its wait is past, whatever came, the read is let go.
        let read = read.recv_timeout(std::time::Duration::from_secs(30));
        while !done.load(Ordering::Relaxed) {
            open(&pipe);
            std::thread::yield_now();
        }
        assert_eq!(read, Ok(Ok(data[..10].to_vec())));
    }

    /// TASK-92.21: a benchmark of reads of parts the cache keeps, on a runtime of four threads, as
    /// the remotes' runtime may be, each a task of its own, all at once: how long 1,024 reads of a
    /// part take, each by a fetcher whose memory holds nothing, so that it reads the part, and its
    /// object's tail, from the cache. `cargo nextest run -p extent_remote --run-ignored only
    /// bench_reads_of_the_cache_on_a_runtime --no-capture`.
    #[test]
    #[ignore]
    fn bench_reads_of_the_cache_on_a_runtime() {
        const PARTS: u64 = 64;
        let data = noise(PARTS * PART);
        let bucket = bucket(&data);
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path(), 2 * PARTS * PART, 0).unwrap());
        let keeping = Fetcher::new(bucket.clone(), limits(64), Memory::new(), Some(cache.clone()));
        let (keeping, mut reads) = (Rc::new(keeping), Reads::new());
        for part in 0..PARTS {
            reads.read(&keeping, location(PARTS * PART), part * PART, 1, Priority::Demand);
            complete_all(&bucket, &mut reads);
        }
        let runtime =
            tokio::runtime::Builder::new_multi_thread().worker_threads(4).build().unwrap();
        let bucket = Arc::new(Direct(bucket));
        let started = std::time::Instant::now();
        let reads: Vec<_> = (0..16 * PARTS)
            .map(|read| {
                let fetcher =
                    Fetcher::new(bucket.clone(), limits(64), Memory::new(), Some(cache.clone()));
                let at = read % PARTS * PART;
                let read = async move {
                    fetcher.read(EXTENT, &location(PARTS * PART), at, PART, Priority::Demand).await
                };
                (at, runtime.spawn(read))
            })
            .collect();
        for (at, read) in reads {
            let read = runtime.block_on(read).unwrap();
            assert!(read == Ok(data[at as usize..(at + PART) as usize].to_vec()), "at {at}");
        }
        println!("{} reads of a part kept: {:?}", 16 * PARTS, started.elapsed());
    }

    /// Read-ahead follows the reads of up to [`STREAMS`] objects; past them, it forgets them all.
    #[test]
    fn read_ahead_follows_so_many_objects() {
        let reads = Reads::new();
        let bucket = SimBucket::new();
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path(), PART, 0).unwrap());
        let fetcher = Fetcher::new(bucket, limits(8), Memory::new(), Some(cache))
            .reading_ahead(reads.spawn());
        let streams = || fetcher.streams.lock().unwrap().len();
        let key = |number: usize| format!("segments/0000000000000001/{number:016x}");
        for number in 0..STREAMS {
            fetcher.read_ahead(&key(number), 0, 0);
        }
        assert_eq!(streams(), STREAMS);
        fetcher.read_ahead(&key(0), 1, 1);
        assert_eq!(streams(), STREAMS);
        fetcher.read_ahead(&key(STREAMS), 0, 0);
        assert_eq!(streams(), 1);
    }
}
