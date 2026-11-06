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
//! A store of a point has no uploader's work to do (doc-7, Points): a point's state never changes,
//! so that attaching one takes no lease, recovers its state from the bucket, and takes it into the
//! store if it holds nothing, and nothing the store does goes to the bucket but its forks' records
//! ([`Remote::forked`], `extent_remote::branch::fork_point`). Any number of hosts hold one at once,
//! each forking it, and none stale.
//!
//! The segments uploading, and the store's directory's marker naming them, are kept together
//! ([`InFlight`]): the marker names every segment that may land, before its upload goes, so that
//! the store opened again knows the branch's last commit for its own. A fork the store makes is a
//! fork of the branch once the segment its commit ends is acknowledged, which gives the child its
//! record, and checkpointed ([`Remote::forked`]): only then is the child's store made, its state
//! taken from the bucket ([`take_branch`]), so that no store here holds a child the bucket has no
//! record of (TASK-165.7). A fork that fails before is deleted in the bucket, as the fork's alone
//! ([`Remote::unfork`]): one whose host crashed stands, its segment landing, as the bucket's
//! protocol has it, or never was, the disk lost.
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

use crate::faults::failpoint;
use event_listener::{Event, Listener as _};
use extent_remote::batch;
use extent_remote::cache::Cache;
use extent_remote::fetch::{self, Budget, Limits, Priority};
use extent_remote::layout::{Child, Footer, Location, Origin, epoch_of};
use extent_remote::record::{Kind, Record};
use extent_remote::store::Store as Bucket;
use extent_remote::{Body, Objects, State, Writer, branch};
use extent_store::{File as _, HostDir, HostFile, Snapshot, Store, read_object};
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

/// How many of a store's segments upload at once: as many of a segment's size ([`BATCH`]) as the
/// host's uploads may hold at once ([`UPLOAD_BYTES`]), so that a store catching up uploads as many
/// at once as the host's budget lets it, about ten keeping 1 GB/s going with PUTs of 0.35 s, and
/// holds no more of its segments' files open for them (TASK-79.19).
const IN_FLIGHT: usize = (UPLOAD_BYTES / BATCH) as usize;

/// The most bytes of extents a segment of what a store holds that its branch doesn't holds, those
/// stored on their own included, but for one extent alone that is more: a store's segment's size
/// (`extent_store::Options::segment_size`).
const BATCH: u64 = 32 << 20;

/// How many segments go between checkpoints.
pub(crate) const CHECKPOINT_EVERY: u64 = 64;

/// The most bytes of objects a host's uploads hold at once ([`UPLOADS`]).
const UPLOAD_BYTES: u64 = 512 << 20;

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
/// file of its own ([`write_new`]), then renamed over it, the directory synced. Of two writes at
/// once, the one renamed last stays.
fn write_durably(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let new = write_new(dir, name, bytes)?;
    std::fs::rename(&new, dir.join(name)).inspect_err(|_| drop(std::fs::remove_file(&new)))?;
    std::fs::File::open(dir)?.sync_all()
}

/// Writes `bytes` to a new file of the file `name`'s in the directory `dir` ([`new_file`]), synced,
/// and returns its path: what goes in place of the file, whole ([`write_durably`]). It goes again
/// if the write fails.
fn write_new(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let new = dir.join(new_file(name));
    let written = || -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&new)?;
        std::io::Write::write_all(&mut file, bytes)?;
        file.sync_all()
    };
    written().inspect_err(|_| drop(std::fs::remove_file(&new)))?;
    // Where a test has another writer of the file write it meanwhile (TASK-184).
    fail::fail_point!("remote::written");
    Ok(new)
}

/// The name of a new file of the file `name`'s ([`write_new`]): one of its own for each write, so
/// that two writers of one file at once each write their own (TASK-184). A shared one, either
/// truncated the other's or removed it from under its rename. What a crash leaves goes with the
/// store's directory, the branch's alone (TASK-165.7).
fn new_file(name: &str) -> String {
    format!("{name}.new-{:016x}", rand::random::<u64>())
}

/// Places the store directory `dir`'s copy of `branch` (TASK-39.7): records, durably, the fencing
/// token its writer takes the branch's lease with from then on, the boot ID of the placement it is,
/// which a later placement's exceeds. A writer takes the lease only with a token as high as the
/// last writer's: a placement cut off takes no branch back from the one after it. A copy never
/// placed takes it with 0.
pub(crate) fn place(dir: &Path, branch: u64, token: u64) -> std::io::Result<()> {
    write_durably(dir, &fence(branch), format!("{token}\n").as_bytes())
}

/// Runs `future` to its end on a runtime of its own, as what is called from no runtime's task
/// does.
pub(crate) fn block_on<F: std::future::Future>(future: F) -> Result<F::Output, Failure> {
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
/// is not deleted (`extent_remote::Error::Stale`, TASK-45.1). A store's directory holds a branch
/// only once the bucket has its record (TASK-165.7), so that the branch it names is its own.
pub fn delete_branch(bucket: &Bucket, branch: u64, token: u64) -> Result<(), Failure> {
    Ok(block_on(branch::delete(bucket, branch, rand::random(), token))??)
}

/// The fencing token of the copy of `branch` in the store directory `dir`: its placement's
/// ([`place`]), if it was placed.
pub(crate) fn placed_token(dir: &Path, branch: u64) -> Result<Option<u64>, Failure> {
    let path = dir.join(fence(branch));
    match std::fs::read_to_string(&path) {
        Ok(text) => (text.trim().parse().map(Some))
            .map_err(|err| Failure::local(err, path.display().to_string())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(Failure::local(err, format!("reading {}", path.display()))),
    }
}

/// The fencing token of this host's placement, `FXDAX_EXTENTD_TOKEN`, which the system's control
/// plane gives it, and which the copies of stores here that were never placed ([`place`]) take
/// their branches with, as stores restored or taken here are. Not 0, which would fence nothing:
/// a host without one keeps no branch (TASK-45).
pub fn host_token() -> Result<u64, Failure> {
    let missing = || {
        Failure::msg(
            "FXDAX_EXTENTD_TOKEN names no placement token: this host's placement's, from the \
             system's control plane, which fences earlier placements",
        )
    };
    match std::env::var("FXDAX_EXTENTD_TOKEN") {
        Ok(token) if !token.is_empty() => match token.parse() {
            Ok(0) => Err(missing()),
            Ok(token) => Ok(token),
            Err(err) => {
                Err(Failure::local(err, format!("FXDAX_EXTENTD_TOKEN {token:?}")))
            }
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

/// Why uploading stopped, or a branch could not be uploaded to: the error as it was, typed, which
/// [`Failure::errno`] maps to its errno, in the one place.
#[derive(Clone, Debug, thiserror::Error)]
pub enum Failure {
    /// What object storage's protocol found of the branch, as `extent_remote` says it: another
    /// writer took it (`Fenced`, `Displaced`), it was deleted, a later placement holds it
    /// (`Stale`, TASK-39.7), another branch has a fork's child's ID (`ChildTaken`, TASK-79.82), or
    /// what it needs is gone or corrupt.
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
    pub(crate) fn msg(message: impl std::fmt::Display + std::fmt::Debug + Send + Sync + 'static) -> Self {
        Self::Local(Arc::new(anyhow::Error::msg(message)))
    }

    /// This host's failure of `what`, with `error`.
    pub(crate) fn local(
        error: impl std::error::Error + Send + Sync + 'static,
        what: impl std::fmt::Display + Send + Sync + 'static,
    ) -> Self {
        Self::Local(Arc::new(anyhow::Error::new(error).context(what)))
    }

    /// The errno of the failure, as doc-4 and doc-7 have them: a branch this writer lost, to
    /// another writer or deleted, or one nothing writes, is one the store takes no more commits for
    /// (`EROFS`); one a later placement holds, which a placement cut off takes back no more, or
    /// one another writer moved on from this host's write-ahead log, which the store opens no more
    /// until `fxdaxctl take` drops it, `ESTALE`; a branch, or a point, gone, `ENOENT`; a fork's child's ID taken, `EEXIST`; a branch
    /// of a newer format, `ENOTSUP`. None for what the store failed to read or write, here or in
    /// object storage, which is the caller's `EIO`, logged.
    pub(crate) fn errno(&self) -> Option<crate::Errno> {
        use crate::Errno;
        use extent_remote::Error;
        match self {
            Self::Remote(Error::Fenced | Error::Displaced | Error::Deleted | Error::Point) => {
                Some(Errno::EROFS)
            }
            Self::Remote(Error::Stale | Error::Diverged) => Some(Errno::ESTALE),
            Self::Remote(Error::NoBranch) | Self::PointDeleted => Some(Errno::ENOENT),
            Self::Remote(Error::ChildTaken(_)) => Some(Errno::EEXIST),
            // A branch of a newer format than this host reads, until it is upgraded (doc-11).
            Self::Remote(Error::Format { .. }) => Some(Errno::ENOTSUP),
            Self::Remote(Error::Lost(_) | Error::Corrupt(_) | Error::Read(_)) | Self::Local(_) => {
                None
            }
        }
    }
}

/// Failures are the same as they say, this host's own as their errors' messages do.
impl PartialEq for Failure {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Remote(error), Self::Remote(other)) => error == other,
            (Self::PointDeleted, Self::PointDeleted) => true,
            (Self::Local(error), Self::Local(other)) => format!("{error:#}") == format!("{other:#}"),
            (Self::Remote(_) | Self::PointDeleted | Self::Local(_), _) => false,
        }
    }
}

impl Eq for Failure {}

impl From<crate::faults::Failpoint> for Failure {
    fn from(failpoint: crate::faults::Failpoint) -> Self {
        Self::from(anyhow::Error::new(failpoint))
    }
}

impl From<anyhow::Error> for Failure {
    fn from(error: anyhow::Error) -> Self {
        Self::Local(Arc::new(error))
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
    /// The bucket the store's branch is kept in.
    bucket: Arc<Bucket>,
    /// Says the uploader stopped, once it has.
    stopped: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    /// fxdax: the store's directory, held taken for this process until the uploader stopped, last
    /// of its fields to go: the store closes first, its last commit the uploader's to upload as it
    /// stops, writing the branch's marker in the directory, which another process, a crash test's
    /// copy, took once the store let it go (TASK-129).
    _dir: extent_store::DirHold<HostDir>,
}

struct Shared {
    store: Arc<Store<HostDir>>,
    fetcher: Arc<Fetcher>,
    /// The branch's writer, once it took the branch.
    writer: std::sync::OnceLock<Arc<Writer<Bucket>>>,
    /// The point the store is of, if it is one, once attached.
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
}

/// The GETs of extents' data a host has in flight: one connection to object storage gives roughly
/// 80-100 MB/s, so a host needs tens (doc-7, Reading); each holds a part, of 1 MiB.
static BUDGET: std::sync::LazyLock<Arc<Budget>> =
    std::sync::LazyLock::new(|| Budget::new(Limits { requests: 64, bytes: 64 << 20 }));

/// What a host's stores' reads keep in memory, objects' tails and the parts fetched last: once for
/// the host, however many of its stores read them, as the forks of one base read its parts, within
/// bounds that hold for the host, not for each store (TASK-92.21, TASK-92.22).
static MEMORY: std::sync::LazyLock<Arc<fetch::Memory>> =
    std::sync::LazyLock::new(fetch::Memory::new);

/// The PUTs of objects a host's stores upload, each holding its object's bytes while it goes:
/// bounded in bytes, so that a host whose stores are far behind, object storage out of reach for a
/// while, holds no more than these at once, whatever their number (TASK-79.19); and in number, as
/// its GETs are.
static UPLOADS: std::sync::LazyLock<Arc<Budget>> =
    std::sync::LazyLock::new(|| Budget::new(Limits { requests: 64, bytes: UPLOAD_BYTES }));

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
#[allow(clippy::disallowed_methods, reason = "the work waits on the disk alone")]
fn blocking<T>(work: impl FnOnce() -> T) -> T {
    tokio::task::block_in_place(work)
}

/// Reads the extents a store holds whose data is in object storage ([`Store::adopt`]), at their
/// locations there, as the store names them, on the remotes' runtime, which their uploads use too,
/// under the host's budget of GETs.
pub struct Fetcher {
    runtime: tokio::runtime::Handle,
    parts: Arc<fetch::Fetcher<Bucket>>,
}

/// A point a store holds ([`Shared::start_point`]): its branch, and what its record names.
struct Point {
    branch: u64,
    origin: Origin,
}

impl Fetcher {
    /// What reads extents' data from `bucket`, keeping the parts it fetches in `cache`, if given,
    /// the host's.
    pub(crate) fn new(bucket: Arc<Bucket>, cache: Option<Arc<Cache>>) -> Arc<Self> {
        let runtime = RUNTIME.handle().clone();
        let runs = runtime.clone();
        let spawn: fetch::Spawn = Arc::new(move |future| drop(runs.spawn(future)));
        let parts = fetch::Fetcher::new(bucket, BUDGET.clone(), MEMORY.clone(), cache);
        Arc::new(Self { runtime, parts: Arc::new(parts.reading_ahead(spawn)) })
    }

    /// `len` bytes at `offset` in `extent`, whose data is at `location` in the bucket: as many as
    /// it holds.
    pub fn read(
        &self,
        extent: ExtentId,
        location: Location,
        offset: u64,
        len: u64,
    ) -> impl std::future::Future<Output = Result<Vec<u8>, Failure>> + Send + use<> {
        let parts = self.parts.clone();
        let read = self.runtime.spawn(async move {
            Ok(parts.read(extent, &location, offset, len, Priority::Demand).await?)
        });
        async move { read.await.map_err(|err| Failure::local(err, "fetching"))? }
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
        let fetcher = Fetcher::new(bucket.clone(), options.cache.clone());
        let dir = store.hold_dir()?;
        let shared = Arc::new(Shared {
            store,
            fetcher,
            writer: Default::default(),
            point: Default::default(),
            forks: Default::default(),
            progress: Mutex::new(Progress::default()),
            changed: Event::new(),
            wake: tokio::sync::Notify::new(),
            options,
        });
        let (started, start) = std::sync::mpsc::channel();
        let (stop, stopped) = std::sync::mpsc::channel();
        let (uploader, uploads) = (shared.clone(), bucket.clone());
        // A task, not a thread of its own (TASK-34).
        RUNTIME.spawn(async move {
            uploader.run(uploads, holder, started).await;
            // The store let go of before the uploader says it stopped: so that dropping this
            // returns with nothing of the uploader's holding the store open, which the process may
            // open again at once (`extent_store::Dir::claim` refusing a second opening), as
            // extentd does a store an image it imported made (TASK-165.6).
            drop(uploader);
            let _ = stop.send(());
            fail::fail_point!("remote::stopped");
        });
        match start.recv() {
            Ok(Ok(())) => {
                Ok(Self { shared, bucket, stopped: Mutex::new(Some(stopped)), _dir: dir })
            }
            Ok(Err(err)) => {
                let _ = stopped.recv();
                Err(err)
            }
            Err(_) => Err(Failure::msg("the uploader stopped")),
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
        failpoint!("remote::forked");
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
        match shared.writer.get() {
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
    /// it never did. Its copy was never placed: it is deleted as this host's placement
    /// (`Options::token`).
    pub fn unfork(&self, child: Child) -> Result<(), Failure> {
        let origin = self.shared.forks.lock().unwrap().get(&child.branch).copied();
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
        failpoint!("remote::child_free");
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
        self.shared.progress.lock().unwrap().next_sequence
    }

    /// The bucket the store's branch is kept in.
    pub(crate) fn bucket(&self) -> &Bucket {
        &self.bucket
    }

    /// What reads the extents the store holds whose data is in object storage.
    pub fn fetcher(&self) -> Arc<Fetcher> {
        self.shared.fetcher.clone()
    }

    /// How many commits waited for the lag to shrink.
    pub fn stalls(&self) -> u64 {
        self.shared.progress.lock().unwrap().stalls
    }

    /// Why uploading stopped, if it did: the store's commits are its branch's no more.
    pub fn failed(&self) -> Option<Failure> {
        self.shared.progress.lock().unwrap().failed.clone()
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
            Ok(Some(writer)) => {
                let writer = Arc::new(writer);
                let _ = self.writer.set(writer.clone());
                started.send(Ok(())).unwrap();
                writer
            }
            // A point's store has nothing to upload.
            Ok(None) => {
                started.send(Ok(())).unwrap();
                while !self.progress.lock().unwrap().stop {
                    self.wake.notified().await;
                }
                return;
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
                    return self.fail(&writer, Failure::msg("the uploader stopped"));
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
    /// branch holds, as a fork's child does, whose store is made only once the bucket has its
    /// record (TASK-165.7).
    ///
    /// What the store does is decided twice ([`Shared::plan`]). Before the lease, from the
    /// branch's state then, so that a store plainly stale fences no writer; it changes nothing
    /// then. Then from the state the writer recovers once it holds the lease, which another writer
    /// may have committed to in between: what the store takes, and what it uploads, is that
    /// state's. A store whose branch moved on in between refuses then, having fenced that writer,
    /// whose commits acknowledged stay the branch's: two hosts attaching the branch at once. The
    /// writer recovers it on from the state found before, reading only the segments since
    /// (TASK-92.6). A point has no writer, which it returns none for ([`Shared::start_point`]).
    async fn start(
        &self,
        bucket: &Arc<Bucket>,
        holder: u64,
    ) -> Result<Option<Writer<Bucket>>, Failure> {
        bucket.probed(holder).await.map_err(Failure::msg)?;
        let branch = self.store.description().branch;
        let ours = self.marker().read()?;
        let held = blocking(|| self.store.snapshot())?;
        let found = match record(bucket, branch).await? {
            None => {
                failpoint!("remote::create_branch");
                branch::create(&**bucket, branch, rand::random()).await?;
                None
            }
            Some(record) if record.is_point() => {
                self.start_point(bucket, branch, record, &held).await?;
                return Ok(None);
            }
            Some(record) => {
                let recovered = extent_remote::recover(&**bucket, branch, record.origin()).await?;
                Self::plan(&recovered.state, &ours, &held)?;
                Some((record, recovered))
            }
        };
        let token = self.token(branch)?;
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
            Plan::Take => blocking(|| take(&self.store, &remote))?,
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
            if !self.store.upload(chain, snapshot.epoch, self.options.cache.clone())? {
                continue;
            }
            // The branch's capacity, if it says none, or another: the store's next commit sets it
            // (TASK-165.3).
            if remote.capacity != Some(snapshot.capacity) {
                blocking(|| self.store.restate_capacity())?;
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
            return Ok(Some(writer));
        }
    }

    /// Attaches the store of `branch`, a point whose record is `record`, holding `held`: reads what
    /// it names, a state that never changes, takes it into the store if it holds nothing, and fails
    /// if it holds anything else. No lease: every commit the store makes is its own, and uploaded
    /// as far as anything waiting for that goes.
    async fn start_point(
        &self,
        bucket: &Arc<Bucket>,
        branch: u64,
        record: Record,
        held: &Snapshot,
    ) -> Result<(), Failure> {
        if record.deleted {
            return Err(Failure::PointDeleted);
        }
        let Kind::Point(origin) = record.kind else {
            return Err(Failure::msg(format!("branch {branch:x} is no point")));
        };
        let state = extent_remote::recover(&**bucket, branch, Some(origin)).await?.state;
        if !Self::holds(&state, held) {
            if !held.extents.is_empty() || !held.root.is_empty() {
                return Err(Failure::msg(format!(
                    "the store of point {branch:x} holds another state"
                )));
            }
            blocking(|| take(&self.store, &state))?;
        }
        let _ = self.point.set(Point { branch, origin });
        let mut progress = self.progress.lock().unwrap();
        progress.uploaded_through(u64::MAX);
        progress.forked_through(u64::MAX);
        Ok(())
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
        match state.last_commit {
            None => Ok(Plan::Diff),
            Some(last) if ours.contains(&last) => Ok(Plan::Diff),
            Some(_) if ours.is_empty() && empty => Ok(Plan::Take),
            Some(_) if ours.is_empty() && Self::holds(state, held) => Ok(Plan::Claim),
            Some(last) => Err(Failure::msg(format!(
                "branch {:x} moved on elsewhere (commit {last:#x}): its store here is stale",
                state.branch
            ))),
        }
    }

    /// Whether a store holding `held` holds what `state` does, extents and root.
    fn holds(state: &State, held: &Snapshot) -> bool {
        let extents = state.extents.iter().map(|(extent, location)| (*extent, location.len));
        extents.eq(held.extents.iter().copied())
            && state.root.as_deref().unwrap_or_default() == held.root
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
            match len >= LARGE && extent.branch == remote.branch {
                true => plan.seal_own(extent.sequence, len),
                false => plan.seal(extent, len),
            }
        }
        plan.commit(tag, frees, root, None);
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
        failpoint!("remote::gather");
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
            let tag = closed.tag();
            let forks: Vec<u64> =
                closed.footer.contents.forks.iter().map(|forked| forked.branch).collect();
            let own = closed.own.iter().map(|(key, file)| Ok((key.clone(), stored(file)?)));
            let own = own.collect::<Result<_, Failure>>()?;
            let objects = Objects { footer: closed.footer, segment: stored(&closed.file)?, own };
            let seq = writer.add_segment(objects)?;
            // Where each fork it ends is, which a fork failing deletes its child by.
            let origin = Origin { parent: writer.branch(), at: seq };
            self.forks.lock().unwrap().extend(forks.iter().map(|&child| (child, origin)));
            segments.push(Uploading { seq, number: closed.number, tag, forks: !forks.is_empty() });
        }
        let took = !segments.is_empty();
        in_flight.upload(writer, segments)?;
        Ok(Gathered { took, closed: closing })
    }

    /// Acknowledges what the writer has uploaded: the commits of the segments it acknowledges are
    /// uploaded, and the store's segments leave it, read in the bucket, into the host's cache if
    /// it has one (TASK-165.8); then the forks they name are made, with a checkpoint at them, which
    /// their children's recovery starts from.
    async fn acknowledged(
        &self,
        writer: &Writer<Bucket>,
        in_flight: &mut InFlight,
    ) -> Result<(), Failure> {
        let through = writer.acknowledge().await?;
        failpoint!("remote::acknowledged");
        let acked = blocking(|| in_flight.acknowledged(through, writer.last_commit()))?;
        let Some(epoch) = acked.last().map(Uploading::epoch) else { return Ok(()) };
        blocking(|| {
            self.store.release(epoch)?;
            for segment in &acked {
                let key = extent_remote::layout::segment(writer.branch(), segment.seq);
                self.store.acknowledged(segment.number, &key)?;
            }
            Ok::<_, Failure>(())
        })?;
        failpoint!("remote::released");
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
            failpoint!("remote::checkpointed");
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

    /// The fencing token of the store's copy of `branch` ([`place`]): `Options::token` if it was
    /// never placed.
    fn token(&self, branch: u64) -> Result<u64, Failure> {
        let placed = placed_token(self.store.dir().path(), branch)?;
        Ok(placed.unwrap_or(self.options.token))
    }
}

/// The record of `branch`, if it has one: a fork of a point its host crashed in the middle of, its
/// record saying so, finished first, which fails if the point was deleted. So the store takes no
/// state of a fork not made.
async fn record(bucket: &Bucket, branch: u64) -> Result<Option<Record>, Failure> {
    let found = branch::record(bucket, branch).await?;
    let Some(point) = found.as_ref().filter(|record| !record.deleted).and_then(Record::forking)
    else {
        return Ok(found);
    };
    branch::finish_point_fork(bucket, branch, point).await?;
    Ok(branch::record(bucket, branch).await?)
}

/// Takes `branch`, as `bucket` holds it, into `store`, which holds nothing, as its own: a fork's
/// child, made once the bucket has its record (`host_store::fork`, TASK-165.7). Its writer, when
/// the store is attached, finds the branch as the store holds it, which needs no upload. No lease
/// is taken: the store takes the branch's state as of now, changing nothing in the bucket.
pub(crate) fn take_branch(
    store: &Store<HostDir>,
    bucket: &Bucket,
    branch: u64,
) -> Result<(), Failure> {
    let state = block_on(async {
        let found = record(bucket, branch).await?;
        let found = found.filter(|record| !record.deleted).ok_or(extent_remote::Error::NoBranch)?;
        Ok::<_, Failure>(extent_remote::recover(bucket, branch, found.origin()).await?.state)
    })??;
    take(store, &state)
}

/// Takes what the branch holds, `state`, into `store`, which holds nothing: a branch opened on a
/// host without it, or a fork's child. The store holds the branch's extents, whose data stays in
/// object storage, at their locations there, read as the guest reads it ([`Fetcher`]), and has its
/// capacity, if it has one (TASK-165.3); its marker names the branch's last commit, its own.
fn take(store: &Store<HostDir>, state: &extent_remote::State) -> Result<(), Failure> {
    if let Some(capacity) = state.capacity {
        store.set_capacity(capacity)?;
    }
    for (extent, location) in &state.extents {
        store.adopt(*extent, location.clone())?;
    }
    store.spend(&state.spent)?;
    store.commit(Some(state.root.as_deref().unwrap_or_default()))?;
    Marker::new(store.dir().path(), state.branch).write(state.last_commit, [])
}

/// The bytes of `file`, a segment's the store closed or an extent's of its own it names, read only
/// as its upload goes, and checked: a byte the host's disk damaged is not uploaded under the CRC
/// it fails (`read_object`, TASK-153.11).
fn stored(file: &Arc<HostFile>) -> Result<Body, Failure> {
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
            Err(err) => return Err(Failure::local(err, format!("reading {}", path.display()))),
        };
        text.split_whitespace()
            .filter(|word| *word != "-")
            .map(|word| u64::from_str_radix(word, 16))
            .collect::<Result<_, _>>()
            .map_err(|err| Failure::local(err, path.display().to_string()))
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
        written.map_err(|err| Failure::local(err, "recording the upload"))
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
    /// with every one uploading, the last uploaded being `writer`'s last commit. No upload goes if
    /// the marker cannot be written. A fork's child has no store here until its record is in the
    /// bucket (TASK-165.7), so its segment marks nothing of the child's.
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
        failpoint!("remote::marked");
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
mod fork_faults;

#[cfg(test)]
mod restore_faults;

#[cfg(test)]
mod tests {
    use crate::test_data::noise;
    use crate::{ExtentId, ExtentStore, VolatileSlice};
    use extent_remote::cache::Cache;
    use extent_remote::store::Store as Bucket;
    use extent_remote::{branch, read, recover};
    use object_store::memory::InMemory;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    const BLOCK: usize = ExtentStore::BLOCK_SIZE as usize;

    /// Uploads only when a commit waits.
    pub(super) fn idle() -> super::Options {
        super::Options { interval: Duration::from_secs(3600), ..super::Options::default() }
    }

    pub(super) fn pattern(seed: u8, len: usize) -> Vec<u8> {
        (0..len).map(|i| seed ^ (i % 251) as u8).collect()
    }

    fn write(store: &ExtentStore, sequence: u64, data: &[u8]) -> ExtentId {
        let extent = ExtentId { branch: store.branch(), sequence };
        store.write(extent, 0, &[VolatileSlice::from(data)]).unwrap();
        extent
    }

    /// A branch's extents, by sequence number, with their data, and its root.
    pub(super) type Uploaded = (Vec<(u64, Vec<u8>)>, Option<Vec<u8>>);

    /// The branch's extents and root, read back from `bucket`.
    pub(super) fn uploaded(bucket: &Arc<Bucket>, branch: u64) -> Uploaded {
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
        assert_eq!(fence.contents, extent_remote::layout::Contents::default());
        let bytes = |footer: &extent_remote::layout::Footer| {
            let held: u64 = footer.contents.extents.iter().map(|(_, _, len)| len).sum();
            held + footer.own().map(|(_, location)| location.len).sum::<u64>()
        };
        assert_eq!(bytes(first), super::BATCH);
        assert_eq!(bytes(last), 2 * super::LARGE + BLOCK as u64);
        assert_eq!((first.contents.commits.len(), &first.contents.root), (0, &None));
        let before = Some(b"before".to_vec());
        assert_eq!((last.contents.commits.len(), &last.contents.root), (1, &before));
        assert_eq!(uploaded(&bucket, branch), (written, Some(b"before".to_vec())));
    }

    /// A store whose commit, of more bytes than `options` let wait, is in a segment closed for want
    /// of room, as a closing that beats the uploader leaves it: with no commit to come, as one in
    /// the lag's wait holds its guest's seal, and holds back every other. And the commit's epoch.
    fn closed_for_room(
        options: super::Options,
    ) -> (tempfile::TempDir, Arc<extent_store::Store<extent_store::HostDir>>, super::Remote, u64)
    {
        const EXTENT: usize = 1 << 20;
        let dir = tempfile::tempdir().unwrap();
        let store = crate::host_store::create(&dir.path().join("store"), 1 << 30, 7).unwrap().store;
        let store = Arc::new(store);
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let remote = super::Remote::attach(store.clone(), bucket, 1, options).unwrap();
        store.write(super::ExtentId::new(7, 1), 0, &pattern(1, EXTENT)).unwrap();
        store.commit_ordered(None).unwrap();
        let epoch = store.epoch().unwrap();
        // The most a segment holds, its last extent past it, and no commit with them.
        for sequence in 2..98 {
            let extent = super::ExtentId::new(7, sequence);
            store.write(extent, 0, &pattern(sequence as u8, EXTENT)).unwrap();
        }
        (dir, store, remote, epoch)
    }

    /// TASK-150.2: a commit stalled on the bytes waiting (`Options::max_lag_bytes`) ends though
    /// what it waits for is in a segment closed for want of room, which only a commit's flush would
    /// end, as the commit, or its guest, have none to come: an uploader's own flush ends it.
    #[test]
    fn commits_stalled_on_the_lag_end_with_their_segment_closed_for_room() {
        let (_dir, store, remote, epoch) =
            closed_for_room(super::Options { max_lag_bytes: BLOCK as u64, ..idle() });
        let stalled = std::sync::Arc::new(remote);
        let waiting = stalled.clone();
        let (ended, end) = std::sync::mpsc::channel();
        std::thread::spawn(move || ended.send(waiting.wait_lag(epoch)).unwrap());
        assert_eq!(end.recv_timeout(Duration::from_secs(30)), Ok(Ok(())));
        assert_eq!(store.unreleased_bytes().unwrap(), 0);
    }

    /// TASK-150.2: so does a sync of the store's commits, or anything else waiting for their upload.
    #[test]
    fn uploads_waited_for_end_with_their_segment_closed_for_room() {
        let (_dir, _store, remote, epoch) = closed_for_room(idle());
        let (ended, end) = std::sync::mpsc::channel();
        std::thread::spawn(move || ended.send(remote.wait_uploaded(epoch)).unwrap());
        assert_eq!(end.recv_timeout(Duration::from_secs(30)), Ok(Ok(())));
    }

    /// TASK-79.19: a gather takes no more of the segments the store closed than may upload at once,
    /// with those uploading; the rest it leaves in the store, until those it took are uploaded.
    #[test]
    fn gathers_take_what_may_upload_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::host_store::create(&dir.path().join("store"), 1 << 30, 7).unwrap().store;
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
        let writer = shared.writer.get().unwrap().clone();
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

    /// A store's uploader holds its directory taken until it stops, after the store closed: the
    /// store's closing let it go, and its uploader, which uploads the store's last commits as it
    /// stops, wrote the branch's marker in the directory after, while another process, a crash
    /// test's copy, had taken it, and the copy found the marker's new file gone (TASK-129, GCE, the
    /// DAX gate's second run).
    #[test]
    fn uploaders_hold_their_stores_directory_until_they_stop() {
        use std::os::fd::AsRawFd;
        // Whether another process could take the directory at `path` now, as `Dir::take` does.
        let free = |path: &std::path::Path| {
            let dir = std::fs::File::open(path).unwrap();
            // SAFETY: Plain system call on the directory's descriptor, which closing unlocks.
            unsafe { libc::flock(dir.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store");
        let branch = crate::host_store::new_branch();
        let store = Arc::new(crate::host_store::create(&path, 1 << 30, branch).unwrap().store);
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let remote = super::Remote::attach(store.clone(), bucket, 1, idle()).unwrap();
        assert!(!free(&path), "taken while the store is open");
        store.close().unwrap();
        assert!(!free(&path), "taken while its uploader runs");
        drop(remote);
        assert!(free(&path), "free once the uploader stopped");
    }

    /// TASK-79.19: a benchmark of a store's uploads keeping up with sustained writes, to a bucket
    /// whose PUTs take 350 ms each, as S3's of a segment of 32 MiB may: 4 GiB written, a MiB an
    /// extent, an ordered commit after each 32 MiB, as a guest's writeback makes them, each waiting
    /// while the lag is past its bound, as the store's do; how long until it is all uploaded, at how
    /// many MB/s, and how long the commits waited for the lag, in all.
    /// `cargo nextest run -p fxdax --run-ignored only bench_sustained_uploads --no-capture`.
    #[test]
    #[ignore]
    fn bench_sustained_uploads() {
        use object_store::throttle::{ThrottleConfig, ThrottledStore};
        const WRITTEN: u64 = 4 << 30;
        const EXTENT: usize = 1 << 20;
        let put = Duration::from_millis(350);
        let config = ThrottleConfig { wait_put_per_call: put, ..ThrottleConfig::default() };
        let objects = Arc::new(ThrottledStore::new(InMemory::new(), config));
        let bucket = Arc::new(Bucket::new(objects, ""));
        let dir = tempfile::tempdir().unwrap();
        let store = ExtentStore::create(dir.path().join("store"), 2 * WRITTEN).unwrap();
        store.attach_remote(bucket, 1, super::Options::default()).unwrap();
        let data = noise(1, EXTENT);
        let (started, mut waited) = (std::time::Instant::now(), Duration::ZERO);
        for sequence in 1..=WRITTEN / EXTENT as u64 {
            write(&store, sequence, &data);
            if sequence % 32 == 0 {
                let committing = std::time::Instant::now();
                store.commit_ordered(None).unwrap();
                waited += committing.elapsed();
            }
        }
        let written = started.elapsed();
        store.wait_uploaded().unwrap();
        let took = started.elapsed();
        let stalls = store.remote().unwrap().stalls();
        let rate = WRITTEN as f64 / took.as_secs_f64() / 1e6;
        println!(
            "{} MiB written in {written:.2?}, uploaded in {took:.2?}: {rate:.0} MB/s; commits took \
             {waited:.2?}, {stalls} of them waiting for the lag",
            WRITTEN >> 20
        );
    }

    /// A store's closed segments are the objects of its branch in the bucket, byte for byte, as
    /// are its extents stored on their own (doc-7, decision 1): the uploader puts its files; and
    /// once the bucket has acknowledged them, they leave the store whole, into the host's cache,
    /// as those objects, which the store's reads read there with the bucket's gone (TASK-165.8).
    #[test]
    fn uploaded_segments_are_the_stores_files() {
        use extent_remote::layout::{self, Footer};
        let dir = tempfile::tempdir().unwrap();
        let store = ExtentStore::create(dir.path().join("store"), 1 << 30).unwrap();
        let memory = Arc::new(InMemory::new());
        let bucket = Arc::new(Bucket::new(memory.clone(), ""));
        let cache = Arc::new(Cache::open(&dir.path().join("cache"), 1 << 30, 0).unwrap());
        let options = super::Options { cache: Some(cache.clone()), ..idle() };
        store.attach_remote(bucket.clone(), 1, options).unwrap();
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
            .filter(|bytes| !layout::footer::<Footer>(bytes).unwrap().contents.commits.is_empty())
            .collect();
        segments.sort();
        assert_eq!(segments.len(), 2);
        let mut own = objects(layout::extents(branch));
        own.sort();
        assert_eq!(own.len(), 1);
        assert_eq!((files("segments"), files("extents")), (Vec::new(), Vec::new()));
        assert_eq!(
            uploaded(&bucket, branch),
            (
                vec![(1, pattern(1, BLOCK)), (2, pattern(2, 4 << 20)), (3, pattern(3, 2 * BLOCK))],
                Some(b"two".to_vec())
            )
        );
        futures::executor::block_on(async {
            use futures::StreamExt as _;
            use object_store::{ObjectStore as _, ObjectStoreExt as _};
            for prefix in ["segments", "extents"] {
                let prefix = object_store::path::Path::from(prefix);
                for meta in memory.list(Some(&prefix)).collect::<Vec<_>>().await {
                    memory.delete(&meta.unwrap().location).await.unwrap();
                }
            }
        });
        let written =
            [(1, pattern(1, BLOCK)), (2, pattern(2, 4 << 20)), (3, pattern(3, 2 * BLOCK))];
        for (sequence, data) in written {
            let extent = ExtentId { branch, sequence };
            let mut read = vec![0; data.len()];
            store.read(extent, 0, &mut [crate::VolatileSliceMut::from(&mut read[..])]).unwrap();
            assert!(read == data, "extent {sequence} reads other data");
        }
    }

    /// fxdax: a byte the host's disk damaged in a file the uploader puts as it is, a sealed
    /// extent's own, fails the upload, and the commit waiting for it, rather than going to object
    /// storage under the CRC it fails (TASK-153.11).
    #[test]
    fn files_with_flipped_bytes_are_not_uploaded() {
        use extent_remote::layout;
        let dir = tempfile::tempdir().unwrap();
        let store = ExtentStore::create(dir.path().join("store"), 1 << 30).unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        store.attach_remote(bucket.clone(), 1, idle()).unwrap();
        let branch = store.branch();
        // Stored on its own, raw, as it doesn't compress.
        let extent = write(&store, 1, &noise(1, 4 << 20));
        store.commit_ordered(None).unwrap();
        let path = dir.path().join(format!("store/extents/{branch:016x}/{:016x}", extent.sequence));
        let mut bytes = std::fs::read(&path).unwrap();
        let stored = extent_frame::Parts::of(&bytes).unwrap().stored[1];
        assert_eq!(stored.encoding, extent_frame::Encoding::Raw);
        bytes[stored.offset as usize + 7] ^= 0x10;
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(store.commit(None), Err(crate::error::Errno::EIO));
        let listed = |prefix: String| -> Vec<String> {
            let listed = extent_remote::Bucket::list(&*bucket, &prefix);
            futures::executor::block_on(listed).unwrap().into_iter().map(|meta| meta.key).collect()
        };
        assert_eq!(listed(layout::extents(branch)), Vec::<String>::new());
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
        assert_eq!(refused.err(), Some(super::Failure::Remote(extent_remote::Error::Stale)));
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

    /// What goes wrong with a write through a [`Gated`] store, as it fails: requests that fail, the
    /// host's bucket's helpers send again.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum Mishap {
        /// It takes no effect.
        Refused,
        /// It takes effect, its answer lost.
        Unanswered,
        /// The host dies as it goes, and it takes no effect.
        Dies,
        /// It takes effect, and the host dies before its answer comes.
        DiesAnswered,
        /// The host dies as it goes, and it lands later ([`Gated::land_late`]), as a dead host's
        /// PUT may.
        DiesLandingLate,
        /// A pass of the collector, which what the host's dying does stands for here, runs to its
        /// end just before it, which goes as it would have (TASK-150.1).
        Collected,
    }

    /// A [`Mishap`] set to happen to a write, by its count ([`Gated::arm`]), and what a host's
    /// dying does.
    struct Armed {
        write: usize,
        mishap: Mishap,
        die: Arc<dyn Fn() + Send + Sync>,
    }

    impl std::fmt::Debug for Armed {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{:?} at write {}", self.mishap, self.write)
        }
    }

    /// An object store whose requests fail while it is shut, and whose writes wait while they are
    /// held, go wrong as it is armed to, or reach a copy of its objects once it is severed.
    #[derive(Debug)]
    pub(super) struct Gated {
        store: Arc<dyn object_store::ObjectStore>,
        shut: std::sync::atomic::AtomicBool,
        hold: tokio::sync::watch::Sender<Hold>,
        /// The writes waiting.
        held: std::sync::atomic::AtomicUsize,
        /// The writes held that went since, landing or failing.
        went: std::sync::atomic::AtomicUsize,
        /// The objects it reaches, once severed ([`Gated::sever`]).
        severed: Mutex<Option<Arc<dyn object_store::ObjectStore>>>,
        /// The writes that went since it was armed.
        writes: std::sync::atomic::AtomicUsize,
        armed: Mutex<Option<Armed>>,
        /// The writes that land later ([`Mishap::DiesLandingLate`]).
        late: Mutex<Vec<LateWrite>>,
    }

    /// A write that lands later: where, what, and how.
    type LateWrite = (object_store::path::Path, object_store::PutPayload, object_store::PutOptions);

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
        pub(super) fn over(store: Arc<dyn object_store::ObjectStore>) -> Self {
            Self {
                store,
                shut: Default::default(),
                hold: tokio::sync::watch::channel(Hold::Nothing).0,
                held: Default::default(),
                went: Default::default(),
                severed: Default::default(),
                writes: Default::default(),
                armed: Default::default(),
                late: Default::default(),
            }
        }

        /// The objects its requests reach.
        fn reached(&self) -> Arc<dyn object_store::ObjectStore> {
            self.severed.lock().unwrap().clone().unwrap_or_else(|| self.store.clone())
        }

        /// Cuts it off from its objects, as a host dead is: its requests reach a copy of them from
        /// now on, which nothing else sees, so that what the host had going runs to its end, doing
        /// nothing a dead host would not, rather than wait for ever.
        pub(super) fn sever(&self) {
            use object_store::{ObjectStore as _, ObjectStoreExt as _};
            let copy = InMemory::new();
            futures::executor::block_on(async {
                use futures::StreamExt as _;
                for meta in self.store.list(None).collect::<Vec<_>>().await {
                    let location = meta.unwrap().location;
                    let bytes = self.store.get(&location).await.unwrap().bytes().await.unwrap();
                    copy.put(&location, bytes.into()).await.unwrap();
                }
            });
            *self.severed.lock().unwrap() = Some(Arc::new(copy));
        }

        /// Has `mishap` happen to the write numbered `write`, counting from 0 from now on, `die`
        /// being what the host's dying does.
        pub(super) fn arm(&self, write: usize, mishap: Mishap, die: Arc<dyn Fn() + Send + Sync>) {
            self.writes.store(0, std::sync::atomic::Ordering::Relaxed);
            *self.armed.lock().unwrap() = Some(Armed { write, mishap, die });
        }

        /// How many writes went since it was armed.
        pub(super) fn writes(&self) -> usize {
            self.writes.load(std::sync::atomic::Ordering::Relaxed)
        }

        /// Lands the writes a dead host's PUTs left, whatever they find, at the objects it was cut
        /// off from.
        pub(super) fn land_late(&self) {
            for (location, payload, options) in std::mem::take(&mut *self.late.lock().unwrap()) {
                let landed = self.store.put_opts(&location, payload, options);
                if let Err(err) = futures::executor::block_on(landed) {
                    eprintln!("{location} landing late: {err}");
                }
            }
        }

        /// What happens to the write of `payload` to `location`, armed with `armed`.
        async fn mishap(
            &self,
            armed: Armed,
            location: &object_store::path::Path,
            payload: object_store::PutPayload,
            options: object_store::PutOptions,
        ) -> object_store::Result<object_store::PutResult> {
            match armed.mishap {
                Mishap::Collected => {
                    let collecting = std::thread::spawn(move || (armed.die)());
                    collecting.join().unwrap();
                    return self.reached().put_opts(location, payload, options).await;
                }
                Mishap::Refused => {}
                Mishap::Unanswered => {
                    self.reached().put_opts(location, payload, options).await?;
                }
                Mishap::Dies => (armed.die)(),
                Mishap::DiesAnswered => {
                    self.reached().put_opts(location, payload, options).await?;
                    (armed.die)();
                }
                Mishap::DiesLandingLate => {
                    (armed.die)();
                    self.late.lock().unwrap().push((location.clone(), payload, options));
                }
            }
            Err(object_store::Error::Generic { store: "gated", source: "a mishap".into() })
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
            Self::wait_for(&self.held, count);
        }

        /// Waits until `count` writes held went, landing or failing, as they go in other threads,
        /// and fails if they don't within 30 s.
        fn wait_went(&self, count: usize) {
            Self::wait_for(&self.went, count);
        }

        /// Waits until `counter` counts `count`, as it counts up in other threads, and fails if it
        /// doesn't within 30 s.
        fn wait_for(counter: &std::sync::atomic::AtomicUsize, count: usize) {
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            let counted = || counter.load(std::sync::atomic::Ordering::Relaxed);
            while counted() < count && std::time::Instant::now() < deadline {
                std::thread::yield_now();
            }
            assert_eq!(counted(), count);
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
            let write = self.writes.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let armed = self.armed.lock().unwrap().take_if(|armed| armed.write == write);
            if let Some(armed) = armed {
                return self.mishap(armed, location, payload, opts).await;
            }
            let key = location.as_ref();
            if !self.hold.borrow().holds(key) {
                return self.reached().put_opts(location, payload, opts).await;
            }
            let mut hold = self.hold.subscribe();
            self.held.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let _ = hold.wait_for(|hold| !hold.holds(key)).await;
            self.held.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            let put = self.reached().put_opts(location, payload, opts).await;
            self.went.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            put
        }

        async fn put_multipart_opts(
            &self,
            location: &object_store::path::Path,
            opts: object_store::PutMultipartOptions,
        ) -> object_store::Result<Box<dyn object_store::MultipartUpload>> {
            self.check()?;
            self.reached().put_multipart_opts(location, opts).await
        }

        async fn get_opts(
            &self,
            location: &object_store::path::Path,
            options: object_store::GetOptions,
        ) -> object_store::Result<object_store::GetResult> {
            self.check()?;
            self.reached().get_opts(location, options).await
        }

        fn delete_stream(
            &self,
            locations: futures::stream::BoxStream<
                'static,
                object_store::Result<object_store::path::Path>,
            >,
        ) -> futures::stream::BoxStream<'static, object_store::Result<object_store::path::Path>>
        {
            self.reached().delete_stream(locations)
        }

        fn list(
            &self,
            prefix: Option<&object_store::path::Path>,
        ) -> futures::stream::BoxStream<'static, object_store::Result<object_store::ObjectMeta>>
        {
            self.reached().list(prefix)
        }

        async fn list_with_delimiter(
            &self,
            prefix: Option<&object_store::path::Path>,
        ) -> object_store::Result<object_store::ListResult> {
            self.reached().list_with_delimiter(prefix).await
        }

        async fn copy_opts(
            &self,
            from: &object_store::path::Path,
            to: &object_store::path::Path,
            options: object_store::CopyOptions,
        ) -> object_store::Result<()> {
            self.reached().copy_opts(from, to, options).await
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
        // The store's segments closed, in the bucket: those of the commits waiting, and the
        // writer's fence.
        let prefix = extent_remote::layout::segments(branch);
        let closed = futures::executor::block_on(extent_remote::Bucket::list(&*bucket, &prefix));
        let closed = closed.unwrap();
        assert_eq!(closed.len(), super::IN_FLIGHT + 1);
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
        // The uploader goes on, cut off: it closes the store's open segment, for the commit
        // waiting or as its interval runs out, writing to the host's disk, and takes what it
        // holds to upload, the waiting commit last, as its marker then names. Only from then on
        // does nothing write to the disk, the uploads refused, never acknowledged: lost sooner,
        // the disk would go as the uploader writes to it, which runs on, forgotten.
        let last = lost.committed().unwrap();
        let marker = super::Marker::new(&dir.path().join("lost"), branch);
        while !marker.read().unwrap().iter().any(|&tag| super::epoch_of(tag) == last) {
            // Returned, the commit waiting says the uploader failed, whose marker never will.
            assert!(!waiting.is_finished(), "the commit waiting returned, its upload not begun");
            std::thread::yield_now();
        }
        // The host is lost: its store and uploader, forgotten, never reach object storage again,
        // and its disk is gone.
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

    /// TASK-79.68: an extent freed while open and written anew before the commit is the branch's
    /// still once a store taking the branch recovers it from object storage: its segment's footer
    /// named it among the commit's frees too, which recovery applies after its extents, and the
    /// branch recovered lacked it.
    #[test]
    fn extents_freed_and_written_anew_survive_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let lost = ExtentStore::create(dir.path().join("lost"), 1 << 30).unwrap();
        let branch = lost.branch();
        lost.attach_remote(bucket.clone(), 1, idle()).unwrap();
        let extent = write(&lost, 1, &pattern(1, BLOCK));
        lost.free(extent).unwrap();
        write(&lost, 1, &pattern(2, BLOCK));
        lost.commit(None).unwrap();
        // The host is lost, what it committed in object storage.
        std::mem::forget(lost);

        let taken = ExtentStore::create_for_branch(dir.path().join("taken"), 1 << 30, branch);
        let taken = taken.unwrap();
        taken.attach_remote(bucket, 2, idle()).unwrap();
        let mut data = vec![0; BLOCK];
        taken.read(extent, 0, &mut [crate::VolatileSliceMut::from(&mut data[..])]).unwrap();
        assert!(data == pattern(2, BLOCK), "the extent read back is not its second write");
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
            matches!(refused, Err(super::Failure::Local(ref err)) if err.to_string().contains("moved on")),
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
            matches!(refused, Err(super::Failure::Local(ref err)) if err.to_string().contains("moved on")),
            "{refused:?}"
        );
        let (extents, root) = uploaded(&bucket, branch);
        assert_eq!(extents, [(1, pattern(1, BLOCK)), (sequence, pattern(2, BLOCK))]);
        assert_eq!(root, Some(b"meanwhile".to_vec()));
    }

    /// A fork of a store kept in object storage is a fork of its branch there, recorded once the
    /// parent uploaded through it: it inherits the parent's extents rather than upload them again,
    /// and what either commits after is its own.
    /// TASK-160.10: a point's store, once attached, which alone says it is one, refuses all that
    /// would change what it holds, with `EROFS`, whoever asks: writes, frees, commits, a resize,
    /// a writer taking it over. What changes none of its extents it
    /// does: it forks, and is made public. Before, it refused none of these itself, and a resize
    /// of one grew it.
    #[test]
    fn points_refuse_what_would_change_them() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let source = ExtentStore::create(dir.path().join("source"), 1 << 30).unwrap();
        source.attach_remote(bucket.clone(), 1, idle()).unwrap();
        let extent = write(&source, 1, &pattern(1, BLOCK));
        source.commit(Some(b"root")).unwrap();
        let path = dir.path().join("point");
        source.point(&path, source.branch() + 1).unwrap();
        drop(source);

        let point = ExtentStore::open(&path).unwrap();
        assert_eq!(point.access().role, crate::StoreRole::Writer, "unattached, it can't know");
        point.attach_remote(bucket.clone(), 2, idle()).unwrap();
        assert_eq!(point.access().role, crate::StoreRole::Point);
        let capacity = point.info().unwrap().capacity;
        let own = ExtentId { branch: point.branch(), sequence: 1 };
        let refused = [
            ("set_capacity", point.set_capacity(2 * capacity)),
            ("write", point.write(own, 0, &[VolatileSlice::from(&pattern(2, BLOCK)[..])])),
            ("free", point.free(extent)),
            ("commit", point.commit(Some(b"changed"))),
            ("commit_ordered", point.commit_ordered(None)),
            ("take_over", point.take_over()),
        ];
        for (what, result) in refused {
            assert_eq!(result, Err(crate::Errno::EROFS), "{what}");
        }
        assert_eq!(point.info().unwrap().capacity, capacity);
        assert_eq!(point.root().unwrap(), b"root");
        point.fork(dir.path().join("fork"), point.branch() + 1).unwrap();
        point.make_public().unwrap();
    }

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
        let origin = record.origin().unwrap();

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

    /// TASK-150.6: an extent's ID is its branch's to write once, on any host: one sealed and freed
    /// is spent on a host that takes the branch, whose local index knows nothing of it, whether
    /// the branch's state there comes from a checkpoint or from its segments' footers; and one
    /// freed while open, never sealed, as a journal's tail is, is written still.
    #[test]
    fn extents_freed_are_spent_on_the_host_that_takes_their_branch() {
        for checkpointed in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
            let first = ExtentStore::create(dir.path().join("first"), 1 << 30).unwrap();
            let branch = first.branch();
            first.attach_remote(bucket.clone(), 1, idle()).unwrap();
            let sealed = write(&first, 1, &pattern(1, BLOCK));
            first.commit(Some(b"root")).unwrap();
            first.free(sealed).unwrap();
            let open = write(&first, 2, &pattern(2, BLOCK));
            first.free(open).unwrap();
            first.commit(None).unwrap();
            // The store closing checkpoints the branch; a host lost doesn't.
            if checkpointed {
                drop(first);
            } else {
                std::mem::forget(first);
            }

            let taken = ExtentStore::create_for_branch(dir.path().join("taken"), 1 << 30, branch);
            let taken = taken.unwrap();
            taken.attach_remote(bucket, 2, idle()).unwrap();
            let data = pattern(3, BLOCK);
            let again = [crate::VolatileSlice::from(&data[..])];
            assert_eq!(taken.write(sealed, 0, &again), Err(crate::Errno::EPERM), "{checkpointed}");
            assert_eq!(taken.write(open, 0, &again), Ok(()), "{checkpointed}");
        }
    }

    /// TASK-165.3: an extent sealed by one commit and freed by the next, both in one segment, whose
    /// footer frees it and seals it not, is spent on a host that takes the branch, as it is on the
    /// store that made it: every reader of a footer spends every free of the branch's own extents.
    #[test]
    fn extents_sealed_and_freed_in_one_segment_are_spent_where_their_branch_is_taken() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let first = ExtentStore::create(dir.path().join("first"), 1 << 30).unwrap();
        let branch = first.branch();
        first.attach_remote(bucket.clone(), 1, idle()).unwrap();
        let sealed = write(&first, 1, &pattern(1, BLOCK));
        first.commit_ordered(Some(b"root")).unwrap();
        first.free(sealed).unwrap();
        first.commit(None).unwrap();
        let footers = footers(&bucket, branch);
        let last = footers.last().unwrap();
        let freed = storage_device::ExtentId::new(branch, sealed.sequence);
        let contents = &last.contents;
        let (commits, extents) = (contents.commits.len(), contents.extents.len());
        assert_eq!((commits, extents, &contents.frees), (1, 0, &vec![freed]));
        let data = pattern(3, BLOCK);
        let again = [crate::VolatileSlice::from(&data[..])];
        assert_eq!(first.write(sealed, 0, &again), Err(crate::Errno::EPERM));
        // Its host lost: the branch's state is its footers'.
        std::mem::forget(first);

        let taken = ExtentStore::create_for_branch(dir.path().join("taken"), 1 << 30, branch);
        let taken = taken.unwrap();
        taken.attach_remote(bucket, 2, idle()).unwrap();
        assert_eq!(taken.write(sealed, 0, &again), Err(crate::Errno::EPERM));
    }

    /// TASK-165.3: a branch's capacity is its state, set by the commit after a resize: a host that
    /// takes the branch has it, not the capacity it made its store with, whether the branch's state
    /// there comes from a checkpoint or from its segments' footers; and a store attaching to a
    /// branch that has none, or another, sets its own with its next commit.
    #[test]
    fn resizes_hold_where_their_branch_is_taken() {
        for checkpointed in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
            let first = ExtentStore::create(dir.path().join("first"), 1 << 30).unwrap();
            let branch = first.branch();
            first.attach_remote(bucket.clone(), 1, idle()).unwrap();
            let recovered = || {
                let recovered = recover(&*bucket, branch, None);
                futures::executor::block_on(recovered).unwrap().state.capacity
            };
            assert_eq!(recovered(), None, "{checkpointed}");
            first.commit(None).unwrap();
            assert_eq!(recovered(), Some(1 << 30), "{checkpointed}");
            write(&first, 1, &pattern(1, BLOCK));
            first.set_capacity(3 << 29).unwrap();
            first.commit(None).unwrap();
            assert_eq!(recovered(), Some(3 << 29), "{checkpointed}");
            if checkpointed {
                drop(first);
            } else {
                std::mem::forget(first);
            }

            let taken = ExtentStore::create_for_branch(dir.path().join("taken"), 1 << 30, branch);
            let taken = taken.unwrap();
            taken.attach_remote(bucket.clone(), 2, idle()).unwrap();
            assert_eq!(taken.info().unwrap().capacity, 3 << 29, "{checkpointed}");
            taken.set_capacity(1 << 31).unwrap();
            taken.commit(None).unwrap();
            assert_eq!(recovered(), Some(1 << 31), "{checkpointed}");
        }
    }

    /// TASK-141.2: a branch whose record says a format newer than this daemon's is refused, naming
    /// the branch and the format, before anything of it is read or written: no store takes it, its
    /// lease is as it was, and no segment of it is written.
    #[test]
    fn branches_of_newer_formats_are_refused() {
        use extent_remote::Bucket as _;
        use extent_remote::bucket::PutMode;
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let first = ExtentStore::create(dir.path().join("first"), 1 << 30).unwrap();
        let branch = first.branch();
        first.attach_remote(bucket.clone(), 1, idle()).unwrap();
        write(&first, 1, &pattern(1, BLOCK));
        first.commit(Some(b"root")).unwrap();
        drop(first);
        // A later release's record, whose format says first what follows, which this one can't
        // read.
        let mut newer = (extent_remote::layout::FORMAT + 1).to_le_bytes().to_vec();
        newer.extend_from_slice(b"what a later format holds");
        let (key, overwrite) = (extent_remote::layout::record(branch), PutMode::Overwrite);
        futures::executor::block_on(bucket.put(&key, newer, overwrite)).unwrap();
        let listed = || {
            let listed = futures::executor::block_on(bucket.list("")).unwrap();
            listed.into_iter().map(|meta| (meta.key, meta.version)).collect::<Vec<_>>()
        };
        let before = listed();

        let taken = ExtentStore::create_for_branch(dir.path().join("taken"), 1 << 30, branch);
        let refused = taken.unwrap().attach_remote(bucket.clone(), 2, idle());
        let format = extent_remote::layout::FORMAT;
        let named = format!(
            "branch {branch:x} is of format {}, which this reader, of format {format}, doesn't \
             read",
            format + 1
        );
        assert_eq!(refused.as_ref().map_err(ToString::to_string), Err(named));
        let newer = extent_remote::Error::Format { branch, format: format + 1 };
        assert_eq!(refused, Err(super::Failure::Remote(newer)));
        assert_eq!(listed(), before, "a branch of a newer format was written to");
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
    ) -> extent_remote::record::Record {
        write(parent, 1, &pattern(1, BLOCK));
        parent.commit(Some(b"root")).unwrap();
        parent.fork(path, child).unwrap();
        futures::executor::block_on(branch::record(&**bucket, child)).unwrap().unwrap()
    }

    /// TASK-79.20: so does a fork's child whose marker was lost, its host crashing once its store
    /// took the child's state, before the marker named the fork's commit: its store holds what its
    /// branch inherited.
    #[test]
    fn forks_whose_marker_was_lost_attach() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let parent = ExtentStore::create(dir.path().join("parent"), 1 << 30).unwrap();
        parent.attach_remote(bucket.clone(), 1, idle()).unwrap();
        let child = parent.branch() + 1;
        forked(&parent, &bucket, &dir.path().join("child"), child);
        std::fs::remove_file(dir.path().join("child").join(super::marker(child))).unwrap();

        let forked = ExtentStore::open(dir.path().join("child")).unwrap();
        assert_eq!(forked.attach_remote(bucket.clone(), 2, idle()), Ok(()));
        assert_eq!(forked.root(), Ok(b"root".to_vec()));
    }

    /// TASK-79.78: a fork to a child it made before, as a fork retried to another path, fails
    /// having made nothing, the bucket having the child's record: no commit of the parent's names
    /// the child again, from another point, which its writers, finding the child forked twice,
    /// would fail on for ever, the parent's branch taken by none again. The parent attaches again.
    #[test]
    fn forks_to_a_child_forked_before_make_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let parent = ExtentStore::create(dir.path().join("parent"), 1 << 30).unwrap();
        parent.attach_remote(bucket.clone(), 1, idle()).unwrap();
        let child = parent.branch() + 1;
        forked(&parent, &bucket, &dir.path().join("child"), child);
        let again = dir.path().join("again");
        let forked = parent.fork(&again, child);
        let errno = forked.as_ref().map(drop).map_err(crate::Error::errno);
        let forked = forked.map_err(|err| err.to_string());
        assert_eq!((forked, again.exists()), (anothers(child), false));
        assert_eq!(errno, Err(Some(crate::Errno::EEXIST)));
        drop(parent);

        let parent = ExtentStore::open(dir.path().join("parent")).unwrap();
        assert_eq!(parent.attach_remote(bucket.clone(), 2, idle()), Ok(()));
        let parents = futures::executor::block_on(recover(&*bucket, parent.branch(), None));
        let forks = parents.unwrap().state.forks;
        let forks: Vec<_> = forks.iter().map(|fork| (fork.child, fork.at, fork.point)).collect();
        assert_eq!(forks, [(child, 2, false)]);
    }

    /// TASK-165.7: a fork is the bucket's: a store with no uploader forks nothing (`ENOTSUP`),
    /// committing nothing, and leaving nothing at the fork's path. Before, it forked here, and its
    /// child took a record of its own once attached, uploading what it inherited.
    #[test]
    fn stores_with_no_uploader_fork_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let parent = ExtentStore::create(dir.path().join("parent"), 1 << 30).unwrap();
        write(&parent, 1, &pattern(1, BLOCK));
        parent.commit(Some(b"root")).unwrap();
        let epoch = parent.committed().unwrap();
        let path = dir.path().join("child");
        let forked = parent.fork(&path, parent.branch() + 1).map_err(|err| err.errno());
        let refused = (forked, path.exists(), parent.committed());
        assert_eq!(refused, (Err(Some(crate::Errno::ENOTSUP)), false, Ok(epoch)));
    }

    /// TASK-165.7: a fork whose host crashed as its segment uploaded leaves no store of its child
    /// here, the store being made only once the bucket has the child's record: the directory it
    /// began at the fork's path is no store, and goes as one left unmade. The segment, landing
    /// late, is the fork, which stands (doc-14): the parent's next writer gives the child its
    /// record, and a store naming the child, made anywhere, takes what it inherited. Before, the
    /// fork's handle, a store of the child's at once, took no branch of its own while the segment
    /// might land, but took its parent's branch itself, through markers naming the fork
    /// (TASK-79.72), and gave the child its record from the parent's state (TASK-79.20).
    #[test]
    fn forks_whose_host_crashed_stand_once_their_segment_lands() {
        let dir = tempfile::tempdir().unwrap();
        let memory: Arc<dyn object_store::ObjectStore> = Arc::new(InMemory::new());
        let gated = Arc::new(Gated::over(memory.clone()));
        let bucket = Arc::new(Bucket::new(memory, ""));
        let path = dir.path().join("child");
        let (branch, child, fork) = forking_held(dir.path(), &gated, &path);
        // The host gone: what it has in flight may land still.
        gated.shut.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(crate::host_store::unmade(&path), "the fork's directory is a store already");
        assert!(ExtentStore::open(&path).is_err(), "the fork's directory opens");
        crate::ExtentStore::delete(&path, None).unwrap();
        assert!(!path.exists());

        gated.hold.send_replace(Hold::Nothing);
        gated.wait_went(1);
        // Its fork goes on unseen, in a copy of the bucket only it reaches, as a dead host's.
        gated.sever();
        gated.shut.store(false, std::sync::atomic::Ordering::Relaxed);
        let next = extent_remote::Writer::open(bucket.clone(), branch, 3, 0);
        futures::executor::block_on(next).unwrap();
        let record = futures::executor::block_on(branch::record(&*bucket, child)).unwrap();
        let origin = extent_remote::layout::Origin { parent: branch, at: 2 };
        assert_eq!(record.map(|record| record.origin()), Some(Some(origin)));
        let taken = ExtentStore::create_for_branch(dir.path().join("taken"), 1 << 30, child);
        let taken = taken.unwrap();
        assert_eq!(taken.attach_remote(bucket.clone(), 4, idle()), Ok(()));
        assert_eq!(taken.root(), Ok(b"root".to_vec()));
        let mut data = vec![0; BLOCK];
        let extent = ExtentId { branch, sequence: 1 };
        taken.read(extent, 0, &mut [crate::VolatileSliceMut::from(&mut data[..])]).unwrap();
        assert_eq!(data, pattern(1, BLOCK));
        // The crashed host's fork, its writer reaching the bucket no more, fails, finding nothing
        // at its path, and deletes nothing there.
        let forked = fork.join().unwrap().map_err(|err| err.to_string());
        let unmade = format!("forking to branch {child:x}: making its store");
        assert_eq!((forked, path.exists()), (Err(unmade), false));
        let record = futures::executor::block_on(branch::record(&*bucket, child)).unwrap();
        assert_eq!(record.map(|record| record.deleted), Some(false));
    }

    /// Why a fork to `child` fails, its writer finding its branch taken.
    fn taken_over(child: u64) -> Result<(), String> {
        Err(format!("forking branch {child:x}: another writer took the branch"))
    }

    /// The host whose disk is at `from`, crashed, restarted with its disk as the crash left it at
    /// `to`: its stores copied there, no store of them open. What the crashed host's writes
    /// had in flight may land still, as a dead host's PUTs may.
    pub(super) fn restart(from: &std::path::Path, to: &std::path::Path) {
        copy_dir(from, to);
    }

    /// Copies the directory `from` to `to`, every file in it as it is.
    fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let (from, to) = (entry.path(), to.join(entry.file_name()));
            match entry.file_type().unwrap().is_dir() {
                true => copy_dir(&from, &to),
                false => drop(std::fs::copy(&from, &to).unwrap()),
            }
        }
    }

    /// A fork, waiting in a thread of its own: what it gives.
    type Forking = std::thread::JoinHandle<Result<(), crate::Error>>;

    /// A fork that fails once its segment landed, its parent's writer taken over before it
    /// acknowledged it (TASK-79.72): the parent, in `dir`, reaches the bucket, `memory`, through
    /// `gated`; its next writer takes its branch through a gate of its own, `next`, whose fence
    /// waits for the fork's segment, which it goes after, until it is let go. The parent holds an
    /// extent in a segment and one stored on its own, [`FailedFork::OWN`], which the child
    /// inherits.
    struct FailedFork {
        memory: Arc<dyn object_store::ObjectStore>,
        gated: Arc<Gated>,
        next: Arc<Gated>,
        branch: u64,
        child: u64,
    }

    /// A writer taking a branch, in a thread of its own: what it gives.
    type Taking = std::thread::JoinHandle<Result<(), extent_remote::Error>>;

    impl FailedFork {
        /// The parent's extent stored on its own.
        const OWN: u64 = 2;

        /// Forks the parent to a handle in `dir`, the fork's segment held, and the next writer,
        /// taking the parent's branch, waiting for it: the fork, waiting for the segment, and the
        /// next writer, in threads of their own.
        fn new(dir: &std::path::Path) -> (Self, Forking, Taking) {
            let memory: Arc<dyn object_store::ObjectStore> = Arc::new(InMemory::new());
            let (gated, next) = (Gated::over(memory.clone()), Gated::over(memory.clone()));
            let (gated, next) = (Arc::new(gated), Arc::new(next));
            let parent = Arc::new(ExtentStore::create(dir.join("parent"), 1 << 30).unwrap());
            parent.attach_remote(Arc::new(Bucket::new(gated.clone(), "")), 1, idle()).unwrap();
            let branch = parent.branch();
            write(&parent, 1, &pattern(1, BLOCK));
            write(&parent, Self::OWN, &pattern(2, super::LARGE as usize));
            parent.commit(Some(b"root")).unwrap();
            let forking = extent_remote::layout::segment(branch, 2);
            gated.hold.send_replace(Hold::Key(forking.clone()));
            next.hold.send_replace(Hold::Key(forking));
            let child = branch + 1;
            let fork = {
                let path = dir.join("child");
                std::thread::spawn(move || parent.fork(path, child))
            };
            gated.wait_held(1);
            let taking = {
                let bucket = Arc::new(Bucket::new(next.clone(), ""));
                let taking = extent_remote::Writer::open(bucket, branch, 3, 0);
                std::thread::spawn(move || futures::executor::block_on(taking).map(drop))
            };
            next.wait_held(1);
            (Self { memory, gated, next, branch, child }, fork, taking)
        }

        /// Lands the fork's segment, after which the parent's writer finds its branch taken, and
        /// the fork fails: the writes of what follows held as `then` says.
        fn land(&self, then: Hold) {
            self.gated.hold.send_replace(then);
            self.gated.wait_went(1);
        }

        /// The bucket as it is.
        fn bucket(&self) -> Arc<Bucket> {
            Arc::new(Bucket::new(self.memory.clone(), ""))
        }

        /// The child's record.
        fn record(&self) -> Option<extent_remote::record::Record> {
            futures::executor::block_on(branch::record(&*self.bucket(), self.child)).unwrap()
        }
    }

    /// A fork that failed, its writer taken over once the fork's segment landed, before it
    /// acknowledged it, leaves its child no branch in object storage, as it leaves it no handle:
    /// the child's branch there is deleted, so that the parent's next writer, recording the fork,
    /// finds it deleted, and the collector keeps nothing the child inherited for it.
    #[test]
    fn forks_failed_once_their_segment_landed_leave_their_child_no_branch() {
        let dir = tempfile::tempdir().unwrap();
        let (failed, fork, taking) = FailedFork::new(dir.path());
        failed.land(Hold::Nothing);
        let forked = fork.join().unwrap().map_err(|err| err.to_string());
        let handle = dir.path().join("child");
        assert_eq!((forked, handle.exists()), (taken_over(failed.child), false));
        failed.next.hold.send_replace(Hold::Nothing);
        assert_eq!(taking.join().unwrap(), Ok(()));

        // TASK-79.82: its deletion gave it the record its fork would first, naming the fork.
        let origin = extent_remote::layout::Origin { parent: failed.branch, at: 2 };
        // Of the fork's owner, which its footer names.
        let owner = failed.record().unwrap().owner;
        let deleted = extent_remote::record::Record {
            deleted: true,
            ..extent_remote::record::Kind::Claim(origin).of(owner)
        };
        assert_eq!(failed.record(), Some(deleted));
        let parents = futures::executor::block_on(recover(&*failed.bucket(), failed.branch, None));
        let fork = extent_remote::layout::Fork { child: failed.child, at: 2, point: false, owner };
        assert_eq!(parents.unwrap().state.forks, [fork]);
    }

    /// TASK-79.78: a fork that failed whose deletion in object storage is refused changes nothing
    /// there, and leaves nothing here: here a later placement takes the child's branch as the
    /// deletion's first write, of its lease, goes, so that the deletion, as this host's placement,
    /// refuses (`ESTALE`): the parent's next writer gives the child its record, live, the later
    /// placement's. Before, the fork's handle stayed, naming the branch, and its store held it
    /// here; now a child's store is made only once its fork is (TASK-165.7).
    #[test]
    fn forks_failed_whose_deletion_is_refused_leave_their_branch_to_its_placement() {
        let dir = tempfile::tempdir().unwrap();
        let (failed, fork, taking) = FailedFork::new(dir.path());
        let lease = extent_remote::layout::lease(failed.child);
        failed.land(Hold::Key(lease.clone()));
        failed.gated.wait_held(1);
        let later =
            futures::executor::block_on(branch::take_lease(&*failed.bucket(), &lease, 5, 5));
        assert_eq!(later, Ok(1));
        failed.gated.hold.send_replace(Hold::Nothing);
        let forked = fork.join().unwrap().map_err(|err| err.to_string());
        let path = dir.path().join("child");
        assert_eq!((forked, path.exists()), (taken_over(failed.child), false));
        failed.next.hold.send_replace(Hold::Nothing);
        assert_eq!(taking.join().unwrap(), Ok(()));

        let origin = extent_remote::layout::Origin { parent: failed.branch, at: 2 };
        let live = extent_remote::record::Kind::Claim(origin).of(0);
        assert_eq!(failed.record().map(unowned), Some(live));
    }

    /// TASK-184: two writers of one file of a store's directory at once each write a new file of
    /// their own, the second writing while the first has written its own and not yet put it in
    /// place, as two writes of a marker. Sharing one, the second truncated the first's and removed
    /// it, or renamed it, from under it: the uploader's write failed (`ENOENT`), stopping its
    /// uploads. (Its cases of a fork's claim went with claims, TASK-165.7.)
    #[test]
    fn writers_of_one_file_at_once_each_write_their_own() {
        use super::marker;
        type Wrote = Result<bool, String>;
        /// What `first` gives, and `second`, which writes as `first` has written its new file;
        /// and what is left in `dir`.
        fn at_once(
            dir: &std::path::Path,
            first: impl FnOnce() -> Wrote,
            second: impl FnOnce() -> Wrote + Send + 'static,
        ) -> (Wrote, Option<Wrote>, Vec<String>) {
            let (second, gave) = (Mutex::new(Some(second)), Arc::new(Mutex::new(None)));
            let guard = {
                let gave = gave.clone();
                // Once: the second's own new file passes here too.
                fail::FailGuard::with_callback("remote::written", move || {
                    let second = second.lock().unwrap().take();
                    if let Some(second) = second {
                        *gave.lock().unwrap() = Some(second());
                    }
                })
                .unwrap()
            };
            let first = first();
            drop(guard);
            let left = std::fs::read_dir(dir).unwrap();
            let mut left: Vec<_> =
                left.map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect();
            left.sort();
            (first, gave.lock().unwrap().take(), left)
        }

        // Two writes of a marker: the one renamed last stays.
        let dir = tempfile::tempdir().unwrap();
        let marked = |dir: &std::path::Path, tag: u64| {
            let marker = super::Marker::new(dir, 3);
            move || marker.write(Some(tag), []).map(|()| true).map_err(|err| err.to_string())
        };
        let gave = at_once(dir.path(), marked(dir.path(), 1), marked(dir.path(), 2));
        assert_eq!(gave, (Ok(true), Some(Ok(true)), vec![marker(3)]));
        assert_eq!(super::Marker::new(dir.path(), 3).read().unwrap(), [1]);
    }

    /// The record of a branch no fork made, its store's own, of no parent, but for its owner
    /// ([`unowned`]).
    fn own_record() -> extent_remote::record::Record {
        extent_remote::record::Kind::Root.of(0)
    }

    /// `record`, but for its owner, which its maker chose at random: as the tests that are not of
    /// owners compare records.
    fn unowned(record: extent_remote::record::Record) -> extent_remote::record::Record {
        extent_remote::record::Record { owner: 0, ..record }
    }

    /// The store of another branch, `branch`, at `path`, attached to `bucket`, holding an extent
    /// and a root: what a guest may name as its fork's child.
    fn others(path: &std::path::Path, bucket: &Arc<Bucket>, branch: u64) -> ExtentStore {
        let others = ExtentStore::create_for_branch(path, 1 << 30, branch).unwrap();
        others.attach_remote(bucket.clone(), 2, idle()).unwrap();
        write(&others, 1, &pattern(3, BLOCK));
        others.commit(Some(b"other")).unwrap();
        others
    }

    /// What a store committing, and its commit uploading, gave, and what its branch then holds.
    type Committed = (Result<(), crate::Errno>, Result<(), crate::Errno>, Uploaded);

    /// Whether the branch of `others`, its store, is live still: it commits, and uploads its
    /// commits.
    fn live(others: &ExtentStore, bucket: &Arc<Bucket>) -> Committed {
        write(others, 2, &pattern(4, BLOCK));
        let committed = others.commit(Some(b"again"));
        (committed, others.wait_uploaded(), uploaded(bucket, others.branch()))
    }

    /// What [`live`] gives for a branch live still.
    fn lives() -> Committed {
        let extents = vec![(1, pattern(3, BLOCK)), (2, pattern(4, BLOCK))];
        (Ok(()), Ok(()), (extents, Some(b"again".to_vec())))
    }

    /// TASK-79.82: a fork to a child whose ID another branch has in object storage, as a guest may
    /// name, fails with `EEXIST`, having made nothing: no footer of the parent's names the child,
    /// whose record the parent's writers would find another's, failing on it for ever, and the
    /// other branch is left as it was.
    #[test]
    fn forks_to_a_branch_the_bucket_has_fail_with_eexist() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let parent = ExtentStore::create(dir.path().join("parent"), 1 << 30).unwrap();
        parent.attach_remote(bucket.clone(), 1, idle()).unwrap();
        let other = parent.branch() + 1;
        let others = others(&dir.path().join("other"), &bucket, other);
        write(&parent, 1, &pattern(1, BLOCK));
        parent.commit(Some(b"root")).unwrap();
        let path = dir.path().join("child");
        let forked = parent.fork(&path, other).map_err(|err| err.errno());
        assert_eq!((forked, path.exists()), (Err(Some(crate::Errno::EEXIST)), false));

        // The parent commits and uploads on, its branch naming no fork.
        parent.commit(Some(b"later")).unwrap();
        assert_eq!(parent.wait_uploaded(), Ok(()));
        let parents = futures::executor::block_on(recover(&*bucket, parent.branch(), None));
        assert_eq!(parents.unwrap().state.forks, []);
        let record = futures::executor::block_on(branch::record(&*bucket, other)).unwrap();
        assert_eq!((record.map(unowned), live(&others, &bucket)), (Some(own_record()), lives()));
    }

    /// A parent attached to the bucket through `gated`, in `dir`, holding an extent and a root,
    /// and its fork to a child whose handle is `path`, waiting in a thread of its own for the
    /// fork's segment, which `gated` holds: the parent's branch and the child.
    fn forking_held(
        dir: &std::path::Path,
        gated: &Arc<Gated>,
        path: &std::path::Path,
    ) -> (u64, u64, Forking) {
        let parent = Arc::new(ExtentStore::create(dir.join("parent"), 1 << 30).unwrap());
        parent.attach_remote(Arc::new(Bucket::new(gated.clone(), "")), 1, idle()).unwrap();
        let branch = parent.branch();
        write(&parent, 1, &pattern(1, BLOCK));
        parent.commit(Some(b"root")).unwrap();
        gated.hold.send_replace(Hold::Key(extent_remote::layout::segment(branch, 2)));
        let child = branch + 1;
        let fork = {
            let path = path.to_owned();
            std::thread::spawn(move || parent.fork(path, child))
        };
        gated.wait_held(1);
        (branch, child, fork)
    }

    /// Why a fork to `child` fails whose parent's writer, giving the child its record, found it
    /// another branch's.
    fn anothers(child: u64) -> Result<(), String> {
        Err(format!("forking to branch {child:x}: another branch has its ID"))
    }

    /// TASK-79.82: a fork whose child's ID another branch took in object storage while the fork
    /// was being made, once it found the ID free, as a store of that branch made elsewhere then,
    /// fails, its parent's writer finding the child's record another's: and its cleanup deletes
    /// the child here alone, leaving the other branch as it was. A fork deletes no branch it did
    /// not make, whatever ID its guest gives. And it harms no parent: its writers go on, as a
    /// writer taking its branch again does, its footer naming a child that is another's.
    #[test]
    fn forks_failed_leave_branches_that_took_their_childs_id_be() {
        let dir = tempfile::tempdir().unwrap();
        let memory: Arc<dyn object_store::ObjectStore> = Arc::new(InMemory::new());
        let gated = Arc::new(Gated::over(memory.clone()));
        let bucket = Arc::new(Bucket::new(memory, ""));
        let path = dir.path().join("child");
        let (_, child, fork) = forking_held(dir.path(), &gated, &path);
        let others = others(&dir.path().join("other"), &bucket, child);
        gated.hold.send_replace(Hold::Nothing);
        let forked = fork.join().unwrap();
        let errno = forked.as_ref().map(drop).map_err(crate::Error::errno);
        let forked = forked.map_err(|err| err.to_string());
        assert_eq!((forked, errno), (anothers(child), Err(Some(crate::Errno::EEXIST))));
        assert!(!path.exists(), "the handle of a fork that failed is left");

        let record = futures::executor::block_on(branch::record(&*bucket, child)).unwrap();
        assert_eq!((record.map(unowned), live(&others, &bucket)), (Some(own_record()), lives()));
        let parent = ExtentStore::open(dir.path().join("parent")).unwrap();
        assert_eq!(parent.attach_remote(bucket.clone(), 3, idle()), Ok(()));
        parent.commit(Some(b"later")).unwrap();
        assert_eq!(parent.wait_uploaded(), Ok(()));
    }

    /// TASK-79.78, TASK-79.82: a fork that fails once its commit was made, here as its flush
    /// fails, is deleted in object storage, as one failing later is: the segment its commit ends,
    /// which names the fork, uploads after, and the parent's writer, giving the child its record,
    /// finds it deleted, so that no branch is kept there that no store holds. It leaves nothing at
    /// its path.
    #[test]
    fn forks_failed_once_committed_leave_their_child_no_branch() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let parent = ExtentStore::create(dir.path().join("parent"), 1 << 30).unwrap();
        parent.attach_remote(bucket.clone(), 1, idle()).unwrap();
        write(&parent, 1, &pattern(1, BLOCK));
        parent.commit(Some(b"root")).unwrap();
        let child = parent.branch() + 1;
        let path = dir.path().join("child");
        let flushed = fail::FailGuard::new("fork::flushed", "return").unwrap();
        let forked = parent.fork(&path, child).map_err(|err| format!("{err:#}"));
        drop(flushed);
        assert!(!path.exists(), "the fork's directory is left");

        parent.commit(Some(b"later")).unwrap();
        assert_eq!(parent.wait_uploaded(), Ok(()));
        let deleted = extent_remote::record::Record {
            deleted: true,
            ..extent_remote::record::Kind::Own.of(0)
        };
        let record = futures::executor::block_on(branch::record(&*bucket, child)).unwrap();
        assert_eq!(record.map(unowned), Some(deleted));
        // The checkpoint at the fork, its child deleted, names it no more.
        let parents = futures::executor::block_on(recover(&*bucket, parent.branch(), None));
        assert_eq!(parents.unwrap().state.forks, []);
        let unflushed = format!(
            "forking branch {child:x}: the fork to branch {child:#x} failed once its commit was \
             made: failpoint fork::flushed"
        );
        assert_eq!(forked, Err(unflushed));
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
            branch::create(&*bucket, 7, 0).await.unwrap();
            extent_remote::Writer::open(bucket.clone(), 7, 1, 0).await.unwrap()
        });
        let writer = Arc::new(writer);
        let path = dir.path().join(super::marker(7));
        let text = || std::fs::read_to_string(&path).unwrap();
        let mut in_flight = super::InFlight::new(super::Marker::new(dir.path(), 7));
        let uploading =
            |seq, tag| super::Uploading { seq, number: seq + 10, tag, forks: false };
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

        let cache = Arc::new(Cache::open(&dir.path().join("cache"), 1 << 30, 0).unwrap());
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
