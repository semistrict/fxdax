// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The process's cache of sealed extents (`crate::extents`), in pages of physical memory
//! (`crate::pages`).
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
//! An extent the default arena holds is that one copy for a tenant's store too, as every VM
//! process may read the default arena; and one a tenant's arena holds that a store reads into the
//! default arena, as a public base's extent its publisher's store had read, moves there: its pages
//! copied in memory, rather than paged in again, and evicted from the tenant's (TASK-234).

use crate::dax::{Origin, Source, Window};
use crate::error::{Errno, Result};
use crate::extents::{ExtentId, PublicReach};
use crate::pages::{ArenaId, page_size};
use crate::profile::Recorder;
use crate::stats::{self, Counter};
use extent_remote::trace;
use extent_store::shared::SharedLock;
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
    /// By the arena their pages are of (TASK-43), and their IDs. Every tenant's, which a panic
    /// holding aborts the daemon (`extent_store::shared`, TASK-149.24); as do the others here.
    extents: SharedLock<Mutex<HashMap<(ArenaId, ExtentId), Cached>>>,
    /// The arenas but the default that extents were cached of, to look for an extent's copy in,
    /// taken after `extents` (TASK-234).
    arenas: SharedLock<Mutex<HashSet<ArenaId>>>,
}

/// A sealed extent cached.
pub(crate) struct Cached {
    extent: Arc<CachedExtent>,
    /// The stores it is read through (TASK-79.7).
    holders: Arc<Holders>,
}

/// The stores a cached extent's pages are read from (TASK-79.7): each that asked for it, as forks
/// of a base share it, held weakly, so that the cache keeps none open, nor its files and disk
/// space, once it closes. A page-in reads through the first open still that holds the extent, or
/// keeps it for a follower's view: a store that frees an extent its siblings map, or closes,
/// leaves them reading it through theirs. Each holds the same data by its ID, which its branch
/// writes once (TASK-79.6).
struct Holders(SharedLock<Mutex<Vec<Holder>>>);

impl Default for Holders {
    fn default() -> Self {
        Self(SharedLock::new("cached extent holders", Mutex::default()))
    }
}

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
        let mut holders = self.0.with(Mutex::lock);
        holders.retain(|holder| holder.store.strong_count() > 0);
        if !holders.iter().any(|holder| std::ptr::eq(holder.store.as_ptr(), Arc::as_ptr(store))) {
            let (store, remote) = (Arc::downgrade(store), remote.map(Arc::downgrade));
            holders.push(Holder { store, remote });
        }
    }

    /// Adds those of `other` open still, as [`Holders::add`] does.
    fn adopt(&self, other: &Holders) {
        for (store, remote) in other.open() {
            self.add(&store, remote.as_ref());
        }
    }

    /// Those open still, in the order they asked, with their object storage.
    fn open(&self) -> Vec<OpenHolder> {
        let holders = self.0.with(Mutex::lock);
        (holders.iter())
            .filter_map(|holder| {
                Some((holder.store.upgrade()?, holder.remote.as_ref().and_then(Weak::upgrade)))
            })
            .collect()
    }

    /// Whether one is open still.
    fn any_open(&self) -> bool {
        self.0.with(Mutex::lock).iter().any(|holder| holder.store.strong_count() > 0)
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
pub(crate) const PARTS_AHEAD: u64 = 2;

static EXTENT_CACHE: OnceLock<ExtentCache> = OnceLock::new();

fn extent_cache() -> &'static ExtentCache {
    EXTENT_CACHE.get_or_init(|| {
        let pager = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(PAGER_THREADS)
            .thread_name("fxdax pager")
            .build()
            .expect("the extent cache's pager");
        ExtentCache {
            pager,
            extents: SharedLock::new("extent cache", Mutex::default()),
            arenas: SharedLock::new("extent cache arenas", Mutex::default()),
        }
    })
}

/// Evicts the pages of the cached extents of `arena`, or of every arena, those no window maps
/// first, then those used longest ago (`CachedExtent::eviction_order`), until `enough` says, of
/// the bytes evicted so far, that they are, or none is left to evict: memory an arena past its cap,
/// or the arenas past the total's, give back (TASK-50.3). Returns the bytes it evicted. Evicted
/// pages are read in again as they are touched.
///
/// fxdax: in a span of its own, `cache.relief` (`extent_remote::trace`, TASK-258).
pub(crate) fn evict_least_used(arena: Option<ArenaId>, enough: impl FnMut(u64) -> bool) -> u64 {
    let span = trace::Span::start("cache.relief");
    if let Some(arena) = arena {
        span.set_count(trace::ARENA, u64::from(arena.0));
    }
    let (evicted, extents) = evict_in_order(arena, enough);
    span.set_count(trace::BYTES, evicted);
    span.set_count(trace::EXTENTS, extents);
    evicted
}

/// Evicts as [`evict_least_used`] says: returns the bytes it evicted, and of how many extents.
fn evict_in_order(arena: Option<ArenaId>, mut enough: impl FnMut(u64) -> bool) -> (u64, u64) {
    let Some(cache) = EXTENT_CACHE.get() else { return (0, 0) };
    let candidates: Vec<Arc<CachedExtent>> = {
        let extents = cache.extents.with(Mutex::lock);
        (extents.iter())
            .filter(|((of, _), _)| arena.is_none_or(|arena| *of == arena))
            .map(|(_, cached)| cached.extent.clone())
            .collect()
    };
    // Past the cache's lock, as each takes its own; those evicted already, which stay cached
    // until freed, left out, each pass having gone through them all (TASK-92.33).
    let mut candidates: Vec<_> = (candidates.into_iter())
        .filter_map(|extent| Some((extent.eviction_order()?, extent)))
        .collect();
    candidates.sort_unstable_by_key(|(order, _)| *order);
    let (mut evicted, mut extents) = (0, 0);
    for (_, extent) in candidates {
        if enough(evicted) {
            break;
        }
        let bytes = extent.evict();
        evicted += bytes;
        extents += u64::from(bytes > 0);
    }
    (evicted, extents)
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
        let mut extents = cache.extents.with(Mutex::lock);
        cache.arenas.with(Mutex::lock).remove(&arena);
        extents.extract_if(|(of, _), _| *of == arena).map(|(_, cached)| cached).collect()
    };
    drop(evicted);
}

/// Which arena an extent's pages are of, and whether a tenant's copy of it may move to the default
/// arena: only an extent reachable from a published generation may (TASK-234), as every VM process
/// maps the default arena, and a tenant's pages there would be every tenant's to read.
pub(crate) enum Placement {
    /// Of `arena`: a tenant's, or the default for a store of none, which takes no other arena's
    /// copy.
    Arena(ArenaId),
    /// Of the default arena, reachable from a published generation, as its store found: a
    /// tenant's copy of it moves there.
    Public(PublicReach),
}

impl Placement {
    fn arena(&self) -> ArenaId {
        match self {
            Placement::Arena(arena) => *arena,
            Placement::Public(_) => ArenaId::DEFAULT,
        }
    }
}

/// The sealed `extent`, of `len` bytes, in the cache, of `placement`'s arena, cached from `store` if it isn't:
/// read from it, or from `remote`, with its page-ins recorded by `recorder`, if any; and from any
/// other store that asked for it, which `store` is one of from now on (TASK-79.7).
pub(crate) fn cached(
    store: &Arc<Store<HostDir>>,
    placement: Placement,
    remote: Option<Arc<crate::remote::Fetcher>>,
    recorder: Option<Arc<Recorder>>,
    extent: ExtentId,
    len: u64,
) -> Result<Arc<CachedExtent>> {
    let cache = extent_cache();
    let mut extents = cache.extents.with(Mutex::lock);
    let key = held_key(&extents, placement.arena(), extent);
    let mut moved = None;
    if !extents.contains_key(&key) {
        let holders = Arc::new(Holders::default());
        let stored = StoredExtent::new(holders.clone(), store, extent, recorder);
        let mut contents: Box<dyn Contents> = Box::new(stored);
        if let Placement::Public(reach) = &placement {
            // A tenant's copy, which moves here (TASK-234): of this extent alone.
            assert_eq!(reach.extent(), extent, "a tenant's copy moved for another extent");
            let arenas = cache.arenas.with(Mutex::lock);
            let other = arenas.iter().find_map(|of| extents.remove(&(*of, extent)));
            if let Some(other) = other {
                holders.adopt(&other.holders);
                moved = Some(other.extent.clone());
                let from = SharedLock::new("promoted extent", Mutex::new(Some(other.extent)));
                contents = Box::new(Promoted { from, stored: contents });
            }
        } else if key.0 != ArenaId::DEFAULT {
            cache.arenas.with(Mutex::lock).insert(key.0);
        }
        let new = Arc::new(CachedExtent::new(key.0, extent, contents, len));
        extents.insert(key, Cached { extent: new, holders });
    }
    let cached = extents.get_mut(&key).expect("cached");
    cached.extent.used();
    cached.holders.add(store, remote.as_ref());
    let found = cached.extent.clone();
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
    if let Some(from) = moved {
        // The tenant's windows map it from here from now on, and nothing of the copy: a fault on
        // it would read it in again. Past the cache's lock, which no window's is taken under.
        let (from_source, to_source): (Arc<dyn Source>, Arc<dyn Source>) =
            (from.clone(), found.clone());
        for window in from.windows() {
            window.rebind(&from_source, to_source.clone());
        }
        // What of it the tenant's arena holds, moved before the caller maps it, so that its MAP
        // maps those pages: moved as asked for, each was a fault of the guest's, and the first
        // sandbox's cold read of the Linux tree took 13.7 s, paging nothing in, 4,422 faults,
        // against the next sandbox's 5.7 s and none (GCE r991).
        for page in from.present_parts() {
            if let Err(errno) = futures::executor::block_on(found.clone().bring_in(page)) {
                log::warn!("fxdax: moving {:?}'s page {page}: {errno:?}", found.extent);
            }
        }
    }
    Ok(found)
}

/// The key `extent` of `arena` is cached by: the default arena's, if it holds it, as every VM
/// process may read it, a tenant's too: one copy (TASK-234).
fn held_key(
    extents: &HashMap<(ArenaId, ExtentId), Cached>,
    arena: ArenaId,
    extent: ExtentId,
) -> (ArenaId, ExtentId) {
    let public = (ArenaId::DEFAULT, extent);
    if arena != ArenaId::DEFAULT && extents.contains_key(&public) {
        public
    } else {
        (arena, extent)
    }
}

/// The cached `extent` of `arena`, if it is cached.
#[cfg(test)]
pub(crate) fn find(arena: ArenaId, extent: ExtentId) -> Option<Arc<CachedExtent>> {
    let cache = EXTENT_CACHE.get()?;
    let extents = cache.extents.with(Mutex::lock);
    extents.get(&(arena, extent)).map(|cached| cached.extent.clone())
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
    let (key, cached, holders) = {
        let extents = cache.extents.with(Mutex::lock);
        let key = held_key(&extents, arena, extent);
        let (cached, holders) =
            extents.get(&key).map(|cached| (cached.extent.clone(), cached.holders.clone()))?;
        (key, cached, holders)
    };
    if !holders.another_holds(store, extent.into()) {
        let mut extents = cache.extents.with(Mutex::lock);
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
    let extents = cache.extents.with(Mutex::lock);
    let idle = extents.values().filter(|cached| Arc::strong_count(&cached.extent) == 1).count();
    Entries { pinned: (extents.len() - idle) as u64, idle: idle as u64 }
}

/// The extents cached, by arena and ID.
#[cfg(test)]
pub(crate) fn keys() -> Vec<(ArenaId, ExtentId)> {
    let Some(cache) = EXTENT_CACHE.get() else { return Vec::new() };
    cache.extents.with(Mutex::lock).keys().copied().collect()
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
            ((open, cached.extent.used.load(Ordering::Relaxed)), *key)
        })
        .collect();
    idle.sort_unstable_by_key(|(order, _)| *order);
    let excess = extents.len().saturating_sub(most - most / 8);
    idle.into_iter().take(excess).filter_map(|(_, key)| extents.remove(&key)).collect()
}

/// A page-in found no pages of its arena to read into.
#[derive(Debug, thiserror::Error)]
#[error("no pages to read into")]
struct NoPages;

/// The errno a page-in that failed with `error` fails its readers with: `ENOENT` for an extent
/// the store doesn't hold, `ENOMEM` for want of pages, `EIO` for the rest.
fn read_errno(error: &anyhow::Error) -> Errno {
    if error.downcast_ref::<NoExtent>().is_some() {
        Errno::ENOENT
    } else if error.downcast_ref::<NoPages>().is_some() {
        Errno::ENOMEM
    } else {
        Errno::EIO
    }
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
    /// `extent`, read through `holders`, laid out as `store` holds it.
    fn new(
        holders: Arc<Holders>,
        store: &Store<HostDir>,
        extent: ExtentId,
        recorder: Option<Arc<Recorder>>,
    ) -> Self {
        let id: storage_device::ExtentId = extent.into();
        // Its offset in the data of the object it is in: a segment's, its own file's, or the one
        // object storage holds it in.
        let offset = match store.place(id) {
            Ok(Some(extent_store::Place::Run { offset, .. })) => offset,
            Ok(Some(extent_store::Place::Remote(location))) => location.offset,
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
                Err(ref error)
                    if let Some(data) = error.downcast_ref::<extent_store::RemoteData>()
                        && let Some(remote) = remote =>
                {
                    let fetched = remote.read(id, data.location.clone(), offset, buf.len() as u64);
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

/// An extent's contents read into the default arena from a tenant's copy of it (TASK-234): its
/// pages in memory copied, and the rest read from the stores; either way, the copy's pages go, and
/// the copy once none is left.
struct Promoted {
    from: SharedLock<Mutex<Option<Arc<CachedExtent>>>>,
    stored: Box<dyn Contents>,
}

impl Contents for Promoted {
    fn read<'a>(
        &'a self,
        offset: u64,
        buf: &'a mut [u8],
    ) -> BoxFuture<'a, std::result::Result<(), anyhow::Error>> {
        let ps = page_size();
        let pages = offset / ps..(offset + buf.len() as u64).div_ceil(ps);
        let from = self.from.with(Mutex::lock).clone();
        Box::pin(async move {
            let copied = from.as_ref().is_some_and(|from| from.copy_present(offset, buf));
            if !copied {
                self.stored.read(offset, buf).await?;
            }
            if let Some(from) = from {
                from.evict_pages(pages);
                if from.present.load(Ordering::Relaxed) == 0 {
                    self.from.with(Mutex::lock).take();
                }
            }
            Ok(())
        })
    }

    fn phase(&self) -> u64 {
        self.stored.phase()
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
    pages: SharedLock<Mutex<Pages>>,
    /// How many of its pages are in memory, which relief asks of every extent cached (TASK-92.33).
    present: AtomicUsize,
    /// When it was last asked for or faulted on, by [`CACHE_CLOCK`]: those longest ago are evicted
    /// first (TASK-50.3, TASK-92.32).
    used: AtomicU64,
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
        let pages = SharedLock::new(
            "cached extent pages",
            Mutex::new(Pages { slots, windows: Vec::new(), next: 0 }),
        );
        Self {
            arena,
            extent,
            contents,
            len,
            pages,
            present: AtomicUsize::new(0),
            used: AtomicU64::new(0),
            changed: event_listener::Event::new(),
        }
    }

    /// Notes that it is in use now: asked for, or faulted on, as a guest keeping its mapping asks
    /// for it only once (TASK-92.32).
    fn used(&self) {
        self.used.store(CACHE_CLOCK.fetch_add(1, Ordering::Relaxed) + 1, Ordering::Relaxed);
    }

    /// When relief evicts it, those first that sort first: whether a window maps it, as what
    /// guests keep mapped they may use without a sign, and an eviction takes from all of them,
    /// then when it was last used. `None` with nothing in memory to evict.
    pub(crate) fn eviction_order(&self) -> Option<(bool, u64)> {
        if self.present.load(Ordering::Relaxed) == 0 {
            return None;
        }
        let pages = self.pages.with(Mutex::lock);
        let mapped = pages.windows.iter().any(|window| window.strong_count() > 0);
        Some((mapped, self.used.load(Ordering::Relaxed)))
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
        let pages = self.pages.with(Mutex::lock);
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
            let pages = self.pages.with(Mutex::lock);
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

    /// Copies its bytes at `offset` into `data`, if its pages they are of are all in memory:
    /// whether they were (TASK-234).
    fn copy_present(&self, offset: u64, data: &mut [u8]) -> bool {
        let ps = page_size();
        let pages = self.pages.with(Mutex::lock);
        let first = offset / ps;
        let count = (data.len() as u64).div_ceil(ps);
        let frames: Option<Vec<_>> = (first..first + count)
            .map(|page| match pages.slots.get(page as usize) {
                Some(Slot::Present(frame)) => Some(frame.as_ptr()),
                _ => None,
            })
            .collect();
        let Some(frames) = frames else { return false };
        for (frame, bytes) in frames.into_iter().zip(data.chunks_mut(ps as usize)) {
            // SAFETY: The page is the extent's, in memory while its lock is held, and sealed: only
            // read.
            unsafe { std::ptr::copy_nonoverlapping(frame, bytes.as_mut_ptr(), bytes.len()) };
        }
        true
    }

    /// The windows that map it.
    fn windows(&self) -> Vec<Arc<Window>> {
        let pages = self.pages.with(Mutex::lock);
        pages.windows.iter().filter_map(Weak::upgrade).collect()
    }

    /// The first pages of its parts of which a page is in memory (TASK-234).
    fn present_parts(&self) -> Vec<u64> {
        let pages = self.pages.with(Mutex::lock);
        let count = pages.slots.len() as u64;
        let mut parts = Vec::new();
        let mut page = 0;
        while page < count {
            let part = self.part_of(page, count);
            if part.clone().any(|page| matches!(pages.slots[page as usize], Slot::Present(_))) {
                parts.push(part.start);
            }
            page = part.end;
        }
        parts
    }

    /// Evicts its pages in memory: unmaps them from the windows that map them, then lets go of
    /// them, those a window's second stage mapped into quarantine (`physmem::quarantine_with`).
    /// Returns the bytes it evicted.
    pub(crate) fn evict(&self) -> u64 {
        self.evict_pages(0..u64::MAX)
    }

    /// Evicts its pages in memory of `range`, as [`CachedExtent::evict`] evicts them all.
    fn evict_pages(&self, range: Range<u64>) -> u64 {
        let (frames, windows) = {
            let mut pages = self.pages.with(Mutex::lock);
            let mut frames = HashSet::new();
            let end = range.end.min(pages.slots.len() as u64) as usize;
            let start = (range.start as usize).min(end);
            for slot in &mut pages.slots[start..end] {
                *slot = match std::mem::replace(slot, Slot::Absent) {
                    Slot::Present(frame) => {
                        frames.insert(frame.frame());
                        self.present.fetch_sub(1, Ordering::Relaxed);
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
            let mut pages = self.pages.with(Mutex::lock);
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

    /// fxdax: the span of a page-in of its part `pages`, `cache.page_in` (`extent_remote::trace`,
    /// TASK-258): its reads, from the store or the bucket, its children.
    fn page_in_span(&self, pages: &Range<u64>) -> trace::Span {
        let span = trace::Span::start("cache.page_in");
        span.set_branch(self.extent.branch);
        span.set_count(trace::EXTENT, self.extent.sequence);
        span.set_count(trace::ARENA, u64::from(self.arena.0));
        span.set_count(trace::BYTES, (pages.end - pages.start) * page_size());
        span
    }

    /// Reads in the part `pages` of the extent, which a page-in marked filling, into pages of its
    /// arena, and puts those still filling then in its table.
    async fn read_part(self: Arc<Self>, pages: Range<u64>) {
        let ps = page_size();
        let read = self.read_pages(pages.clone()).await;
        let mut table = self.pages.with(Mutex::lock);
        match read {
            Ok(frames) => {
                for (page, frame) in pages.zip(frames) {
                    let slot = &mut table.slots[page as usize];
                    if let Slot::Filling = slot {
                        *slot = Slot::Present(frame);
                        self.present.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            Err(error) => {
                // What the read failed with, not its errno alone, which said nothing of why
                // (TASK-217).
                let errno = read_errno(&error);
                log::error!(
                    "fxdax: reading {:?} at {:#x}: {errno:?}: {error:#}",
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
    async fn read_pages(
        &self,
        pages: Range<u64>,
    ) -> std::result::Result<Vec<physmem::Page>, anyhow::Error> {
        let ps = page_size();
        let count = (pages.end - pages.start) as usize;
        let frames = physmem::alloc_run(self.arena, count, physmem::Wait::Yes)
            .map_err(|_| anyhow::Error::new(NoPages))?;
        let offset = pages.start * ps;
        let len = count * ps as usize;
        // What of it the extent holds; past it, zeros.
        let read = self.len.saturating_sub(offset).min(len as u64) as usize;
        let consecutive = frames
            .windows(2)
            .all(|pair| pair[1].as_ptr() as usize == pair[0].as_ptr() as usize + ps as usize);
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
                unsafe {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), frame.as_ptr(), bytes.len())
                };
            }
            result
        };
        result?;
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
        let table = self.pages.with(Mutex::lock);
        for page in pages {
            if let Some(Slot::Present(frame)) = table.slots.get(page as usize) {
                found(page, frame);
            }
        }
    }

    fn bring_in(self: Arc<Self>, page: u64) -> BoxFuture<'static, Result<bool>> {
        Box::pin(async move {
            self.used();
            let mut waited = false;
            loop {
                let listener = {
                    let mut table = self.pages.with(Mutex::lock);
                    match table.slots.get(page as usize) {
                        None => return Err(Errno::ENOENT),
                        Some(Slot::Present(_)) => return Ok(waited),
                        Some(Slot::Failed(errno)) if waited => return Err(*errno),
                        Some(Slot::Filling | Slot::Evicting(_)) => {}
                        Some(Slot::Absent | Slot::Failed(_)) => {
                            for part in self.parts_to_read(&mut table, page) {
                                // fxdax: a child of what faulted, or read, on the page.
                                let span = self.page_in_span(&part);
                                let reading = span.instrument(self.clone().read_part(part));
                                extent_cache().pager.spawn(reading);
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
        let count = self.pages.with(Mutex::lock).slots.len() as u64;
        self.part_of(page, count).end.saturating_sub(page).max(1)
    }

    fn mapped(&self, window: &Arc<Window>) {
        self.pages.with(Mutex::lock).windows.push(Arc::downgrade(window));
    }

    fn unmapped(&self, window: &Window) {
        let mut table = self.pages.with(Mutex::lock);
        table.windows.retain(|mapped| !std::ptr::eq(mapped.as_ptr(), window));
    }

    fn origin(&self) -> Origin {
        Origin::Extent(self.extent)
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

    fn origin(&self) -> Origin {
        Origin::Zeros
    }
}

/// Zeros, for windows to map ([`Zeros`]): made once, and kept for the process's life. `ENOMEM` if
/// the first can't be made, which each window has made as it is created (`Dax::new`): a free maps
/// them holding the store's locks, memory out, as memory run out finds them.
pub(crate) fn zeros() -> Result<Arc<dyn Source>> {
    static ZEROS: OnceLock<Arc<Zeros>> = OnceLock::new();
    if let Some(zeros) = ZEROS.get() {
        return Ok(zeros.clone());
    }
    let page = physmem::alloc(ArenaId::DEFAULT, physmem::Wait::Yes).map_err(|_| Errno::ENOMEM)?;
    // SAFETY: The page is allocated, and only this one's, never written after.
    unsafe { std::ptr::write_bytes(page.as_ptr(), 0, page_size() as usize) };
    Ok(ZEROS.get_or_init(|| Arc::new(Zeros(page))).clone())
}
