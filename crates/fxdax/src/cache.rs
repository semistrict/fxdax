// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The process's cache of sealed extents, and the memory of open ones (`crate::extents`), in pages
//! of physical memory (`crate::pages`).
//!
//! A sealed extent is cached once, keyed by its ID and the arena its pages are of, which every store
//! and window in the process shares: a table of its pages ([`CachedExtent`]), each read in from a
//! store that holds the extent as it is first read or mapped, a part (`extent_frame::PART`) at a
//! time, on the pager's threads, and evicted, its extents asked for longest ago first, as memory
//! runs out (`physmem::relieve_with`) or the daemon is asked to give memory back. Windows map its
//! pages read-only: one copy of each page, whichever store or fork maps it, as an extent never
//! changes once sealed, its ID its branch's to write once, which the store keeps to (TASK-79.6).
//! A page evicted is unmapped from each window that maps it first; freed, it waits in quarantine
//! for the VM processes that may reach it still (`physmem::quarantine_with`, TASK-79.9).
//!
//! An open extent a window maps writably is pages of its store's arena ([`OpenPages`]), zeros as
//! they come in, which its commit writes to the store, then moves into the sealed extent's table,
//! uncopied ([`CachedExtent::supply`]).

use crate::dax::{Deferred, Source, Window};
use crate::error::{Errno, Result};
use crate::extents::ExtentId;
use crate::pages::{ArenaId, page_size};
use crate::profile::Recorder;
use crate::stats::{self, Counter};
use extent_store::{HostDir, Store};
use futures::future::BoxFuture;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use storage_device::NoExtent;

/// The process's cache of sealed extents (see the module docs): its pager's threads, which read
/// their pages from the stores, and the extents, by arena and ID.
struct ExtentCache {
    /// The threads the pager reads extents' parts on.
    pager: tokio::runtime::Runtime,
    /// By the arena their pages are of (TASK-43), and their IDs.
    extents: Mutex<HashMap<(ArenaId, ExtentId), Cached>>,
}

/// A sealed extent cached.
pub(crate) struct Cached {
    extent: Arc<CachedExtent>,
    /// The stores it is read through (TASK-79.7).
    holders: Arc<Holders>,
    /// What waits for the guest that sealed it to lose its access to its pages, if the sealing
    /// commit didn't (`ExtentStore::commit`): another store's guest, a fork's, waits for it before
    /// it maps them.
    sealing: Option<Sealing>,
    /// When it was last asked for, by [`CACHE_CLOCK`]: those longest ago are evicted first
    /// (TASK-50.3).
    used: AtomicU64,
}

/// A seal of a cached extent not yet made: its store's guest may store to its pages until it is.
struct Sealing {
    store: Weak<Store<HostDir>>,
    made: Arc<dyn Deferred>,
}

/// The stores a cached extent's pages are read from (TASK-79.7): each that asked for it, as forks
/// of a base share it, held weakly, so that the cache keeps none open, nor its files and disk
/// space, once it closes. A page-in reads through the first open still that holds the extent, or
/// keeps it for a follower's view: a store that frees an extent its siblings map, or closes,
/// leaves them reading it through theirs. Each holds the same data by its ID, which its branch
/// writes once (TASK-79.6).
#[derive(Default)]
struct Holders(Mutex<Vec<Holder>>);

/// A store a cached extent's pages are read from, and its object storage, which holds them if the
/// store holds them without their data.
struct Holder {
    store: Weak<Store<HostDir>>,
    remote: Option<Weak<crate::remote::Fetcher>>,
}

/// A [`Holder`] open still.
type OpenHolder = (Arc<Store<HostDir>>, Option<Arc<crate::remote::Fetcher>>);

impl Holders {
    /// Adds `store`, reading through `remote`, unless it is there, letting go of those closed.
    fn add(&self, store: &Arc<Store<HostDir>>, remote: Option<&Arc<crate::remote::Fetcher>>) {
        let mut holders = self.0.lock().unwrap();
        holders.retain(|holder| holder.store.strong_count() > 0);
        if !holders.iter().any(|holder| std::ptr::eq(holder.store.as_ptr(), Arc::as_ptr(store))) {
            let (store, remote) = (Arc::downgrade(store), remote.map(Arc::downgrade));
            holders.push(Holder { store, remote });
        }
    }

    /// Those open still, in the order they asked, with their object storage.
    fn open(&self) -> Vec<OpenHolder> {
        let holders = self.0.lock().unwrap();
        (holders.iter())
            .filter_map(|holder| {
                Some((holder.store.upgrade()?, holder.remote.as_ref().and_then(Weak::upgrade)))
            })
            .collect()
    }

    /// Whether one is open still.
    fn any_open(&self) -> bool {
        self.0.lock().unwrap().iter().any(|holder| holder.store.strong_count() > 0)
    }

    /// Whether one open still, but `store`, holds `extent`.
    fn another_holds(&self, store: &Arc<Store<HostDir>>, extent: storage_device::ExtentId) -> bool {
        (self.open().into_iter())
            .filter(|(holder, _)| !Arc::ptr_eq(holder, store))
            .any(|(holder, _)| holder.extent(extent).is_ok_and(|held| held.is_some()))
    }
}

/// The cache's clock, which counts the extents asked for.
static CACHE_CLOCK: AtomicU64 = AtomicU64::new(0);

/// The most extents the cache keeps (TASK-104): each its table of pages, and the stores it reads
/// through, which an extent nothing maps and whose pages were evicted keeps still. Under pgbench,
/// small extents by the hundred thousand, the tables of extents never asked for again would be
/// most of what the cache holds. Past this, those asked for longest ago that nothing else holds (no
/// window maps them) go, down to seven eighths of it; one asked for again is read anew from its
/// store. Tests lower it.
pub(crate) static MOST_CACHED: AtomicUsize = AtomicUsize::new(1 << 16);

/// The number of extents past which the cache next lets go of those over its bound: the bound, or,
/// if its last pass couldn't get below it, as the windows' mappings hold the extents they map, an
/// eighth of the bound more than it left (TASK-109.2). Each lookup past the bound sorted every
/// extent cached, under the cache's lock, and let go of none: an 8 GiB window, mapping more than
/// the bound, never full, so never reclaiming, made each MAP 3-6 ms.
static TRIM_AT: AtomicUsize = AtomicUsize::new(0);

/// How many threads the cache's pager reads extents' pages on: a read of the store, and the
/// decompression of a part, keep one busy, while page-ins of other extents, and of what a fork's
/// boot profile names, wait for theirs.
pub(crate) const PAGER_THREADS: usize = 6;

/// How many parts after those a reader reading on asks for are read ahead of its requests.
const PARTS_AHEAD: u64 = 2;

static EXTENT_CACHE: OnceLock<ExtentCache> = OnceLock::new();

fn extent_cache() -> &'static ExtentCache {
    EXTENT_CACHE.get_or_init(|| {
        let pager = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(PAGER_THREADS)
            .thread_name("fxdax pager")
            .build()
            .expect("the extent cache's pager");
        ExtentCache { pager, extents: Mutex::new(HashMap::new()) }
    })
}

/// Evicts the pages of the cached extents of `arena`, or of every arena, those asked for longest
/// ago first, until `enough` says, of the bytes evicted so far, that they are, or none is left to
/// evict: memory an arena past its cap, or the arenas past the total's, give back (TASK-50.3).
/// Returns the bytes it evicted. Evicted pages are read in again as they are touched.
pub(crate) fn evict_least_used(arena: Option<ArenaId>, mut enough: impl FnMut(u64) -> bool) -> u64 {
    let Some(cache) = EXTENT_CACHE.get() else { return 0 };
    let mut candidates: Vec<(u64, Arc<CachedExtent>)> = {
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
        evicted += extent.evict();
    }
    evicted
}

/// Gives, once, the relief of arenas under pressure (TASK-50.3) its evictor, which frees pages of
/// cached extents (`physmem::relieve_with`): before a store of any arena opens, whose allocations
/// may wait for it.
pub(crate) fn relieve_pressure() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let relieved = physmem::relieve_with(|arena, bytes| {
            evict_least_used(arena, |evicted| evicted >= bytes)
        });
        // Without its thread, an allocation that finds its arena out of memory fails.
        if let Err(err) = relieved {
            log::error!("fxdax: starting the thread that frees capped memory: {err}");
        }
    });
}

/// Lets go of the extents cached in `arena`: a tenant's, which nothing holds now
/// (`crate::pages::Arena`, TASK-43.4). Their pages are freed as the last holders of the extents,
/// page-ins in flight, let them go.
pub(crate) fn evict_arena(arena: ArenaId) {
    let Some(cache) = EXTENT_CACHE.get() else { return };
    let evicted: Vec<Cached> = {
        let mut extents = cache.extents.lock().unwrap();
        extents.extract_if(|(of, _), _| *of == arena).map(|(_, cached)| cached).collect()
    };
    drop(evicted);
}

/// The sealed `extent`, of `len` bytes, in the cache, of `arena`, cached from `store` if it isn't:
/// read from it, or from `remote`, with its page-ins recorded by `recorder`, if any; and from any
/// other store that asked for it, which `store` is one of from now on (TASK-79.7).
pub(crate) fn cached(
    store: &Arc<Store<HostDir>>,
    arena: ArenaId,
    remote: Option<Arc<crate::remote::Fetcher>>,
    recorder: Option<Arc<Recorder>>,
    extent: ExtentId,
    len: u64,
) -> Result<Arc<CachedExtent>> {
    let cache = extent_cache();
    let (cached, sealing) = {
        let mut extents = cache.extents.lock().unwrap();
        let key = (arena, extent);
        let cached = extents.entry(key).or_insert_with(|| {
            let holders = Arc::new(Holders::default());
            let stored =
                StoredExtent::new(holders.clone(), store, remote.as_deref(), extent, recorder);
            Cached {
                extent: Arc::new(CachedExtent::new(arena, extent, Box::new(stored), len)),
                holders,
                sealing: None,
                used: AtomicU64::new(0),
            }
        });
        cached.used.store(CACHE_CLOCK.fetch_add(1, Ordering::Relaxed) + 1, Ordering::Relaxed);
        cached.holders.add(store, remote.as_ref());
        let sealing = (cached.sealing.as_ref())
            .filter(|sealing| !std::ptr::eq(sealing.store.as_ptr(), Arc::as_ptr(store)))
            .map(|sealing| sealing.made.clone());
        let found = (cached.extent.clone(), sealing);
        let most = MOST_CACHED.load(Ordering::Relaxed);
        let gone = (extents.len() > most.max(TRIM_AT.load(Ordering::Relaxed))).then(|| {
            let started = std::time::Instant::now();
            let gone = let_go_least_used(&mut extents, most);
            TRIM_AT.store(extents.len() + most / 8, Ordering::Relaxed);
            stats::count_since(Counter::CacheTrims, Counter::CacheTrimNs, started);
            gone
        });
        drop(extents);
        // Freed past the cache's lock.
        drop(gone);
        found
    };
    // Another store's extent, a fork's parent's, whose guest may still store to its pages until its
    // seal is made: not shared with this store's guest before then.
    if let Some(sealing) = sealing {
        sealing.wait();
    }
    Ok(cached)
}

/// The cached `extent` of `arena`, if it is cached.
#[cfg(test)]
pub(crate) fn find(arena: ArenaId, extent: ExtentId) -> Option<Arc<CachedExtent>> {
    let cache = EXTENT_CACHE.get()?;
    let extents = cache.extents.lock().unwrap();
    extents.get(&(arena, extent)).map(|cached| cached.extent.clone())
}

/// Records, of each cached extent of `sealed` by `arena`, that `store`'s guest may store to its
/// pages until `made` (`ExtentStore::commit`).
pub(crate) fn sealing(
    store: &Arc<Store<HostDir>>,
    sealed: impl Iterator<Item = (ArenaId, ExtentId)>,
    made: &Arc<dyn Deferred>,
) {
    let mut extents = extent_cache().extents.lock().unwrap();
    for key in sealed {
        if let Some(cached) = extents.get_mut(&key) {
            cached.sealing = Some(Sealing { store: Arc::downgrade(store), made: made.clone() });
        }
    }
}

/// Lets go of the cached `extent` of `arena`, which `store` frees, unless another store that asked
/// for it holds it still, which reads its pages through itself (TASK-79.7): one that holds it reads
/// them anew. Returns what keeps its pages, which the window's mappings of it map zeros in place
/// of, if it was cached.
pub(crate) fn freed(
    store: &Arc<Store<HostDir>>,
    arena: ArenaId,
    extent: ExtentId,
) -> Option<Arc<CachedExtent>> {
    let cache = EXTENT_CACHE.get()?;
    let key = (arena, extent);
    let (cached, holders) = {
        let extents = cache.extents.lock().unwrap();
        extents.get(&key).map(|cached| (cached.extent.clone(), cached.holders.clone()))?
    };
    if !holders.another_holds(store, extent.into()) {
        let mut extents = cache.extents.lock().unwrap();
        let same = extents.get(&key).is_some_and(|now| Arc::ptr_eq(&now.extent, &cached));
        let removed = same.then(|| extents.remove(&key));
        drop(extents);
        drop(removed);
    }
    Some(cached)
}

/// The extents cached: how many something but the cache holds (`pinned`), as a window's mapping, a
/// read or a page-in does, which it keeps however many it holds, and how many nothing else does
/// (`idle`), which it lets go of past its bound (TASK-104), the extent daemon's gauges
/// (`crate::extentd::metrics`, TASK-131.6).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Entries {
    pub pinned: u64,
    pub idle: u64,
}

/// The extents cached, as of now: a pass over them, under the cache's lock.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn entries() -> Entries {
    let Some(cache) = EXTENT_CACHE.get() else { return Entries::default() };
    let extents = cache.extents.lock().unwrap();
    let idle = extents.values().filter(|cached| Arc::strong_count(&cached.extent) == 1).count();
    Entries { pinned: (extents.len() - idle) as u64, idle: idle as u64 }
}

/// The extents cached, by arena and ID.
#[cfg(test)]
pub(crate) fn keys() -> Vec<(ArenaId, ExtentId)> {
    let Some(cache) = EXTENT_CACHE.get() else { return Vec::new() };
    cache.extents.lock().unwrap().keys().copied().collect()
}

/// Takes out of the cache `extents` those asked for longest ago that nothing else holds, down to
/// seven eighths of `most`, and returns them, for the caller to let go of past the cache's lock
/// (TASK-104).
fn let_go_least_used(
    extents: &mut HashMap<(ArenaId, ExtentId), Cached>,
    most: usize,
) -> Vec<Cached> {
    // Those no store open reads first, which none asks for but to cache them anew.
    let mut idle: Vec<((bool, u64), (ArenaId, ExtentId))> = (extents.iter())
        .filter(|(_, cached)| Arc::strong_count(&cached.extent) == 1)
        .map(|(key, cached)| {
            let open = cached.holders.any_open();
            ((open, cached.used.load(Ordering::Relaxed)), *key)
        })
        .collect();
    idle.sort_unstable_by_key(|(order, _)| *order);
    let excess = extents.len().saturating_sub(most - most / 8);
    idle.into_iter().take(excess).filter_map(|(_, key)| extents.remove(&key)).collect()
}

/// What a cached extent's pages are read from: its contents, in parts.
pub(crate) trait Contents: Send + Sync {
    /// Reads its bytes at `offset` into `buf`: a future, which a read that waits, as a fetch from
    /// object storage does, holds no thread while it waits on.
    fn read<'a>(
        &'a self,
        offset: u64,
        buf: &'a mut [u8],
    ) -> BoxFuture<'a, std::result::Result<(), anyhow::Error>>;

    /// Where, in the extent, its parts of `extent_frame::PART` bytes begin: an offset within the
    /// first, in whole pages. A part is decompressed whole, so reading less of it costs nearly as
    /// much.
    fn phase(&self) -> u64 {
        0
    }
}

/// A sealed extent's contents, in the stores that hold it, or in object storage, if one holds it
/// without its data.
struct StoredExtent {
    /// The stores it is read through (TASK-79.7).
    holders: Arc<Holders>,
    extent: ExtentId,
    /// Where the parts of the object its data is in begin in it: its offset in the object, in
    /// whole pages, which page-ins are.
    phase: u64,
    /// What records its page-ins for its base's boot profile, if anything does.
    recorder: Option<Arc<Recorder>>,
}

impl StoredExtent {
    /// `extent`, read through `holders`, laid out as `store`, reading through `remote`, holds it.
    fn new(
        holders: Arc<Holders>,
        store: &Store<HostDir>,
        remote: Option<&crate::remote::Fetcher>,
        extent: ExtentId,
        recorder: Option<Arc<Recorder>>,
    ) -> Self {
        let id: storage_device::ExtentId = extent.into();
        // Its offset in the data of the object it is in: a segment's, its own file's, or the one
        // object storage holds it in.
        let offset = match store.place(id) {
            Ok(Some(extent_store::Place::Run { offset, .. })) => offset,
            Ok(Some(extent_store::Place::Remote)) => {
                remote.and_then(|remote| remote.offset(id)).unwrap_or(0)
            }
            _ => 0,
        };
        let phase = offset % extent_frame::PART / page_size() * page_size();
        Self { holders, extent, phase, recorder }
    }

}

impl Contents for StoredExtent {
    /// A read of the local store, which doesn't wait for long, on the pager's thread; or a fetch,
    /// which the page-in waits for holding no thread.
    fn read<'a>(
        &'a self,
        offset: u64,
        buf: &'a mut [u8],
    ) -> BoxFuture<'a, std::result::Result<(), anyhow::Error>> {
        stats::add(Counter::PageIns, 1);
        if let Some(recorder) = &self.recorder {
            recorder.record(self.extent, offset, buf.len() as u64);
        }
        let id: storage_device::ExtentId = self.extent.into();
        // Through the first of the stores that asked for it that holds it still (TASK-79.7).
        let mut failed = anyhow::Error::new(NoExtent(id));
        for (store, remote) in self.holders.open() {
            // An extent freed since it was mapped, which a follower's view keeps, reads as kept
            // (doc-8).
            let read = match store.read(id, offset, buf) {
                Err(error) if error.downcast_ref::<NoExtent>().is_some() => {
                    store.read_kept(id, offset, buf)
                }
                read => read,
            };
            match read {
                Ok(()) => {
                    stats::add(Counter::PageInBytes, buf.len() as u64);
                    return Box::pin(std::future::ready(Ok(())));
                }
                Err(error)
                    if error.downcast_ref::<extent_store::RemoteData>().is_some()
                        && let Some(remote) = remote =>
                {
                    let fetched = remote.read(id, offset, buf.len() as u64);
                    return Box::pin(async move {
                        let data = fetched.await?;
                        stats::add(Counter::FetchedBytes, data.len() as u64);
                        buf[..data.len()].copy_from_slice(&data);
                        buf[data.len()..].fill(0);
                        Ok(())
                    });
                }
                Err(error) => failed = error,
            }
        }
        Box::pin(std::future::ready(Err(failed)))
    }

    fn phase(&self) -> u64 {
        self.phase
    }
}

/// A page of a cached extent.
enum Slot {
    /// Not in memory.
    Absent,
    /// Being read in, which [`CachedExtent::changed`] tells of the end of.
    Filling,
    /// In memory.
    Present(physmem::Page),
    /// Being evicted: unmapped from the windows that map it, then let go of.
    Evicting(physmem::Page),
    /// Its read in failed, as the next waiting for it are told; asked for again, it is read in
    /// anew.
    Failed(Errno),
}

/// A cached extent's pages, and the windows that map them, under its lock.
struct Pages {
    slots: Vec<Slot>,
    windows: Vec<Weak<Window>>,
    /// The page after the last part a page-in asked for: where a reader reading on asks next.
    next: u64,
}

/// A sealed extent in the cache: a table of its pages. See the module docs.
pub(crate) struct CachedExtent {
    arena: ArenaId,
    extent: ExtentId,
    contents: Box<dyn Contents>,
    len: u64,
    pages: Mutex<Pages>,
    /// Notified as pages come in, fail to, or go.
    changed: event_listener::Event,
}

impl CachedExtent {
    /// `extent`, of `len` bytes, read from `contents` into pages of `arena`.
    pub(crate) fn new(
        arena: ArenaId,
        extent: ExtentId,
        contents: Box<dyn Contents>,
        len: u64,
    ) -> Self {
        let count = len.div_ceil(page_size()) as usize;
        let slots = std::iter::repeat_with(|| Slot::Absent).take(count).collect();
        // A reader from its start reads on.
        let pages = Mutex::new(Pages { slots, windows: Vec::new(), next: 0 });
        Self { arena, extent, contents, len, pages, changed: event_listener::Event::new() }
    }

    /// The part `page` is in, of its `pages`, as page numbers: a part of an object
    /// (`extent_frame::PART`), which a sealed extent's data is stored and fetched in.
    fn part_of(&self, page: u64, pages: u64) -> Range<u64> {
        let (ps, part, phase) = (page_size(), extent_frame::PART, self.contents.phase());
        let index = (page * ps + phase) / part;
        let start = (index * part).saturating_sub(phase) / ps;
        let end = ((index + 1) * part - phase).div_ceil(ps).min(pages);
        start..end
    }

    /// How many of its pages are in memory.
    #[cfg(test)]
    pub(crate) fn held(&self) -> u64 {
        let pages = self.pages.lock().unwrap();
        pages.slots.iter().filter(|slot| matches!(slot, Slot::Present(_))).count() as u64
    }

    /// Reads its bytes at `offset` into `data`, reading in its pages that aren't in memory, as a
    /// read of the extent that maps nothing does: blocks the calling thread while they are read
    /// in. Past its end, zeros.
    pub(crate) fn read(self: &Arc<Self>, offset: u64, data: &mut [u8]) -> Result<()> {
        let ps = page_size();
        let held = self.len.saturating_sub(offset).min(data.len() as u64) as usize;
        let (bytes, zeros) = data.split_at_mut(held);
        zeros.fill(0);
        let mut done = 0;
        while done < bytes.len() {
            let at = offset + done as u64;
            let (page, within) = (at / ps, (at % ps) as usize);
            let len = (ps as usize - within).min(bytes.len() - done);
            futures::executor::block_on(self.clone().bring_in(page))?;
            let pages = self.pages.lock().unwrap();
            // Evicted meanwhile: read in again.
            let Some(Slot::Present(frame)) = pages.slots.get(page as usize) else { continue };
            // SAFETY: The page is the extent's, in memory while its lock is held, and sealed: only
            // read.
            let memory = unsafe { std::slice::from_raw_parts(frame.as_ptr(), ps as usize) };
            bytes[done..done + len].copy_from_slice(&memory[within..within + len]);
            done += len;
        }
        Ok(())
    }

    /// Moves the pages of `open` in `pages` that it doesn't have in memory, its own as the commit
    /// that sealed it wrote them, into its table, uncopied: the commit's (`ExtentStore::commit`).
    /// Those of another arena's than its own stay, read in from the store as they are asked for,
    /// so that no arena's memory holds another's data.
    pub(crate) fn supply(&self, pages: Range<u64>, open: &OpenPages) {
        if open.arena != self.arena {
            return;
        }
        let moved = open.take(pages);
        let mut table = self.pages.lock().unwrap();
        for (page, frame) in moved {
            if let Some(slot @ (Slot::Absent | Slot::Failed(_))) = table.slots.get_mut(page as usize)
            {
                *slot = Slot::Present(frame);
            }
        }
        drop(table);
        self.changed.notify(usize::MAX);
    }

    /// Evicts its pages in memory: unmaps them from the windows that map them, then lets go of
    /// them, those a window's second stage mapped into quarantine (`physmem::quarantine_with`).
    /// Returns the bytes it evicted.
    pub(crate) fn evict(&self) -> u64 {
        let (frames, windows) = {
            let mut pages = self.pages.lock().unwrap();
            let mut frames = HashSet::new();
            for slot in &mut pages.slots {
                *slot = match std::mem::replace(slot, Slot::Absent) {
                    Slot::Present(frame) => {
                        frames.insert(frame.frame());
                        Slot::Evicting(frame)
                    }
                    other => other,
                };
            }
            pages.windows.retain(|window| window.strong_count() > 0);
            (frames, pages.windows.iter().filter_map(Weak::upgrade).collect::<Vec<_>>())
        };
        if frames.is_empty() {
            return 0;
        }
        let identity = self as *const Self as *const () as usize;
        for window in windows {
            window.unmap_pages_of(identity, &|frame| frames.contains(&frame));
        }
        let evicted: Vec<physmem::Page> = {
            let mut pages = self.pages.lock().unwrap();
            (pages.slots.iter_mut())
                .filter_map(|slot| match std::mem::replace(slot, Slot::Absent) {
                    Slot::Evicting(frame) => Some(frame),
                    other => {
                        *slot = other;
                        None
                    }
                })
                .collect()
        };
        self.changed.notify(usize::MAX);
        let bytes = evicted.len() as u64 * page_size();
        drop(evicted);
        bytes
    }

    /// Reads in the part `pages` of the extent, which a page-in marked filling, into pages of its
    /// arena, and puts those still filling then in its table.
    async fn read_part(self: Arc<Self>, pages: Range<u64>) {
        let ps = page_size();
        let read = self.read_pages(pages.clone()).await;
        let mut table = self.pages.lock().unwrap();
        match read {
            Ok(frames) => {
                for (page, frame) in pages.zip(frames) {
                    let slot = &mut table.slots[page as usize];
                    if let Slot::Filling = slot {
                        *slot = Slot::Present(frame);
                    }
                }
            }
            Err(errno) => {
                log::error!(
                    "fxdax: reading {:?} at {:#x}: {errno:?}",
                    self.extent,
                    pages.start * ps
                );
                for page in pages {
                    let slot = &mut table.slots[page as usize];
                    if let Slot::Filling = slot {
                        *slot = Slot::Failed(errno);
                    }
                }
            }
        }
        drop(table);
        self.changed.notify(usize::MAX);
    }

    /// The pages `pages` of the extent, read in: pages of its arena, consecutive where they can be
    /// (`physmem::alloc_run`), so that a window maps them in as few runs as can be (TASK-108.1),
    /// and read into in place if they are, else copied into.
    async fn read_pages(&self, pages: Range<u64>) -> Result<Vec<physmem::Page>> {
        let ps = page_size();
        let count = (pages.end - pages.start) as usize;
        let frames = physmem::alloc_run(self.arena, count, physmem::Wait::Yes)
            .map_err(|_| Errno::ENOMEM)?;
        let offset = pages.start * ps;
        let len = count * ps as usize;
        // What of it the extent holds; past it, zeros.
        let read = self.len.saturating_sub(offset).min(len as u64) as usize;
        let consecutive = frames.windows(2).all(|pair| {
            pair[1].as_ptr() as usize == pair[0].as_ptr() as usize + ps as usize
        });
        let result = if consecutive {
            // SAFETY: The pages are consecutive in this process's memory, and allocated, and only
            // this read's until it puts them in the table.
            let memory = unsafe { std::slice::from_raw_parts_mut(frames[0].as_ptr(), len) };
            let (data, past) = memory.split_at_mut(read);
            past.fill(0);
            self.contents.read(offset, data).await
        } else {
            let mut data = vec![0; len];
            let result = self.contents.read(offset, &mut data[..read]).await;
            for (frame, bytes) in frames.iter().zip(data.chunks(ps as usize)) {
                // SAFETY: As above, a page at a time.
                unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), frame.as_ptr(), bytes.len()) };
            }
            result
        };
        result.map_err(|error| {
            if error.downcast_ref::<NoExtent>().is_some() { Errno::ENOENT } else { Errno::EIO }
        })?;
        Ok(frames)
    }

    /// Marks the parts of `page`, and those after it a reader reading on asks for next, filling,
    /// as far as they aren't in memory or being read in, and returns them, to read in; under its
    /// lock.
    fn parts_to_read(&self, table: &mut Pages, page: u64) -> Vec<Range<u64>> {
        let count = table.slots.len() as u64;
        let mut parts = Vec::new();
        let part = self.part_of(page, count);
        let sequential = part.start == table.next;
        let mut next = part.clone();
        parts.push(part);
        if sequential {
            for _ in 0..PARTS_AHEAD {
                if next.end >= count {
                    break;
                }
                next = self.part_of(next.end, count);
                parts.push(next.clone());
            }
        }
        table.next = parts.last().map_or(u64::MAX, |part| part.end);
        (parts.into_iter())
            .filter_map(|part| {
                let mut wanted = false;
                for page in part.clone() {
                    let slot = &mut table.slots[page as usize];
                    if matches!(slot, Slot::Absent | Slot::Failed(_)) {
                        *slot = Slot::Filling;
                        wanted = true;
                    }
                }
                wanted.then_some(part)
            })
            .collect()
    }
}

impl Source for CachedExtent {
    fn each_present(&self, pages: Range<u64>, found: &mut dyn FnMut(u64, &physmem::Page)) {
        let table = self.pages.lock().unwrap();
        for page in pages {
            if let Some(Slot::Present(frame)) = table.slots.get(page as usize) {
                found(page, frame);
            }
        }
    }

    fn bring_in(self: Arc<Self>, page: u64) -> BoxFuture<'static, Result<bool>> {
        Box::pin(async move {
            let mut waited = false;
            loop {
                let listener = {
                    let mut table = self.pages.lock().unwrap();
                    match table.slots.get(page as usize) {
                        None => return Err(Errno::ENOENT),
                        Some(Slot::Present(_)) => return Ok(waited),
                        Some(Slot::Failed(errno)) if waited => return Err(*errno),
                        Some(Slot::Filling | Slot::Evicting(_)) => {}
                        Some(Slot::Absent | Slot::Failed(_)) => {
                            for part in self.parts_to_read(&mut table, page) {
                                extent_cache().pager.spawn(self.clone().read_part(part));
                            }
                        }
                    }
                    // Listened for under the lock, which what changes the page takes.
                    self.changed.listen()
                };
                listener.await;
                waited = true;
            }
        })
    }

    /// The rest of its part, which a page-in reads whole.
    fn brought_in_with(&self, page: u64) -> u64 {
        let count = self.pages.lock().unwrap().slots.len() as u64;
        self.part_of(page, count).end.saturating_sub(page).max(1)
    }

    fn mapped(&self, window: &Arc<Window>) {
        self.pages.lock().unwrap().windows.push(Arc::downgrade(window));
    }

    fn unmapped(&self, window: &Window) {
        let mut table = self.pages.lock().unwrap();
        table.windows.retain(|mapped| !std::ptr::eq(mapped.as_ptr(), window));
    }
}

/// The memory of an open extent a window maps writably: pages of its store's arena, zeros as they
/// come in, until the commit that seals it moves them into its cached extent. Never evicted: its
/// commit writes them to the store.
pub(crate) struct OpenPages {
    arena: ArenaId,
    pages: Mutex<Vec<Option<physmem::Page>>>,
}

impl OpenPages {
    /// No pages yet, of `arena`.
    pub(crate) fn new(arena: ArenaId) -> Arc<Self> {
        Arc::new(Self { arena, pages: Mutex::new(Vec::new()) })
    }

    /// `count` new pages of zeros, of its arena, at most a chunk's: consecutive in physical memory
    /// where they can be, after `after` where it can, the page before them in the extent, and else
    /// of the smallest free run that holds them (`physmem::alloc_run`), so that a window maps the
    /// extent in runs as long as they are, each one change of its VM process's memory, rather than
    /// a page at a time (TASK-108.1, TASK-131.1). Taken a page at a time, they would be the single
    /// pages free among those in use, scattered. `ENOMEM` once relief finds nothing to evict for
    /// them.
    fn zeroed(&self, after: Option<&physmem::Page>, count: usize) -> Result<Vec<physmem::Page>> {
        debug_assert!(count <= physmem::CHUNK_SIZE / page_size() as usize);
        let mut run: Vec<physmem::Page> = Vec::with_capacity(count);
        while run.len() < count {
            let Some(next) = run.last().or(after).and_then(physmem::alloc_after) else { break };
            run.push(next);
        }
        let rest = physmem::alloc_run(self.arena, count - run.len(), physmem::Wait::Yes)
            .map_err(|_| Errno::ENOMEM)?;
        run.extend(rest);
        let ps = page_size() as usize;
        let breaks = (run.windows(2))
            .filter(|pair| pair[1].as_ptr() as usize != pair[0].as_ptr() as usize + ps);
        stats::add(Counter::OpenAllocs, 1);
        stats::add(Counter::OpenAllocPages, run.len() as u64);
        stats::add(Counter::OpenAllocRuns, 1 + breaks.count() as u64);
        for page in &run {
            // SAFETY: The page is allocated, and only this extent's.
            unsafe { std::ptr::write_bytes(page.as_ptr(), 0, page_size() as usize) };
        }
        Ok(run)
    }

    /// Brings `pages` into memory, as a guest about to write them maps them: so that they are
    /// mapped now rather than one fault at a time as it writes them. Each run of them not in
    /// memory is allocated a chunk's pages at most at once ([`OpenPages::zeroed`]): those brought
    /// in before an allocation fails stay, as its arena's cap refuses the rest.
    pub(crate) fn commit(&self, pages: Range<u64>) -> Result<()> {
        let mut table = self.pages.lock().unwrap();
        if table.len() < pages.end as usize {
            table.resize_with(pages.end as usize, || None);
        }
        let mut page = pages.start as usize;
        while page < pages.end as usize {
            if table[page].is_some() {
                page += 1;
                continue;
            }
            let chunk = physmem::CHUNK_SIZE / page_size() as usize;
            let end = (page..pages.end as usize).find(|at| table[*at].is_some());
            let end = end.unwrap_or(pages.end as usize).min(page + chunk);
            let before = page.checked_sub(1).and_then(|before| table[before].as_ref());
            let run = self.zeroed(before, end - page)?;
            for (slot, frame) in table[page..end].iter_mut().zip(run) {
                *slot = Some(frame);
            }
            page = end;
        }
        Ok(())
    }

    /// Reads its bytes at `offset` into `data`: zeros where it has no page.
    pub(crate) fn read(&self, offset: u64, data: &mut [u8]) {
        let ps = page_size() as usize;
        let table = self.pages.lock().unwrap();
        let mut done = 0;
        while done < data.len() {
            let at = offset as usize + done;
            let (page, within) = (at / ps, at % ps);
            let len = (ps - within).min(data.len() - done);
            match table.get(page).and_then(Option::as_ref) {
                Some(frame) => {
                    // SAFETY: The page is the extent's, in memory while its lock is held; a guest
                    // storing to it meanwhile changes what is read, as it would its own memory.
                    let memory = unsafe { std::slice::from_raw_parts(frame.as_ptr(), ps) };
                    data[done..done + len].copy_from_slice(&memory[within..within + len]);
                }
                None => data[done..done + len].fill(0),
            }
            done += len;
        }
    }

    /// Writes `data` at `offset`, bringing the pages it writes into memory.
    pub(crate) fn write(&self, offset: u64, data: &[u8]) -> Result<()> {
        let ps = page_size() as usize;
        let end = offset + data.len() as u64;
        self.commit(offset / ps as u64..end.div_ceil(ps as u64))?;
        let table = self.pages.lock().unwrap();
        let mut done = 0;
        while done < data.len() {
            let at = offset as usize + done;
            let (page, within) = (at / ps, at % ps);
            let len = (ps - within).min(data.len() - done);
            let frame = table[page].as_ref().expect("a page brought in");
            // SAFETY: As in `read`, written.
            let memory = unsafe { std::slice::from_raw_parts_mut(frame.as_ptr(), ps) };
            memory[within..within + len].copy_from_slice(&data[done..done + len]);
            done += len;
        }
        Ok(())
    }

    /// Takes its pages of `pages`, as they move to its sealed extent.
    fn take(&self, pages: Range<u64>) -> Vec<(u64, physmem::Page)> {
        let mut table = self.pages.lock().unwrap();
        let held = table.len() as u64;
        (pages.filter(|page| *page < held))
            .filter_map(|page| Some((page, table[page as usize].take()?)))
            .collect()
    }
}

impl Source for OpenPages {
    fn each_present(&self, pages: Range<u64>, found: &mut dyn FnMut(u64, &physmem::Page)) {
        let table = self.pages.lock().unwrap();
        for page in pages {
            if let Some(Some(frame)) = table.get(page as usize) {
                found(page, frame);
            }
        }
    }

    fn bring_in(self: Arc<Self>, page: u64) -> BoxFuture<'static, Result<bool>> {
        Box::pin(std::future::ready(self.commit(page..page + 1).map(|()| false)))
    }

    fn is_open(&self) -> bool {
        true
    }
}

/// Zeros, which windows map read-only where nothing else is to be read: one page of them, of the
/// default arena, which every VM process maps, wherever they are mapped.
struct Zeros(physmem::Page);

impl Source for Zeros {
    fn each_present(&self, pages: Range<u64>, found: &mut dyn FnMut(u64, &physmem::Page)) {
        for page in pages {
            found(page, &self.0);
        }
    }

    fn bring_in(self: Arc<Self>, _page: u64) -> BoxFuture<'static, Result<bool>> {
        Box::pin(std::future::ready(Ok(false)))
    }
}

/// Zeros, for windows to map ([`Zeros`]): made once, and kept for the process's life.
pub(crate) fn zeros() -> Arc<dyn Source> {
    static ZEROS: OnceLock<Arc<Zeros>> = OnceLock::new();
    ZEROS
        .get_or_init(|| {
            let page = physmem::alloc(ArenaId::DEFAULT, physmem::Wait::Yes)
                .expect("a page of zeros for DAX windows");
            // SAFETY: The page is allocated, and only this one's, never written after.
            unsafe { std::ptr::write_bytes(page.as_ptr(), 0, page_size() as usize) };
            Arc::new(Zeros(page))
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The physical memory offsets of `open`'s pages, in order.
    fn offsets(open: &OpenPages) -> Vec<u64> {
        let table = open.pages.lock().unwrap();
        table
            .iter()
            .map(|page| page.as_ref().expect("a page brought in").location().offset)
            .collect()
    }

    /// An open extent's pages brought in are consecutive in physical memory, each run after the
    /// page before it in the extent, though single pages are free among pages in use, which pages
    /// taken a page at a time would be (TASK-131.1). Each test runs in a process of its own
    /// (nextest), whose arena is its own.
    #[test]
    fn open_pages_are_brought_in_in_runs() {
        let ps = page_size();
        let row = physmem::alloc_run(ArenaId::DEFAULT, 64, physmem::Wait::No).unwrap();
        let (freed, kept): (Vec<_>, Vec<_>) =
            row.into_iter().enumerate().partition(|(at, _)| at % 2 == 0);
        drop(freed);
        let open = OpenPages::new(ArenaId::DEFAULT);
        open.commit(0..8).unwrap();
        open.commit(8..12).unwrap();
        open.write(12 * ps, &vec![1; 4 * ps as usize]).unwrap();
        let offsets = offsets(&open);
        let first = offsets[0];
        assert_eq!(offsets, (0..16).map(|page| first + page * ps).collect::<Vec<_>>());
        drop(kept);
    }
}
