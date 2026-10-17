// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An extent store for a guest's own filesystem: what a VMM's virtio extent-store device serves
//! (backlog doc-4), when the guest runs Fxfs itself rather than fxdax serving it over virtio-fs.
//!
//! An [`ExtentStore`] is an image (`crates/extent_store`) whose extents the guest writes, reads and
//! frees by ID. Its I/O copies between the image's files and guest memory, and blocks the calling
//! thread: a device's queue threads call it.
//!
//! Sealed extents are cached in the in-process kernel's memory, keyed by extent ID, which every
//! store and window in the process shares: each is a pager-backed VMO (`FxExtent`, Fuchsia's
//! `FxBlob` as an extent), whose pages the pager reads from the store as they are first read or
//! mapped, and which the kernel evicts under memory pressure. Windows map it read-only: one copy of
//! each page, whichever store or fork maps it, as an extent never changes once sealed. Reads go to
//! the store, for the guest's page cache, which keeps what they read: the host keeps no copy of it.
//! The store's files bypass the host's page cache.
//!
//! The guest's open extents map writably (doc-4's `DAX_WRITE`): an extent mapped so is memory of
//! the kernel's until the next commit, which reads and writes of it use too. The commit writes it
//! to the store, seals it, maps the window's mappings of it to its VMO in the cache, and moves its
//! memory's pages there.

use crate::dax::{Dax, DaxWindow, Deferred, Keep, SecondStage};
use crate::error::{Errno, Error, IntoErrno, Result};
use crate::executor::Executor;
use crate::kernel::page_size;
use crate::memory::{VolatileSlice, VolatileSliceMut};
use crate::stats::{self, Counter};
use extent_store::{Description, HostDir, NoSpace, Options, Store, TooSmall, Unaligned};
use fxfs_platform::fuchsia::fxextent::{ExtentSource, FxExtent};
use fxfs_platform::fuchsia::pager::Pager;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use storage_device::buffer_allocator::{BufferAllocator, BufferSource};
use storage_device::{NoExtent, OtherBranch, SealedExtent};
use vfs::execution_scope::ExecutionScope;

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

/// The process's cache of sealed extents (see the module docs): their pager, on an executor of
/// its own, which reads their pages from the stores, and the extents, by ID.
struct ExtentCache {
    pager: Arc<Pager>,
    allocator: Arc<BufferAllocator>,
    extents: Mutex<HashMap<ExtentId, Cached>>,
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
}

/// How many threads the cache's pager reads extents' pages on.
const PAGER_THREADS: u8 = 2;
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
        let block_size = ExtentStore::BLOCK_SIZE as usize;
        let allocator =
            Arc::new(BufferAllocator::new(block_size, BufferSource::new(PAGER_BUFFERS)));
        ExtentCache { pager, allocator, extents: Mutex::new(HashMap::new()), _executor: executor }
    })
}

/// A sealed extent's contents, in a store, or, fxdax, in object storage, if the store holds it
/// without its data.
struct StoredExtent {
    store: Arc<Store<HostDir>>,
    extent: storage_device::ExtentId,
    remote: Option<Arc<crate::remote::Fetcher>>,
    /// Where the parts of the object its data is in begin in it (`read_ahead_phase`).
    phase: u64,
}

impl StoredExtent {
    fn new(
        store: Arc<Store<HostDir>>,
        extent: storage_device::ExtentId,
        remote: Option<Arc<crate::remote::Fetcher>>,
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
        Self { store, extent, remote, phase }
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
        match self.store.read(self.extent, offset, buf) {
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

/// What a window's mapping keeps alive: an open extent's memory, or a sealed extent in the cache.
type Pages = Arc<dyn Send + Sync>;

/// Whether `pages` keeps `vmo` alive.
fn keeps(pages: &Pages, vmo: &Arc<zx::Vmo>) -> bool {
    pages.identity() == vmo.identity()
}

/// An open extent that a window maps writably: its contents, in the kernel's memory, and how long
/// it is.
struct OpenExtent {
    vmo: Arc<zx::Vmo>,
    len: u64,
    /// fxdax: how long the store held it as written when it was opened here, which its commit
    /// keeps at least.
    stored: u64,
    /// The window's mappings of it: their offsets in the window and in the extent, and lengths.
    mappings: Vec<(u64, u64, u64)>,
}

/// An extent store a guest's filesystem lives on. See the module docs.
pub struct ExtentStore {
    store: Arc<Store<HostDir>>,
    /// The open extents mapped writably, until the next commit. Locked before `dax`.
    open: Mutex<HashMap<ExtentId, OpenExtent>>,
    dax: Mutex<Option<Dax<Pages>>>,
    /// fxdax: zeros, the window's size of them, which the window maps where it mapped an extent
    /// freed since: made once one is.
    zeros: std::sync::OnceLock<Arc<zx::Vmo>>,
    /// fxdax: the uploader of the store's branch to object storage, if it has one: durable
    /// commits are uploaded ones then (doc-4's `COMMIT_DURABLE`).
    remote: std::sync::OnceLock<crate::remote::Remote>,
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

/// fxdax: the errno of the uploader's failure, as doc-4 has them: a branch another writer took is
/// one the store takes no more commits for.
fn remote_errno(failure: crate::remote::Failure) -> Errno {
    match failure {
        crate::remote::Failure::Taken => Errno::EROFS,
        crate::remote::Failure::Other(err) => {
            log::error!("fxdax: uploading the store's commits: {err}");
            Errno::EIO
        }
    }
}

/// The errno of the store's error, as doc-4 has them.
fn errno(error: anyhow::Error) -> Errno {
    if error.downcast_ref::<NoExtent>().is_some() {
        Errno::ENOENT
    } else if error.downcast_ref::<SealedExtent>().is_some()
        || error.downcast_ref::<OtherBranch>().is_some()
    {
        Errno::EPERM
    } else if error.downcast_ref::<NoSpace>().is_some()
        || error.downcast_ref::<TooSmall>().is_some()
    {
        Errno::ENOSPC
    } else if error.downcast_ref::<Unaligned>().is_some() {
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
        if let Err(err) = self.store.close() {
            log::error!("fxdax: closing an extent store: {err:#}");
        }
    }
}

impl ExtentStore {
    /// The size of the blocks extents are read and written in.
    pub const BLOCK_SIZE: u32 = 4096;

    /// The most bytes an extent holds (doc-4's `max_extent`).
    pub const MAX_EXTENT: u64 = 64 << 20;

    /// A new store at `path`, which must not exist, holding at most `capacity` bytes, with a branch
    /// of its own.
    pub fn create(path: impl AsRef<Path>, capacity: u64) -> Result<Self, Error> {
        Self::create_for_branch(path, capacity, crate::storage::new_branch())
    }

    /// fxdax: a new store at `path`, as [`ExtentStore::create`] makes, of `branch`: one kept in
    /// object storage, opened on a host without it, takes what its branch holds once attached
    /// ([`ExtentStore::attach_remote`]).
    pub fn create_for_branch(
        path: impl AsRef<Path>,
        capacity: u64,
        branch: u64,
    ) -> Result<Self, Error> {
        let path = path.as_ref();
        if capacity < Self::BLOCK_SIZE as u64 {
            return Err(Error::msg("the store is smaller than a block"));
        }
        let description = Description { block_size: Self::BLOCK_SIZE, capacity, branch };
        std::fs::create_dir(path).map_err(Error::new)?;
        let created = extent_store::create_host(path, description, true, Options::default());
        match created {
            Ok(store) => Ok(Self::new(store)),
            Err(error) => {
                // Leave nothing half made behind.
                let _ = std::fs::remove_dir_all(path);
                Err(Error::new(error))
            }
        }
    }

    /// The store at `path`, or the fork whose handle `path` is ([`ExtentStore::fork`]), as of its
    /// last commit.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let store = extent_store::open_host(path.as_ref(), true, Options::default());
        Ok(Self::new(store.map_err(Error::new)?))
    }

    fn new(store: Store<HostDir>) -> Self {
        Self {
            store: Arc::new(store),
            open: Mutex::new(HashMap::new()),
            dax: Mutex::new(None),
            zeros: Default::default(),
            remote: Default::default(),
        }
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
    /// remote, the fork's branch is a fork of the store's in object storage too, made once every
    /// commit before it is uploaded, which inherits its extents rather than uploads them again
    /// (doc-7, Forks). The store must not change meanwhile.
    pub fn fork(&self, path: impl AsRef<Path>, branch: u64) -> Result<(), Error> {
        let path = path.as_ref();
        self.make_durable()?;
        extent_store::fork_host(&self.store, path, branch).map_err(Error::new)?;
        if let Some(remote) = self.remote.get() {
            let dir = self.store.dir().path();
            let forked =
                (remote.fork(branch)).and_then(|last| crate::remote::mark(dir, branch, last, &[]));
            if let Err(err) = forked {
                // Leave no fork its branch doesn't have behind.
                let _ = extent_store::delete_host(path, Options::default());
                return Err(Error::new(anyhow::anyhow!("forking branch {branch:x}: {err}")));
            }
        }
        Ok(())
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

    /// Sets the most bytes the store's extents may hold, durably: grows it, or shrinks it no lower
    /// than what its extents hold (`ENOSPC`). The guest's filesystem, which may hold more, agrees
    /// to a shrink first (the extent daemon's `RESIZE` asks it).
    pub fn set_capacity(&self, capacity: u64) -> Result<()> {
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
            // Read into the guest's page cache, which keeps it: not into the host's cache too.
            None => self.read_stored(extent, offset, &mut data)?,
        }
        let mut at = 0;
        for buffer in buffers {
            at += buffer.copy_from(&data[at..]);
        }
        Ok(())
    }

    /// Reads a sealed extent's bytes from the store, or, fxdax, from object storage, if the store
    /// holds it without its data.
    fn read_stored(&self, extent: ExtentId, offset: u64, data: &mut [u8]) -> Result<()> {
        stats::add(Counter::ReadStoredBytes, data.len() as u64);
        match self.store.read(extent.into(), offset, data) {
            Err(error) if error.downcast_ref::<extent_store::RemoteData>().is_some() => {
                let remote = self.remote.get().ok_or(Errno::EIO)?;
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
            if extents.get(&extent).is_some_and(|cached| Arc::ptr_eq(&cached.store, &self.store)) {
                extents.remove(&extent)
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
        let keep: Pages = zeros.clone();
        dax.replace_kept(kept, keep, &zeros)
    }

    /// Commits what was written and freed, and `root` as the store's root if given, durably
    /// (`COMMIT` with `COMMIT_DURABLE`). It seals the extents written before it, and those stored
    /// to through the window, whose writable mappings it maps read-only.
    pub fn commit(&self, root: Option<&[u8]>) -> Result<()> {
        self.commit_with(root, true)
    }

    /// fxdax: commits as [`ExtentStore::commit`] does, but without waiting for durability: a
    /// barrier, ordering what came before before what comes after (`COMMIT` without
    /// `COMMIT_DURABLE`). [`ExtentStore::flush`] makes the commits ordered so far durable.
    pub fn commit_ordered(&self, root: Option<&[u8]>) -> Result<()> {
        self.commit_with(root, false)
    }

    /// Makes the commits ordered so far durable.
    pub fn flush(&self) -> Result<()> {
        self.store.flush().map_err(errno)
    }

    /// Makes the commits ordered so far durable: uploaded, if the store has a remote.
    fn make_durable(&self) -> Result<()> {
        let epoch = self.store.epoch().map_err(errno)?;
        self.flush()?;
        match self.remote.get() {
            Some(remote) => remote.wait_uploaded(epoch).map_err(remote_errno),
            None => Ok(()),
        }
    }

    /// Whether commits are waiting for [`ExtentStore::flush`].
    pub fn flush_pending(&self) -> bool {
        self.store.flush_pending()
    }

    fn commit_with(&self, root: Option<&[u8]>, durable: bool) -> Result<()> {
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
        }
        self.store.commit_ordered(root).map_err(errno)?;
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
                    if let Some(cached) = extents.get_mut(extent) {
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
        if durable {
            return self.make_durable();
        }
        match self.remote.get() {
            // Within the lag the remote allows.
            Some(remote) => {
                let epoch = self.store.epoch().map_err(errno)?;
                remote.wait_lag(epoch).map_err(remote_errno)
            }
            None => Ok(()),
        }
    }

    /// Maps the `sealed` extents, open until the commit that sealed them, read-only where the
    /// window maps them, from their sealed pages, which their memory becomes.
    fn seal_mapped(&self, dax: &mut Dax<Pages>, sealed: &[(ExtentId, OpenExtent)]) -> Result<()> {
        for &(extent, ref extent_open) in sealed {
            let sealed = self.cached(extent, extent_open.len);
            // The window maps the sealed extent first, so that nothing reads its memory once its
            // pages move: a read meanwhile pages in from the store, which holds them.
            let pages = extent_open.len.next_multiple_of(page_size());
            let mut mapped = Vec::new();
            let is_kept = |pages: &Pages| keeps(pages, &extent_open.vmo);
            for &(window_offset, offset, len) in &extent_open.mappings {
                // What a read-only mapping maps past the sealed extent, zeros.
                let within = (offset + len).min(pages).max(offset) - offset;
                if within > 0 {
                    let keep: Pages = sealed.clone();
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
                    let keep: Pages = zeros.clone();
                    dax.make_read_only(past, len - within, keep, &zeros, past, is_kept)?;
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
        self.store.root().map_err(errno)
    }

    /// Creates the DAX window of `size` bytes that [`map`](Self::map) maps extents into, for a
    /// hypervisor whose `second_stage` the VMM maintains: each change to the window's mappings is
    /// made there too, and the guest's faults on the window must be passed to
    /// [`DaxWindow::fault`]. A store has one window.
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
    /// the pages mapped.
    ///
    /// # Safety
    ///
    /// The memory must be a page-aligned mapping of `size` bytes that only the guest uses, until
    /// the store is dropped.
    #[cfg(target_os = "linux")]
    pub unsafe fn create_dax_window_in_host_memory(
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
        create: impl FnOnce() -> Result<(Dax<Pages>, DaxWindow), Error>,
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
        if self.map_open(extent, offset, len, window_offset, writable)? {
            return Ok(());
        }
        let (extent_len, _) =
            self.store.extent(extent.into()).map_err(errno)?.ok_or(Errno::ENOENT)?;
        let end = offset.checked_add(len).ok_or(Errno::EINVAL)?;
        if !offset.is_multiple_of(page_size()) {
            return Err(Errno::EINVAL);
        }
        let cached = self.cached(extent, extent_len);
        let mut dax = self.dax.lock().unwrap();
        let dax = dax.as_mut().ok_or(Errno::ENOSYS)?;
        let within = end.min(extent_len.next_multiple_of(page_size())).max(offset);
        if within > offset {
            let keep: Pages = cached.clone();
            dax.map(window_offset, keep, cached.vmo(), offset, within - offset, false)?;
        }
        self.map_zeros(dax, window_offset + (within - offset), end - within)
    }

    /// Maps zeros, read-only, at `len` bytes of the window at `window_offset`, if any.
    fn map_zeros(&self, dax: &mut Dax<Pages>, window_offset: u64, len: u64) -> Result<()> {
        if len == 0 {
            return Ok(());
        }
        let zeros = self.zeros(dax)?;
        let keep: Pages = zeros.clone();
        dax.map(window_offset, keep, &zeros, window_offset, len, false)
    }

    /// The window's size of zeros, which it maps where nothing else is to be read.
    fn zeros(&self, dax: &Dax<Pages>) -> Result<Arc<zx::Vmo>> {
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
            let vmo = zx::Vmo::create(Self::MAX_EXTENT).errno()?;
            if len > 0 {
                let mut data = vec![0; len as usize];
                self.store.read(extent.into(), 0, &mut data).map_err(errno)?;
                vmo.write(&data, 0).errno()?;
            }
            let vmo = Arc::new(vmo);
            open.insert(extent, OpenExtent { vmo, len, stored: len, mappings: Vec::new() });
        }
        let extent_open = open.get_mut(&extent).unwrap();
        // Its memory past what was written is zeros, until it is written and stored to.
        if writable {
            extent_open.len = extent_open.len.max(end);
        }
        extent_open.mappings.push((window_offset, offset, len));
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
        let keep: Pages = vmo.clone();
        dax.map(window_offset, keep, vmo, offset, len, writable)?;
        Ok(true)
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
        let cache = extent_cache();
        let (cached, sealing) = {
            let mut extents = cache.extents.lock().unwrap();
            let cached = extents.entry(extent).or_insert_with(|| {
                let remote = self.remote.get().map(crate::remote::Remote::fetcher);
                let source = StoredExtent::new(self.store.clone(), extent.into(), remote);
                Cached {
                    extent: FxExtent::new(
                        cache.pager.clone(),
                        cache.allocator.clone(),
                        Box::new(source),
                        len,
                        extent.sequence,
                    ),
                    store: self.store.clone(),
                    sealing: None,
                }
            });
            let shared = !Arc::ptr_eq(&cached.store, &self.store);
            (cached.extent.clone(), cached.sealing.clone().filter(|_| shared))
        };
        // fxdax: another store's extent, a fork's parent's, whose guest may still store to its
        // pages until its seal is made: not shared with this store's guest before then.
        if let Some(sealing) = sealing {
            sealing.wait();
        }
        cached
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dax::{Access, Protection, SecondStage};
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
        store.unmap(ps, 2 * ps).unwrap();
        assert!(!window.is_mapped(ps));
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
        fn map(&self, offset: u64, _: *mut u8, _: u64, len: u64, protection: Protection) {
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
        fn map(&self, _: u64, _: *mut u8, _: u64, _: u64, _: Protection) {}
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
        block_on(async {
            window.fault(0, Access::Read).await.unwrap();
            copy_window.fault(0, Access::Read).await.unwrap();
        });

        let extent_cached = cached(extent).unwrap();
        // Each window's mapping holds the one copy of the extent's pages, which the cache keeps.
        assert_eq!(Arc::strong_count(&extent_cached), 4);
        drop(extent_cached);
        store.unmap(0, ps).unwrap();
        copy.unmap(0, ps).unwrap();
        // Unmapped, it stays cached, for the next read or mapping, until the kernel evicts its
        // pages.
        assert_eq!(Arc::strong_count(&cached(extent).unwrap()), 2);
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
        extents.get(&extent).map(|cached| cached.extent.clone())
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
            cache.allocator.clone(),
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
