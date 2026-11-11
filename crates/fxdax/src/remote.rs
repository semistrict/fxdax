// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own: what a host keeps its stores' branches in object storage with (backlog doc-7;
//! TASK-28.5): its configuration ([`Host`], [`host_token`]), the runtime its stores' uploaders and
//! fetches run on and their budgets ([`runtime`], [`Fetcher`]), and the errno of each failure
//! ([`errno`]). The uploader is extent_store's (`extent_store::upload`, TASK-165.9).

use extent_remote::cache::Cache;
use extent_remote::fetch::{self, Budget, Limits, Priority};
use extent_remote::layout::Location;
use extent_remote::store::Store as Bucket;
use extent_store::upload::{Failure, Runtime, UPLOAD_BYTES};
use std::sync::Arc;
use storage_device::ExtentId;

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
            Err(err) => Err(Failure::local(err, format!("FXDAX_EXTENTD_TOKEN {token:?}"))),
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

/// The errno of `failure`, as doc-4 and doc-7 have them: a branch this writer lost, to another
/// writer or deleted, or one nothing writes, is one the store takes no more commits for (`EROFS`);
/// one a later placement holds, which a placement cut off takes back no more, or one another
/// writer moved on from this host's write-ahead log, which the store opens no more until
/// `fxdaxctl take` drops it, `ESTALE`; a branch, or a point, gone, `ENOENT`; a fork's child's ID
/// taken, `EEXIST`; a branch of a newer format, `ENOTSUP`. None for what the store failed to read
/// or write, here or in object storage, which is the caller's `EIO`, logged.
pub(crate) fn errno(failure: &Failure) -> Option<crate::Errno> {
    use crate::Errno;
    use extent_remote::Error;
    match failure {
        Failure::Remote(Error::Fenced | Error::Displaced | Error::Deleted | Error::Point) => {
            Some(Errno::EROFS)
        }
        Failure::Remote(Error::Stale | Error::Diverged) => Some(Errno::ESTALE),
        Failure::Remote(Error::NoBranch) | Failure::PointDeleted => Some(Errno::ENOENT),
        Failure::Remote(Error::ChildTaken(_)) => Some(Errno::EEXIST),
        // A branch of a newer format than this host reads, until it is upgraded (doc-11).
        Failure::Remote(Error::Format { .. }) => Some(Errno::ENOTSUP),
        Failure::Remote(Error::Lost(_) | Error::Corrupt(_) | Error::Read(_))
        | Failure::Local(_) => None,
    }
}

impl From<crate::faults::Failpoint> for Failure {
    fn from(failpoint: crate::faults::Failpoint) -> Self {
        Self::from(anyhow::Error::new(failpoint))
    }
}

/// The runtime the host's stores' uploaders run on, and the budget of their uploads.
pub fn runtime() -> Runtime {
    Runtime { handle: RUNTIME.handle().clone(), uploads: UPLOADS.clone() }
}

/// fxdax: how many stores' uploaders run in this process, as it changes: what an extent daemon's
/// drain waits to see none of (TASK-141.4): Linux's alone, as the daemon is.
#[cfg(target_os = "linux")]
pub(crate) fn uploaders() -> tokio::sync::watch::Receiver<usize> {
    extent_store::upload::uploaders()
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
/// its GETs are. fxdax: in bytes, its upload memory ([`set_upload_memory`], TASK-317).
static UPLOADS: std::sync::LazyLock<Arc<Budget>> = std::sync::LazyLock::new(|| {
    let bytes = *UPLOAD_MEMORY.get_or_init(|| UPLOAD_BYTES);
    Budget::new(Limits { requests: 64, bytes })
});

/// fxdax: the most bytes of objects the host's uploads hold at once ([`UPLOADS`]), as
/// [`set_upload_memory`] set it, or [`UPLOAD_BYTES`].
static UPLOAD_MEMORY: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

/// fxdax: has the host's uploads hold at most `bytes` of the objects they upload at once
/// (TASK-317): extentd's `FXDAX_EXTENTD_UPLOAD_MEMORY`. However far behind the bucket its stores
/// fall, what waits for it is on the host's disk, their write-ahead logs, and what of it is in
/// memory is what this lets upload at once. Before any store opens; no less than the largest
/// object a store uploads, an extent's of 64 MiB, which goes alone.
pub fn set_upload_memory(bytes: u64) -> Result<(), String> {
    if bytes < crate::ExtentStore::MAX_EXTENT {
        return Err(format!(
            "an upload memory of {bytes} bytes: less than an extent's {}",
            crate::ExtentStore::MAX_EXTENT
        ));
    }
    match UPLOAD_MEMORY.set(bytes) {
        Ok(()) => Ok(()),
        Err(_) if UPLOAD_MEMORY.get() == Some(&bytes) => Ok(()),
        Err(_) => Err("the upload memory is set already, as stores opened".to_owned()),
    }
}

/// fxdax: what the host's uploads hold of the objects they upload, in bytes (TASK-317).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadMemory {
    /// Held now.
    pub held: u64,
    /// The most held at once.
    pub max_held: u64,
    /// The most they may hold at once ([`set_upload_memory`]).
    pub budget: u64,
}

/// fxdax: what the host's uploads hold now, the most they held, and their budget (TASK-317).
pub fn upload_memory() -> UploadMemory {
    let (held, max_held) = UPLOADS.held();
    UploadMemory { held, max_held, budget: UPLOADS.limits().bytes }
}

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

/// Reads the extents a store holds whose data is in object storage (`extent_store::Store::adopt`), at their
/// locations there, as the store names them, on the remotes' runtime, which their uploads use too,
/// under the host's budget of GETs.
pub struct Fetcher {
    runtime: tokio::runtime::Handle,
    parts: Arc<fetch::Fetcher<Bucket>>,
}

impl Fetcher {
    /// What reads extents' data from `bucket`, keeping the parts it fetches in `cache`, if given,
    /// the host's.
    pub(crate) fn new(bucket: Arc<Bucket>, cache: Option<Arc<Cache>>) -> Arc<Self> {
        let runtime = RUNTIME.handle().clone();
        let runs = runtime.clone();
        // A read ahead is for the guest request whose read started it, if any (TASK-260).
        let spawn: fetch::Spawn = Arc::new(move |future| {
            drop(runs.spawn(crate::request::within(crate::request::current(), future)))
        });
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
        // fxdax: for the guest request current, if any, there too: what the object store's
        // requests for it log (TASK-260).
        let read =
            self.runtime.spawn(crate::request::within(crate::request::current(), async move {
                Ok(parts.read(extent, &location, offset, len, Priority::Demand).await?)
            }));
        async move { read.await.map_err(|err| Failure::local(err, "fetching"))? }
    }
}

#[cfg(test)]
mod fork_faults;

#[cfg(test)]
mod restore_faults;

#[cfg(test)]
mod write_budgets;

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

    /// The URL `bucket` is this process's by, made so if it is none's yet: a memory bucket of its
    /// own, by its address, which the stores the tests make in it name (TASK-165.9).
    pub(super) fn url_of(bucket: &Arc<Bucket>) -> String {
        let url = format!("memory:///remote-tests-{:p}", Arc::as_ptr(bucket));
        crate::host_store::use_bucket(&url, bucket.clone());
        url
    }

    /// A new store at `path`, of `capacity` bytes, of `branch`, a new one if none, its branch kept
    /// in `bucket`, its uploader uploading as `keeping` says.
    pub(super) fn kept_in(
        path: impl AsRef<std::path::Path>,
        (capacity, branch): (u64, Option<u64>),
        bucket: &Arc<Bucket>,
        keeping: extent_store::upload::Options,
    ) -> Result<ExtentStore, crate::Error> {
        let branch = branch.unwrap_or_else(crate::host_store::new_branch);
        ExtentStore::create_kept_with(path, capacity, branch, &url_of(bucket), keeping)
    }

    /// Has the store at `path` name `bucket` as the bucket its branch is kept in: a host restarted
    /// whose stores reach the bucket otherwise than before, as the fault tests' do (TASK-165.9).
    /// A store whose host died before it described it names nothing.
    pub(super) fn naming(path: &std::path::Path, bucket: &Arc<Bucket>) {
        let description = path.join("store");
        let text = match std::fs::read_to_string(&description) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
            text => text.unwrap(),
        };
        let named: String = (text.lines())
            .map(|line| match line.starts_with("bucket ") {
                true => format!("bucket {}\n", url_of(bucket)),
                false => format!("{line}\n"),
            })
            .collect();
        std::fs::write(&description, named).unwrap();
    }

    /// The store at `path`, opened, its uploader uploading as `keeping` says.
    pub(super) fn reopened(
        path: impl AsRef<std::path::Path>,
        keeping: extent_store::upload::Options,
    ) -> Result<ExtentStore, crate::Error> {
        ExtentStore::open_with(
            path,
            crate::extents::Opening { keeping, ..crate::extents::Opening::here()? },
        )
    }

    /// Uploads only when a commit waits.
    /// How this process's stores keep their branches ([`crate::host_store::keeping`]), but
    /// uploading nothing until a commit waits for it, or the hour is up.
    pub(super) fn idle() -> extent_store::upload::Options {
        let keeping = crate::host_store::keeping().unwrap();
        extent_store::upload::Options { interval: Duration::from_secs(3600), ..keeping }
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
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), "branches"));
        let store =
            kept_in(dir.path().join("store"), (1 << 30, None), &bucket.clone(), idle()).unwrap();
        let branch = store.branch();
        write(&store, 1, &pattern(1, BLOCK));
        store.commit(Some(b"first")).unwrap();
        // What it held before is uploaded on attaching.
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
        let store = reopened(dir.path().join("store"), idle()).unwrap();
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

    /// doc-7, Fencing: a writer whose create fails on an earlier writer's segment, landed late where
    /// its own was to go, takes the branch again, which keeps its guest. A store taking a branch
    /// over from a writer still uploading, two segments of it in flight, the second landing past the
    /// fence, where the store's first goes, commits on.
    #[test]
    fn takeovers_of_writers_still_uploading_keep_their_guests() {
        use extent_remote::{Batch, Commit, Writer};
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let (branch, token) =
            (crate::host_store::new_branch(), crate::host_store::token().unwrap());
        // The writer before, its two segments in flight.
        let before = futures::executor::block_on(async {
            branch::create(&*bucket, branch, 1).await.unwrap();
            Writer::open(bucket.clone(), branch, 10, token).await.unwrap()
        });
        let mut seqs = Vec::new();
        for tag in [1, 2] {
            let commit = Commit { tag, ..Commit::default() };
            let objects = Batch::from(commit).close(&mut before.chain()).frame();
            seqs.push(before.add_segment(objects).unwrap());
        }
        // A store takes the branch over, opening, its fence after the writer's.
        let path = dir.path().join("store");
        made_for(&path, branch, &bucket);
        let store = reopened(&path, idle()).unwrap();
        // The second lands, past the fence, void, where the store's first goes; the first finds
        // the fence at its place.
        futures::executor::block_on(async {
            before.upload(seqs[1]).await.unwrap();
            assert_eq!(before.upload(seqs[0]).await, Err(extent_remote::Error::Fenced));
        });
        // The store's writer, displaced, takes the branch again, and its commit is the branch's.
        write(&store, 1, &pattern(1, BLOCK));
        let epoch = store.remote().unwrap().status().unwrap().lease.unwrap().epoch;
        let committed = store.commit(Some(b"after"));
        let remote = store.remote().unwrap();
        assert_eq!((committed, remote.failed()), (Ok(()), None));
        assert_eq!(remote.status().unwrap().lease.unwrap().epoch, epoch + 1);
        let after = (vec![(1, pattern(1, BLOCK))], Some(b"after".to_vec()));
        assert_eq!(uploaded(&bucket, branch), after);
    }

    /// TASK-195: a store reads what it holds after the collector compacted it twice, its writer
    /// checkpointing between, with the host's cache and without. It names an extent where the
    /// writer's checkpoints move it, as the writer does (`Writer::following`), from where the
    /// collector keeps the moves: compacted from its segment into moves, and from those into moves
    /// of moves, the segment goes, and the first moves. Before, the store named it in the segment
    /// as long as it held it: gone, and its moves gone, the guest's read failed (`EIO`) on a live
    /// branch, which a recovery read through the moves of moves.
    #[test]
    fn extents_compacted_twice_read_where_the_store_names_them() {
        compacted_twice(false);
        compacted_twice(true);
    }

    /// TASK-217: a store whose host died, its disk as the death left it and the host's cache lost
    /// with it, opens and reads what it holds after the collector compacted its segment and took
    /// it, the writer's checkpoint having followed the moves: opening names its extents as the
    /// branch's newest checkpoint does, in the moves. Before (GCE r757, round 40, a soak's daemon
    /// killed and its cache lost), the store opened from its own index, which named the extent in
    /// the segment the collector had taken, and the checking VM's reads failed EIO (`lost: ... in
    /// segments/<branch>/<seq>`); TASK-165.9 removed the index, and the opening recovers the branch.
    #[test]
    fn extents_compacted_read_once_their_host_died_and_its_cache_went() {
        const EXTENT: usize = 64 * BLOCK;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store");
        let objects: Arc<dyn object_store::ObjectStore> = Arc::new(InMemory::new());
        // The dying host's way to the objects, and what reaches them after it.
        let gated = Arc::new(Gated::over(objects.clone()));
        let reached = Arc::new(Bucket::new(gated.clone(), ""));
        let bucket = Arc::new(Bucket::new(objects, ""));
        let options = extent_remote::collector::Options { compact_below: 0.5, grace: 0 };
        let collect = |holder| {
            let pass = extent_remote::collector::collect(&*bucket, holder, &options);
            futures::executor::block_on(pass).unwrap()
        };
        let cache = Some(Arc::new(Cache::open(&dir.path().join("cache"), 1 << 30, 0).unwrap()));
        let keeping = extent_store::upload::Options { cache, ..idle() };
        // One segment, of extents 1 to 16, which don't compress, freed but 1 to 3.
        let store = kept_in(&path, (16 * EXTENT as u64, None), &reached, keeping).unwrap();
        let branch = store.branch();
        for sequence in 1..=16 {
            write(&store, sequence, &noise(sequence, EXTENT));
        }
        store.commit(None).unwrap();
        for sequence in 4..=16 {
            store.free(ExtentId { branch, sequence }).unwrap();
        }
        let checkpointed = || {
            for _ in 0..extent_store::upload::CHECKPOINT_EVERY {
                store.commit(None).unwrap();
            }
        };
        checkpointed();
        // The segment compacted, 1 to 3 into its moves; the writer's checkpoint follows them; the
        // segment goes.
        assert_eq!(collect(1).compacted, 1);
        checkpointed();
        collect(2);
        let keys: Vec<String> =
            futures::executor::block_on(extent_remote::Bucket::list(&*bucket, ""))
                .unwrap()
                .into_iter()
                .map(|meta| meta.key)
                .collect();
        let moves = format!("moves/segments/{branch:016x}/");
        let first = format!("segments/{branch:016x}/{:016x}", 1);
        assert!(keys.iter().any(|key| key.starts_with(&moves)), "{keys:?}");
        assert!(!keys.contains(&first), "{keys:?}");
        // The host dies: its disk copied as it is, its cache not, its store cut off.
        let restarted = dir.path().join("restarted");
        restart(&path, &restarted);
        gated.sever();
        naming(&restarted, &bucket);
        let opened = reopened(&restarted, idle()).unwrap();
        let mut data = vec![0; EXTENT];
        let read = opened.read(
            ExtentId { branch, sequence: 1 },
            0,
            &mut [crate::VolatileSliceMut::from(&mut data[..])],
        );
        assert_eq!(read.map(|()| data == noise(1, EXTENT)), Ok(true));
        drop(store);
    }

    /// TASK-195: so does a fork's store read what it inherited, its parent deleted: it names the
    /// extents where its own writer's checkpoints move them, from the parent's segment, compacted
    /// into moves, then into moves of moves, the segment and the first moves gone.
    #[test]
    fn forks_read_what_they_inherited_compacted_twice() {
        const EXTENT: usize = 64 * BLOCK;
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let options = extent_remote::collector::Options { compact_below: 0.5, grace: 0 };
        let collect = |holder| {
            let pass = extent_remote::collector::collect(&*bucket, holder, &options);
            futures::executor::block_on(pass).unwrap()
        };
        // The parent's segment, of extents 1 to 16, which don't compress, freed but 1 to 3; its
        // fork, which inherits them there; and the parent deleted.
        let path = dir.path().join("parent");
        let parent = kept_in(&path, (16 * EXTENT as u64, None), &bucket, idle()).unwrap();
        let branch = parent.branch();
        for sequence in 1..=16 {
            write(&parent, sequence, &noise(sequence, EXTENT));
        }
        parent.commit(None).unwrap();
        for sequence in 4..=16 {
            parent.free(ExtentId { branch, sequence }).unwrap();
        }
        parent.commit(None).unwrap();
        let child = dir.path().join("child");
        parent.fork(&child, branch + 1).unwrap();
        drop(parent);
        crate::host_store::delete(&path, Some(&bucket), crate::host_store::token).unwrap();
        // The fork's store, open all along: the segment compacted, 1 to 3 in its moves.
        let child = reopened(&child, idle()).unwrap();
        assert_eq!(collect(1).compacted, 1);
        // It frees 2 and 3, and its writer checkpoints, naming 1 in the moves.
        child.free(ExtentId { branch, sequence: 2 }).unwrap();
        child.free(ExtentId { branch, sequence: 3 }).unwrap();
        for _ in 0..extent_store::upload::CHECKPOINT_EVERY {
            child.commit(None).unwrap();
        }
        // The segment goes, no state naming it there; the moves are compacted, then go too.
        collect(3);
        collect(4);
        let keys: Vec<String> =
            futures::executor::block_on(extent_remote::Bucket::list(&*bucket, ""))
                .unwrap()
                .into_iter()
                .map(|meta| meta.key)
                .filter(|key| key.starts_with("moves/") || key.starts_with("segments/"))
                .collect();
        let once = format!("moves/segments/{branch:016x}/");
        let twice = format!("moves/moves/segments/{branch:016x}/");
        assert!(keys.iter().any(|key| key.starts_with(&twice)), "{keys:?}");
        assert!(!keys.iter().any(|key| key.starts_with(&once)), "{keys:?}");
        let mut data = vec![0; EXTENT];
        let extent = ExtentId { branch, sequence: 1 };
        let read = child.read(extent, 0, &mut [crate::VolatileSliceMut::from(&mut data[..])]);
        assert_eq!(read.map(|()| data == noise(1, EXTENT)), Ok(true));
    }

    /// [`extents_compacted_twice_read_where_the_store_names_them`], with the host's cache if
    /// `cached`.
    fn compacted_twice(cached: bool) {
        const EXTENT: usize = 64 * BLOCK;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store");
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let options = extent_remote::collector::Options { compact_below: 0.5, grace: 0 };
        let collect = |holder| {
            let pass = extent_remote::collector::collect(&*bucket, holder, &options);
            futures::executor::block_on(pass).unwrap()
        };
        let cache =
            cached.then(|| Arc::new(Cache::open(&dir.path().join("cache"), 1 << 30, 0).unwrap()));
        let keeping = extent_store::upload::Options { cache, ..idle() };
        // One segment, of extents 1 to 16, which don't compress, freed but 1 to 3.
        let store = kept_in(&path, (16 * EXTENT as u64, None), &bucket, keeping).unwrap();
        let store = Arc::new(store);
        let branch = store.branch();
        for sequence in 1..=16 {
            write(&store, sequence, &noise(sequence, EXTENT));
        }
        store.commit(None).unwrap();
        for sequence in 4..=16 {
            store.free(ExtentId { branch, sequence }).unwrap();
        }
        // Its writer checkpoints, as every so many segments, each commit here one.
        let checkpointed = || {
            for _ in 0..extent_store::upload::CHECKPOINT_EVERY {
                store.commit(None).unwrap();
            }
        };
        checkpointed();
        // The segment compacted: 1 to 3 in its moves.
        assert_eq!(collect(1).compacted, 1);
        // The store frees 2 and 3, and its writer checkpoints, as every so many segments, naming 1
        // in the moves, which hold a third of what they did.
        store.free(ExtentId { branch, sequence: 2 }).unwrap();
        store.free(ExtentId { branch, sequence: 3 }).unwrap();
        checkpointed();
        // A follower of the store, which reads what the store names.
        let follower = store.follow();
        // The segment goes, no state naming it there; the moves are compacted, then go too.
        collect(3);
        collect(4);
        let keys: Vec<String> =
            futures::executor::block_on(extent_remote::Bucket::list(&*bucket, ""))
                .unwrap()
                .into_iter()
                .map(|meta| meta.key)
                .filter(|key| key.starts_with("moves/") || key.starts_with("segments/"))
                .collect();
        let twice = format!("moves/moves/segments/{branch:016x}/");
        let once = format!("moves/segments/{branch:016x}/");
        assert!(keys.iter().any(|key| key.starts_with(&twice)), "{keys:?}");
        assert!(!keys.iter().any(|key| key.starts_with(&once)), "{keys:?}");
        // The store, open all along, reads it there, and so does its follower.
        let extent = ExtentId { branch, sequence: 1 };
        let reads = |store: &ExtentStore| {
            let mut data = vec![0; EXTENT];
            let read = store.read(extent, 0, &mut [crate::VolatileSliceMut::from(&mut data[..])]);
            read.map(|()| data == noise(1, EXTENT))
        };
        assert_eq!([reads(&store), reads(&follower)], [Ok(true), Ok(true)], "cached: {cached}");
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

    /// TASK-165.9: a store whose bucket was out of reach holds its commits in its write-ahead log,
    /// segments the commits closed, which upload once it is in reach again, in order, each the
    /// object its file is, after the writer's fence; what the commits made, and the root, the
    /// branch has then. (Before, a store attaching uploaded what its branch lacked in batches.)
    #[test]
    fn logs_caught_up_upload_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let gated = Arc::new(Gated::new());
        let bucket = Arc::new(Bucket::new(gated.clone(), ""));
        let store = kept_in(dir.path().join("store"), (1 << 30, None), &bucket, idle()).unwrap();
        let branch = store.branch();
        gated.shut.store(true, std::sync::atomic::Ordering::Relaxed);
        let mut written = Vec::new();
        for sequence in 1..=3 {
            let data = pattern(sequence as u8, 2 * BLOCK);
            write(&store, sequence, &data);
            written.push((sequence, data));
            store.commit_ordered(Some(format!("root {sequence}").as_bytes())).unwrap();
        }
        store.flush().unwrap();
        // Out of reach: the store holds what it committed, the bucket nothing of it.
        let behind = Arc::new(Bucket::new(gated.store.clone(), ""));
        assert_eq!(uploaded(&behind, branch), (vec![], None));
        gated.shut.store(false, std::sync::atomic::Ordering::Relaxed);
        store.wait_uploaded().unwrap();
        let footers = footers(&bucket, branch);
        let commits: Vec<_> = footers.iter().map(|footer| footer.contents.commits.len()).collect();
        assert_eq!(commits, [0, 1], "the fence, then the segment of the commits");
        assert_eq!(uploaded(&bucket, branch), (written, Some(b"root 3".to_vec())));
    }

    /// A store whose commit, of more bytes than `options` let wait, is in a segment closed for want
    /// of room, as a closing that beats the uploader leaves it: with no commit to come, as one in
    /// the lag's wait holds its guest's seal, and holds back every other. And the commit's epoch.
    fn closed_for_room(
        options: extent_store::upload::Options,
    ) -> (tempfile::TempDir, crate::host_store::Opened, u64) {
        const EXTENT: usize = 1 << 20;
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let path = dir.path().join("store");
        let opened =
            crate::host_store::create_unmade(&path, 1 << 30, 7, Some(&url_of(&bucket)), options);
        let opened = opened.unwrap();
        let store = &opened.store;
        store.write(super::ExtentId::new(7, 1), 0, &pattern(1, EXTENT)).unwrap();
        store.commit_ordered(None).unwrap();
        let epoch = store.epoch().unwrap();
        // The most a segment holds, its last extent past it, and no commit with them.
        for sequence in 2..98 {
            let extent = super::ExtentId::new(7, sequence);
            store.write(extent, 0, &pattern(sequence as u8, EXTENT)).unwrap();
        }
        (dir, opened, epoch)
    }

    /// TASK-150.2: a commit stalled on the bytes waiting (`Options::max_lag_bytes`) ends though
    /// what it waits for is in a segment closed for want of room, which only a commit's flush would
    /// end, as the commit, or its guest, have none to come: an uploader's own flush ends it.
    #[test]
    fn commits_stalled_on_the_lag_end_with_their_segment_closed_for_room() {
        let lag = extent_store::upload::Options { max_lag_bytes: BLOCK as u64, ..idle() };
        let (_dir, crate::host_store::Opened { store, remote, .. }, epoch) = closed_for_room(lag);
        let stalled = std::sync::Arc::new(remote);
        let waiting = stalled.clone();
        let (ended, end) = std::sync::mpsc::channel();
        std::thread::spawn(move || ended.send(waiting.wait_lag(epoch)).unwrap());
        assert_eq!(end.recv_timeout(Duration::from_secs(30)), Ok(Ok(())));
        assert_eq!(store.wal_bytes().unwrap(), 0);
    }

    /// TASK-150.2: so does a sync of the store's commits, or anything else waiting for their upload.
    #[test]
    fn uploads_waited_for_end_with_their_segment_closed_for_room() {
        let (_dir, crate::host_store::Opened { remote, .. }, epoch) = closed_for_room(idle());
        let (ended, end) = std::sync::mpsc::channel();
        std::thread::spawn(move || ended.send(remote.wait_uploaded(epoch)).unwrap());
        assert_eq!(end.recv_timeout(Duration::from_secs(30)), Ok(Ok(())));
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
        let opened = crate::host_store::create(&path, 1 << 30, branch, idle()).unwrap();
        let crate::host_store::Opened { store, remote, .. } = opened;
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
    #[ignore = "a benchmark: --run-ignored only, alone"]
    fn bench_sustained_uploads() {
        use object_store::throttle::{ThrottleConfig, ThrottledStore};
        const WRITTEN: u64 = 4 << 30;
        const EXTENT: usize = 1 << 20;
        let put = Duration::from_millis(350);
        let config = ThrottleConfig { wait_put_per_call: put, ..ThrottleConfig::default() };
        let objects = Arc::new(ThrottledStore::new(InMemory::new(), config));
        let bucket = Arc::new(Bucket::new(objects, ""));
        let dir = tempfile::tempdir().unwrap();
        let store = kept_in(
            dir.path().join("store"),
            (2 * WRITTEN, None),
            &bucket,
            extent_store::upload::Options::default(),
        )
        .unwrap();
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
        let memory = Arc::new(InMemory::new());
        let bucket = Arc::new(Bucket::new(memory.clone(), ""));
        let cache = Arc::new(Cache::open(&dir.path().join("cache"), 1 << 30, 0).unwrap());
        let options = extent_store::upload::Options { cache: Some(cache.clone()), ..idle() };
        let store =
            kept_in(dir.path().join("store"), (1 << 30, None), &bucket.clone(), options).unwrap();
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
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let store =
            kept_in(dir.path().join("store"), (1 << 30, None), &bucket.clone(), idle()).unwrap();
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
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let store =
            kept_in(dir.path().join("store"), (1 << 30, None), &bucket.clone(), idle()).unwrap();
        write(&store, 1, &pattern(1, BLOCK));
        store.commit(None).unwrap();
        let other = extent_remote::Writer::open(
            bucket.clone(),
            store.branch(),
            2,
            crate::host_store::token().unwrap(),
        );
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
        let first = kept_in(&first_path, (1 << 30, None), &bucket.clone(), idle()).unwrap();
        let branch = first.branch();
        ExtentStore::place(&first_path, 1).unwrap();
        write(&first, 1, &pattern(1, BLOCK));
        first.commit(Some(b"first")).unwrap();

        // The branch moves to another host, a later placement, while the first still runs.
        drop(kept_in(&second_path, (1 << 30, Some(branch)), &bucket, idle()).unwrap());
        ExtentStore::place(&second_path, 2).unwrap();
        let second = reopened(&second_path, idle()).unwrap();
        assert_eq!(second.root(), Ok(b"first".to_vec()));
        write(&first, 2, &pattern(2, BLOCK));
        assert_eq!(first.commit(None), Err(crate::Errno::EROFS));
        drop((first, second));

        // The first, restarted, takes it back no more: its token is the earlier.
        let refused = reopened(&first_path, idle()).err().and_then(|err| err.errno());
        assert_eq!(refused, Some(crate::Errno::ESTALE));
        let lease = futures::executor::block_on(extent_remote::Bucket::get(
            &*bucket,
            &extent_remote::layout::lease(branch),
        ));
        let lease: extent_remote::layout::Lease =
            extent_remote::layout::decode(&lease.unwrap().bytes).unwrap();
        assert_eq!(lease.token, 2);

        // Placed later still, it does.
        ExtentStore::place(&first_path, 3).unwrap();
        let later = reopened(&first_path, idle()).unwrap();
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
        let first = kept_in(&first_path, (1 << 30, None), &bucket.clone(), idle()).unwrap();
        let branch = first.branch();
        ExtentStore::place(&first_path, 1).unwrap();
        write(&first, 1, &pattern(1, BLOCK));
        first.commit(Some(b"first")).unwrap();
        drop(first);
        // The branch moves to another host, a later placement.
        drop(kept_in(&second_path, (1 << 30, Some(branch)), &bucket, idle()).unwrap());
        ExtentStore::place(&second_path, 2).unwrap();
        let second = reopened(&second_path, idle()).unwrap();

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
        let options = extent_store::upload::Options { token: 9, ..idle() };
        let store =
            kept_in(dir.path().join("store"), (1 << 30, None), &bucket.clone(), options).unwrap();
        let lease = futures::executor::block_on(extent_remote::Bucket::get(
            &*bucket,
            &extent_remote::layout::lease(store.branch()),
        ));
        let lease = extent_remote::layout::Lease::decode(&lease.unwrap().bytes).unwrap();
        assert_eq!(lease.token, 9);
    }

    /// TASK-165.9: a store whose bucket acknowledged commits of its branch, opened where the
    /// bucket has no such branch, deleted or another bucket, refuses (`ENOENT`), rather than make
    /// the branch anew without them.
    #[test]
    fn stores_whose_bucket_lacks_their_branch_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let path = dir.path().join("store");
        let store = kept_in(&path, (1 << 30, None), &bucket, idle()).unwrap();
        write(&store, 1, &pattern(1, BLOCK));
        store.commit(Some(b"acknowledged")).unwrap();
        drop(store);
        let other = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        naming(&path, &other);
        let refused = reopened(&path, idle()).err().and_then(|err| err.errno());
        assert_eq!(refused, Some(crate::Errno::ENOENT));
        // Its own bucket named again, it opens as it was.
        naming(&path, &bucket);
        assert_eq!(reopened(&path, idle()).unwrap().root(), Ok(b"acknowledged".to_vec()));
    }

    /// TASK-192: a store dropped uploads what it committed, its open segment's commits too, as
    /// `extent_store::upload`'s docs say: its uploader closes the open segment, uploads it and
    /// stops, before the store closes. Before, the store closed first, and its uploader found the
    /// open segment's commits it could no longer close.
    #[test]
    fn dropped_stores_upload_their_open_segments_commits() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let store =
            kept_in(dir.path().join("store"), (1 << 30, None), &bucket.clone(), idle()).unwrap();
        let branch = store.branch();
        write(&store, 1, &pattern(1, BLOCK));
        store.commit_ordered(Some(b"ordered")).unwrap();
        // Not uploaded yet: the uploader waits an hour, unless asked.
        assert_eq!(uploaded(&bucket, branch), (vec![], None));
        drop(store);
        assert_eq!(
            uploaded(&bucket, branch),
            (vec![(1, pattern(1, BLOCK))], Some(b"ordered".to_vec()))
        );
    }

    /// TASK-39.7: waiting for uploads returns once every commit so far is in the branch.
    #[test]
    fn waits_for_uploads() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let store =
            kept_in(dir.path().join("store"), (1 << 30, None), &bucket.clone(), idle()).unwrap();
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

    /// An object store whose requests fail while it is shut, whose writes fail while it is full,
    /// and whose writes wait while they are held, go wrong as it is armed to, or reach a copy of
    /// its objects once it is severed.
    #[derive(Debug)]
    pub(super) struct Gated {
        store: Arc<dyn object_store::ObjectStore>,
        shut: std::sync::atomic::AtomicBool,
        /// Its writes fail as a file bucket's on a full disk do, `ENOSPC` behind a generic error
        /// (TASK-314).
        full: std::sync::atomic::AtomicBool,
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
                full: Default::default(),
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

        /// Whether it has room for a write: not while full.
        fn room(&self) -> object_store::Result<()> {
            match self.full.load(std::sync::atomic::Ordering::Relaxed) {
                true => Err(object_store::Error::Generic {
                    store: "gated",
                    source: Box::new(std::io::Error::from_raw_os_error(libc::ENOSPC)),
                }),
                false => Ok(()),
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
            self.room()?;
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
            self.room()?;
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

    /// While object storage is out of reach, or `failing` otherwise, commits go on until the lag
    /// reaches its bound, as `options` set it, two commits of a block each, then wait, failing
    /// nothing, the bucket's failures counted (TASK-314); they go on once it answers again, and
    /// every commit is uploaded.
    fn waits_at_the_lag_bound(
        options: extent_store::upload::Options,
        failing: fn(&Gated) -> &std::sync::atomic::AtomicBool,
    ) {
        use extent_remote::store::Request;
        let dir = tempfile::tempdir().unwrap();
        let gated = Arc::new(Gated::new());
        let bucket = Arc::new(Bucket::new(gated.clone(), ""));
        let store = Arc::new(
            kept_in(dir.path().join("store"), (1 << 30, None), &bucket.clone(), options).unwrap(),
        );
        let creates_failed = bucket.stats().failures(Request::Create);
        failing(&gated).store(true, std::sync::atomic::Ordering::Relaxed);
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
        while remote.stalls() == 0 || bucket.stats().failures(Request::Create) == creates_failed {
            std::thread::yield_now();
        }
        assert!(!third.is_finished());
        assert_eq!(remote.failed(), None, "the uploader stopped");
        failing(&gated).store(false, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(third.join().unwrap(), Ok(()));
        store.commit(None).unwrap();
        let (extents, _) = uploaded(&bucket, store.branch());
        assert_eq!(extents.iter().map(|(sequence, _)| *sequence).collect::<Vec<_>>(), [1, 2, 3]);
    }

    /// TASK-317: what a store's uploads hold in memory while the bucket takes none of them, however
    /// far behind they fall, is their budget, the host's upload memory: segments of incompressible
    /// data, a MiB an extent, each closed by a commit waiting for its upload, twice as many as the
    /// store uploads at once, which the bucket holds unanswered. What waits is in the store's
    /// write-ahead log, on disk.
    #[test]
    fn uploads_a_bucket_takes_none_of_hold_no_more_than_their_budget() {
        // Seven segments' room, of 32 MiB and their tails each: more than a framing holds.
        const BUDGET: u64 = 256 << 20;
        // What a store's framing holds besides, one segment at a time: its data, and its framed
        // bytes, twice as they grow (`extent_store::segments`, TASK-325).
        const FRAMING: u64 = 4 * (32 << 20);
        // And the heap's own: the store's and the bucket's, and a commit's buffers.
        const SLACK: u64 = 16 << 20;
        super::set_upload_memory(BUDGET).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let gated = Arc::new(Gated::new());
        let bucket = Arc::new(Bucket::new(gated.clone(), ""));
        let options =
            extent_store::upload::Options { max_lag: u64::MAX, max_lag_bytes: u64::MAX, ..idle() };
        let store = Arc::new(
            kept_in(dir.path().join("store"), (8 << 30, None), &bucket.clone(), options).unwrap(),
        );
        // The writer's fence is in; the commits' segments are held.
        gated.hold.send_replace(Hold::Segments);
        let segments = 2 * extent_store::upload::IN_FLIGHT;
        let mut waiting = Vec::new();
        let data = noise(1, 1 << 20);
        let heap = crate::heap::peak_while(|| {
            let mut sequence = 0;
            for _ in 0..segments {
                for _ in 0..32 {
                    sequence += 1;
                    write(&store, sequence, &data);
                    store.commit_ordered(None).unwrap();
                }
                let committing = store.clone();
                waiting.push(std::thread::spawn(move || committing.commit(None)));
            }
            // As many as the budget lets go at once: seven at least, or more of segments closed
            // short, as commits waiting close them.
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            let held = || gated.held.load(std::sync::atomic::Ordering::Relaxed);
            while held() < 7 && std::time::Instant::now() < deadline {
                std::thread::yield_now();
            }
            assert!(held() >= 7, "{} uploads held", held());
        });
        let memory = super::upload_memory();
        let behind = store.remote().unwrap().status().unwrap().pending_bytes;
        gated.hold.send_replace(Hold::Nothing);
        for commit in waiting {
            assert_eq!(commit.join().unwrap(), Ok(()));
        }
        eprintln!("heap {} MiB, {memory:?}, {} MiB behind", heap >> 20, behind >> 20);
        assert!(behind >= (segments as u64) << 25, "{behind} bytes behind");
        assert_eq!(memory.budget, BUDGET);
        assert!(memory.max_held <= BUDGET, "{memory:?}");
        assert!(heap <= BUDGET + FRAMING + SLACK, "{} MiB held", heap >> 20);
    }

    /// Segments upload at once: a commit made while those before it upload goes up with them.
    #[test]
    fn segments_upload_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let gated = Arc::new(Gated::new());
        let bucket = Arc::new(Bucket::new(gated.clone(), ""));
        let store = Arc::new(
            kept_in(dir.path().join("store"), (1 << 30, None), &bucket.clone(), idle()).unwrap(),
        );
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
        let gated = Arc::new(Gated::new());
        let bucket = Arc::new(Bucket::new(gated.clone(), ""));
        let store = Arc::new(
            kept_in(dir.path().join("store"), (1 << 30, None), &bucket.clone(), idle()).unwrap(),
        );
        let branch = store.branch();
        // The writer's fence is segment 0; the commits' are 1 to 4.
        gated.hold.send_replace(Hold::SegmentsFrom(1));
        let waiting: Vec<_> = (1..=extent_store::upload::IN_FLIGHT as u64)
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
        let ordered = extent_store::upload::IN_FLIGHT as u64 + 1;
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
        assert_eq!(closed.len(), extent_store::upload::IN_FLIGHT + 1);
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
        use extent_remote::layout;
        use std::sync::atomic::Ordering::Relaxed;
        let dir = tempfile::tempdir().unwrap();
        let gated = Arc::new(Gated::over(store.clone()));
        let max_lag = 2;
        let options = extent_store::upload::Options {
            interval: Duration::from_millis(1),
            max_lag,
            ..Default::default()
        };
        let lost = Arc::new(
            kept_in(
                dir.path().join("lost"),
                (1 << 30, None),
                &Arc::new(Bucket::new(gated.clone(), prefix)),
                options,
            )
            .unwrap(),
        );
        let branch = lost.branch();
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
        // waiting, writing to the host's disk, and takes the segment framed, the waiting commit
        // last, to upload. Only from then on does nothing write to the disk, the uploads refused,
        // never acknowledged: lost sooner, the disk would go as the uploader writes to it, which
        // runs on, forgotten (TASK-165.9: the write-ahead log's segments, before a marker).
        let last = lost.committed().unwrap();
        let segments = dir.path().join("lost").join(format!("segments/{branch:016x}"));
        let framed = || -> Vec<u64> {
            let files = std::fs::read_dir(&segments).into_iter().flatten();
            let bytes = files.filter_map(|file| std::fs::read(file.unwrap().path()).ok());
            let footers = bytes.filter_map(|bytes| layout::footer::<layout::Footer>(&bytes).ok());
            footers.flat_map(|footer| footer.contents.commits).collect()
        };
        while !framed().iter().any(|&tag| extent_remote::layout::epoch_of(tag) == last) {
            // Returned, the commit waiting says the uploader failed, which never framed it.
            assert!(!waiting.is_finished(), "the commit waiting returned, its segment not framed");
            std::thread::yield_now();
        }
        // The host is lost: its store and uploader, forgotten, never reach object storage again,
        // and its disk is gone: renamed away at once, as its threads, forgotten, may still make
        // files in it, which a removal racing them finds (TASK-301: a log becoming a spare).
        std::mem::forget(lost);
        drop(waiting);
        std::fs::rename(dir.path().join("lost"), dir.path().join("gone")).unwrap();

        let bucket = Arc::new(Bucket::new(store, prefix));
        let taken =
            kept_in(dir.path().join("taken"), (1 << 30, Some(branch)), &bucket.clone(), idle())
                .unwrap();
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
        let lost =
            kept_in(dir.path().join("lost"), (1 << 30, None), &bucket.clone(), idle()).unwrap();
        let branch = lost.branch();
        let extent = write(&lost, 1, &pattern(1, BLOCK));
        lost.free(extent).unwrap();
        write(&lost, 1, &pattern(2, BLOCK));
        lost.commit(None).unwrap();
        // The host is lost, what it committed in object storage.
        std::mem::forget(lost);

        let taken =
            kept_in(dir.path().join("taken"), (1 << 30, Some(branch)), &bucket, idle()).unwrap();
        let mut data = vec![0; BLOCK];
        taken.read(extent, 0, &mut [crate::VolatileSliceMut::from(&mut data[..])]).unwrap();
        assert!(data == pattern(2, BLOCK), "the extent read back is not its second write");
    }

    /// TASK-92.6: a store taking a branch its host lost, the branch's segments since its last
    /// checkpoint many, recovers the branch once, its lease taken between the start and the rest:
    /// by the tails of its segments, which hold their footers, a window of them at once, not by
    /// the segments whole. The bucket, which the lost host's store probed, it probes not again.
    /// TASK-165.11: what a durable commit, an open, a fork and a fork's first open cost on a real
    /// bucket, FXDAX_PHASES_BUCKET's (`s3://bucket/prefix`, under a prefix of the run's own), by
    /// phase: each one's time, and the bucket's requests it made, by kind, their count and time
    /// (`Stats::request_ns`), the rest local (closing and framing the segment, flushing). A
    /// measurement, not a check: ignored, run on a host that reaches the bucket, as
    /// scripts/test-host.sh runs it with `FXDAX_TEST_PARTS=workspace` and
    /// `FXDAX_TEST_ARGS='--run-ignored only --no-capture bucket_round_trips_by_phase'`. Each
    /// operation is a line, `PHASE ...`.
    #[test]
    #[ignore = "a measurement against FXDAX_PHASES_BUCKET's bucket"]
    fn bucket_round_trips_by_phase() {
        use extent_remote::store::Request;
        let url = std::env::var("FXDAX_PHASES_BUCKET").expect("FXDAX_PHASES_BUCKET names a bucket");
        let url = format!("{}/phases-{:016x}/", url.trim_end_matches('/'), rand::random::<u64>());
        let bucket = crate::host_store::bucket(&url).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut phases: std::collections::BTreeMap<&str, Vec<f64>> = Default::default();
        // Runs `operation`, a `phase`, and says what it took and asked of the bucket.
        let mut measure = |phase: &'static str, operation: &mut dyn FnMut()| {
            let (before, started) = (bucket.stats(), std::time::Instant::now());
            operation();
            let took = started.elapsed().as_secs_f64() * 1000.0;
            let after = bucket.stats();
            let mut line = format!("PHASE {phase} ms={took:.1}");
            let mut waited = 0.0;
            for request in Request::ALL {
                let count = after.requests(request) - before.requests(request);
                let ns = after.request_ns(request) - before.request_ns(request);
                if count > 0 {
                    line += &format!(" {}={count}/{:.1}ms", request.name(), ns as f64 / 1e6);
                    waited += ns as f64 / 1e6;
                }
            }
            println!("{line} bucket-ms={waited:.1}");
            phases.entry(phase).or_default().push(took);
        };
        let base = dir.path().join("base");
        let branch = crate::host_store::new_branch();
        let mut store = None;
        measure("create", &mut || {
            let made = ExtentStore::create_kept_with(&base, 1 << 30, branch, &url, idle());
            store = Some(made.unwrap());
        });
        let store = store.unwrap();
        for sequence in 1..=20 {
            write(&store, sequence, &pattern(sequence as u8, BLOCK));
            measure("durable-commit", &mut || store.commit(None).unwrap());
        }
        drop(store);
        for _ in 0..10 {
            measure("open", &mut || drop(reopened(&base, idle()).unwrap()));
        }
        let store = reopened(&base, idle()).unwrap();
        let forks: Vec<_> = (0..10).map(|fork| dir.path().join(format!("fork-{fork}"))).collect();
        for fork in &forks {
            let child = crate::host_store::new_branch();
            measure("fork", &mut || store.fork(fork, child).unwrap());
        }
        drop(store);
        for fork in &forks {
            measure("fork-open", &mut || drop(reopened(fork, idle()).unwrap()));
        }
        for (phase, mut took) in phases {
            took.sort_by(f64::total_cmp);
            println!(
                "PHASES {phase} n={} median-ms={:.1} min-ms={:.1} max-ms={:.1}",
                took.len(),
                took[took.len() / 2],
                took[0],
                took[took.len() - 1]
            );
        }
    }

    /// TASK-165.11: a durable commit, as a guest's fsync makes one, costs the bucket what doc-7's
    /// Uploads and Fencing say it needs, one after the other: the create of the segment its commit
    /// closes, and a read of the lease after it, which acknowledges it; nothing else, as no
    /// checkpoint is due and no fork made. On GCS each is a round trip (GCE r740: a guest's
    /// fdatasync about 130 ms).
    #[test]
    fn durable_commits_cost_their_segments_create_and_a_lease_read() {
        use extent_remote::store::Request;
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let store =
            kept_in(dir.path().join("store"), (1 << 30, None), &bucket.clone(), idle()).unwrap();
        write(&store, 1, &pattern(1, BLOCK));
        store.commit(None).unwrap();
        let before = bucket.stats();
        write(&store, 2, &pattern(2, BLOCK));
        store.commit(None).unwrap();
        let after = bucket.stats();
        let asked: Vec<_> = (Request::ALL.iter())
            .map(|&request| (request.name(), after.requests(request) - before.requests(request)))
            .collect();
        assert_eq!(
            asked,
            [
                ("get", 1),
                ("get-range", 0),
                ("head", 0),
                ("list", 0),
                ("create", 1),
                ("update", 0),
                ("overwrite", 0),
                ("delete", 0),
            ]
        );
    }

    #[test]
    fn takes_recover_their_branches_once_by_their_segments_tails() {
        use extent_remote::layout;
        use extent_remote::store::Request;
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let lost =
            kept_in(dir.path().join("lost"), (1 << 30, None), &bucket.clone(), idle()).unwrap();
        let branch = lost.branch();
        // Its writer's fence, then a segment of a MiB for each commit, uploaded as it is durable.
        for sequence in 1..=4 {
            write(&lost, sequence, &noise(sequence, 1 << 20));
            lost.commit(None).unwrap();
        }
        // The host is lost, its uploader stopping where it is, with no checkpoint.
        std::mem::forget(lost);

        let before = bucket.stats();
        let taken =
            kept_in(dir.path().join("taken"), (1 << 30, Some(branch)), &bucket.clone(), idle())
                .unwrap();
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
    #[ignore = "needs an S3 bucket: scripts/test-rustfs.sh, CI's object-stores job"]
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
        let options = extent_store::upload::Options { interval, max_lag: 2, ..Default::default() };
        waits_at_the_lag_bound(options, |gated| &gated.shut);
    }

    #[test]
    fn commits_wait_at_the_lag_bound_in_bytes() {
        let interval = Duration::from_millis(1);
        let options = extent_store::upload::Options {
            interval,
            max_lag_bytes: 2 * BLOCK as u64,
            ..Default::default()
        };
        waits_at_the_lag_bound(options, |gated| &gated.shut);
    }

    /// TASK-314: a bucket that keeps failing its writes, its disk full, holds its stores' commits
    /// at the lag bound, failing none and losing none: as the guest sees it, its fsyncs wait once
    /// the lag reaches its bound (its syncs at once), until the bucket has room (doc-12).
    #[test]
    fn commits_wait_at_the_lag_bound_while_the_bucket_is_full() {
        let interval = Duration::from_millis(1);
        let options = extent_store::upload::Options { interval, max_lag: 2, ..Default::default() };
        waits_at_the_lag_bound(options, |gated| &gated.full);
    }

    /// A branch opens on a host without it: an empty store of it takes what the branch holds, and
    /// writes on; the store it came from, opened again, finds the branch moved on, and refuses.
    #[test]
    fn opens_branches_elsewhere() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let first =
            kept_in(dir.path().join("first"), (1 << 30, None), &bucket.clone(), idle()).unwrap();
        let branch = first.branch();
        let large = pattern(2, 4 << 20);
        write(&first, 1, &pattern(1, BLOCK));
        write(&first, 2, &large);
        first.commit(Some(b"root")).unwrap();
        drop(first);

        let second =
            kept_in(dir.path().join("second"), (1 << 30, Some(branch)), &bucket.clone(), idle())
                .unwrap();
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

        // fxdax: the first, opened again, its write-ahead log empty, takes the branch as it is
        // (TASK-165.9): before, a store that held the branch refused once it moved on.
        let first = reopened(dir.path().join("first"), idle()).unwrap();
        assert_eq!(first.root(), Ok(b"root".to_vec()));
        let mut data = vec![0; BLOCK];
        let extent = ExtentId { branch, sequence };
        first.read(extent, 0, &mut [crate::VolatileSliceMut::from(&mut data[..])]).unwrap();
        assert_eq!(data, pattern(3, BLOCK));
        drop(first);
        let (extents, root) = uploaded(&bucket, branch);
        assert_eq!(
            extents.iter().map(|(sequence, _)| *sequence).collect::<Vec<_>>(),
            [1, 2, sequence]
        );
        assert_eq!(root, Some(b"root".to_vec()));
    }

    /// TASK-260: a read for a guest request of what only object storage holds logs the requests
    /// it makes of the bucket, on the remotes' runtime, with the guest request.
    #[cfg(target_os = "linux")]
    #[test]
    fn reads_of_object_storage_are_logged_with_their_guest_request() {
        use crate::request::{RequestId, within_now};
        let lines = crate::extentd::logging::capture();
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let first =
            kept_in(dir.path().join("first"), (1 << 30, None), &bucket.clone(), idle()).unwrap();
        let branch = first.branch();
        write(&first, 1, &pattern(1, BLOCK));
        first.commit(None).unwrap();
        drop(first);
        let second =
            kept_in(dir.path().join("second"), (1 << 30, Some(branch)), &bucket.clone(), idle())
                .unwrap();
        lines.lock().unwrap().clear();
        let request = RequestId {
            guest: std::num::NonZeroU64::new(42).unwrap(),
            store: Some("/stores/vm1".into()),
        };
        let mut data = vec![0; BLOCK];
        let extent = ExtentId { branch, sequence: 1 };
        within_now(Some(request), || {
            second.read(extent, 0, &mut [crate::VolatileSliceMut::from(&mut data[..])]).unwrap()
        });
        assert_eq!(data, pattern(1, BLOCK));
        // Each request of the bucket the read made: its kind, and the request it names.
        let mut made: Vec<(String, String)> = (lines.lock().unwrap().iter())
            .filter_map(|line| line.strip_prefix("DEBUG extent_remote::store: object storage: "))
            .map(|line| {
                let kind = line.split(' ').next().unwrap().to_owned();
                (kind, line.split_once(" request_id=").map_or("none", |(_, id)| id).to_owned())
            })
            .collect();
        made.dedup();
        assert_eq!(made, [("get-range".to_owned(), "42 store=/stores/vm1".to_owned())]);
        drop(second);
    }

    /// Opens the store at `path`, whose bucket's objects `gated` reaches, the write of its branch's
    /// lease held until `meanwhile` ran.
    fn open_racing(
        path: std::path::PathBuf,
        gated: &Arc<Gated>,
        branch: u64,
        meanwhile: impl FnOnce(),
    ) -> Result<ExtentStore, crate::Error> {
        gated.hold.send_replace(Hold::Key(extent_remote::layout::lease(branch)));
        let opening = std::thread::spawn(move || reopened(path, idle()));
        gated.wait_held(1);
        meanwhile();
        gated.hold.send_replace(Hold::Nothing);
        opening.join().unwrap()
    }

    /// The directory of a store of `branch` at `path`, its branch kept in `bucket`, made whole, and
    /// not opened: what opening takes the branch into, as the bucket holds it.
    fn made_for(path: &std::path::Path, branch: u64, bucket: &Arc<Bucket>) {
        std::fs::create_dir(path).unwrap();
        let description =
            extent_store::Description { block_size: BLOCK as u32, capacity: 1 << 30, branch };
        extent_store::create_host(path, description, &url_of(bucket), true).unwrap();
        extent_store::made_host(path).unwrap();
    }

    /// TASK-79.4: a store opening takes the branch as of its lease, not as of what it found
    /// before: the commit another writer made meanwhile, which frees the extent the branch held
    /// before and writes another, is the branch's, and the store's.
    #[test]
    fn takes_take_what_the_branch_holds_once_leased() {
        let dir = tempfile::tempdir().unwrap();
        let gated = Arc::new(Gated::new());
        let bucket = Arc::new(Bucket::new(gated.clone(), ""));
        let first = kept_in(dir.path().join("first"), (1 << 30, None), &bucket, idle()).unwrap();
        let branch = first.branch();
        let before = write(&first, 1, &pattern(1, BLOCK));
        first.commit(Some(b"before")).unwrap();

        made_for(&dir.path().join("second"), branch, &bucket);
        let second = open_racing(dir.path().join("second"), &gated, branch, || {
            write(&first, 2, &pattern(2, BLOCK));
            first.free(before).unwrap();
            first.commit(Some(b"meanwhile")).unwrap();
        });
        let second = second.unwrap();
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

    /// TASK-79.4, TASK-165.9: a store whose write-ahead log holds commits the branch moved past
    /// refuses to open (`ESTALE`), those commits never acknowledged: before its lease is taken, its
    /// log diverged as recovery first finds the branch; and once it holds it, another writer having
    /// committed between the two (`wal_follows`), having fenced that writer, whose commits stay the
    /// branch's. `ExtentStore::take` drops the log, and opens the store as the branch is.
    #[test]
    fn stores_whose_branch_moved_on_before_their_lease_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let memory: Arc<dyn object_store::ObjectStore> = Arc::new(InMemory::new());
        let gated = Arc::new(Gated::over(memory.clone()));
        let bucket = Arc::new(Bucket::new(gated.clone(), ""));
        // The first host's, through a gate of its own.
        let dying = Arc::new(Gated::over(memory));
        let lost = dir.path().join("lost");
        let first =
            kept_in(&lost, (1 << 30, None), &Arc::new(Bucket::new(dying.clone(), "")), idle());
        let first = first.unwrap();
        let branch = first.branch();
        write(&first, 1, &pattern(1, BLOCK));
        first.commit(Some(b"before")).unwrap();
        // A commit its log holds, durable on its disk alone, as its host dies: restarted with its
        // disk as it was, reaching the bucket again; what it had going does nothing more.
        write(&first, 2, &pattern(2, BLOCK));
        first.commit_ordered(Some(b"local")).unwrap();
        first.flush().unwrap();
        let path = dir.path().join("first");
        restart(&lost, &path);
        dying.sever();
        drop(first);
        naming(&path, &bucket);

        // The branch moves to another host, which takes it, as the bucket has it.
        let taken = kept_in(dir.path().join("taken"), (1 << 30, Some(branch)), &bucket, idle());
        let taken = taken.unwrap();
        assert_eq!(taken.root(), Ok(b"before".to_vec()));
        // Its writer's fence moved the branch past no commit: the first's log follows it still.
        let opened = open_racing(path.clone(), &gated, branch, || {
            let info = taken.info().unwrap();
            let sequence = info.next_sequence.max(info.sequence_floor);
            write(&taken, sequence, &pattern(3, BLOCK));
            taken.commit(Some(b"meanwhile")).unwrap();
        });
        assert_eq!(opened.err().and_then(|err| err.errno()), Some(crate::Errno::ESTALE));
        // The writer that committed meanwhile was fenced, its commit the branch's.
        let (extents, root) = uploaded(&bucket, branch);
        assert_eq!(extents.len(), 2);
        assert_eq!(root, Some(b"meanwhile".to_vec()));
        drop(taken);
        // Plainly diverged now, before the lease.
        assert_eq!(
            reopened(&path, idle()).err().and_then(|err| err.errno()),
            Some(crate::Errno::ESTALE)
        );
        // Taken, its log dropped: the store holds the branch as it is, its local commit lost.
        ExtentStore::take(&path, 1 << 30, branch, None).unwrap();
        let first = reopened(&path, idle()).unwrap();
        assert_eq!(first.root(), Ok(b"meanwhile".to_vec()));
    }

    /// A fork of a store kept in object storage is a fork of its branch there, recorded once the
    /// parent uploaded through it: it inherits the parent's extents rather than upload them again,
    /// and what either commits after is its own.
    /// TASK-160.10: a point's store, which its bucket says is one, refuses all that
    /// would change what it holds, with `EROFS`, whoever asks: writes, frees, commits, a resize,
    /// a writer taking it over. What changes none of its extents it
    /// does: it forks, and is made public. Before, it refused none of these itself, and a resize
    /// of one grew it.
    #[test]
    fn points_refuse_what_would_change_them() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let source =
            kept_in(dir.path().join("source"), (1 << 30, None), &bucket.clone(), idle()).unwrap();
        let extent = write(&source, 1, &pattern(1, BLOCK));
        source.commit(Some(b"root")).unwrap();
        let path = dir.path().join("point");
        source.point(&path, source.branch() + 1).unwrap();
        drop(source);

        let point = reopened(&path, idle()).unwrap();
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
        let parent =
            kept_in(dir.path().join("parent"), (1 << 30, None), &bucket.clone(), idle()).unwrap();
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

        let forked = reopened(dir.path().join("child"), idle()).unwrap();
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
        let parent = Arc::new(
            kept_in(dir.path().join("parent"), (1 << 30, None), &bucket.clone(), idle()).unwrap(),
        );
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

        let forked = reopened(dir.path().join("child"), idle()).unwrap();
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
            let first = kept_in(dir.path().join("first"), (1 << 30, None), &bucket.clone(), idle())
                .unwrap();
            let branch = first.branch();
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

            let taken = kept_in(dir.path().join("taken"), (1 << 30, Some(branch)), &bucket, idle())
                .unwrap();
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
        let first =
            kept_in(dir.path().join("first"), (1 << 30, None), &bucket.clone(), idle()).unwrap();
        let branch = first.branch();
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

        let taken =
            kept_in(dir.path().join("taken"), (1 << 30, Some(branch)), &bucket, idle()).unwrap();
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
            let first = kept_in(dir.path().join("first"), (1 << 30, None), &bucket.clone(), idle())
                .unwrap();
            let branch = first.branch();
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

            let taken =
                kept_in(dir.path().join("taken"), (1 << 30, Some(branch)), &bucket.clone(), idle())
                    .unwrap();
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
        let first =
            kept_in(dir.path().join("first"), (1 << 30, None), &bucket.clone(), idle()).unwrap();
        let branch = first.branch();
        write(&first, 1, &pattern(1, BLOCK));
        first.commit(Some(b"root")).unwrap();
        drop(first);
        // A later release's record, whose format says first what follows, which this one can't
        // read.
        let mut newer = (extent_remote::layout::FORMAT + 1).to_le_bytes().to_vec();
        newer.extend_from_slice(b"what a later format holds");
        let (key, overwrite) = (extent_remote::layout::record(branch), PutMode::Overwrite);
        futures::executor::block_on(bucket.put(&key, newer.into(), overwrite)).unwrap();
        let listed = || {
            let listed = futures::executor::block_on(bucket.list("")).unwrap();
            listed.into_iter().map(|meta| (meta.key, meta.version)).collect::<Vec<_>>()
        };
        let before = listed();

        let taken = kept_in(dir.path().join("taken"), (1 << 30, Some(branch)), &bucket, idle());
        let refused = taken.err().unwrap();
        let format = extent_remote::layout::FORMAT;
        let named = format!(
            "branch {branch:x} is of format {}, which this reader, of format {format}, doesn't \
             read",
            format + 1
        );
        assert!(format!("{refused:#}").contains(&named), "{refused:#}");
        assert_eq!(refused.errno(), Some(crate::Errno::ENOTSUP));
        assert_eq!(listed(), before, "a branch of a newer format was written to");
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

    /// TASK-79.78: a fork to a child it made before, as a fork retried to another path, fails
    /// having made nothing, the bucket having the child's record: no commit of the parent's names
    /// the child again, from another point, which its writers, finding the child forked twice,
    /// would fail on for ever, the parent's branch taken by none again. The parent attaches again.
    #[test]
    fn forks_to_a_child_forked_before_make_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(Arc::new(InMemory::new()), ""));
        let parent =
            kept_in(dir.path().join("parent"), (1 << 30, None), &bucket.clone(), idle()).unwrap();
        let child = parent.branch() + 1;
        forked(&parent, &bucket, &dir.path().join("child"), child);
        let again = dir.path().join("again");
        let forked = parent.fork(&again, child);
        let errno = forked.as_ref().map(drop).map_err(crate::Error::errno);
        let forked = forked.map_err(|err| err.to_string());
        assert_eq!((forked, again.exists()), (anothers(child), false));
        assert_eq!(errno, Err(Some(crate::Errno::EEXIST)));
        drop(parent);

        let parent = reopened(dir.path().join("parent"), idle()).unwrap();
        let parents = futures::executor::block_on(recover(&*bucket, parent.branch(), None));
        let forks = parents.unwrap().state.forks;
        let forks: Vec<_> = forks.iter().map(|fork| (fork.child, fork.at, fork.point)).collect();
        assert_eq!(forks, [(child, 2, false)]);
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
        let next = extent_remote::Writer::open(
            bucket.clone(),
            branch,
            3,
            crate::host_store::token().unwrap(),
        );
        futures::executor::block_on(next).unwrap();
        let record = futures::executor::block_on(branch::record(&*bucket, child)).unwrap();
        let origin = extent_remote::layout::Origin { parent: branch, at: 2 };
        assert_eq!(record.map(|record| record.origin()), Some(Some(origin)));
        let taken = kept_in(dir.path().join("taken"), (1 << 30, Some(child)), &bucket, idle());
        let taken = taken.unwrap();
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
            let parent = Arc::new(
                kept_in(
                    dir.join("parent"),
                    (1 << 30, None),
                    &Arc::new(Bucket::new(gated.clone(), "")),
                    idle(),
                )
                .unwrap(),
            );
            let branch = parent.branch();
            write(&parent, 1, &pattern(1, BLOCK));
            write(&parent, Self::OWN, &pattern(2, (4 << 20) as usize));
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
                let taking = extent_remote::Writer::open(
                    bucket,
                    branch,
                    3,
                    crate::host_store::token().unwrap(),
                );
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
        let others = kept_in(path, (1 << 30, Some(branch)), &bucket.clone(), idle()).unwrap();
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
        let parent =
            kept_in(dir.path().join("parent"), (1 << 30, None), &bucket.clone(), idle()).unwrap();
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
        let parent = Arc::new(
            kept_in(
                dir.join("parent"),
                (1 << 30, None),
                &Arc::new(Bucket::new(gated.clone(), "")),
                idle(),
            )
            .unwrap(),
        );
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
        let parent = reopened(dir.path().join("parent"), idle()).unwrap();
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
        let parent =
            kept_in(dir.path().join("parent"), (1 << 30, None), &bucket.clone(), idle()).unwrap();
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

    /// A store of a branch taken elsewhere keeps what it reads from the bucket in the host's cache,
    /// and reads it there next.
    #[test]
    fn reads_keep_what_they_fetch() {
        let dir = tempfile::tempdir().unwrap();
        let memory = Arc::new(InMemory::new());
        let bucket = Arc::new(Bucket::new(memory.clone(), ""));
        let cache = Arc::new(Cache::open(&dir.path().join("cache"), 1 << 30, 0).unwrap());
        let options = extent_store::upload::Options { cache: Some(cache.clone()), ..idle() };
        let first =
            kept_in(dir.path().join("first"), (1 << 30, None), &bucket.clone(), idle()).unwrap();
        let branch = first.branch();
        let data = pattern(2, 2 << 20);
        let extent = write(&first, 1, &data);
        first.commit(None).unwrap();
        drop(first);

        let second =
            kept_in(dir.path().join("second"), (1 << 30, Some(branch)), &bucket.clone(), options)
                .unwrap();
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
