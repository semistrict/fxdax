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

use crate::dax::{Dax, DaxWindow, SecondStage};
use crate::error::{Errno, Error, IntoErrno, Result};
use crate::executor::Executor;
use crate::kernel::page_size;
use crate::memory::{VolatileSlice, VolatileSliceMut};
use extent_store::{Description, HostDir, NoSpace, Options, Store, Unaligned};
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
}

/// How many threads the cache's pager reads extents' pages on.
const PAGER_THREADS: u8 = 2;
/// The buffers the cache's pager reads into, at most `READ_AHEAD_SIZE` (128 KiB) each.
const PAGER_BUFFERS: usize = 4 << 20;

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

/// A sealed extent's contents, in a store.
struct StoredExtent {
    store: Arc<Store<HostDir>>,
    extent: storage_device::ExtentId,
}

impl ExtentSource for StoredExtent {
    fn read(&self, offset: u64, buf: &mut [u8]) -> std::result::Result<(), anyhow::Error> {
        self.store.read(self.extent, offset, buf)
    }
}

/// What a window's mapping keeps alive: an open extent's memory, or a sealed extent in the cache.
type Pages = Arc<dyn Send + Sync>;

/// Whether `pages` keeps `vmo` alive.
fn keeps(pages: &Pages, vmo: &Arc<zx::Vmo>) -> bool {
    std::ptr::addr_eq(Arc::as_ptr(pages), Arc::as_ptr(vmo))
}

/// An open extent that a window maps writably: its contents, in the kernel's memory, and how long
/// it is.
struct OpenExtent {
    vmo: Arc<zx::Vmo>,
    len: u64,
    /// The window's mappings of it: their offsets in the window and in the extent, and lengths.
    mappings: Vec<(u64, u64, u64)>,
}

/// An extent store a guest's filesystem lives on. See the module docs.
pub struct ExtentStore {
    store: Arc<Store<HostDir>>,
    /// The open extents mapped writably, until the next commit. Locked before `dax`.
    open: Mutex<HashMap<ExtentId, OpenExtent>>,
    dax: Mutex<Option<Dax<Pages>>>,
}

/// The errno of the store's error, as doc-4 has them.
fn errno(error: anyhow::Error) -> Errno {
    if error.downcast_ref::<NoExtent>().is_some() {
        Errno::ENOENT
    } else if error.downcast_ref::<SealedExtent>().is_some()
        || error.downcast_ref::<OtherBranch>().is_some()
    {
        Errno::EPERM
    } else if error.downcast_ref::<NoSpace>().is_some() {
        Errno::ENOSPC
    } else if error.downcast_ref::<Unaligned>().is_some() {
        Errno::EINVAL
    } else {
        log::error!("fxdax: extent store: {error:#}");
        Errno::EIO
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
        let path = path.as_ref();
        if capacity < Self::BLOCK_SIZE as u64 {
            return Err(Error::msg("the store is smaller than a block"));
        }
        let description = Description {
            block_size: Self::BLOCK_SIZE,
            capacity,
            branch: crate::storage::new_branch(),
        };
        std::fs::create_dir(path).map_err(Error::new)?;
        let created = HostDir::new(path, true)
            .map_err(anyhow::Error::new)
            .and_then(|dir| Store::create(dir, description, Options::default()));
        match created {
            Ok(store) => Ok(Self::new(store)),
            Err(error) => {
                // Leave nothing half made behind.
                let _ = std::fs::remove_dir_all(path);
                Err(Error::new(error))
            }
        }
    }

    /// The store at `path`, as of its last commit.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let dir = HostDir::new(path, true).map_err(Error::new)?;
        Ok(Self::new(Store::open(dir, Options::default()).map_err(Error::new)?))
    }

    fn new(store: Store<HostDir>) -> Self {
        Self { store: Arc::new(store), open: Mutex::new(HashMap::new()), dax: Mutex::new(None) }
    }

    /// The branch whose extents the guest mints.
    pub fn branch(&self) -> u64 {
        self.store.description().branch
    }

    /// What the store holds (`INFO`).
    pub fn info(&self) -> Result<ExtentInfo> {
        let info = self.store.info().map_err(errno)?;
        Ok(ExtentInfo {
            capacity: info.capacity,
            used: info.used,
            available: info.available,
            next_sequence: info.next_sequence,
        })
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
            None => self.store.read(extent.into(), offset, &mut data).map_err(errno)?,
        }
        let mut at = 0;
        for buffer in buffers {
            at += buffer.copy_from(&data[at..]);
        }
        Ok(())
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
        if self.open.lock().unwrap().remove(&extent).is_some() {
            if self.store.extent(extent.into()).map_err(errno)?.is_none() {
                return Ok(());
            }
        }
        self.store.free(extent.into()).map_err(errno)?;
        // Its pages, if read from this store: another that holds it reads them anew.
        let mut extents = extent_cache().extents.lock().unwrap();
        if extents.get(&extent).is_some_and(|cached| Arc::ptr_eq(&cached.store, &self.store)) {
            extents.remove(&extent);
        }
        Ok(())
    }

    /// Commits what was written and freed, and `root` as the store's root if given, durably
    /// (`COMMIT`). It seals the extents written before it, and those stored to through the window,
    /// whose writable mappings it maps read-only.
    pub fn commit(&self, root: Option<&[u8]>) -> Result<()> {
        let mut open = self.open.lock().unwrap();
        for (extent, extent_open) in open.iter() {
            let mut data = vec![0; extent_open.len as usize];
            extent_open.vmo.read(&mut data, 0).errno()?;
            self.store.write((*extent).into(), 0, &data).map_err(errno)?;
        }
        self.store.commit(root).map_err(errno)?;
        let mut dax = self.dax.lock().unwrap();
        for (extent, extent_open) in open.drain() {
            let sealed = self.cached(extent, extent_open.len);
            // The window maps the sealed extent first, so that nothing reads its memory once its
            // pages move: a read meanwhile pages in from the store, which holds them.
            if let Some(dax) = dax.as_mut() {
                let is_kept = |pages: &Pages| keeps(pages, &extent_open.vmo);
                for (window_offset, offset, len) in extent_open.mappings {
                    let keep: Pages = sealed.clone();
                    dax.make_read_only(window_offset, len, keep, sealed.vmo(), offset, is_kept)?;
                }
            }
            // Its memory is the sealed extent's pages, but for those paged in meanwhile.
            let pages = extent_open.len.next_multiple_of(page_size());
            sealed.supply_pages(0..pages, &extent_open.vmo, 0);
        }
        Ok(())
    }

    /// The root as of the last commit, empty if none set one (`ROOT`).
    pub fn root(&self) -> Result<Vec<u8>> {
        self.store.root().map_err(errno)
    }

    /// Creates the DAX window of `size` bytes that [`map`](Self::map) maps extents into, for a
    /// hypervisor whose `second_stage` the VMM maintains, as
    /// [`Filesystem::create_dax_window`](crate::Filesystem::create_dax_window) does. A store has
    /// one window.
    pub fn create_dax_window(
        &self,
        size: u64,
        second_stage: Option<Arc<dyn SecondStage>>,
    ) -> Result<DaxWindow, Error> {
        self.install_dax(|| Ok(Dax::new(size, second_stage)?))
    }

    /// Creates the DAX window in the `size` bytes of host memory at `base`, for a hypervisor that
    /// follows host memory, as
    /// [`Filesystem::create_dax_window_in_host_memory`](crate::Filesystem::create_dax_window_in_host_memory)
    /// does.
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
                crate::dax::uffd::UffdSecondStage::new(base.as_ptr() as usize, size, prefault)
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
    /// Unless `writable`, the range must be within the extent, but for the padding of its last
    /// page, which reads as zeros: of a sealed extent, its cached pages; of an open one, its memory
    /// as it is stored to and written, until the commit that seals it maps the sealed pages there.
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
        self.dax.lock().unwrap().as_ref().ok_or(Errno::ENOSYS)?.check_range(window_offset, len)?;
        if self.map_open(extent, offset, len, window_offset, writable)? {
            return Ok(());
        }
        let (extent_len, _) =
            self.store.extent(extent.into()).map_err(errno)?.ok_or(Errno::ENOENT)?;
        let pages = extent_len.next_multiple_of(page_size());
        let end = offset.checked_add(len).ok_or(Errno::EINVAL)?;
        if !offset.is_multiple_of(page_size()) || end > pages {
            return Err(Errno::EINVAL);
        }
        let cached = self.cached(extent, extent_len);
        let mut dax = self.dax.lock().unwrap();
        let dax = dax.as_mut().ok_or(Errno::ENOSYS)?;
        let keep: Pages = cached.clone();
        dax.map(window_offset, keep, cached.vmo(), offset, len, false)
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
            open.insert(extent, OpenExtent { vmo: Arc::new(vmo), len, mappings: Vec::new() });
        }
        let extent_open = open.get_mut(&extent).unwrap();
        if writable {
            extent_open.len = extent_open.len.max(end);
        } else if end > extent_open.len.next_multiple_of(page_size()) {
            return Err(Errno::EINVAL);
        }
        extent_open.mappings.push((window_offset, offset, len));
        let mut dax = self.dax.lock().unwrap();
        let dax = dax.as_mut().ok_or(Errno::ENOSYS)?;
        let vmo = &extent_open.vmo;
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
        let mut extents = cache.extents.lock().unwrap();
        let cached = extents.entry(extent).or_insert_with(|| {
            let source = StoredExtent { store: self.store.clone(), extent: extent.into() };
            Cached {
                extent: FxExtent::new(
                    cache.pager.clone(),
                    cache.allocator.clone(),
                    Box::new(source),
                    len,
                    extent.sequence,
                ),
                store: self.store.clone(),
            }
        });
        cached.extent.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dax::Access;
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
        let window = store.create_dax_window(4 * ps, None).unwrap();
        store.commit(None).unwrap();
        store.map(extent, 0, 2 * ps, ps, false).unwrap();
        assert_eq!(store.map(extent, 0, 3 * ps, 0, false), Err(Errno::EINVAL), "past the extent");
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
        store.unmap(ps, 2 * ps).unwrap();
        assert!(!window.is_mapped(ps));
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

    /// Windows mapping the same extent, from a store and a copy of it, map the same pages.
    #[test]
    fn windows_share_extents_pages() {
        let (store, dir) = store(1 << 20);
        let ps = page_size();
        let extent = id(&store, 1);
        write(&store, extent, 0, &pattern(6, ps as usize)).unwrap();
        store.commit(None).unwrap();
        let copy_path = dir.path().join("copy");
        extent_store::copy_store(
            &dir.path().join("store"),
            &copy_path,
            store.branch() + 1,
            |file, to| std::io::copy(&mut &*file, &mut std::fs::File::create_new(to)?).map(|_| ()),
        )
        .unwrap();
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
        // Not past what was written, nor of one never written.
        assert_eq!(store.map(written, ps, ps, 3 * ps, false), Err(Errno::EINVAL));
        assert_eq!(store.map(id(&store, 3), 0, ps, 3 * ps, false), Err(Errno::ENOENT));
        block_on(async {
            for page in 0..3 {
                let access = if page == 1 { Access::Write } else { Access::Read };
                window.fault(page * ps, access).await.unwrap();
            }
        });
        // SAFETY: The test faulted the pages in, and only it accesses them.
        let bytes = unsafe { std::slice::from_raw_parts_mut(window.host_base(), 3 * ps as usize) };
        bytes[ps as usize..2 * ps as usize].copy_from_slice(&pattern(2, ps as usize));
        assert!(bytes[..ps as usize] == pattern(1, ps as usize)[..], "the written data");
        assert!(bytes[2 * ps as usize..] == pattern(2, ps as usize)[..], "the stored data");
        assert!(block_on(window.fault(0, Access::Write)).is_err(), "the mapping is writable");

        store.commit(None).unwrap();
        block_on(async {
            for page in [0, 2] {
                window.fault(page * ps, Access::Read).await.unwrap();
            }
        });
        // SAFETY: As above.
        let bytes = unsafe { std::slice::from_raw_parts(window.host_base(), 3 * ps as usize) };
        assert!(bytes[..ps as usize] == pattern(1, ps as usize)[..], "the sealed written data");
        assert!(bytes[2 * ps as usize..] == pattern(2, ps as usize)[..], "the sealed stored data");
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
        // Another window mapping the sealed extent, of a copy of the store, maps the same pages.
        let copy_path = dir.path().join("copy");
        extent_store::copy_store(
            &dir.path().join("store"),
            &copy_path,
            store.branch() + 1,
            |file, to| std::io::copy(&mut &*file, &mut std::fs::File::create_new(to)?).map(|_| ()),
        )
        .unwrap();
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
}
