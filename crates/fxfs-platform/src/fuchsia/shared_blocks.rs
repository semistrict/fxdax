// fxdax: not in Fuchsia. The pages of the blocks that frozen owners hold (see Fxfs's
// `Allocator::snapshot`), which the files of every volume reading them share.

//! A frozen owner's data on the device never changes while it is frozen, so files whose pages are
//! that data, such as a file in a volume and the same file in its snapshots, can share one copy of
//! each page. A cache VMO per frozen owner holds the pages by device offset, and files' VMOs are
//! supplied with them as shared pages (`zx_pager_supply_shared_pages`): a file writing to one gets
//! a copy of its own, and the kernel takes the page back from the files sharing it when it evicts it
//! from the cache. Nothing faults on or maps the caches; pages enter them only when files read the
//! blocks. A cache is dropped when its owner is no longer frozen, before its blocks can be reused.

use anyhow::Error;
use fuchsia_sync::Mutex;
use fxfs::log::*;
use fxfs::object_store::FrozenPage;
use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::sync::Arc;
use storage_units::page_size;

pub struct SharedBlocks {
    /// The pager of the caches, which supplies their pages.
    pager: zx::Pager,
    /// The port of the caches' page requests, which nothing waits on as nothing faults on them.
    port: zx::Port,
    block_size: u64,
    /// The size of each cache, which has a page for each block of the device.
    cache_size: u64,
    /// The caches, by frozen owner.
    caches: Mutex<HashMap<u64, Arc<zx::Vmo>>>,
}

impl SharedBlocks {
    /// Creates the caches of a device of `device_size` bytes in blocks of `block_size`.
    pub fn new(device_size: u64, block_size: u64) -> Result<Self, Error> {
        Ok(Self {
            pager: zx::Pager::create(zx::PagerOptions::empty())?,
            port: zx::Port::create(),
            block_size,
            cache_size: device_size / block_size * page_size().get(),
            caches: Mutex::new(HashMap::new()),
        })
    }

    fn cache(&self, owner_object_id: u64) -> Result<Arc<zx::Vmo>, zx::Status> {
        let mut caches = self.caches.lock();
        if let Some(cache) = caches.get(&owner_object_id) {
            return Ok(cache.clone());
        }
        let cache = Arc::new(self.pager.create_vmo(
            zx::VmoOptions::empty(),
            &self.port,
            owner_object_id,
            self.cache_size,
        )?);
        caches.insert(owner_object_id, cache.clone());
        Ok(cache)
    }

    /// Returns where in its cache the page whose content starts at `device_offset` is. Each block
    /// has a page, which also holds the blocks after it when pages are larger than blocks.
    fn cache_range(&self, device_offset: u64) -> Range<u64> {
        let start = device_offset / self.block_size * page_size().get();
        start..start + page_size().get()
    }

    /// Supplies the page `page` of `vmo`, a file's VMO that `pager` backs, with a page shared with
    /// every other file whose page this is, its content being in `content` at `content_offset`.
    /// Returns whether the page was already cached. `NOT_FOUND` means the page was evicted from the
    /// cache before it could be shared, and is the caller's to supply.
    pub fn supply(
        &self,
        pager: &crate::fuchsia::pager::Pager,
        vmo: &zx::Vmo,
        page: FrozenPage,
        content: &zx::Vmo,
        content_offset: u64,
    ) -> Result<bool, zx::Status> {
        let cache = self.cache(page.owner_object_id)?;
        // TASK-26.3 phase A: every extent is the device's one, so the offset within it is the
        // device offset. Phase D keys the caches by extent.
        let cache_range = self.cache_range(page.extent_offset);
        let range = page.offset..page.offset + page_size().get();
        match pager.supply_shared_pages(vmo, range.clone(), &cache, cache_range.start) {
            Err(zx::Status::NOT_FOUND) => {}
            result => return result.map(|()| true),
        }
        // Cache a copy of the page, unless another file did meanwhile, whose copy is then shared.
        // Supplying takes the pages it supplies, which `content` still needs.
        let mut copy = vec![0; page_size().get() as usize];
        content.read(&mut copy, content_offset)?;
        let aux = zx::Vmo::create(page_size().get())?;
        aux.write(&copy, 0)?;
        self.pager.supply_pages(&cache, cache_range.clone(), &aux, 0)?;
        pager.supply_shared_pages(vmo, range, &cache, cache_range.start).map(|()| false)
    }

    /// Drops the caches of the owners that are no longer `frozen_owners`, whose blocks may be
    /// reused. Their pages are taken back from the files sharing them.
    pub fn retain(&self, frozen_owners: &BTreeMap<u64, u64>) {
        let dropped: Vec<_> = {
            let mut caches = self.caches.lock();
            let owners: Vec<u64> =
                caches.keys().filter(|owner| !frozen_owners.contains_key(owner)).copied().collect();
            owners.into_iter().filter_map(|owner| caches.remove(&owner)).collect()
        };
        if !dropped.is_empty() {
            info!(count = dropped.len(); "Dropping the caches of owners no longer frozen");
        }
    }
}
