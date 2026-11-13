// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: what tests of the protocol need (the `testing` feature, TASK-88): a bucket whose requests
//! take effect when the test says ([`SimBucket`]): in any order, failing before or after taking
//! effect, or after whoever made them has gone; a writer's creates of segments, if it says, only
//! once an object at their keys was deleted; and the collector's moves, which a test follows as a
//! reader does ([`MovesMaps`]). extent_store's simulation of doc-7's protocol runs on it, as this
//! crate's own tests do.

use crate::bucket::{Bucket, ByteRange, Error, Listing, Meta, Object, PutMode, Version};
use crate::layout::{self, Branch, Kind};
pub use crate::moves::{Moved, MovesMaps};
use futures::channel::oneshot;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

enum Op {
    Get(String),
    GetRange(String, ByteRange),
    Head(String),
    List(String),
    ListDelimited(String),
    Put(String, Vec<u8>, PutMode),
    Delete(String),
}

enum Reply {
    Object(Object),
    Bytes(Vec<u8>),
    Meta(Meta),
    List(Vec<Meta>),
    Listing(Listing),
    Version(Version),
    Done,
}

struct Stored {
    bytes: Vec<u8>,
    version: u64,
    modified: u64,
}

struct Request {
    op: Op,
    reply: oneshot::Sender<Result<Reply, Error>>,
}

#[derive(Default)]
struct Inner {
    objects: BTreeMap<String, Stored>,
    versions: u64,
    clock: u64,
    queue: Vec<Request>,
    /// The writers whose creates of segments are held ([`SimBucket::hold`]): of each branch, those
    /// of this lease epoch and before.
    held: BTreeMap<Branch, u64>,
    /// The creates held, out of the queue, each until an object at its key is deleted.
    parked: Vec<Request>,
    /// Each request completed, in order, and what it gave ([`SimBucket::journal`]).
    journal: Vec<String>,
}

/// How a request the simulation completes fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    None,
    /// It fails without taking effect.
    Before,
    /// It takes effect, then fails.
    After,
}

#[derive(Default)]
pub struct SimBucket {
    inner: Mutex<Inner>,
}

impl SimBucket {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    fn request(&self, op: Op) -> impl Future<Output = Result<Reply, Error>> + Send + use<> {
        let (reply, receiver) = oneshot::channel();
        self.inner.lock().unwrap().enqueue(Request { op, reply });
        // A request dropped unanswered failed.
        async move { receiver.await.unwrap_or(Err(Error::Unavailable)) }
    }

    /// How many requests wait, but for those held.
    pub fn waiting(&self) -> usize {
        self.inner.lock().unwrap().queue.len()
    }

    /// Holds the creates of segments of `branch` by its writers of lease epoch `epoch` and before,
    /// those waiting and those to come, each until a delete of an object at its key takes effect,
    /// when it waits again: a writer taken over, whose creates are in flight while the next one's
    /// segments take their places, which the collector compacts and deletes, landing then
    /// (TASK-79.18).
    pub fn hold(&self, branch: Branch, epoch: u64) {
        let inner = &mut *self.inner.lock().unwrap();
        let held = inner.held.entry(branch).or_default();
        *held = (*held).max(epoch);
        for request in std::mem::take(&mut inner.queue) {
            inner.enqueue(request);
        }
    }

    /// Lets every create held wait again, and holds none from now on.
    pub fn release(&self) {
        let inner = &mut *self.inner.lock().unwrap();
        inner.held.clear();
        inner.queue.append(&mut inner.parked);
    }

    /// Completes waiting request `index`, failing it as `fault` says.
    pub fn complete(&self, index: usize, fault: Fault) {
        let mut inner = self.inner.lock().unwrap();
        let request = inner.queue.swap_remove(index);
        let asked = request.op.describe();
        let result = match fault {
            Fault::Before => Err(Error::Unavailable),
            Fault::None => inner.apply(request.op),
            Fault::After => inner.apply(request.op).and(Err(Error::Unavailable)),
        };
        let gave = match &result {
            Ok(_) => "done".to_owned(),
            Err(error) => format!("{error:?}"),
        };
        inner.journal.push(format!("{asked} {fault:?}: {gave}"));
        drop(inner);
        // Its maker may have gone; it took effect regardless.
        let _ = request.reply.send(result);
    }

    /// What waiting request `index` asks: `get KEY START..END` for a ranged read, `get KEY last N`
    /// for its end, and the operation and key for the others.
    pub fn describe(&self, index: usize) -> String {
        self.inner.lock().unwrap().queue[index].op.describe_key()
    }

    /// Every request completed, in the order they were, each as [`SimBucket::describe`] says,
    /// a put's with a hash of its bytes, and how it failed, if it did: what a run of a seed of a
    /// simulation asked of the bucket, which another run of that seed must ask the same.
    pub fn journal(&self) -> Vec<String> {
        self.inner.lock().unwrap().journal.clone()
    }

    /// Runs `future`, completing its requests as they come, none failing: what it gives, and what
    /// they asked, in order ([`SimBucket::describe`]).
    pub fn served<T>(&self, future: impl Future<Output = T>) -> (T, Vec<String>) {
        let mut future = std::pin::pin!(future);
        let mut context = Context::from_waker(futures::task::noop_waker_ref());
        let mut asked = Vec::new();
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return (output, asked);
            }
            assert!(self.waiting() > 0, "waiting for no request");
            asked.push(self.describe(0));
            self.complete(0, Fault::None);
        }
    }

    /// Runs `future`, as [`SimBucket::served`] does, but completing in each round every request
    /// waiting: what it gives, and what each round asked, sorted.
    pub fn served_in_rounds<T>(&self, future: impl Future<Output = T>) -> (T, Vec<Vec<String>>) {
        let mut future = std::pin::pin!(future);
        let mut context = Context::from_waker(futures::task::noop_waker_ref());
        let mut rounds = Vec::new();
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return (output, rounds);
            }
            assert!(self.waiting() > 0, "waiting for no request");
            let mut round: Vec<_> = (0..self.waiting()).map(|index| self.describe(index)).collect();
            round.sort();
            while self.waiting() > 0 {
                self.complete(0, Fault::None);
            }
            rounds.push(round);
        }
    }

    /// The first waiting request whose description holds `text`.
    pub fn position(&self, text: &str) -> Option<usize> {
        (0..self.waiting()).find(|&index| self.describe(index).contains(text))
    }

    pub fn advance(&self, ticks: u64) {
        self.inner.lock().unwrap().clock += ticks;
    }

    /// The keys of every object.
    pub fn keys(&self) -> Vec<String> {
        self.inner.lock().unwrap().objects.keys().cloned().collect()
    }
}

impl Op {
    /// The operation and its key ([`SimBucket::describe`]).
    fn describe_key(&self) -> String {
        match self {
            Op::GetRange(key, ByteRange::Bounded(range)) => format!("get {key} {range:?}"),
            Op::GetRange(key, ByteRange::Suffix(n)) => format!("get {key} last {n}"),
            Op::Get(key) => format!("get {key}"),
            Op::Head(key) => format!("head {key}"),
            Op::List(prefix) => format!("list {prefix}"),
            Op::ListDelimited(prefix) => format!("list-delimited {prefix}"),
            Op::Put(key, ..) => format!("put {key}"),
            Op::Delete(key) => format!("delete {key}"),
        }
    }

    /// As [`Op::describe_key`], a put's with a hash of its bytes and its mode, as the journal
    /// keeps it: SipHash of fixed keys, the same in every process.
    fn describe(&self) -> String {
        use std::hash::{DefaultHasher, Hasher as _};
        match self {
            Op::Put(key, bytes, mode) => {
                let mut hasher = DefaultHasher::new();
                hasher.write(bytes);
                format!("put {key} {:016x} {mode:?}", hasher.finish())
            }
            op => op.describe_key(),
        }
    }
}

impl Inner {
    /// Queues `request`, or holds it, if it is a create of a segment by a writer whose creates are
    /// held ([`SimBucket::hold`]).
    fn enqueue(&mut self, request: Request) {
        // A create's footer is read only for a branch whose creates are held, as only the
        // recreated profile's are.
        let held = |key: &str, bytes: &[u8]| {
            let Kind::Segment { branch, .. } = layout::kind(key) else { return false };
            let Some(&held) = self.held.get(&branch) else { return false };
            layout::Segment::footer(bytes).is_ok_and(|footer| footer.epoch <= held)
        };
        match &request.op {
            Op::Put(key, bytes, PutMode::Create) if held(key, bytes) => self.parked.push(request),
            _ => self.queue.push(request),
        }
    }

    fn meta(&self, key: &str) -> Option<Meta> {
        self.objects.get(key).map(|stored| Meta {
            key: key.to_owned(),
            version: Version { e_tag: Some(stored.version.to_string()), version: None },
            size: stored.bytes.len() as u64,
            modified: stored.modified,
        })
    }

    fn apply(&mut self, op: Op) -> Result<Reply, Error> {
        match op {
            Op::Get(key) => {
                let meta = self.meta(&key).ok_or(Error::NotFound)?;
                Ok(Reply::Object(Object { bytes: self.objects[&key].bytes.clone(), meta }))
            }
            Op::GetRange(key, range) => {
                let bytes = &self.objects.get(&key).ok_or(Error::NotFound)?.bytes;
                let len = bytes.len() as u64;
                // Refused as `object_store` refuses it, as though the store were out of reach: of
                // no bytes, or beginning past the object's end.
                let empty = matches!(&range, ByteRange::Bounded(range) if range.is_empty());
                if empty || !range.begins_within(len) {
                    return Err(Error::Unavailable);
                }
                let range = match range {
                    ByteRange::Bounded(range) => range.start..range.end.min(len),
                    ByteRange::Suffix(n) => len.saturating_sub(n)..len,
                };
                Ok(Reply::Bytes(bytes[range.start as usize..range.end as usize].to_vec()))
            }
            Op::Head(key) => self.meta(&key).map(Reply::Meta).ok_or(Error::NotFound),
            Op::List(prefix) => Ok(Reply::List(
                self.objects
                    .range(prefix.clone()..)
                    .take_while(|(key, _)| key.starts_with(&prefix))
                    .map(|(key, _)| self.meta(key).unwrap())
                    .collect(),
            )),
            Op::ListDelimited(prefix) => {
                assert!(prefix.is_empty() || prefix.ends_with('/'), "a prefix of keys: {prefix}");
                let mut listing = Listing::default();
                let under = self.objects.range(prefix.clone()..);
                for (key, _) in under.take_while(|(key, _)| key.starts_with(&prefix)) {
                    match key[prefix.len()..].find('/') {
                        Some(slash) => {
                            let common = &key[..prefix.len() + slash + 1];
                            if listing.prefixes.last().is_none_or(|last| last != common) {
                                listing.prefixes.push(common.to_owned());
                            }
                        }
                        None => listing.objects.push(self.meta(key).unwrap()),
                    }
                }
                Ok(Reply::Listing(listing))
            }
            Op::Put(key, bytes, mode) => {
                let existing = self.objects.get(&key).map(|stored| stored.version);
                match (mode, existing) {
                    (PutMode::Create, Some(_)) => return Err(Error::AlreadyExists),
                    (PutMode::Update(_), None) => return Err(Error::Precondition),
                    (PutMode::Update(Version { e_tag, .. }), Some(existing))
                        if e_tag != Some(existing.to_string()) =>
                    {
                        return Err(Error::Precondition);
                    }
                    _ => {}
                }
                self.versions += 1;
                let stored = Stored { bytes, version: self.versions, modified: self.clock };
                self.objects.insert(key, stored);
                Ok(Reply::Version(Version {
                    e_tag: Some(self.versions.to_string()),
                    version: None,
                }))
            }
            Op::Delete(key) => {
                // The creates held at its key wait again once an object there is deleted.
                if self.objects.remove(&key).is_some() {
                    let (unparked, parked) = std::mem::take(&mut self.parked)
                        .into_iter()
                        .partition(|request| matches!(&request.op, Op::Put(at, ..) if *at == key));
                    self.parked = parked;
                    self.queue.extend::<Vec<_>>(unparked);
                }
                Ok(Reply::Done)
            }
        }
    }
}

macro_rules! reply {
    ($future:expr, $variant:ident) => {{
        let future = $future;
        async move {
            match future.await? {
                Reply::$variant(value) => Ok(value),
                _ => unreachable!(),
            }
        }
    }};
}

impl Bucket for SimBucket {
    fn get(&self, key: &str) -> impl Future<Output = Result<Object, Error>> + Send {
        reply!(self.request(Op::Get(key.to_owned())), Object)
    }

    fn get_range(
        &self,
        key: &str,
        range: ByteRange,
    ) -> impl Future<Output = Result<Vec<u8>, Error>> + Send {
        reply!(self.request(Op::GetRange(key.to_owned(), range)), Bytes)
    }

    fn head(&self, key: &str) -> impl Future<Output = Result<Meta, Error>> + Send {
        reply!(self.request(Op::Head(key.to_owned())), Meta)
    }

    fn list(&self, prefix: &str) -> impl Future<Output = Result<Vec<Meta>, Error>> + Send {
        reply!(self.request(Op::List(prefix.to_owned())), List)
    }

    fn list_delimited(&self, prefix: &str) -> impl Future<Output = Result<Listing, Error>> + Send {
        reply!(self.request(Op::ListDelimited(prefix.to_owned())), Listing)
    }

    fn put(
        &self,
        key: &str,
        bytes: bytes::Bytes,
        mode: PutMode,
    ) -> impl Future<Output = Result<Version, Error>> + Send {
        reply!(self.request(Op::Put(key.to_owned(), bytes.into(), mode)), Version)
    }

    fn delete(&self, key: &str) -> impl Future<Output = Result<(), Error>> + Send {
        let future = self.request(Op::Delete(key.to_owned()));
        async move { future.await.map(|_| ()) }
    }

    fn now(&self) -> u64 {
        self.inner.lock().unwrap().clock
    }
}

/// The same bucket, its requests taking effect at once and never failing: the simulation's view,
/// to check what the branches hold.
pub struct Direct(pub Arc<SimBucket>);

impl Direct {
    fn apply(&self, op: Op) -> Result<Reply, Error> {
        self.0.inner.lock().unwrap().apply(op)
    }
}

impl Bucket for Direct {
    fn get(&self, key: &str) -> impl Future<Output = Result<Object, Error>> + Send {
        reply!(std::future::ready(self.apply(Op::Get(key.to_owned()))), Object)
    }

    fn get_range(
        &self,
        key: &str,
        range: ByteRange,
    ) -> impl Future<Output = Result<Vec<u8>, Error>> + Send {
        reply!(std::future::ready(self.apply(Op::GetRange(key.to_owned(), range))), Bytes)
    }

    fn head(&self, key: &str) -> impl Future<Output = Result<Meta, Error>> + Send {
        reply!(std::future::ready(self.apply(Op::Head(key.to_owned()))), Meta)
    }

    fn list(&self, prefix: &str) -> impl Future<Output = Result<Vec<Meta>, Error>> + Send {
        reply!(std::future::ready(self.apply(Op::List(prefix.to_owned()))), List)
    }

    fn list_delimited(&self, prefix: &str) -> impl Future<Output = Result<Listing, Error>> + Send {
        reply!(std::future::ready(self.apply(Op::ListDelimited(prefix.to_owned()))), Listing)
    }

    fn put(
        &self,
        key: &str,
        bytes: bytes::Bytes,
        mode: PutMode,
    ) -> impl Future<Output = Result<Version, Error>> + Send {
        reply!(std::future::ready(self.apply(Op::Put(key.to_owned(), bytes.into(), mode))), Version)
    }

    fn delete(&self, key: &str) -> impl Future<Output = Result<(), Error>> + Send {
        let result = self.apply(Op::Delete(key.to_owned())).map(|_| ());
        std::future::ready(result)
    }

    fn now(&self) -> u64 {
        self.0.now()
    }
}

/// fxdax: the live branches of every cell of `bucket`, as `fxdax-gc --branches` lists them
/// (TASK-395.4).
pub async fn live(
    bucket: &impl Bucket,
) -> Result<BTreeMap<Branch, crate::record::Record>, crate::Error> {
    let mut live = BTreeMap::new();
    for cell in crate::cell::listed(bucket).await {
        live.extend(crate::branch::live(bucket, cell).await?);
    }
    Ok(live)
}

/// fxdax: sweeps of the collector over every cell of `bucket`, of `options`, each as a holder of
/// its own, until two in a row delete and compact nothing: their reports, summed. Two, as a pass
/// over a dead cell retires the references that kept what its parents' cells reclaim at their next,
/// and a pass retiring them again deletes nothing (TASK-395.4). It fails as a sweep fails, and
/// panics after [`SWEEPS`]: passes that never settle are a bug.
pub async fn collect_until_quiet(
    bucket: &impl Bucket,
    options: &crate::collector::Options,
) -> Result<crate::collector::Report, crate::Error> {
    let mut collector = crate::collector::Collector::new(*options);
    let (mut total, mut quiet) = (crate::collector::Report::default(), 0);
    for sweep in 0..SWEEPS {
        let report = collector.collect(bucket, 1 << 40 | sweep).await?;
        total += report;
        quiet = if report.deleted == 0 && report.compacted == 0 { quiet + 1 } else { 0 };
        if quiet == 2 {
            return Ok(total);
        }
    }
    panic!("{SWEEPS} sweeps of the collector, which kept doing something: {total:?}");
}

/// fxdax: how many sweeps [`collect_until_quiet`] runs at most.
pub const SWEEPS: u64 = 32;

/// fxdax: what the collector should have collected of `bucket` and left (TASK-390), once
/// passes ran alone, of `compact_below`, past the grace period, the branches' writers all done:
/// each a line, `garbage left: KEY`, an object no live branch needs, or `left uncompacted: KEY, ...`,
/// one a live branch names whose live extents take less than `compact_below` of its data, its
/// moves not written. The live branches are those whose records say so, recovered as their
/// records say they were made; one that fails to recover is passed over, as a claim's may.
///
/// It works out what the live branches need as doc-7 says, not as the collector does, so that a
/// collector that keeps or compacts too little of what is freed fails it, as one that loses what is
/// live fails the tests' reads: the space a pass frees, not only what it keeps, is checked. It
/// was the simulation's end checks; TASK-293's compressed segments, never compacted, went unseen
/// by every test outside it.
///
/// fxdax: per cell and conservatively, as doc-7 has the collector since TASK-395: each fork point a
/// reference names, or a live branch's fork to another cell, is needed whole, its state's extents
/// along their ways from where they were sealed, and what recovering it reads, whatever its
/// children hold of it; a reference is garbage once its child is deleted, of that origin, in
/// another cell holding no live branch and no reference; the collector's leases and marks stay.
pub async fn uncollected(bucket: &impl Bucket, compact_below: f64) -> Vec<String> {
    use crate::layout::{Location, Origin};
    use crate::record::{Kind as Made, Record};
    use storage_device::ExtentId;
    let keys: BTreeSet<String> =
        crate::bucket::list(bucket, "").await.into_iter().map(|meta| meta.key).collect();
    let mut records: BTreeMap<Branch, Record> = BTreeMap::new();
    let mut references = Vec::new();
    for key in &keys {
        match layout::kind(key) {
            Kind::Record { branch } => {
                if let Ok(Some(record)) = crate::branch::record(bucket, branch).await {
                    records.insert(branch, record);
                }
            }
            Kind::Reference { parent, at, child } => references.push((key, parent, at, child)),
            _ => {}
        }
    }
    // A cell no live branch and no reference keeps, whose pass retires its children's references.
    let dead = |cell: layout::Cell| {
        records.iter().filter(|(branch, _)| branch.cell == cell).all(|(_, record)| record.deleted)
            && !references.iter().any(|(_, parent, _, _)| parent.cell == cell)
    };
    let mut maps = MovesMaps::default();
    let mut needed = BTreeSet::new();
    let mut touched = BTreeSet::new();
    let mut named: BTreeMap<String, BTreeMap<ExtentId, u64>> = BTreeMap::new();
    let mut points: BTreeSet<Origin> = BTreeSet::new();
    let mut retirable = BTreeSet::new();
    for &(key, parent, at, child) in &references {
        let origin = Origin { parent, at };
        let retired = records.get(&child).is_some_and(|record| {
            record.deleted && record.origin() == Some(origin) && child.cell != parent.cell
        }) && dead(child.cell);
        match retired {
            true => drop(retirable.insert(key.clone())),
            false => drop(points.insert(origin)),
        }
    }
    for (&branch, record) in &records {
        let key = layout::record(branch);
        // Deleted branches' records stay (TASK-174).
        if record.deleted {
            if !record.may_drop() {
                needed.insert(key);
            }
            continue;
        }
        needed.insert(key);
        needed.insert(layout::lease(branch));
        let (origin, point): (Option<Origin>, bool) = match record.kind {
            Made::Root | Made::Own => (None, false),
            Made::Fork(origin) | Made::Claim(origin) | Made::Forking { origin, .. } => {
                (Some(origin), false)
            }
            Made::Point(origin) => (Some(origin), true),
        };
        let Ok(recovered) = crate::recovery::recover(bucket, branch, origin).await else {
            continue;
        };
        // Its forks to other cells, whose children may have no references yet.
        for fork in &recovered.state.forks {
            if fork.child.cell != branch.cell {
                points.insert(Origin { parent: branch, at: fork.at });
            }
        }
        touched.extend(recovered.touched.iter().cloned());
        needed.extend(recovered.touched);
        // Its writer may name its extents anywhere on their ways from where they were sealed,
        // unless its state is from a checkpoint of the writer holding its lease (TASK-175).
        let lease = crate::branch::lease_epoch(bucket, &layout::lease(branch)).await.ok();
        let behind = lease != recovered.checkpointer;
        // Its pins, where a writer taken over or taking it over may still create (TASK-388).
        match crate::collector::pinned(&recovered.state, lease.unwrap_or(0)) {
            crate::collector::Pinned::All => {
                let prefix = layout::segments(branch);
                needed.extend(keys.iter().filter(|key| key.starts_with(&prefix)).cloned());
            }
            crate::collector::Pinned::Only(pinned) => {
                needed.extend(pinned.iter().map(|&seq| layout::segment(branch, seq)));
            }
        }
        for (&extent, location) in &recovered.state.extents {
            needed.insert(location.key.clone());
            named.entry(location.key.clone()).or_default().insert(extent, location.len);
            let mut key = location.key.clone();
            while let Some(from) = layout::moved_from(&key).filter(|_| behind) {
                needed.insert(key);
                key = from;
            }
            // Its readers follow the moves of what its state names, until a checkpoint records
            // where they went: as a point's do for as long as it lives, as it has no writer to
            // record them, and a branch's whose writer has not checkpointed since.
            let mut at: Location = location.clone();
            while keys.contains(&layout::moves(&at.key)) {
                let Ok(Moved::To(to)) = maps.moved(bucket, extent, &at.key).await else { break };
                needed.insert(layout::moves(&at.key));
                needed.insert(to.key.clone());
                if point {
                    named.entry(to.key.clone()).or_default().insert(extent, to.len);
                }
                at = to;
            }
        }
    }
    // Each fork point kept whole, once (TASK-395.4).
    let checkpoints = crate::recovery::Checkpoints::new(bucket, 0);
    for origin in points {
        let Ok((state, read)) =
            crate::recovery::recover_at(&checkpoints, origin.parent, origin.at).await
        else {
            continue;
        };
        touched.extend(read.iter().cloned());
        needed.extend(read);
        for (&extent, location) in &state.extents {
            needed.insert(location.key.clone());
            named.entry(location.key.clone()).or_default().insert(extent, location.len);
            let mut key = location.key.clone();
            while let Some(from) = layout::moved_from(&key) {
                needed.insert(key);
                key = from;
            }
            let mut at: Location = location.clone();
            while keys.contains(&layout::moves(&at.key)) {
                let Ok(Moved::To(to)) = maps.moved(bucket, extent, &at.key).await else { break };
                needed.insert(layout::moves(&at.key));
                needed.insert(to.key.clone());
                named.entry(to.key.clone()).or_default().insert(extent, to.len);
                at = to;
            }
        }
    }
    let mut left = Vec::new();
    for key in &keys {
        // fxdax: cells' objects stay, as their branches' records do, their epochs fencing their
        // volumes' leases; blocks of object IDs taken, for good (TASK-364); and the collector's
        // leases and marks, and references not yet retirable (TASK-395).
        let kept = match layout::kind(key) {
            Kind::Cell { .. }
            | Kind::ObjectIds { .. }
            | Kind::GcLease { .. }
            | Kind::Marks { .. } => true,
            Kind::Reference { .. } => !retirable.contains(key),
            _ => false,
        };
        if !kept && !needed.contains(key) {
            left.push(format!("garbage left: {key}"));
        }
    }
    for (key, extents) in named {
        let compactable = matches!(layout::kind(&key), Kind::Segment { .. } | Kind::Moves { .. });
        if !compactable || touched.contains(&key) || keys.contains(&layout::moves(&key)) {
            continue;
        }
        let Some(object) = crate::bucket::get(bucket, &key).await else { continue };
        let Ok(data) = layout::data(&object.bytes) else { continue };
        let used: u64 = extents.values().sum();
        if (used as f64) < compact_below * data.len() as f64 {
            left.push(format!("left uncompacted: {key}, {used} of its {} bytes live", data.len()));
        }
    }
    left
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::test_branch;
    use futures::executor::block_on;

    /// TASK-79.40: the simulation refuses the ranged reads `object_store` refuses, as it does, as
    /// though the store were out of reach: of no bytes, or starting past the object's end, as its
    /// objects in memory and S3's and GCS's HTTP answers alike; and, as those answers, a suffix of
    /// no bytes, or of an object of none. A range ending past the object's end reads short.
    #[test]
    fn ranges_are_refused_as_object_stores_refuse_them() {
        let sim = Direct(SimBucket::new());
        let memory = std::sync::Arc::new(object_store::memory::InMemory::new());
        let memory = crate::store::Store::new(memory, "");
        for bucket in [&sim as &dyn Ranged, &memory] {
            bucket.put_bytes("key", (0..100).collect());
            bucket.put_bytes("empty", Vec::new());
        }
        for range in [0..10, 90..110, 99..100, 100..110, 150..160, 10..10] {
            let range = ByteRange::Bounded(range);
            let (simulated, refused) = (sim.read("key", &range), memory.read("key", &range));
            assert_eq!(simulated, refused, "{range:?}");
        }
        assert_eq!(sim.read("key", &ByteRange::Suffix(10)), Ok((90..100).collect()));
        assert_eq!(sim.read("key", &ByteRange::Suffix(200)), Ok((0..100).collect()));
        assert_eq!(sim.read("key", &ByteRange::Suffix(0)), Err(Error::Unavailable));
        assert_eq!(sim.read("empty", &ByteRange::Suffix(10)), Err(Error::Unavailable));
    }

    /// TASK-79.73: a create of a branch's segment, its writer's creates held, waits until a delete
    /// of an object at its key takes effect, then waits as any request does; a delete of none
    /// lets it go no more than any other request does.
    #[test]
    fn held_creates_wait_for_objects_at_their_keys_to_be_deleted() {
        let bucket = SimBucket::new();
        bucket.hold(test_branch(2), 1);
        let key = layout::segment(test_branch(2), 4);
        let footer =
            layout::Chain::new(test_branch(2), 1, 4, None, 0).close(layout::Contents::default());
        let stale = layout::Segment { data: Vec::new(), footer }.encode();
        let created = bucket.put(&key, stale.into(), PutMode::Create);
        assert_eq!(bucket.waiting(), 0);
        let deleted = bucket.delete(&key);
        bucket.complete(0, Fault::None);
        assert_eq!((block_on(deleted), bucket.waiting()), (Ok(()), 0));

        block_on(Direct(bucket.clone()).put(&key, vec![1].into(), PutMode::Create)).unwrap();
        let deleted = bucket.delete(&key);
        bucket.complete(0, Fault::None);
        assert_eq!((block_on(deleted), bucket.describe(0)), (Ok(()), format!("put {key}")));
        bucket.complete(0, Fault::None);
        let version = Version { e_tag: Some("2".to_owned()), version: None };
        assert_eq!(block_on(created), Ok(version));
    }

    /// A create that lands and then fails, retried, finds its own object and succeeds; one that finds
    /// another's does not.
    #[test]
    fn ambiguous_creates_are_resolved_by_reading_back() {
        use crate::bucket::{Created, create};
        use futures::executor::LocalPool;
        use futures::task::LocalSpawnExt as _;
        use std::cell::RefCell;
        use std::rc::Rc;

        let sim = SimBucket::new();
        let mut pool = LocalPool::new();
        let results = Rc::new(RefCell::new(Vec::new()));
        for (key, bytes) in [("ours", b"ours"), ("theirs", b"ours")] {
            let (sim, results) = (sim.clone(), results.clone());
            pool.spawner()
                .spawn_local(async move {
                    let created = create(&*sim, key, bytes.to_vec()).await;
                    results.borrow_mut().push((key, matches!(created, Created::Created)));
                })
                .unwrap();
        }
        pool.run_until_stalled();
        // "theirs" holds another's object before its create lands.
        block_on(Direct(sim.clone()).put("theirs", b"theirs".to_vec().into(), PutMode::Create))
            .unwrap();
        // The first create of each lands and fails; what follows succeeds.
        let mut first = true;
        while sim.waiting() > 0 {
            sim.complete(0, if first { Fault::After } else { Fault::None });
            first = false;
            pool.run_until_stalled();
        }
        let mut results = results.borrow().clone();
        results.sort();
        assert_eq!(results, [("ours", true), ("theirs", false)]);
    }

    /// A bucket the test above reads ranges of, at once.
    trait Ranged {
        fn put_bytes(&self, key: &str, bytes: Vec<u8>);
        fn read(&self, key: &str, range: &ByteRange) -> Result<Vec<u8>, Error>;
    }

    impl<B: Bucket> Ranged for B {
        fn put_bytes(&self, key: &str, bytes: Vec<u8>) {
            block_on(self.put(key, bytes.into(), PutMode::Create)).unwrap();
        }

        fn read(&self, key: &str, range: &ByteRange) -> Result<Vec<u8>, Error> {
            block_on(self.get_range(key, range.clone()))
        }
    }
}
