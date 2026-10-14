//! An extent store for a guest's own filesystem: what a VMM's virtio extent-store device serves
//! (backlog doc-4), when the guest runs Fxfs itself rather than fxdax serving it over virtio-fs.
//!
//! An [`ExtentStore`] is an image (`crates/extent_store`) whose extents the guest writes, reads and
//! frees by ID. Its I/O copies between the image's files and guest memory, and blocks the calling
//! thread: a device's queue threads call it.
//!
//! Sealed extents map into a DAX window read-only, from a cache of their contents in the in-process
//! kernel's memory, keyed by extent ID, which every window in the process shares: windows that map
//! the same extent map the same pages, whichever store or fork they map it from, as an extent never
//! changes once sealed. An extent's pages stay cached while a window maps them.

use crate::dax::{Dax, DaxWindow, SecondStage};
use crate::error::{Errno, Error, IntoErrno, Result};
use crate::kernel::page_size;
use crate::memory::{VolatileSlice, VolatileSliceMut};
use extent_store::{Description, HostDir, NoSpace, Options, Store, Unaligned};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, Weak};
use storage_device::{NoExtent, OtherBranch, SealedExtent};

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

/// An extent's contents, in the kernel's memory, padded with zeros to whole pages.
struct Cached {
    vmo: zx::Vmo,
}

/// The extents cached, by ID, while windows map them.
static CACHE: Mutex<Option<HashMap<ExtentId, Weak<Cached>>>> = Mutex::new(None);

/// An extent store a guest's filesystem lives on. See the module docs.
pub struct ExtentStore {
    store: Store<HostDir>,
    dax: Mutex<Option<Dax<Arc<Cached>>>>,
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
        Self { store, dax: Mutex::new(None) }
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
        self.store.read(extent.into(), offset, &mut data).map_err(errno)?;
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
        self.store.write(extent.into(), offset, &data).map_err(errno)
    }

    /// Frees `extent`, if the store holds it (`FREE`).
    pub fn free(&self, extent: ExtentId) -> Result<()> {
        self.store.free(extent.into()).map_err(errno)
    }

    /// Commits what was written and freed, and `root` as the store's root if given, durably
    /// (`COMMIT`). It seals the extents written before it.
    pub fn commit(&self, root: Option<&[u8]>) -> Result<()> {
        self.store.commit(root).map_err(errno)
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
        create: impl FnOnce() -> Result<(Dax<Arc<Cached>>, DaxWindow), Error>,
    ) -> Result<DaxWindow, Error> {
        let mut dax = self.dax.lock().unwrap();
        if dax.is_some() {
            return Err(Errno::EEXIST.into());
        }
        let (new, window) = create()?;
        *dax = Some(new);
        Ok(window)
    }

    /// Maps `len` bytes at `offset` in `extent`, which must be sealed, into the DAX window at
    /// `window_offset`, read-only, replacing what was mapped there (`MAP`). Offsets and length must
    /// be multiples of the page size, and the range within the extent, but for the padding of its
    /// last page, which reads as zeros. Without a window, fails with `ENOSYS`.
    pub fn map(&self, extent: ExtentId, offset: u64, len: u64, window_offset: u64) -> Result<()> {
        self.dax.lock().unwrap().as_ref().ok_or(Errno::ENOSYS)?.check_range(window_offset, len)?;
        let (extent_len, open) =
            self.store.extent(extent.into()).map_err(errno)?.ok_or(Errno::ENOENT)?;
        let pages = extent_len.next_multiple_of(page_size());
        let end = offset.checked_add(len).ok_or(Errno::EINVAL)?;
        if open || !offset.is_multiple_of(page_size()) || end > pages {
            return Err(Errno::EINVAL);
        }
        let cached = self.cached(extent, extent_len, pages)?;
        let mut dax = self.dax.lock().unwrap();
        let dax = dax.as_mut().ok_or(Errno::ENOSYS)?;
        let vmo = &cached.vmo;
        // The mapping keeps the extent's pages cached.
        dax.map(window_offset, cached.clone(), vmo, offset, len, false)
    }

    /// Unmaps `len` bytes of the DAX window at `window_offset` (`UNMAP`).
    pub fn unmap(&self, window_offset: u64, len: u64) -> Result<()> {
        let mut dax = self.dax.lock().unwrap();
        let dax = dax.as_mut().ok_or(Errno::ENOSYS)?;
        dax.check_range(window_offset, len)?;
        dax.unmap(window_offset, len)
    }

    /// `extent`'s contents, `len` bytes padded to `pages`, cached in the kernel's memory, read in
    /// if no window maps them.
    fn cached(&self, extent: ExtentId, len: u64, pages: u64) -> Result<Arc<Cached>> {
        let mut cache = CACHE.lock().unwrap();
        let cache = cache.get_or_insert_with(HashMap::new);
        if let Some(cached) = cache.get(&extent).and_then(Weak::upgrade) {
            return Ok(cached);
        }
        let mut data = vec![0; len as usize];
        self.store.read(extent.into(), 0, &mut data).map_err(errno)?;
        let vmo = zx::Vmo::create(pages).errno()?;
        vmo.write(&data, 0).errno()?;
        let cached = Arc::new(Cached { vmo });
        cache.retain(|_, cached| cached.strong_count() > 0);
        cache.insert(extent, Arc::downgrade(&cached));
        Ok(cached)
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
        assert_eq!(store.map(extent, 0, ps, 0), Err(Errno::ENOSYS));
        let window = store.create_dax_window(4 * ps, None).unwrap();
        // Not while it is open.
        assert_eq!(store.map(extent, 0, ps, 0), Err(Errno::EINVAL));
        store.commit(None).unwrap();
        store.map(extent, 0, 2 * ps, ps).unwrap();
        assert_eq!(store.map(extent, 0, 3 * ps, 0), Err(Errno::EINVAL), "past the extent");
        assert_eq!(store.map(id(&store, 9), 0, ps, 0), Err(Errno::ENOENT));

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
        store.map(extent, 0, ps, 0).unwrap();
        copy.map(extent, 0, ps, 0).unwrap();
        block_on(async {
            window.fault(0, Access::Read).await.unwrap();
            copy_window.fault(0, Access::Read).await.unwrap();
        });

        let cached = CACHE.lock().unwrap().as_ref().unwrap()[&extent].upgrade().unwrap();
        // Each window's mapping holds the one copy of the extent's pages.
        assert_eq!(Arc::strong_count(&cached), 3);
        drop(cached);
        store.unmap(0, ps).unwrap();
        copy.unmap(0, ps).unwrap();
        assert!(CACHE.lock().unwrap().as_ref().unwrap()[&extent].upgrade().is_none());
    }
}
