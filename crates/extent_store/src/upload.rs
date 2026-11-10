// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: a store's branch kept in object storage, and its uploader (backlog doc-7, Uploads;
//! doc-14; TASK-28.5, TASK-165.9), which runs on the host's runtime, its uploads within the host's
//! budget ([`Runtime`]). fxdax's `remote` keeps the host's configuration, budgets, runtime and
//! errno mapping.
//!
//! **Opening a store** ([`open`]) is the same whether its branch was here, elsewhere or never
//! anywhere (doc-14, Opening a store): the branch's writer takes its lease, with the token the
//! store's placement names, or the host's, recovers it and fences its end
//! (`extent_remote::Writer`); the store opens as the bucket holds the branch, its write-ahead log
//! put after the branch's end by its commits, what of it landed acknowledged and the rest stamped
//! anew on the writer's chain ([`Recovering::open`]). A log another writer's commits moved past
//! refuses to open (`extent_remote::Error::Diverged`, `ESTALE`) until `fxdaxctl take` drops it;
//! one that plainly diverged before the lease is taken refuses before, fencing no writer. A branch
//! with no record is made, a root (`extent_remote::branch::create`).
//!
//! **A point** takes no lease (doc-7, Points): its store holds its state, recovered, which nothing
//! writes, and no log; nothing it does goes to the bucket but its forks' records
//! ([`Remote::forked`], `extent_remote::branch::fork_point`). Any number of hosts hold one at once.
//!
//! **The uploader** hands the store's write-ahead log's segments, framed, whose commits are durable
//! ([`Store::closed`]), to the writer (`Writer::add_segment`), each the object it is in the bucket,
//! or stamped anew after a takeover; uploads them, [`IN_FLIGHT`] at once at most, each read only as
//! its upload goes, within the host's budget; and acknowledges them in order once a read of the
//! lease shows them the branch's ([`Store::acknowledged`]): their extents are the bucket's from then
//! on, their files the host's cache's. The store closes its open segment for the uploader when a
//! commit waits for its upload, at a fork, as its interval runs out, and as the uploader stops:
//! dropping it uploads what the store made durable, its open segment's commits too (TASK-192).
//! Commits waiting to be durable (`COMMIT_DURABLE`) complete once acknowledged.
//!
//! A fork the store makes is a fork of the branch once the segment its commit ends is
//! acknowledged, which gives the child its record, and checkpointed ([`Remote::forked`]): the
//! child's store is made only then, opening it recovering the child from the bucket. A fork that
//! fails before is deleted in the bucket, as the fork's alone ([`Remote::unfork`]).
//!
//! A writer displaced, an earlier writer's segment found where its next goes, landed late past its
//! fence (`extent_remote::Error::Displaced`), has the uploader take the branch again, the guest
//! kept (doc-7, Fencing; TASK-204): a writer of a lease of its own, the store's write-ahead log put
//! after the branch's end ([`Store::rebase`]), as opening the store does. Every other failure of
//! its writer's stops the uploader for good, and the commits waiting for uploads fail with it from
//! then on: its writer fenced, by a later writer, its branch deleted, or the log diverged, another
//! writer's commits past it. Only opening the store anew takes the branch again then.

use crate::{Dir, DirHold, File, Opening, Recovering, Store, read_object};
use event_listener::{Event, Listener as _};
use extent_remote::cache::Cache;
use extent_remote::fetch::Budget;
use extent_remote::layout::{Chain, Child, Origin, epoch_of};
use extent_remote::record::{Kind, Record};
use extent_remote::store::Store as Bucket;
use extent_remote::wal::Fenced;
use extent_remote::{Body, Objects, Writer, branch};
use futures::StreamExt as _;
use futures::future::{BoxFuture, Either, FutureExt as _};
use futures::stream::FuturesOrdered;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// How many of a store's segments upload at once: as many of a segment's size as the host's
/// uploads may hold at once ([`UPLOAD_BYTES`]), so that a store catching up uploads as many at once
/// as the host's budget lets it, about ten keeping 1 GB/s going with PUTs of 0.35 s, and holds no
/// more of its segments' files open for them (TASK-79.19).
pub const IN_FLIGHT: usize = (UPLOAD_BYTES / SEGMENT) as usize;

/// About the most bytes of a segment: a store's segments' size
/// (`extent_store::Options::segment_size`).
const SEGMENT: u64 = 32 << 20;

/// How many segments go between checkpoints, by default ([`Options::checkpoint_every`]).
pub const CHECKPOINT_EVERY: u64 = 64;

/// The most bytes of objects a host's uploads hold at once: its budget's ([`Runtime::uploads`]).
pub const UPLOAD_BYTES: u64 = 512 << 20;

/// A step of the uploader's, which the failpoint of its name fails (the `fail` crate's): the
/// function it is in returns a [`Failure`] there, or whatever a test set the failpoint to do
/// happens.
macro_rules! failing {
    ($name:literal) => {
        fail::fail_point!($name, |_| Err(Failure::msg(concat!("failpoint ", $name))))
    };
}

/// What a host gives its stores' uploaders: the runtime they run on, sized by the host, not by
/// how many stores it serves, as each waits on object storage mostly, and does its disk work in
/// `block_in_place`, which leaves the runtime's threads to the others (TASK-34); and the budget of
/// the PUTs of objects they upload, each holding its object's bytes while it goes, bounded in
/// bytes, [`UPLOAD_BYTES`], so that a host whose stores are far behind holds no more than these
/// at once, whatever their number (TASK-79.19), and in number.
#[derive(Clone)]
pub struct Runtime {
    pub handle: tokio::runtime::Handle,
    pub uploads: Arc<Budget>,
}

/// How many stores' uploaders run in this process ([`open`]): until each, its store dropped, has
/// uploaded what its store made durable, and stopped.
static UPLOADERS: std::sync::LazyLock<tokio::sync::watch::Sender<usize>> =
    std::sync::LazyLock::new(|| tokio::sync::watch::channel(0).0);

/// How many stores' uploaders run in this process, as it changes: what an extent daemon's drain
/// waits to see none of (fxdax TASK-141.4).
pub fn uploaders() -> tokio::sync::watch::Receiver<usize> {
    UPLOADERS.subscribe()
}

/// Runs `future` to its end on a runtime of its own, as what is called from no runtime's task
/// does.
pub fn block_on<F: std::future::Future>(future: F) -> Result<F::Output, Failure> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| Failure::local(err, "a runtime"))?;
    Ok(runtime.block_on(future))
}

/// Deletes `branch` in `bucket` (`extent_remote::branch::delete`), as the placement whose fencing
/// token is `token`: its writer stops at its next acknowledgement, no writer takes it again, and
/// the collector reclaims what no other branch inherited from it, as its forks' branches live on
/// (TASK-39.4). Deleting it again does nothing. A later placement's branch, whose token is higher,
/// is not deleted (`extent_remote::Error::Stale`, TASK-45.1).
pub fn delete_branch(bucket: &Bucket, branch: u64, token: u64) -> Result<(), Failure> {
    Ok(block_on(branch::delete(bucket, branch, rand::random(), token))??)
}

/// Why uploading stopped, or a branch could not be uploaded to, or opened: the error as it was,
/// typed, which fxdax's `remote` maps to its errno, in the one place.
#[derive(Clone, Debug, thiserror::Error)]
pub enum Failure {
    /// What object storage's protocol found of the branch, as `extent_remote` says it: another
    /// writer took it (`Fenced`, `Displaced`), it was deleted, a later placement holds it
    /// (`Stale`, TASK-39.7), another writer's commits moved it on from the store's write-ahead log
    /// (`Diverged`), another branch has a fork's child's ID (`ChildTaken`, TASK-79.82), or what it
    /// needs is gone or corrupt.
    #[error(transparent)]
    Remote(#[from] extent_remote::Error),
    /// The point a store is of was deleted: it forks no more, and its forks live on.
    #[error("the point was deleted")]
    PointDeleted,
    /// This host's own: its files, a runtime, the uploader gone, as the error says.
    #[error("{0:#}")]
    Local(Arc<anyhow::Error>),
}

impl Failure {
    /// This host's failure, saying `message`.
    pub fn msg(message: impl std::fmt::Display + std::fmt::Debug + Send + Sync + 'static) -> Self {
        Self::Local(Arc::new(anyhow::Error::msg(message)))
    }

    /// This host's failure of `what`, with `error`.
    pub fn local(
        error: impl std::error::Error + Send + Sync + 'static,
        what: impl std::fmt::Display + Send + Sync + 'static,
    ) -> Self {
        Self::Local(Arc::new(anyhow::Error::new(error).context(what)))
    }
}

/// Failures are the same as they say, this host's own as their errors' messages do.
impl PartialEq for Failure {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Remote(error), Self::Remote(other)) => error == other,
            (Self::PointDeleted, Self::PointDeleted) => true,
            (Self::Local(error), Self::Local(other)) => {
                format!("{error:#}") == format!("{other:#}")
            }
            (Self::Remote(_) | Self::PointDeleted | Self::Local(_), _) => false,
        }
    }
}

impl Eq for Failure {}

/// The store's errors are this host's, but what object storage's protocol found of the branch, as
/// a write-ahead log diverged from it.
impl From<anyhow::Error> for Failure {
    fn from(error: anyhow::Error) -> Self {
        match error.downcast::<extent_remote::Error>() {
            Ok(error) => Self::Remote(error),
            Err(error) => Self::Local(Arc::new(error)),
        }
    }
}

/// How an uploader uploads.
#[derive(Clone, Debug)]
pub struct Options {
    /// How long durable commits wait at most before they are uploaded.
    pub interval: Duration,
    /// How many commits may wait to be uploaded: the next waits, while object storage is out of
    /// reach, rather than the lag grow without bound (doc-7, decision 3).
    pub max_lag: u64,
    /// How many bytes of the store's write-ahead log may wait to be acknowledged, likewise: what
    /// the local disk holds until uploaded.
    pub max_lag_bytes: u64,
    /// The host's cache of the bucket's objects, which its stores share: what the bucket
    /// acknowledged of a store's goes into it (TASK-165.8).
    pub cache: Option<Arc<Cache>>,
    /// The fencing token of a store never placed: its host's placement's.
    pub token: u64,
    /// fxdax: how many segments go between checkpoints, which recoveries start from: the segments
    /// a checkpoint covers are no longer read by them, and the collector may compact them.
    pub checkpoint_every: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(1),
            max_lag: 4096,
            max_lag_bytes: 1 << 30,
            cache: None,
            token: 0,
            checkpoint_every: CHECKPOINT_EVERY,
        }
    }
}

/// Opens the store `recovering` recovered, keeping its branch in `bucket` (see the module's
/// docs): as `holder` of the branch's lease, which must differ from every other taking's, with
/// `store_options`, its uploader uploading as `options` say, on `runtime`. Returns the store and
/// its uploader; or why it can't be opened: its log diverged, its branch deleted, a later
/// placement's, or out of reach.
pub fn open<D: Dir>(
    recovering: Recovering<D>,
    store_options: crate::Options,
    bucket: Arc<Bucket>,
    (holder, options): (u64, Options),
    runtime: &Runtime,
) -> Result<(Arc<Store<D>>, Remote<D>), Failure> {
    let sleep: Clock = Arc::new(|interval| tokio::time::sleep(interval).boxed());
    let shared = Shared::new(options, runtime.uploads.clone(), sleep);
    let (started, start) = std::sync::mpsc::channel();
    let (stop, stopped) = std::sync::mpsc::channel();
    let (uploader, uploads) = (shared.clone(), bucket.clone());
    UPLOADERS.send_modify(|running| *running += 1);
    // A task, not a thread of its own (TASK-34).
    runtime.handle.spawn(async move {
        // Its conditional writes checked before any is relied on (doc-7, Fencing).
        match uploads.probed(holder).await {
            Ok(()) => uploader.run(recovering, store_options, uploads, holder, started).await,
            Err(err) => started.send(Err(Failure::msg(err))).unwrap(),
        }
        // The store let go of before the uploader says it stopped: so that dropping this returns
        // with nothing of the uploader's holding the store open, which the process may open again
        // at once (`Dir::claim` refusing a second opening), as extentd does a store an image it
        // imported made (TASK-165.6).
        drop(uploader);
        UPLOADERS.send_modify(|running| *running -= 1);
        let _ = stop.send(());
        fail::fail_point!("remote::stopped");
    });
    let opened = match start.recv() {
        Ok(Ok(store)) => store,
        Ok(Err(err)) => {
            let _ = stopped.recv();
            return Err(err);
        }
        Err(_) => return Err(Failure::msg("the uploader stopped")),
    };
    let dir = opened.hold_dir()?;
    let remote = Remote { shared, bucket, stopped: Mutex::new(Some(stopped)), _dir: dir };
    Ok((opened, remote))
}

/// A store's uploader (see the module's docs). Dropping it closes the store's open segment, if its
/// store is open still, uploads what the store made durable, then stops.
pub struct Remote<D: Dir> {
    shared: Arc<Shared<D>>,
    /// The bucket the store's branch is kept in.
    bucket: Arc<Bucket>,
    /// Says the uploader stopped, once it has.
    stopped: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    /// The store's directory, held taken for this process until the uploader stopped, last of its
    /// fields to go (TASK-129).
    _dir: DirHold<D>,
}

pub(crate) struct Shared<D: Dir, B: extent_remote::Bucket = Bucket> {
    /// The store, once opened.
    store: std::sync::OnceLock<Arc<Store<D>>>,
    /// The branch's writer, once it took the branch, and as it takes it again (TASK-204).
    writer: Mutex<Option<Arc<Writer<B>>>>,
    /// The fencing token its writers take the branch's lease with, once one did.
    token: std::sync::OnceLock<u64>,
    /// The point the store is of, if it is one.
    point: std::sync::OnceLock<Point>,
    /// The origins of the forks of the segments the uploader took, by their children: what a fork
    /// that fails deletes its child by (TASK-165.7).
    forks: Mutex<std::collections::BTreeMap<u64, Origin>>,
    progress: Mutex<Progress>,
    /// Notified as the progress changes: what waits for uploads listens to it, blocking or not
    /// (TASK-43.6).
    changed: Event,
    /// Wakes the uploader: a commit waits, or it is to stop.
    wake: tokio::sync::Notify,
    options: Options,
    /// The host's budget of uploads, which its writer uploads within.
    uploads: Arc<Budget>,
    /// When its interval is up ([`Clock`]).
    clock: Clock,
}

/// What an uploader's interval runs out on: a sleep of the host's runtime's, or, in the
/// simulation, of its scheduler's, which says when each runs out (TASK-88).
pub(crate) type Clock = Arc<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync>;

/// Runs `work`, which blocks on the store's disk, on the uploaders' runtime: on its thread, which
/// hands its other tasks to another meanwhile; or as it is, on a thread no runtime runs, as the
/// simulation's (`tokio::task::block_in_place` runs it so there).
#[allow(clippy::disallowed_methods, reason = "the work waits on the disk alone")]
fn blocking<T>(work: impl FnOnce() -> T) -> T {
    tokio::task::block_in_place(work)
}

/// A point a store holds: its branch, and what its record names.
struct Point {
    branch: u64,
    origin: Origin,
}

/// fxdax: what a commit stalled behind object storage waits for, said as it stalls: the bytes not
/// uploaded, and of them those past the lag bound, which a stall over the bound waits on alone,
/// the commits not uploaded and the oldest's age, which a slow upload shows (TASK-92.52).
fn stalled(pending: u64, max: u64, commits: u64, oldest: Option<Duration>) -> String {
    let oldest = oldest.map_or("none seen yet".to_owned(), |age| {
        format!("the oldest seen {} ms ago", age.as_millis())
    });
    format!(
        "fxdax: commits wait for object storage: {pending} bytes not uploaded, {} past the lag \
         bound of {max}; {commits} commits not uploaded, {oldest}",
        pending.saturating_sub(max)
    )
}

#[derive(Default)]
struct Progress {
    /// The last epoch uploaded.
    uploaded: u64,
    /// The last epoch uploaded whose forks are made in object storage: their children have their
    /// records, and a checkpoint at them, which their recovery starts from.
    forked: u64,
    /// Why uploading stopped, if it did.
    failed: Option<Failure>,
    /// Whether a commit waits to be uploaded: the next batch goes now.
    wanted: bool,
    /// How many commits waited for the lag to shrink.
    stalls: u64,
    /// The lowest sequence number the guest may mint: its writer's epoch's range (doc-7).
    next_sequence: u64,
    stop: bool,
    /// Whether, stopping, it stops at once: uploading nothing more, its uploads in flight given up
    /// on, no checkpoint written ([`Remote::cut_short`]).
    cut: bool,
    /// The branch's lease, once its writer took it.
    lease: Option<Lease>,
    /// The store's latest commits not uploaded, as the uploader saw them, and when it did: the
    /// first is when the oldest commit waiting was seen first (TASK-50.5).
    seen: VecDeque<(u64, SystemTime)>,
}

/// A branch's lease, as its writer took it: with its placement's fencing token, of the epoch, and
/// when.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lease {
    /// The fencing token of the placement that took it.
    pub token: u64,
    /// Its epoch: one more than the lease's before.
    pub epoch: u64,
    /// When the writer took it.
    pub taken: SystemTime,
}

/// What a store's uploader has done, and has left to do (TASK-50.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadStatus {
    /// Its branch's lease, once taken.
    pub lease: Option<Lease>,
    /// The epoch of the store's latest commit uploaded.
    pub uploaded: u64,
    /// The bytes of its write-ahead log not yet acknowledged.
    pub pending_bytes: u64,
    /// When the uploader first saw the oldest commit not yet uploaded, if one is not: within its
    /// interval of when the commit was made.
    pub oldest_pending: Option<SystemTime>,
    /// How many commits waited for the lag to shrink ([`Remote::stalls`]).
    pub stalls: u64,
}

impl Progress {
    /// Notes the store's commits through `epoch` uploaded.
    fn uploaded_through(&mut self, epoch: u64) {
        self.uploaded = self.uploaded.max(epoch);
        while self.seen.front().is_some_and(|(seen, _)| *seen <= self.uploaded) {
            self.seen.pop_front();
        }
    }

    /// Notes the forks of the store's commits through `epoch`, uploaded, made in object storage.
    fn forked_through(&mut self, epoch: u64) {
        self.forked = self.forked.max(epoch);
    }
}

impl<D: Dir> Remote<D> {
    /// The store.
    fn store(&self) -> &Arc<Store<D>> {
        self.shared.store()
    }

    /// Waits until the commits up to `epoch` are uploaded and acknowledged.
    pub fn wait_uploaded(&self, epoch: u64) -> Result<(), Failure> {
        let shared = &self.shared;
        if shared.ask(epoch, false)? {
            return Ok(());
        }
        shared.wait(|progress| shared.reached(progress, epoch, false))
    }

    /// Returns once the commits up to `epoch` are uploaded and acknowledged, as
    /// [`Remote::wait_uploaded`] does: a future, which holds no thread while it waits, as for a
    /// sync that object storage keeps waiting (TASK-43.6).
    pub fn uploaded(
        &self,
        epoch: u64,
    ) -> impl std::future::Future<Output = Result<(), Failure>> + Send + use<D> {
        let shared = self.shared.clone();
        async move {
            if shared.ask(epoch, false)? {
                return Ok(());
            }
            loop {
                let changed = {
                    let progress = shared.progress.lock().unwrap();
                    if let Some(reached) = shared.reached(&progress, epoch, false) {
                        return reached;
                    }
                    shared.changed.listen()
                };
                changed.await;
            }
        }
    }

    /// The epoch of the store's latest commit uploaded and acknowledged, as the uploader says,
    /// whether or not it stopped.
    pub fn last_uploaded(&self) -> u64 {
        self.shared.last_uploaded()
    }

    /// What the uploader has done, and has left to do.
    pub fn status(&self) -> Result<UploadStatus, Failure> {
        let shared = &self.shared;
        shared.observe()?;
        let pending_bytes = self.store().wal_bytes()?;
        let progress = shared.progress.lock().unwrap();
        Ok(UploadStatus {
            lease: progress.lease,
            uploaded: progress.uploaded,
            pending_bytes,
            oldest_pending: progress.seen.front().map(|(_, seen)| *seen),
            stalls: progress.stalls,
        })
    }

    /// Waits, if more commits wait to be uploaded than `Options::max_lag`, with `epoch`'s, or the
    /// write-ahead log holds more than `Options::max_lag_bytes`, until they no longer do.
    pub fn wait_lag(&self, epoch: u64) -> Result<(), Failure> {
        let (shared, epoch) = (&self.shared, epoch.saturating_sub(self.shared.options.max_lag));
        if shared.ask(epoch, true)? {
            return Ok(());
        }
        shared.wait(|progress| shared.reached(progress, epoch, true))
    }

    /// Makes the fork the store made of its branch with its commit `epoch`
    /// ([`Store::commit_fork`]) a fork of the branch in object storage, once the commits up to the
    /// fork are uploaded: the segment the fork's commit ends names `child`, whose record the
    /// acknowledgement makes, naming the branch and the segment as its origin, and a checkpoint at
    /// the fork is where its recovery starts. `child`'s ID may be another branch's by then, which
    /// took it after [`Remote::child_free`] found it free (`extent_remote::Error::ChildTaken`): the
    /// fork failed, and the uploader goes on.
    ///
    /// A fork of a point commits nothing: the child's record is the fork, and fails if the point
    /// was deleted ([`Failure::PointDeleted`]; `branch::fork_point`).
    pub fn forked(&self, epoch: u64, child: Child) -> Result<(), Failure> {
        failing!("remote::forked");
        let shared = &self.shared;
        if let Some(point) = shared.point.get() {
            let (of, origin) = (point.branch, point.origin);
            let forking = branch::fork_point(&*self.bucket, child.branch, of, origin, child.owner);
            return match block_on(forking)? {
                Err(extent_remote::Error::Deleted) => Err(Failure::PointDeleted),
                forked => Ok(forked?),
            };
        }
        let child = child.branch;
        // Its commit's segment uploads now, if it is not yet uploaded.
        shared.ask(epoch, false)?;
        shared.wait(|progress| match progress.forked >= epoch {
            true => Some(Ok(())),
            false => progress.failed.clone().map(Err),
        })?;
        match shared.writer() {
            Some(writer) if writer.refused(child) => {
                Err(extent_remote::Error::ChildTaken(child).into())
            }
            _ => Ok(()),
        }
    }

    /// Deletes `child` in object storage, a fork the store made that [`Remote::forked`] failed to
    /// make: its segment may land after all, its writer taken over before acknowledging it, and the
    /// branch's next writer gives the child a record then, which would keep what it inherited for
    /// ever, no store holding it. Deleted first, it gets a record saying so, which the segment
    /// landing finds (TASK-79.78). Only a branch the fork made is deleted, its record naming the
    /// fork's owner: one of another's, which took the child's ID, is left as it is (TASK-79.82). As
    /// the fork's segment gave the child its origin, if the uploader took it; or of no parent, if
    /// it never did. It is deleted as the host's placement (`Options::token`).
    pub fn unfork(&self, child: Child) -> Result<(), Failure> {
        let origin = self.shared.fork_origin(child.branch);
        let forked = match (origin, child.point) {
            (None, _) => Kind::Own,
            (Some(origin), true) => Kind::Point(origin),
            (Some(origin), false) => Kind::Fork(origin),
        };
        let (holder, token) = (rand::random(), self.shared.options.token);
        let deleting =
            branch::delete_fork(&*self.bucket, child.branch, forked, child.owner, holder, token);
        if !block_on(deleting)?? {
            log::warn!(
                "fxdax: leaving branch {:x} in object storage, another's: a fork to its ID failed \
                 here",
                child.branch
            );
        }
        Ok(())
    }

    /// Whether `child`'s ID is free in object storage for a fork the store is making of its
    /// branch, before the fork's commit: unless another branch has it, as a guest may give any,
    /// whose record its parent's writers, finding it another's, would fail on for ever. One GET of
    /// the child's record.
    pub fn child_free(&self, child: u64) -> Result<bool, Failure> {
        let found = block_on(branch::record(&*self.bucket, child))??;
        failing!("remote::child_free");
        Ok(found.is_none())
    }

    /// Whether the store holds a point: nothing it does goes to object storage but its forks'
    /// records.
    pub fn is_point(&self) -> bool {
        self.shared.point.get().is_some()
    }

    /// The lowest sequence number the store's guest may mint extents' IDs from, which the writer's
    /// epoch gives: no writer of the branch before minted from there.
    pub fn next_sequence(&self) -> u64 {
        self.shared.next_sequence()
    }

    /// The bucket the store's branch is kept in.
    pub fn bucket(&self) -> &Bucket {
        &self.bucket
    }

    /// How many commits waited for the lag to shrink.
    pub fn stalls(&self) -> u64 {
        self.shared.progress.lock().unwrap().stalls
    }

    /// Why uploading stopped, if it did: the store's commits are its branch's no more.
    pub fn failed(&self) -> Option<Failure> {
        self.shared.failed()
    }

    /// What has the uploader, once it stops, or as it is stopping, stop at once (TASK-216): as a
    /// writer opening the store again here waits for it, which the store's write-ahead log keeps
    /// what it hadn't uploaded for, and whose opening uploads it. Stopped so, its uploads in flight
    /// are given up on, as a crashed host's are, landing or not, and its writer writes no
    /// checkpoint. Its uploader gone, it does nothing. Stopping as dropping it does otherwise, it
    /// uploads all its store made durable first, which takes as long as the bucket takes.
    pub fn cut_short(&self) -> impl Fn() + Send + Sync + 'static {
        let shared = Arc::downgrade(&self.shared);
        move || {
            let Some(shared) = shared.upgrade() else { return };
            shared.progress.lock().unwrap().cut = true;
            shared.wake.notify_one();
        }
    }

    /// Has the uploader, stopping, upload all its store made durable first, as it does unless
    /// cut short ([`Remote::cut_short`]): for a store another writer took over, as it was, its
    /// uploader going on, whose going later no writer waits for.
    pub fn drain_on_stop(&self) {
        self.shared.progress.lock().unwrap().cut = false;
    }
}

impl<D: Dir> Drop for Remote<D> {
    fn drop(&mut self) {
        self.shared.stop();
        if let Some(stopped) = self.stopped.get_mut().unwrap().take() {
            let _ = stopped.recv();
        }
    }
}

impl<D: Dir, B: extent_remote::Bucket> Shared<D, B> {
    /// An uploader that uploads as `options` say, within `uploads`, its interval running out on
    /// `clock`: started by [`Shared::run`].
    pub(crate) fn new(options: Options, uploads: Arc<Budget>, clock: Clock) -> Arc<Self> {
        Arc::new(Self {
            store: std::sync::OnceLock::new(),
            writer: Default::default(),
            token: Default::default(),
            point: Default::default(),
            forks: Default::default(),
            progress: Mutex::new(Progress::default()),
            changed: Event::new(),
            wake: tokio::sync::Notify::new(),
            options,
            uploads,
            clock,
        })
    }

    /// The epoch of the store's latest commit uploaded and acknowledged.
    pub(crate) fn last_uploaded(&self) -> u64 {
        self.progress.lock().unwrap().uploaded
    }

    /// The epoch of the store's latest commit whose forks are made, their children's records
    /// and the checkpoint at them in the bucket.
    #[cfg(all(test, feature = "shuttle"))]
    pub(crate) fn forked(&self) -> u64 {
        self.progress.lock().unwrap().forked
    }

    /// The lowest sequence number the store's guest may mint extents' IDs from.
    pub(crate) fn next_sequence(&self) -> u64 {
        self.progress.lock().unwrap().next_sequence
    }

    /// Why uploading stopped, if it did.
    pub(crate) fn failed(&self) -> Option<Failure> {
        self.progress.lock().unwrap().failed.clone()
    }

    /// The branch's writer, once one took it.
    pub(crate) fn writer(&self) -> Option<Arc<Writer<B>>> {
        self.writer.lock().unwrap().clone()
    }

    /// Where the fork to `child` is in the branch, once the uploader took the segment its commit
    /// ends.
    pub(crate) fn fork_origin(&self, child: u64) -> Option<Origin> {
        self.forks.lock().unwrap().get(&child).copied()
    }

    /// Has the uploader stop: once it uploaded what its store made durable, its open segment's
    /// commits too, unless cut short ([`Remote::cut_short`]).
    pub(crate) fn stop(&self) {
        self.progress.lock().unwrap().stop = true;
        self.changed.notify(usize::MAX);
        self.wake.notify_one();
    }

    /// Waits until the uploader is woken, or `interval` runs out first: whether it was woken.
    async fn woken(&self, interval: Duration) -> bool {
        let notified = std::pin::pin!(self.wake.notified());
        let ran_out = (self.clock)(interval);
        matches!(futures::future::select(notified, ran_out).await, Either::Left(_))
    }

    fn store(&self) -> &Arc<Store<D>> {
        self.store.get().expect("the uploader opened its store")
    }

    /// Notes the store's latest commit, if it isn't uploaded, as seen now.
    fn observe(&self) -> Result<(), Failure> {
        let epoch = self.store().epoch()?;
        let mut progress = self.progress.lock().unwrap();
        let newer = progress.seen.back().is_none_or(|(seen, _)| *seen < epoch);
        if epoch > progress.uploaded && newer {
            progress.seen.push_back((epoch, SystemTime::now()));
        }
        Ok(())
    }

    /// Whether the commits up to `epoch` are uploaded and acknowledged, and, if `stall`, the bytes
    /// waiting within `Options::max_lag_bytes`: if not, asks the uploader for them, counting a
    /// stall if `stall`.
    pub(crate) fn ask(&self, epoch: u64, stall: bool) -> Result<bool, Failure> {
        self.observe()?;
        let mut progress = self.progress.lock().unwrap();
        if self.within(progress.uploaded, epoch, stall)? {
            return Ok(true);
        }
        progress.wanted = true;
        if stall {
            progress.stalls += 1;
            let pending = self.store().wal_bytes()?;
            let commits = epoch.saturating_sub(progress.uploaded);
            let oldest = progress.seen.front().and_then(|(_, seen)| seen.elapsed().ok());
            let max = self.options.max_lag_bytes;
            log::warn!("{}", stalled(pending, max, commits, oldest));
        }
        drop(progress);
        self.wake.notify_one();
        Ok(false)
    }

    /// What waiting for `epoch` as [`Shared::ask`] says finds in `progress`: done, or failed, or,
    /// if nothing yet, `None`.
    fn reached(&self, progress: &Progress, epoch: u64, stall: bool) -> Option<Result<(), Failure>> {
        match self.within(progress.uploaded, epoch, stall) {
            Ok(true) => Some(Ok(())),
            Ok(false) => progress.failed.clone().map(Err),
            Err(err) => Some(Err(err)),
        }
    }

    /// Whether, `uploaded` up to, the commits up to `epoch` are, and, if `stall`, the bytes of the
    /// write-ahead log within `Options::max_lag_bytes`: bytes only bound how far commits go ahead,
    /// not when one is durable.
    fn within(&self, uploaded: u64, epoch: u64, stall: bool) -> Result<bool, Failure> {
        let max_bytes = self.options.max_lag_bytes;
        Ok(uploaded >= epoch && (!stall || self.store().wal_bytes()? <= max_bytes))
    }

    /// Waits, blocking, until `reached` finds in the progress what it waits for, done or failed.
    fn wait(
        &self,
        reached: impl Fn(&Progress) -> Option<Result<(), Failure>>,
    ) -> Result<(), Failure> {
        loop {
            let changed = {
                let progress = self.progress.lock().unwrap();
                if let Some(reached) = reached(&progress) {
                    return reached;
                }
                self.changed.listen()
            };
            changed.wait();
        }
    }

    pub(crate) async fn run(
        &self,
        recovering: Recovering<D>,
        options: crate::Options,
        bucket: Arc<B>,
        holder: u64,
        started: std::sync::mpsc::Sender<Result<Arc<Store<D>>, Failure>>,
    ) {
        let interval = self.options.interval;
        let writer = match self.start(recovering, options, &bucket, holder).await {
            Ok(writer) => {
                started.send(Ok(self.store().clone())).unwrap();
                writer
            }
            Err(err) => return started.send(Err(err)).unwrap(),
        };
        // A point's store has nothing to upload.
        let Some(mut writer) = writer else {
            while !self.progress.lock().unwrap().stop {
                self.wake.notified().await;
            }
            return;
        };
        let mut segments = 0;
        let mut in_flight = InFlight::default();
        // Whether the next gather closes the store's open segment: for a commit waiting for its
        // upload, or a fork, or as the interval is up, or the uploader stops; and whether the last
        // took segments, after which the store may hold more closed, which the next takes,
        // closing none for them (TASK-92.15).
        let (mut close, mut more) = (false, false);
        loop {
            if let Err(err) = self.observe() {
                return self.fail(&writer, err);
            }
            let (stop, cut) = {
                let mut progress = self.progress.lock().unwrap();
                close |= std::mem::take(&mut progress.wanted);
                (progress.stop, progress.cut)
            };
            // Cut short (TASK-216): what it has in flight, and what it would have taken, the
            // store's log keeps, for the store's next opening.
            if stop && cut {
                return self.fail(&writer, Failure::msg("the uploader stopped, cut short"));
            }
            if (close || more || stop) && in_flight.len() < IN_FLIGHT {
                let gathered = blocking(|| self.gather(&writer, &mut in_flight, close || stop));
                let gathered = match gathered {
                    Ok(gathered) => gathered,
                    Err(err) => match self.after(err, &bucket).await {
                        Ok(taken) => {
                            (writer, in_flight, more) = (taken, InFlight::default(), true);
                            continue;
                        }
                        Err(err) => return self.fail(&writer, err),
                    },
                };
                // Done with once closed; the segments the store had closed taking the room, it
                // closes the next time.
                close &= !gathered.closed;
                more = gathered.took;
                if more {
                    continue;
                }
                if stop && in_flight.is_empty() {
                    // A checkpoint on clean shutdown (doc-7, Checkpoints).
                    if let Err(err) = writer.checkpoint().await {
                        log::warn!("fxdax: checkpointing branch {:x}: {err}", writer.branch());
                    }
                    return self.fail(&writer, Failure::msg("the uploader stopped"));
                }
            }
            let woken = self.woken(interval);
            if in_flight.is_empty() {
                close |= !woken.await;
                continue;
            }
            let uploaded =
                match futures::future::select(in_flight.next(), std::pin::pin!(woken)).await {
                    Either::Left((uploaded, _)) => uploaded.expect("an upload in flight"),
                    Either::Right((woken, _)) => {
                        close |= !woken;
                        continue;
                    }
                };
            let acked = match uploaded {
                Ok(()) => self.acknowledged(&writer, &mut in_flight).await,
                Err(err) => Err(err.into()),
            };
            if let Err(err) = acked {
                match self.after(err, &bucket).await {
                    Ok(taken) => {
                        (writer, in_flight, more) = (taken, InFlight::default(), true);
                        continue;
                    }
                    Err(err) => return self.fail(&writer, err),
                }
            }
            segments += 1;
            if segments % self.options.checkpoint_every == 0
                && let Err(err) = writer.checkpoint().await
            {
                log::warn!("fxdax: checkpointing branch {:x}: {err}", writer.branch());
            }
        }
    }

    /// Stops uploading, for `err`: commits waiting for uploads, and forks, fail with it.
    fn fail(&self, writer: &Writer<B>, err: Failure) {
        let mut progress = self.progress.lock().unwrap();
        if !progress.stop {
            log::error!("fxdax: uploading branch {:x}: {err}", writer.branch());
        }
        progress.failed = Some(err);
        self.changed.notify(usize::MAX);
    }

    /// Opens the store `recovering` recovered, as the bucket holds its branch (see the module's
    /// docs), and returns its writer, none for a point. Its log is checked to follow the branch as
    /// recovery finds it before the lease is taken, so that a store plainly stale fences no writer;
    /// then, the lease taken, the writer recovers it on from there (TASK-92.6) and the store opens
    /// as the writer found its end, which another writer may have moved on meanwhile.
    async fn start(
        &self,
        recovering: Recovering<D>,
        options: crate::Options,
        bucket: &Arc<B>,
        holder: u64,
    ) -> Result<Option<Arc<Writer<B>>>, Failure> {
        let branch = recovering.branch();
        let cache = self.options.cache.clone();
        // The record, and the lease the writer takes after recovering the branch, at once: the
        // taking goes on from this read of it, a round trip less (TASK-165.11).
        let lease = extent_remote::layout::lease(branch);
        let leased = branch::read_lease(&**bucket, &lease);
        let (found, lease) = futures::join!(record(&**bucket, branch), leased);
        let found = match found? {
            // A branch the bucket acknowledged commits of, which it no longer has: deleted, or
            // another bucket. Making it anew would lose them.
            None if recovering.base().is_some() => {
                return Err(extent_remote::Error::NoBranch.into());
            }
            None => {
                failing!("remote::create_branch");
                branch::create(&**bucket, branch, rand::random()).await?;
                None
            }
            Some(record) if record.is_point() => {
                let state = self.point(bucket, branch, record).await?;
                let opening = Opening { state, fenced: None, cache };
                let store = blocking(|| recovering.open(options, opening))?;
                let _ = self.store.set(Arc::new(store));
                let mut progress = self.progress.lock().unwrap();
                progress.uploaded_through(u64::MAX);
                progress.forked_through(u64::MAX);
                return Ok(None);
            }
            Some(record) => {
                let recovered = extent_remote::recover(&**bucket, branch, record.origin()).await?;
                let last = recovered.state.last_commit;
                // Where the writer's chain would go: the log's restamping, not looked at here.
                let chain = Chain::new(branch, 0, recovered.end, recovered.state.last_segment, 0);
                let found = Fenced { recovered: last, last_commit: last, chain };
                if !recovering.follows(&found) {
                    return Err(extent_remote::Error::Diverged.into());
                }
                Some((record, recovered))
            }
        };
        let token = recovering.token().unwrap_or(self.options.token);
        // Its state, once it holds the lease, recovered on from what was found before.
        let writer = match found {
            None => Writer::open(bucket.clone(), branch, holder, token).await?,
            Some((record, recovered)) => {
                let found = (recovered, lease);
                Writer::open_recovered_from(bucket.clone(), record, found, holder, token).await?
            }
        };
        let writer = Arc::new(writer.uploading_within(self.uploads.clone()));
        let opening = Opening { state: writer.state(), fenced: Some(writer.fenced()), cache };
        let store = blocking(|| recovering.open(options, opening))?;
        let _ = self.store.set(Arc::new(store));
        let _ = self.token.set(token);
        self.took(&writer, token);
        Ok(Some(writer))
    }

    /// `writer` took the branch, with the fencing token `token`: the store names what it reads in
    /// the bucket where the writer's checkpoints move it, as the writer does (TASK-195); the
    /// branch's commits are uploaded, its forks made, and what the store's write-ahead log holds
    /// after them is the uploader's to upload.
    fn took(&self, writer: &Arc<Writer<B>>, token: u64) {
        let following = Arc::downgrade(self.store());
        writer.following(move |moved| {
            let Some(store) = following.upgrade() else { return };
            if let Err(err) = store.moved(moved) {
                log::warn!("fxdax: following the collector's moves: {err:#}");
            }
        });
        *self.writer.lock().unwrap() = Some(writer.clone());
        let lease = Lease { token, epoch: writer.epoch(), taken: SystemTime::now() };
        let mut progress = self.progress.lock().unwrap();
        (progress.next_sequence, progress.lease) = (writer.chain().next_sequence(), Some(lease));
        if let Some(last) = writer.state().last_commit {
            progress.uploaded_through(epoch_of(last));
            progress.forked_through(epoch_of(last));
        }
        drop(progress);
        self.changed.notify(usize::MAX);
    }

    /// What uploading goes on with after `err`: if its writer was displaced, an earlier writer's
    /// segment, landed late past its fence, where its own was to go, a writer taking the branch
    /// again ([`Shared::retake`]); else `err`, which stops it.
    async fn after(&self, err: Failure, bucket: &Arc<B>) -> Result<Arc<Writer<B>>, Failure> {
        match err {
            Failure::Remote(extent_remote::Error::Displaced) => self.retake(bucket).await,
            err => Err(err),
        }
    }

    /// Takes the branch again, its writer displaced (doc-7, Fencing; TASK-204): a writer of a lease
    /// of its own fences the branch past every segment landed, and the store's write-ahead log goes
    /// after the branch's end ([`Store::rebase`]), what of it landed acknowledged, the rest stamped
    /// anew, which the new writer uploads. The guest's commits wait for it, none lost: the late
    /// segment follows none of the branch's, void, and the branch's last commit is the store's own
    /// or the one its log goes on from. Fails as opening a store does: `Diverged` if another
    /// writer's commits moved the branch past the log, fenced if a later writer has it.
    async fn retake(&self, bucket: &Arc<B>) -> Result<Arc<Writer<B>>, Failure> {
        let store = self.store().clone();
        let branch = store.description().branch;
        let record = record(&**bucket, branch).await?.ok_or(extent_remote::Error::NoBranch)?;
        let recovered = extent_remote::recover(&**bucket, branch, record.origin()).await?;
        let token = *self.token.get().expect("a writer took the branch before");
        let holder = rand::random();
        let writer =
            Writer::open_recovered(bucket.clone(), record, recovered, holder, token).await?;
        let writer = Arc::new(writer.uploading_within(self.uploads.clone()));
        let fenced = writer.fenced();
        blocking(|| store.rebase(&fenced))?;
        log::warn!("fxdax: branch {branch:x} displaced, taken again at epoch {}", writer.epoch());
        self.took(&writer, token);
        Ok(writer)
    }

    /// Opens the store of `branch` as a point, whose record is `record`: its state, which never
    /// changes. No lease: nothing the store does goes to the bucket but its forks' records.
    async fn point(
        &self,
        bucket: &Arc<B>,
        branch: u64,
        record: Record,
    ) -> Result<extent_remote::State, Failure> {
        if record.deleted {
            return Err(Failure::PointDeleted);
        }
        let Kind::Point(origin) = record.kind else {
            return Err(Failure::msg(format!("branch {branch:x} is no point")));
        };
        let state = extent_remote::recover(&**bucket, branch, Some(origin)).await?.state;
        let _ = self.point.set(Point { branch, origin });
        Ok(state)
    }

    /// Adds the segments of the store's write-ahead log whose commits are durable to the writer, to
    /// be uploaded as they are, after those uploading (`in_flight`), as many as may upload at once
    /// with them: the rest stay in the store, unread, until those are uploaded. If `close`, the
    /// store closes its open segment first if fewer wait: for a commit waiting for its upload, a
    /// fork, the interval up, or the uploader stopping; for no other, as the commits no one waits
    /// for wait in it for those, rather than each go in a segment of its own (TASK-92.15).
    fn gather(
        &self,
        writer: &Arc<Writer<B>>,
        in_flight: &mut InFlight,
        close: bool,
    ) -> Result<Gathered, Failure> {
        failing!("remote::gather");
        let store = self.store();
        let room = IN_FLIGHT.saturating_sub(in_flight.len());
        // What a waiter wants is durable; then closed, if fewer segments wait than may upload, and
        // what the closing closed durable too.
        store.flush()?;
        let mut closed = store.closed(room)?;
        let closing = close && closed.len() < room;
        if closing {
            store.close_now()?;
            store.flush()?;
            closed.extend(store.closed(room - closed.len())?);
        }
        let mut segments = Vec::with_capacity(closed.len());
        for closed in closed {
            let (number, tag) = (closed.number, closed.tag());
            let forks: Vec<u64> =
                closed.footer.contents.forks.iter().map(|forked| forked.branch).collect();
            let own = closed.own.iter().map(|(key, file)| Ok((key.clone(), stored(file)?)));
            let own = own.collect::<Result<_, Failure>>()?;
            let footer = closed.footer.clone();
            let size = closed.file.size().map_err(|err| Failure::local(err, "a segment"))?;
            let closed = Arc::new(closed);
            let segment = Body::new(size, move || blocking(|| closed.bytes()));
            let seq = writer.add_segment(Objects { footer, segment, own })?;
            // Where each fork it ends is, which a fork failing deletes its child by.
            let origin = Origin { parent: writer.branch(), at: seq };
            self.forks.lock().unwrap().extend(forks.iter().map(|&child| (child, origin)));
            segments.push(Uploading { seq, number, tag, forks: !forks.is_empty() });
        }
        let took = !segments.is_empty();
        in_flight.upload(writer, segments);
        Ok(Gathered { took, closed: closing })
    }

    /// Acknowledges what the writer has uploaded: the commits of the segments it acknowledges are
    /// uploaded, and the store's segments leave it, read in the bucket, into the host's cache if
    /// it has one (TASK-165.8); then the forks they name are made, with a checkpoint at them, which
    /// their children's recovery starts from.
    async fn acknowledged(
        &self,
        writer: &Writer<B>,
        in_flight: &mut InFlight,
    ) -> Result<(), Failure> {
        let through = writer.acknowledge().await?;
        failing!("remote::acknowledged");
        let acked = in_flight.acknowledged(through);
        let Some(epoch) = acked.last().map(Uploading::epoch) else { return Ok(()) };
        blocking(|| {
            for segment in &acked {
                self.store().acknowledged(segment.number)?;
            }
            Ok::<_, Failure>(())
        })?;
        failing!("remote::released");
        let forks = acked.iter().any(|segment| segment.forks);
        {
            let mut progress = self.progress.lock().unwrap();
            progress.uploaded_through(epoch);
            if !forks {
                progress.forked_through(epoch);
            }
        }
        self.changed.notify(usize::MAX);
        if forks {
            // Their children have their records, which the acknowledgement made: the checkpoint
            // makes the forks.
            writer.checkpoint().await?;
            failing!("remote::checkpointed");
            self.progress.lock().unwrap().forked_through(epoch);
            self.changed.notify(usize::MAX);
        }
        Ok(())
    }
}

/// The record of `branch`, if it has one: a fork of a point its host crashed in the middle of, its
/// record saying so, finished first, which fails if the point was deleted. So the store takes no
/// state of a fork not made.
async fn record<B: extent_remote::Bucket>(
    bucket: &B,
    branch: u64,
) -> Result<Option<Record>, Failure> {
    let found = branch::record(bucket, branch).await?;
    let Some(point) = found.as_ref().filter(|record| !record.deleted).and_then(Record::forking)
    else {
        return Ok(found);
    };
    branch::finish_point_fork(bucket, branch, point).await?;
    Ok(branch::record(bucket, branch).await?)
}

/// The bytes of `file`, an extent's of its own a segment of the write-ahead log names, read only
/// as its upload goes, and checked: a byte the host's disk damaged is not uploaded under the CRC
/// it fails (`read_object`, TASK-153.11).
fn stored<F: File>(file: &Arc<F>) -> Result<Body, Failure> {
    let size = file.size().map_err(|err| Failure::local(err, "a file to upload"))?;
    let file = file.clone();
    Ok(Body::new(size, move || blocking(|| read_object(&*file))))
}

/// What a gather did ([`Shared::gather`]).
struct Gathered {
    /// Whether it took segments the store closed: the store may have closed more meanwhile.
    took: bool,
    /// Whether it closed the store's open segment, as asked: what was committed before is in the
    /// segments the store closed, taken, or to be taken next.
    closed: bool,
}

/// A segment uploading: its number in the branch, and in the store; the tag of the store's commit
/// it ends with; and whether forks are made at its end.
struct Uploading {
    seq: u64,
    number: u64,
    tag: u64,
    forks: bool,
}

impl Uploading {
    /// The store's epoch of the commit it ends with.
    fn epoch(&self) -> u64 {
        epoch_of(self.tag)
    }
}

/// The segments uploading, in order, and the futures of their uploads: an acknowledgement takes
/// exactly the segments the writer acknowledged. Each is in the store's write-ahead log, durably,
/// before its upload goes (TASK-79.72's rule), so that nothing lands in the writer's name that the
/// log doesn't hold.
#[derive(Default)]
struct InFlight {
    segments: VecDeque<Uploading>,
    uploads: FuturesOrdered<BoxFuture<'static, Result<(), extent_remote::Error>>>,
}

impl InFlight {
    /// Uploads `segments`, which `writer` took, after those uploading.
    fn upload<B: extent_remote::Bucket>(
        &mut self,
        writer: &Arc<Writer<B>>,
        segments: Vec<Uploading>,
    ) {
        for segment in segments {
            let (writer, seq) = (writer.clone(), segment.seq);
            self.uploads.push_back(async move { writer.upload(seq).await }.boxed());
            self.segments.push_back(segment);
        }
    }

    /// The next upload done, in order.
    fn next(
        &mut self,
    ) -> impl Future<Output = Option<Result<(), extent_remote::Error>>> + Unpin + '_ {
        self.uploads.next()
    }

    /// The segments the writer acknowledged, `through` the last, which upload no more.
    fn acknowledged(&mut self, through: Option<u64>) -> Vec<Uploading> {
        let taken = |segment: &&Uploading| through.is_some_and(|through| segment.seq <= through);
        let count = self.segments.iter().take_while(taken).count();
        self.segments.drain(..count).collect()
    }

    /// How many uploads are in flight.
    fn len(&self) -> usize {
        self.uploads.len()
    }

    fn is_empty(&self) -> bool {
        self.uploads.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Description, MemDir};
    use extent_remote::fetch::Limits;
    use storage_device::ExtentId;

    const BRANCH: u64 = 7;

    /// TASK-92.52: a stall says what it waits for: the bytes past the lag bound, which a stall over
    /// it waits on, and the commits not uploaded and the oldest's age, which a slow upload shows.
    #[test]
    fn a_stall_says_what_it_waits_for() {
        let over = stalled(5 << 20, 4 << 20, 3, Some(Duration::from_millis(250)));
        assert_eq!(
            over,
            "fxdax: commits wait for object storage: 5242880 bytes not uploaded, 1048576 past the \
             lag bound of 4194304; 3 commits not uploaded, the oldest seen 250 ms ago"
        );
        let slow = stalled(1 << 20, 4 << 20, 1, None);
        assert_eq!(
            slow,
            "fxdax: commits wait for object storage: 1048576 bytes not uploaded, 0 past the lag \
             bound of 4194304; 1 commits not uploaded, none seen yet"
        );
    }

    /// TASK-79.19: a gather takes no more of the segments the store closed than may upload at once,
    /// with those uploading; the rest it leaves in the store, until those it took are uploaded.
    #[test]
    fn gathers_take_what_may_upload_at_once() {
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        let host =
            Runtime { handle: runtime.handle().clone(), uploads: Budget::new(Limits::UNBOUNDED) };
        let bucket = Arc::new(Bucket::new(Arc::new(object_store::memory::InMemory::new()), ""));
        let dir = MemDir::new();
        let description = Description { block_size: 4096, capacity: 1 << 30, branch: BRANCH };
        Store::make(&dir, description, Some("memory:///gathers")).unwrap();
        // Its uploader uploads nothing it is not asked to within the hour: the test gathers.
        let options = Options { interval: Duration::from_secs(3600), ..Options::default() };
        let recovering = Store::recover_unmade(dir).unwrap();
        let (store, remote) =
            open(recovering, crate::Options::default(), bucket.clone(), (1, options), &host)
                .unwrap();
        store.made().unwrap();
        // Each commit closes its segment.
        let closed = 2 * IN_FLIGHT + 1;
        for sequence in 1..=closed as u64 {
            store.write(ExtentId::new(BRANCH, sequence), 0, &[sequence as u8; 4096]).unwrap();
            store.close_segment().unwrap();
            store.commit(None).unwrap();
        }
        let shared = &remote.shared;
        let writer = shared.writer.lock().unwrap().clone().unwrap();
        let mut in_flight = InFlight::default();
        let mut taken = Vec::new();
        while shared.gather(&writer, &mut in_flight, true).unwrap().took {
            taken.push(in_flight.len());
            runtime.block_on(async {
                while let Some(uploaded) = in_flight.next().await {
                    uploaded.unwrap();
                }
                shared.acknowledged(&writer, &mut in_flight).await.unwrap();
            });
        }
        assert_eq!(taken, [IN_FLIGHT, IN_FLIGHT, 1]);
        assert_eq!(remote.last_uploaded(), store.epoch().unwrap());
        drop(remote);
        let state = runtime.block_on(extent_remote::recover(&*bucket, BRANCH, None)).unwrap().state;
        let sequences: Vec<u64> = state.extents.keys().map(|extent| extent.sequence).collect();
        assert_eq!(sequences, (1..=closed as u64).collect::<Vec<_>>());
    }
}

/// fxdax: a store committing, sealing and forking, its uploader uploading what it commits, and
/// passes of the collector running over the bucket all along (TASK-291.1), under Shuttle, whose
/// scheduler chooses every interleaving of their threads at each of the store's locks and at each
/// request to the bucket, and replays a failing one from its seed.
/// `cargo nextest run -p extent_store --features shuttle --lib shuttle_tests::`.
#[cfg(all(test, feature = "shuttle"))]
mod shuttle_tests {
    use super::*;
    use crate::{Description, MemDir};
    use extent_remote::bucket::{ByteRange, Error as BucketError, Meta, Object, PutMode, Version};
    use extent_remote::collector;
    use extent_remote::fetch::Limits;
    use extent_remote::testing::{Direct, SimBucket};
    use shuttle::scheduler::{PctScheduler, RandomScheduler, Scheduler};
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::sync::atomic::Ordering::SeqCst;
    use storage_device::ExtentId;

    const BLOCK: usize = 4096;
    const BRANCH: u64 = 7;
    /// The branch the writer forks.
    const CHILD: u64 = BRANCH + 1;

    /// Small segments, so that commits close several, and extents of two blocks or more in
    /// objects of their own, which upload before the segments naming them (TASK-79.3); flushes of
    /// two blocks at most held in their log records, larger ones sealed (TASK-301).
    const STORE: crate::Options = crate::Options {
        max_extent: 64 << 20,
        segment_size: 3 * BLOCK as u64,
        segment_age: Duration::from_secs(3600),
        large_extent: 2 * BLOCK as u64,
        open_files: 2,
        view_lag: None,
        view_kept_bytes: None,
        held_most: 2 * BLOCK as u64,
        spilled_most: 2048,
    };

    /// The collector's passes compact what is mostly dead, and delete at once.
    const COLLECTING: collector::Options = collector::Options { compact_below: 0.9, grace: 0 };

    /// The schedules each test runs, randomly and by PCT; `SHUTTLE_SOAK` multiplies them.
    fn runs() -> usize {
        let soak: usize = std::env::var("SHUTTLE_SOAK").map_or(1, |soak| soak.parse().unwrap());
        200 * soak
    }

    /// A bucket in memory whose requests each give Shuttle's scheduler a turn first, as a real
    /// bucket's round trips let the other threads go on: so that a pass of the collector, the
    /// uploader and the store interleave at each. Its requests take effect at once, and never fail.
    /// Its gate, once armed, holds up the next listing of the whole bucket, a pass's of its objects.
    struct Turns {
        direct: Direct,
        gate: Gate,
    }

    impl Turns {
        fn new() -> Arc<Self> {
            Arc::new(Self { direct: Direct(SimBucket::new()), gate: Gate::default() })
        }
    }

    /// Holds up a listing of the whole bucket, once armed, until opened.
    #[derive(Default)]
    struct Gate {
        armed: shuttle::sync::atomic::AtomicBool,
        reached: shuttle::sync::atomic::AtomicBool,
        open: shuttle::sync::atomic::AtomicBool,
    }

    impl Gate {
        fn arm(&self) {
            self.armed.store(true, SeqCst);
        }

        /// Waits until a listing held up reaches the gate.
        fn reached(&self) {
            until(|| self.reached.load(SeqCst));
        }

        fn open(&self) {
            self.open.store(true, SeqCst);
        }

        /// A listing of `prefix` goes through the gate.
        fn pass(&self, prefix: &str) {
            if prefix.is_empty() && self.armed.swap(false, SeqCst) {
                self.reached.store(true, SeqCst);
                until(|| self.open.load(SeqCst));
            }
        }
    }

    impl extent_remote::Bucket for Turns {
        fn get(&self, key: &str) -> impl Future<Output = Result<Object, BucketError>> + Send {
            shuttle::thread::yield_now();
            self.direct.get(key)
        }

        fn get_range(
            &self,
            key: &str,
            range: ByteRange,
        ) -> impl Future<Output = Result<Vec<u8>, BucketError>> + Send {
            shuttle::thread::yield_now();
            self.direct.get_range(key, range)
        }

        fn head(&self, key: &str) -> impl Future<Output = Result<Meta, BucketError>> + Send {
            shuttle::thread::yield_now();
            self.direct.head(key)
        }

        fn list(
            &self,
            prefix: &str,
        ) -> impl Future<Output = Result<Vec<Meta>, BucketError>> + Send {
            shuttle::thread::yield_now();
            self.gate.pass(prefix);
            self.direct.list(prefix)
        }

        fn put(
            &self,
            key: &str,
            bytes: extent_remote::Bytes,
            mode: PutMode,
        ) -> impl Future<Output = Result<Version, BucketError>> + Send {
            shuttle::thread::yield_now();
            self.direct.put(key, bytes, mode)
        }

        fn delete(&self, key: &str) -> impl Future<Output = Result<(), BucketError>> + Send {
            shuttle::thread::yield_now();
            self.direct.delete(key)
        }

        fn now(&self) -> u64 {
            self.direct.now()
        }
    }

    /// The bytes of the extent at `at`, `blocks` of them.
    fn pattern(at: u64, blocks: usize) -> Vec<u8> {
        (0..blocks * BLOCK).map(|i| (i % 251) as u8 ^ at as u8).collect()
    }

    /// Waits, giving the scheduler turns, for what `receiver` gets: a thread of Shuttle's blocking
    /// on the channel would hold up every other.
    fn received<T>(receiver: &std::sync::mpsc::Receiver<T>) -> T {
        loop {
            match receiver.try_recv() {
                Ok(received) => return received,
                Err(std::sync::mpsc::TryRecvError::Empty) => shuttle::thread::yield_now(),
                Err(err) => panic!("{err}"),
            }
        }
    }

    /// What the branch `branch`, recovered from `bucket`, holds, if not `holds`, extent by extent.
    fn differs(
        bucket: &Arc<Turns>,
        branch: u64,
        holds: &BTreeMap<ExtentId, Vec<u8>>,
    ) -> Result<(), String> {
        let failed = |err: extent_remote::Error| format!("branch {branch:x}: {err}");
        let record = shuttle::future::block_on(branch::record(&**bucket, branch));
        let record = record.map_err(failed)?.ok_or(format!("branch {branch:x}: no record"))?;
        let recovering = extent_remote::recover(&**bucket, branch, record.origin());
        let state = shuttle::future::block_on(recovering).map_err(failed)?.state;
        let held: Vec<&ExtentId> = state.extents.keys().collect();
        if held != holds.keys().collect::<Vec<_>>() {
            return Err(format!("branch {branch:x} holds {held:?}, not {:?}", holds.keys()));
        }
        for (extent, location) in &state.extents {
            let read = shuttle::future::block_on(extent_remote::read(bucket, *extent, location));
            if read.as_ref() != Ok(&holds[extent]) {
                return Err(format!("branch {branch:x}: {extent:?} reads {read:?}"));
            }
        }
        Ok(())
    }

    /// Waits, giving the scheduler turns, until `done`.
    fn until(done: impl Fn() -> bool) {
        while !done() {
            shuttle::thread::yield_now();
        }
    }

    /// A store of `branch` on a disk of its own, made empty, and its uploader, running on a thread
    /// of its own as `holder`, whose writer has taken the branch.
    struct Session {
        uploader: Arc<Shared<MemDir, Turns>>,
        running: shuttle::thread::JoinHandle<()>,
        store: Arc<Store<MemDir>>,
        branch: u64,
        /// What the store's extents hold, by their IDs.
        holds: BTreeMap<ExtentId, Vec<u8>>,
        /// The first sequence number of its own extents'.
        next: u64,
    }

    impl Session {
        fn open(bucket: &Arc<Turns>, branch: u64, holder: u64) -> Self {
            let dir = MemDir::new();
            let description = Description { block_size: BLOCK as u32, capacity: 1 << 30, branch };
            Store::make(&dir, description, None).unwrap();
            let recovering = Store::recover_unmade(dir).unwrap();
            // Its interval never runs out: the writer asks for what it commits.
            let never: Clock = Arc::new(|_| futures::future::pending().boxed());
            let options = Options { token: holder, ..Options::default() };
            let uploader = Shared::new(options, Budget::new(Limits::UNBOUNDED), never);
            let (started, start) = std::sync::mpsc::channel();
            let running = {
                let (uploader, bucket) = (uploader.clone(), bucket.clone());
                shuttle::thread::spawn(move || {
                    let running = uploader.run(recovering, STORE, bucket, holder, started);
                    shuttle::future::block_on(running)
                })
            };
            let store = received(&start).unwrap();
            store.made().unwrap();
            let next = uploader.next_sequence();
            Self { uploader, running, store, branch, holds: BTreeMap::new(), next }
        }

        fn extent(&self, at: u64) -> ExtentId {
            ExtentId::new(self.branch, self.next + at)
        }

        fn write(&mut self, at: u64, blocks: usize) {
            let bytes = pattern(self.branch ^ at, blocks);
            self.store.write(self.extent(at), 0, &bytes).unwrap();
            self.holds.insert(self.extent(at), bytes);
        }

        fn free(&mut self, extent: ExtentId) {
            self.store.free(extent).unwrap();
            self.holds.remove(&extent);
        }

        /// Asks the uploader for what the store committed.
        fn ask(&self) {
            self.uploader.ask(self.store.epoch().unwrap(), false).unwrap();
        }

        /// The uploader uploads all the store made durable, and stops; and the store closes.
        fn end(self) -> Result<BTreeMap<ExtentId, Vec<u8>>, String> {
            let last = self.store.epoch().unwrap();
            self.uploader.stop();
            self.running.join().unwrap();
            drop(self.store);
            let (failed, uploaded) = (self.uploader.failed(), self.uploader.last_uploaded());
            if failed != Some(Failure::msg("the uploader stopped")) || uploaded != last {
                return Err(format!("the uploader: {failed:?}, through {uploaded} of {last}"));
            }
            Ok(self.holds)
        }
    }

    /// One schedule: the writer writes, frees, commits and forks, asking for uploads; once the
    /// fork is made, the child's store, on another host, writes, frees what it inherited and
    /// commits, meanwhile; their uploaders upload; passes of the collector run all along. Then the
    /// uploaders upload what is left and stop, a last pass runs alone, and each branch, recovered
    /// from the bucket, holds what its store's last commit left, each extent reading as it was
    /// written. What went wrong, if anything did.
    fn collected_while_committing() -> Result<(), String> {
        let bucket = Turns::new();
        let mut parent = Session::open(&bucket, BRANCH, 1);
        // Passes, one after another, for as long as the writers write and their uploads go.
        let writing = Arc::new(shuttle::sync::atomic::AtomicBool::new(true));
        let collecting = {
            let (bucket, writing) = (bucket.clone(), writing.clone());
            shuttle::thread::spawn(move || {
                for holder in 100.. {
                    let pass = collector::collect(&*bucket, holder, &COLLECTING);
                    shuttle::future::block_on(pass)
                        .map_err(|err| format!("pass {holder}: {err}"))?;
                    if !writing.load(SeqCst) {
                        return Ok::<_, String>(());
                    }
                }
                unreachable!("passes for ever")
            })
        };

        parent.write(0, 1);
        parent.store.commit_ordered(None).unwrap();
        // Asked for that commit with an extent of its own open, the uploader closes the open
        // segment between commits: its footer's next sequence number counts the extent, which the
        // next segment names, its object uploaded first (TASK-79.3).
        parent.write(1, 3);
        parent.ask();
        parent.write(2, 2);
        parent.free(parent.extent(0));
        parent.store.commit(None).unwrap();
        parent.ask();
        parent.write(3, 1);
        parent.store.commit_fork(Child { branch: CHILD, owner: 9, point: false }).unwrap();
        let fork = parent.store.epoch().unwrap();
        parent.ask();
        until(|| parent.uploader.forked() >= fork);
        // The child, made by the fork, whose record the collector may not have read yet: what its
        // store uploads, it knows nothing of until it reads the record, its segments not those of a
        // branch it recovered.
        let mut child = Session::open(&bucket, CHILD, 2);
        child.holds = parent.holds.clone();
        child.write(0, 1);
        child.free(parent.extent(1));
        child.store.commit_ordered(None).unwrap();
        child.ask();
        parent.free(parent.extent(1));
        parent.write(4, 2);
        parent.store.commit_ordered(None).unwrap();
        parent.ask();
        child.write(1, 3);
        child.store.commit(None).unwrap();
        child.ask();
        parent.free(parent.extent(3));
        parent.store.commit(None).unwrap();
        parent.ask();
        child.free(parent.extent(2));
        child.write(2, 1);
        child.store.commit(None).unwrap();
        child.ask();
        let parent = parent.end()?;
        let child = child.end()?;
        writing.store(false, SeqCst);
        collecting.join().unwrap()?;
        let pass = collector::collect(&*bucket, 200, &COLLECTING);
        shuttle::future::block_on(pass).map_err(|err| format!("the last pass: {err}"))?;
        differs(&bucket, BRANCH, &parent)?;
        differs(&bucket, CHILD, &child)
    }

    /// Runs `schedule` under `scheduler`'s interleavings.
    fn explore(scheduler: impl Scheduler + 'static, schedule: impl Fn() + Send + Sync + 'static) {
        let mut config = shuttle::Config::new();
        config.stack_size = 4 << 20;
        shuttle::Runner::new(scheduler, config).run(schedule);
    }

    /// TASK-291.1: whatever the interleaving, what each store committed and its uploader
    /// acknowledged stays its branch's, the parent's and its fork's child's, the collector running
    /// all along.
    ///
    /// No rule of the collector's off has it lose anything here in thousands of schedules (seven
    /// tried, `candidates_first` among them): once the branch has a checkpoint of its own, each
    /// pass lists the records again after recovering what they name, which leaves a pass's loss
    /// little room between two of its requests. [`listed_while_forking`] holds a pass up there.
    #[test]
    fn collection_keeps_what_a_store_commits_meanwhile() {
        let schedule = || collected_while_committing().unwrap();
        explore(RandomScheduler::new(runs()), schedule);
        explore(PctScheduler::new(3, runs()), schedule);
    }

    /// One schedule: a pass of the collector, held up as it lists the bucket's objects, while the
    /// store's fork to the child is made and the child's store, opened, commits and its uploader
    /// uploads its segment. Then the pass goes on, the uploaders upload what is left and stop, a
    /// last pass runs alone, and each branch, recovered from the bucket, holds what its store's
    /// last commit left. What went wrong, if anything did.
    fn listed_while_forking() -> Result<(), String> {
        let bucket = Turns::new();
        let mut parent = Session::open(&bucket, BRANCH, 1);
        parent.write(0, 1);
        parent.write(1, 3);
        parent.store.commit_ordered(None).unwrap();
        parent.ask();
        let before = parent.store.epoch().unwrap();
        until(|| parent.uploader.last_uploaded() >= before);
        parent.store.commit_fork(Child { branch: CHILD, owner: 9, point: false }).unwrap();
        let fork = parent.store.epoch().unwrap();
        bucket.gate.arm();
        let passing = {
            let bucket = bucket.clone();
            shuttle::thread::spawn(move || {
                let pass = collector::collect(&*bucket, 100, &COLLECTING);
                shuttle::future::block_on(pass).map_err(|err| format!("the pass: {err}"))
            })
        };
        bucket.gate.reached();
        parent.ask();
        until(|| parent.uploader.forked() >= fork);
        let mut child = Session::open(&bucket, CHILD, 2);
        child.holds = parent.holds.clone();
        child.write(0, 1);
        child.free(parent.extent(0));
        child.store.commit_ordered(None).unwrap();
        child.ask();
        let committed = child.store.epoch().unwrap();
        until(|| child.uploader.last_uploaded() >= committed);
        bucket.gate.open();
        passing.join().unwrap()?;
        let parent = parent.end()?;
        let child = child.end()?;
        let pass = collector::collect(&*bucket, 200, &COLLECTING);
        shuttle::future::block_on(pass).map_err(|err| format!("the last pass: {err}"))?;
        differs(&bucket, BRANCH, &parent)?;
        differs(&bucket, CHILD, &child)
    }

    /// A pass listing the bucket's objects before it reads the branches' records keeps what a
    /// child, made after the listing began, uploads before it ends: the pass reads the child's
    /// record, and its recovery names the segment.
    #[test]
    fn a_pass_keeps_what_a_child_made_as_it_lists_uploads() {
        explore(RandomScheduler::new(runs() / 10), || listed_while_forking().unwrap());
    }

    /// The collector listing the bucket's objects after it read what the branches need, not before:
    /// a child whose record no pass read, made meanwhile, has its segment listed, which no branch
    /// the pass recovered names, and deleted.
    #[test]
    fn a_pass_keeping_what_a_child_made_as_it_lists_uploads_needs_candidates_first() {
        let all = extent_remote::Rules::ALL;
        let off = extent_remote::Rules { candidates_first: false, ..all };
        explore(RandomScheduler::new(runs() / 10), move || {
            extent_remote::set_rules(off);
            let listed = listed_while_forking();
            extent_remote::set_rules(all);
            let extent = "ExtentId { branch: 8, sequence: 1099511627776 }";
            let segment = "segments/0000000000000008/0000000000000001";
            let lost = format!("branch 8: {extent} reads Err(Lost(\"{extent} in {segment}\"))");
            assert_eq!(listed, Err(lost));
        });
    }
}
