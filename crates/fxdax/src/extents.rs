// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An extent store for a guest's own filesystem, Fxfs in its kernel: what a VMM's virtio
//! extent-store device serves (backlog doc-4), through the extent daemon (`crate::extentd`).
//!
//! An [`ExtentStore`] is an image (`crates/extent_store`) whose extents the guest writes, reads and
//! frees by ID. Its I/O copies between the image's files and guest memory, and blocks the calling
//! thread: a device's queue threads call it.
//!
//! Sealed extents are cached in the in-process kernel's memory, keyed by extent ID, which every
//! store and window in the process shares: each is a pager-backed VMO (`FxExtent`, Fuchsia's
//! `FxBlob` as an extent), whose pages the pager reads from the store as they are first read or
//! mapped, and which the kernel evicts under memory pressure. Windows map it read-only: one copy of
//! each page, whichever store or fork maps it, as an extent never changes once sealed. Reads of
//! sealed extents read it too (TASK-32): the guest's filesystem reads its metadata, which it maps
//! not, so a fork reads what its base's other forks read before from memory, not the store. Reads
//! of open extents, and a follower's, go to the store. The store's files bypass the host's page
//! cache.
//!
//! The guest's open extents map writably (doc-4's `DAX_WRITE`): an extent mapped so is memory of
//! the kernel's until the next commit, which reads and writes of it use too. The commit writes it
//! to the store, seals it, maps the window's mappings of it to its VMO in the cache, and moves its
//! memory's pages there.

use crate::dax::{Dax, DaxWindow, Deferred, Keep, SecondStage};
use crate::error::{Errno, Error, IntoErrno, Result};
use crate::executor::Executor;
use crate::host_store;
use crate::kernel::{Arena, ArenaId, page_size};
use crate::memory::{VolatileSlice, VolatileSliceMut};
use crate::profile::{self, Recorder};
use crate::stats::{self, Counter};
use extent_store::{HostDir, NoSpace, Store, TooLong, TooSmall, Unaligned};
use fxfs_platform::fuchsia::fxextent::{ExtentSource, FxExtent};
use fxfs_platform::fuchsia::pager::Pager;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use storage_device::buffer_allocator::{BufferAllocator, BufferSource};
use storage_device::{NoExtent, OtherBranch, SealedExtent};
use vfs::execution_scope::ExecutionScope;
use zircon::vm::pmm::with_arena;

/// An extent's ID: the branch that wrote it, and the sequence number that branch gave it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExtentId {
    /// The branch that wrote it: the store's, or one it was forked from.
    pub branch: u64,
    /// Its sequence number, which its branch gives each extent it writes once.
    pub sequence: u64,
}

impl From<ExtentId> for storage_device::ExtentId {
    fn from(id: ExtentId) -> Self {
        Self::new(id.branch, id.sequence)
    }
}

/// What a store holds, as doc-4's `INFO` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtentInfo {
    /// The most bytes the store's extents may hold.
    pub capacity: u64,
    /// The bytes its extents hold.
    pub used: u64,
    /// The bytes it can still take.
    pub available: u64,
    /// One more than the highest sequence number the store's branch has written.
    pub next_sequence: u64,
    /// fxdax: the lowest sequence number the guest may mint from: its writer's range's start, with
    /// a remote (doc-7, Fencing), past which the store need hold nothing.
    pub sequence_floor: u64,
}

/// fxdax: whose the bytes a store's extents hold are (TASK-49): what billing a store alone charges
/// it, and what it shares with the branches it was forked from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreBytes {
    /// Of the extents its branch wrote.
    pub own: u64,
    /// Of the extents it inherited from the branches it was forked from, which may still share
    /// them.
    pub inherited: u64,
}

/// The process's cache of sealed extents (see the module docs): their pager, on an executor of
/// its own, which reads their pages from the stores, and the extents, by ID.
struct ExtentCache {
    pager: Arc<Pager>,
    /// fxdax: the buffers the pager reads into, by arena (TASK-43): their pages move into the
    /// extents' VMOs, which are of the same arena, so that no arena's memory holds another's data.
    allocators: Mutex<HashMap<ArenaId, Arc<BufferAllocator>>>,
    /// fxdax: by the arena their pages are of (TASK-43), and their IDs.
    extents: Mutex<HashMap<(ArenaId, ExtentId), Cached>>,
    _executor: Executor,
}

/// A sealed extent cached, and the store its pages are read from.
struct Cached {
    extent: Arc<FxExtent>,
    store: Arc<Store<HostDir>>,
    /// fxdax: what waits for the guest that sealed it to lose its access to its pages, if the
    /// sealing commit didn't (`ExtentStore::commit`): another store's guest, a fork's, waits
    /// for it before it maps them.
    sealing: Option<Arc<dyn Deferred>>,
    /// fxdax: when it was last asked for, by [`CACHE_CLOCK`]: those longest ago are evicted first
    /// (TASK-50.3).
    used: AtomicU64,
}

/// fxdax: the cache's clock, which counts the extents asked for.
static CACHE_CLOCK: AtomicU64 = AtomicU64::new(0);

/// zircon's `ZX_VMO_OP_FXDAX_EVICT`: a pager-backed VMO's clean pages evicted now, whatever their
/// age (`zircon::vm::vm_object_paged::VmObjectPaged::evict_range`).
const VMO_OP_EVICT: u32 = 0x8000_0001;

/// fxdax: evicts the clean pages of the cached extents of `arena`, or of every arena, those asked
/// for longest ago first, until `enough` says, of the bytes evicted so far, that they are, or none
/// is left to evict: memory an arena past its cap, or the arenas past the total's, give back
/// (TASK-50.3). Returns the bytes it evicted, as the extents held them: pages faulted in again
/// meanwhile do not hide them. Evicted pages are paged in again as they are touched.
pub(crate) fn evict_least_used(arena: Option<ArenaId>, mut enough: impl FnMut(u64) -> bool) -> u64 {
    let Some(cache) = EXTENT_CACHE.get() else { return 0 };
    let mut candidates: Vec<(u64, Arc<FxExtent>)> = {
        let extents = cache.extents.lock().unwrap();
        (extents.iter())
            .filter(|((of, _), _)| arena.is_none_or(|arena| *of == arena))
            .map(|(_, cached)| (cached.used.load(Ordering::Relaxed), cached.extent.clone()))
            .collect()
    };
    candidates.sort_by_key(|(used, _)| *used);
    let mut evicted = 0;
    for (_, extent) in candidates {
        if enough(evicted) {
            break;
        }
        let vmo = extent.vmo();
        // Its clean pages, all of them, which eviction on demand takes whatever their age.
        let held = vmo.info().map_or(0, |info| info.committed_bytes);
        if held == 0 {
            continue;
        }
        let size = vmo.get_size().unwrap_or(0);
        match vmo.op_range(zx::VmoOp::from_raw(VMO_OP_EVICT), 0, size) {
            Ok(()) => evicted += held,
            Err(status) => log::warn!("fxdax: evicting a cached extent's pages: {status}"),
        }
    }
    evicted
}

/// fxdax: gives, once, the kernel's relief of arenas under pressure (TASK-50.3) its evictor, which
/// frees pages of cached extents (`zircon::vm::pmm::relieve_with`): before a store of any arena
/// opens, whose allocations may wait for it.
fn relieve_pressure() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let relieved = zircon::vm::pmm::relieve_with(|arena, bytes| {
            evict_least_used(arena, |evicted| evicted >= bytes)
        });
        // Without its thread, the kernel's evictor frees what it may.
        if let Err(err) = relieved {
            log::error!("fxdax: starting the thread that frees capped memory: {err}");
        }
    });
}

/// How many threads the cache's pager reads extents' pages on: a read of the store, and the
/// decompression of a part, keep one busy, while page-ins of other extents, and of what a fork's
/// boot profile names, wait for theirs.
const PAGER_THREADS: u8 = 6;
/// The buffers the cache's pager reads into, at most a part (`extent_frame::PART`, 1 MiB) each
/// (`StoredExtent::read_ahead_size`): 32 page-ins at once.
const PAGER_BUFFERS: usize = 32 << 20;

static EXTENT_CACHE: OnceLock<ExtentCache> = OnceLock::new();

fn extent_cache() -> &'static ExtentCache {
    EXTENT_CACHE.get_or_init(|| {
        let executor = Executor::new(PAGER_THREADS);
        // The pager pages in on the executor it is created on.
        let pager = futures::executor::block_on(
            executor.run(async { Pager::new(ExecutionScope::new()).map(Arc::new) }),
        )
        .expect("the extent cache's pager");
        ExtentCache {
            pager,
            allocators: Default::default(),
            extents: Mutex::new(HashMap::new()),
            _executor: executor,
        }
    })
}

/// fxdax: lets go of the extents cached in `arena`, and of the buffers the pager reads into for it:
/// a tenant's, which nothing holds now (`crate::kernel::Arena`, TASK-43.4). Their pages are freed
/// as the last holders of the extents, page-ins in flight, let them go.
pub(crate) fn evict_arena(arena: ArenaId) {
    let Some(cache) = EXTENT_CACHE.get() else { return };
    let evicted: Vec<Cached> = {
        let mut extents = cache.extents.lock().unwrap();
        extents.extract_if(|(of, _), _| *of == arena).map(|(_, cached)| cached).collect()
    };
    drop(evicted);
    cache.allocators.lock().unwrap().remove(&arena);
}

impl ExtentCache {
    /// The buffers the pager reads extents' pages of `arena` into, which are of `arena` too.
    fn allocator(&self, arena: ArenaId) -> Arc<BufferAllocator> {
        let mut allocators = self.allocators.lock().unwrap();
        (allocators.entry(arena))
            .or_insert_with(|| {
                let block_size = ExtentStore::BLOCK_SIZE as usize;
                // Trusted: the kernel's alone, read into in place (`FxExtent::aligned_read`).
                let source = with_arena(arena, || BufferSource::new_trusted(PAGER_BUFFERS));
                Arc::new(BufferAllocator::new(block_size, source))
            })
            .clone()
    }
}

/// A sealed extent's contents, in a store, or, fxdax, in object storage, if the store holds it
/// without its data.
struct StoredExtent {
    store: Arc<Store<HostDir>>,
    extent: storage_device::ExtentId,
    remote: Option<Arc<crate::remote::Fetcher>>,
    /// Where the parts of the object its data is in begin in it (`read_ahead_phase`).
    phase: u64,
    /// fxdax: what records its page-ins for its base's boot profile, if anything does.
    recorder: Option<Arc<Recorder>>,
}

impl StoredExtent {
    fn new(
        store: Arc<Store<HostDir>>,
        extent: storage_device::ExtentId,
        remote: Option<Arc<crate::remote::Fetcher>>,
        recorder: Option<Arc<Recorder>>,
    ) -> Self {
        // Its offset in the data of the object it is in: a segment's, its own file's, or, fxdax,
        // the one object storage holds it in.
        let offset = match store.place(extent) {
            Ok(Some(extent_store::Place::Run { offset, .. })) => offset,
            Ok(Some(extent_store::Place::Remote)) => {
                remote.as_ref().and_then(|remote| remote.offset(extent)).unwrap_or(0)
            }
            _ => 0,
        };
        // In whole pages, which page-ins are.
        let phase = offset % extent_frame::PART / page_size() * page_size();
        Self { store, extent, remote, phase, recorder }
    }
}

impl ExtentSource for StoredExtent {
    /// A read of the local store, which doesn't wait for long, so it reads on the pager's thread;
    /// or a fetch, which the page-in waits for holding no thread.
    fn read<'a>(
        &'a self,
        offset: u64,
        buf: &'a mut [u8],
    ) -> futures::future::BoxFuture<'a, std::result::Result<(), anyhow::Error>> {
        stats::add(Counter::PageIns, 1);
        if let Some(recorder) = &self.recorder {
            let extent = ExtentId { branch: self.extent.branch, sequence: self.extent.sequence };
            recorder.record(extent, offset, buf.len() as u64);
        }
        // fxdax: an extent freed since it was mapped, which a follower's view keeps, reads as
        // kept (doc-8).
        let read = match self.store.read(self.extent, offset, buf) {
            Err(error) if error.downcast_ref::<NoExtent>().is_some() => {
                self.store.read_kept(self.extent, offset, buf)
            }
            read => read,
        };
        match read {
            Err(error) if error.downcast_ref::<extent_store::RemoteData>().is_some() => {
                let Some(remote) = &self.remote else {
                    return Box::pin(std::future::ready(Err(error)));
                };
                let fetched = remote.read(self.extent, offset, buf.len() as u64);
                Box::pin(async move {
                    let data = fetched.await?;
                    stats::add(Counter::FetchedBytes, data.len() as u64);
                    buf[..data.len()].copy_from_slice(&data);
                    buf[data.len()..].fill(0);
                    Ok(())
                })
            }
            read => {
                if read.is_ok() {
                    stats::add(Counter::PageInBytes, buf.len() as u64);
                }
                Box::pin(std::future::ready(read))
            }
        }
    }

    /// A part of an object (`extent_frame::PART`), which a sealed extent's data is stored and
    /// fetched in, and decompressed whole: reading less of it costs nearly as much.
    fn read_ahead_size(&self) -> u64 {
        extent_frame::PART
    }

    fn read_ahead_phase(&self) -> u64 {
        self.phase
    }
}

/// A window's mapping: what it keeps alive, an open extent's memory, a sealed extent in the cache,
/// or zeros; and what it maps, which a snapshot of its VM maps again where the VM is restored
/// ([`ExtentStore::window_mappings`]).
#[derive(Clone)]
struct Mapped {
    pages: Arc<dyn Send + Sync>,
    maps: Maps,
}

/// What a window's mapping maps.
#[derive(Clone, Copy)]
enum Maps {
    /// The extent's bytes: those at `offset` at the window's `window_offset`, where the mapping
    /// began, and those after them in order.
    Extent {
        extent: ExtentId,
        window_offset: u64,
        offset: u64,
        writable: bool,
    },
    Zeros,
}

impl Mapped {
    fn extent(
        pages: Arc<dyn Send + Sync>,
        extent: ExtentId,
        window_offset: u64,
        offset: u64,
        writable: bool,
    ) -> Self {
        Self { pages, maps: Maps::Extent { extent, window_offset, offset, writable } }
    }

    fn zeros(zeros: &Arc<zx::Vmo>) -> Self {
        Self { pages: zeros.clone(), maps: Maps::Zeros }
    }

    /// Whether it keeps `vmo` alive.
    fn keeps(&self, vmo: &Arc<zx::Vmo>) -> bool {
        self.identity() == vmo.identity()
    }

    /// Where in its extent the window's byte at `window_offset`, which it maps, is: none for
    /// zeros.
    fn extent_offset(&self, window_offset: u64) -> Option<u64> {
        match self.maps {
            Maps::Extent { window_offset: start, offset, .. } => {
                Some(offset + (window_offset - start))
            }
            Maps::Zeros => None,
        }
    }
}

impl Keep for Mapped {
    fn identity(&self) -> usize {
        self.pages.identity()
    }
}

/// A mapping of a store's DAX window, as a snapshot of its VM records it, to map it again where
/// the VM is restored ([`ExtentStore::window_mappings`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowMapping {
    /// Where in the window it is.
    pub window_offset: u64,
    /// How many bytes it maps.
    pub len: u64,
    /// What it maps.
    pub contents: MappingContents,
}

/// What a [`WindowMapping`] maps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappingContents {
    /// An extent's bytes.
    Extent {
        /// The extent.
        extent: ExtentId,
        /// Where in it the bytes mapped start.
        offset: u64,
        /// Whether the mapping is writable: of an open extent.
        writable: bool,
    },
    /// Zeros, read-only.
    Zeros,
}

/// An open extent that a window maps writably: its contents, in the kernel's memory, and how long
/// it is.
struct OpenExtent {
    vmo: Arc<zx::Vmo>,
    len: u64,
    /// fxdax: how long the store held it as written when it was opened here, which its commit
    /// keeps at least.
    stored: u64,
}

/// An extent store a guest's filesystem lives on. See the module docs.
pub struct ExtentStore {
    store: Arc<Store<HostDir>>,
    /// The open extents mapped writably, until the next commit. Locked before `dax`.
    open: Mutex<HashMap<ExtentId, OpenExtent>>,
    dax: Mutex<Option<Dax<Mapped>>>,
    /// fxdax: zeros, the window's size of them, which the window maps where it mapped an extent
    /// freed since: made once one is.
    zeros: std::sync::OnceLock<Arc<zx::Vmo>>,
    /// fxdax: the arenas of the kernel's physical memory its extents' pages are of (TASK-43).
    arenas: Arenas,
    /// fxdax: the uploader of the store's branch to object storage, if it has one: durable
    /// commits are uploaded ones then (doc-4's `COMMIT_DURABLE`).
    remote: std::sync::OnceLock<crate::remote::Remote>,
    /// fxdax: what records the store's page-ins for its base's boot profile, if it is a fork of a
    /// base that has none yet, and what writes it at the window's end, which closing stops early.
    profiling: Option<(Arc<Recorder>, std::sync::mpsc::Sender<()>)>,
    /// fxdax: held, shared, by what opens extents, writes and writable mappings, and alone by a
    /// fork only once nothing is open, which none comes between ([`ExtentStore::fork_quiescent`]).
    opening: std::sync::RwLock<()>,
    /// fxdax: a follower's (backlog doc-8), what it follows ([`ExtentStore::follow`]).
    following: Option<Following>,
    /// fxdax: notified as it commits, for its followers' daemon to tell them, holding no thread
    /// meanwhile (TASK-34).
    committed: event_listener::Event,
}

/// A follower's part of its store: the writer's store it follows, and the view it reads it
/// through.
struct Following {
    writer: Arc<ExtentStore>,
    view: Mutex<extent_store::View>,
}

/// The barriers a write is ordered with (doc-4's `WRITE` flags).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriteBarriers {
    /// A barrier precedes the write (`WRITE_BARRIER`).
    pub before: bool,
    /// A commit follows it (`WRITE_COMMIT`), which seals the extent it writes too.
    pub after: Option<Commit>,
}

/// When a commit completes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Commit {
    /// Once ordered: the store makes it durable soon after.
    Ordered,
    /// Once durable (`COMMIT_DURABLE`).
    Durable,
}

/// fxdax: how much of an open extent's `data` its commit keeps: as far as its last block holding
/// anything but zeros, and one block at least, so that the extent is there to read.
fn kept_len(data: &[u8]) -> usize {
    let block = ExtentStore::BLOCK_SIZE as usize;
    let blocks = data.chunks(block).rposition(|block| !is_zeros(block));
    blocks.map_or(block, |last| (last + 1) * block).min(data.len())
}

/// Whether `bytes` are all zeros: as words, which the compiler vectorizes, rather than byte by
/// byte; a commit scans the pages mapped ahead of a guest's writes.
fn is_zeros(bytes: &[u8]) -> bool {
    let words = bytes.chunks_exact(8);
    let rest = words.remainder().iter().fold(0, |any, byte| any | *byte);
    let any = words.fold(0u64, |any, word| any | u64::from_ne_bytes(word.try_into().unwrap()));
    any == 0 && rest == 0
}

/// fxdax: the errno of the uploader's failure, as doc-4 has them: a branch another writer took, or
/// a later placement holds, is one the store takes no more commits for.
fn remote_errno(failure: crate::remote::Failure) -> Errno {
    match failure {
        crate::remote::Failure::Taken | crate::remote::Failure::Stale => Errno::EROFS,
        crate::remote::Failure::Other(err) => {
            log::error!("fxdax: uploading the store's commits: {err}");
            Errno::EIO
        }
    }
}

/// fxdax: a follower's request for `extent`, which its view's commits don't hold and it doesn't
/// pin: a bug of its guest's follower, never a stale read (doc-8, testing 1): `EIO`, logged. A
/// view cut off is `ESTALE`.
fn outside_view(extent: ExtentId, error: anyhow::Error) -> Errno {
    if error.downcast_ref::<extent_store::CutOff>().is_some() {
        return Errno::ESTALE;
    }
    // Written after the view's commit: as Fxfs's journal replay finds the end of the commit.
    if error.downcast_ref::<NoExtent>().is_some()
        && error.downcast_ref::<extent_store::NotHeld>().is_none()
    {
        return Errno::ENOENT;
    }
    log::error!("fxdax: a follower's request for {extent:?}, outside its view: {error:#}");
    Errno::EIO
}

/// The errno of the store's error, as doc-4 has them.
fn errno(error: anyhow::Error) -> Errno {
    if error.downcast_ref::<extent_store::CutOff>().is_some() {
        // A follower too far behind its writer (doc-8, decision 7): it remounts.
        Errno::ESTALE
    } else if error.downcast_ref::<NoExtent>().is_some() {
        Errno::ENOENT
    } else if error.downcast_ref::<SealedExtent>().is_some()
        || error.downcast_ref::<OtherBranch>().is_some()
    {
        Errno::EPERM
    } else if error.downcast_ref::<NoSpace>().is_some()
        || error.downcast_ref::<TooSmall>().is_some()
    {
        Errno::ENOSPC
    } else if error.downcast_ref::<Unaligned>().is_some()
        || error.downcast_ref::<TooLong>().is_some()
    {
        Errno::EINVAL
    } else {
        log::error!("fxdax: extent store: {error:#}");
        Errno::EIO
    }
}

// fxdax: closing the store frees its branch to be opened again, though caches of its extents may
// hold its store still, to read them.
impl Drop for ExtentStore {
    fn drop(&mut self) {
        // fxdax: what the fork recorded, its base's profile, if the window is not over yet.
        if let Some((recorder, _)) = self.profiling.take()
            && let Err(err) = recorder.write()
        {
            log::warn!("fxdax: writing a boot profile: {err}");
        }
        // A follower's store is its writer's.
        if self.following.is_none()
            && let Err(err) = self.store.close()
        {
            log::error!("fxdax: closing an extent store: {err:#}");
        }
    }
}

impl ExtentStore {
    /// The size of the blocks extents are read and written in.
    pub const BLOCK_SIZE: u32 = host_store::BLOCK_SIZE;

    /// The most bytes an extent holds (doc-4's `max_extent`).
    pub const MAX_EXTENT: u64 = 64 << 20;

    /// A new store at `path`, which must not exist, holding at most `capacity` bytes, with a branch
    /// of its own.
    pub fn create(path: impl AsRef<Path>, capacity: u64) -> Result<Self, Error> {
        Self::create_for_branch(path, capacity, host_store::new_branch())
    }

    /// fxdax: a new store at `path`, as [`ExtentStore::create`] makes, of `branch`: one kept in
    /// object storage, opened on a host without it, takes what its branch holds once attached
    /// ([`ExtentStore::attach_remote`]).
    pub fn create_for_branch(
        path: impl AsRef<Path>,
        capacity: u64,
        branch: u64,
    ) -> Result<Self, Error> {
        Self::create_in(path, capacity, branch, None)
    }

    /// fxdax: a new store at `path`, as [`ExtentStore::create_for_branch`] makes, a tenant's,
    /// whose own extents' pages are of `arena` ([`ExtentStore::open_in`]).
    pub fn create_in(
        path: impl AsRef<Path>,
        capacity: u64,
        branch: u64,
        arena: Option<Arc<Arena>>,
    ) -> Result<Self, Error> {
        let store = host_store::create(path.as_ref(), capacity, branch).map_err(Error::new)?;
        Ok(Self::new(store, arena))
    }

    /// The store at `path`, or the fork whose handle `path` is ([`ExtentStore::fork`]), as of its
    /// last commit.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        Self::open_in(path, None)
    }

    /// fxdax: the store at `path`, as [`ExtentStore::open`] has it, a tenant's (TASK-43): the
    /// pages of its own extents, and of those it inherited from other branches but public ones
    /// ([`ExtentStore::make_public`]), are of `arena` of the kernel's physical memory, which it
    /// holds, and what reads into the extent cache for it, until they are gone (TASK-79.36); or of
    /// the default arena, without one. Those of public branches' extents, which every tenant's
    /// forks of them share, are of the default arena.
    pub fn open_in(path: impl AsRef<Path>, arena: Option<Arc<Arena>>) -> Result<Self, Error> {
        let store = host_store::open(path.as_ref()).map_err(Error::new)?;
        Ok(Self::new(store, arena))
    }

    fn new(store: Store<HostDir>, arena: Option<Arc<Arena>>) -> Self {
        relieve_pressure();
        let store = Arc::new(store);
        let arenas = Arenas { own: arena, public: Arc::new(public_branches(store.dir().path())) };
        Self {
            profiling: Self::profile(&store, &arenas),
            arenas,
            store,
            open: Mutex::new(HashMap::new()),
            dax: Mutex::new(None),
            zeros: Default::default(),
            remote: Default::default(),
            opening: Default::default(),
            following: None,
            committed: event_listener::Event::new(),
        }
    }

    /// fxdax: a follower of the store (backlog doc-8): a store of its own, read-only, with a window
    /// of its own, which reads the store, and maps its extents' cached pages, as of its latest
    /// commit, through a view of it, while the store's writer goes on. Anything outside its
    /// view's commits and its pins it refuses with `EIO`, logged; what would change the store,
    /// with `EROFS`. Its view moves to the writer's latest commit as it advances.
    pub fn follow(self: &Arc<Self>) -> ExtentStore {
        let following = Following { writer: self.clone(), view: Mutex::new(self.store.view()) };
        Self {
            profiling: None,
            arenas: self.arenas.clone(),
            store: self.store.clone(),
            open: Mutex::new(HashMap::new()),
            dax: Mutex::new(None),
            zeros: Default::default(),
            remote: Default::default(),
            opening: Default::default(),
            following: Some(following),
            committed: event_listener::Event::new(),
        }
    }

    /// fxdax: whether it is a follower's ([`ExtentStore::follow`]).
    pub fn is_follower(&self) -> bool {
        self.following.is_some()
    }

    /// fxdax: makes the store a new writer's, which its followers go on following: one opened for
    /// followers alone, or a writer's that went. `EBUSY` unless nothing is open in it
    /// ([`ExtentStore::is_quiescent`]): what a writer that went had written since its last commit
    /// its new writer's filesystem doesn't know of. The window goes, which was the writer's that
    /// went: the new one creates its own.
    pub fn take_over(&self) -> Result<()> {
        self.writable().map_err(|_| Errno::EINVAL)?;
        if !self.is_quiescent()? {
            return Err(Errno::EBUSY);
        }
        let window = self.dax.lock().unwrap().take();
        drop(window);
        Ok(())
    }

    /// fxdax: cuts the store's followers off: their reads, advances and pins fail with `ESTALE`
    /// from now on, until they follow it anew. What its writer's going with writes uncommitted
    /// does: the store is no new writer's ([`ExtentStore::take_over`]) while they follow it.
    pub fn cut_followers(&self) {
        self.store.cut_views();
    }

    /// fxdax: a follower's view, or `EINVAL` if it is a writer's store.
    fn view(&self) -> Result<std::sync::MutexGuard<'_, extent_store::View>> {
        let following = self.following.as_ref().ok_or(Errno::EINVAL)?;
        Ok(following.view.lock().unwrap())
    }

    /// fxdax: `EROFS` if it is a follower's, which changes nothing.
    fn writable(&self) -> Result<()> {
        if self.following.is_some() { Err(Errno::EROFS) } else { Ok(()) }
    }

    /// The epoch of the store's latest commit: its writer's, if a follower's.
    pub fn committed(&self) -> Result<u64> {
        self.store.epoch().map_err(errno)
    }

    /// fxdax: the epoch of the commit a follower's view is as of.
    pub fn epoch(&self) -> Result<u64> {
        Ok(self.view()?.epoch())
    }

    /// fxdax: moves a follower's view to the writer's latest commit, holding what the commits
    /// since the one it was as of held, until it lets go of them ([`ExtentStore::let_go`]): what
    /// its guest's follower reads as it applies them. Returns the commit's epoch.
    pub fn advance(&self) -> Result<u64> {
        let mut view = self.view()?;
        view.advance_keeping().map_err(errno)?;
        Ok(view.epoch())
    }

    /// fxdax: lets go of what a follower's view held of the commits before the commit of
    /// `epoch`, at most its own: their frees go, but for its pins. One of an earlier epoch than
    /// it holds since, coming after its next advance, changes nothing.
    pub fn let_go(&self, epoch: u64) -> Result<()> {
        self.view()?.let_go(epoch);
        Ok(())
    }

    /// fxdax: keeps `extent`, which a follower's view holds, whatever commit it advances to,
    /// until unpinned: what its guest holds open that the writer deletes.
    pub fn pin(&self, extent: ExtentId) -> Result<()> {
        self.view()?.pin(extent.into()).map_err(errno)
    }

    /// fxdax: lets go of `extent`, pinned.
    pub fn unpin(&self, extent: ExtentId) -> Result<()> {
        self.view()?.unpin(extent.into());
        Ok(())
    }

    /// fxdax: waits at most `timeout` for a commit of the store after the commit of `epoch`,
    /// returning the latest commit's epoch, if one came: what tells its followers.
    pub fn newer(&self, epoch: u64, timeout: std::time::Duration) -> Option<u64> {
        self.store.newer(epoch, timeout)
    }

    /// fxdax: what says when the store's writer next commits ([`ExtentStore::newer`] then has it),
    /// through the writer's store, if a follower's: a future, which holds no thread.
    pub fn commits(&self) -> event_listener::EventListener {
        match &self.following {
            Some(following) => following.writer.committed.listen(),
            None => self.committed.listen(),
        }
    }

    /// fxdax: what the store's followers are (TASK-42.7): their views, how far behind, and the
    /// bytes of frees they keep, which count as used.
    pub fn follower_stats(&self) -> Result<(extent_store::ViewStats, u64)> {
        let kept = self.store.usage().map_err(errno)?.kept_for_views;
        Ok((self.store.view_stats(), kept))
    }

    /// fxdax: the store's uploader, which a follower's writer's is.
    fn uploader(&self) -> Option<&crate::remote::Remote> {
        match &self.following {
            Some(following) => following.writer.remote.get(),
            None => self.remote.get(),
        }
    }

    /// fxdax: for a fork of a base, reads the base's boot profile into the extent cache, behind
    /// the fork, if there is one; or records the fork's page-ins as the base's profile, and writes
    /// it once the window is over.
    fn profile(
        store: &Arc<Store<HostDir>>,
        arenas: &Arenas,
    ) -> Option<(Arc<Recorder>, std::sync::mpsc::Sender<()>)> {
        let base = store.parent()?;
        let path = profile::path(store.dir().path(), base);
        match profile::read(&path) {
            Ok(units) => {
                replay(store.clone(), arenas.clone(), units);
                return None;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                log::warn!("fxdax: reading the boot profile at {}: {err}", path.display());
                return None;
            }
        }
        // One fork of the base records it, of those that start before it is written: not a
        // recorder and a thread each of a few hundred starting at once (TASK-34).
        static RECORDING: Mutex<std::collections::BTreeSet<std::path::PathBuf>> =
            Mutex::new(std::collections::BTreeSet::new());
        if !RECORDING.lock().unwrap().insert(path.clone()) {
            return None;
        }
        let recorder = Arc::new(Recorder::new(path.clone(), store.description().branch));
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let writer = recorder.clone();
        let recorded = path.clone();
        let spawned = std::thread::Builder::new().name("fxdax profile".into()).spawn(move || {
            // At the window's end, or the store's closing, which writes it itself.
            if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                stopped.recv_timeout(profile::WINDOW)
                && let Err(err) = writer.write()
            {
                log::warn!("fxdax: writing a boot profile: {err}");
            }
            RECORDING.lock().unwrap().remove(&recorded);
        });
        if let Err(err) = spawned {
            log::warn!("fxdax: starting a boot profile's writer: {err}");
            RECORDING.lock().unwrap().remove(&path);
            return None;
        }
        Some((recorder, stop))
    }

    /// fxdax: keeps the store's branch in `bucket` from now on, as `holder` of its lease, which must
    /// differ from every other taking's: uploads what the store holds that the branch doesn't, then
    /// its commits as `options` say; durable commits wait to be uploaded.
    pub fn attach_remote(
        &self,
        bucket: Arc<extent_remote::store::Store>,
        holder: u64,
        options: crate::remote::Options,
    ) -> std::result::Result<(), crate::remote::Failure> {
        let remote = crate::remote::Remote::attach(self.store.clone(), bucket, holder, options)?;
        let had = || crate::remote::Failure::Other("the store already has a remote".to_owned());
        self.remote.set(remote).map_err(|_| had())
    }

    /// fxdax: forks the store, as of its last commit, made durable first: `branch`, a new branch of
    /// the store holding what it holds, whose handle is a new directory at `path`, which
    /// [`ExtentStore::open`] opens. It takes the same time and space whatever the store holds,
    /// sharing the store's files rather than copying them (`extent_store::fork_host`). With a
    /// remote, the fork's branch is a fork of the store's in object storage too, made once the
    /// fork's commit is uploaded, which inherits its extents rather than uploads them again (doc-7,
    /// Forks).
    pub fn fork(&self, path: impl AsRef<Path>, branch: u64) -> Result<(), Error> {
        self.writable()?;
        let path = path.as_ref();
        self.make_durable()?;
        let epoch = extent_store::fork_host(&self.store, path, branch).map_err(Error::new)?;
        if let Some(remote) = self.remote.get()
            && let Err(err) = remote.forked(epoch)
        {
            // Leave no fork its branch doesn't have behind: here, then in object storage, where
            // its segment may have landed after all (`Remote::unfork`). A child opened meanwhile
            // stays, with its branch.
            let unforked = (host_store::delete(path, None).map_err(|err| err.to_string()))
                .and_then(|()| remote.unfork(branch).map_err(|err| err.to_string()));
            if let Err(why) = unforked {
                log::warn!("fxdax: leaving branch {branch:x}, whose fork failed: {why}");
            }
            return Err(Error::new(anyhow::anyhow!("forking branch {branch:x}: {err}")));
        }
        Ok(())
    }

    /// fxdax: places the store at `path`, or the fork whose handle `path` is, at the placement
    /// whose fencing token, its boot ID, is `token` (TASK-39.7): its writer takes its branch's
    /// lease in object storage with that token from then on, which a later placement's exceeds, so
    /// a placement cut off takes no branch back from the one after it (`ESTALE`), and its commits
    /// fail once a later one takes it (`EROFS`).
    pub fn place(path: impl AsRef<Path>, token: u64) -> Result<(), Error> {
        let (dir, branch) = extent_store::resolve_host(path.as_ref()).map_err(Error::new)?;
        crate::remote::place(&dir, branch, token).map_err(Error::new)
    }

    /// fxdax: deletes the store at `path`, or the fork whose handle `path` is, which nothing has
    /// open (`extent_store::delete_host`): its extents go but for those other branches hold, and a
    /// store whose forks live keeps its directory, holding what they share of it, until the last
    /// of them goes (TASK-39.4). With `bucket`, its branch there is deleted first, its forks'
    /// branches living on: a deletion that stopped partway is finished by deleting again. Fails
    /// with `ENOENT` if there is none at `path`, and with `ESTALE` if a later placement holds its
    /// branch in `bucket`.
    pub fn delete(
        path: impl AsRef<Path>,
        bucket: Option<&extent_remote::store::Store>,
    ) -> Result<(), Error> {
        host_store::delete(path.as_ref(), bucket)
    }

    /// fxdax: the store's uploader, if it has one.
    #[cfg(test)]
    pub(crate) fn remote(&self) -> Option<&crate::remote::Remote> {
        self.remote.get()
    }

    /// The branch whose extents the guest mints.
    pub fn branch(&self) -> u64 {
        self.store.description().branch
    }

    /// What the store holds (`INFO`).
    pub fn info(&self) -> Result<ExtentInfo> {
        let info = self.store.info().map_err(errno)?;
        // fxdax: from its writer's range, with a remote (doc-7, Fencing).
        let floor = self.remote.get().map_or(0, crate::remote::Remote::next_sequence);
        Ok(ExtentInfo {
            capacity: info.capacity,
            used: info.used,
            available: info.available,
            next_sequence: info.next_sequence,
            sequence_floor: floor,
        })
    }

    /// fxdax: whose the bytes its extents hold are: its branch's own, or inherited (TASK-49).
    pub fn bytes(&self) -> Result<StoreBytes> {
        let info = self.store.info().map_err(errno)?;
        Ok(StoreBytes { own: info.own, inherited: info.inherited })
    }

    /// Sets the most bytes the store's extents may hold, durably: grows it, or shrinks it no lower
    /// than what its extents hold (`ENOSPC`). The guest's filesystem, which may hold more, agrees
    /// to a shrink first (the extent daemon's `RESIZE` asks it).
    pub fn set_capacity(&self, capacity: u64) -> Result<()> {
        self.writable()?;
        self.store.set_capacity(capacity).map_err(errno)
    }

    /// Reads the bytes at `offset` in `extent` into `buffers`, in order (`READ`). Blocks never
    /// written read as zeros.
    pub fn read(
        &self,
        extent: ExtentId,
        offset: u64,
        buffers: &mut [VolatileSliceMut],
    ) -> Result<()> {
        let len = buffers.iter().map(VolatileSliceMut::len).sum();
        stats::add(Counter::Reads, 1);
        stats::add(Counter::ReadBytes, len as u64);
        let mut data = vec![0; len];
        let open = self.open.lock().unwrap().get(&extent).map(|open| open.vmo.clone());
        match open {
            // An open extent mapped writably is in memory.
            Some(vmo) => {
                let end = offset.checked_add(len as u64).ok_or(Errno::EINVAL)?;
                if end > Self::MAX_EXTENT {
                    return Err(Errno::EINVAL);
                }
                vmo.read(&mut data, offset).errno()?;
            }
            None => {
                if !self.read_cached(extent, offset, &mut data)? {
                    self.read_stored(extent, offset, &mut data)?;
                }
            }
        }
        let mut at = 0;
        for buffer in buffers {
            at += buffer.copy_from(&data[at..]);
        }
        Ok(())
    }

    /// fxdax: reads `extent`'s bytes at `offset` from the extent cache, if it is sealed: which pages
    /// them in from the store, or object storage, unless they are in memory already, read by any
    /// store or fork before (TASK-32). Returns whether it read them: not a follower's, whose view
    /// decides what it holds, nor an open extent's, nor one the store doesn't hold.
    fn read_cached(&self, extent: ExtentId, offset: u64, data: &mut [u8]) -> Result<bool> {
        if self.following.is_some() {
            return Ok(false);
        }
        let Some((len, false)) = self.store.extent(extent.into()).map_err(errno)? else {
            return Ok(false);
        };
        let end = offset.checked_add(data.len() as u64).ok_or(Errno::EINVAL)?;
        if end > Self::MAX_EXTENT {
            return Err(Errno::EINVAL);
        }
        // Past its end, zeros.
        let held = len.saturating_sub(offset).min(data.len() as u64) as usize;
        let (bytes, zeros) = data.split_at_mut(held);
        zeros.fill(0);
        if bytes.is_empty() {
            return Ok(true);
        }
        let cached = self.cached(extent, len);
        match cached.vmo().read(bytes, offset) {
            Ok(()) => Ok(true),
            // Freed meanwhile, or its page-in failed: as the store says.
            Err(status) => {
                log::debug!("fxdax: reading {extent:?} at {offset} from the cache: {status}");
                Ok(false)
            }
        }
    }

    /// Reads a sealed extent's bytes from the store, or, fxdax, from object storage, if the store
    /// holds it without its data.
    fn read_stored(&self, extent: ExtentId, offset: u64, data: &mut [u8]) -> Result<()> {
        stats::add(Counter::ReadStoredBytes, data.len() as u64);
        let read = match &self.following {
            Some(following) => {
                let view = following.view.lock().unwrap();
                self.store.read_view(&view, extent.into(), offset, data)
            }
            None => self.store.read(extent.into(), offset, data),
        };
        match read {
            Err(error) if error.downcast_ref::<extent_store::RemoteData>().is_some() => {
                let remote = self.uploader().ok_or(Errno::EIO)?;
                let fetched = futures::executor::block_on(remote.fetcher().read(
                    extent.into(),
                    offset,
                    data.len() as u64,
                ))
                .map_err(remote_errno)?;
                data[..fetched.len()].copy_from_slice(&fetched);
                data[fetched.len()..].fill(0);
                Ok(())
            }
            Err(error) if self.following.is_some() => Err(outside_view(extent, error)),
            read => read.map_err(errno),
        }
    }

    /// Writes as [`write`](Self::write) does, ordered with `barriers` (`WRITE`'s flags): one
    /// request, where each is a round trip from the guest.
    pub fn write_with_barriers(
        &self,
        extent: ExtentId,
        offset: u64,
        buffers: &[VolatileSlice],
        barriers: WriteBarriers,
    ) -> Result<()> {
        if barriers.before {
            self.commit_ordered(None)?;
        }
        self.write(extent, offset, buffers)?;
        match barriers.after {
            None => Ok(()),
            Some(Commit::Ordered) => self.commit_ordered(None),
            Some(Commit::Durable) => self.commit(None),
        }
    }

    /// Writes the bytes of `buffers`, in order, at `offset` in `extent`, which must be the store's
    /// branch's, and open (`WRITE`).
    pub fn write(&self, extent: ExtentId, offset: u64, buffers: &[VolatileSlice]) -> Result<()> {
        self.writable()?;
        let _opening = self.opening.read().unwrap();
        let mut data = vec![0; buffers.iter().map(VolatileSlice::len).sum()];
        let mut at = 0;
        for buffer in buffers {
            at += buffer.copy_to(&mut data[at..]);
        }
        if let Some(open) = self.open.lock().unwrap().get_mut(&extent) {
            // An open extent mapped writably is in memory, which the commit writes to the store.
            let end = offset.checked_add(data.len() as u64).ok_or(Errno::EINVAL)?;
            if end > Self::MAX_EXTENT {
                return Err(Errno::EINVAL);
            }
            open.vmo.write(&data, offset).errno()?;
            open.len = open.len.max(end);
            return Ok(());
        }
        self.store.write(extent.into(), offset, &data).map_err(errno)
    }

    /// Frees `extent`, if the store holds it (`FREE`).
    pub fn free(&self, extent: ExtentId) -> Result<()> {
        self.writable()?;
        // An open extent's memory is dropped as the window's mappings of it go.
        let open = self.open.lock().unwrap().remove(&extent);
        if let Some(open) = &open {
            self.replace_kept(open.vmo.identity())?;
            if self.store.extent(extent.into()).map_err(errno)?.is_none() {
                return Ok(());
            }
        }
        self.store.free(extent.into()).map_err(errno)?;
        // Its pages, if read from this store: another that holds it reads them anew.
        let cached = {
            let mut extents = extent_cache().extents.lock().unwrap();
            let key = (self.arenas.of(extent), extent);
            if extents.get(&key).is_some_and(|cached| Arc::ptr_eq(&cached.store, &self.store)) {
                extents.remove(&key)
            } else {
                None
            }
        };
        // fxdax: the window's mappings of it, which kept its pages, map zeros instead: a guest
        // rewriting the same blocks, sealing each version (a database's log, fsynced at each
        // transaction), filled the cache with pages only the window's mappings kept, which no
        // eviction could take (ENOMEM).
        if let Some(cached) = cached {
            self.replace_kept(cached.extent.identity())?;
        }
        Ok(())
    }

    /// Maps zeros where the window maps what keeps a freed extent's pages, which has the identity
    /// `kept` (`Dax::replace_kept`).
    fn replace_kept(&self, kept: usize) -> Result<()> {
        let mut dax = self.dax.lock().unwrap();
        let Some(dax) = dax.as_mut() else { return Ok(()) };
        let zeros = self.zeros(dax)?;
        dax.replace_kept(kept, Mapped::zeros(&zeros), &zeros)
    }

    /// Commits what was written and freed, and `root` as the store's root if given, durably
    /// (`COMMIT` with `COMMIT_DURABLE`). It seals the extents written before it, and those stored
    /// to through the window, whose writable mappings it maps read-only.
    pub fn commit(&self, root: Option<&[u8]>) -> Result<()> {
        self.writable()?;
        self.commit_with(root, true)
    }

    /// fxdax: commits as [`ExtentStore::commit`] does, but without waiting for durability: a
    /// barrier, ordering what came before before what comes after (`COMMIT` without
    /// `COMMIT_DURABLE`). [`ExtentStore::flush`] makes the commits ordered so far durable.
    pub fn commit_ordered(&self, root: Option<&[u8]>) -> Result<()> {
        self.writable()?;
        self.commit_with(root, false)
    }

    /// Makes the commits ordered so far durable.
    pub fn flush(&self) -> Result<()> {
        self.store.flush().map_err(errno)
    }

    /// Makes the commits ordered so far durable: uploaded, if the store has a remote.
    fn make_durable(&self) -> Result<()> {
        let epoch = self.durable_epoch()?;
        match self.remote.get() {
            Some(remote) => remote.wait_uploaded(epoch).map_err(remote_errno),
            None => Ok(()),
        }
    }

    /// fxdax: makes every commit ordered so far durable, and returns the epoch they are of: what
    /// [`ExtentStore::uploaded`] waits for. It blocks on the store's I/O.
    pub fn durable_epoch(&self) -> Result<u64> {
        let epoch = self.store.epoch().map_err(errno)?;
        self.flush()?;
        Ok(epoch)
    }

    /// fxdax: returns once the commits up to `epoch` ([`ExtentStore::durable_epoch`]) are in
    /// object storage, as [`ExtentStore::wait_uploaded`] does: a future, which holds no thread
    /// while it waits (TASK-43.6). `ENOTSUP` without a remote.
    pub fn uploaded(
        &self,
        epoch: u64,
    ) -> impl std::future::Future<Output = Result<()>> + Send + use<> {
        let waiting = self.remote.get().map(|remote| remote.uploaded(epoch));
        async move {
            match waiting {
                Some(waiting) => waiting.await.map_err(remote_errno),
                None => Err(Errno::ENOTSUP),
            }
        }
    }

    /// fxdax: what the store's uploader has done, and has left to do, if it has one
    /// ([`ExtentStore::attach_remote`], TASK-50.5).
    pub fn upload_status(&self) -> Result<Option<crate::remote::UploadStatus>> {
        self.remote.get().map(|remote| remote.status().map_err(remote_errno)).transpose()
    }

    /// fxdax: waits until every commit ordered so far is durable and in object storage, as before
    /// a clean stop (TASK-39.7). `ENOTSUP` without a remote ([`ExtentStore::attach_remote`]).
    pub fn wait_uploaded(&self) -> Result<()> {
        if self.remote.get().is_none() {
            return Err(Errno::ENOTSUP);
        }
        self.make_durable()
    }

    /// Whether commits are waiting for [`ExtentStore::flush`].
    pub fn flush_pending(&self) -> bool {
        self.store.flush_pending()
    }

    /// fxdax: whether cleaning the store behind its commits is due ([`host_store::cleaning_due`],
    /// TASK-79.2): a follower's never is, which its writer cleans.
    pub fn cleaning_due(&self) -> Result<bool> {
        if self.following.is_some() {
            return Ok(false);
        }
        host_store::cleaning_due(&self.store).map_err(errno)
    }

    /// fxdax: cleans the store behind its commits ([`host_store::clean_behind`], TASK-79.2). It
    /// blocks on the store's I/O.
    pub fn clean_behind(&self) -> Result<()> {
        self.writable()?;
        host_store::clean_behind(&self.store).map_err(errno)
    }

    fn commit_with(&self, root: Option<&[u8]>, durable: bool) -> Result<()> {
        let started = std::time::Instant::now();
        let committed = self.commit_and_seal(root, durable);
        stats::count_since(Counter::Commits, Counter::CommitNs, started);
        committed
    }

    /// Commits, seals the open extents, and waits for object storage as `durable` says: the
    /// commit's phases, each counted ([`stats::Stats::commits`]).
    fn commit_and_seal(&self, root: Option<&[u8]>, durable: bool) -> Result<()> {
        let started = std::time::Instant::now();
        let mut open = self.open.lock().unwrap();
        for (extent, extent_open) in open.iter() {
            let mut data = vec![0; extent_open.len as usize];
            extent_open.vmo.read(&mut data, 0).errno()?;
            // fxdax: what was mapped writably, past what was stored to, the store leaves out: the
            // pages a guest maps ahead of its writes, which read as zeros past the extent's end as
            // they would within it (doc-4). Written whole, they took the store's room. The cached
            // extent, which pages them in from the store, is as long as the window maps it still,
            // so that one mapping of it replaces each, as before.
            let kept = (kept_len(&data) as u64).max(extent_open.stored);
            self.store.write((*extent).into(), 0, &data[..kept as usize]).map_err(errno)?;
            stats::add(Counter::CommitWrittenBytes, kept);
        }
        stats::add_since(Counter::CommitWriteNs, started);
        let committing = std::time::Instant::now();
        self.store.commit_ordered(root).map_err(errno)?;
        self.committed.notify(usize::MAX);
        stats::add_since(Counter::CommitStoreNs, committing);
        let sealing_started = std::time::Instant::now();
        let mut dax = self.dax.lock().unwrap();
        let sealed: Vec<(ExtentId, OpenExtent)> = open.drain().collect();
        if let Some(dax) = dax.as_mut() {
            // fxdax: the guest's access to the pages, which it is to lose, taken away without
            // waiting for its VM process to make the change: what it may reach meanwhile, the
            // open extents' memory, is kept until then, and nothing shares their pages with
            // another guest before then (`cached`). It may still store to them meanwhile, as it
            // must not: what it stores is what it reads, until they are paged out, its own.
            let (sealing, deferred) = dax.deferring(|dax| self.seal_mapped(dax, &sealed));
            if let Some(deferred) = deferred {
                let mut extents = extent_cache().extents.lock().unwrap();
                for (extent, _) in &sealed {
                    if let Some(cached) = extents.get_mut(&(self.arenas.of(*extent), *extent)) {
                        cached.sealing = Some(deferred.clone());
                    }
                }
                drop(extents);
                deferred.keep_until_made(Box::new(sealed));
            }
            sealing?;
        }
        // Durable without holding up the next writes and commits meanwhile.
        drop((dax, open));
        stats::add_since(Counter::CommitSealNs, sealing_started);
        let waiting = std::time::Instant::now();
        let waited = if durable {
            self.make_durable()
        } else {
            match self.remote.get() {
                // Within the lag the remote allows.
                Some(remote) => {
                    let epoch = self.store.epoch().map_err(errno)?;
                    remote.wait_lag(epoch).map_err(remote_errno)
                }
                None => Ok(()),
            }
        };
        stats::add_since(Counter::CommitWaitNs, waiting);
        waited
    }

    /// Maps the `sealed` extents, open until the commit that sealed them, read-only where the
    /// window maps them, from their sealed pages, which their memory becomes.
    fn seal_mapped(&self, dax: &mut Dax<Mapped>, sealed: &[(ExtentId, OpenExtent)]) -> Result<()> {
        for &(extent, ref extent_open) in sealed {
            let sealed = self.cached(extent, extent_open.len);
            // The window maps the sealed extent first, so that nothing reads its memory once its
            // pages move: a read meanwhile pages in from the store, which holds them.
            let pages = extent_open.len.next_multiple_of(page_size());
            let mut mapped = Vec::new();
            let is_kept = |mapped: &Mapped| mapped.keeps(&extent_open.vmo);
            // fxdax: as the window maps it now, each mapping at the offset it maps: a range
            // unmapped and mapped again to another part of the extent was sealed to the part it
            // mapped first (TASK-79.10).
            for (window_offset, end, keep) in dax.kept(extent_open.vmo.identity()) {
                let Some(offset) = keep.extent_offset(window_offset) else { continue };
                let len = end - window_offset;
                // What a read-only mapping maps past the sealed extent, zeros.
                let within = (offset + len).min(pages).max(offset) - offset;
                if within > 0 {
                    let keep = Mapped::extent(sealed.clone(), extent, window_offset, offset, false);
                    mapped.extend(dax.make_read_only(
                        window_offset,
                        within,
                        keep,
                        sealed.vmo(),
                        offset,
                        is_kept,
                    )?);
                }
                if within < len {
                    let past = window_offset + within;
                    let zeros = self.zeros(dax)?;
                    dax.make_read_only(
                        past,
                        len - within,
                        Mapped::zeros(&zeros),
                        &zeros,
                        past,
                        is_kept,
                    )?;
                }
            }
            // Its memory is the sealed extent's pages, but for those paged in meanwhile.
            sealed.supply_pages(0..pages, &extent_open.vmo, 0);
            // Mapped again, read-only, so that the guest's next loads of them do not fault one
            // page at a time, each a trip to the store's process from the guest's.
            for (window_offset, len) in mapped {
                // The faults the pages are otherwise mapped on would show what fails.
                if let Err(err) = dax.populate(window_offset, len) {
                    log::warn!("mapping sealed extent {extent:?}'s pages again: {err:?}");
                }
            }
        }
        Ok(())
    }

    /// The root as of the last commit, empty if none set one (`ROOT`).
    pub fn root(&self) -> Result<Vec<u8>> {
        match &self.following {
            Some(following) => Ok(following.view.lock().unwrap().root().to_vec()),
            None => self.store.root().map_err(errno),
        }
    }

    /// Creates the DAX window of `size` bytes that [`map`](Self::map) maps extents into, each
    /// change to whose mappings is made in `second_stage`, if given, rather than in this process's
    /// memory: the extent daemon's connection to the VM process (`crate::extentd`), which makes it
    /// in the VM's memory. The guest's faults on the window must be passed to
    /// [`DaxWindow::fault`], as the daemon passes those its VM process reports. A store has one
    /// window.
    pub fn create_dax_window(
        &self,
        size: u64,
        second_stage: Option<Arc<dyn SecondStage>>,
    ) -> Result<DaxWindow, Error> {
        self.install_dax(|| Ok(Dax::new(size, second_stage)?))
    }

    /// Creates the DAX window in the `size` bytes of host memory at `base`, for a hypervisor that
    /// follows host memory, such as KVM: the store takes the memory over, maps the window's pages
    /// there, and serves the faults on the rest with userfaultfd, on a thread of its own.
    /// `prefault`, if the hypervisor can map pages for the guest ahead of its accesses, is told of
    /// the pages mapped. All in this process, as tests have it: a VM process's window is the
    /// daemon's ([`RemoteExtentStore`](crate::RemoteExtentStore)'s of the same name).
    ///
    /// # Safety
    ///
    /// The memory must be a page-aligned mapping of `size` bytes that only the guest uses, until
    /// the store is dropped.
    #[cfg(all(test, target_os = "linux"))]
    unsafe fn create_dax_window_in_host_memory(
        &self,
        base: std::ptr::NonNull<u8>,
        size: u64,
        prefault: Option<Arc<dyn crate::Prefault>>,
    ) -> Result<DaxWindow, Error> {
        self.install_dax(|| {
            // SAFETY: The caller hands the memory over.
            let stage = unsafe {
                crate::dax::uffd::UffdSecondStage::in_process(
                    base.as_ptr() as usize,
                    size,
                    prefault,
                )
            }?;
            Dax::in_host_memory(stage)
        })
    }

    fn install_dax(
        &self,
        create: impl FnOnce() -> Result<(Dax<Mapped>, DaxWindow), Error>,
    ) -> Result<DaxWindow, Error> {
        let mut dax = self.dax.lock().unwrap();
        if dax.is_some() {
            return Err(Errno::EEXIST.into());
        }
        let (new, window) = create()?;
        *dax = Some(new);
        Ok(window)
    }

    /// Maps `len` bytes at `offset` in `extent` into the DAX window at `window_offset`, replacing
    /// what was mapped there (`MAP`). Offsets and length must be multiples of the page size.
    /// Without a window, fails with `ENOSYS`.
    ///
    /// Unless `writable`, the mapping is of a sealed extent's cached pages, or of an open one's
    /// memory as it is stored to and written, until the commit that seals it maps the sealed pages
    /// there. fxdax: what of the range is past the extent reads as zeros, as the padding of its
    /// last page does: blocks a write allocated, which a seal sealed unwritten, and which the write
    /// then moves (doc-4), which a guest maps as it reads them meanwhile.
    /// A writable mapping is of an extent of the store's branch that is open or new
    /// (`DAX_WRITE`), within `MAX_EXTENT`, which it grows to hold the range: of another branch's,
    /// or a sealed one, it fails with `EPERM`.
    pub fn map(
        &self,
        extent: ExtentId,
        offset: u64,
        len: u64,
        window_offset: u64,
        writable: bool,
    ) -> Result<()> {
        if writable {
            self.writable()?;
        }
        let start = std::time::Instant::now();
        let mapped = self.map_counted(extent, offset, len, window_offset, writable);
        stats::count_since(Counter::Maps, Counter::MapNs, start);
        mapped
    }

    fn map_counted(
        &self,
        extent: ExtentId,
        offset: u64,
        len: u64,
        window_offset: u64,
        writable: bool,
    ) -> Result<()> {
        self.dax.lock().unwrap().as_ref().ok_or(Errno::ENOSYS)?.check_range(window_offset, len)?;
        let extent_len = match &self.following {
            // A follower's view holds sealed extents alone.
            Some(following) => {
                let view = following.view.lock().unwrap();
                self.store.view_extent(&view, extent.into()).map_err(|e| outside_view(extent, e))?
            }
            None => {
                if self.map_open(extent, offset, len, window_offset, writable)? {
                    return Ok(());
                }
                self.store.extent(extent.into()).map_err(errno)?.ok_or(Errno::ENOENT)?.0
            }
        };
        let end = offset.checked_add(len).ok_or(Errno::EINVAL)?;
        if !offset.is_multiple_of(page_size()) {
            return Err(Errno::EINVAL);
        }
        let cached = self.cached(extent, extent_len);
        let mut dax = self.dax.lock().unwrap();
        let dax = dax.as_mut().ok_or(Errno::ENOSYS)?;
        let within = end.min(extent_len.next_multiple_of(page_size())).max(offset);
        if within > offset {
            let keep = Mapped::extent(cached.clone(), extent, window_offset, offset, false);
            dax.map(window_offset, keep, cached.vmo(), offset, within - offset, false)?;
        }
        self.map_zeros(dax, window_offset + (within - offset), end - within)
    }

    /// Maps zeros, read-only, at `len` bytes of the window at `window_offset`, if any.
    fn map_zeros(&self, dax: &mut Dax<Mapped>, window_offset: u64, len: u64) -> Result<()> {
        if len == 0 {
            return Ok(());
        }
        let zeros = self.zeros(dax)?;
        dax.map(window_offset, Mapped::zeros(&zeros), &zeros, window_offset, len, false)
    }

    /// The window's size of zeros, which it maps where nothing else is to be read.
    fn zeros(&self, dax: &Dax<Mapped>) -> Result<Arc<zx::Vmo>> {
        if let Some(zeros) = self.zeros.get() {
            return Ok(zeros.clone());
        }
        let zeros = Arc::new(zx::Vmo::create(dax.size()).errno()?);
        Ok(self.zeros.get_or_init(|| zeros).clone())
    }

    /// Maps an open extent's memory, which the commit that seals it maps the sealed pages in place
    /// of: writably if `writable`, which opens a new extent. Returns whether it did: not if the
    /// extent is sealed, and the mapping read-only.
    fn map_open(
        &self,
        extent: ExtentId,
        offset: u64,
        len: u64,
        window_offset: u64,
        writable: bool,
    ) -> Result<bool> {
        let end = offset.checked_add(len).ok_or(Errno::EINVAL)?;
        if !offset.is_multiple_of(page_size()) || end > Self::MAX_EXTENT {
            return Err(Errno::EINVAL);
        }
        if writable && extent.branch != self.branch() {
            return Err(Errno::EPERM);
        }
        let _opening = writable.then(|| self.opening.read().unwrap());
        let mut open = self.open.lock().unwrap();
        if !open.contains_key(&extent) {
            // Its contents so far: written, and open, or none.
            let len = match self.store.extent(extent.into()).map_err(errno)? {
                Some((_, false)) if writable => return Err(Errno::EPERM),
                Some((_, false)) => return Ok(false),
                Some((len, true)) => len,
                None if writable => 0,
                None => return Err(Errno::ENOENT),
            };
            let vmo =
                with_arena(self.arenas.own(), || zx::Vmo::create(Self::MAX_EXTENT)).errno()?;
            if len > 0 {
                let mut data = vec![0; len as usize];
                self.store.read(extent.into(), 0, &mut data).map_err(errno)?;
                vmo.write(&data, 0).errno()?;
            }
            let vmo = Arc::new(vmo);
            open.insert(extent, OpenExtent { vmo, len, stored: len });
        }
        let extent_open = open.get_mut(&extent).unwrap();
        // Its memory past what was written is zeros, until it is written and stored to.
        if writable {
            extent_open.len = extent_open.len.max(end);
        }
        let mut dax = self.dax.lock().unwrap();
        let dax = dax.as_mut().ok_or(Errno::ENOSYS)?;
        let vmo = &extent_open.vmo;
        if writable {
            // The guest maps what it is about to write: its pages are committed and mapped now,
            // rather than one fault at a time as it writes them.
            vmo.op_range(zx::VmoOp::COMMIT, offset, len).errno().inspect_err(|errno| {
                log::warn!(
                    "fxdax: committing {len:#x} bytes at {offset:#x} of {extent:?}: {errno:?}"
                )
            })?;
        }
        let keep = Mapped::extent(vmo.clone(), extent, window_offset, offset, writable);
        dax.map(window_offset, keep, vmo, offset, len, writable)?;
        Ok(true)
    }

    /// Maps zeros, read-only, at `len` bytes of the DAX window at `window_offset`, as a mapping a
    /// snapshot recorded did ([`MappingContents::Zeros`]).
    pub fn map_zeros_at(&self, window_offset: u64, len: u64) -> Result<()> {
        let mut dax = self.dax.lock().unwrap();
        let dax = dax.as_mut().ok_or(Errno::ENOSYS)?;
        dax.check_range(window_offset, len)?;
        self.map_zeros(dax, window_offset, len)
    }

    /// The DAX window's mappings, in order: what a snapshot of the VM records, to map them again
    /// where it is restored, at the same offsets, with [`ExtentStore::map`] and
    /// [`ExtentStore::map_zeros_at`]. Without a window, none.
    pub fn window_mappings(&self) -> Vec<WindowMapping> {
        let dax = self.dax.lock().unwrap();
        let Some(dax) = dax.as_ref() else { return Vec::new() };
        dax.mappings()
            .map(|(start, end, mapped)| WindowMapping {
                window_offset: start,
                len: end - start,
                contents: match mapped.maps {
                    Maps::Extent { extent, window_offset, offset, writable } => {
                        MappingContents::Extent {
                            extent,
                            offset: offset + (start - window_offset),
                            writable,
                        }
                    }
                    Maps::Zeros => MappingContents::Zeros,
                },
            })
            .collect()
    }

    /// Forks the store as [`ExtentStore::fork`] does, but only once nothing is open
    /// ([`ExtentStore::is_quiescent`]), and no write or writable mapping comes between that and the
    /// fork, which would seal it under its writer: `EBUSY` otherwise. So a running VM's store forks,
    /// its guest's filesystem frozen (TASK-39.3), as a snapshot of the VM does (TASK-39.1).
    pub fn fork_quiescent(&self, path: impl AsRef<Path>, branch: u64) -> Result<(), Error> {
        self.writable()?;
        let _opening = self.opening.write().unwrap();
        if !self.is_quiescent()? {
            return Err(Errno::EBUSY.into());
        }
        self.fork(path, branch)
    }

    /// Whether nothing is open: no extent written or mapped writably since the last commit, which
    /// a fork of the store as of that commit would not hold; as with its guest's filesystem frozen,
    /// a point a snapshot of its VM is taken at.
    pub fn is_quiescent(&self) -> Result<bool> {
        Ok(self.open.lock().unwrap().is_empty() && !self.store.has_open().map_err(errno)?)
    }

    /// Unmaps `len` bytes of the DAX window at `window_offset` (`UNMAP`).
    pub fn unmap(&self, window_offset: u64, len: u64) -> Result<()> {
        let mut dax = self.dax.lock().unwrap();
        let dax = dax.as_mut().ok_or(Errno::ENOSYS)?;
        dax.check_range(window_offset, len)?;
        dax.unmap(window_offset, len)
    }

    /// The sealed `extent`, of `len` bytes, in the cache: read from this store if no other store
    /// has it cached.
    fn cached(&self, extent: ExtentId, len: u64) -> Arc<FxExtent> {
        let remote = self.uploader().map(crate::remote::Remote::fetcher);
        let recorder = self.profiling.as_ref().map(|(recorder, _)| recorder.clone());
        cached(&self.store, self.arenas.of(extent), remote, recorder, extent, len)
    }

    /// fxdax: makes the store's branch public, and those it inherited from (TASK-43): a base,
    /// which every tenant forks, whose extents' pages, and its ancestors', are of the default
    /// arena of the kernel's physical memory, which every tenant's VM processes hold read-only.
    /// Stores opened since take it so; a tenant opens no public store itself, only forks of it.
    pub fn make_public(&self) -> Result<()> {
        self.writable()?;
        let parents: HashMap<u64, Option<u64>> =
            self.store.lineage().branches().into_iter().collect();
        let mut branch = Some(self.branch());
        while let Some(public) = branch {
            let marker = self.store.dir().path().join(public_marker(public));
            std::fs::File::create(&marker).and_then(|file| file.sync_all()).map_err(|err| {
                log::error!("fxdax: marking branch {public:x} public: {err}");
                Errno::EIO
            })?;
            branch = parents.get(&public).copied().flatten();
        }
        std::fs::File::open(self.store.dir().path()).and_then(|dir| dir.sync_all()).map_err(|err| {
            log::error!("fxdax: marking branches public: {err}");
            Errno::EIO
        })
    }

    /// fxdax: whether the store's branch is public ([`ExtentStore::make_public`]).
    pub fn is_public(&self) -> bool {
        self.arenas.public.contains(&self.branch())
    }

    /// fxdax: the arena of the kernel's physical memory its own extents' pages are of
    /// ([`ExtentStore::open_in`]): its tenant's, or the default.
    pub fn arena(&self) -> ArenaId {
        self.arenas.own()
    }
}

/// fxdax: the arenas of the kernel's physical memory a store's extents' pages are of (TASK-43).
#[derive(Clone)]
struct Arenas {
    /// The store's, its tenant's, held for as long as the store and what reads into the extent
    /// cache for it, which clone these, allocate in it (TASK-79.36); or none, the default.
    own: Option<Arc<Arena>>,
    /// The public branches of its lineage, whose extents' pages are of the default arena.
    public: Arc<std::collections::BTreeSet<u64>>,
}

impl Arenas {
    /// The store's arena.
    fn own(&self) -> ArenaId {
        self.own.as_ref().map_or(ArenaId::DEFAULT, |arena| arena.id())
    }

    /// The arena `extent`'s pages are of.
    fn of(&self, extent: ExtentId) -> ArenaId {
        if self.public.contains(&extent.branch) { ArenaId::DEFAULT } else { self.own() }
    }
}

/// fxdax: the name of the file in a store's directory that marks `branch` public.
fn public_marker(branch: u64) -> String {
    format!("public-{branch:016x}")
}

/// fxdax: the branches of the store in `dir` marked public.
fn public_branches(dir: &Path) -> std::collections::BTreeSet<u64> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            log::warn!("fxdax: listing {} for public branches: {err}", dir.display());
            return Default::default();
        }
    };
    entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter_map(|name| u64::from_str_radix(name.strip_prefix("public-")?, 16).ok())
        .collect()
}

/// The sealed `extent`, of `len` bytes, in the cache, cached from `store` if it isn't: read from
/// it, or, fxdax, from `remote`, with its page-ins recorded by `recorder`, if any.
fn cached(
    store: &Arc<Store<HostDir>>,
    arena: ArenaId,
    remote: Option<Arc<crate::remote::Fetcher>>,
    recorder: Option<Arc<Recorder>>,
    extent: ExtentId,
    len: u64,
) -> Arc<FxExtent> {
    let cache = extent_cache();
    let (cached, sealing) = {
        let mut extents = cache.extents.lock().unwrap();
        let cached = extents.entry((arena, extent)).or_insert_with(|| {
            let source = StoredExtent::new(store.clone(), extent.into(), remote, recorder);
            Cached {
                // fxdax: its pages of `arena`, as the pager supplies them.
                extent: with_arena(arena, || {
                    FxExtent::new(
                        cache.pager.clone(),
                        cache.allocator(arena),
                        Box::new(source),
                        len,
                        extent.sequence,
                    )
                }),
                store: store.clone(),
                sealing: None,
                used: AtomicU64::new(0),
            }
        });
        cached.used.store(CACHE_CLOCK.fetch_add(1, Ordering::Relaxed) + 1, Ordering::Relaxed);
        let shared = !Arc::ptr_eq(&cached.store, store);
        (cached.extent.clone(), cached.sealing.clone().filter(|_| shared))
    };
    // fxdax: another store's extent, a fork's parent's, whose guest may still store to its
    // pages until its seal is made: not shared with this store's guest before then.
    if let Some(sealing) = sealing {
        sealing.wait();
    }
    cached
}

/// How many threads read a boot profile's units into the cache at once.
const REPLAY_THREADS: usize = 4;

/// fxdax: reads `units`, a base's boot profile, into the extent cache from `store`, a fork of the
/// base, on threads of their own: those of the extents the store holds, sealed, whose data it
/// holds, by a read of each unit's first byte, which pages in the unit.
fn replay(store: Arc<Store<HostDir>>, arenas: Arenas, units: Vec<profile::Unit>) {
    let units = Arc::new(units);
    for first in 0..REPLAY_THREADS {
        let (store, arenas, units) = (store.clone(), arenas.clone(), units.clone());
        let spawned = std::thread::Builder::new().name("fxdax replay".into()).spawn(move || {
            for unit in units.iter().skip(first).step_by(REPLAY_THREADS) {
                let id: storage_device::ExtentId = unit.extent.into();
                let Ok(Some((len, false))) = store.extent(id) else { continue };
                if matches!(store.place(id), Ok(Some(extent_store::Place::Remote))) {
                    continue;
                }
                let extent = cached(&store, arenas.of(unit.extent), None, None, unit.extent, len);
                match extent.vmo().read(&mut [0; 1], unit.offset) {
                    Ok(()) => stats::add(Counter::Prefetches, 1),
                    Err(status) => log::debug!(
                        "fxdax: reading {:?} at {} ahead: {status}",
                        unit.extent,
                        unit.offset
                    ),
                }
            }
        });
        if let Err(err) = spawned {
            log::warn!("fxdax: starting a boot profile's replay: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dax::{Access, Protection, SecondStage};
    use crate::kernel::PhysLocation;
    use futures::executor::block_on;

    const BLOCK: usize = ExtentStore::BLOCK_SIZE as usize;

    fn store(capacity: u64) -> (ExtentStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (ExtentStore::create(dir.path().join("store"), capacity).unwrap(), dir)
    }

    fn id(store: &ExtentStore, sequence: u64) -> ExtentId {
        ExtentId { branch: store.branch(), sequence }
    }

    /// `len` bytes of `byte` plus their offset modulo 251.
    fn pattern(byte: u8, len: usize) -> Vec<u8> {
        (0..len).map(|i| byte ^ (i % 251) as u8).collect()
    }

    fn write(store: &ExtentStore, extent: ExtentId, offset: u64, data: &[u8]) -> Result<()> {
        // In two buffers, as a request's descriptors may hold it.
        let (first, second) = data.split_at(data.len() / 2);
        store.write(extent, offset, &[VolatileSlice::from(first), VolatileSlice::from(second)])
    }

    fn read(store: &ExtentStore, extent: ExtentId, offset: u64, len: usize) -> Result<Vec<u8>> {
        let mut data = vec![0; len];
        let (first, second) = data.split_at_mut(BLOCK.min(len));
        store.read(
            extent,
            offset,
            &mut [VolatileSliceMut::from(first), VolatileSliceMut::from(second)],
        )?;
        Ok(data)
    }

    #[test]
    fn serves_extents() {
        let (store, dir) = store(1 << 20);
        let (a, b) = (id(&store, 1), id(&store, 2));
        write(&store, a, 0, &pattern(1, 2 * BLOCK)).unwrap();
        write(&store, b, BLOCK as u64, &pattern(2, BLOCK)).unwrap();
        assert_eq!(read(&store, a, 0, 2 * BLOCK), Ok(pattern(1, 2 * BLOCK)));
        assert_eq!(read(&store, b, 0, 2 * BLOCK), Ok([vec![0; BLOCK], pattern(2, BLOCK)].concat()));
        store.commit(Some(b"root")).unwrap();
        write(&store, id(&store, 3), 0, &pattern(3, BLOCK)).unwrap();
        store.free(b).unwrap();

        // As doc-4 has them: an extent not held, a sealed one, another branch's, unaligned I/O, and
        // too much.
        assert_eq!(read(&store, b, 0, BLOCK), Err(Errno::ENOENT));
        assert_eq!(write(&store, a, 2 * BLOCK as u64, &[0; BLOCK]), Err(Errno::EPERM));
        let other = ExtentId { branch: store.branch() + 1, sequence: 1 };
        assert_eq!(write(&store, other, 0, &[0; BLOCK]), Err(Errno::EPERM));
        assert_eq!(write(&store, id(&store, 4), 1, &[0; BLOCK]), Err(Errno::EINVAL));
        assert_eq!(write(&store, id(&store, 4), 0, &vec![0; 1 << 20]), Err(Errno::ENOSPC));
        let info = store.info().unwrap();
        assert_eq!((info.used, info.next_sequence), (3 * BLOCK as u64, 4));

        // What wasn't committed is gone once the store is opened again.
        let branch = store.branch();
        drop(store);
        let store = ExtentStore::open(dir.path().join("store")).unwrap();
        assert_eq!(store.branch(), branch);
        assert_eq!(store.root(), Ok(b"root".to_vec()));
        assert_eq!(read(&store, b, BLOCK as u64, BLOCK), Ok(pattern(2, BLOCK)));
        assert_eq!(read(&store, id(&store, 3), 0, BLOCK), Err(Errno::ENOENT));
    }

    /// Reads of sealed extents come from the extent cache (TASK-32): a fork reading what its base
    /// read before reads nothing from the store. Past an extent's end, zeros, as the store reads.
    #[test]
    fn reads_of_sealed_extents_come_from_the_cache() {
        let (store, dir) = store(1 << 20);
        let extent = id(&store, 1);
        write(&store, extent, 0, &pattern(1, 2 * BLOCK)).unwrap();
        // Open: from the store.
        let before = stats::stats();
        assert_eq!(read(&store, extent, 0, 2 * BLOCK), Ok(pattern(1, 2 * BLOCK)));
        assert_eq!(stats::stats().since(&before).read_stored_bytes, 2 * BLOCK as u64);
        store.commit(None).unwrap();

        let before = stats::stats();
        let tail = [pattern(1, 2 * BLOCK)[BLOCK..].to_vec(), vec![0; BLOCK]].concat();
        assert_eq!(read(&store, extent, BLOCK as u64, 2 * BLOCK), Ok(tail));
        let first = stats::stats().since(&before);
        assert_eq!((first.read_stored_bytes, first.page_ins), (0, 1));

        let path = dir.path().join("fork");
        store.fork(&path, store.branch() + 1).unwrap();
        let fork = ExtentStore::open(&path).unwrap();
        let before = stats::stats();
        assert_eq!(read(&fork, extent, 0, 2 * BLOCK), Ok(pattern(1, 2 * BLOCK)));
        let again = stats::stats().since(&before);
        assert_eq!((again.read_stored_bytes, again.page_ins, again.page_in_bytes), (0, 0, 0));
    }

    /// A follower reads its writer's store as of its view's commit while the writer goes on: what
    /// the writer freed since, still; what it wrote since, not (ENOENT, as a journal's end); and,
    /// advanced, what the commits between held until it lets go of them, and then, loudly (EIO),
    /// not.
    #[test]
    fn followers_read_their_views_commits() {
        let (store, _dir) = store(1 << 20);
        let store = Arc::new(store);
        let (a, b) = (id(&store, 1), id(&store, 2));
        write(&store, a, 0, &pattern(1, BLOCK)).unwrap();
        store.commit(Some(b"one")).unwrap();
        let follower = store.follow();
        assert!(follower.is_follower());
        store.free(a).unwrap();
        write(&store, b, 0, &pattern(2, BLOCK)).unwrap();
        store.commit(Some(b"two")).unwrap();
        assert_eq!(read(&follower, a, 0, BLOCK), Ok(pattern(1, BLOCK)));
        assert_eq!(read(&follower, b, 0, BLOCK), Err(Errno::ENOENT));
        assert_eq!(follower.root(), Ok(b"one".to_vec()));

        let epoch = follower.advance().unwrap();
        assert_eq!(follower.epoch(), Ok(epoch));
        assert_eq!(follower.root(), Ok(b"two".to_vec()));
        assert_eq!(read(&follower, b, 0, BLOCK), Ok(pattern(2, BLOCK)));
        assert_eq!(read(&follower, a, 0, BLOCK), Ok(pattern(1, BLOCK)));
        follower.let_go(epoch).unwrap();
        assert_eq!(read(&follower, a, 0, BLOCK), Err(Errno::EIO));
        // Pinned, an extent stays, whatever commit the follower is as of.
        follower.pin(b).unwrap();
        store.free(b).unwrap();
        store.commit(None).unwrap();
        let epoch = follower.advance().unwrap();
        follower.let_go(epoch).unwrap();
        assert_eq!(read(&follower, b, 0, BLOCK), Ok(pattern(2, BLOCK)));
        follower.unpin(b).unwrap();
        assert_eq!(read(&follower, b, 0, BLOCK), Err(Errno::EIO));

        // Dropped, it leaves its writer's store open.
        drop(follower);
        write(&store, id(&store, 3), 0, &pattern(3, BLOCK)).unwrap();
        store.commit(None).unwrap();
    }

    /// A follower changes nothing: what would, it refuses with EROFS.
    #[test]
    fn followers_change_nothing() {
        let (store, dir) = store(1 << 20);
        let store = Arc::new(store);
        let a = id(&store, 1);
        write(&store, a, 0, &pattern(1, BLOCK)).unwrap();
        store.commit(None).unwrap();
        let follower = store.follow();
        assert_eq!(write(&follower, id(&store, 2), 0, &pattern(2, BLOCK)), Err(Errno::EROFS));
        assert_eq!(follower.free(a), Err(Errno::EROFS));
        assert_eq!(follower.commit(None), Err(Errno::EROFS));
        assert_eq!(follower.commit_ordered(None), Err(Errno::EROFS));
        assert_eq!(follower.set_capacity(2 << 20), Err(Errno::EROFS));
        let forked = follower.fork(dir.path().join("fork"), host_store::new_branch());
        assert_eq!(forked.map_err(|err| err.errno()), Err(Some(Errno::EROFS)));
        let _window = follower.create_dax_window(4 * page_size(), None).unwrap();
        assert_eq!(follower.map(id(&store, 2), 0, page_size(), 0, true), Err(Errno::EROFS));
        assert_eq!(store.follow().advance().map(|_| ()), Ok(()));
        assert_eq!(store.advance(), Err(Errno::EINVAL));
    }

    /// A follower maps what its view's commit holds, sharing the cached pages the writer maps;
    /// what the writer wrote since, it doesn't (ENOENT).
    #[test]
    fn followers_map_their_views_commits() {
        let (store, _dir) = store(1 << 20);
        let store = Arc::new(store);
        let ps = page_size();
        let a = id(&store, 1);
        write(&store, a, 0, &pattern(4, ps as usize)).unwrap();
        store.commit(None).unwrap();
        let follower = store.follow();
        let b = id(&store, 2);
        write(&store, b, 0, &pattern(5, ps as usize)).unwrap();
        store.commit(None).unwrap();
        let window = follower.create_dax_window(4 * ps, None).unwrap();
        follower.map(a, 0, ps, 0, false).unwrap();
        assert_eq!(follower.map(b, 0, ps, ps, false), Err(Errno::ENOENT));
        block_on(window.fault(0, Access::Read)).unwrap();
        // SAFETY: The test faulted the page in, and nothing else accesses it.
        let bytes = unsafe { std::slice::from_raw_parts(window.host_base(), ps as usize) };
        assert!(bytes == pattern(4, ps as usize), "the follower's window maps other data");
    }

    /// A new writer takes a store over from its followers, or from a writer that went, once
    /// nothing is open in it, which they go on following; its window goes with the writer that
    /// went. Cut off, the followers fail with ESTALE.
    #[test]
    fn writers_take_over_stores_followed() {
        let (store, _dir) = store(1 << 20);
        let store = Arc::new(store);
        let follower = store.follow();
        assert_eq!(follower.take_over(), Err(Errno::EINVAL));
        store.take_over().unwrap();
        let _window = store.create_dax_window(4 * page_size(), None).unwrap();
        let a = id(&store, 1);
        write(&store, a, 0, &pattern(1, BLOCK)).unwrap();
        assert_eq!(store.take_over(), Err(Errno::EBUSY));
        store.commit(None).unwrap();
        store.take_over().unwrap();
        let _window = store.create_dax_window(4 * page_size(), None).unwrap();
        follower.advance().unwrap();
        assert_eq!(read(&follower, a, 0, BLOCK), Ok(pattern(1, BLOCK)));

        store.cut_followers();
        assert_eq!(read(&follower, a, 0, BLOCK), Err(Errno::ESTALE));
        assert_eq!(follower.advance(), Err(Errno::ESTALE));
        assert_eq!(follower.pin(a), Err(Errno::ESTALE));
        let fresh = store.follow();
        assert_eq!(read(&fresh, a, 0, BLOCK), Ok(pattern(1, BLOCK)));
    }

    /// A follower learns of its writer's commits after its view's.
    #[test]
    fn followers_hear_of_commits() {
        let (store, _dir) = store(1 << 20);
        let store = Arc::new(store);
        store.commit(None).unwrap();
        let follower = store.follow();
        let epoch = follower.epoch().unwrap();
        assert_eq!(follower.newer(epoch, std::time::Duration::from_millis(10)), None);
        write(&store, id(&store, 1), 0, &pattern(1, BLOCK)).unwrap();
        store.commit(None).unwrap();
        let newer = follower.newer(epoch, std::time::Duration::from_secs(10));
        assert_eq!(newer, Some(follower.advance().unwrap()));
    }

    #[test]
    fn maps_sealed_extents() {
        let (store, _dir) = store(1 << 20);
        let ps = page_size();
        let extent = id(&store, 1);
        // Not a whole number of pages where pages are 16 KiB.
        let len = ps as usize + BLOCK;
        write(&store, extent, 0, &pattern(5, len)).unwrap();
        assert_eq!(store.map(extent, 0, ps, 0, false), Err(Errno::ENOSYS));
        let window = store.create_dax_window(6 * ps, None).unwrap();
        store.commit(None).unwrap();
        store.map(extent, 0, 2 * ps, ps, false).unwrap();
        // Past the extent, zeros.
        store.map(extent, 0, 3 * ps, 3 * ps, false).unwrap();
        assert_eq!(store.map(id(&store, 9), 0, ps, 0, false), Err(Errno::ENOENT));

        block_on(async {
            window.fault(ps, Access::Read).await.unwrap();
            window.fault(2 * ps, Access::Read).await.unwrap();
        });
        // SAFETY: The test faulted the pages in, and nothing else accesses them.
        let bytes = unsafe {
            std::slice::from_raw_parts(window.host_base().add(ps as usize), 2 * ps as usize)
        };
        assert!(bytes[..len] == pattern(5, len)[..], "the window maps other data");
        assert!(bytes[len..].iter().all(|byte| *byte == 0), "the padding isn't zeros");
        block_on(window.fault(5 * ps, Access::Read)).unwrap();
        // SAFETY: As above.
        let past = unsafe {
            std::slice::from_raw_parts(window.host_base().add(5 * ps as usize), ps as usize)
        };
        assert!(past.iter().all(|byte| *byte == 0), "past the extent isn't zeros");
        // Unmapped, the range has nothing to fault in, which an access's fault there is told.
        store.unmap(ps, 2 * ps).unwrap();
        assert!(!window.is_mapped(ps));
        assert_eq!(block_on(window.fault(2 * ps, Access::Read)), Err(Errno::ENOENT));
    }

    /// A commit that seals an open extent maps it read-only where the window still maps it, and
    /// leaves what was mapped over part of its mapping since.
    #[test]
    fn seals_map_what_the_open_extent_still_maps() {
        let (store, _dir) = store(1 << 20);
        let ps = page_size();
        let (sealed, open) = (id(&store, 1), id(&store, 2));
        write(&store, sealed, 0, &pattern(1, ps as usize)).unwrap();
        store.commit(None).unwrap();
        let _window = store.create_dax_window(4 * ps, None).unwrap();
        store.map(open, 0, 2 * ps, 0, true).unwrap();
        write(&store, open, 0, &pattern(2, 2 * ps as usize)).unwrap();
        store.map(sealed, 0, ps, ps, false).unwrap();
        store.commit(None).unwrap();
        let extent = |extent, offset| MappingContents::Extent { extent, offset, writable: false };
        assert_eq!(
            store.window_mappings(),
            [
                WindowMapping { window_offset: 0, len: ps, contents: extent(open, 0) },
                WindowMapping { window_offset: ps, len: ps, contents: extent(sealed, 0) },
            ]
        );
    }

    /// A window range unmapped, then mapped again to another offset of the same open extent, is
    /// sealed to that offset, not to the one it mapped before: the guest's loads there would read
    /// another part of the extent (TASK-79.10; the sandbox team's pgbench, whose index build found
    /// a key duplicated, TASK-59).
    #[test]
    fn seals_map_a_reused_range_at_its_offset() {
        let (store, _dir) = store(1 << 20);
        let ps = page_size();
        let open = id(&store, 1);
        let _window = store.create_dax_window(4 * ps, None).unwrap();
        write(&store, open, 0, &pattern(1, 2 * ps as usize)).unwrap();
        store.map(open, 0, ps, 0, true).unwrap();
        store.unmap(0, ps).unwrap();
        store.map(open, ps, ps, 0, true).unwrap();
        store.commit(None).unwrap();
        let sealed = MappingContents::Extent { extent: open, offset: ps, writable: false };
        assert_eq!(
            store.window_mappings(),
            [WindowMapping { window_offset: 0, len: ps, contents: sealed }]
        );
    }

    /// Commits ordered wait for a flush, which makes them durable.
    #[test]
    fn ordered_commits_wait_for_a_flush() {
        let (store, _dir) = store(1 << 20);
        write(&store, id(&store, 1), 0, &pattern(1, BLOCK)).unwrap();
        store.commit_ordered(None).unwrap();
        assert!(store.flush_pending());
        store.flush().unwrap();
        assert!(!store.flush_pending());
    }

    /// What a read-only mapping of an open extent maps past its last page, which a writable one
    /// would have grown it to, is zeros, read-only, once a commit seals it.
    #[test]
    fn seals_map_zeros_past_the_extents_end() {
        let (store, _dir) = store(1 << 20);
        let ps = page_size();
        let open = id(&store, 1);
        let _window = store.create_dax_window(4 * ps, None).unwrap();
        write(&store, open, 0, &pattern(2, ps as usize)).unwrap();
        store.map(open, 0, 3 * ps, ps, false).unwrap();
        store.commit(None).unwrap();
        let sealed = MappingContents::Extent { extent: open, offset: 0, writable: false };
        assert_eq!(
            store.window_mappings(),
            [
                WindowMapping { window_offset: ps, len: ps, contents: sealed },
                WindowMapping {
                    window_offset: 2 * ps,
                    len: 2 * ps,
                    contents: MappingContents::Zeros
                },
            ]
        );
    }

    /// A read-only mapping of a sealed extent maps zeros past its last page, and only zeros if it
    /// is all past it.
    #[test]
    fn mappings_past_a_sealed_extents_end_map_zeros() {
        let (store, _dir) = store(1 << 20);
        let ps = page_size();
        let sealed = id(&store, 1);
        write(&store, sealed, 0, &pattern(3, 2 * ps as usize)).unwrap();
        store.commit(None).unwrap();
        let _window = store.create_dax_window(8 * ps, None).unwrap();
        store.map(sealed, ps, 3 * ps, 0, false).unwrap();
        store.map(sealed, 2 * ps, ps, 5 * ps, false).unwrap();
        let contents = MappingContents::Extent { extent: sealed, offset: ps, writable: false };
        assert_eq!(
            store.window_mappings(),
            [
                WindowMapping { window_offset: 0, len: ps, contents },
                WindowMapping { window_offset: ps, len: 2 * ps, contents: MappingContents::Zeros },
                WindowMapping { window_offset: 5 * ps, len: ps, contents: MappingContents::Zeros },
            ]
        );
    }

    /// Mappings of open extents are of pages within the largest extent.
    #[test]
    fn open_mappings_are_pages_within_the_largest_extent() {
        let (store, _dir) = store(1 << 30);
        let ps = page_size();
        let _window = store.create_dax_window(ps, None).unwrap();
        let last = ExtentStore::MAX_EXTENT - ps;
        assert_eq!(store.map(id(&store, 1), ps / 2, ps, 0, true), Err(Errno::EINVAL));
        assert_eq!(store.map(id(&store, 1), last + ps, ps, 0, true), Err(Errno::EINVAL));
        store.map(id(&store, 1), last, ps, 0, true).unwrap();
    }

    /// TASK-50.3: makes a store at `path` of `extents` sealed extents of a MiB each, and opens it
    /// in `arena`, with a window mapping them all, read-only.
    fn mapped_extents(path: &Path, extents: u64, arena: &Arc<Arena>) -> (ExtentStore, DaxWindow) {
        const EXTENT: u64 = 1 << 20;
        let store = ExtentStore::create(path, 1 << 30).unwrap();
        for sequence in 1..=extents {
            write(&store, id(&store, sequence), 0, &pattern(sequence as u8, EXTENT as usize))
                .unwrap();
        }
        store.commit(None).unwrap();
        drop(store);
        let store = ExtentStore::open_in(path, Some(arena.clone())).unwrap();
        let window = store.create_dax_window(extents * EXTENT, None).unwrap();
        for sequence in 1..=extents {
            store.map(id(&store, sequence), 0, EXTENT, (sequence - 1) * EXTENT, false).unwrap();
        }
        (store, window)
    }

    /// TASK-50.3: reads `extents` extents of `mapped_extents`' through `window`, a page at a time,
    /// twice over, checking their bytes, and calls `read` after each.
    fn read_twice(window: &DaxWindow, extents: u64, read: impl Fn(u64, u64)) {
        const EXTENT: usize = 1 << 20;
        let ps = page_size();
        for round in 0..2 {
            for sequence in 1..=extents {
                let expected = pattern(sequence as u8, EXTENT);
                for offset in (0..EXTENT as u64).step_by(ps as usize) {
                    let at = (sequence - 1) * EXTENT as u64 + offset;
                    let at_page = format!("round {round}, extent {sequence}, offset {offset:#x}");
                    block_on(window.fault(at, Access::Read)).expect(&at_page);
                    // SAFETY: The page is mapped, and nothing else accesses it.
                    let byte = unsafe { *window.host_base().add(at as usize) };
                    assert_eq!(byte, expected[offset as usize], "{at_page}");
                }
                read(round, sequence);
            }
        }
    }

    /// TASK-50.3: a store whose arena is capped reads through its window more than the cap holds:
    /// the extents asked for longest ago give their pages back, paged in again as they are touched,
    /// and the arena stays within its cap.
    #[test]
    fn capped_arenas_evict_their_least_used_extents() {
        let chunk = 2u64 << 20;
        let dir = tempfile::tempdir().unwrap();
        let held = Arena::new();
        let arena = held.id();
        let node = zircon::vm::pmm::node();
        node.set_arena_cap(arena, Some(4 * chunk));
        let (store, window) = mapped_extents(&dir.path().join("store"), 16, &held);
        let first = id(&store, 1);
        // What the cache holds of extent 1.
        let cached = || {
            let extents = EXTENT_CACHE.get().unwrap().extents.lock().unwrap();
            extents[&(arena, first)].extent.vmo().info().unwrap().committed_bytes
        };
        read_twice(&window, 16, |round, sequence| {
            let held = node.arena_bytes(arena);
            assert!(held <= 4 * chunk, "round {round}, extent {sequence}: {held}");
            // Once all were read, the one asked for longest ago gave its pages back, as its
            // first page's read in again.
            if (round, sequence) == (0, 16) {
                assert_eq!(cached(), 0, "extent 1 was not evicted");
            }
        });
    }

    /// TASK-50.3: a kernel out of memory, its arenas uncapped, evicts the extents asked for longest
    /// ago, any arena's, rather than wait for memory for ever, and abort.
    #[test]
    fn kernels_out_of_memory_evict_their_least_used_extents() {
        let memory = 48u64 << 20;
        crate::kernel::configure(crate::kernel::Config::with_memory(memory)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let node = zircon::vm::pmm::node();
        let (_store, window) = mapped_extents(&dir.path().join("store"), 64, &Arena::new());
        read_twice(&window, 64, |_, _| ());
        assert_eq!(node.count_free_chunks(), 0, "the kernel never ran out");
    }

    /// TASK-50.3: a store whose arena is capped, with nothing it holds to evict, as an open
    /// extent's pages, refuses to map more for the guest to write than the cap holds: the mapping
    /// fails, with ENOMEM, rather than wait, and the arena stays within its cap.
    #[test]
    fn capped_arenas_refuse_what_they_cannot_evict() {
        let chunk = 2u64 << 20;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store");
        drop(ExtentStore::create(&path, 1 << 30).unwrap());
        let held = Arena::new();
        let arena = held.id();
        let node = zircon::vm::pmm::node();
        node.set_arena_cap(arena, Some(2 * chunk));
        let store = ExtentStore::open_in(&path, Some(held)).unwrap();
        let len = 4 * chunk;
        let _window = store.create_dax_window(len, None).unwrap();
        assert_eq!(store.map(id(&store, 1), 0, len, 0, true), Err(Errno::ENOMEM));
        assert_eq!(node.arena_bytes(arena), 2 * chunk);
    }

    /// A store holds a block at least; one asked for less is not made, nor its directory.
    #[test]
    fn stores_hold_a_block_at_least() {
        let dir = tempfile::tempdir().unwrap();
        let block = ExtentStore::BLOCK_SIZE as u64;
        let small = dir.path().join("small");
        let error = ExtentStore::create(&small, block - 1).err().unwrap();
        assert_eq!(error.to_string(), "the store is smaller than a block");
        assert!(!small.exists());
        let store = ExtentStore::create(dir.path().join("one"), block).unwrap();
        assert_eq!(store.info().unwrap().capacity, block);
    }

    /// Bytes are zeros when every one is, words and the bytes after them alike.
    #[test]
    fn zeros_are_every_byte() {
        assert!(is_zeros(&[0; 17]));
        assert!(is_zeros(&[]));
        let mut tail = [0; 9];
        tail[8] = 5;
        assert!(!is_zeros(&tail));
        let mut pair = [0; 10];
        pair[8..].copy_from_slice(&[3, 3]);
        assert!(!is_zeros(&pair));
        assert!(!is_zeros(&[7; 16]));
    }

    /// TASK-39.1: the window's mappings, as a snapshot of the VM records them once nothing is
    /// open, mapped again in a window of a fork of the store, map the same bytes at the same
    /// offsets: mappings split by later ones keep their place in their extents.
    #[test]
    fn window_mappings_map_the_same_again_in_a_fork() {
        let (store, dir) = store(1 << 20);
        let ps = page_size();
        let (sealed, open) = (id(&store, 1), id(&store, 2));
        write(&store, sealed, 0, &pattern(1, 2 * ps as usize)).unwrap();
        store.commit(None).unwrap();
        let window = store.create_dax_window(8 * ps, None).unwrap();
        store.map(sealed, 0, 2 * ps, 0, false).unwrap();
        store.map(sealed, ps, ps, 4 * ps, false).unwrap();
        store.map_zeros_at(2 * ps, ps).unwrap();
        // Zeros over the first page of the first mapping: what is left of it maps the extent's
        // second page.
        store.map_zeros_at(0, ps).unwrap();
        store.map(open, 0, ps, 6 * ps, true).unwrap();
        write(&store, open, 0, &pattern(2, ps as usize)).unwrap();
        assert_eq!(store.is_quiescent(), Ok(false));
        assert_eq!(
            store.window_mappings()[4],
            WindowMapping {
                window_offset: 6 * ps,
                len: ps,
                contents: MappingContents::Extent { extent: open, offset: 0, writable: true },
            }
        );
        store.commit(None).unwrap();
        assert_eq!(store.is_quiescent(), Ok(true));
        let extent = |extent, offset| MappingContents::Extent { extent, offset, writable: false };
        let mappings = store.window_mappings();
        assert_eq!(
            mappings,
            [
                WindowMapping { window_offset: 0, len: ps, contents: MappingContents::Zeros },
                WindowMapping { window_offset: ps, len: ps, contents: extent(sealed, ps) },
                WindowMapping { window_offset: 2 * ps, len: ps, contents: MappingContents::Zeros },
                WindowMapping { window_offset: 4 * ps, len: ps, contents: extent(sealed, ps) },
                WindowMapping { window_offset: 6 * ps, len: ps, contents: extent(open, 0) },
            ]
        );

        let path = dir.path().join("fork");
        store.fork(&path, store.branch() + 1).unwrap();
        let fork = ExtentStore::open(&path).unwrap();
        let fork_window = fork.create_dax_window(8 * ps, None).unwrap();
        for mapping in &mappings {
            match mapping.contents {
                MappingContents::Extent { extent, offset, writable } => {
                    fork.map(extent, offset, mapping.len, mapping.window_offset, writable)
                }
                MappingContents::Zeros => fork.map_zeros_at(mapping.window_offset, mapping.len),
            }
            .unwrap();
        }
        assert_eq!(fork.window_mappings(), mappings);
        for mapping in &mappings {
            let bytes = |window: &DaxWindow| {
                block_on(window.fault(mapping.window_offset, Access::Read)).unwrap();
                // SAFETY: The test faulted the page in, and nothing else accesses it.
                unsafe {
                    std::slice::from_raw_parts(
                        window.host_base().add(mapping.window_offset as usize),
                        ps as usize,
                    )
                }
                .to_vec()
            };
            assert!(bytes(&window) == bytes(&fork_window), "{mapping:?} maps other bytes");
        }
    }

    /// fxdax: a sealed extent stored compressed pages in a part (`extent_frame::PART`) at a time,
    /// decompressing each once, however many faults read it.
    #[test]
    fn page_ins_read_whole_parts() {
        let (store, _dir) = store(64 << 20);
        let ps = page_size();
        let extent = id(&store, 1);
        // Large enough to be stored on its own, compressed, in four parts.
        let len = 4 * extent_frame::PART as usize;
        write(&store, extent, 0, &pattern(5, len)).unwrap();
        let window = store.create_dax_window(len as u64, None).unwrap();
        store.commit(None).unwrap();
        store.map(extent, 0, len as u64, 0, false).unwrap();
        let before = stats::stats();
        // From its end, so that nothing is read ahead.
        block_on(async {
            let pages: Vec<u64> = (0..len as u64).step_by(ps as usize).collect();
            for page in pages.into_iter().rev() {
                window.fault(page, Access::Read).await.unwrap();
            }
        });
        let counted = stats::stats().since(&before);
        assert_eq!((counted.page_ins, counted.page_in_bytes, counted.unpacks), (4, len as u64, 4));
        // SAFETY: The test faulted the pages in, and nothing else accesses them.
        let bytes = unsafe { std::slice::from_raw_parts(window.host_base(), len) };
        assert!(*bytes == pattern(5, len)[..], "the window maps other data");
    }

    /// fxdax: page-ins of an extent sealed in a segment, at an offset that is not a part's, are of
    /// the segment's parts, each read once: four for an extent of 3 MiB in four parts.
    #[test]
    fn page_ins_read_the_parts_extents_are_in() {
        let (store, _dir) = store(64 << 20);
        let ps = page_size();
        let (before, extent) = (id(&store, 1), id(&store, 2));
        write(&store, before, 0, &pattern(4, 300 << 10)).unwrap();
        let len = 3 * extent_frame::PART as usize;
        write(&store, extent, 0, &pattern(5, len)).unwrap();
        let window = store.create_dax_window(len as u64, None).unwrap();
        store.commit(None).unwrap();
        // Its segment closed, compressed in parts, the extent 300 KiB into the first.
        store.store.close_segment().unwrap();
        store.commit(None).unwrap();
        store.map(extent, 0, len as u64, 0, false).unwrap();
        let counted = stats::stats();
        // From its end, so that nothing is read ahead.
        block_on(async {
            let pages: Vec<u64> = (0..len as u64).step_by(ps as usize).collect();
            for page in pages.into_iter().rev() {
                window.fault(page, Access::Read).await.unwrap();
            }
        });
        let counted = stats::stats().since(&counted);
        assert_eq!((counted.page_ins, counted.page_in_bytes, counted.unpacks), (4, len as u64, 4));
        // SAFETY: The test faulted the pages in, and nothing else accesses them.
        let bytes = unsafe { std::slice::from_raw_parts(window.host_base(), len) };
        assert!(*bytes == pattern(5, len)[..], "the window maps other data");
    }

    /// fxdax: a reader reading an extent from its start has the parts after the one it reads read
    /// ahead of it, before it faults on them.
    #[test]
    fn page_ins_read_ahead_of_a_reader_reading_on() {
        let (store, _dir) = store(64 << 20);
        let extent = id(&store, 1);
        let len = 4 * extent_frame::PART as usize;
        write(&store, extent, 0, &pattern(5, len)).unwrap();
        let window = store.create_dax_window(len as u64, None).unwrap();
        store.commit(None).unwrap();
        store.map(extent, 0, len as u64, 0, false).unwrap();
        let before = stats::stats();
        block_on(window.fault(0, Access::Read)).unwrap();
        // The first part, and the two after it, read ahead, in time.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let read = || stats::stats().since(&before).page_in_bytes;
        while read() < 3 * extent_frame::PART && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let counted = stats::stats().since(&before);
        assert_eq!((counted.page_ins, counted.page_in_bytes), (3, 3 * extent_frame::PART));
    }

    /// fxdax: a fork of a base records the units of the base's extents its page-ins read, as the
    /// base's boot profile, which closing it writes; the forks opened after it read them into the
    /// cache, and record nothing.
    #[test]
    fn forks_record_and_replay_their_bases_boot_profile() {
        let (base, dir) = store(64 << 20);
        let ps = page_size();
        let extent = id(&base, 1);
        let part = extent_frame::PART;
        let len = 4 * part as usize;
        write(&base, extent, 0, &pattern(5, len)).unwrap();
        base.commit(None).unwrap();
        let (first, second) = (dir.path().join("first"), dir.path().join("second"));
        base.fork(&first, base.branch() + 1).unwrap();
        base.fork(&second, base.branch() + 2).unwrap();
        let profile = profile::path(&dir.path().join("store"), base.branch());
        drop(base);

        // The first fork reads two of the extent's parts, from its end.
        let fork = ExtentStore::open(&first).unwrap();
        let window = fork.create_dax_window(len as u64, None).unwrap();
        fork.map(extent, 0, len as u64, 0, false).unwrap();
        block_on(async {
            window.fault(3 * part + ps, Access::Read).await.unwrap();
            window.fault(part, Access::Read).await.unwrap();
        });
        assert!(!profile.exists());
        drop(window);
        drop(fork);
        let units = profile::read(&profile).unwrap();
        let at = |offset| profile::Unit { extent, offset, len: part };
        assert_eq!(units, [at(3 * part), at(part)]);

        // The second reads them ahead of its guest, and records nothing.
        let before = stats::stats();
        let fork = ExtentStore::open(&second).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while stats::stats().since(&before).prefetches < 2 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(stats::stats().since(&before).prefetches, 2);
        assert!(fork.profiling.is_none());
        drop(fork);
        assert_eq!(profile::read(&profile).unwrap(), units);
    }

    /// fxdax: a read fault on a sealed extent maps the pages its page-in supplied after it, a part
    /// of the extent, rather than a few: reading on takes no more faults.
    #[test]
    fn read_faults_map_what_page_ins_supplied() {
        let (store, _dir) = store(64 << 20);
        let ps = page_size();
        let extent = id(&store, 1);
        let part = extent_frame::PART;
        let len = 4 * part as usize;
        write(&store, extent, 0, &pattern(5, len)).unwrap();
        let stage = Arc::new(RecordingSecondStage::default());
        let window = store.create_dax_window(len as u64, Some(stage.clone())).unwrap();
        store.commit(None).unwrap();
        // Nothing of it is in memory: mapping it maps no page yet.
        store.map(extent, 0, len as u64, 0, false).unwrap();
        assert_eq!(*stage.0.lock().unwrap(), []);
        // A fault on each part's first page maps the part.
        block_on(async {
            for start in (0..len as u64).step_by(part as usize) {
                window.fault(start, Access::Read).await.unwrap();
            }
        });
        let mapped: std::collections::BTreeSet<u64> = (stage.0.lock().unwrap().iter())
            .filter(|(op, ..)| *op == "map")
            .map(|(_, page, _)| *page)
            .collect();
        let pages: std::collections::BTreeSet<u64> = (0..len as u64).step_by(ps as usize).collect();
        assert_eq!(mapped, pages);
    }

    /// fxdax: a part paged in is pages in a run of physical memory, which a window maps with one
    /// host mapping, as the parts of an extent paged in at once are: a window of a fork maps a
    /// base's extents, paged in, as cheaply as the base's window mapped them.
    #[test]
    fn windows_map_a_part_paged_in_with_one_mapping() {
        let (base, dir) = store(64 << 20);
        let extent = id(&base, 1);
        let part = extent_frame::PART;
        let parts = 8;
        let len = parts * part;
        write(&base, extent, 0, &pattern(5, len as usize)).unwrap();
        let window = base.create_dax_window(len, None).unwrap();
        base.commit(None).unwrap();
        let forked = dir.path().join("fork");
        base.fork(&forked, base.branch() + 1).unwrap();
        base.map(extent, 0, len, 0, false).unwrap();
        // Each part paged in by a fault of its own, at once, as a guest's threads fault.
        std::thread::scope(|scope| {
            for start in (0..len).step_by(part as usize) {
                let window = &window;
                scope.spawn(move || block_on(window.fault(start, Access::Read)).unwrap());
            }
        });
        let fork = ExtentStore::open(&forked).unwrap();
        let stage = Arc::new(RunRecordingSecondStage::default());
        let _window = fork.create_dax_window(len, Some(stage.clone())).unwrap();
        fork.map(extent, 0, len, 0, false).unwrap();
        let runs = stage.0.lock().unwrap().clone();
        let in_one_run: Vec<bool> = (0..parts)
            .map(|at| {
                let (start, end) = (at * part, (at + 1) * part);
                runs.iter().any(|(offset, len)| *offset <= start && end <= offset + len)
            })
            .collect();
        assert_eq!(in_one_run, vec![true; parts as usize], "runs mapped: {runs:?}");
    }

    /// A second stage recording the runs it maps: their offsets and lengths.
    #[derive(Default)]
    struct RunRecordingSecondStage(std::sync::Mutex<Vec<(u64, u64)>>);

    impl SecondStage for RunRecordingSecondStage {
        fn map(&self, offset: u64, _: PhysLocation, len: u64, _: Protection) {
            self.0.lock().unwrap().push((offset, len));
        }

        fn protect(&self, _: u64, _: u64, _: Protection) {}

        fn unmap(&self, _: u64, _: u64) {}
    }

    /// fxdax: a store of a branch kept in object storage, opened where its data is not, maps the
    /// branch's extents: their pages are fetched as the window's faults page them in.
    #[test]
    fn maps_extents_whose_data_is_in_object_storage() {
        let dir = tempfile::tempdir().unwrap();
        let ps = page_size();
        let len = ps as usize + BLOCK;
        let bucket = Arc::new(extent_remote::store::Store::new(
            Arc::new(object_store::memory::InMemory::new()),
            "",
        ));
        let options = crate::remote::Options::default();
        let first = ExtentStore::create(dir.path().join("first"), 1 << 20).unwrap();
        first.attach_remote(bucket.clone(), 1, options.clone()).unwrap();
        let extent = id(&first, 1);
        write(&first, extent, 0, &pattern(7, len)).unwrap();
        first.commit(None).unwrap();
        let branch = first.branch();
        drop(first);

        let store =
            ExtentStore::create_for_branch(dir.path().join("second"), 1 << 20, branch).unwrap();
        store.attach_remote(bucket, 2, options).unwrap();
        assert_eq!(read(&store, extent, 0, len), Ok(pattern(7, len)));
        let window = store.create_dax_window(4 * ps, None).unwrap();
        store.map(extent, 0, 2 * ps, ps, false).unwrap();
        block_on(async {
            window.fault(ps, Access::Read).await.unwrap();
            window.fault(2 * ps, Access::Read).await.unwrap();
        });
        // SAFETY: The test faulted the pages in, and nothing else accesses them.
        let bytes = unsafe {
            std::slice::from_raw_parts(window.host_base().add(ps as usize), 2 * ps as usize)
        };
        assert!(bytes[..len] == pattern(7, len)[..], "the window maps other data");
        assert!(bytes[len..].iter().all(|byte| *byte == 0), "the padding isn't zeros");
    }

    /// A window maps as many open extents writably as it has room for, each written first, as a
    /// guest's filesystem writes many small files.
    #[test]
    fn maps_many_open_extents_writably() {
        const EXTENTS: u64 = 1024;
        let (store, _dir) = store(1 << 30);
        let ps = page_size();
        let chunk = 2 << 20;
        let _window = store.create_dax_window(EXTENTS * chunk, None).unwrap();
        for i in 0..EXTENTS {
            let extent = id(&store, 1000 + i);
            write(&store, extent, 0, &pattern(i as u8, BLOCK)).unwrap();
            assert_eq!(store.map(extent, 0, ps, i * chunk, true), Ok(()), "extent {i}");
        }
    }

    /// A second stage recording the changes made to it, page by page.
    #[derive(Default)]
    struct RecordingSecondStage(std::sync::Mutex<Vec<(&'static str, u64, Protection)>>);

    impl SecondStage for RecordingSecondStage {
        fn map(&self, offset: u64, _: PhysLocation, len: u64, protection: Protection) {
            let mut ops = self.0.lock().unwrap();
            for page in (0..len).step_by(page_size() as usize) {
                ops.push(("map", offset + page, protection));
            }
        }

        fn protect(&self, offset: u64, len: u64, protection: Protection) {
            let mut ops = self.0.lock().unwrap();
            for page in (0..len).step_by(page_size() as usize) {
                ops.push(("protect", offset + page, protection));
            }
        }

        fn unmap(&self, offset: u64, len: u64) {
            let mut ops = self.0.lock().unwrap();
            for page in (0..len).step_by(page_size() as usize) {
                ops.push(("unmap", offset + page, Protection::None));
            }
        }
    }

    /// fxdax: a commit keeps an open extent as far as what was stored to it, one block at least:
    /// the pages mapped writably past it, which nothing was stored to, read as zeros past its end,
    /// through the store and the window alike, and take no room.
    #[test]
    fn commit_leaves_out_what_nothing_was_stored_to() {
        let (store, _dir) = store(1 << 20);
        let ps = page_size();
        let window = store.create_dax_window(8 * ps, None).unwrap();
        let (stored, unused) = (id(&store, 1), id(&store, 2));
        store.map(stored, 0, 4 * ps, 0, true).unwrap();
        store.map(unused, 0, 4 * ps, 4 * ps, true).unwrap();
        write(&store, stored, 0, &pattern(4, BLOCK)).unwrap();
        let used = store.info().unwrap().used;
        store.commit(None).unwrap();

        assert_eq!(store.info().unwrap().used, used + 2 * BLOCK as u64);
        assert_eq!(
            read(&store, stored, 0, 2 * BLOCK),
            Ok([pattern(4, BLOCK), vec![0; BLOCK]].concat())
        );
        assert_eq!(read(&store, unused, 0, BLOCK), Ok(vec![0; BLOCK]));
        block_on(async {
            window.fault(3 * ps, Access::Read).await.unwrap();
            window.fault(7 * ps, Access::Read).await.unwrap();
        });
        // SAFETY: The test faulted the pages in, and nothing else accesses them.
        let bytes = unsafe { std::slice::from_raw_parts(window.host_base(), 8 * ps as usize) };
        assert!(bytes[..BLOCK] == pattern(4, BLOCK)[..], "the stored data");
        assert!(bytes[3 * ps as usize..4 * ps as usize].iter().all(|byte| *byte == 0));
        assert!(bytes[7 * ps as usize..].iter().all(|byte| *byte == 0));
    }

    /// The hypervisor's second stage follows the window: an open extent's pages are mapped
    /// writable as the guest maps them, and read-only once a commit seals them; a sealed extent's
    /// are mapped read-only as the guest faults on them; unmapping unmaps them.
    #[test]
    fn second_stage_follows_the_window() {
        use Protection::{Read, ReadWrite};
        let (store, _dir) = store(1 << 20);
        let ps = page_size();
        let stage = Arc::new(RecordingSecondStage::default());
        let window = store.create_dax_window(4 * ps, Some(stage.clone())).unwrap();
        let take = || std::mem::take(&mut *stage.0.lock().unwrap());
        let open = id(&store, 1);
        store.map(open, 0, 2 * ps, 0, true).unwrap();
        assert_eq!(take(), [("map", 0, ReadWrite), ("map", ps, ReadWrite)]);
        // What the extent holds: a commit leaves out what nothing was stored to.
        write(&store, open, 0, &pattern(3, 2 * ps as usize)).unwrap();
        // Sealed, the pages are mapped again, read-only, from the sealed extent.
        store.commit(None).unwrap();
        assert_eq!(
            take(),
            [
                ("unmap", 0, Protection::None),
                ("unmap", ps, Protection::None),
                ("map", 0, Read),
                ("map", ps, Read)
            ]
        );
        store.map(open, 0, ps, 3 * ps, false).unwrap();
        block_on(window.fault(3 * ps, Access::Read)).unwrap();
        assert_eq!(take(), [("map", 3 * ps, Read)]);
        store.unmap(0, 2 * ps).unwrap();
        assert_eq!(take(), [("unmap", 0, Protection::None), ("unmap", ps, Protection::None)]);
    }

    /// fxdax: freeing an extent maps zeros where the window mapped it, read-only, rather than
    /// what kept its pages: a sealed extent's, and an open one's; others stay.
    #[test]
    fn freeing_an_extent_maps_zeros_in_its_place() {
        let (store, _dir) = store(1 << 20);
        let ps = page_size();
        let window = store.create_dax_window(8 * ps, None).unwrap();
        let (sealed, kept, open) = (id(&store, 1), id(&store, 2), id(&store, 3));
        store.map(sealed, 0, 2 * ps, 0, true).unwrap();
        store.map(kept, 0, ps, 4 * ps, true).unwrap();
        store.commit(None).unwrap();
        store.map(open, 0, ps, 6 * ps, true).unwrap();
        assert!(window.is_mapped(0) && window.is_mapped(ps) && window.is_mapped(6 * ps));

        let sealed_pages = cached(sealed).unwrap();
        let weak = Arc::downgrade(&sealed_pages);
        drop(sealed_pages);
        store.free(sealed).unwrap();
        store.free(open).unwrap();
        // Mapped, but no longer keeping the extents' pages: the sealed extent is gone.
        assert!(window.is_mapped(0) && window.is_mapped(ps) && window.is_mapped(6 * ps));
        assert!(weak.upgrade().is_none(), "the window still keeps the freed extent's pages");
        assert!(cached(sealed).is_none());
        // Zeros, read-only: a store faults rather than writing them.
        block_on(async {
            window.fault(0, Access::Read).await.unwrap();
            window.fault(6 * ps, Access::Read).await.unwrap();
        });
        // SAFETY: The test faulted the pages in, and nothing else accesses them.
        let bytes = unsafe { std::slice::from_raw_parts(window.host_base(), ps as usize) };
        assert!(bytes.iter().all(|byte| *byte == 0), "a freed extent's range doesn't read zeros");
        assert!(window.is_mapped(4 * ps), "an extent not freed was unmapped");
    }

    /// A second stage that defers the changes that take access away until the test makes them.
    #[derive(Default)]
    struct DeferringStage {
        deferral: Arc<TestDeferral>,
    }

    #[derive(Default)]
    struct TestDeferral {
        /// Whether the changes are made, what is kept until they are, and how many waited.
        state: std::sync::Mutex<(bool, Vec<Box<dyn Send>>, usize)>,
        changed: std::sync::Condvar,
    }

    impl TestDeferral {
        fn make(&self) {
            let kept = {
                let mut state = self.state.lock().unwrap();
                state.0 = true;
                std::mem::take(&mut state.1)
            };
            self.changed.notify_all();
            drop(kept);
        }

        fn waiting(&self) -> usize {
            self.state.lock().unwrap().2
        }
    }

    impl Deferred for TestDeferral {
        fn wait(&self) {
            let mut state = self.state.lock().unwrap();
            state.2 += 1;
            self.changed.notify_all();
            while !state.0 {
                state = self.changed.wait(state).unwrap();
            }
        }

        fn keep_until_made(&self, keep: Box<dyn Send>) {
            self.state.lock().unwrap().1.push(keep);
        }
    }

    impl SecondStage for DeferringStage {
        fn map(&self, _: u64, _: PhysLocation, _: u64, _: Protection) {}
        fn protect(&self, _: u64, _: u64, _: Protection) {}
        fn unmap(&self, _: u64, _: u64) {}
        fn end_deferring(&self) -> Option<Arc<dyn Deferred>> {
            Some(self.deferral.clone())
        }
    }

    /// fxdax: a commit takes its guest's access to the pages it seals away without waiting for the
    /// change to be made, keeping what the guest may still reach until it is: another store, a
    /// fork's, that maps the extent waits for it; the store that sealed it doesn't.
    #[test]
    fn forks_wait_for_seals_to_be_made() {
        let (store, dir) = store(1 << 20);
        let ps = page_size();
        let stage = Arc::new(DeferringStage::default());
        let _window = store.create_dax_window(4 * ps, Some(stage.clone())).unwrap();
        let extent = id(&store, 1);
        store.map(extent, 0, ps, 0, true).unwrap();
        write(&store, extent, 0, &pattern(6, BLOCK)).unwrap();
        store.commit(None).unwrap();
        assert!(stage.deferral.state.lock().unwrap().1.len() == 1, "nothing kept");
        store.map(extent, 0, ps, ps, false).unwrap();
        assert_eq!(stage.deferral.waiting(), 0, "the sealing store waited");

        let fork = dir.path().join("fork");
        store.fork(&fork, store.branch() + 1).unwrap();
        let fork = Arc::new(ExtentStore::open(&fork).unwrap());
        let _fork_window = fork.create_dax_window(4 * ps, None).unwrap();
        let mapped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mapping = std::thread::spawn({
            let (fork, mapped, deferral) = (fork.clone(), mapped.clone(), stage.deferral.clone());
            move || {
                fork.map(extent, 0, ps, 0, false).unwrap();
                mapped.store(true, std::sync::atomic::Ordering::SeqCst);
                let _state = deferral.state.lock().unwrap();
                deferral.changed.notify_all();
            }
        });
        // The fork's MAP waits for the seal.
        let deferral = &stage.deferral;
        let mut state = deferral.state.lock().unwrap();
        while state.2 == 0 && !mapped.load(std::sync::atomic::Ordering::SeqCst) {
            state = deferral.changed.wait(state).unwrap();
        }
        drop(state);
        assert!(!mapped.load(std::sync::atomic::Ordering::SeqCst), "the fork didn't wait");
        deferral.make();
        mapping.join().unwrap();
        assert!(mapped.load(std::sync::atomic::Ordering::SeqCst));
        assert!(deferral.state.lock().unwrap().1.is_empty(), "kept after the seal was made");
    }

    /// A window in host memory takes whole pages.
    #[cfg(target_os = "linux")]
    #[test]
    fn dax_window_in_host_memory_must_be_whole_pages() {
        let (store, _dir) = store(1 << 20);
        let ps = page_size() as usize;
        let mut memory = vec![0u8; 3 * ps];
        let aligned = memory.as_mut_ptr().wrapping_add(ps - memory.as_ptr() as usize % ps);
        let base = std::ptr::NonNull::new(aligned).unwrap();
        // SAFETY: Fails before using the memory.
        let result = unsafe { store.create_dax_window_in_host_memory(base, ps as u64 + 1, None) };
        let error = result.expect_err("succeeded").to_string();
        assert_eq!(error, "DAX window memory must be whole pages");
    }

    /// A writable mapping of an open extent maps its pages as it is made, for the guest to store
    /// to without faulting on each.
    #[test]
    fn maps_open_extents_pages_writably_at_once() {
        let (store, _dir) = store(1 << 20);
        let ps = page_size();
        let window = store.create_dax_window(4 * ps, None).unwrap();
        store.map(id(&store, 1), 0, 2 * ps, ps, true).unwrap();
        let mapped: Vec<bool> = (0..4).map(|page| window.is_mapped(page * ps)).collect();
        assert_eq!(mapped, [false, true, true, false]);
    }

    /// A hypervisor that maps a window's host memory for the guest, recording the pages it is told
    /// the window maps there.
    #[cfg(target_os = "linux")]
    #[derive(Default)]
    struct RecordingPrefault(std::sync::Mutex<Vec<u64>>);

    #[cfg(target_os = "linux")]
    impl crate::Prefault for RecordingPrefault {
        fn prefault(&self, offset: u64, len: u64) {
            let ps = page_size();
            self.0.lock().unwrap().extend((offset..offset + len).step_by(ps as usize));
        }
    }

    /// The hypervisor maps an open extent's pages for the guest as the window maps them writably,
    /// so that the guest's stores to them do not fault.
    #[cfg(target_os = "linux")]
    #[test]
    fn prefaults_open_extents_pages() {
        let (store, _dir) = store(1 << 20);
        let ps = page_size();
        // SAFETY: Maps anonymous memory, which the test owns.
        let memory = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                4 * ps as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(memory, libc::MAP_FAILED);
        let prefault = Arc::new(RecordingPrefault::default());
        // SAFETY: The memory is the test's, and outlives the store, whose window is leaked with it.
        let _window = unsafe {
            store.create_dax_window_in_host_memory(
                std::ptr::NonNull::new(memory.cast()).unwrap(),
                4 * ps,
                Some(prefault.clone()),
            )
        }
        .unwrap();
        store.map(id(&store, 1), 0, 2 * ps, ps, true).unwrap();
        assert_eq!(*prefault.0.lock().unwrap(), [ps, 2 * ps]);
        drop(store);
        // SAFETY: Unmaps the memory, which nothing uses any more.
        assert_eq!(unsafe { libc::munmap(memory, 4 * ps as usize) }, 0);
    }

    /// Windows mapping the same extent, from a store and a fork of it, map the same pages.
    #[test]
    fn windows_share_extents_pages() {
        let (store, dir) = store(1 << 20);
        let ps = page_size();
        let extent = id(&store, 1);
        write(&store, extent, 0, &pattern(6, ps as usize)).unwrap();
        store.commit(None).unwrap();
        let copy_path = dir.path().join("copy");
        store.fork(&copy_path, store.branch() + 1).unwrap();
        let copy = ExtentStore::open(&copy_path).unwrap();
        let (window, copy_window) =
            (store.create_dax_window(ps, None).unwrap(), copy.create_dax_window(ps, None).unwrap());
        store.map(extent, 0, ps, 0, false).unwrap();
        copy.map(extent, 0, ps, 0, false).unwrap();

        // Counted before anything pages in: a page-in holds the extent too, a while after the
        // fault it serves returns.
        let extent_cached = cached(extent).unwrap();
        // Each window's mapping holds the one copy of the extent's pages, which the cache keeps.
        assert_eq!(Arc::strong_count(&extent_cached), 4);
        copy.unmap(0, ps).unwrap();
        assert_eq!(Arc::strong_count(&extent_cached), 3, "unmapped, the mapping let go");
        copy.map(extent, 0, ps, 0, false).unwrap();
        block_on(async {
            window.fault(0, Access::Read).await.unwrap();
            copy_window.fault(0, Access::Read).await.unwrap();
        });
        for window in [&window, &copy_window] {
            // SAFETY: The test faulted the page in, and nothing writes it.
            let mapped = unsafe { std::slice::from_raw_parts(window.host_base(), ps as usize) };
            assert!(mapped == pattern(6, ps as usize), "a window maps other data");
        }
        store.unmap(0, ps).unwrap();
        copy.unmap(0, ps).unwrap();
        // Unmapped, it stays cached, for the next read or mapping, until the kernel evicts its
        // pages.
        assert!(Arc::ptr_eq(&cached(extent).unwrap(), &extent_cached));
        drop(extent_cached);
        // Freed by the store whose cached pages they are, it is no longer cached.
        store.free(extent).unwrap();
        assert!(cached(extent).is_none());
        assert_eq!(read(&copy, extent, 0, BLOCK), Ok(pattern(6, BLOCK)));
    }

    /// A read-only mapping of an open extent maps its memory, which sees what is written and
    /// stored to it, until the commit that seals it maps the sealed pages there: as a guest maps an
    /// extent it takes for sealed while a seal is in flight.
    #[test]
    fn maps_open_extents_read_only() {
        let (store, _dir) = store(1 << 20);
        let ps = page_size();
        let window = store.create_dax_window(4 * ps, None).unwrap();
        let (written, stored) = (id(&store, 1), id(&store, 2));
        write(&store, written, 0, &pattern(1, ps as usize)).unwrap();
        store.map(written, 0, ps, 0, false).unwrap();
        // One only stored to, through a writable mapping.
        store.map(stored, 0, ps, ps, true).unwrap();
        store.map(stored, 0, ps, 2 * ps, false).unwrap();
        // Past what was written, zeros; not of one never written.
        store.map(written, ps, ps, 3 * ps, false).unwrap();
        assert_eq!(store.map(id(&store, 3), 0, ps, 3 * ps, false), Err(Errno::ENOENT));
        block_on(async {
            for page in 0..4 {
                let access = if page == 1 { Access::Write } else { Access::Read };
                window.fault(page * ps, access).await.unwrap();
            }
        });
        // SAFETY: The test faulted the pages in, and only it accesses them.
        let bytes = unsafe { std::slice::from_raw_parts_mut(window.host_base(), 4 * ps as usize) };
        bytes[ps as usize..2 * ps as usize].copy_from_slice(&pattern(2, ps as usize));
        assert!(bytes[..ps as usize] == pattern(1, ps as usize)[..], "the written data");
        assert!(bytes[2 * ps as usize..3 * ps as usize] == pattern(2, ps as usize)[..]);
        assert!(bytes[3 * ps as usize..].iter().all(|byte| *byte == 0), "past what was written");
        assert!(block_on(window.fault(0, Access::Write)).is_err(), "the mapping is writable");

        store.commit(None).unwrap();
        block_on(async {
            for page in [0, 2, 3] {
                window.fault(page * ps, Access::Read).await.unwrap();
            }
        });
        // SAFETY: As above.
        let bytes = unsafe { std::slice::from_raw_parts(window.host_base(), 4 * ps as usize) };
        assert!(bytes[..ps as usize] == pattern(1, ps as usize)[..], "the sealed written data");
        assert!(bytes[2 * ps as usize..3 * ps as usize] == pattern(2, ps as usize)[..]);
        assert!(bytes[3 * ps as usize..].iter().all(|byte| *byte == 0), "past the sealed extent");
        assert_eq!(Arc::strong_count(&cached(stored).unwrap()), 4);
    }

    /// The process's cached sealed `extent`, if it is cached.
    fn cached(extent: ExtentId) -> Option<Arc<FxExtent>> {
        let extents = extent_cache().extents.lock().unwrap();
        extents.get(&(ArenaId::DEFAULT, extent)).map(|cached| cached.extent.clone())
    }

    /// Stores through a writable mapping of a new extent are its data once a commit seals it, after
    /// which the mapping is read-only, and the sealed extent's pages the ones stored to.
    #[test]
    fn maps_open_extents_writably() {
        let (store, dir) = store(1 << 20);
        let ps = page_size();
        let window = store.create_dax_window(4 * ps, None).unwrap();
        let (new, written) = (id(&store, 1), id(&store, 2));
        // One with data written to it before, which the mapping holds.
        write(&store, written, 0, &pattern(7, ps as usize)).unwrap();
        store.map(new, ps, ps, 0, true).unwrap();
        store.map(written, ps, ps, ps, true).unwrap();
        store.map(written, 0, ps, 2 * ps, true).unwrap();
        block_on(async {
            for page in 0..2 {
                window.fault(page * ps, Access::Write).await.unwrap();
            }
            window.fault(2 * ps, Access::Read).await.unwrap();
        });
        // SAFETY: The test faulted the pages in, and nothing else accesses them.
        let bytes = unsafe { std::slice::from_raw_parts_mut(window.host_base(), 3 * ps as usize) };
        assert!(bytes[2 * ps as usize..] == pattern(7, ps as usize)[..], "the old data is gone");
        bytes[..ps as usize].copy_from_slice(&pattern(8, ps as usize));
        bytes[ps as usize..2 * ps as usize].copy_from_slice(&pattern(9, ps as usize));
        // Reads of an open extent see the stores.
        assert_eq!(read(&store, new, ps, BLOCK), Ok(pattern(8, BLOCK)));

        // Not another branch's, nor once sealed.
        let other = ExtentId { branch: store.branch() + 1, sequence: 1 };
        assert_eq!(store.map(other, 0, ps, 3 * ps, true), Err(Errno::EPERM));
        store.commit(None).unwrap();
        assert_eq!(store.map(new, 0, ps, 3 * ps, true), Err(Errno::EPERM));
        // The mapping is read-only, of the sealed extent's cached pages.
        assert!(block_on(window.fault(0, Access::Write)).is_err(), "the mapping is writable");
        let new_cached = cached(new).unwrap();
        // Another window mapping the sealed extent, of a fork of the store, maps the same pages.
        let copy_path = dir.path().join("copy");
        store.fork(&copy_path, store.branch() + 1).unwrap();
        let copy = ExtentStore::open(&copy_path).unwrap();
        let copy_window = copy.create_dax_window(ps, None).unwrap();
        copy.map(new, ps, ps, 0, false).unwrap();
        block_on(copy_window.fault(0, Access::Read)).unwrap();
        // The cache, the two windows' mappings, and this.
        assert_eq!(Arc::strong_count(&new_cached), 4);
        // SAFETY: The test faulted the page in, and nothing writes it.
        let copied = unsafe { std::slice::from_raw_parts(copy_window.host_base(), ps as usize) };
        assert!(copied == pattern(8, ps as usize), "the copy's window maps other data");
        drop((new_cached, copy_window, copy));

        drop(store);
        let store = ExtentStore::open(dir.path().join("store")).unwrap();
        let new_data = [vec![0; ps as usize], pattern(8, ps as usize)].concat();
        assert_eq!(read(&store, new, 0, 2 * ps as usize), Ok(new_data));
        let written_data = [pattern(7, ps as usize), pattern(9, ps as usize)].concat();
        assert_eq!(read(&store, written, 0, 2 * ps as usize), Ok(written_data));
    }

    /// What one page-in reads (`fxextent`'s read-ahead).
    const READ_AHEAD: u64 = 128 * 1024;

    /// A source whose reads all wait until `readers` of them are waiting at once, then fill the
    /// buffer with the `pattern` of their chunk.
    struct Held {
        readers: usize,
        /// The reads waiting, and their wakers.
        waiting: Mutex<(usize, Vec<std::task::Waker>)>,
        /// Notified when all are.
        all: std::sync::Condvar,
    }

    struct HeldSource(Arc<Held>);

    impl ExtentSource for HeldSource {
        fn read<'a>(
            &'a self,
            offset: u64,
            buf: &'a mut [u8],
        ) -> futures::future::BoxFuture<'a, std::result::Result<(), anyhow::Error>> {
            let held = &self.0;
            let mut counted = false;
            Box::pin(async move {
                std::future::poll_fn(|context| {
                    let mut waiting = held.waiting.lock().unwrap();
                    if !counted {
                        counted = true;
                        waiting.0 += 1;
                        if waiting.0 == held.readers {
                            waiting.1.drain(..).for_each(std::task::Waker::wake);
                            held.all.notify_all();
                        }
                    }
                    if waiting.0 >= held.readers {
                        return std::task::Poll::Ready(());
                    }
                    waiting.1.push(context.waker().clone());
                    std::task::Poll::Pending
                })
                .await;
                buf.copy_from_slice(&pattern((offset / READ_AHEAD) as u8, buf.len()));
                Ok(())
            })
        }
    }

    /// fxdax (TASK-28.8): page-ins waiting for their extents' reads hold no thread: more reads
    /// wait at once than the pager has threads, and all go on.
    #[test]
    fn page_ins_wait_holding_no_thread() {
        let readers = 4 * PAGER_THREADS as usize;
        let held = Arc::new(Held {
            readers,
            waiting: Mutex::new((0, Vec::new())),
            all: std::sync::Condvar::new(),
        });
        let cache = extent_cache();
        let extent = FxExtent::new(
            cache.pager.clone(),
            cache.allocator(ArenaId::DEFAULT),
            Box::new(HeldSource(held.clone())),
            readers as u64 * READ_AHEAD,
            u64::MAX,
        );
        let faults: Vec<_> = (0..readers as u64)
            .map(|chunk| {
                let extent = extent.clone();
                std::thread::spawn(move || {
                    let mut data = vec![0; BLOCK];
                    extent.vmo().read(&mut data, chunk * READ_AHEAD).unwrap();
                    data
                })
            })
            .collect();
        // Were page-ins to hold their threads while they waited, no more than the pager's would.
        let waiting = held.waiting.lock().unwrap();
        let (waiting, timeout) = held
            .all
            .wait_timeout_while(waiting, std::time::Duration::from_secs(30), |waiting| {
                waiting.0 < readers
            })
            .unwrap();
        assert!(!timeout.timed_out(), "only {} of {readers} reads waited at once", waiting.0);
        drop(waiting);
        for (chunk, fault) in faults.into_iter().enumerate() {
            assert_eq!(fault.join().unwrap(), pattern(chunk as u8, BLOCK));
        }
    }
}
