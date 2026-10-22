// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A bucket whose requests take effect when the simulation says: in any order, failing before or
//! after taking effect, or after whoever made them has gone.

use crate::bucket::{Bucket, ByteRange, Error, Meta, Object, PutMode, Version};
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
        self.inner.lock().unwrap().queue.push(Request { op, reply });
        // A request dropped unanswered failed.
        async move { receiver.await.unwrap_or(Err(Error::Unavailable)) }
    }

    /// How many requests wait.
    pub fn waiting(&self) -> usize {
        self.inner.lock().unwrap().queue.len()
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
                let range = match range {
                    ByteRange::Bounded(range) => range.start.min(len)..range.end.min(len),
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
