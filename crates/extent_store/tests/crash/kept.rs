// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the kept rounds (TASK-165.9's AC #4): a store keeping its branch in a bucket, its
//! uploader uploading and acknowledging on a runtime's threads as the workload goes, crashes at a
//! random point; then the branch is taken from the bucket alone, its disk lost, and the store is
//! opened again from what the crash left. Taken from the bucket, it holds a prefix of the commits
//! at least as late as the last its uploader acknowledged; opened from its disk, one at least as
//! late as the last made durable, its write-ahead log following the branch, which the store
//! taking it fenced. Not a function of its seed alone, as the rounds of `rounds` are: the
//! uploader's threads interleave with the workload as they will.
//!
//! fxdax: passes of the collector run over the bucket all along the workload, compacting, each
//! after the last, on the host's runtime, as its collector would; one is cut at a random request
//! of its, as the host dies under it ([`Cut`]); and a full pass runs after the crash, before the
//! branch is taken and the store opened again (TASK-291.2).

use super::rounds::{BLOCK, BRANCH, Model, OPTIONS, workload};
use extent_remote::bucket::{ByteRange, Error as BucketError, Meta, Object, PutMode, Version};
use extent_remote::collector;
use extent_remote::fetch::{Budget, Limits};
use extent_remote::store::Store as Bucket;
use extent_store::upload::{self, Remote, Runtime};
use extent_store::{Description, MemDir, Recovering, RemoteData, Store};
use object_store::memory::InMemory;
use rand::rngs::StdRng;
use rand::{RngExt as _, SeedableRng as _};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::time::Duration;

/// How many kept rounds the test runs.
pub const KEPT_ROUNDS: u64 = 100;

/// How many rounds of cut openings the test runs.
pub const CUT_ROUNDS: u64 = 30;

/// What the bucket's URL is to the store's description; the rounds hand the uploader the bucket.
const URL: &str = "memory:///kept";

/// A host's: its stores' uploaders' runtime, and the bucket their branches are kept in, as it
/// reaches it: since its last crash, a copy of what the bucket held then ([`Host::crash`]).
struct Host {
    runtime: tokio::runtime::Runtime,
    bucket: RefCell<Arc<Bucket>>,
    memory: RefCell<Arc<InMemory>>,
}

impl Host {
    fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let memory = Arc::new(InMemory::new());
        let bucket = Arc::new(Bucket::new(memory.clone(), ""));
        Self { runtime, bucket: RefCell::new(bucket), memory: RefCell::new(memory) }
    }

    /// The bucket, as the host reaches it now.
    fn bucket(&self) -> Arc<Bucket> {
        self.bucket.borrow().clone()
    }

    /// The host crashes: what it had going reaches the bucket no more, as a dead host's work does
    /// not, but its writes landed so far. Its stores and uploaders, which go on until dropped,
    /// write to what it reached before; what opens on it from now on reaches a copy of that, as
    /// it is now.
    fn crash(&self) {
        let before = self.memory.borrow().clone();
        let copy = Arc::new(InMemory::new());
        self.runtime.block_on(async {
            use futures::StreamExt as _;
            use object_store::{ObjectStore as _, ObjectStoreExt as _};
            for meta in before.list(None).collect::<Vec<_>>().await {
                let location = meta.unwrap().location;
                let bytes = before.get(&location).await.unwrap().bytes().await.unwrap();
                copy.put(&location, bytes.into()).await.unwrap();
            }
        });
        *self.bucket.borrow_mut() = Arc::new(Bucket::new(copy.clone(), ""));
        *self.memory.borrow_mut() = copy;
    }

    /// The store `recovering` recovered, opened by an uploader uploading every `interval`, as
    /// holder `holder` of its branch's lease.
    fn open(
        &self,
        recovering: Recovering<MemDir>,
        holder: u64,
        interval: Duration,
    ) -> Result<(Arc<Store<MemDir>>, Remote<MemDir>), upload::Failure> {
        let runtime = Runtime {
            handle: self.runtime.handle().clone(),
            uploads: Budget::new(Limits::UNBOUNDED),
        };
        // Checkpoints often, so that the collector's passes find segments recoveries no longer read,
        // which they compact.
        let options =
            upload::Options { interval, checkpoint_every: 4, ..upload::Options::default() };
        upload::open(recovering, OPTIONS, self.bucket(), (holder, options), &runtime)
    }

    /// What `store` holds, its extents whose data is in the bucket read there.
    fn contents(&self, store: &Store<MemDir>) -> Model {
        let mut extents = BTreeMap::new();
        for (extent, len) in store.extents().unwrap() {
            let mut data = vec![0; len as usize];
            if let Err(err) = store.read(extent, 0, &mut data) {
                let RemoteData { location, .. } =
                    err.downcast_ref::<RemoteData>().cloned().unwrap_or_else(|| panic!("{err:#}"));
                let bucket = self.bucket();
                let read = extent_remote::fetch::read(&bucket, extent, &location);
                data = self.runtime.block_on(read).unwrap();
            }
            extents.insert(extent, data);
        }
        Model { extents, root: store.root().unwrap() }
    }
}

/// The collector's passes compact what is mostly dead, and delete it at once.
const COLLECTING: collector::Options = collector::Options { compact_below: 0.9, grace: 0 };

/// The host's bucket, as its collector reaches it: its first so many requests go, and none after,
/// which never complete, as a dead host's do not. A failing request would be no cut: a pass
/// retries what fails for ever.
struct Cut {
    bucket: Arc<Bucket>,
    left: AtomicU64,
}

impl Cut {
    /// Whether the next request goes.
    fn goes(&self) -> bool {
        self.left.try_update(SeqCst, SeqCst, |left| left.checked_sub(1)).is_ok()
    }

    /// Whether it cut a request.
    fn cut(&self) -> bool {
        self.left.load(SeqCst) == 0
    }

    /// `request`, if it goes; one never completing, if not.
    fn request<T>(
        &self,
        request: impl Future<Output = T> + Send,
    ) -> impl Future<Output = T> + Send {
        let goes = self.goes();
        async move {
            if !goes {
                std::future::pending::<()>().await;
            }
            request.await
        }
    }
}

impl extent_remote::Bucket for Cut {
    fn get(&self, key: &str) -> impl Future<Output = Result<Object, BucketError>> + Send {
        self.request(self.bucket.get(key))
    }

    fn get_range(
        &self,
        key: &str,
        range: ByteRange,
    ) -> impl Future<Output = Result<Vec<u8>, BucketError>> + Send {
        self.request(self.bucket.get_range(key, range))
    }

    fn head(&self, key: &str) -> impl Future<Output = Result<Meta, BucketError>> + Send {
        self.request(self.bucket.head(key))
    }

    fn list(&self, prefix: &str) -> impl Future<Output = Result<Vec<Meta>, BucketError>> + Send {
        self.request(self.bucket.list(prefix))
    }

    fn put(
        &self,
        key: &str,
        bytes: Vec<u8>,
        mode: PutMode,
    ) -> impl Future<Output = Result<Version, BucketError>> + Send {
        self.request(self.bucket.put(key, bytes, mode))
    }

    fn delete(&self, key: &str) -> impl Future<Output = Result<(), BucketError>> + Send {
        self.request(self.bucket.delete(key))
    }

    fn now(&self) -> u64 {
        self.bucket.now()
    }

    fn pause(&self, failures: u32) -> impl Future<Output = ()> + Send {
        self.bucket.pause(failures)
    }
}

/// Passes of the collector over `cut`, one every `interval`, on a thread of their own, as the
/// host's collector's, for as long as the host lives: until the pass going is cut, or the host crashes,
/// which aborts them ([`Passes::abort`]).
struct Passes {
    /// What the passes done compacted and deleted.
    done: Arc<(AtomicU64, AtomicU64)>,
    abort: futures::future::AbortHandle,
    thread: std::thread::JoinHandle<Result<Result<(), String>, futures::future::Aborted>>,
}

impl Passes {
    fn start(cut: Arc<Cut>, interval: Duration) -> Self {
        let (abort, registration) = futures::future::AbortHandle::new_pair();
        let done = Arc::<(AtomicU64, AtomicU64)>::default();
        let doing = done.clone();
        // Made on its thread: the passes' future, which the thread alone polls, need not be Send.
        let thread = std::thread::spawn(move || {
            let passes = async move {
                for holder in 100.. {
                    let pass = collector::collect(&*cut, holder, &COLLECTING).await;
                    let report = pass.map_err(|err| format!("pass {holder}: {err}"))?;
                    doing.0.fetch_add(report.compacted as u64, SeqCst);
                    doing.1.fetch_add(report.deleted as u64, SeqCst);
                    // The thread's alone: the passes wait for nothing else on it.
                    std::thread::sleep(interval);
                }
                unreachable!("passes for ever")
            };
            futures::executor::block_on(futures::future::Abortable::new(passes, registration))
        });
        Self { done, abort, thread }
    }

    /// The host dies with its collector: what its pass going did so far stays in the bucket.
    /// Returns how many objects the passes done compacted and deleted; or how a pass failed.
    fn abort(self) -> Result<(u64, u64), String> {
        self.abort.abort();
        if let Ok(failed) = self.thread.join().unwrap() {
            failed?;
        }
        Ok((self.done.0.load(SeqCst), self.done.1.load(SeqCst)))
    }
}

/// What a round's first crash left: the directory, what each commit began with, the last made
/// durable, the last the uploader acknowledged, and the first of the workload's.
struct Crashed {
    dir: MemDir,
    epochs: Vec<Model>,
    durable: usize,
    acknowledged: usize,
    first: usize,
    /// Whether the host's collector was cut in a pass before the crash.
    cut: bool,
    /// How many objects its passes compacted and deleted before.
    collected: (u64, u64),
}

const DESCRIPTION: Description =
    Description { block_size: BLOCK as u32, capacity: 4 << 20, branch: BRANCH };

/// A store made on `host`, kept in its bucket, its uploader uploading every `interval`, crashing
/// in a random workload, as `rng` says, as its file system fails from a random point on.
fn crashed(host: &Host, round: u64, rng: &mut StdRng, interval: Duration) -> Crashed {
    let dir = MemDir::new();
    Store::make(&dir, DESCRIPTION, Some(URL)).unwrap();
    let (store, remote) =
        host.open(Store::recover_unmade(dir.clone()).unwrap(), 1, interval).unwrap();
    store.made().unwrap();
    // The commits opening it made, the branch's capacity set, hold nothing.
    let first = store.epoch().unwrap() as usize;
    let mut epochs = vec![Model::default(); first + 1];
    let cut = Arc::new(Cut { bucket: host.bucket(), left: rng.random_range(0..400).into() });
    let passes = Passes::start(cut.clone(), interval);
    dir.fail_after(Some(rng.random_range(0..600)));
    let (durable, _) = workload(rng, &store, &mut epochs, Model::default(), first);
    let acknowledged = remote.last_uploaded() as usize;
    let dir = dir.crash(round);
    let collected = passes.abort().unwrap_or_else(|err| panic!("round {round}: {err}"));
    host.crash();
    // The host is gone: its uploader, which stops at the store's failure, or uploads what it
    // has left where none sees it, and its store.
    drop(remote);
    drop(store);
    Crashed { dir, epochs, durable, acknowledged, first, cut: cut.cut(), collected }
}

/// Opens the store `crashed` left on `host`, as holder `holder`, and checks it holds a prefix of
/// its commits at least as late as `least`: returns that commit's epoch.
fn reopened(
    host: &Host,
    round: u64,
    crashed: &Crashed,
    dir: MemDir,
    holder: u64,
    least: usize,
) -> usize {
    let recovering = Store::recover(dir)
        .unwrap_or_else(|err| panic!("round {round}: the store doesn't recover: {err:#}"));
    let (store, remote) = host
        .open(recovering, holder, HOUR)
        .unwrap_or_else(|err| panic!("round {round}: the store doesn't open: {err}"));
    let epochs = &crashed.epochs;
    let epoch = store.epoch().unwrap() as usize;
    let Crashed { durable, acknowledged, .. } = crashed;
    assert!(
        (least..epochs.len()).contains(&epoch),
        "round {round}: opened at epoch {epoch}, of {least}..{}, {durable} durable, \
         {acknowledged} acknowledged",
        epochs.len()
    );
    assert_eq!(host.contents(&store), epochs[epoch], "round {round}: at epoch {epoch}");
    drop(remote);
    drop(store);
    epoch
}

/// An uploader's interval that uploads nothing a test does not wait for.
const HOUR: Duration = Duration::from_secs(3600);

/// What a round of the kept rounds found: whether its uploader had acknowledged a commit of the
/// workload's when it crashed, whether the store opened from its disk held commits the bucket
/// lacked, whether its collector was cut in a pass, and whether its passes compacted and deleted
/// objects before the crash.
pub struct Kept {
    pub acknowledged: bool,
    pub ahead: bool,
    pub cut: bool,
    pub compacted: bool,
    pub deleted: bool,
}

/// Round `round` of the kept rounds (see the module's docs).
pub fn kept(round: u64) -> Kept {
    let host = Host::new();
    let mut rng = StdRng::seed_from_u64(round);
    let interval = Duration::from_millis(rng.random_range(1..=20));
    let crashed = crashed(&host, round, &mut rng, interval);
    let Crashed { epochs, acknowledged, durable, first, cut, collected, .. } = &crashed;

    // A pass after the crash, over what it left, completes, as the next host's collector's would.
    let bucket = host.bucket();
    let pass = collector::collect(&*bucket, 1 << 20, &COLLECTING);
    host.runtime.block_on(pass).unwrap_or_else(|err| panic!("round {round}: the pass: {err}"));

    // Its disk lost, a store of the branch's on another host takes it from the bucket.
    let elsewhere = MemDir::new();
    Store::make(&elsewhere, DESCRIPTION, Some(URL)).unwrap();
    let recovering = Store::recover_unmade(elsewhere).unwrap();
    let (taken, remote) = host.open(recovering, 2, interval).unwrap();
    taken.made().unwrap();
    let epoch = taken.epoch().unwrap() as usize;
    assert!(
        (*acknowledged..epochs.len()).contains(&epoch),
        "round {round}: taken at epoch {epoch}, of {acknowledged}..{}",
        epochs.len()
    );
    assert_eq!(host.contents(&taken), epochs[epoch], "round {round}: taken at epoch {epoch}");
    drop(remote);
    drop(taken);

    // Its disk kept, the store opens again: its log follows the branch, which the taking fenced
    // past no commit, and what it made durable it holds.
    let reopened = reopened(&host, round, &crashed, crashed.dir.clone(), 3, (*durable).max(epoch));
    let (compacted, deleted) = (collected.0 > 0, collected.1 > 0);
    Kept {
        acknowledged: acknowledged > first,
        ahead: reopened > epoch,
        cut: *cut,
        compacted,
        deleted,
    }
}

/// Round `round` of the cut openings: the store a kept round's first crash left opened again, its
/// file system failing at each of the opening's writes in turn, the host crashing there; opened
/// again after, it holds a prefix at least as late as its last durable commit, whatever the cut
/// opening wrote: a segment closed again from its log, the WAL's base moved on past what the
/// bucket acknowledged, the files that left for the bucket, a capacity restated. Returns how many
/// writes the opening made.
pub fn cut(round: u64) -> u64 {
    let host = Host::new();
    let mut rng = StdRng::seed_from_u64(round);
    let interval = Duration::from_millis(rng.random_range(1..=20));
    let crashed = crashed(&host, round, &mut rng, interval);
    let mut holder = 10;
    for writes in 0.. {
        // A copy of what the crash left, which the opening cut at its write `writes` changes.
        let dir = crashed.dir.crash(0);
        dir.fail_after(Some(writes));
        let opening = Store::recover(dir.clone()).map_err(upload::Failure::from);
        let opened = opening.and_then(|recovering| host.open(recovering, holder, HOUR));
        let whole = opened.is_ok() && !dir.has_failed();
        drop(opened);
        if whole {
            return writes;
        }
        let epoch =
            reopened(&host, round, &crashed, dir.crash(writes), holder + 1, crashed.durable);
        assert!(epoch >= crashed.acknowledged, "round {round}: cut at {writes}");
        holder += 2;
    }
    unreachable!()
}
