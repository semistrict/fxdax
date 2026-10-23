// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A bucket whose requests take effect when the simulation says: in any order, failing before or
//! after taking effect, or after whoever made them has gone; a writer's creates of segments, if it
//! says, only once an object at their keys was deleted.

use crate::bucket::{Bucket, ByteRange, Error, Meta, Object, PutMode, Version};
use crate::layout::{self, Kind};
use futures::channel::oneshot;
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

enum Op {
    Get(String),
    GetRange(String, ByteRange),
    Head(String),
    List(String),
    Put(String, Vec<u8>, PutMode),
    Delete(String),
}

enum Reply {
    Object(Object),
    Bytes(Vec<u8>),
    Meta(Meta),
    List(Vec<Meta>),
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
    held: BTreeMap<u64, u64>,
    /// The creates held, out of the queue, each until an object at its key is deleted.
    parked: Vec<Request>,
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
    pub fn hold(&self, branch: u64, epoch: u64) {
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
        let result = match fault {
            Fault::Before => Err(Error::Unavailable),
            Fault::None => inner.apply(request.op),
            Fault::After => inner.apply(request.op).and(Err(Error::Unavailable)),
        };
        drop(inner);
        // Its maker may have gone; it took effect regardless.
        let _ = request.reply.send(result);
    }

    /// What waiting request `index` asks: `get KEY START..END` for a ranged read, `get KEY last N`
    /// for its end, and the operation and key for the others.
    pub fn describe(&self, index: usize) -> String {
        match &self.inner.lock().unwrap().queue[index].op {
            Op::GetRange(key, ByteRange::Bounded(range)) => format!("get {key} {range:?}"),
            Op::GetRange(key, ByteRange::Suffix(n)) => format!("get {key} last {n}"),
            Op::Get(key) => format!("get {key}"),
            Op::Head(key) => format!("head {key}"),
            Op::List(prefix) => format!("list {prefix}"),
            Op::Put(key, ..) => format!("put {key}"),
            Op::Delete(key) => format!("delete {key}"),
        }
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

impl Inner {
    /// Queues `request`, or holds it, if it is a create of a segment by a writer whose creates are
    /// held ([`SimBucket::hold`]).
    fn enqueue(&mut self, request: Request) {
        let held = |key: &str, bytes: &[u8]| {
            let Kind::Segment { branch, .. } = layout::kind(key) else { return false };
            let epoch = layout::Segment::footer(bytes).map(|footer| footer.epoch);
            self.held.get(&branch).is_some_and(|&held| epoch.is_ok_and(|epoch| epoch <= held))
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
                self.objects.remove(&key);
                let (unparked, parked) = std::mem::take(&mut self.parked)
                    .into_iter()
                    .partition(|request| matches!(&request.op, Op::Put(at, ..) if *at == key));
                self.parked = parked;
                self.queue.extend::<Vec<_>>(unparked);
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

    fn put(
        &self,
        key: &str,
        bytes: Vec<u8>,
        mode: PutMode,
    ) -> impl Future<Output = Result<Version, Error>> + Send {
        reply!(self.request(Op::Put(key.to_owned(), bytes, mode)), Version)
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

    fn put(
        &self,
        key: &str,
        bytes: Vec<u8>,
        mode: PutMode,
    ) -> impl Future<Output = Result<Version, Error>> + Send {
        reply!(std::future::ready(self.apply(Op::Put(key.to_owned(), bytes, mode))), Version)
    }

    fn delete(&self, key: &str) -> impl Future<Output = Result<(), Error>> + Send {
        let result = self.apply(Op::Delete(key.to_owned())).map(|_| ());
        std::future::ready(result)
    }

    fn now(&self) -> u64 {
        self.0.now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// A bucket the test above reads ranges of, at once.
    trait Ranged {
        fn put_bytes(&self, key: &str, bytes: Vec<u8>);
        fn read(&self, key: &str, range: &ByteRange) -> Result<Vec<u8>, Error>;
    }

    impl<B: Bucket> Ranged for B {
        fn put_bytes(&self, key: &str, bytes: Vec<u8>) {
            block_on(self.put(key, bytes, PutMode::Create)).unwrap();
        }

        fn read(&self, key: &str, range: &ByteRange) -> Result<Vec<u8>, Error> {
            block_on(self.get_range(key, range.clone()))
        }
    }
}
