//! A bucket whose requests take effect when the simulation says: in any order, failing before or
//! after taking effect, or after whoever made them has gone.

use crate::bucket::{Bucket, Error, Meta, Object, PutMode, Version};
use futures::channel::oneshot;
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

enum Op {
    Get(String),
    Head(String),
    List(String),
    Put(String, Vec<u8>, PutMode),
    Delete(String),
}

enum Reply {
    Object(Object),
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
            version: Version(stored.version.to_string()),
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
                    (PutMode::Update(Version(version)), Some(existing))
                        if version != existing.to_string() =>
                    {
                        return Err(Error::Precondition);
                    }
                    _ => {}
                }
                self.versions += 1;
                let stored = Stored { bytes, version: self.versions, modified: self.clock };
                self.objects.insert(key, stored);
                Ok(Reply::Version(Version(self.versions.to_string())))
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
