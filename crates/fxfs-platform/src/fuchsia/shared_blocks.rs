// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// fxdax: not in Fuchsia. The pages of the blocks that frozen owners hold (see Fxfs's
// `Allocator::snapshot`), which the files of every volume reading them share.

//! A frozen owner's data on the device never changes while it is frozen, so files whose pages are
//! that data, such as a file in a volume and the same file in its snapshots, can share one copy of
//! each page. A cache VMO per frozen owner holds the pages, each of a block of one of the owner's
//! extents (backlog doc-4), at a slot the cache gives the block when a file first reads it. Files'
//! VMOs are supplied with them as shared pages (`zx_pager_supply_shared_pages`): a file writing to
//! one gets a copy of its own, and the kernel takes the page back from the files sharing it when it
//! evicts it from the cache. Nothing faults on or maps the caches; pages enter them only when files
//! read the blocks. A cache is dropped when its owner is no longer frozen, before its extents can be
//! freed.

use anyhow::Error;
use fuchsia_sync::Mutex;
use fxfs::log::*;
use fxfs::object_store::FrozenPage;
use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::sync::Arc;
use storage_device::ExtentId;
use storage_units::page_size;

pub struct SharedBlocks {
    /// The pager of the caches, which supplies their pages.
    pager: zx::Pager,
    /// The port of the caches' page requests, which nothing waits on as nothing faults on them.
    port: zx::Port,
    block_size: u64,
    /// The size of each cache, which has a page for as many blocks as the device holds.
    cache_size: u64,
    /// The caches, by frozen owner.
    caches: Mutex<HashMap<u64, Cache>>,
}

/// A frozen owner's cache.
struct Cache {
    vmo: Arc<zx::Vmo>,
    /// The page each block cached has, by its extent and its offset in the extent, in blocks.
    pages: HashMap<(ExtentId, u64), u64>,
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

    /// Returns `page`'s owner's cache, and where in it the page whose content starts at the
    /// page's offset in its extent is. Each block has a page, which also holds the blocks after it
    /// when pages are larger than blocks.
    fn cache_range(&self, page: &FrozenPage) -> Result<(Arc<zx::Vmo>, Range<u64>), zx::Status> {
        let mut caches = self.caches.lock();
        let cache = match caches.entry(page.owner_object_id) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => entry.insert(Cache {
                vmo: Arc::new(self.pager.create_vmo(
                    zx::VmoOptions::empty(),
                    &self.port,
                    page.owner_object_id,
                    self.cache_size,
                )?),
                pages: HashMap::new(),
            }),
        };
        let slots = self.cache_size / page_size().get();
        let next = cache.pages.len() as u64;
        let slot =
            *cache.pages.entry((page.extent, page.extent_offset / self.block_size)).or_insert(next);
        if slot >= slots {
            // The cache is full: the page isn't shared, and the caller supplies it.
            cache.pages.remove(&(page.extent, page.extent_offset / self.block_size));
            return Err(zx::Status::NOT_FOUND);
        }
        let start = slot * page_size().get();
        Ok((cache.vmo.clone(), start..start + page_size().get()))
    }

    /// Supplies the page `page` of `vmo`, a file's VMO that `pager` backs, with a page shared with
    /// every other file whose page this is, its content being in `content` at `content_offset`.
    /// Returns whether the page was already cached. `NOT_FOUND` means the page was evicted from the
    /// cache before it could be shared, or the cache is full, and is the caller's to supply.
    pub fn supply(
        &self,
        pager: &crate::fuchsia::pager::Pager,
        vmo: &zx::Vmo,
        page: FrozenPage,
        content: &zx::Vmo,
        content_offset: u64,
    ) -> Result<bool, zx::Status> {
        let (cache, cache_range) = self.cache_range(&page)?;
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

    /// Drops the caches of the owners that are no longer `frozen_owners`, whose extents may be
    /// freed. Their pages are taken back from the files sharing them.
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
