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
//! Every failure of its writer's stops the uploader for good, and the commits waiting for uploads
//! fail with it from then on: its writer fenced, its branch deleted, or an earlier writer's segment
//! found where its next goes (`extent_remote::Error::Displaced`), which leaves the segments the
//! store closed numbered for a place the branch no longer has. Only attaching the store anew takes
//! the branch again.

use event_listener::{Event, Listener as _};
use extent_remote::batch::Unframed;
use extent_remote::cache::Cache;
use extent_remote::fetch::{self, Budget, Limits, Priority};
use extent_remote::store::{Store as Bucket, probe};
use extent_remote::{Batch, Commit, Objects, State, Writer, branch};
use extent_store::{HostDir, Snapshot, Store};
use futures::StreamExt as _;
use futures::future::Either;
use futures::stream::FuturesOrdered;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use storage_device::ExtentId;

/// The size from which an extent is stored on its own (doc-7).
const LARGE: u64 = 4 << 20;

/// How many segments upload at once.
const IN_FLIGHT: usize = 4;

/// How many segments go between checkpoints.
const CHECKPOINT_EVERY: u64 = 64;

/// The file in a store's directory that names the last commit of a branch of the store it uploaded,
/// and those it is uploading: a branch whose last commit is none of them, another host moved on.
fn marker(branch: u64) -> String {
    format!("remote-{branch:016x}")
}

/// The file in a store's directory that holds the fencing token of its copy of a branch of it
/// ([`place`]).
fn fence(branch: u64) -> String {
    format!("fence-{branch:016x}")
}

/// Places the store directory `dir`'s copy of `branch` (TASK-39.7): records, durably, the fencing
/// token its writer takes the branch's lease with from then on, the boot ID of the placement it is,
/// which a later placement's exceeds. A writer takes the lease only with a token as high as the
/// last writer's: a placement cut off takes no branch back from the one after it. A copy never
/// placed takes it with 0.
pub(crate) fn place(dir: &std::path::Path, branch: u64, token: u64) -> std::io::Result<()> {
    let name = fence(branch);
    let new = dir.join(format!("{name}.new"));
    let mut file = std::fs::File::create(&new)?;
    std::io::Write::write_all(&mut file, format!("{token}\n").as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&new, dir.join(&name))?;
    std::fs::File::open(dir)?.sync_all()
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
pub(crate) fn placed_token(dir: &std::path::Path, branch: u64) -> Result<Option<u64>, Failure> {
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
pub(crate) fn unmark(dir: &std::path::Path, branch: u64) -> std::io::Result<()> {
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
    std::sync::LazyLock::new(|| Budget::new(Limits { gets: 64, bytes: 64 << 20 }));

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

/// Where a fork is answered: with the tag of the branch's last commit, which its child inherits.
type Forked = std::sync::mpsc::Sender<Result<Option<u64>, Failure>>;

#[derive(Default)]
struct Progress {
    /// The last epoch uploaded.
    uploaded: u64,
    /// Why uploading stopped, if it did.
    failed: Option<Failure>,
    /// Whether a commit waits to be uploaded: the next batch goes now.
    wanted: bool,
    /// How many commits waited for the lag to shrink.
    stalls: u64,
    /// The lowest sequence number the guest may mint: its writer's epoch's range (doc-7).
    next_sequence: u64,
    /// The forks to make once every commit made durable is uploaded: their branches, and where to
    /// answer with the branch's last commit.
    forks: Vec<(u64, Forked)>,
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
}

impl Progress {
    /// Notes the store's commits through `epoch` uploaded.
    fn uploaded_through(&mut self, epoch: u64) {
        self.uploaded = self.uploaded.max(epoch);
        while self.seen.front().is_some_and(|(seen, _)| *seen <= self.uploaded) {
            self.seen.pop_front();
        }
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
        loop {
            let changed = {
                let progress = shared.progress.lock().unwrap();
                if let Some(reached) = shared.reached(&progress, epoch, false) {
                    return reached;
                }
                shared.changed.listen()
            };
            changed.wait();
        }
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
        })
    }

    /// Waits, if more commits wait to be uploaded than `Options::max_lag`, with `epoch`'s, or they
    /// wrote more than `Options::max_lag_bytes`, until they no longer do.
    pub fn wait_lag(&self, epoch: u64) -> Result<(), Failure> {
        let (shared, epoch) = (&self.shared, epoch.saturating_sub(self.shared.options.max_lag));
        if shared.ask(epoch, true)? {
            return Ok(());
        }
        loop {
            let changed = {
                let progress = shared.progress.lock().unwrap();
                if let Some(reached) = shared.reached(&progress, epoch, true) {
                    return reached;
                }
                shared.changed.listen()
            };
            changed.wait();
        }
    }

    /// Forks `child` from the branch at its end, once every commit made durable before is uploaded:
    /// its record names the branch, and the segment of the fork, as its origin. Returns the tag of
    /// the branch's last commit, which the child inherits.
    pub fn fork(&self, child: u64) -> Result<Option<u64>, Failure> {
        let (answer, forked) = std::sync::mpsc::channel();
        {
            let mut progress = self.shared.progress.lock().unwrap();
            if let Some(failed) = &progress.failed {
                return Err(failed.clone());
            }
            progress.forks.push((child, answer));
        }
        self.shared.wake.notify_one();
        forked.recv().map_err(|_| Failure::Other("the uploader stopped".into()))?
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
    /// Whether the commits up to `epoch` are uploaded and acknowledged, and, if `stall`, the bytes
    /// waiting within `Options::max_lag_bytes`: if not, asks the uploader for them, counting a
    /// stall if `stall`.
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
        // The segments uploading, in order, as the futures of their uploads, which give the epochs
        // of the store's commits they hold, and their commits' tags.
        let mut uploads = FuturesOrdered::new();
        let mut uploading = Vec::new();
        let (mut wanted, mut due) = (false, false);
        loop {
            if let Err(err) = self.observe() {
                return self.fail(&writer, err);
            }
            let (stop, forking) = {
                let mut progress = self.progress.lock().unwrap();
                wanted |= std::mem::take(&mut progress.wanted);
                (progress.stop, !progress.forks.is_empty())
            };
            if (wanted || due || stop || forking) && uploads.len() < IN_FLIGHT {
                (wanted, due) = (false, false);
                match blocking(|| self.gather(&writer, &mut uploading)) {
                    Ok(gathered) if !gathered.is_empty() => {
                        for (seq, epoch, number) in gathered {
                            let writer = writer.clone();
                            uploads.push_back(async move {
                                writer.upload(seq).await.map(|()| (epoch, number))
                            });
                        }
                        // More may have been made durable meanwhile.
                        wanted = true;
                        continue;
                    }
                    // Every segment closed is uploaded: forks asked for are answered.
                    Ok(_) if forking && uploads.is_empty() => {
                        if let Err(err) = self.fork(&writer, true).await {
                            return self.fail(&writer, err);
                        }
                        continue;
                    }
                    Ok(_) if stop && uploads.is_empty() => {
                        // A checkpoint on clean shutdown (doc-7, Checkpoints).
                        if let Err(err) = writer.checkpoint().await {
                            log::warn!("fxdax: checkpointing branch {:x}: {err}", writer.branch());
                        }
                        return self.fail(&writer, Failure::Other("the uploader stopped".into()));
                    }
                    Ok(_) => {}
                    Err(err) => return self.fail(&writer, err),
                }
            }
            let woken = tokio::time::timeout(interval, self.wake.notified());
            if uploads.is_empty() {
                due = woken.await.is_err();
                continue;
            }
            match futures::future::select(uploads.next(), std::pin::pin!(woken)).await {
                Either::Left((uploaded, _)) => {
                    let uploaded = uploaded.expect("an upload in flight").map_err(Failure::from);
                    let acked = match uploaded {
                        Ok((epoch, number)) => {
                            self.acknowledged(&writer, epoch, number, &mut uploading).await
                        }
                        Err(err) => Err(err),
                    };
                    if let Err(err) = acked {
                        return self.fail(&writer, err);
                    }
                    // Forks the segment made, whose children have their records now.
                    if let Err(err) = self.fork(&writer, false).await {
                        return self.fail(&writer, err);
                    }
                    segments += 1;
                    if segments % CHECKPOINT_EVERY == 0
                        && let Err(err) = writer.checkpoint().await
                    {
                        log::warn!("fxdax: checkpointing branch {:x}: {err}", writer.branch());
                    }
                }
                Either::Right((woken, _)) => due = woken.is_err(),
            }
        }
    }

    /// Stops uploading, for `err`: commits waiting for uploads, and forks, fail with it.
    fn fail(&self, writer: &Writer<Bucket>, err: Failure) {
        let mut progress = self.progress.lock().unwrap();
        if !progress.stop {
            log::error!("fxdax: uploading branch {:x}: {err}", writer.branch());
        }
        for (_, answer) in progress.forks.drain(..) {
            let _ = answer.send(Err(err.clone()));
        }
        progress.failed = Some(err);
        self.changed.notify(usize::MAX);
    }

    /// Answers the forks asked for whose segments are acknowledged, their children's records made
    /// ([`Writer::acknowledge`]), with a checkpoint at the fork, which the children's recovery
    /// starts from; and, if `all`, the others, which no segment made, with an error.
    async fn fork(&self, writer: &Writer<Bucket>, all: bool) -> Result<(), Failure> {
        let forked = writer.state().forks;
        let made = |child: u64| forked.iter().any(|fork| fork.child == child);
        let (ready, waiting): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.progress.lock().unwrap().forks)
                .into_iter()
                .partition(|(child, _)| made(*child));
        self.progress.lock().unwrap().forks.extend(waiting);
        if !ready.is_empty() {
            writer.checkpoint().await?;
        }
        let last = writer.state().last_commit;
        for (_, answer) in ready {
            let _ = answer.send(Ok(last));
        }
        if all {
            for (child, answer) in std::mem::take(&mut self.progress.lock().unwrap().forks) {
                let unmade = format!("the store made no fork {child:x}");
                let _ = answer.send(Err(Failure::Other(unmade)));
            }
        }
        Ok(())
    }

    /// Takes the branch, and uploads what the store holds that it doesn't: unless the branch moved
    /// on elsewhere since the store last uploaded to it. A store that holds nothing takes what the
    /// branch holds.
    async fn start(&self, bucket: &Arc<Bucket>, holder: u64) -> Result<Writer<Bucket>, Failure> {
        probe(&**bucket, holder).await.map_err(Failure::Other)?;
        let branch = self.store.description().branch;
        match branch::record(&**bucket, branch).await? {
            None => branch::create(&**bucket, branch).await?,
            Some(record) => {
                let state = extent_remote::recover(&**bucket, branch, record.origin).await?.state;
                let ours = self.marker()?;
                let empty = blocking(|| Ok::<_, Failure>(self.store.extents()?.is_empty()))?
                    && self.store.root()?.is_empty();
                match state.last_commit {
                    // Nothing committed, or the store's commit last.
                    None => {}
                    Some(last) if ours.contains(&last) => {}
                    Some(_) if empty && ours.is_empty() => blocking(|| self.take(&state))?,
                    Some(last) => {
                        return Err(Failure::Other(format!(
                            "branch {branch:x} moved on elsewhere (commit {last:#x}): its store \
                             here is stale"
                        )));
                    }
                }
            }
        }
        let token = self.token()?;
        let writer = Writer::open(bucket.clone(), branch, holder, token).await?;
        let lease = Lease { token, epoch: writer.epoch(), taken: SystemTime::now() };
        {
            let mut progress = self.progress.lock().unwrap();
            (progress.next_sequence, progress.lease) =
                (writer.chain().next_sequence(), Some(lease));
        }
        // What the store holds that the branch doesn't, as of a commit after which the store
        // closes its segments for this writer: in a segment of its own, if it changes anything,
        // closed on the writer's chain, which then stamps the store's footers, which follow it.
        loop {
            let snapshot = blocking(|| self.store.snapshot())?;
            let mut chain = writer.chain();
            let tag = chain.tag(snapshot.epoch);
            let commit = blocking(|| self.diff(&writer.state(), &snapshot, tag))?;
            // Its footer stamped, its data framed only once the store takes the chain.
            let first = (!commit.is_empty()).then(|| Batch::from(commit).close(&mut chain));
            // A commit since, and the snapshot is stale.
            if !self.store.upload(chain, snapshot.epoch)? {
                continue;
            }
            // What it holds is durable before it is uploaded.
            blocking(|| self.store.flush())?;
            if let Some(first) = first {
                self.send(&writer, tag, first).await?;
            }
            self.progress.lock().unwrap().uploaded_through(snapshot.epoch);
            return Ok(writer);
        }
    }

    /// The commit, tagged `tag`, that makes `remote`, the branch's state, hold what `snapshot`, the
    /// store's, does: the extents the store holds that the branch doesn't, read from the store,
    /// the frees of those the branch holds that the store doesn't, and the store's root, if the
    /// branch's differs.
    fn diff(&self, remote: &State, snapshot: &Snapshot, tag: u64) -> Result<Commit, Failure> {
        let mut commit = Commit { tag, ..Commit::default() };
        for &(extent, len) in &snapshot.extents {
            if !remote.extents.contains_key(&extent) {
                self.add(&mut commit, extent, len)?;
            }
        }
        let held: BTreeSet<ExtentId> = snapshot.extents.iter().map(|(extent, _)| *extent).collect();
        commit.frees =
            remote.extents.keys().filter(|extent| !held.contains(extent)).copied().collect();
        if remote.root.as_deref() != Some(&snapshot.root[..]) && !snapshot.root.is_empty() {
            commit.root = Some(snapshot.root.clone());
        }
        Ok(commit)
    }

    /// Adds `extent`, of `len` bytes, read from the store, to `commit`.
    fn add(&self, commit: &mut Commit, extent: ExtentId, len: u64) -> Result<(), Failure> {
        let mut data = vec![0; len as usize];
        let read = self.store.read_kept(extent, 0, &mut data);
        read.map_err(|err| Failure::Other(format!("reading {extent:?}: {err:#}")))?;
        if len >= LARGE { &mut commit.own } else { &mut commit.extents }.push((extent, data));
        Ok(())
    }

    /// Has the store close its open segment, and takes the segments it closed whose commits are
    /// durable, to be uploaded as they are: returns, for each, its number in the branch, its last
    /// commit's epoch, and its number in the store. `uploading` gains their tags.
    fn gather(
        &self,
        writer: &Writer<Bucket>,
        uploading: &mut Vec<u64>,
    ) -> Result<Vec<(u64, u64, u64)>, Failure> {
        // What a waiter wants is durable, then closed, and what the closing closed durable too.
        self.store.flush()?;
        self.store.close_now()?;
        self.store.flush()?;
        let mut gathered = Vec::new();
        for closed in self.store.closed()? {
            let read = |err: std::io::Error| Failure::Other(format!("reading a segment: {err}"));
            let segment = closed.bytes().map_err(read)?.into();
            let own = closed.own_bytes().map_err(read)?;
            let own = own.into_iter().map(|(key, bytes)| (key, bytes.into())).collect();
            uploading.extend(closed.footer.commits.last());
            self.mark(writer.state().last_commit, uploading)?;
            let seq = writer.add_segment(Objects { segment, own })?;
            gathered.push((seq, closed.last(), closed.number));
        }
        Ok(gathered)
    }

    /// Acknowledges the segment uploaded of the store's commits up to `epoch`, the first of
    /// `uploading`, and those uploaded after it: its commits are uploaded, and the store's segment
    /// `number` may go.
    async fn acknowledged(
        &self,
        writer: &Writer<Bucket>,
        epoch: u64,
        number: u64,
        uploading: &mut Vec<u64>,
    ) -> Result<(), Failure> {
        writer.acknowledge().await?;
        uploading.remove(0);
        blocking(|| {
            self.mark(writer.state().last_commit, uploading)?;
            self.store.release(epoch)?;
            self.store.uploaded(number)
        })?;
        self.progress.lock().unwrap().uploaded_through(epoch);
        self.changed.notify(usize::MAX);
        Ok(())
    }

    /// Uploads `segment`, the segment of commit `tag` closed on the writer's chain, and
    /// acknowledges it, the marker naming the commit while it uploads.
    async fn send(
        &self,
        writer: &Writer<Bucket>,
        tag: u64,
        segment: Unframed,
    ) -> Result<(), Failure> {
        let acked = writer.state().last_commit;
        blocking(|| self.mark(acked, &[tag]))?;
        let seq = writer.add_segment(segment.frame())?;
        writer.upload(seq).await?;
        writer.acknowledge().await?;
        blocking(|| self.mark(writer.state().last_commit, &[]))?;
        Ok(())
    }

    /// The commits the marker names: the last uploaded, and the one uploading.
    fn marker(&self) -> Result<Vec<u64>, Failure> {
        let path = self.store.dir().path().join(marker(self.store.description().branch));
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

    /// The fencing token of the store's copy of its branch ([`place`]): `Options::token` if it was
    /// never placed.
    fn token(&self) -> Result<u64, Failure> {
        let placed = placed_token(self.store.dir().path(), self.store.description().branch)?;
        Ok(placed.unwrap_or(self.options.token))
    }

    /// Records the last commit uploaded, and those uploading, durably.
    fn mark(&self, acked: Option<u64>, uploading: &[u64]) -> Result<(), Failure> {
        mark(self.store.dir().path(), self.store.description().branch, acked, uploading)
    }

    /// Takes what the branch holds into the store, which holds nothing: a branch opened on a host
    /// without it. The store holds the branch's extents, whose data stays in object storage, read
    /// as the guest reads it ([`Fetcher`]).
    fn take(&self, state: &extent_remote::State) -> Result<(), Failure> {
        for (extent, location) in &state.extents {
            self.store.adopt(*extent, location.len)?;
        }
        self.store.commit(Some(state.root.as_deref().unwrap_or_default()))?;
        self.mark(state.last_commit, &[])
    }
}

/// Records in the store directory `dir` the last commit of its branch `branch` uploaded, and those
/// uploading, durably ([`marker`]).
pub(crate) fn mark(
    dir: &std::path::Path,
    branch: u64,
    acked: Option<u64>,
    uploading: &[u64],
) -> Result<(), Failure> {
    let word = |tag: Option<u64>| tag.map_or("-".to_owned(), |tag| format!("{tag:x}"));
    let written = || -> std::io::Result<()> {
        let new = dir.join(format!("{}.new", marker(branch)));
        let mut file = std::fs::File::create(&new)?;
        std::io::Write::write_all(
            &mut file,
            format!(
                "{}{}\n",
                word(acked),
                uploading.iter().map(|tag| format!(" {tag:x}")).collect::<String>()
            )
            .as_bytes(),
        )?;
        file.sync_all()?;
        std::fs::rename(&new, dir.join(marker(branch)))?;
        std::fs::File::open(dir)?.sync_all()
    };
    written().map_err(|err| Failure::Other(format!("recording the upload: {err}")))
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

    /// An object store whose requests fail while it is shut, and whose segments' writes wait
    /// while they are held.
    #[derive(Debug)]
    struct Gated {
        store: Arc<dyn object_store::ObjectStore>,
        shut: std::sync::atomic::AtomicBool,
        hold: tokio::sync::watch::Sender<bool>,
        /// The segments' writes waiting.
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
                hold: tokio::sync::watch::channel(false).0,
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
            if location.as_ref().starts_with("segments/") {
                let mut hold = self.hold.subscribe();
                self.held.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let _ = hold.wait_for(|hold| !*hold).await;
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
        gated.hold.send_replace(true);
        let commits: Vec<_> = (1..=3)
            .map(|sequence| {
                let committing = store.clone();
                let commit = std::thread::spawn(move || {
                    write(&committing, sequence, &pattern(sequence as u8, BLOCK));
                    committing.commit(None)
                });
                let deadline = std::time::Instant::now() + Duration::from_secs(30);
                let held = || gated.held.load(std::sync::atomic::Ordering::Relaxed);
                while held() < sequence as usize && std::time::Instant::now() < deadline {
                    std::thread::yield_now();
                }
                assert_eq!(held(), sequence as usize);
                commit
            })
            .collect();
        gated.hold.send_replace(false);
        for commit in commits {
            assert_eq!(commit.join().unwrap(), Ok(()));
        }
        let (extents, _) = uploaded(&bucket, store.branch());
        assert_eq!(extents.iter().map(|(sequence, _)| *sequence).collect::<Vec<_>>(), [1, 2, 3]);
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
