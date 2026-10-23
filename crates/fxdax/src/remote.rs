// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own: a store's branch kept in object storage (backlog doc-7, Uploads; TASK-28.5).
//!
//! An uploader takes the branch's lease, uploads what the store holds that the branch doesn't, a
//! segment of one commit closed on its writer's chain (`extent_frame::segment::Chain`), and then,
//! behind the store, the segments the store closes, their footers stamped by the chain from there
//! on, as they are, each with the extents of its own it names first: a closed local segment is the
//! object (doc-7, decision 1). The store closes a segment when it fills or ages, at a fork, and
//! when the uploader asks, within an interval of the commits it makes durable. Once a segment is
//! acknowledged, the data of the extents its commits freed may go from the store, and commits
//! waiting to be durable (`COMMIT_DURABLE`) complete.
//!
//! The segments uploading, and the store's directory's marker naming them, are kept together
//! ([`InFlight`]): the marker names every segment that may land, before its upload goes, so that
//! the store opened again knows the branch's last commit for its own. A fork the store makes is a
//! fork of the branch once the segment its commit ends is acknowledged, which gives the child its
//! record, and checkpointed: the child's marker names the fork's commit ([`Remote::forked`]).
//!
//! What a host holds to upload is bounded, however far behind its stores are (TASK-79.19): a
//! store's segments upload [`IN_FLIGHT`] at once, the rest staying in the store meanwhile, each
//! object read only as its upload goes, within a budget the host's stores share ([`UPLOADS`]); and
//! what a store holds that its branch doesn't goes in segments of a [`BATCH`] at most.
//!
//! Every failure of its writer's stops the uploader for good, and the commits waiting for uploads
//! fail with it from then on: its writer fenced, its branch deleted, or an earlier writer's segment
//! found where its next goes (`extent_remote::Error::Displaced`), which leaves the segments the
//! store closed numbered for a place the branch no longer has. Only attaching the store anew takes
//! the branch again.

use event_listener::{Event, Listener as _};
use extent_remote::batch;
use extent_remote::cache::Cache;
use extent_remote::fetch::{self, Budget, Limits, Priority};
use extent_remote::layout::{Footer, Origin, Record, epoch_of, tag};
use extent_remote::store::Store as Bucket;
use extent_remote::{Body, Objects, State, Writer, branch};
use extent_store::{File as _, HostDir, HostFile, Snapshot, Store, read_whole};
use futures::future::{BoxFuture, Either, FutureExt as _};
use futures::stream::FuturesOrdered;
use futures::{StreamExt as _, TryStreamExt as _};
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use storage_device::ExtentId;

/// The size from which an extent is stored on its own (doc-7).
const LARGE: u64 = 4 << 20;

/// How many of a store's segments upload at once.
const IN_FLIGHT: usize = 4;

/// The most bytes of extents a segment of what a store holds that its branch doesn't holds, those
/// stored on their own included, but for one extent alone that is more: a store's segment's size
/// (`extent_store::Options::segment_size`).
const BATCH: u64 = 32 << 20;

/// How many segments go between checkpoints.
const CHECKPOINT_EVERY: u64 = 64;

/// The file in a store's directory that names the last commit of a branch of the store it uploaded,
/// and those it is uploading ([`Marker`]): a branch whose last commit is none of them, another host
/// moved on.
fn marker(branch: u64) -> String {
    format!("remote-{branch:016x}")
}

/// The file in a store's directory that holds the fencing token of its copy of a branch of it
/// ([`place`]).
fn fence(branch: u64) -> String {
    format!("fence-{branch:016x}")
}

/// Writes `bytes` to the file `name` in the directory `dir`, durably, whole or not at all: to a new
/// file, synced, then renamed over it, the directory synced.
fn write_durably(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let new = dir.join(format!("{name}.new"));
    let mut file = std::fs::File::create(&new)?;
    std::io::Write::write_all(&mut file, bytes)?;
    file.sync_all()?;
    std::fs::rename(&new, dir.join(name))?;
    std::fs::File::open(dir)?.sync_all()
}

/// Places the store directory `dir`'s copy of `branch` (TASK-39.7): records, durably, the fencing
/// token its writer takes the branch's lease with from then on, the boot ID of the placement it is,
/// which a later placement's exceeds. A writer takes the lease only with a token as high as the
/// last writer's: a placement cut off takes no branch back from the one after it. A copy never
/// placed takes it with 0.
pub(crate) fn place(dir: &Path, branch: u64, token: u64) -> std::io::Result<()> {
    write_durably(dir, &fence(branch), format!("{token}\n").as_bytes())
}

/// Deletes `branch` in `bucket` (`extent_remote::branch::delete`), as the placement whose fencing
/// token is `token`: its writer stops at its next acknowledgement, no writer takes it again, and
/// the collector reclaims what no other branch inherited from it, as its forks' branches live on
/// (TASK-39.4). Deleting it again does nothing. A later placement's branch, whose token is higher,
/// is not deleted ([`Failure::Stale`], TASK-45.1).
pub fn delete_branch(bucket: &Bucket, branch: u64, token: u64) -> Result<(), Failure> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| Failure::Other(format!("a runtime: {err}")))?;
    match runtime.block_on(branch::delete(bucket, branch, rand::random(), token)) {
        Err(extent_remote::Error::Stale) => Err(Failure::Stale),
        deleted => {
            deleted.map_err(|err| Failure::Other(format!("deleting branch {branch:x}: {err}")))
        }
    }
}

/// The fencing token of the copy of `branch` in the store directory `dir`: its placement's
/// ([`place`]), if it was placed.
pub(crate) fn placed_token(dir: &Path, branch: u64) -> Result<Option<u64>, Failure> {
    let path = dir.join(fence(branch));
    match std::fs::read_to_string(&path) {
        Ok(text) => (text.trim().parse().map(Some))
            .map_err(|err| Failure::Other(format!("{}: {err}", path.display()))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(Failure::Other(format!("reading {}: {err}", path.display()))),
    }
}

/// The fencing token of this host's placement, `FXDAX_EXTENTD_TOKEN`, which the system's control
/// plane gives it, and which the copies of stores here that were never placed ([`place`]) take
/// their branches with, as stores restored or taken here are. Not 0, which would fence nothing:
/// a host without one keeps no branch (TASK-45).
pub fn host_token() -> Result<u64, Failure> {
    let missing = || {
        Failure::Other(
            "FXDAX_EXTENTD_TOKEN names no placement token: this host's placement's, from the \
             system's control plane, which fences earlier placements"
                .into(),
        )
    };
    match std::env::var("FXDAX_EXTENTD_TOKEN") {
        Ok(token) if !token.is_empty() => match token.parse() {
            Ok(0) => Err(missing()),
            Ok(token) => Ok(token),
            Err(err) => Err(Failure::Other(format!("FXDAX_EXTENTD_TOKEN {token:?}: {err}"))),
        },
        _ => Err(missing()),
    }
}

/// This host, as one of fxdax's multi-node system, the only kind there is (TASK-45): the bucket
/// its stores' branches are kept in, and its placement's fencing token, as the environment names
/// them. Tests and development use an in-memory bucket (`memory:///NAME`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Host {
    /// The bucket's URL, `FXDAX_EXTENTD_REMOTE`.
    pub bucket: String,
    /// The placement's fencing token, `FXDAX_EXTENTD_TOKEN` ([`host_token`]).
    pub token: u64,
}

impl Host {
    /// The host the environment names, or why it names none.
    pub fn from_env() -> Result<Self, String> {
        let bucket = std::env::var("FXDAX_EXTENTD_REMOTE").ok().filter(|url| !url.is_empty());
        let bucket = bucket.ok_or_else(|| {
            "FXDAX_EXTENTD_REMOTE names no bucket: a host of fxdax's multi-node system keeps its \
             stores' branches in one (tests and development: memory:///NAME)"
                .to_owned()
        })?;
        let token = host_token().map_err(|err| err.to_string())?;
        Ok(Self { bucket, token })
    }
}

/// Removes the marker and fencing token of `branch`, deleted, from the store directory `dir`.
pub(crate) fn unmark(dir: &Path, branch: u64) -> std::io::Result<()> {
    for name in [marker(branch), fence(branch)] {
        match std::fs::remove_file(dir.join(name)) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            removed => removed?,
        }
    }
    Ok(())
}

/// Why uploading stopped, or a branch could not be uploaded to.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Failure {
    /// Another writer took the branch, or it was deleted.
    #[error("another writer took the branch")]
    Taken,
    /// A later placement, whose fencing token is higher, holds the branch (TASK-39.7).
    #[error("a later placement holds the branch")]
    Stale,
    /// Anything else, as it says.
    #[error("{0}")]
    Other(String),
}

impl From<extent_remote::Error> for Failure {
    fn from(error: extent_remote::Error) -> Self {
        match error {
            extent_remote::Error::Fenced | extent_remote::Error::Deleted => Self::Taken,
            extent_remote::Error::Stale => Self::Stale,
            other => Self::Other(other.to_string()),
        }
    }
}

impl From<anyhow::Error> for Failure {
    fn from(error: anyhow::Error) -> Self {
        Self::Other(format!("{error:#}"))
    }
}

/// How an uploader uploads, and where a store's reads keep what they fetch.
#[derive(Clone, Debug)]
pub struct Options {
    /// How long durable commits wait at most before they are uploaded.
    pub interval: Duration,
    /// How many commits may wait to be uploaded: the next waits, while object storage is out of
    /// reach, rather than the lag grow without bound (doc-7, decision 3).
    pub max_lag: u64,
    /// How many bytes the commits waiting to be uploaded may have written, likewise: what the local
    /// disk holds until uploaded.
    pub max_lag_bytes: u64,
    /// Where the parts of objects fetched are kept: the host's cache, which its stores share.
    pub cache: Option<Arc<Cache>>,
    /// The fencing token of a store's copy never placed ([`place`]): its host's placement's, as
    /// [`host_token`] has it.
    pub token: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(1),
            max_lag: 4096,
            max_lag_bytes: 1 << 30,
            cache: None,
            token: 0,
        }
    }
}

/// A store's uploader. Dropping it uploads what the store made durable, then stops.
pub struct Remote {
    shared: Arc<Shared>,
    /// Says the uploader stopped, once it has.
    stopped: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

struct Shared {
    store: Arc<Store<HostDir>>,
    fetcher: Arc<Fetcher>,
    progress: Mutex<Progress>,
    /// Notified as the progress changes: what waits for uploads listens to it, blocking or not
    /// (TASK-43.6).
    changed: Event,
    /// Wakes the uploader: a commit waits, or it is to stop.
    wake: tokio::sync::Notify,
    options: Options,
}

/// The GETs of extents' data a host has in flight: one connection to object storage gives roughly
/// 80-100 MB/s, so a host needs tens (doc-7, Reading); each holds a part, of 1 MiB.
static BUDGET: std::sync::LazyLock<Arc<Budget>> =
    std::sync::LazyLock::new(|| Budget::new(Limits { requests: 64, bytes: 64 << 20 }));

/// The PUTs of objects a host's stores upload, each holding its object's bytes while it goes:
/// bounded in bytes, so that a host whose stores are far behind, object storage out of reach for a
/// while, holds no more than these at once, whatever their number (TASK-79.19); and in number, as
/// its GETs are.
static UPLOADS: std::sync::LazyLock<Arc<Budget>> =
    std::sync::LazyLock::new(|| Budget::new(Limits { requests: 64, bytes: 512 << 20 }));

/// The runtime every store's uploader, and its fetches, run on (TASK-34): sized by the host, not
/// by how many stores it serves, as each store waits on object storage mostly, and does its disk
/// work in `block_in_place`, which leaves the runtime's threads to the others.
static RUNTIME: std::sync::LazyLock<tokio::runtime::Runtime> = std::sync::LazyLock::new(|| {
    let threads = std::thread::available_parallelism().map_or(2, |cpus| cpus.get().clamp(2, 8));
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(threads)
        .thread_name("fxdax remote")
        .enable_all()
        .build()
        .expect("the remotes' runtime")
});

/// Runs `work`, which blocks on the store's disk, on the remotes' runtime: on its thread, which
/// hands its other tasks to another meanwhile.
fn blocking<T>(work: impl FnOnce() -> T) -> T {
    tokio::task::block_in_place(work)
}

/// Reads the extents a store holds whose data is in object storage ([`Store::adopt`]), on the
/// remotes' runtime, which their uploads use too, under the host's budget of GETs.
pub struct Fetcher {
    runtime: tokio::runtime::Handle,
    parts: Arc<fetch::Fetcher<Bucket>>,
    /// The branch's writer, once it took the branch.
    writer: std::sync::OnceLock<Arc<Writer<Bucket>>>,
}

impl Fetcher {
    /// Where `extent`'s data begins in its object's data, if the branch's object storage holds it.
    pub fn offset(&self, extent: ExtentId) -> Option<u64> {
        Some(self.writer.get()?.location(extent)?.offset)
    }

    /// `len` bytes at `offset` in `extent`: as many as it holds.
    pub fn read(
        &self,
        extent: ExtentId,
        offset: u64,
        len: u64,
    ) -> impl std::future::Future<Output = Result<Vec<u8>, Failure>> + Send + use<> {
        let location = self.writer.get().and_then(|writer| writer.location(extent));
        let parts = self.parts.clone();
        let read = self.runtime.spawn(async move {
            let location = location.ok_or_else(|| {
                Failure::Other(format!("{extent:?} is not in the branch's object storage"))
            })?;
            Ok(parts.read(extent, &location, offset, len, Priority::Demand).await?)
        });
        async move { read.await.map_err(|err| Failure::Other(format!("fetching: {err}")))? }
    }
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
    /// The branch's lease, once its writer took it.
    lease: Option<Lease>,
    /// The store's latest commits not uploaded, as the uploader saw them, and when it did: the
    /// first is when the oldest commit waiting was seen first (TASK-50.5).
    seen: std::collections::VecDeque<(u64, SystemTime)>,
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
    /// The bytes of its commits not yet uploaded.
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

impl Remote {
    /// Uploads `store`'s branch to `bucket` as `options` say, as `holder` of its lease, which must
    /// differ from every other taking's. Returns once the branch holds what the store does.
    pub fn attach(
        store: Arc<Store<HostDir>>,
        bucket: Arc<Bucket>,
        holder: u64,
        options: Options,
    ) -> Result<Self, Failure> {
        let runtime = RUNTIME.handle().clone();
        let cache = options.cache.clone();
        let runs = runtime.clone();
        let spawn: fetch::Spawn = Arc::new(move |future| drop(runs.spawn(future)));
        let parts = fetch::Fetcher::new(bucket.clone(), BUDGET.clone(), cache).reading_ahead(spawn);
        let parts = Arc::new(parts);
        let fetcher = Arc::new(Fetcher { runtime, parts, writer: Default::default() });
        let shared = Arc::new(Shared {
            store,
            fetcher,
            progress: Mutex::new(Progress::default()),
            changed: Event::new(),
            wake: tokio::sync::Notify::new(),
            options,
        });
        let (started, start) = std::sync::mpsc::channel();
        let (stop, stopped) = std::sync::mpsc::channel();
        let uploader = shared.clone();
        // A task, not a thread of its own (TASK-34).
        RUNTIME.spawn(async move {
            uploader.run(bucket, holder, started).await;
            let _ = stop.send(());
        });
        match start.recv() {
            Ok(Ok(())) => Ok(Self { shared, stopped: Mutex::new(Some(stopped)) }),
            Ok(Err(err)) => {
                let _ = stopped.recv();
                Err(err)
            }
            Err(_) => Err(Failure::Other("the uploader stopped".into())),
        }
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
    ) -> impl std::future::Future<Output = Result<(), Failure>> + Send + use<> {
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

    /// What the uploader has done, and has left to do.
    pub fn status(&self) -> Result<UploadStatus, Failure> {
        let shared = &self.shared;
        shared.observe()?;
        let pending_bytes = shared.store.unreleased_bytes()?;
        let progress = shared.progress.lock().unwrap();
        Ok(UploadStatus {
            lease: progress.lease,
            uploaded: progress.uploaded,
            pending_bytes,
            oldest_pending: progress.seen.front().map(|(_, seen)| *seen),
            stalls: progress.stalls,
        })
    }

    /// Waits, if more commits wait to be uploaded than `Options::max_lag`, with `epoch`'s, or they
    /// wrote more than `Options::max_lag_bytes`, until they no longer do.
    pub fn wait_lag(&self, epoch: u64) -> Result<(), Failure> {
        let (shared, epoch) = (&self.shared, epoch.saturating_sub(self.shared.options.max_lag));
        if shared.ask(epoch, true)? {
            return Ok(());
        }
        shared.wait(|progress| shared.reached(progress, epoch, true))
    }

    /// Makes `child`, which the store forked from its branch with its commit `epoch`
    /// ([`Store::fork`]), a fork of the branch in object storage too, once the commits up to the
    /// fork are uploaded: its record names the branch, and the segment the fork's commit ends, as
    /// its origin, and a checkpoint at the fork is where its recovery starts. Its marker names the
    /// fork's commit, which the child inherits, as the last it uploaded.
    pub fn forked(&self, child: u64, epoch: u64) -> Result<(), Failure> {
        let shared = &self.shared;
        // Its commit's segment uploads now, if it is not yet uploaded.
        shared.ask(epoch, false)?;
        shared.wait(|progress| match progress.forked >= epoch {
            true => Some(Ok(())),
            false => progress.failed.clone().map(Err),
        })?;
        let lease = shared.progress.lock().unwrap().lease.expect("a writer took the branch");
        Marker::new(shared.store.dir().path(), child).write(Some(tag(lease.epoch, epoch)), [])
    }

    /// The lowest sequence number the store's guest may mint extents' IDs from, which the writer's
    /// epoch gives: no writer of the branch before minted from there.
    pub fn next_sequence(&self) -> u64 {
        self.shared.progress.lock().unwrap().next_sequence
    }

    /// What reads the extents the store holds whose data is in object storage.
    pub fn fetcher(&self) -> Arc<Fetcher> {
        self.shared.fetcher.clone()
    }

    /// How many commits waited for the lag to shrink.
    pub fn stalls(&self) -> u64 {
        self.shared.progress.lock().unwrap().stalls
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        self.shared.progress.lock().unwrap().stop = true;
        self.shared.changed.notify(usize::MAX);
        self.shared.wake.notify_one();
        if let Some(stopped) = self.stopped.get_mut().unwrap().take() {
            let _ = stopped.recv();
        }
    }
}

impl Shared {
    /// Notes the store's latest commit, if it isn't uploaded, as seen now.
    fn observe(&self) -> Result<(), Failure> {
        let epoch = self.store.epoch()?;
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
    fn ask(&self, epoch: u64, stall: bool) -> Result<bool, Failure> {
        self.observe()?;
        let mut progress = self.progress.lock().unwrap();
        if self.within(progress.uploaded, epoch, stall)? {
            return Ok(true);
        }
        progress.wanted = true;
        if stall {
            progress.stalls += 1;
            log::warn!(
                "fxdax: commits wait for object storage: {} not uploaded",
                epoch.saturating_sub(progress.uploaded)
            );
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

    /// Whether, `uploaded` up to, the commits up to `epoch` are, and, if `stall`, the bytes
    /// waiting within `Options::max_lag_bytes`: bytes only bound how far commits go ahead, not when
    /// one is durable.
    fn within(&self, uploaded: u64, epoch: u64, stall: bool) -> Result<bool, Failure> {
        let max_bytes = self.options.max_lag_bytes;
        Ok(uploaded >= epoch && (!stall || self.store.unreleased_bytes()? <= max_bytes))
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

    async fn run(
        &self,
        bucket: Arc<Bucket>,
        holder: u64,
        started: std::sync::mpsc::Sender<Result<(), Failure>>,
    ) {
        let interval = self.options.interval;
        let writer = match self.start(&bucket, holder).await {
            Ok(writer) => {
                let writer = Arc::new(writer);
                let _ = self.fetcher.writer.set(writer.clone());
                started.send(Ok(())).unwrap();
                writer
            }
            Err(err) => return started.send(Err(err)).unwrap(),
        };
        let mut segments = 0;
        let mut in_flight = InFlight::new(self.marker());
        // Whether the next gather closes the store's open segment: for a commit waiting for its
        // upload, or a fork, or as the interval is up; and whether the last took segments, after
        // which the store may hold more closed, which the next takes, closing none for them
        // (TASK-92.15).
        let (mut close, mut more) = (false, false);
        loop {
            if let Err(err) = self.observe() {
                return self.fail(&writer, err);
            }
            let stop = {
                let mut progress = self.progress.lock().unwrap();
                close |= std::mem::take(&mut progress.wanted);
                progress.stop
            };
            if (close || more || stop) && in_flight.len() < IN_FLIGHT {
                let gathered = blocking(|| self.gather(&writer, &mut in_flight, close || stop));
                let gathered = match gathered {
                    Ok(gathered) => gathered,
                    Err(err) => return self.fail(&writer, err),
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
                    return self.fail(&writer, Failure::Other("the uploader stopped".into()));
                }
            }
            let woken = tokio::time::timeout(interval, self.wake.notified());
            if in_flight.is_empty() {
                close |= woken.await.is_err();
                continue;
            }
            let uploaded =
                match futures::future::select(in_flight.next(), std::pin::pin!(woken)).await {
                    Either::Left((uploaded, _)) => uploaded.expect("an upload in flight"),
                    Either::Right((woken, _)) => {
                        close |= woken.is_err();
                        continue;
                    }
                };
            let acked = match uploaded {
                Ok(()) => self.acknowledged(&writer, &mut in_flight).await,
                Err(err) => Err(err.into()),
            };
            if let Err(err) = acked {
                return self.fail(&writer, err);
            }
            segments += 1;
            if segments % CHECKPOINT_EVERY == 0
                && let Err(err) = writer.checkpoint().await
            {
                log::warn!("fxdax: checkpointing branch {:x}: {err}", writer.branch());
            }
        }
    }

    /// Stops uploading, for `err`: commits waiting for uploads, and forks, fail with it.
    fn fail(&self, writer: &Writer<Bucket>, err: Failure) {
        let mut progress = self.progress.lock().unwrap();
        if !progress.stop {
            log::error!("fxdax: uploading branch {:x}: {err}", writer.branch());
        }
        progress.failed = Some(err);
        self.changed.notify(usize::MAX);
    }

    /// Takes the branch, and uploads what the store holds that it doesn't: unless the branch moved
    /// on elsewhere since the store last uploaded to it. A store that holds nothing takes what the
    /// branch holds. A fork's child whose record its parent's host crashed before giving it takes
    /// it from its parent's state ([`Shared::record`]).
    ///
    /// What the store does is decided twice ([`Shared::plan`]). Before the lease, from the
    /// branch's state then, so that a store plainly stale fences no writer; it changes nothing
    /// then. Then from the state the writer recovers once it holds the lease, which another writer
    /// may have committed to in between: what the store takes, and what it uploads, is that
    /// state's. A store whose branch moved on in between refuses then, having fenced that writer,
    /// whose commits acknowledged stay the branch's: two hosts attaching the branch at once. The
    /// writer recovers it on from the state found before, reading only the segments since
    /// (TASK-92.6).
    async fn start(&self, bucket: &Arc<Bucket>, holder: u64) -> Result<Writer<Bucket>, Failure> {
        bucket.probed(holder).await.map_err(Failure::Other)?;
        let branch = self.store.description().branch;
        let ours = self.marker().read()?;
        let held = blocking(|| self.store.snapshot())?;
        let found = match self.record(bucket, branch).await? {
            None => {
                branch::create(&**bucket, branch).await?;
                None
            }
            Some(record) => {
                let recovered = extent_remote::recover(&**bucket, branch, record.origin).await?;
                Self::plan(&recovered.state, &ours, &held)?;
                Some((record, recovered))
            }
        };
        let token = self.token()?;
        // Its state, once it holds the lease, recovered on from what was found before.
        let writer = match found {
            None => Writer::open(bucket.clone(), branch, holder, token).await?,
            Some((record, recovered)) => {
                Writer::open_recovered(bucket.clone(), record, recovered, holder, token).await?
            }
        };
        let writer = writer.uploading_within(UPLOADS.clone());
        let remote = writer.state();
        match Self::plan(&remote, &ours, &held)? {
            Plan::Diff => {}
            Plan::Take => blocking(|| self.take(&remote))?,
            Plan::Claim => blocking(|| self.marker().write(remote.last_commit, []))?,
        }
        let lease = Lease { token, epoch: writer.epoch(), taken: SystemTime::now() };
        {
            let mut progress = self.progress.lock().unwrap();
            (progress.next_sequence, progress.lease) =
                (writer.chain().next_sequence(), Some(lease));
        }
        // What the store holds that the branch doesn't, as of a commit after which the store
        // closes its segments for this writer: in segments of their own, if it changes anything,
        // closed on the writer's chain, which then stamps the store's footers, which follow them.
        loop {
            let snapshot = blocking(|| self.store.snapshot())?;
            let mut chain = writer.chain();
            let tag = chain.tag(snapshot.epoch);
            // Their footers stamped, their data read only once the store takes the chain, as
            // they upload.
            let segments: Vec<Footer> = (Self::diff(&remote, &snapshot, tag).into_iter())
                .map(|plan| plan.close(&mut chain))
                .collect();
            // A commit since, and the snapshot is stale.
            if !self.store.upload(chain, snapshot.epoch)? {
                continue;
            }
            // What it holds is durable before it is uploaded.
            blocking(|| self.store.flush())?;
            if !segments.is_empty() {
                self.send(&writer, tag, segments).await?;
            }
            // It made no fork.
            let mut progress = self.progress.lock().unwrap();
            progress.uploaded_through(snapshot.epoch);
            progress.forked_through(snapshot.epoch);
            return Ok(writer);
        }
    }

    /// The record of the store's branch, `branch`, if it has one. A fork's child has none yet if
    /// its parent's host crashed once the fork's segment landed, before giving it its record,
    /// which the parent's next writer gives it, from the forks the parent's state names. The child
    /// attaching first takes it so too: a branch of its own, it would fail every writer of its
    /// parent after, giving it its record. A child whose fork its parent's state names not, its
    /// segment never uploaded, or made while the parent's branch was kept nowhere, has a branch of
    /// its own.
    async fn record(&self, bucket: &Bucket, branch: u64) -> Result<Option<Record>, Failure> {
        if let Some(record) = branch::record(bucket, branch).await? {
            return Ok(Some(record));
        }
        let Some(parent) = self.store.parent() else { return Ok(None) };
        // A parent deleted gave the children of its forks their records first.
        let Some(parents) = branch::record(bucket, parent).await?.filter(|record| !record.deleted)
        else {
            return Ok(None);
        };
        let state = extent_remote::recover(bucket, parent, parents.origin).await?.state;
        let Some(fork) = state.forks.iter().find(|fork| fork.child == branch) else {
            return Ok(None);
        };
        branch::record_fork(bucket, branch, Origin { parent, at: fork.at }).await?;
        Ok(branch::record(bucket, branch).await?)
    }

    /// What a store whose marker names `ours`, holding `held`, does with its branch, whose state is
    /// `state`: it uploads what it holds that the branch doesn't if the branch's last commit is one
    /// it names, or the branch has none; it takes what the branch holds if it holds nothing and
    /// names no commit; it claims the branch's last commit as its own if it names none, its marker
    /// lost, but holds what the branch holds, extents and root, as a store does that took the
    /// branch, or a fork's child, its host crashing before the marker named the commit: it has
    /// nothing to upload, so reverts nothing. Otherwise the branch moved on elsewhere since the
    /// store last uploaded to it, and the store is stale.
    fn plan(state: &State, ours: &[u64], held: &Snapshot) -> Result<Plan, Failure> {
        let empty = held.extents.is_empty() && held.root.is_empty();
        let holds = || {
            let extents = state.extents.iter().map(|(extent, location)| (*extent, location.len));
            extents.eq(held.extents.iter().copied())
                && state.root.as_deref().unwrap_or_default() == held.root
        };
        match state.last_commit {
            None => Ok(Plan::Diff),
            Some(last) if ours.contains(&last) => Ok(Plan::Diff),
            Some(_) if ours.is_empty() && empty => Ok(Plan::Take),
            Some(_) if ours.is_empty() && holds() => Ok(Plan::Claim),
            Some(last) => Err(Failure::Other(format!(
                "branch {:x} moved on elsewhere (commit {last:#x}): its store here is stale",
                state.branch
            ))),
        }
    }

    /// The segments, planned, that make `remote`, the branch's state, hold what `snapshot`, the
    /// store's, does: the extents the store holds that the branch doesn't, a [`BATCH`] of them at
    /// most in each, those of [`LARGE`] bytes or more stored on their own; the last makes the
    /// commit, tagged `tag`, which frees those the branch holds that the store doesn't, and sets
    /// the store's root, if the branch's differs. None if nothing differs. A crash before the last
    /// is uploaded leaves the branch with extents its root does not reach, its last commit as it
    /// was: the store attaching again uploads the rest.
    fn diff(remote: &State, snapshot: &Snapshot, tag: u64) -> Vec<batch::Plan> {
        let added: Vec<_> = (snapshot.extents.iter())
            .filter(|(extent, _)| !remote.extents.contains_key(extent))
            .copied()
            .collect();
        let held: BTreeSet<ExtentId> = snapshot.extents.iter().map(|(extent, _)| *extent).collect();
        let frees: Vec<_> =
            remote.extents.keys().filter(|extent| !held.contains(extent)).copied().collect();
        let differs = remote.root.as_deref() != Some(&snapshot.root[..]);
        let root = (differs && !snapshot.root.is_empty()).then(|| snapshot.root.clone());
        if added.is_empty() && frees.is_empty() && root.is_none() {
            return Vec::new();
        }
        let mut plans = Vec::new();
        let mut plan = batch::Plan::default();
        for (extent, len) in added {
            if plan.bytes() > 0 && plan.bytes() + len > BATCH {
                plans.push(std::mem::take(&mut plan));
            }
            plan.seal(extent, len, len >= LARGE);
        }
        plan.commit(tag, frees, root);
        plans.push(plan);
        plans
    }

    /// Adds the segments the store closed whose commits are durable to the writer, to be uploaded
    /// as they are, after those uploading (`in_flight`), as many as may upload at once with them:
    /// the rest stay in the store, unread, until those are uploaded. If `close`, the store closes
    /// its open segment first if fewer wait: for a commit waiting for its upload, a fork, the
    /// interval up, or the uploader stopping; for no other, as the commits no one waits for wait
    /// in it for those, rather than each go in a segment of its own (TASK-92.15).
    fn gather(
        &self,
        writer: &Arc<Writer<Bucket>>,
        in_flight: &mut InFlight,
        close: bool,
    ) -> Result<Gathered, Failure> {
        let room = IN_FLIGHT.saturating_sub(in_flight.len());
        // What a waiter wants is durable; then closed, if fewer segments wait than may upload, and
        // what the closing closed durable too.
        self.store.flush()?;
        let mut closed = self.store.closed(room)?;
        let closing = close && closed.len() < room;
        if closing {
            self.store.close_now()?;
            self.store.flush()?;
            closed.extend(self.store.closed(room - closed.len())?);
        }
        let mut segments = Vec::with_capacity(closed.len());
        for closed in closed {
            let (tag, forks) = (closed.tag(), !closed.footer.forks.is_empty());
            let own = closed.own.iter().map(|(key, file)| Ok((key.clone(), stored(file)?)));
            let own = own.collect::<Result<_, Failure>>()?;
            let objects = Objects { footer: closed.footer, segment: stored(&closed.file)?, own };
            let seq = writer.add_segment(objects)?;
            segments.push(Uploading { seq, number: closed.number, tag, forks });
        }
        let took = !segments.is_empty();
        in_flight.upload(writer, segments)?;
        Ok(Gathered { took, closed: closing })
    }

    /// Acknowledges what the writer has uploaded: the commits of the segments it acknowledges are
    /// uploaded, and the store's segments may go; then the forks they name are made, with a
    /// checkpoint at them, which their children's recovery starts from.
    async fn acknowledged(
        &self,
        writer: &Writer<Bucket>,
        in_flight: &mut InFlight,
    ) -> Result<(), Failure> {
        let through = writer.acknowledge().await?;
        let acked = blocking(|| in_flight.acknowledged(through, writer.last_commit()))?;
        let Some(epoch) = acked.last().map(Uploading::epoch) else { return Ok(()) };
        blocking(|| {
            self.store.release(epoch)?;
            for segment in &acked {
                self.store.uploaded(segment.number)?;
            }
            Ok::<_, Failure>(())
        })?;
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
            self.progress.lock().unwrap().forked_through(epoch);
            self.changed.notify(usize::MAX);
        }
        Ok(())
    }

    /// Uploads `segments`, those the commit tagged `tag` ends, planned and closed on the writer's
    /// chain, as many at once as the store's do, each reading the extents it holds from the store
    /// as it goes, which keeps for its uploader what commits free meanwhile; and acknowledges them,
    /// the marker naming the commit while they upload.
    async fn send(
        &self,
        writer: &Writer<Bucket>,
        tag: u64,
        segments: Vec<Footer>,
    ) -> Result<(), Failure> {
        let acked = writer.last_commit();
        blocking(|| self.marker().write(acked, [tag]))?;
        let store = self.store.clone();
        let read: batch::Read = Arc::new(move |extent, data| {
            let read = blocking(|| store.read_kept(extent, 0, data));
            read.map_err(|err| std::io::Error::other(format!("{extent:?}: {err:#}")))
        });
        let seqs = (segments.into_iter())
            .map(|footer| writer.add_segment(batch::objects(footer, read.clone())))
            .collect::<Result<Vec<_>, _>>()?;
        let uploads = futures::stream::iter(seqs).map(|seq| writer.upload(seq));
        uploads.buffer_unordered(IN_FLIGHT).try_collect::<()>().await?;
        writer.acknowledge().await?;
        blocking(|| self.marker().write(writer.last_commit(), []))
    }

    /// The marker of the store's branch.
    fn marker(&self) -> Marker {
        Marker::new(self.store.dir().path(), self.store.description().branch)
    }

    /// The fencing token of the store's copy of its branch ([`place`]): `Options::token` if it was
    /// never placed.
    fn token(&self) -> Result<u64, Failure> {
        let placed = placed_token(self.store.dir().path(), self.store.description().branch)?;
        Ok(placed.unwrap_or(self.options.token))
    }

    /// Takes what the branch holds into the store, which holds nothing: a branch opened on a host
    /// without it. The store holds the branch's extents, whose data stays in object storage, read
    /// as the guest reads it ([`Fetcher`]).
    fn take(&self, state: &extent_remote::State) -> Result<(), Failure> {
        for (extent, location) in &state.extents {
            self.store.adopt(*extent, location.len)?;
        }
        self.store.commit(Some(state.root.as_deref().unwrap_or_default()))?;
        self.marker().write(state.last_commit, [])
    }
}

/// The bytes of `file`, a segment's the store closed or an extent's of its own it names, read only
/// as its upload goes.
fn stored(file: &Arc<HostFile>) -> Result<Body, Failure> {
    let size = file.size().map_err(|err| Failure::Other(format!("a file to upload: {err}")))?;
    let file = file.clone();
    Ok(Body::new(size, move || blocking(|| read_whole(&*file))))
}

/// What a gather did ([`Shared::gather`]).
struct Gathered {
    /// Whether it took segments the store closed: the store may have closed more meanwhile.
    took: bool,
    /// Whether it closed the store's open segment, as asked: what was committed before is in the
    /// segments the store closed, taken, or to be taken next.
    closed: bool,
}

/// What attaching a store does with its branch ([`Shared::plan`]).
enum Plan {
    /// Uploads what the store holds that the branch doesn't.
    Diff,
    /// Takes what the branch holds into the store, which holds nothing.
    Take,
    /// Names the branch's last commit in the store's marker, which was lost, as its own: the store
    /// holds what the branch holds.
    Claim,
}

/// The marker of a branch of a store, in the store's directory ([`marker`]): the commits it names,
/// by their tags, the last uploaded (`-` for none) and those uploading.
struct Marker {
    dir: PathBuf,
    branch: u64,
}

impl Marker {
    fn new(dir: &Path, branch: u64) -> Self {
        Self { dir: dir.to_owned(), branch }
    }

    /// The commits it names: the last uploaded, and those uploading; none if there is no marker.
    fn read(&self) -> Result<Vec<u64>, Failure> {
        let path = self.dir.join(marker(self.branch));
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(Failure::Other(format!("reading {}: {err}", path.display()))),
        };
        text.split_whitespace()
            .filter(|word| *word != "-")
            .map(|word| u64::from_str_radix(word, 16))
            .collect::<Result<_, _>>()
            .map_err(|err| Failure::Other(format!("{}: {err}", path.display())))
    }

    /// Names `acked`, the last commit uploaded, if any, and `uploading`, durably.
    fn write(
        &self,
        acked: Option<u64>,
        uploading: impl IntoIterator<Item = u64>,
    ) -> Result<(), Failure> {
        let mut text = acked.map_or("-".to_owned(), |tag| format!("{tag:x}"));
        for tag in uploading {
            text += &format!(" {tag:x}");
        }
        text.push('\n');
        let written = write_durably(&self.dir, &marker(self.branch), text.as_bytes());
        written.map_err(|err| Failure::Other(format!("recording the upload: {err}")))
    }
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

/// The segments uploading, in order, the futures of their uploads, and the marker naming them: it
/// names every segment that may land before any upload goes, and an acknowledgement takes exactly
/// the segments the writer acknowledged.
struct InFlight {
    marker: Marker,
    segments: VecDeque<Uploading>,
    uploads: FuturesOrdered<BoxFuture<'static, Result<(), extent_remote::Error>>>,
}

impl InFlight {
    fn new(marker: Marker) -> Self {
        Self { marker, segments: VecDeque::new(), uploads: FuturesOrdered::new() }
    }

    /// Uploads `segments`, which `writer` took, after those uploading: once the marker names them
    /// with every one uploading, the last uploaded being `writer`'s last commit. Nothing changes if
    /// the marker cannot be written.
    fn upload(
        &mut self,
        writer: &Arc<Writer<Bucket>>,
        segments: Vec<Uploading>,
    ) -> Result<(), Failure> {
        if segments.is_empty() {
            return Ok(());
        }
        let tags = self.tags().chain(segments.iter().map(|segment| segment.tag));
        self.marker.write(writer.last_commit(), tags)?;
        for segment in segments {
            let (writer, seq) = (writer.clone(), segment.seq);
            self.uploads.push_back(async move { writer.upload(seq).await }.boxed());
            self.segments.push_back(segment);
        }
        Ok(())
    }

    /// The next upload done, in order.
    fn next(
        &mut self,
    ) -> impl Future<Output = Option<Result<(), extent_remote::Error>>> + Unpin + '_ {
        self.uploads.next()
    }

    /// The segments the writer acknowledged, `through` the last, which uploads no more: the marker
    /// names, from then on, `last_commit`, the branch's last, and those still uploading.
    fn acknowledged(
        &mut self,
        through: Option<u64>,
        last_commit: Option<u64>,
    ) -> Result<Vec<Uploading>, Failure> {
        let taken = |segment: &&Uploading| through.is_some_and(|through| segment.seq <= through);
        let count = self.segments.iter().take_while(taken).count();
        let acked: Vec<_> = self.segments.drain(..count).collect();
        if !acked.is_empty() {
            self.marker.write(last_commit, self.tags())?;
        }
        Ok(acked)
    }

    /// The tags of the segments uploading.
    fn tags(&self) -> impl Iterator<Item = u64> + '_ {
        self.segments.iter().map(|segment| segment.tag)
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
    use crate::{ExtentId, ExtentStore, VolatileSlice};
    use extent_remote::cache::Cache;
    use extent_remote::store::Store as Bucket;
    use extent_remote::{branch, read, recover};
    use object_store::memory::InMemory;
    use std::sync::Arc;
    use std::time::Duration;

    const BLOCK: usize = ExtentStore::BLOCK_SIZE as usize;

    /// Uploads only when a commit waits.
    fn idle() -> super::Options {
        super::Options { interval: Duration::from_secs(3600), ..super::Options::default() }
    }

    fn pattern(seed: u8, len: usize) -> Vec<u8> {
        (0..len).map(|i| seed ^ (i % 251) as u8).collect()
    }

    fn write(store: &ExtentStore, sequence: u64, data: &[u8]) -> ExtentId {
        let extent = ExtentId { branch: store.branch(), sequence };
        store.write(extent, 0, &[VolatileSlice::from(data)]).unwrap();
        extent
    }

    /// A branch's extents, by sequence number, with their data, and its root.
    type Uploaded = (Vec<(u64, Vec<u8>)>, Option<Vec<u8>>);

    /// The branch's extents and root, read back from `bucket`.
    fn uploaded(bucket: &Arc<Bucket>, branch: u64) -> Uploaded {
        futures::executor::block_on(async {
            let state = recover(&**bucket, branch, None).await.unwrap().state;
            let mut extents = Vec::new();
            for (extent, location) in &state.extents {
                extents.push((extent.sequence, read(bucket, *extent, location).await.unwrap()));
            }
            (extents, state.root)
        })
    }

    /// A store's extents, its root, and its commits made durable are uploaded; extents written and
    /// freed between two uploads never are.
    #[test]
    fn uploads_what_the_store_makes_durable() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExtentStore::create(dir.path().join("store"), 1 << 30).unwrap();
        let branch = store.branch();
        write(&store, 1, &pattern(1, BLOCK));
        store.commit(Some(b"first")).unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), "branches"));
        // What it held before is uploaded on attaching.
        store.attach_remote(bucket.clone(), 1, idle()).unwrap();
        assert_eq!(
            uploaded(&bucket, branch),
            (vec![(1, pattern(1, BLOCK))], Some(b"first".to_vec()))
        );

        let large = pattern(3, 4 << 20);
        write(&store, 2, &pattern(2, 2 * BLOCK));
        write(&store, 3, &large);
        write(&store, 4, &pattern(4, BLOCK));
        store.commit_ordered(None).unwrap();
        store.free(ExtentId { branch, sequence: 1 }).unwrap();
        store.free(ExtentId { branch, sequence: 4 }).unwrap();
        store.commit(Some(b"second")).unwrap();
        assert_eq!(
            uploaded(&bucket, branch),
            (vec![(2, pattern(2, 2 * BLOCK)), (3, large.clone())], Some(b"second".to_vec()))
        );
        // Extent 4, written and freed between two uploads, never was.
        let keys = futures::executor::block_on(extent_remote::Bucket::list(&*bucket, "")).unwrap();
        assert_eq!(keys.iter().filter(|meta| meta.key.starts_with("extents/")).count(), 1);
        let segments = keys.iter().filter(|meta| meta.key.starts_with("segments/")).count();

        // Opened again, the store's uploader finds the branch as it left it.
        drop(store);
        let store = ExtentStore::open(dir.path().join("store")).unwrap();
        store.attach_remote(bucket.clone(), 2, idle()).unwrap();
        // Its guest mints from the second writer's range.
        let info = store.info().unwrap();
        assert_eq!(info.sequence_floor, 2 << extent_remote::EPOCH_SHIFT);
        // What the store holds ends far below: the guest looks for what a crash left there only.
        assert!(info.next_sequence < 1 << extent_remote::EPOCH_SHIFT, "{info:?}");
        let keys = futures::executor::block_on(extent_remote::Bucket::list(&*bucket, "")).unwrap();
        // Its fence, and nothing more.
        assert_eq!(
            keys.iter().filter(|meta| meta.key.starts_with("segments/")).count(),
            segments + 1
        );
        write(&store, 5, &pattern(5, BLOCK));
        store.commit(None).unwrap();
        let (extents, root) = uploaded(&bucket, branch);
        assert_eq!(extents.iter().map(|(sequence, _)| *sequence).collect::<Vec<_>>(), [2, 3, 5]);
        assert_eq!(root, Some(b"second".to_vec()));
    }

    /// The footers of `branch`'s segments in `bucket`, in order.
    fn footers(bucket: &Bucket, branch: u64) -> Vec<extent_remote::layout::Footer> {
        use extent_remote::layout;
        futures::executor::block_on(async {
            let listed = extent_remote::Bucket::list(bucket, &layout::segments(branch)).await;
            let mut footers = Vec::new();
            for meta in listed.unwrap() {
                let object = extent_remote::Bucket::get(bucket, &meta.key).await.unwrap();
                footers.push(layout::footer(&object.bytes).unwrap());
            }
            footers
        })
    }

    /// TASK-79.19: what a store holds that its branch doesn't, attaching, goes in segments of a
    /// batch at most each, its extents stored on their own counted, rather than in one holding it
    /// all; the last makes the commit, and sets the root. Its writer's fence goes first.
    #[test]
    fn attaches_upload_what_the_branch_lacks_in_batches() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExtentStore::create(dir.path().join("store"), 1 << 30).unwrap();
        let branch = store.branch();
        let large = super::BATCH / super::LARGE + 2;
        let mut written = Vec::new();
        for sequence in 1..=large {
            let data = pattern(sequence as u8, super::LARGE as usize);
            write(&store, sequence, &data);
            written.push((sequence, data));
        }
        write(&store, large + 1, &pattern(0, BLOCK));
        written.push((large + 1, pattern(0, BLOCK)));
        store.commit(Some(b"before")).unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        store.attach_remote(bucket.clone(), 1, idle()).unwrap();

        let footers = footers(&bucket, branch);
        let [fence, first, last] = &footers[..] else {
            panic!("{} segments: {footers:?}", footers.len())
        };
        assert_eq!((fence.commits.len(), fence.extents.len(), fence.own.len()), (0, 0, 0));
        let bytes = |footer: &extent_remote::layout::Footer| {
            let held: u64 = footer.extents.iter().map(|(_, _, len)| len).sum();
            held + footer.own.iter().map(|(_, location)| location.len).sum::<u64>()
        };
        assert_eq!(bytes(first), super::BATCH);
        assert_eq!(bytes(last), 2 * super::LARGE + BLOCK as u64);
        assert_eq!((first.commits.len(), &first.root), (0, &None));
        assert_eq!((last.commits.len(), &last.root), (1, &Some(b"before".to_vec())));
        assert_eq!(uploaded(&bucket, branch), (written, Some(b"before".to_vec())));
    }

    /// TASK-79.19: a gather takes no more of the segments the store closed than may upload at once,
    /// with those uploading; the rest it leaves in the store, until those it took are uploaded.
    #[test]
    fn gathers_take_what_may_upload_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::host_store::create(&dir.path().join("store"), 1 << 30, 7).unwrap();
        let store = Arc::new(store);
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let remote = super::Remote::attach(store.clone(), bucket.clone(), 1, idle()).unwrap();
        // Each commit closes its segment.
        let closed = 2 * super::IN_FLIGHT + 1;
        for sequence in 1..=closed as u64 {
            let extent = super::ExtentId::new(7, sequence);
            store.write(extent, 0, &pattern(sequence as u8, BLOCK)).unwrap();
            store.close_segment().unwrap();
            store.commit(None).unwrap();
        }
        let shared = &remote.shared;
        let writer = shared.fetcher.writer.get().unwrap().clone();
        let mut in_flight = super::InFlight::new(shared.marker());
        let mut taken = Vec::new();
        while shared.gather(&writer, &mut in_flight, true).unwrap().took {
            taken.push(in_flight.len());
            super::RUNTIME.block_on(async {
                while let Some(uploaded) = in_flight.next().await {
                    uploaded.unwrap();
                }
                shared.acknowledged(&writer, &mut in_flight).await.unwrap();
            });
        }
        assert_eq!(taken, [super::IN_FLIGHT, super::IN_FLIGHT, 1]);
        let (extents, _) = uploaded(&bucket, 7);
        let sequences: Vec<_> = extents.iter().map(|(sequence, _)| *sequence).collect();
        assert_eq!(sequences, (1..=closed as u64).collect::<Vec<_>>());
    }

    /// A store's closed segments are the objects of its branch in the bucket, byte for byte, as
    /// are its extents stored on their own (doc-7, decision 1): the uploader puts its files.
    #[test]
    fn uploaded_segments_are_the_stores_files() {
        use extent_remote::layout::{self, Footer};
        let dir = tempfile::tempdir().unwrap();
        let store = ExtentStore::create(dir.path().join("store"), 1 << 30).unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        store.attach_remote(bucket.clone(), 1, idle()).unwrap();
        let branch = store.branch();
        write(&store, 1, &pattern(1, BLOCK));
        write(&store, 2, &pattern(2, 4 << 20));
        store.commit(Some(b"one")).unwrap();
        write(&store, 3, &pattern(3, 2 * BLOCK));
        store.commit(Some(b"two")).unwrap();

        // The store's files of `kind`, by their contents.
        let files = |kind: &str| -> Vec<Vec<u8>> {
            let path = dir.path().join("store").join(kind).join(format!("{branch:016x}"));
            let mut files: Vec<_> = std::fs::read_dir(path)
                .unwrap()
                .map(|entry| std::fs::read(entry.unwrap().path()).unwrap())
                .collect();
            files.sort();
            files
        };
        // The bucket's objects under `prefix`.
        let objects = |prefix: String| -> Vec<Vec<u8>> {
            futures::executor::block_on(async {
                let listed = extent_remote::Bucket::list(&*bucket, &prefix).await.unwrap();
                let mut objects = Vec::new();
                for meta in listed {
                    let object = extent_remote::Bucket::get(&*bucket, &meta.key).await.unwrap();
                    objects.push(object.bytes);
                }
                objects
            })
        };
        // The branch's segments but its writer's fence, which holds no commit.
        let mut segments: Vec<_> = objects(layout::segments(branch))
            .into_iter()
            .filter(|bytes| !layout::footer::<Footer>(bytes).unwrap().commits.is_empty())
            .collect();
        segments.sort();
        assert_eq!(segments.len(), 2);
        assert_eq!(segments, files("segments"));
        let mut own = objects(layout::extents(branch));
        own.sort();
        assert_eq!(own.len(), 1);
        assert_eq!(own, files("extents"));
        assert_eq!(
            uploaded(&bucket, branch),
            (
                vec![(1, pattern(1, BLOCK)), (2, pattern(2, 4 << 20)), (3, pattern(3, 2 * BLOCK))],
                Some(b"two".to_vec())
            )
        );
    }

    /// A commit waits for its upload no longer than it takes: once another writer has the branch,
    /// a durable commit fails.
    #[test]
    fn durable_commits_fail_once_another_takes_the_branch() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExtentStore::create(dir.path().join("store"), 1 << 30).unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        store.attach_remote(bucket.clone(), 1, idle()).unwrap();
        write(&store, 1, &pattern(1, BLOCK));
        store.commit(None).unwrap();
        let other = extent_remote::Writer::open(bucket.clone(), store.branch(), 2, 0);
        futures::executor::block_on(other).unwrap();
        write(&store, 2, &pattern(2, BLOCK));
        assert_eq!(store.commit(None), Err(crate::Errno::EROFS));
    }

    /// TASK-39.7: a store's copy takes its branch with its placement's fencing token: a later
    /// placement's, higher, fences it, its commits failing, and it takes the branch back no more,
    /// though its store is as current as the branch; a placement later still takes it.
    #[test]
    fn later_placements_fence_earlier_ones() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let (first_path, second_path) = (dir.path().join("first"), dir.path().join("second"));
        let first = ExtentStore::create(&first_path, 1 << 30).unwrap();
        let branch = first.branch();
        ExtentStore::place(&first_path, 1).unwrap();
        first.attach_remote(bucket.clone(), 1, idle()).unwrap();
        write(&first, 1, &pattern(1, BLOCK));
        first.commit(Some(b"first")).unwrap();

        // The branch moves to another host, a later placement, while the first still runs.
        ExtentStore::create_for_branch(&second_path, 1 << 30, branch).unwrap();
        ExtentStore::place(&second_path, 2).unwrap();
        let second = ExtentStore::open(&second_path).unwrap();
        second.attach_remote(bucket.clone(), 2, idle()).unwrap();
        assert_eq!(second.root(), Ok(b"first".to_vec()));
        write(&first, 2, &pattern(2, BLOCK));
        assert_eq!(first.commit(None), Err(crate::Errno::EROFS));
        drop((first, second));

        // The first, restarted, takes it back no more: its token is the earlier.
        let stale = ExtentStore::open(&first_path).unwrap();
        let refused = stale.attach_remote(bucket.clone(), 3, idle());
        assert_eq!(refused.err(), Some(super::Failure::Stale));
        drop(stale);
        let lease = futures::executor::block_on(extent_remote::Bucket::get(
            &*bucket,
            &extent_remote::layout::lease(branch),
        ));
        let lease: extent_remote::layout::Lease =
            extent_remote::layout::decode(&lease.unwrap().bytes).unwrap();
        assert_eq!((lease.holder, lease.token), (2, 2));

        // Placed later still, it does.
        ExtentStore::place(&first_path, 3).unwrap();
        let later = ExtentStore::open(&first_path).unwrap();
        later.attach_remote(bucket.clone(), 4, idle()).unwrap();
        assert_eq!(later.root(), Ok(b"first".to_vec()));
    }

    /// TASK-45.1: a placement cut off deletes no branch from under a later one: its delete fails
    /// (`ESTALE`), leaving the branch in the bucket, which the later placement goes on writing; the
    /// later placement's delete deletes it.
    #[test]
    fn earlier_placements_delete_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let (first_path, second_path) = (dir.path().join("first"), dir.path().join("second"));
        let first = ExtentStore::create(&first_path, 1 << 30).unwrap();
        let branch = first.branch();
        ExtentStore::place(&first_path, 1).unwrap();
        first.attach_remote(bucket.clone(), 1, idle()).unwrap();
        write(&first, 1, &pattern(1, BLOCK));
        first.commit(Some(b"first")).unwrap();
        drop(first);
        // The branch moves to another host, a later placement.
        ExtentStore::create_for_branch(&second_path, 1 << 30, branch).unwrap();
        ExtentStore::place(&second_path, 2).unwrap();
        let second = ExtentStore::open(&second_path).unwrap();
        second.attach_remote(bucket.clone(), 2, idle()).unwrap();

        let refused = ExtentStore::delete(&first_path, Some(&bucket));
        assert_eq!(refused.err().and_then(|err| err.errno()), Some(crate::Errno::ESTALE));
        write(&second, 2, &pattern(2, BLOCK));
        second.commit(Some(b"second")).unwrap();
        drop(second);
        let state = futures::executor::block_on(extent_remote::recover(&*bucket, branch, None))
            .unwrap()
            .state;
        assert_eq!(state.root, Some(b"second".to_vec()));

        ExtentStore::delete(&second_path, Some(&bucket)).unwrap();
        let deleted = futures::executor::block_on(extent_remote::branch::record(&*bucket, branch));
        assert!(deleted.unwrap().unwrap().deleted, "the branch lives");
    }

    /// TASK-39.7: a store's copy never placed takes its branch with its host's token, as a store
    /// restored or taken on a host is.
    #[test]
    fn unplaced_copies_take_their_hosts_token() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let store = ExtentStore::create(dir.path().join("store"), 1 << 30).unwrap();
        let options = super::Options { token: 9, ..idle() };
        store.attach_remote(bucket.clone(), 1, options).unwrap();
        let lease = futures::executor::block_on(extent_remote::Bucket::get(
            &*bucket,
            &extent_remote::layout::lease(store.branch()),
        ));
        let lease = extent_remote::layout::Lease::decode(&lease.unwrap().bytes).unwrap();
        assert_eq!(lease.token, 9);
    }

    /// TASK-39.7: waiting for uploads returns once every commit so far is in the branch, and
    /// refuses a store without a remote.
    #[test]
    fn waits_for_uploads() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let store = ExtentStore::create(dir.path().join("store"), 1 << 30).unwrap();
        assert_eq!(store.wait_uploaded(), Err(crate::Errno::ENOTSUP));
        store.attach_remote(bucket.clone(), 1, idle()).unwrap();
        write(&store, 1, &pattern(1, BLOCK));
        store.commit_ordered(Some(b"ordered")).unwrap();
        // Not uploaded yet: the uploader waits an hour, unless asked.
        assert_eq!(uploaded(&bucket, store.branch()).1, None);
        store.wait_uploaded().unwrap();
        assert_eq!(
            uploaded(&bucket, store.branch()),
            (vec![(1, pattern(1, BLOCK))], Some(b"ordered".to_vec()))
        );
    }

    /// Which writes a [`Gated`] store holds.
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Hold {
        Nothing,
        /// The writes of segments.
        Segments,
        /// The writes of segments of this number or more.
        SegmentsFrom(u64),
        /// The write of the object at this key.
        Key(String),
    }

    impl Hold {
        fn holds(&self, key: &str) -> bool {
            use extent_remote::layout::{Kind, kind};
            match self {
                Hold::Nothing => false,
                Hold::Segments => key.starts_with("segments/"),
                Hold::SegmentsFrom(from) => {
                    matches!(kind(key), Kind::Segment { seq, .. } if seq >= *from)
                }
                Hold::Key(held) => held == key,
            }
        }
    }

    /// An object store whose requests fail while it is shut, and whose writes wait while they are
    /// held.
    #[derive(Debug)]
    struct Gated {
        store: Arc<dyn object_store::ObjectStore>,
        shut: std::sync::atomic::AtomicBool,
        hold: tokio::sync::watch::Sender<Hold>,
        /// The writes waiting.
        held: std::sync::atomic::AtomicUsize,
    }

    impl std::fmt::Display for Gated {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "gated")
        }
    }

    impl Gated {
        fn new() -> Self {
            Self::over(Arc::new(InMemory::new()))
        }

        /// The objects of `store`, gated.
        fn over(store: Arc<dyn object_store::ObjectStore>) -> Self {
            Self {
                store,
                shut: Default::default(),
                hold: tokio::sync::watch::channel(Hold::Nothing).0,
                held: Default::default(),
            }
        }

        fn check(&self) -> object_store::Result<()> {
            if self.shut.load(std::sync::atomic::Ordering::Relaxed) {
                Err(object_store::Error::Generic { store: "gated", source: "shut".into() })
            } else {
                Ok(())
            }
        }

        /// Waits until `count` writes wait, as they come in other threads, and fails if they
        /// don't within 30 s.
        fn wait_held(&self, count: usize) {
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            let held = || self.held.load(std::sync::atomic::Ordering::Relaxed);
            while held() < count && std::time::Instant::now() < deadline {
                std::thread::yield_now();
            }
            assert_eq!(held(), count);
        }
    }

    #[async_trait::async_trait]
    impl object_store::ObjectStore for Gated {
        async fn put_opts(
            &self,
            location: &object_store::path::Path,
            payload: object_store::PutPayload,
            opts: object_store::PutOptions,
        ) -> object_store::Result<object_store::PutResult> {
            self.check()?;
            let key = location.as_ref();
            if self.hold.borrow().holds(key) {
                let mut hold = self.hold.subscribe();
                self.held.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let _ = hold.wait_for(|hold| !hold.holds(key)).await;
                self.held.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            }
            self.store.put_opts(location, payload, opts).await
        }

        async fn put_multipart_opts(
            &self,
            location: &object_store::path::Path,
            opts: object_store::PutMultipartOptions,
        ) -> object_store::Result<Box<dyn object_store::MultipartUpload>> {
            self.check()?;
            self.store.put_multipart_opts(location, opts).await
        }

        async fn get_opts(
            &self,
            location: &object_store::path::Path,
            options: object_store::GetOptions,
        ) -> object_store::Result<object_store::GetResult> {
            self.check()?;
            self.store.get_opts(location, options).await
        }

        fn delete_stream(
            &self,
            locations: futures::stream::BoxStream<
                'static,
                object_store::Result<object_store::path::Path>,
            >,
        ) -> futures::stream::BoxStream<'static, object_store::Result<object_store::path::Path>>
        {
            self.store.delete_stream(locations)
        }

        fn list(
            &self,
            prefix: Option<&object_store::path::Path>,
        ) -> futures::stream::BoxStream<'static, object_store::Result<object_store::ObjectMeta>>
        {
            self.store.list(prefix)
        }

        async fn list_with_delimiter(
            &self,
            prefix: Option<&object_store::path::Path>,
        ) -> object_store::Result<object_store::ListResult> {
            self.store.list_with_delimiter(prefix).await
        }

        async fn copy_opts(
            &self,
            from: &object_store::path::Path,
            to: &object_store::path::Path,
            options: object_store::CopyOptions,
        ) -> object_store::Result<()> {
            self.store.copy_opts(from, to, options).await
        }
    }

    /// While object storage is out of reach, commits go on until the lag reaches its bound, as
    /// `options` set it, two commits of a block each, then wait; they go on once it is in reach
    /// again.
    fn waits_at_the_lag_bound(options: super::Options) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(ExtentStore::create(dir.path().join("store"), 1 << 30).unwrap());
        let gated = Arc::new(Gated::new());
        let bucket = Arc::new(Bucket::new(gated.clone(), ""));
        store.attach_remote(bucket.clone(), 1, options).unwrap();
        gated.shut.store(true, std::sync::atomic::Ordering::Relaxed);
        for sequence in 1..=2 {
            write(&store, sequence, &pattern(sequence as u8, BLOCK));
            store.commit_ordered(None).unwrap();
        }
        let third = {
            let store = store.clone();
            std::thread::spawn(move || {
                write(&store, 3, &pattern(3, BLOCK));
                store.commit_ordered(None)
            })
        };
        let remote = store.remote().unwrap();
        while remote.stalls() == 0 {
            std::thread::yield_now();
        }
        assert!(!third.is_finished());
        gated.shut.store(false, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(third.join().unwrap(), Ok(()));
        store.commit(None).unwrap();
        let (extents, _) = uploaded(&bucket, store.branch());
        assert_eq!(extents.iter().map(|(sequence, _)| *sequence).collect::<Vec<_>>(), [1, 2, 3]);
    }

    /// Segments upload at once: a commit made while those before it upload goes up with them.
    #[test]
    fn segments_upload_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(ExtentStore::create(dir.path().join("store"), 1 << 30).unwrap());
        let gated = Arc::new(Gated::new());
        let bucket = Arc::new(Bucket::new(gated.clone(), ""));
        store.attach_remote(bucket.clone(), 1, idle()).unwrap();
        gated.hold.send_replace(Hold::Segments);
        let commits: Vec<_> = (1..=3)
            .map(|sequence| {
                let committing = store.clone();
                let commit = std::thread::spawn(move || {
                    write(&committing, sequence, &pattern(sequence as u8, BLOCK));
                    committing.commit(None)
                });
                gated.wait_held(sequence as usize);
                commit
            })
            .collect();
        gated.hold.send_replace(Hold::Nothing);
        for commit in commits {
            assert_eq!(commit.join().unwrap(), Ok(()));
        }
        let (extents, _) = uploaded(&bucket, store.branch());
        assert_eq!(extents.iter().map(|(sequence, _)| *sequence).collect::<Vec<_>>(), [1, 2, 3]);
    }

    /// TASK-92.15: an ordered commit that no one waits for stays in the store's open segment while
    /// the uploader takes the segments that commits waiting for their uploads closed: it closes
    /// the open segment for those waiting, or when it is due, not each time an upload is done.
    /// Here four segments, one for each commit waiting, upload, held in the bucket; a commit is
    /// made that no one waits for; then they land, one after another: the store closes none for
    /// it, which uploads once one waits for it.
    #[test]
    fn uploads_done_close_no_segment_for_commits_no_one_waits_for() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(ExtentStore::create(dir.path().join("store"), 1 << 30).unwrap());
        let branch = store.branch();
        let gated = Arc::new(Gated::new());
        let bucket = Arc::new(Bucket::new(gated.clone(), ""));
        store.attach_remote(bucket.clone(), 1, idle()).unwrap();
        // The writer's fence is segment 0; the commits' are 1 to 4.
        gated.hold.send_replace(Hold::SegmentsFrom(1));
        let waiting: Vec<_> = (1..=super::IN_FLIGHT as u64)
            .map(|sequence| {
                let committing = store.clone();
                let commit = std::thread::spawn(move || {
                    write(&committing, sequence, &pattern(sequence as u8, BLOCK));
                    committing.commit(None)
                });
                gated.wait_held(sequence as usize);
                commit
            })
            .collect();
        let ordered = super::IN_FLIGHT as u64 + 1;
        write(&store, ordered, &pattern(ordered as u8, BLOCK));
        store.commit_ordered(None).unwrap();
        // The first lands alone: its commit is acknowledged; then the rest.
        let mut waiting = waiting.into_iter();
        gated.hold.send_replace(Hold::SegmentsFrom(2));
        assert_eq!(waiting.next().unwrap().join().unwrap(), Ok(()));
        gated.hold.send_replace(Hold::Nothing);
        for commit in waiting {
            assert_eq!(commit.join().unwrap(), Ok(()));
        }
        // The store's segments closed, framed: those of the commits waiting.
        let closed =
            std::fs::read_dir(dir.path().join("store/segments").join(format!("{branch:016x}")));
        assert_eq!(closed.unwrap().count(), super::IN_FLIGHT);
        store.wait_uploaded().unwrap();
        let (extents, _) = uploaded(&bucket, branch);
        let sequences: Vec<_> = extents.iter().map(|(sequence, _)| *sequence).collect();
        assert_eq!(sequences, (1..=ordered).collect::<Vec<_>>());
    }

    /// A branch survives the loss of its host. Cut off from object storage, the host's store goes
    /// on committing, ordered, until the lag reaches its bound, `max_lag` commits, then waits.
    /// Lost then, its store, its uploader and its disk, its commits made durable, which waited for
    /// their uploads, are all in the branch, which another host's empty store takes, losing the
    /// lag's commits at most; and writes on. In the objects of `store` under `prefix`.
    fn survives_its_hosts_loss(store: Arc<dyn object_store::ObjectStore>, prefix: &str) {
        use std::sync::atomic::Ordering::Relaxed;
        let dir = tempfile::tempdir().unwrap();
        let gated = Arc::new(Gated::over(store.clone()));
        let max_lag = 2;
        let options =
            super::Options { interval: Duration::from_millis(1), max_lag, ..Default::default() };
        let lost = Arc::new(ExtentStore::create(dir.path().join("lost"), 1 << 30).unwrap());
        let branch = lost.branch();
        lost.attach_remote(Arc::new(Bucket::new(gated.clone(), prefix)), 1, options).unwrap();
        write(&lost, 1, &pattern(1, BLOCK));
        lost.commit(Some(b"durable")).unwrap();
        gated.shut.store(true, Relaxed);
        for sequence in 2..=1 + max_lag {
            write(&lost, sequence, &pattern(sequence as u8, BLOCK));
            lost.commit_ordered(Some(b"ordered")).unwrap();
        }
        let waiting = {
            let lost = lost.clone();
            std::thread::spawn(move || {
                write(&lost, 2 + max_lag, &pattern(9, BLOCK));
                lost.commit_ordered(None)
            })
        };
        let remote = lost.remote().unwrap();
        while remote.stalls() == 0 {
            std::thread::yield_now();
        }
        assert!(!waiting.is_finished(), "a commit past the lag went on");
        // The host is lost: its store and uploader stop where they are, never reaching object
        // storage again, and its disk is gone.
        std::mem::forget(lost);
        drop(waiting);
        std::fs::remove_dir_all(dir.path().join("lost")).unwrap();

        let bucket = Arc::new(Bucket::new(store, prefix));
        let taken = ExtentStore::create_for_branch(dir.path().join("taken"), 1 << 30, branch);
        let taken = taken.unwrap();
        taken.attach_remote(bucket.clone(), 2, idle()).unwrap();
        assert_eq!(taken.root(), Ok(b"durable".to_vec()));
        let mut data = vec![0; BLOCK];
        let durable = ExtentId { branch, sequence: 1 };
        taken.read(durable, 0, &mut [crate::VolatileSliceMut::from(&mut data[..])]).unwrap();
        assert_eq!(data, pattern(1, BLOCK));
        let info = taken.info().unwrap();
        let sequence = info.next_sequence.max(info.sequence_floor);
        write(&taken, sequence, &pattern(3, BLOCK));
        taken.commit(Some(b"taken")).unwrap();
        drop(taken);
        // The lag's commits, and no more, lost. Read back with a reactor, which a real bucket's
        // client needs.
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (extents, root) = {
            let _entered = runtime.enter();
            uploaded(&bucket, branch)
        };
        let sequences: Vec<u64> = extents.iter().map(|(sequence, _)| *sequence).collect();
        assert_eq!(sequences, [1, sequence]);
        assert_eq!(root, Some(b"taken".to_vec()));
    }

    #[test]
    fn branches_survive_their_hosts_loss() {
        survives_its_hosts_loss(Arc::new(InMemory::new()), "");
    }

    /// Bytes that don't compress, from a xorshift generator seeded by `seed`.
    fn noise(seed: u64, len: usize) -> Vec<u8> {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64 ^ seed;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect()
    }

    /// TASK-92.6: a store taking a branch its host lost, the branch's segments since its last
    /// checkpoint many, recovers the branch once, its lease taken between the start and the rest:
    /// by the tails of its segments, which hold their footers, a window of them at once, not by
    /// the segments whole. The bucket, which the lost host's store probed, it probes not again.
    #[test]
    fn takes_recover_their_branches_once_by_their_segments_tails() {
        use extent_remote::layout;
        use extent_remote::store::Request;
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let lost = ExtentStore::create(dir.path().join("lost"), 1 << 30).unwrap();
        let branch = lost.branch();
        lost.attach_remote(bucket.clone(), 1, idle()).unwrap();
        // Its writer's fence, then a segment of a MiB for each commit, uploaded as it is durable.
        for sequence in 1..=4 {
            write(&lost, sequence, &noise(sequence, 1 << 20));
            lost.commit(None).unwrap();
        }
        // The host is lost, its uploader stopping where it is, with no checkpoint.
        std::mem::forget(lost);

        let taken = ExtentStore::create_for_branch(dir.path().join("taken"), 1 << 30, branch);
        let taken = taken.unwrap();
        let before = bucket.stats();
        taken.attach_remote(bucket.clone(), 2, idle()).unwrap();
        let after = bucket.stats();
        let asked: Vec<_> = (Request::ALL.iter())
            .map(|&request| (request.name(), after.requests(request) - before.requests(request)))
            .collect();
        let sizes: std::collections::BTreeMap<_, _> =
            futures::executor::block_on(extent_remote::Bucket::list(&*bucket, ""))
                .unwrap()
                .into_iter()
                .map(|meta| (meta.key, meta.size))
                .collect();
        let tails: u64 = (0..=4)
            .map(|seq| sizes[&layout::segment(branch, seq)].min(extent_remote::object::TAIL))
            .sum();
        let (record, lease) = (sizes[&layout::record(branch)], sizes[&layout::lease(branch)]);
        // The record, before the lease and after; the lease, taken, and read again once the fence
        // is in. The segments' tails: one, then two, then four at once, the last two past the
        // end; and the first past the end again, once the lease is taken. The checkpoints listed
        // as each of those finds the end, and the segments as the fence goes in.
        assert_eq!(
            (asked, after.downloaded - before.downloaded),
            (
                vec![
                    ("get", 4),
                    ("get-range", 8),
                    ("head", 0),
                    ("list", 4),
                    ("create", 1),
                    ("update", 1),
                    ("overwrite", 0),
                    ("delete", 0),
                ],
                tails + 2 * record + 2 * lease
            )
        );
        let mut data = vec![0; 1 << 20];
        let extent = ExtentId { branch, sequence: 4 };
        taken.read(extent, 0, &mut [crate::VolatileSliceMut::from(&mut data[..])]).unwrap();
        assert_eq!(data, noise(4, 1 << 20));
    }

    /// As `branches_survive_their_hosts_loss`, in the bucket FXDAX_S3_URL names: RustFS's
    /// (scripts/test-rustfs.sh), or GCS's, from a GCE host.
    #[test]
    #[ignore]
    fn rustfs_branches_survive_their_hosts_loss() {
        let url = std::env::var("FXDAX_S3_URL").expect("FXDAX_S3_URL names the bucket to use");
        let (store, prefix) = extent_remote::store::object_store_at(&url).unwrap();
        // A prefix of the run's own, in a bucket runs share.
        let prefix = format!("{prefix}/{:016x}", rand::random::<u64>());
        survives_its_hosts_loss(store, prefix.trim_start_matches('/'));
    }

    #[test]
    fn commits_wait_at_the_lag_bound() {
        let interval = Duration::from_millis(1);
        waits_at_the_lag_bound(super::Options { interval, max_lag: 2, ..Default::default() });
    }

    #[test]
    fn commits_wait_at_the_lag_bound_in_bytes() {
        let interval = Duration::from_millis(1);
        let options =
            super::Options { interval, max_lag_bytes: 2 * BLOCK as u64, ..Default::default() };
        waits_at_the_lag_bound(options);
    }

    /// A branch opens on a host without it: an empty store of it takes what the branch holds, and
    /// writes on; the store it came from, opened again, finds the branch moved on, and refuses.
    #[test]
    fn opens_branches_elsewhere() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let first = ExtentStore::create(dir.path().join("first"), 1 << 30).unwrap();
        let branch = first.branch();
        first.attach_remote(bucket.clone(), 1, idle()).unwrap();
        let large = pattern(2, 4 << 20);
        write(&first, 1, &pattern(1, BLOCK));
        write(&first, 2, &large);
        first.commit(Some(b"root")).unwrap();
        drop(first);

        let second = ExtentStore::create_for_branch(dir.path().join("second"), 1 << 30, branch);
        let second = second.unwrap();
        second.attach_remote(bucket.clone(), 2, idle()).unwrap();
        assert_eq!(second.root(), Ok(b"root".to_vec()));
        let mut data = vec![0; 4 << 20];
        let extent = ExtentId { branch, sequence: 2 };
        second.read(extent, 0, &mut [crate::VolatileSliceMut::from(&mut data[..])]).unwrap();
        assert_eq!(data, large);
        let info = second.info().unwrap();
        let sequence = info.next_sequence.max(info.sequence_floor);
        write(&second, sequence, &pattern(3, BLOCK));
        second.commit(None).unwrap();
        drop(second);

        let stale = ExtentStore::open(dir.path().join("first")).unwrap();
        let refused = stale.attach_remote(bucket.clone(), 3, idle());
        assert!(
            matches!(refused, Err(super::Failure::Other(ref err)) if err.contains("moved on")),
            "{refused:?}"
        );
        let (extents, root) = uploaded(&bucket, branch);
        assert_eq!(
            extents.iter().map(|(sequence, _)| *sequence).collect::<Vec<_>>(),
            [1, 2, sequence]
        );
        assert_eq!(root, Some(b"root".to_vec()));
    }

    /// Attaches `store` to the objects of `memory` through a [`Gated`] store holding the write of
    /// its branch's lease, in a thread of its own: once the write waits, runs `meanwhile`, then lets
    /// it go. Returns the store, and what attaching it gave.
    fn attach_racing(
        store: ExtentStore,
        memory: &Arc<InMemory>,
        holder: u64,
        meanwhile: impl FnOnce(),
    ) -> (ExtentStore, Result<(), super::Failure>) {
        let gated = Arc::new(Gated::over(memory.clone()));
        let lease = extent_remote::layout::lease(store.branch());
        gated.hold.send_replace(Hold::Key(lease));
        let bucket = Arc::new(Bucket::new(gated.clone(), ""));
        let attaching = std::thread::spawn(move || {
            let attached = store.attach_remote(bucket, holder, idle());
            (store, attached)
        });
        gated.wait_held(1);
        meanwhile();
        gated.hold.send_replace(Hold::Nothing);
        attaching.join().unwrap()
    }

    /// TASK-79.4: an empty store attaching takes the branch as of its lease, not as of what it
    /// found before: the commit another writer made meanwhile, which frees the extent the branch
    /// held before and writes another, is the branch's, and the store's.
    #[test]
    fn takes_take_what_the_branch_holds_once_leased() {
        let dir = tempfile::tempdir().unwrap();
        let memory = Arc::new(InMemory::new());
        let bucket = Arc::new(Bucket::new(memory.clone(), ""));
        let first = ExtentStore::create(dir.path().join("first"), 1 << 30).unwrap();
        let branch = first.branch();
        first.attach_remote(bucket.clone(), 1, idle()).unwrap();
        let before = write(&first, 1, &pattern(1, BLOCK));
        first.commit(Some(b"before")).unwrap();

        let empty = ExtentStore::create_for_branch(dir.path().join("second"), 1 << 30, branch);
        let (second, attached) = attach_racing(empty.unwrap(), &memory, 2, || {
            write(&first, 2, &pattern(2, BLOCK));
            first.free(before).unwrap();
            first.commit(Some(b"meanwhile")).unwrap();
        });
        assert_eq!(attached, Ok(()));
        assert_eq!(second.root(), Ok(b"meanwhile".to_vec()));
        let mut data = vec![0; BLOCK];
        let written = ExtentId { branch, sequence: 2 };
        second.read(written, 0, &mut [crate::VolatileSliceMut::from(&mut data[..])]).unwrap();
        assert_eq!(data, pattern(2, BLOCK));
        assert_eq!(
            uploaded(&bucket, branch),
            (vec![(2, pattern(2, BLOCK))], Some(b"meanwhile".to_vec()))
        );
    }

    /// TASK-79.4: a store attaching whose last commit was the branch's before its lease, but is not
    /// once it holds it, another writer having committed meanwhile, refuses: the commit is the
    /// branch's, not undone by what the store holds.
    #[test]
    fn stores_whose_branch_moved_on_before_their_lease_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let memory = Arc::new(InMemory::new());
        let bucket = Arc::new(Bucket::new(memory.clone(), ""));
        let first = ExtentStore::create(dir.path().join("first"), 1 << 30).unwrap();
        let branch = first.branch();
        first.attach_remote(bucket.clone(), 1, idle()).unwrap();
        write(&first, 1, &pattern(1, BLOCK));
        first.commit(Some(b"before")).unwrap();
        drop(first);
        // The branch moves to another host, which takes it.
        let taken = ExtentStore::create_for_branch(dir.path().join("taken"), 1 << 30, branch);
        let taken = taken.unwrap();
        taken.attach_remote(bucket.clone(), 2, idle()).unwrap();

        let stale = ExtentStore::open(dir.path().join("first")).unwrap();
        let info = taken.info().unwrap();
        let sequence = info.next_sequence.max(info.sequence_floor);
        let (_, refused) = attach_racing(stale, &memory, 3, || {
            write(&taken, sequence, &pattern(2, BLOCK));
            taken.commit(Some(b"meanwhile")).unwrap();
        });
        assert!(
            matches!(refused, Err(super::Failure::Other(ref err)) if err.contains("moved on")),
            "{refused:?}"
        );
        let (extents, root) = uploaded(&bucket, branch);
        assert_eq!(extents, [(1, pattern(1, BLOCK)), (sequence, pattern(2, BLOCK))]);
        assert_eq!(root, Some(b"meanwhile".to_vec()));
    }

    /// A fork of a store kept in object storage is a fork of its branch there, recorded once the
    /// parent uploaded through it: it inherits the parent's extents rather than upload them again,
    /// and what either commits after is its own.
    #[test]
    fn forks_inherit_their_parents_extents() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let parent = ExtentStore::create(dir.path().join("parent"), 1 << 30).unwrap();
        parent.attach_remote(bucket.clone(), 1, idle()).unwrap();
        let large = pattern(2, 4 << 20);
        let small = write(&parent, 1, &pattern(1, BLOCK));
        let own = write(&parent, 2, &large);
        // Committed, not yet durable: the fork makes it so, and uploads it, first.
        parent.commit_ordered(Some(b"root")).unwrap();
        let child = parent.branch() + 1;
        let checkpoints = || {
            let prefix = extent_remote::layout::checkpoints(parent.branch());
            futures::executor::block_on(extent_remote::Bucket::list(&*bucket, &prefix)).unwrap()
        };
        assert_eq!(checkpoints().len(), 0);
        parent.fork(dir.path().join("child"), child).unwrap();
        // A checkpoint at the fork, which the child recovers from.
        assert_eq!(checkpoints().len(), 1);
        write(&parent, 3, &pattern(3, BLOCK));
        parent.commit(None).unwrap();

        let record = futures::executor::block_on(branch::record(&*bucket, child)).unwrap().unwrap();
        let origin = record.origin.unwrap();

        assert_eq!(origin.parent, parent.branch());
        let inherited = |state: &extent_remote::State| {
            let parents = [
                format!("segments/{:016x}/", parent.branch()),
                format!("extents/{:016x}/", parent.branch()),
            ];
            state
                .extents
                .iter()
                .filter(|(_, location)| {
                    parents.iter().any(|prefix| location.key.starts_with(prefix))
                })
                .map(|(extent, _)| extent.sequence)
                .collect::<Vec<_>>()
        };

        let forked = ExtentStore::open(dir.path().join("child")).unwrap();
        forked.attach_remote(bucket.clone(), 2, idle()).unwrap();
        let state =
            futures::executor::block_on(recover(&*bucket, child, Some(origin))).unwrap().state;
        assert_eq!(inherited(&state), [small.sequence, own.sequence]);
        assert_eq!(state.root, Some(b"root".to_vec()));
        let mut data = vec![0; 4 << 20];
        forked.read(own, 0, &mut [crate::VolatileSliceMut::from(&mut data[..])]).unwrap();
        assert_eq!(data, large);
        let info = forked.info().unwrap();
        let sequence = info.next_sequence.max(info.sequence_floor);
        let mine = write(&forked, sequence, &pattern(4, BLOCK));
        forked.commit(None).unwrap();
        drop(forked);
        let state =
            futures::executor::block_on(recover(&*bucket, child, Some(origin))).unwrap().state;
        assert_eq!(inherited(&state), [small.sequence, own.sequence]);
        let held: Vec<_> = state.extents.keys().map(|extent| extent.sequence).collect();
        assert_eq!(held, [small.sequence, own.sequence, mine.sequence]);
        // And one when the parent's uploader stops, the store closing.
        let parent_branch = parent.branch();
        drop(parent);
        let prefix = extent_remote::layout::checkpoints(parent_branch);
        let listed = futures::executor::block_on(extent_remote::Bucket::list(&*bucket, &prefix));
        assert_eq!(listed.unwrap().len(), 2);
    }

    /// TASK-79.39: a fork's child takes its branch through its marker, which names the fork's
    /// commit, the last the child inherits, though a later commit's segment is acknowledged with
    /// the fork's.
    #[test]
    fn forks_name_their_commit_though_later_ones_are_acknowledged_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let gated = Arc::new(Gated::new());
        let bucket = Arc::new(Bucket::new(gated.clone(), ""));
        let parent = Arc::new(ExtentStore::create(dir.path().join("parent"), 1 << 30).unwrap());
        parent.attach_remote(bucket.clone(), 1, idle()).unwrap();
        let branch = parent.branch();
        write(&parent, 1, &pattern(1, BLOCK));
        parent.commit(Some(b"root")).unwrap();
        // The fork's segment, after the writer's fence and the commit's, lands after the next.
        let segment = |seq| extent_remote::layout::segment(branch, seq);
        let (forking, next) = (segment(2), segment(3));
        gated.hold.send_replace(Hold::Key(forking));
        let child = branch + 1;
        let fork = {
            let (parent, path) = (parent.clone(), dir.path().join("child"));
            std::thread::spawn(move || parent.fork(path, child))
        };
        gated.wait_held(1);
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let later = {
            let parent = parent.clone();
            std::thread::spawn(move || {
                write(&parent, 2, &pattern(2, BLOCK));
                parent.commit(None)
            })
        };
        let landed =
            || futures::executor::block_on(extent_remote::Bucket::head(&*bucket, &next)).is_ok();
        while !landed() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(landed(), "the commit after the fork never uploaded");
        gated.hold.send_replace(Hold::Nothing);
        assert_eq!(later.join().unwrap(), Ok(()));
        fork.join().unwrap().unwrap();

        let forked = ExtentStore::open(dir.path().join("child")).unwrap();
        assert_eq!(forked.attach_remote(bucket.clone(), 2, idle()), Ok(()));
        assert_eq!(forked.root(), Ok(b"root".to_vec()));
    }

    /// TASK-79.20: a store that took its branch, its host crashing before its marker named the
    /// branch's last commit, holds what the branch holds: attaching again, the branch is its own
    /// still, and its marker names the commit again.
    #[test]
    fn takes_whose_marker_was_lost_attach_again() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let first = ExtentStore::create(dir.path().join("first"), 1 << 30).unwrap();
        let branch = first.branch();
        first.attach_remote(bucket.clone(), 1, idle()).unwrap();
        write(&first, 1, &pattern(1, BLOCK));
        first.commit(Some(b"root")).unwrap();
        drop(first);
        let path = dir.path().join("taken");
        let taken = ExtentStore::create_for_branch(&path, 1 << 30, branch).unwrap();
        taken.attach_remote(bucket.clone(), 2, idle()).unwrap();
        drop(taken);
        let marker = path.join(super::marker(branch));
        let named = std::fs::read_to_string(&marker).unwrap();
        std::fs::remove_file(&marker).unwrap();

        let taken = ExtentStore::open(&path).unwrap();
        assert_eq!(taken.attach_remote(bucket.clone(), 3, idle()), Ok(()));
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), named);
        assert_eq!(taken.root(), Ok(b"root".to_vec()));
    }

    /// Forks `parent`, attached to `bucket`, to `child`, whose handle is at `path`, the parent
    /// holding an extent and a root: returns the child's record.
    fn forked(
        parent: &ExtentStore,
        bucket: &Arc<Bucket>,
        path: &std::path::Path,
        child: u64,
    ) -> extent_remote::layout::Record {
        write(parent, 1, &pattern(1, BLOCK));
        parent.commit(Some(b"root")).unwrap();
        parent.fork(path, child).unwrap();
        futures::executor::block_on(branch::record(&**bucket, child)).unwrap().unwrap()
    }

    /// TASK-79.20: so does a fork's child whose marker was lost, its parent's host crashing once
    /// the fork was made in object storage, before the marker named the fork's commit: its store
    /// holds what its branch inherited.
    #[test]
    fn forks_whose_marker_was_lost_attach() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let parent = ExtentStore::create(dir.path().join("parent"), 1 << 30).unwrap();
        parent.attach_remote(bucket.clone(), 1, idle()).unwrap();
        let child = parent.branch() + 1;
        forked(&parent, &bucket, &dir.path().join("child"), child);
        std::fs::remove_file(dir.path().join("parent").join(super::marker(child))).unwrap();

        let forked = ExtentStore::open(dir.path().join("child")).unwrap();
        assert_eq!(forked.attach_remote(bucket.clone(), 2, idle()), Ok(()));
        assert_eq!(forked.root(), Ok(b"root".to_vec()));
    }

    /// TASK-79.20: a fork's child whose record, and so marker, was lost, its parent's host crashing
    /// once the fork's segment landed, before it gave the child its record, takes its record from
    /// its parent's state, as the parent's next writer would: a fork still, which the parent,
    /// attaching again, finds so.
    #[test]
    fn forks_whose_record_was_lost_record_themselves() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let parent = ExtentStore::create(dir.path().join("parent"), 1 << 30).unwrap();
        parent.attach_remote(bucket.clone(), 1, idle()).unwrap();
        let child = parent.branch() + 1;
        let record = forked(&parent, &bucket, &dir.path().join("child"), child);
        let key = extent_remote::layout::record(child);
        futures::executor::block_on(extent_remote::Bucket::delete(&*bucket, &key)).unwrap();
        std::fs::remove_file(dir.path().join("parent").join(super::marker(child))).unwrap();

        let forked = ExtentStore::open(dir.path().join("child")).unwrap();
        assert_eq!(forked.attach_remote(bucket.clone(), 2, idle()), Ok(()));
        let recorded = futures::executor::block_on(branch::record(&*bucket, child)).unwrap();
        assert_eq!(recorded, Some(record));
        assert_eq!(forked.root(), Ok(b"root".to_vec()));
        drop((forked, parent));
        let parent = ExtentStore::open(dir.path().join("parent")).unwrap();
        assert_eq!(parent.attach_remote(bucket.clone(), 3, idle()), Ok(()));
    }

    /// A fork made while its parent kept its branch nowhere, the parent attaching after, is no fork
    /// of the parent's branch, whose state names it not: its child takes a record of its own, and
    /// uploads what it inherited.
    #[test]
    fn forks_their_parents_branch_names_not_take_records_of_their_own() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let parent = ExtentStore::create(dir.path().join("parent"), 1 << 30).unwrap();
        write(&parent, 1, &pattern(1, BLOCK));
        parent.commit(Some(b"root")).unwrap();
        let child = parent.branch() + 1;
        parent.fork(dir.path().join("child"), child).unwrap();
        parent.attach_remote(bucket.clone(), 1, idle()).unwrap();

        let forked = ExtentStore::open(dir.path().join("child")).unwrap();
        assert_eq!(forked.attach_remote(bucket.clone(), 2, idle()), Ok(()));
        let record = futures::executor::block_on(branch::record(&*bucket, child)).unwrap();
        assert_eq!(record.map(|record| record.origin), Some(None));
        assert_eq!(
            uploaded(&bucket, child),
            (vec![(1, pattern(1, BLOCK))], Some(b"root".to_vec()))
        );
    }

    /// A marker names the last commit uploaded, or none, and those uploading, and reads back so;
    /// with none written, it names nothing.
    #[test]
    fn markers_name_what_was_uploaded_and_what_uploads() {
        let dir = tempfile::tempdir().unwrap();
        let marker = super::Marker::new(dir.path(), 7);
        let text = || std::fs::read_to_string(dir.path().join(super::marker(7))).unwrap();
        assert_eq!(marker.read(), Ok(vec![]));
        marker.write(None, []).unwrap();
        assert_eq!((text(), marker.read()), ("-\n".to_owned(), Ok(vec![])));
        marker.write(Some(0x1a), [0x1b, 0x2c]).unwrap();
        assert_eq!((text(), marker.read()), ("1a 1b 2c\n".to_owned(), Ok(vec![0x1a, 0x1b, 0x2c])));
    }

    /// The marker names every segment uploading, those before too, before any of their uploads
    /// goes; an acknowledgement takes every segment through the writer's last acknowledged, and the
    /// marker names, from then on, the branch's last commit and those still uploading. One that
    /// takes none leaves the marker as it was.
    #[test]
    fn segments_uploading_are_marked_until_acknowledged() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let writer = futures::executor::block_on(async {
            branch::create(&*bucket, 7).await.unwrap();
            extent_remote::Writer::open(bucket.clone(), 7, 1, 0).await.unwrap()
        });
        let writer = Arc::new(writer);
        let path = dir.path().join(super::marker(7));
        let text = || std::fs::read_to_string(&path).unwrap();
        let mut in_flight = super::InFlight::new(super::Marker::new(dir.path(), 7));
        let uploading = |seq, tag| super::Uploading { seq, number: seq + 10, tag, forks: false };
        in_flight.upload(&writer, vec![uploading(3, 0x13)]).unwrap();
        assert_eq!((text(), in_flight.len()), ("- 13\n".to_owned(), 1));
        in_flight.upload(&writer, vec![uploading(4, 0x14), uploading(5, 0x15)]).unwrap();
        assert_eq!((text(), in_flight.len()), ("- 13 14 15\n".to_owned(), 3));

        assert!(in_flight.acknowledged(None, None).unwrap().is_empty());
        let acked = in_flight.acknowledged(Some(4), Some(0x14)).unwrap();
        let acked: Vec<_> = acked.iter().map(|segment| (segment.seq, segment.number)).collect();
        assert_eq!(acked, [(3, 13), (4, 14)]);
        assert_eq!(text(), "14 15\n");
        std::fs::write(&path, "kept\n").unwrap();
        assert!(in_flight.acknowledged(Some(4), Some(0x14)).unwrap().is_empty());
        assert_eq!(text(), "kept\n");
        // Their uploads are in flight still, which the run loop finishes.
        assert_eq!(in_flight.len(), 3);
    }

    /// A store of a branch taken elsewhere keeps what it reads from the bucket in the host's cache,
    /// and reads it there next.
    #[test]
    fn reads_keep_what_they_fetch() {
        let dir = tempfile::tempdir().unwrap();
        let memory = Arc::new(InMemory::new());
        let bucket = Arc::new(Bucket::new(memory.clone(), ""));
        let first = ExtentStore::create(dir.path().join("first"), 1 << 30).unwrap();
        let branch = first.branch();
        first.attach_remote(bucket.clone(), 1, idle()).unwrap();
        let data = pattern(2, 2 << 20);
        let extent = write(&first, 1, &data);
        first.commit(None).unwrap();
        drop(first);

        let cache = Arc::new(Cache::new(&dir.path().join("cache"), 1 << 30, 0).unwrap());
        let second = ExtentStore::create_for_branch(dir.path().join("second"), 1 << 30, branch);
        let second = second.unwrap();
        let options = super::Options { cache: Some(cache.clone()), ..idle() };
        second.attach_remote(bucket.clone(), 2, options).unwrap();
        let read = || {
            let mut read = vec![0; 2 << 20];
            second.read(extent, 0, &mut [crate::VolatileSliceMut::from(&mut read[..])]).unwrap();
            read
        };
        assert_eq!(read(), data);
        // Its parts, as they are stored: compressed.
        assert_ne!(cache.bytes(), 0);
        assert!(cache.bytes() < (2 << 20) / 100, "{} bytes kept", cache.bytes());

        // With the branch's segments gone from the bucket, its data is read from the cache.
        futures::executor::block_on(async {
            use futures::StreamExt as _;
            use object_store::{ObjectStore as _, ObjectStoreExt as _};
            let prefix = object_store::path::Path::from("segments");
            for meta in memory.list(Some(&prefix)).collect::<Vec<_>>().await {
                memory.delete(&meta.unwrap().location).await.unwrap();
            }
        });
        assert_eq!(read(), data);
    }
}
