//! A host's reads of extents from a bucket (doc-7, Reading): by whole [`PART`]s of objects, each
//! checked against its CRC; one GET for a part however many reads want it at once; GETs in flight
//! bounded in number and bytes, by a [`Budget`] a host's fetchers share, where reads waiting for
//! their data go before read-ahead.

use crate::Error;
use crate::bucket::{self, Bucket, ByteRange};
use crate::cache::Cache;
use crate::layout::{self, Frame, Location, PART};
use crate::reader::{MOVES, Moved, MovesMaps};
use futures::channel::oneshot;
use futures::future::{BoxFuture, FutureExt as _, Shared};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use storage_device::ExtentId;

/// Whether a read waits for its data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    Demand,
    Ahead,
}

/// How many GETs may be in flight, of how many bytes in all. One GET goes whatever its size.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub gets: usize,
    pub bytes: u64,
}

/// GETs in flight, and those waiting to go, demand first, each in the order it came.
pub struct Budget {
    limits: Limits,
    queues: Mutex<Queues>,
}

#[derive(Default)]
struct Queues {
    gets: usize,
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

/// A GET's place in its budget, given up when it is dropped.
struct Grant {
    budget: Option<Arc<Budget>>,
    bytes: u64,
}

impl Drop for Grant {
    fn drop(&mut self) {
        if let Some(budget) = self.budget.take() {
            let mut queues = budget.queues.lock().unwrap();
            queues.gets -= 1;
            queues.bytes -= self.bytes;
            budget.admit(&mut queues);
        }
    }
}

impl Budget {
    pub fn new(limits: Limits) -> Arc<Self> {
        Arc::new(Self { limits, queues: Mutex::default() })
    }

    /// A place for a GET of `bytes`: its ticket, by which [`Budget::promote`] finds it, and the wait
    /// for it.
    fn enter(
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

    /// Moves the GET of `ticket`, if it waits behind demand, up to it.
    fn promote(self: &Arc<Self>, ticket: u64) {
        let mut queues = self.queues.lock().unwrap();
        let ahead = &mut queues.waiting[Priority::Ahead as usize];
        if let Some(at) = ahead.iter().position(|waiter| waiter.ticket == ticket) {
            let waiter = ahead.remove(at).unwrap();
            queues.waiting[Priority::Demand as usize].push_back(waiter);
            self.admit(&mut queues);
        }
    }

    /// Lets the GETs waiting first go, as far as the limits allow.
    fn admit(self: &Arc<Self>, queues: &mut Queues) {
        while let Some(queue) = queues.waiting.iter_mut().find(|queue| !queue.is_empty()) {
            let bytes = queue[0].bytes;
            let fits = queues.gets < self.limits.gets && queues.bytes + bytes <= self.limits.bytes;
            if queues.gets > 0 && !fits {
                return;
            }
            let waiter = queue.pop_front().unwrap();
            queues.gets += 1;
            queues.bytes += bytes;
            let grant = Grant { budget: Some(self.clone()), bytes };
            // Its reader gave up: its place goes to the next.
            if let Err(mut grant) = waiter.go.send(grant) {
                grant.budget = None;
                queues.gets -= 1;
                queues.bytes -= bytes;
            }
        }
    }
}

/// An object's frame, and the CRCs of its parts.
#[derive(Debug)]
struct Tail {
    frame: Frame,
    crcs: Vec<u32>,
}

impl Tail {
    /// Part `part` of the object at `key`, of `bytes` read from its start, if it passes its check.
    fn check(&self, key: &str, part: u64, mut bytes: Vec<u8>) -> Result<Option<Vec<u8>>, Error> {
        let crc = *self
            .crcs
            .get(part as usize)
            .ok_or_else(|| Error::Corrupt(format!("part {part} of {key}, past its end")))?;
        let range = self.frame.part(part);
        let len = (range.end - range.start) as usize;
        if bytes.len() < len {
            return Ok(None);
        }
        bytes.truncate(len);
        Ok((layout::crc(&bytes) == crc).then_some(bytes))
    }
}

/// How many bytes of an object's end are read for its tail: the CRCs of 1020 parts, and its trailer.
const TAIL: u64 = 4096;

/// How many objects' tails are kept.
const TAILS: usize = 4096;

/// A GET, or what it found, which reads that come while it is in flight share.
type Flight<T> = Shared<BoxFuture<'static, Result<T, Error>>>;

/// A part's bytes, or none if the object is gone.
type Part = Option<Arc<[u8]>>;

/// The GETs of parts that reads wait for.
#[derive(Default)]
struct Parts {
    flights: HashMap<(String, u64), Waited>,
    /// Numbers flights, so that a reader leaving one leaves only that one.
    flown: u64,
}

struct Waited {
    number: u64,
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
    cache: Option<Arc<Cache>>,
    /// Objects' tails, or the GETs for them, by key; none if the object is gone.
    tails: Mutex<HashMap<String, Flight<Option<Arc<Tail>>>>>,
}

impl<B: Bucket> Fetcher<B> {
    /// Reads `bucket` under `budget`, keeping the parts it fetches in `cache`, if any.
    pub fn new(bucket: Arc<B>, budget: Arc<Budget>, cache: Option<Arc<Cache>>) -> Self {
        let shared = Arc::new(Fetching { bucket, budget, cache, tails: Mutex::default() });
        Self { shared, parts: Arc::default(), streams: Arc::default(), spawn: None }
    }

    /// Reads ahead of reads that wait for their data, by `spawn`, into the cache: with none, it
    /// reads nothing ahead.
    pub fn reading_ahead(self, spawn: Spawn) -> Self {
        Self { spawn: Some(spawn), ..self }
    }

    /// `len` bytes at `offset` in `extent`, at `location` or where the collector moved it from
    /// there: as many as it holds.
    pub async fn read(
        &self,
        extent: ExtentId,
        location: &Location,
        offset: u64,
        len: u64,
        priority: Priority,
    ) -> Result<Vec<u8>, Error> {
        let mut maps = MovesMaps::default();
        let mut location = location.clone();
        for _ in 0..MOVES {
            let start = location.offset + offset.min(location.len);
            let end = location.offset + offset.saturating_add(len).min(location.len);
            if start == end {
                return Ok(Vec::new());
            }
            let (first, last) = (start / PART, (end - 1) / PART);
            if priority == Priority::Demand {
                self.read_ahead(&location.key, first, last);
            }
            let parts = (first..=last).map(|part| self.part(&location.key, part, priority));
            let parts = futures::future::try_join_all(parts).await?;
            if let Some(parts) = parts.into_iter().collect::<Option<Vec<_>>>() {
                let mut bytes = Vec::with_capacity((end - start) as usize);
                for part in parts {
                    let at = start + bytes.len() as u64;
                    let from = ((at % PART) as usize).min(part.len());
                    let to = (from + (end - at) as usize).min(part.len());
                    bytes.extend_from_slice(&part[from..to]);
                }
                if bytes.len() as u64 != end - start {
                    return Err(Error::Corrupt(format!("{extent:?} past the end of its object")));
                }
                return Ok(bytes);
            }
            let bucket = &*self.shared.bucket;
            location = match maps.moved(bucket, extent, &location.key).await? {
                Moved::To(to) => to,
                Moved::No | Moved::Dropped => {
                    return Err(Error::Lost(format!("{extent:?} in {}", location.key)));
                }
            };
        }
        Err(Error::Corrupt(format!("{extent:?} moved too often")))
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
            let parts = parts.start..parts.end.min(tail.frame.parts());
            let reads = parts.map(|part| this.part(&key, part, Priority::Ahead));
            for read in futures::future::join_all(reads).await {
                if let Err(err) = read {
                    log::debug!("fxdax: reading {key} ahead: {err}");
                }
            }
        }));
    }

    /// Part `part` of the object at `key`: the GET for it in flight, or a new one. The GET goes,
    /// and gives up its place in the budget, once no read waits for it.
    async fn part(&self, key: &str, part: u64, priority: Priority) -> Result<Part, Error> {
        let id = (key.to_owned(), part);
        let (number, flight) = {
            let mut parts = self.parts.lock().unwrap();
            let parts = &mut *parts;
            let waited = parts.flights.entry(id.clone()).or_insert_with(|| {
                let (ticket, granted) = self.shared.budget.enter(priority, PART);
                let shared = self.shared.clone();
                let flight = async move { shared.part(&id.0, id.1, granted).await };
                parts.flown += 1;
                Waited { number: parts.flown, ticket, readers: 0, flight: flight.boxed().shared() }
            });
            if priority == Priority::Demand {
                self.shared.budget.promote(waited.ticket);
            }
            waited.readers += 1;
            (waited.number, waited.flight.clone())
        };
        let _reading = Reading { parts: &self.parts, id: (key.to_owned(), part), number };
        flight.await
    }
}

/// A read waiting for a part's flight, or done with it.
struct Reading<'a> {
    parts: &'a Mutex<Parts>,
    id: (String, u64),
    number: u64,
}

impl Drop for Reading<'_> {
    fn drop(&mut self) {
        let mut parts = self.parts.lock().unwrap();
        if let Some(waited) = parts.flights.get_mut(&self.id).filter(|w| w.number == self.number) {
            waited.readers -= 1;
            if waited.readers == 0 {
                parts.flights.remove(&self.id);
            }
        }
    }
}

impl<B: Bucket> Fetching<B> {
    /// Part `part` of the object at `key`, checked against its CRC: kept, or fetched once `granted`
    /// lets its GET go, and kept then. One fetched that fails its check is fetched again, once.
    async fn part(
        self: &Arc<Self>,
        key: &str,
        part: u64,
        granted: impl Future<Output = Grant>,
    ) -> Result<Part, Error> {
        if let Some(kept) = self.kept(key, part).await? {
            return Ok(Some(kept.into()));
        }
        let range = ByteRange::Bounded(part * PART..(part + 1) * PART);
        // The object's tail, if it is not kept, under the part's grant.
        let grant = granted.await;
        let get = bucket::get_range(&*self.bucket, key, range.clone());
        let (mut bytes, tail) = futures::join!(get, self.tail(key));
        drop(grant);
        let Some(tail) = tail? else { return Ok(None) };
        for attempt in 0..2 {
            if attempt > 0 {
                let (_, granted) = self.budget.enter(Priority::Demand, PART);
                let grant = granted.await;
                bytes = bucket::get_range(&*self.bucket, key, range.clone()).await;
                drop(grant);
            }
            let Some(fetched) = bytes.take() else { return Ok(None) };
            if let Some(checked) = tail.check(key, part, fetched)? {
                self.keep(key, &tail, part, &checked);
                return Ok(Some(checked.into()));
            }
        }
        Err(Error::Corrupt(format!("part {part} of {key}, by its CRC, twice")))
    }

    /// Part `part` of the object at `key`, if the cache has it and it passes its check: one that
    /// fails is dropped from the cache.
    async fn kept(self: &Arc<Self>, key: &str, part: u64) -> Result<Option<Vec<u8>>, Error> {
        let Some(cache) = &self.cache else { return Ok(None) };
        let bytes = match cache.get(key, part) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Ok(None),
            Err(err) => {
                log::warn!("fxdax: reading part {part} of {key} from the cache: {err}");
                return Ok(None);
            }
        };
        // Gone from the bucket: its reader follows where it went.
        let Some(tail) = self.tail(key).await? else { return Ok(None) };
        if let Some(checked) = tail.check(key, part, bytes)? {
            return Ok(Some(checked));
        }
        log::warn!("fxdax: part {part} of {key} in the cache failed its CRC");
        if let Err(err) = cache.forget(key, part) {
            log::warn!("fxdax: dropping part {part} of {key} from the cache: {err}");
        }
        Ok(None)
    }

    /// Keeps `bytes`, checked, in the cache.
    fn keep(&self, key: &str, tail: &Tail, part: u64, bytes: &[u8]) {
        if let Some(cache) = &self.cache
            && let Err(err) = cache.put(key, tail.frame.size(), part, bytes)
        {
            log::warn!("fxdax: keeping part {part} of {key} in the cache: {err}");
        }
    }

    /// The tail of the object at `key`: kept, or in flight, or fetched now.
    fn tail(self: &Arc<Self>, key: &str) -> Flight<Option<Arc<Tail>>> {
        let mut tails = self.tails.lock().unwrap();
        if let Some(tail) = tails.get(key) {
            return tail.clone();
        }
        if tails.len() >= TAILS {
            tails.clear();
        }
        let (shared, key_owned) = (self.clone(), key.to_owned());
        let tail = async move {
            let fetched = shared.fetch_tail(&key_owned).await;
            // What failed, or found the object gone, is not kept.
            if !matches!(fetched, Ok(Some(_))) {
                shared.tails.lock().unwrap().remove(&key_owned);
            }
            fetched
        };
        let tail = tail.boxed().shared();
        tails.insert(key.to_owned(), tail.clone());
        tail
    }

    async fn fetch_tail(&self, key: &str) -> Result<Option<Arc<Tail>>, Error> {
        let Some(mut suffix) = bucket::get_range(&*self.bucket, key, ByteRange::Suffix(TAIL)).await
        else {
            return Ok(None);
        };
        let frame = Frame::of_trailer(&suffix)?;
        if (suffix.len() as u64) < TAIL && frame.size() != suffix.len() as u64 {
            return Err(Error::Corrupt(format!("{key}, by its trailer")));
        }
        let crcs = frame.crcs();
        if (crcs.end - crcs.start) + layout::TRAILER > suffix.len() as u64 {
            let range = ByteRange::Bounded(crcs.start..frame.size());
            let Some(longer) = bucket::get_range(&*self.bucket, key, range).await else {
                return Ok(None);
            };
            suffix = longer;
        }
        let crcs = frame.crcs_in(&suffix)?;
        Ok(Some(Arc::new(Tail { frame, crcs })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bucket::PutMode;
    use crate::layout::Moves;
    use crate::sim::bucket::{Direct, Fault, SimBucket};
    use futures::executor::{LocalPool, block_on};
    use futures::task::LocalSpawnExt as _;
    use std::cell::RefCell;
    use std::rc::Rc;

    const KEY: &str = "segments/0000000000000001/0000000000000000";

    fn pattern(len: u64) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8 ^ (i / PART) as u8).collect()
    }

    /// A bucket holding an object at [`KEY`] of `data`, its requests completed by the test.
    fn bucket(data: &[u8]) -> Arc<SimBucket> {
        let bucket = SimBucket::new();
        let bytes = layout::with_footer(data, &());
        block_on(Direct(bucket.clone()).put(KEY, bytes, PutMode::Create)).unwrap();
        bucket
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

    fn limits(gets: usize) -> Arc<Budget> {
        Budget::new(Limits { gets, bytes: u64::MAX })
    }

    /// Reads fetch the parts that hold what they want, whole, and the object's tail, once.
    #[test]
    fn reads_fetch_whole_parts() {
        let data = pattern(2 * PART + 300);
        let bucket = bucket(&data);
        let fetcher = Rc::new(Fetcher::new(bucket.clone(), limits(8), None));
        let mut reads = Reads::new();
        let location = location(data.len() as u64);
        let read = reads.read(&fetcher, location.clone(), PART - 10, 20, Priority::Demand);
        let requests = complete_all(&bucket, &mut reads);
        let parts = [format!("get {KEY} 0..{PART}"), format!("get {KEY} {PART}..{}", 2 * PART)];
        assert_eq!(
            requests,
            [parts[0].clone(), parts[1].clone(), format!("get {KEY} last {TAIL}")]
        );
        let want = data[(PART - 10) as usize..(PART + 10) as usize].to_vec();
        assert_eq!(reads.result(read), Some(Ok(want)));

        // The tail is kept; past the extent's end, a read reads short.
        let read = reads.read(&fetcher, location, 2 * PART + 200, 1000, Priority::Demand);
        let requests = complete_all(&bucket, &mut reads);
        assert_eq!(requests, [format!("get {KEY} {}..{}", 2 * PART, 3 * PART)]);
        assert_eq!(reads.result(read), Some(Ok(data[(2 * PART + 200) as usize..].to_vec())));
        assert!(fetcher.parts.lock().unwrap().flights.is_empty());
    }

    /// Reads of one part while its GET is in flight share it.
    #[test]
    fn reads_of_a_part_share_its_get() {
        let data = pattern(PART);
        let bucket = bucket(&data);
        let fetcher = Rc::new(Fetcher::new(bucket.clone(), limits(8), None));
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
        let data = pattern(6 * PART);
        let bucket = bucket(&data);
        let fetcher = Rc::new(Fetcher::new(bucket.clone(), limits(1), None));
        let mut reads = Reads::new();
        let location = location(6 * PART);
        let first = reads.read(&fetcher, location.clone(), 0, 1, Priority::Demand);
        // Its part's GET goes, and its tail's with it.
        assert_eq!(bucket.waiting(), 2);
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
        let data = pattern(3 * PART);
        let bucket = bucket(&data);
        let fetcher = Rc::new(Fetcher::new(bucket.clone(), limits(1), None));
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

    /// A part that fails its CRC is fetched again, once; failing again, the read fails.
    #[test]
    fn parts_failing_their_crcs_are_fetched_again_once() {
        let data = pattern(PART + 100);
        let bucket = bucket(&data);
        let fetcher = Rc::new(Fetcher::new(bucket.clone(), limits(8), None));
        let mut reads = Reads::new();
        let location = location(PART + 100);
        let good = layout::with_footer(&data, &());
        let mut flipped = good.clone();
        flipped[PART as usize + 7] ^= 1;
        let put = |bytes: &[u8]| {
            block_on(Direct(bucket.clone()).put(KEY, bytes.to_vec(), PutMode::Overwrite)).unwrap();
        };

        // Corrupt when first read, whole when read again.
        put(&flipped);
        let read = reads.read(&fetcher, location.clone(), PART, 100, Priority::Demand);
        bucket.complete(bucket.position("last").unwrap(), Fault::None);
        bucket.complete(0, Fault::None);
        reads.run();
        put(&good);
        assert_eq!(complete_all(&bucket, &mut reads).len(), 1);
        assert_eq!(reads.result(read), Some(Ok(data[PART as usize..].to_vec())));

        // Corrupt both times.
        put(&flipped);
        let read = reads.read(&fetcher, location, PART, 100, Priority::Demand);
        assert_eq!(complete_all(&bucket, &mut reads).len(), 2);
        let corrupt = format!("part 1 of {KEY}, by its CRC, twice");
        assert_eq!(reads.result(read), Some(Err(Error::Corrupt(corrupt))));
    }

    /// Parts fetched are kept, and read kept, with no GET.
    #[test]
    fn reads_keep_what_they_fetch() {
        let data = pattern(2 * PART);
        let bucket = bucket(&data);
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path(), 8 * PART, 0).unwrap());
        let fetcher = Rc::new(Fetcher::new(bucket.clone(), limits(8), Some(cache.clone())));
        let mut reads = Reads::new();
        let read = reads.read(&fetcher, location(2 * PART), PART + 5, 10, Priority::Demand);
        assert_eq!(complete_all(&bucket, &mut reads).len(), 2);
        let want = Some(Ok(data[(PART + 5) as usize..][..10].to_vec()));
        assert_eq!(reads.result(read), want);
        assert_eq!(cache.bytes(), PART);

        let read = reads.read(&fetcher, location(2 * PART), PART + 5, 10, Priority::Demand);
        assert_eq!(bucket.waiting(), 0);
        assert_eq!(reads.result(read), want);
    }

    /// A part kept that fails its CRC is dropped and fetched; fetched twice failing it, the read
    /// fails.
    #[test]
    fn parts_kept_failing_their_crcs_are_fetched() {
        let data = pattern(PART);
        let bucket = bucket(&data);
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path(), 8 * PART, 0).unwrap());
        let fetcher = Rc::new(Fetcher::new(bucket.clone(), limits(8), Some(cache.clone())));
        let mut reads = Reads::new();
        reads.read(&fetcher, location(PART), 0, 10, Priority::Demand);
        complete_all(&bucket, &mut reads);
        let flip = |bytes: &mut Vec<u8>| bytes[3] ^= 1;
        let mut kept = std::fs::read(cache.path(KEY)).unwrap();
        flip(&mut kept);
        std::fs::write(cache.path(KEY), kept).unwrap();

        // The bucket's is whole.
        let read = reads.read(&fetcher, location(PART), 0, 10, Priority::Demand);
        assert_eq!(complete_all(&bucket, &mut reads), [format!("get {KEY} 0..{PART}")]);
        assert_eq!(reads.result(read), Some(Ok(data[..10].to_vec())));
        assert_eq!(cache.get(KEY, 0).unwrap(), Some(data.clone()));

        // Neither is.
        let mut kept = std::fs::read(cache.path(KEY)).unwrap();
        flip(&mut kept);
        std::fs::write(cache.path(KEY), kept).unwrap();
        let mut object = layout::with_footer(&data, &());
        flip(&mut object);
        block_on(Direct(bucket.clone()).put(KEY, object, PutMode::Overwrite)).unwrap();
        let read = reads.read(&fetcher, location(PART), 0, 10, Priority::Demand);
        assert_eq!(complete_all(&bucket, &mut reads).len(), 2);
        let corrupt = format!("part 0 of {KEY}, by its CRC, twice");
        assert_eq!(reads.result(read), Some(Err(Error::Corrupt(corrupt))));
        assert_eq!(cache.get(KEY, 0).unwrap(), None);
    }

    /// Reads read ahead the part after them, and twice as many each time they go on where the
    /// last stopped; a read elsewhere reads only the part after it ahead again.
    #[test]
    fn read_ahead_grows_along_sequential_reads() {
        let data = pattern(12 * PART);
        let bucket = bucket(&data);
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path(), 64 * PART, 0).unwrap());
        let mut reads = Reads::new();
        let fetcher = Fetcher::new(bucket.clone(), limits(8), Some(cache.clone()));
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
        assert_eq!(cache.bytes(), 12 * PART);
    }

    /// A read of an extent whose object the collector compacted reads it where it moved.
    #[test]
    fn reads_follow_moves() {
        let data = pattern(3000);
        let bucket = bucket(&data);
        let direct = Direct(bucket.clone());
        let to = Location { key: layout::moves(KEY), offset: 10, len: 1000 };
        let mut moved = vec![0; 10];
        moved.extend_from_slice(&data[1000..2000]);
        let moves = layout::with_footer(&moved, &Moves { extents: vec![(EXTENT, to)] });
        block_on(direct.put(&layout::moves(KEY), moves, PutMode::Create)).unwrap();
        block_on(direct.delete(KEY)).unwrap();
        let fetcher = Rc::new(Fetcher::new(bucket.clone(), limits(8), None));
        let mut reads = Reads::new();
        let from = Location { key: KEY.into(), offset: 1000, len: 1000 };
        let read = reads.read(&fetcher, from, 100, 50, Priority::Demand);
        complete_all(&bucket, &mut reads);
        assert_eq!(reads.result(read), Some(Ok(data[1100..1150].to_vec())));
    }
}
