// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own: a store's branch kept in object storage (backlog doc-7, Uploads; TASK-28.5).
//!
//! An uploader takes the branch's lease, uploads what the store holds that the branch doesn't, and
//! then, behind the store, the commits it makes durable: within an interval of each other, a batch
//! at a time, each batch one segment of the branch, whose extents freed within it are not uploaded
//! at all. Once a segment is acknowledged, the data of the extents its commits freed may go from
//! the store, and commits waiting to be durable (`COMMIT_DURABLE`) complete.

use extent_remote::cache::Cache;
use extent_remote::fetch::{self, Budget, Limits, Priority};
use extent_remote::store::{Store as Bucket, probe};
use extent_remote::{Commit, Writer, branch};
use extent_store::{Committed, HostDir, Store};
use futures::StreamExt as _;
use futures::future::Either;
use futures::stream::FuturesOrdered;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
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

/// Commits' tags in the branch: the writer's epoch above this bit, the store's epoch below, so that
/// no two writers' commits share one.
const TAG_SHIFT: u32 = 40;

/// Why uploading stopped, or a branch could not be uploaded to.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Failure {
    /// Another writer took the branch, or it was deleted.
    #[error("another writer took the branch")]
    Taken,
    /// Anything else, as it says.
    #[error("{0}")]
    Other(String),
}

impl From<extent_remote::Error> for Failure {
    fn from(error: extent_remote::Error) -> Self {
        match error {
            extent_remote::Error::Fenced | extent_remote::Error::Deleted => Self::Taken,
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
}

impl Default for Options {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(1),
            max_lag: 4096,
            max_lag_bytes: 1 << 30,
            cache: None,
        }
    }
}

/// A store's uploader. Dropping it uploads what the store made durable, then stops.
pub struct Remote {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

struct Shared {
    store: Arc<Store<HostDir>>,
    fetcher: Arc<Fetcher>,
    progress: Mutex<Progress>,
    changed: Condvar,
    /// Wakes the uploader: a commit waits, or it is to stop.
    wake: tokio::sync::Notify,
    options: Options,
}

/// The GETs of extents' data a host has in flight: one connection to object storage gives roughly
/// 80-100 MB/s, so a host needs tens (doc-7, Reading); each holds a part, of 1 MiB.
static BUDGET: std::sync::LazyLock<Arc<Budget>> =
    std::sync::LazyLock::new(|| Budget::new(Limits { gets: 64, bytes: 64 << 20 }));

/// Reads the extents a store holds whose data is in object storage ([`Store::adopt`]), on the
/// remote's runtime, which its uploads use too, under the host's budget of GETs.
pub struct Fetcher {
    runtime: tokio::runtime::Runtime,
    parts: Arc<fetch::Fetcher<Bucket>>,
    /// The branch's writer, once it took the branch.
    writer: std::sync::OnceLock<Arc<Writer<Bucket>>>,
}

impl Fetcher {
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
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("fxdax remote")
            .enable_all()
            .build()
            .map_err(|err| Failure::Other(format!("a runtime: {err}")))?;
        let cache = options.cache.clone();
        let runs = runtime.handle().clone();
        let spawn: fetch::Spawn = Arc::new(move |future| drop(runs.spawn(future)));
        let parts = fetch::Fetcher::new(bucket.clone(), BUDGET.clone(), cache).reading_ahead(spawn);
        let parts = Arc::new(parts);
        let fetcher = Arc::new(Fetcher { runtime, parts, writer: Default::default() });
        let shared = Arc::new(Shared {
            store,
            fetcher,
            progress: Mutex::new(Progress::default()),
            changed: Condvar::new(),
            wake: tokio::sync::Notify::new(),
            options,
        });
        let (started, start) = std::sync::mpsc::channel();
        let uploader = shared.clone();
        let thread = std::thread::Builder::new()
            .name("fxdax upload".into())
            .spawn(move || {
                let runtime = uploader.fetcher.runtime.handle().clone();
                runtime.block_on(uploader.run(bucket, holder, started));
            })
            .map_err(|err| Failure::Other(format!("starting the uploader: {err}")))?;
        match start.recv() {
            Ok(Ok(())) => Ok(Self { shared, thread: Some(thread) }),
            Ok(Err(err)) => {
                let _ = thread.join();
                Err(err)
            }
            Err(_) => Err(Failure::Other("the uploader stopped".into())),
        }
    }

    /// Waits until the commits up to `epoch` are uploaded and acknowledged.
    pub fn wait_uploaded(&self, epoch: u64) -> Result<(), Failure> {
        self.wait(epoch, false)
    }

    /// Waits, if more commits wait to be uploaded than `Options::max_lag`, with `epoch`'s, or they
    /// wrote more than `Options::max_lag_bytes`, until they no longer do.
    pub fn wait_lag(&self, epoch: u64) -> Result<(), Failure> {
        self.wait(epoch.saturating_sub(self.shared.options.max_lag), true)
    }

    fn wait(&self, epoch: u64, stall: bool) -> Result<(), Failure> {
        let store = &self.shared.store;
        let max_bytes = self.shared.options.max_lag_bytes;
        // Bytes only bound how far commits go ahead, not when one is durable.
        let within = |uploaded: u64| -> Result<bool, Failure> {
            Ok(uploaded >= epoch && (!stall || store.unreleased_bytes()? <= max_bytes))
        };
        let mut progress = self.shared.progress.lock().unwrap();
        if within(progress.uploaded)? {
            return Ok(());
        }
        progress.wanted = true;
        if stall {
            progress.stalls += 1;
            log::warn!(
                "fxdax: commits wait for object storage: {} not uploaded",
                epoch.saturating_sub(progress.uploaded)
            );
        }
        self.shared.changed.notify_all();
        self.shared.wake.notify_one();
        loop {
            if within(progress.uploaded)? {
                return Ok(());
            }
            if let Some(failed) = &progress.failed {
                return Err(failed.clone());
            }
            progress = self.shared.changed.wait(progress).unwrap();
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
        self.shared.changed.notify_all();
        self.shared.wake.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Shared {
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
            let (stop, forking) = {
                let mut progress = self.progress.lock().unwrap();
                wanted |= std::mem::take(&mut progress.wanted);
                (progress.stop, !progress.forks.is_empty())
            };
            if (wanted || due || stop || forking) && uploads.len() < IN_FLIGHT {
                (wanted, due) = (false, false);
                match self.close(&writer, &mut uploading) {
                    Ok(Some((seq, epoch))) => {
                        let writer = writer.clone();
                        uploads.push_back(async move { writer.upload(seq).await.map(|()| epoch) });
                        // More may have been made durable meanwhile.
                        wanted = true;
                        continue;
                    }
                    // Every commit made durable is uploaded: the forks go at its end.
                    Ok(None) if forking && uploads.is_empty() => {
                        if let Err(err) = self.fork(&writer).await {
                            return self.fail(&writer, err);
                        }
                        continue;
                    }
                    Ok(None) if stop && uploads.is_empty() => {
                        // A checkpoint on clean shutdown (doc-7, Checkpoints).
                        if let Err(err) = writer.checkpoint().await {
                            log::warn!("fxdax: checkpointing branch {:x}: {err}", writer.branch());
                        }
                        return self.fail(&writer, Failure::Other("the uploader stopped".into()));
                    }
                    Ok(None) => {}
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
                        Ok(epoch) => self.acknowledged(&writer, epoch, &mut uploading).await,
                        Err(err) => Err(err),
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
        self.changed.notify_all();
    }

    /// Makes the forks asked for at the end of the segments uploaded: each child gets its record
    /// once the segment of its fork is acknowledged.
    async fn fork(&self, writer: &Writer<Bucket>) -> Result<(), Failure> {
        let forks = std::mem::take(&mut self.progress.lock().unwrap().forks);
        for (child, answer) in forks {
            let forked = async {
                let seq = writer.fork(child)?;
                writer.upload(seq).await?;
                writer.acknowledge().await?;
                // And a checkpoint at the fork, which the child's recovery starts from.
                writer.checkpoint().await?;
                Ok::<_, Failure>(writer.state().last_commit)
            };
            let forked = forked.await;
            let failed = forked.as_ref().err().cloned();
            let _ = answer.send(forked);
            if let Some(err) = failed {
                return Err(err);
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
                let empty = self.store.extents()?.is_empty() && self.store.root()?.is_empty();
                match state.last_commit {
                    // Nothing committed, or the store's commit last.
                    None => {}
                    Some(last) if ours.contains(&last) => {}
                    Some(_) if empty && ours.is_empty() => self.take(&state)?,
                    Some(last) => {
                        return Err(Failure::Other(format!(
                            "branch {branch:x} moved on elsewhere (commit {last:#x}): its store \
                             here is stale"
                        )));
                    }
                }
            }
        }
        let writer = Writer::open(bucket.clone(), branch, holder).await?;
        self.progress.lock().unwrap().next_sequence = writer.next_sequence();
        let snapshot = self.store.upload()?;
        // What it holds is durable before it is uploaded.
        self.store.flush()?;
        let remote = writer.state();
        let mut commit = Commit { tag: self.tag(&writer, snapshot.epoch), ..Commit::default() };
        let held: BTreeSet<ExtentId> = snapshot.extents.iter().map(|(extent, _)| *extent).collect();
        for &(extent, len) in &snapshot.extents {
            if !remote.extents.contains_key(&extent) {
                self.add(&mut commit, extent, len)?;
            }
        }
        commit.frees =
            remote.extents.keys().filter(|extent| !held.contains(extent)).copied().collect();
        if remote.root.as_deref() != Some(&snapshot.root[..]) && !snapshot.root.is_empty() {
            commit.root = Some(snapshot.root.clone());
        }
        let changes = !commit.extents.is_empty()
            || !commit.own.is_empty()
            || !commit.frees.is_empty()
            || commit.root.is_some();
        if changes {
            self.send(&writer, commit).await?;
        }
        self.progress.lock().unwrap().uploaded = snapshot.epoch;
        Ok(writer)
    }

    /// Adds `extent`, of `len` bytes, read from the store, to `commit`.
    fn add(&self, commit: &mut Commit, extent: ExtentId, len: u64) -> Result<(), Failure> {
        let mut data = vec![0; len as usize];
        let read = self.store.read_kept(extent, 0, &mut data);
        read.map_err(|err| Failure::Other(format!("reading {extent:?}: {err:#}")))?;
        if len >= LARGE { &mut commit.own } else { &mut commit.extents }.push((extent, data));
        Ok(())
    }

    /// Closes a segment of the commits made durable since the last, as one commit, to be uploaded:
    /// returns its number and the last's epoch. `uploading` gains its tag.
    fn close(
        &self,
        writer: &Writer<Bucket>,
        uploading: &mut Vec<u64>,
    ) -> Result<Option<(u64, u64)>, Failure> {
        // What a waiter wants is durable before it is uploaded.
        self.store.flush()?;
        let committed = self.store.committed()?;
        let Some(last) = committed.last().map(|committed| committed.epoch) else {
            return Ok(None);
        };
        let batch = Batch::of(&committed);
        let mut commit =
            Commit { tag: self.tag(writer, last), root: batch.root, ..Commit::default() };
        commit.frees = batch.freed.into_iter().collect();
        for (extent, len) in batch.written {
            self.add(&mut commit, extent, len)?;
        }
        uploading.push(commit.tag);
        self.mark(writer.state().last_commit, uploading)?;
        writer.commit(commit)?;
        Ok(Some((writer.close().expect("a segment of a commit closes"), last)))
    }

    /// Acknowledges the segment uploaded of the store's commits up to `epoch`, the first of
    /// `uploading`, and those uploaded after it: its commits are uploaded.
    async fn acknowledged(
        &self,
        writer: &Writer<Bucket>,
        epoch: u64,
        uploading: &mut Vec<u64>,
    ) -> Result<(), Failure> {
        writer.acknowledge().await?;
        uploading.remove(0);
        self.mark(writer.state().last_commit, uploading)?;
        self.store.release(epoch)?;
        let mut progress = self.progress.lock().unwrap();
        progress.uploaded = progress.uploaded.max(epoch);
        self.changed.notify_all();
        Ok(())
    }

    /// Uploads `commit` as a segment, and acknowledges it.
    async fn send(&self, writer: &Writer<Bucket>, commit: Commit) -> Result<(), Failure> {
        let acked = writer.state().last_commit;
        self.mark(acked, &[commit.tag])?;
        writer.commit(commit)?;
        let seq = writer.close().expect("a segment of a commit closes");
        writer.upload(seq).await?;
        writer.acknowledge().await?;
        self.mark(writer.state().last_commit, &[])
    }

    /// The tag of the commit uploading the store's commits up to `epoch`.
    fn tag(&self, writer: &Writer<Bucket>, epoch: u64) -> u64 {
        (writer.epoch() << TAG_SHIFT) | epoch
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

/// What a batch of commits did together.
#[derive(Default)]
struct Batch {
    /// The extents written and not freed after, with their lengths.
    written: BTreeMap<ExtentId, u64>,
    /// The extents freed that none of them wrote.
    freed: BTreeSet<ExtentId>,
    /// The last root set.
    root: Option<Vec<u8>>,
}

impl Batch {
    /// What `committed`, in order, did.
    fn of(committed: &[Committed]) -> Self {
        let mut batch = Self::default();
        for committed in committed {
            for &(extent, len) in &committed.written {
                batch.written.insert(extent, len);
            }
            for &extent in &committed.freed {
                if batch.written.remove(&extent).is_none() {
                    batch.freed.insert(extent);
                }
            }
            if committed.root.is_some() {
                batch.root = committed.root.clone();
            }
        }
        batch
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
    fn uploaded(bucket: &Bucket, branch: u64) -> Uploaded {
        futures::executor::block_on(async {
            let state = recover(bucket, branch, None).await.unwrap().state;
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
        assert_eq!(store.info().unwrap().next_sequence, 2 << extent_remote::EPOCH_SHIFT);
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
        let other = extent_remote::Writer::open(bucket.clone(), store.branch(), 2);
        futures::executor::block_on(other).unwrap();
        write(&store, 2, &pattern(2, BLOCK));
        assert_eq!(store.commit(None), Err(crate::Errno::EROFS));
    }

    /// An object store whose requests fail while it is shut, and whose segments' writes wait
    /// while they are held.
    #[derive(Debug)]
    struct Gated {
        store: InMemory,
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
            Self {
                store: InMemory::new(),
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
        let sequence = second.info().unwrap().next_sequence;
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
        let sequence = forked.info().unwrap().next_sequence;
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
