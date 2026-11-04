// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! DAX windows: guest physical memory into which an extent-store device maps extents, so that the
//! guest's Fxfs reads their pages directly instead of copying them through requests.
//!
//! A window is a range of guest physical memory, whose mappings each map a range of a [`Source`]'s
//! pages (`crate::cache`), read-only: a sealed extent's cached pages, shared by every window that
//! maps it, which are read in from the store as they are first needed; or zeros. The guest's loads
//! fault pages in. The window keeps the pages it has mapped, each a page of physical memory
//! (`crate::pages`), as page tables do; a source evicting a page unmaps it from each window that
//! maps it first.
//!
//! The window maps its pages in its [`SecondStage`], if it has one: its translation outside the
//! daemon. A VM's window is the extent daemon's (`crate::extentd`), whose second stage is its
//! connection to the VM process, which makes each change in the VM's memory. KVM maps guest memory
//! from the host memory backing it, and follows changes to it: the VM process's own stage
//! (`dax::uffd`) takes that memory over, maps the window's pages into it, and reports the faults
//! KVM takes there itself, with userfaultfd, to the daemon, which passes them to the window
//! ([`DaxWindow::fault`]). A window without a second stage, as tests' are, has its pages mapped at
//! its host address.
//!
//! fxdax: a writer's window copies on write (backlog doc-13, [`CopyOnWrite`]): a store to a page,
//! a sealed extent's or zeros, gives the page a private copy holding what the page held, mapped
//! writable at the same offset; the mapping stays the source's. Only the guest's `MAP` or `UNMAP`
//! over a private page ends it: a source evicting its pages and a free mapping zeros leave it. A
//! window holds a bound of them, reserved against the tenant's memory as it is made, past which a
//! store is refused, as every store is to a window that doesn't copy, a follower's. The private
//! pages are the second stage's memory ([`SecondStage::make_private`]): the VM process's own, laid
//! out as the window is, so that they cost it no mapping each (`dax::uffd`); the window keeps where
//! they are and how many.

#[cfg(all(test, feature = "shuttle"))]
mod shuttle_tests;
#[cfg(target_os = "linux")]
pub(crate) mod uffd;

use crate::error::{Errno, Result};
use crate::pages::{PhysLocation, page_size};
use futures::future::BoxFuture;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(feature = "shuttle")]
use shuttle::sync::Mutex;
#[cfg(not(feature = "shuttle"))]
use std::sync::Mutex;

/// The access a guest's fault on a window was for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Access {
    /// A load, or an instruction fetch.
    Read,
    /// A store.
    Write,
}

/// A window's second stage: a translation of it outside the daemon, which follows its mappings,
/// such as the extent daemon's connection to a VM process, which makes them in the VM's memory.
///
/// Each change the window makes to its pages is made here, rather than in this process's memory,
/// with offsets into the window. Calls for a window are serialized. They cannot fail: if the
/// hypervisor refuses one, the guest's memory no longer matches the window's, and the
/// implementation must panic.
pub trait SecondStage: Send + Sync {
    /// Maps the `len` bytes at `offset`, where nothing is mapped, read-only, to the pages at
    /// `phys` of physical memory, consecutive in one arena's object
    /// ([`arena_memory`](crate::pages::arena_memory)), from which another process can map them:
    /// a store to them must fault.
    fn map(&self, offset: u64, phys: PhysLocation, len: u64);

    /// Unmaps the mapped `len` bytes at `offset`.
    fn unmap(&self, offset: u64, len: u64);

    /// fxdax: the window maps the `len` bytes at `offset` anew; called once it has, before any of
    /// the pages it maps there now are mapped here, as the window maps a page here only once it is
    /// in memory. A stage that withholds pages a fault found nothing at (`dax::uffd`) gives them
    /// back, so that accesses to them fault again.
    fn window_mapped(&self, _offset: u64, _len: u64) {}

    /// fxdax: the window begins a change of its mapping of a range, which may unmap what it maps
    /// there, then map the range anew ([`window_mapped`](Self::window_mapped)) and its pages in
    /// memory: what this thread makes here until [`end_change`](Self::end_change) is one change,
    /// which a stage in another process, sent it whole (`crate::extentd`), may make at once
    /// ([`remap`](Self::remap), TASK-91.16). Changes may nest.
    fn begin_change(&self) {}

    /// fxdax: ends [`begin_change`](Self::begin_change).
    fn end_change(&self) {}

    /// fxdax: unmaps the mapped `len` bytes at `offset`, then makes `anew`, each within them, in
    /// order: as [`unmap`](Self::unmap), then [`window_mapped`](Self::window_mapped) or
    /// [`map`](Self::map) for each, would one after another, the window's change of a range it
    /// mapped, which a stage may make at once rather than in steps (TASK-91.16).
    fn remap(&self, offset: u64, len: u64, anew: &[Anew]) {
        self.unmap(offset, len);
        for anew in anew {
            match *anew {
                Anew::Mapped { offset, len } => self.window_mapped(offset, len),
                Anew::Map { offset, phys, len } => self.map(offset, phys, len),
            }
        }
    }

    /// fxdax: makes the page at `offset` private (backlog doc-13), in place of what is mapped
    /// there, if anything: a page of the stage's own memory, holding the bytes of the page at
    /// `from`, the source's, which stays in memory until the change is made, or zeros, mapped
    /// writable: the only memory of a window a store changes. An unmap or a map over it ends it.
    /// Only a window that copies on write makes one, and only a stage of such a window need know
    /// how.
    fn make_private(&self, offset: u64, from: Option<PhysLocation>) {
        unimplemented!("{offset:#x} made private ({from:?}) in a stage that doesn't copy");
    }
}

/// fxdax: what a window's change makes of a range it unmapped ([`SecondStage::remap`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anew {
    /// The window maps the range anew ([`SecondStage::window_mapped`]).
    Mapped { offset: u64, len: u64 },
    /// It maps the pages there ([`SecondStage::map`]).
    Map { offset: u64, phys: PhysLocation, len: u64 },
}

/// A hypervisor that maps a window's host memory for the guest itself, such as KVM, which the
/// window tells of the pages it maps there, so that it can map them for the guest ahead of the
/// guest's accesses rather than on a fault on each page, as KVM's `KVM_PRE_FAULT_MEMORY` does.
#[cfg(target_os = "linux")]
pub trait Prefault: Send + Sync {
    /// Maps the `len` bytes at `offset` into the window for the guest, which the window has mapped
    /// in its host memory: those the guest asks for to read next (TASK-112.1), and a private page
    /// as it is made (fxdax: the rest are left to the guest's accesses, TASK-109). It is called
    /// with the window's mappings locked, and must not wait on the window, or for the guest.
    fn prefault(&self, offset: u64, len: u64);
}

/// What resolves the faults on a window's second stage in host memory: the VM process's connection
/// to the extent daemon, which holds the window (`crate::extentd`), or the window itself, in tests
/// whose window is in this process's host memory (`Dax::in_host_memory`).
#[cfg(target_os = "linux")]
pub(crate) trait Faults: Send + Sync {
    /// The window's size in bytes.
    fn size(&self) -> u64;

    /// As [`DaxWindow::fault`], for a store to a page the stage has, which only the guest makes.
    fn fault(&self, offset: u64, access: Access) -> BoxFuture<'_, Result<()>>;

    /// As [`DaxWindow::fault`], for a fault on a page the stage hasn't, whose access may not be
    /// the guest's: what the daemon counts apart from the guest's stores (`Stats::fault_stores`).
    fn fault_page(&self, offset: u64, access: Access) -> BoxFuture<'_, Result<()>>;

    /// fxdax: maps the page at `offset`, to read, ahead of the guest's accesses, as
    /// [`fault_page`](Self::fault_page) does a load's: a probe of the stage's own, of a page the
    /// guest may map nothing at, which fails then as quietly as it may (`ENOENT`), as no access of
    /// the guest's was refused.
    fn probe(&self, offset: u64) -> BoxFuture<'_, Result<()>>;
}

/// A test's window in this process's host memory resolves the faults on it itself.
#[cfg(all(test, target_os = "linux"))]
impl Faults for Window {
    fn size(&self) -> u64 {
        Window::size(self)
    }

    fn fault(&self, offset: u64, access: Access) -> BoxFuture<'_, Result<()>> {
        Box::pin(Window::fault(self, offset, access))
    }

    fn fault_page(&self, offset: u64, access: Access) -> BoxFuture<'_, Result<()>> {
        Box::pin(Window::fault(self, offset, access))
    }

    fn probe(&self, offset: u64) -> BoxFuture<'_, Result<()>> {
        Box::pin(Window::fault(self, offset, Access::Read))
    }
}

/// fxdax: a change to a [`SecondStage`] begun ([`SecondStage::begin_change`]), which ends however
/// what makes it does.
struct Change<'a>(&'a dyn SecondStage);

impl<'a> Change<'a> {
    fn begin(stage: &'a dyn SecondStage) -> Self {
        stage.begin_change();
        Self(stage)
    }
}

impl Drop for Change<'_> {
    fn drop(&mut self) {
        self.0.end_change();
    }
}

/// What a window's mapping maps pages of (`crate::cache`): a sealed extent's pages, or zeros. Its
/// pages are numbered from its first, each a page of physical memory once it is in memory, which
/// the source keeps until it lets go of it: a page it evicts, it first unmaps from each window it
/// was told maps it ([`Window::unmap_pages_of`]).
///
/// Its lock is taken after a window's, never before: a window looks its pages up, and maps them,
/// under its own.
pub(crate) trait Source: Send + Sync {
    /// Calls `found` with each page of `pages` in memory, by number, under the source's lock, so
    /// that it stays in memory while `found` maps it.
    fn each_present(&self, pages: Range<u64>, found: &mut dyn FnMut(u64, &physmem::Page));

    /// Brings page `page` into memory: completes once it is in, holding no thread meanwhile,
    /// returning whether it waited for it; or fails, the page not in memory.
    fn bring_in(self: Arc<Self>, page: u64) -> BoxFuture<'static, Result<bool>>;

    /// How many pages from `page` a fault on it maps, as far as they are in memory: those bringing
    /// it in brought in with it, which the guest reading on would fault on next.
    fn brought_in_with(&self, _page: u64) -> u64 {
        1
    }

    /// `window` maps it from now on, until [`Source::unmapped`]: a page it evicts, it unmaps there
    /// first.
    fn mapped(&self, _window: &Arc<Window>) {}

    /// `window` maps it no more.
    fn unmapped(&self, _window: &Window) {}

    /// fxdax: whether its pages are zeros, which a store's private page is made of without a
    /// copy, nor bringing anything in (backlog doc-13).
    fn is_zeros(&self) -> bool {
        false
    }
}

/// The identity of `source`, the same for each of its handles: what a window finds the mappings of
/// a source by.
pub(crate) fn source_identity<T: ?Sized>(source: &Arc<T>) -> usize {
    Arc::as_ptr(source) as *const () as usize
}

/// A DAX window of an extent-store device. See the module docs.
///
/// Handles to a window are cheap to clone, and may be used from any thread.
#[derive(Clone)]
pub struct DaxWindow(Arc<Window>);

impl DaxWindow {
    /// Resolves the guest's fault at `offset` into the window, for `access`: maps the page, in the
    /// window's second stage, or, without one, at its host address, so that the guest can retry
    /// the access; fxdax: a store's makes it private, in a window that copies on write.
    ///
    /// Completes once the page is in memory, holding no thread meanwhile. Fails if nothing is
    /// mapped there, with `ENOENT`, or with `EACCES` for a store the window refuses: to a window
    /// that doesn't copy on write, or past its private pages; the guest cannot go on.
    pub async fn fault(&self, offset: u64, access: Access) -> Result<()> {
        self.0.fault(offset, access).await
    }

    /// The memory in this process where the pages of a window without a second stage, as tests'
    /// are, are mapped.
    #[cfg(test)]
    pub(crate) fn host_base(&self) -> *mut u8 {
        let host_base = self.0.host_base().expect("a test's window has no second stage");
        host_base as *mut u8
    }

    /// The window's size in bytes.
    pub fn size(&self) -> u64 {
        self.0.size()
    }

    /// Whether anything is mapped at `offset`.
    pub fn is_mapped(&self, offset: u64) -> bool {
        self.0.is_mapped(offset)
    }

    /// Whether it has pages mapped in its second stage, or in host memory, which reach them.
    pub fn maps_pages(&self) -> bool {
        !self.0.state.lock().unwrap().ptes.is_empty()
    }

    /// fxdax: makes the `len` bytes at `offset` private now, as stores to each page would, and
    /// maps them writable (doc-13's `PRIVATE`). See [`Window::make_private`].
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) async fn make_private(&self, offset: u64, len: u64) -> Result<()> {
        self.0.make_private(offset, len).await
    }

    /// fxdax: the private pages it holds (backlog doc-13): none unless it copies on write.
    pub fn private_pages(&self) -> u64 {
        self.0.private_pages()
    }
}

impl std::fmt::Debug for DaxWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaxWindow")
            .field("host_base", &self.0.host_base().map(|base| base as *mut u8))
            .field("size", &self.size())
            .finish()
    }
}

/// Where a window's pages are mapped: in exactly one place.
enum Translation {
    /// In its second stage alone, another process's (a VMM's) or another range of this one's.
    Stage(Arc<dyn SecondStage>),
    /// In host memory: a reservation of host address space, at `base`, as long as the window.
    Host { base: usize },
}

/// A mapping of a window: a range of its source's pages, from `page`, up to the window's `end`,
/// read-only.
struct Mapping {
    end: u64,
    source: Arc<dyn Source>,
    page: u64,
}

/// A page the window has mapped.
#[derive(Clone, Copy)]
enum Pte {
    /// A page of its mapping's source: which page of physical memory it is.
    Source { frame: u32 },
    /// fxdax: a private page (backlog doc-13), of the second stage's memory, or, without one, of
    /// this process's, mapped in place ([`Window::map_private`]).
    Private,
}

/// A window's mappings and the pages it has mapped, under its lock.
#[derive(Default)]
struct WindowState {
    /// The mapped ranges of the window, by start. They do not overlap.
    mappings: BTreeMap<u64, Mapping>,
    /// The starts of the mappings, by the identity of their source ([`source_identity`]).
    by_source: HashMap<usize, BTreeSet<u64>>,
    /// The pages mapped, by offset: each within a mapping, and its source's, or a private page.
    ptes: BTreeMap<u64, Pte>,
    /// fxdax: the private pages' offsets, of a window that copies on write ([`CopyOnWrite`]):
    /// each mapped there, writable, in `ptes`, within a mapping, which is still its source's.
    private: BTreeSet<u64>,
}

/// fxdax: what a private page holds of its source's page ([`Window::private_bytes`]).
enum Held {
    /// The page at a location of physical memory, which the second stage copies.
    At(PhysLocation),
    /// The page's bytes, copied, for a window without a second stage.
    Bytes(Box<[u8]>),
}

/// fxdax: what a change to a window's range does to the private pages in it (backlog doc-13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Private {
    /// They go, as the guest's `MAP` or `UNMAP` over them ends them.
    Free,
    /// They stay, as the host's own changes leave them: a free's zeros.
    Keep,
}

/// fxdax: how a window copies on write (backlog doc-13): against which arena its private pages are
/// counted, its tenant's, and how many it holds at most, which it reserved against the arena as it
/// was made (`physmem::pin`), and lets go of as it goes. The pages are its second stage's memory
/// ([`SecondStage::make_private`]), its VM process's, not the arena's: the reservation still counts
/// them against the tenant as they are reserved, and no store waits for the arena's memory.
pub(crate) struct CopyOnWrite {
    arena: crate::pages::ArenaId,
    most: u64,
}

impl CopyOnWrite {
    /// Reserves `most` pages of `arena`'s memory for a window's private pages: `EINVAL` for none,
    /// `ENOMEM` past what the arena may hold that nothing can evict.
    pub(crate) fn reserve(arena: crate::pages::ArenaId, most: u64) -> Result<Self> {
        let count = usize::try_from(most).map_err(|_| Errno::EINVAL)?;
        if count == 0 {
            return Err(Errno::EINVAL);
        }
        if !physmem::pin(arena, count) {
            return Err(Errno::ENOMEM);
        }
        Ok(Self { arena, most })
    }
}

impl Drop for CopyOnWrite {
    fn drop(&mut self) {
        physmem::unpin(self.arena, self.most as usize);
    }
}

/// The process's windows' mappings, their bytes, and the pages they have mapped, all windows'
/// together, as their states change: the extent daemon's gauges (`crate::extentd::metrics`,
/// TASK-131.6). fxdax: and their private pages, of those (backlog doc-13).
static LIVE_MAPPINGS: AtomicU64 = AtomicU64::new(0);
static LIVE_MAPPED_BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE_PTES: AtomicU64 = AtomicU64::new(0);
static LIVE_PRIVATE: AtomicU64 = AtomicU64::new(0);

/// The process's windows' mappings, as of now.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Live {
    /// Their mapped ranges.
    pub mappings: u64,
    /// The bytes those ranges span.
    pub mapped_bytes: u64,
    /// The pages they have mapped in their second stages: of those bytes, those the guest reaches
    /// without a fault.
    pub pages: u64,
    /// fxdax: of those pages, the private ones of windows that copy on write (backlog doc-13).
    pub private_pages: u64,
}

/// The process's windows' mappings, as of now.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn live() -> Live {
    Live {
        mappings: LIVE_MAPPINGS.load(Ordering::Relaxed),
        mapped_bytes: LIVE_MAPPED_BYTES.load(Ordering::Relaxed),
        pages: LIVE_PTES.load(Ordering::Relaxed),
        private_pages: LIVE_PRIVATE.load(Ordering::Relaxed),
    }
}

impl Drop for WindowState {
    fn drop(&mut self) {
        let bytes: u64 = self.mappings.iter().map(|(start, mapping)| mapping.end - start).sum();
        LIVE_MAPPINGS.fetch_sub(self.mappings.len() as u64, Ordering::Relaxed);
        LIVE_MAPPED_BYTES.fetch_sub(bytes, Ordering::Relaxed);
        LIVE_PTES.fetch_sub(self.ptes.len() as u64, Ordering::Relaxed);
        LIVE_PRIVATE.fetch_sub(self.private.len() as u64, Ordering::Relaxed);
    }
}

impl WindowState {
    /// Records that the window has mapped `pte` at `offset`, where it had mapped nothing.
    fn insert_pte(&mut self, offset: u64, pte: Pte) {
        let previous = self.ptes.insert(offset, pte);
        debug_assert!(previous.is_none(), "a page mapped over another at {offset:#x}");
        LIVE_PTES.fetch_add(1, Ordering::Relaxed);
    }

    /// fxdax: records that the window has mapped a private page at `offset`, where it had mapped
    /// nothing.
    fn insert_private(&mut self, offset: u64) {
        self.insert_pte(offset, Pte::Private);
        let inserted = self.private.insert(offset);
        debug_assert!(inserted, "a private page over another at {offset:#x}");
        LIVE_PRIVATE.fetch_add(1, Ordering::Relaxed);
    }

    /// fxdax: forgets the private pages of `start..end`, which the window has unmapped, the stage
    /// letting go of them with what it unmapped.
    fn forget_private(&mut self, start: u64, end: u64) {
        let offsets: Vec<u64> = self.private.range(start..end).copied().collect();
        LIVE_PRIVATE.fetch_sub(offsets.len() as u64, Ordering::Relaxed);
        for offset in offsets {
            self.private.remove(&offset);
        }
    }

    /// Records that the window has unmapped the page at `offset`, which it had mapped.
    fn remove_pte(&mut self, offset: u64) {
        if self.ptes.remove(&offset).is_some() {
            LIVE_PTES.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// The mapping `offset` is in, and its start.
    fn mapping_at(&self, offset: u64) -> Option<(u64, &Mapping)> {
        let (start, mapping) = self.mappings.range(..=offset).next_back()?;
        (mapping.end > offset).then_some((*start, mapping))
    }

    /// The starts of the mappings overlapping `start..end`: as they do not overlap each other,
    /// the last to start before it, and those starting in it.
    fn overlapping(&self, start: u64, end: u64) -> Vec<u64> {
        let before = self.mappings.range(..start).next_back();
        before
            .filter(|(_, mapping)| mapping.end > start)
            .map(|(start, _)| *start)
            .into_iter()
            .chain(self.mappings.range(start..end).map(|(start, _)| *start))
            .collect()
    }

    fn insert(&mut self, start: u64, mapping: Mapping) {
        let identity = source_identity(&mapping.source);
        self.by_source.entry(identity).or_default().insert(start);
        LIVE_MAPPINGS.fetch_add(1, Ordering::Relaxed);
        LIVE_MAPPED_BYTES.fetch_add(mapping.end - start, Ordering::Relaxed);
        let previous = self.mappings.insert(start, mapping);
        debug_assert!(previous.is_none(), "a mapping over another at {start:#x}");
    }

    fn remove(&mut self, start: u64) -> Mapping {
        let mapping = self.mappings.remove(&start).expect("a mapping there");
        LIVE_MAPPINGS.fetch_sub(1, Ordering::Relaxed);
        LIVE_MAPPED_BYTES.fetch_sub(mapping.end - start, Ordering::Relaxed);
        let identity = source_identity(&mapping.source);
        let starts = self.by_source.get_mut(&identity).expect("its source's mappings");
        starts.remove(&start);
        if starts.is_empty() {
            self.by_source.remove(&identity);
        }
        mapping
    }

    /// Forgets the mappings of `start..end`, keeping what of them is around it, and returns the
    /// sources of those it forgot.
    fn forget(&mut self, start: u64, end: u64) -> Vec<Arc<dyn Source>> {
        let ps = page_size();
        let mut forgotten = Vec::new();
        for mapping_start in self.overlapping(start, end) {
            let mapping = self.remove(mapping_start);
            if mapping_start < start {
                let before = Mapping { end: start, source: mapping.source.clone(), ..mapping };
                self.insert(mapping_start, before);
            }
            if mapping.end > end {
                let page = mapping.page + (end - mapping_start) / ps;
                let after = Mapping { page, source: mapping.source.clone(), ..mapping };
                self.insert(end, after);
            }
            forgotten.push(mapping.source);
        }
        forgotten
    }
}

/// A window. See the module docs.
pub(crate) struct Window {
    size: u64,
    translation: Translation,
    state: Mutex<WindowState>,
    /// fxdax: how it copies on write, if it does (backlog doc-13): a store to a window that
    /// doesn't, a follower's, is refused.
    copy_on_write: Option<CopyOnWrite>,
}

impl Window {
    /// A window of `size` bytes, which must be a multiple of the page size, whose pages are mapped
    /// in `second_stage`, or, without one, in this process's memory; copying on write as
    /// `copy_on_write` says, if given.
    fn new(
        size: u64,
        second_stage: Option<Arc<dyn SecondStage>>,
        copy_on_write: Option<CopyOnWrite>,
    ) -> Result<Self> {
        if size == 0 || !size.is_multiple_of(page_size()) {
            return Err(Errno::EINVAL);
        }
        let len = usize::try_from(size).map_err(|_| Errno::EINVAL)?;
        // fxdax: no more private pages than the window has pages.
        if copy_on_write.as_ref().is_some_and(|copying| copying.most > size / page_size()) {
            return Err(Errno::EINVAL);
        }
        let translation = match second_stage {
            Some(stage) => Translation::Stage(stage),
            None => Translation::Host { base: reserve(None, len).map_err(|_| Errno::ENOMEM)? },
        };
        let state = Mutex::new(WindowState::default());
        Ok(Self { size, translation, state, copy_on_write })
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn stage(&self) -> Option<&Arc<dyn SecondStage>> {
        match &self.translation {
            Translation::Stage(stage) => Some(stage),
            Translation::Host { .. } => None,
        }
    }

    fn host_base(&self) -> Option<usize> {
        match self.translation {
            Translation::Host { base } => Some(base),
            Translation::Stage(_) => None,
        }
    }

    fn is_mapped(&self, offset: u64) -> bool {
        self.state.lock().unwrap().mapping_at(offset).is_some()
    }

    /// Resolves the guest's fault at `offset` for `access`: a load's maps the page
    /// ([`Window::resolve`]); fxdax: a store's makes it private, as every page is mapped
    /// read-only, in a window that copies on write ([`Window::copy`]), and is refused in one that
    /// doesn't.
    pub(crate) async fn fault(&self, offset: u64, access: Access) -> Result<()> {
        use crate::stats::{self, Counter};
        let page = offset - offset % page_size();
        let start = std::time::Instant::now();
        let faulted = match (access, &self.copy_on_write) {
            (Access::Read, _) => self.resolve(page).await,
            (Access::Write, Some(_)) => self.copy(page).await,
            (Access::Write, None) => match self.is_mapped(page) {
                true => Err(Errno::EACCES),
                false => Err(Errno::ENOENT),
            },
        };
        stats::count_since(Counter::Faults, Counter::FaultNs, start);
        if faulted == Ok(true) {
            stats::add(Counter::FaultWaits, 1);
        }
        faulted.map(|_| ())
    }

    /// Maps the page at `offset`, which a load's fault was on, bringing it into memory first, with
    /// those its source brought in with it (`Source::brought_in_with`), and returns whether it
    /// waited for the page to be brought in. A page evicted, or mapped anew, meanwhile is resolved
    /// as it is then.
    async fn resolve(&self, offset: u64) -> Result<bool> {
        let ps = page_size();
        let mut waited = false;
        loop {
            let (source, page, mapped) = {
                let state = self.state.lock().unwrap();
                let (start, mapping) = state.mapping_at(offset).ok_or(Errno::ENOENT)?;
                if state.ptes.contains_key(&offset) {
                    return Ok(waited);
                }
                let page = mapping.page + (offset - start) / ps;
                let left = (mapping.end - offset) / ps;
                (mapping.source.clone(), page, left.min(mapping.source.brought_in_with(page)))
            };
            waited |= source.clone().bring_in(page).await?;
            let mut state = self.state.lock().unwrap();
            let same = state.mapping_at(offset).is_some_and(|(start, mapping)| {
                source_identity(&mapping.source) == source_identity(&source)
                    && mapping.page + (offset - start) / ps == page
            });
            if same {
                self.install(&mut state, offset, offset + mapped * ps);
                if state.ptes.contains_key(&offset) {
                    return Ok(waited);
                }
            }
        }
    }

    /// fxdax: makes the page at `offset` private, as a store to it does in a window that copies on
    /// write (backlog doc-13), unless it is already: a page holding what the page held, its
    /// source's bytes, brought in first, or, of zeros, zeros, nothing copied; mapped writable in
    /// its place ([`Window::map_private`]), one change of the second stage, the mapping still its
    /// source's. The guest's view doesn't change: what it read there it reads, and no store was
    /// lost, as the page was read-only. Returns whether it waited for the source's page. `EACCES`
    /// past the window's private pages, which the guest keeps within: the store is refused, as one
    /// to a window that doesn't copy is, and its VM stops.
    async fn copy(&self, offset: u64) -> Result<bool> {
        let ps = page_size();
        let copying = self.copy_on_write.as_ref().expect("a window that copies on write");
        let mut waited = false;
        loop {
            let (source, page) = {
                let state = self.state.lock().unwrap();
                let (start, mapping) = state.mapping_at(offset).ok_or(Errno::ENOENT)?;
                if state.private.contains(&offset) {
                    return Ok(waited);
                }
                if state.private.len() as u64 >= copying.most {
                    return Err(Errno::EACCES);
                }
                (mapping.source.clone(), mapping.page + (offset - start) / ps)
            };
            let zeros = source.is_zeros();
            if !zeros {
                waited |= source.clone().bring_in(page).await?;
            }
            let _change = self.stage().map(|stage| Change::begin(stage.as_ref()));
            let mut state = self.state.lock().unwrap();
            // Mapped anew meanwhile, by the guest's MAP or a free's zeros, the store is to what is
            // mapped now.
            let same = state.mapping_at(offset).is_some_and(|(start, mapping)| {
                source_identity(&mapping.source) == source_identity(&source)
                    && mapping.page + (offset - start) / ps == page
            });
            if !same && rules().copy_rechecks_mapping {
                continue;
            }
            if state.private.contains(&offset) {
                return Ok(waited);
            }
            // Counted as it is mapped: two stores may have found room for one.
            if state.private.len() as u64 >= copying.most && rules().copy_counts_under_lock {
                return Err(Errno::EACCES);
            }
            // What it holds: the source's page, in memory still, which the window's lock keeps so,
            // as a source evicting it unmaps it here first, under the lock; or zeros.
            let mut held = None;
            if !zeros {
                source.each_present(page..page + 1, &mut |_, frame| {
                    held = Some(self.private_bytes(frame));
                });
                // Evicted since it was brought in: brought in again.
                if held.is_none() {
                    continue;
                }
            }
            if state.ptes.contains_key(&offset) {
                state.remove_pte(offset);
            }
            state.insert_private(offset);
            self.map_private(offset, held);
            return Ok(waited);
        }
    }

    /// fxdax: what a private page copies of the source's page `frame` ([`Window::map_private`]):
    /// where it is, for the second stage to copy it, which it keeps from being freed until the
    /// stage has made the copy, as it took the page away; or, without a stage, its bytes.
    fn private_bytes(&self, frame: &physmem::Page) -> Held {
        match self.translation {
            Translation::Stage(_) => {
                // Once freed, it waits for the stage to copy it (`physmem::quarantine_with`).
                frame.mark_staged();
                Held::At(frame.location())
            }
            Translation::Host { .. } => {
                let ps = page_size() as usize;
                // SAFETY: The source's page is in memory while its lock is held, and never changes,
                // a sealed extent's.
                Held::Bytes(unsafe { std::slice::from_raw_parts(frame.as_ptr(), ps) }.into())
            }
        }
    }

    /// fxdax: maps a private page at `offset` (backlog doc-13), writable, in place of what was
    /// mapped there: in the second stage, its own memory, holding what `held` says, or zeros
    /// ([`SecondStage::make_private`]); or, without a stage, anonymous memory of this process.
    fn map_private(&self, offset: u64, held: Option<Held>) {
        match &self.translation {
            Translation::Stage(stage) => {
                let from = held.map(|held| match held {
                    Held::At(location) => location,
                    Held::Bytes(_) => unreachable!("a stage's private page held as bytes"),
                });
                stage.make_private(offset, from);
            }
            Translation::Host { base } => {
                let ps = page_size() as usize;
                let at = (base + offset as usize) as *mut libc::c_void;
                let (prot, flags) = (
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_FIXED,
                );
                // SAFETY: Anonymous memory over the window's reservation, which only the guest, a
                // test, accesses: zeros.
                let addr = unsafe { libc::mmap(at, ps, prot, flags, -1, 0) };
                assert_eq!(addr, at, "map a private page: {}", std::io::Error::last_os_error());
                if let Some(Held::Bytes(bytes)) = held {
                    // SAFETY: The page just mapped, `ps` bytes, as `bytes` are.
                    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), at.cast(), ps) };
                }
            }
        }
    }

    /// fxdax: makes the `len` bytes at `offset` private now, as stores to each of their pages
    /// would ([`Window::copy`]), mapped writable, so for the guest ahead of its accesses as the
    /// stage maps private pages: doc-13's `PRIVATE`, a `write(2)`'s pages in one round trip
    /// rather than a host fault each. `ENOMEM`, nothing made private, if they would take the
    /// window past its private pages; `EINVAL` for a window that doesn't copy on write, a range
    /// not within it, or one with a page mapped nothing at, none made private.
    async fn make_private(&self, offset: u64, len: u64) -> Result<()> {
        let copying = self.copy_on_write.as_ref().ok_or(Errno::EINVAL)?;
        self.check_range(offset, len)?;
        let ps = page_size();
        let pages = (offset..offset + len).step_by(ps as usize);
        {
            let state = self.state.lock().unwrap();
            let mut more = 0;
            for at in pages.clone() {
                state.mapping_at(at).ok_or(Errno::EINVAL)?;
                more += u64::from(!state.private.contains(&at));
            }
            if state.private.len() as u64 + more > copying.most {
                return Err(Errno::ENOMEM);
            }
        }
        for at in pages {
            // Past the bound, room taken by stores meanwhile.
            if let Err(errno) = self.copy(at).await {
                return Err(if errno == Errno::EACCES { Errno::ENOMEM } else { errno });
            }
        }
        Ok(())
    }

    /// fxdax: the private pages it holds (backlog doc-13): none unless it copies on write.
    pub(crate) fn private_pages(&self) -> u64 {
        self.state.lock().unwrap().private.len() as u64
    }

    /// Maps as [`Window::map_as`] does, as the guest's `MAP`, which ends the private pages there.
    #[cfg(test)]
    fn map(
        self: &Arc<Self>,
        offset: u64,
        source: &Arc<dyn Source>,
        source_offset: u64,
        len: u64,
    ) -> Result<()> {
        self.map_as(offset, source, source_offset, len, Private::Free)
    }

    /// Maps `len` bytes of `source`'s pages from `source_offset` into the window at `offset`,
    /// read-only, replacing what was mapped there, and then its pages in memory: one change to the
    /// second stage, if there is one (`SecondStage::begin_change`). fxdax: the private pages there
    /// go or stay as `private` says: the guest's `MAP` ends them (`Private::Free`), the host's own
    /// changes leave them, a free's zeros.
    fn map_as(
        self: &Arc<Self>,
        offset: u64,
        source: &Arc<dyn Source>,
        source_offset: u64,
        len: u64,
        private: Private,
    ) -> Result<()> {
        let _change = self.stage().map(|stage| Change::begin(stage.as_ref()));
        let end = offset + len;
        let mut state = self.state.lock().unwrap();
        self.unmap_ptes(&mut state, offset, end, private);
        let forgotten = state.forget(offset, end);
        let identity = source_identity(source);
        let first = !state.by_source.contains_key(&identity);
        let page = source_offset / page_size();
        state.insert(offset, Mapping { end, source: source.clone(), page });
        if first {
            source.mapped(self);
        }
        if let Some(stage) = self.stage() {
            stage.window_mapped(offset, len);
        }
        // Its pages in memory are mapped now, so that the guest's first accesses to them do not
        // fault. fxdax: but where a private page stays.
        self.install(&mut state, offset, end);
        self.unmapped(&state, forgotten);
        Ok(())
    }

    /// Unmaps `len` bytes of the window at `offset`: fxdax: the guest's `UNMAP`, which ends the
    /// private pages there.
    fn unmap(&self, offset: u64, len: u64) {
        self.unmap_as(offset, len, Private::Free);
    }

    /// fxdax: unmaps as [`Window::unmap`] does, its private pages there going or staying as
    /// `private` says.
    fn unmap_as(&self, offset: u64, len: u64, private: Private) {
        let end = offset + len;
        let mut state = self.state.lock().unwrap();
        self.unmap_ptes(&mut state, offset, end, private);
        let forgotten = state.forget(offset, end);
        self.unmapped(&state, forgotten);
    }

    /// Tells the sources `forgotten` that no longer have a mapping in the window so.
    fn unmapped(&self, state: &WindowState, forgotten: Vec<Arc<dyn Source>>) {
        let mut told = BTreeSet::new();
        for source in forgotten {
            let identity = source_identity(&source);
            if !state.by_source.contains_key(&identity) && told.insert(identity) {
                source.unmapped(self);
            }
        }
    }

    /// Unmaps the pages the window has mapped of `source`, by its identity, that `evicted` says
    /// are of the frames it evicts: before it lets go of them. fxdax: a private page within its
    /// mappings is none of its pages, and stays (backlog doc-13).
    pub(crate) fn unmap_pages_of(&self, source: usize, evicted: &dyn Fn(u32) -> bool) {
        let mut state = self.state.lock().unwrap();
        let Some(starts) = state.by_source.get(&source) else { return };
        let ranges: Vec<(u64, u64)> =
            starts.iter().map(|start| (*start, state.mappings[start].end)).collect();
        let mut gone = Vec::new();
        for (start, end) in ranges {
            gone.extend(
                (state.ptes.range(start..end))
                    .filter(|(_, pte)| match pte {
                        Pte::Source { frame, .. } => evicted(*frame),
                        Pte::Private => !rules().eviction_skips_private,
                    })
                    .map(|(offset, _)| *offset),
            );
        }
        for offset in &gone {
            state.remove_pte(*offset);
        }
        self.unmap_runs(&gone);
    }

    /// Maps, where nothing is mapped, the pages in memory of the mappings of `start..end`: in runs
    /// of pages consecutive in the window and in an arena's object, each one change of the second
    /// stage.
    fn install(&self, state: &mut WindowState, start: u64, end: u64) {
        let ps = page_size();
        let staged = self.stage().is_some();
        // The pages to map, in order: their offsets, and where they are.
        let mut pages: Vec<(u64, PhysLocation)> = Vec::new();
        for mapping_start in state.overlapping(start, end) {
            let mapping = &state.mappings[&mapping_start];
            let (from, to) = (start.max(mapping_start), end.min(mapping.end));
            let first = mapping.page + (from - mapping_start) / ps;
            let ptes = &state.ptes;
            let mut found = Vec::new();
            mapping.source.each_present(first..first + (to - from) / ps, &mut |page, frame| {
                let offset = from + (page - first) * ps;
                if ptes.contains_key(&offset) {
                    return;
                }
                // Once freed, it waits for the stage to take it away (`physmem::quarantine_with`,
                // TASK-79.9).
                if staged {
                    frame.mark_staged();
                }
                found.push((offset, frame.location(), frame.frame()));
            });
            for (offset, location, frame) in found {
                state.insert_pte(offset, Pte::Source { frame });
                pages.push((offset, location));
            }
        }
        // fxdax: the runs of pages consecutive in the window, which the maps below are made of
        // (`stage_segments`, TASK-131.1).
        let segments = 1 + pages.windows(2).filter(|pair| pair[0].0 + ps != pair[1].0).count();
        if self.stage().is_some() && !pages.is_empty() {
            crate::stats::add(crate::stats::Counter::StageSegments, segments as u64);
        }
        let mut run: Option<(u64, PhysLocation, u64)> = None;
        for (offset, location) in pages {
            match &mut run {
                Some((at, first, len))
                    if *at + *len == offset
                        && first.arena == location.arena
                        && first.offset + *len == location.offset =>
                {
                    *len += ps;
                }
                _ => {
                    if let Some((at, first, len)) = run.replace((offset, location, ps)) {
                        self.map_run(at, first, len);
                    }
                }
            }
        }
        if let Some((at, first, len)) = run {
            self.map_run(at, first, len);
        }
    }

    /// Unmaps the pages the window has mapped of `start..end`. fxdax: its private pages there too,
    /// which the stage lets go of as it unmaps them, if `private` says they go; else they stay
    /// mapped.
    fn unmap_ptes(&self, state: &mut WindowState, start: u64, end: u64, private: Private) {
        let gone: Vec<u64> = (state.ptes.range(start..end))
            .map(|(offset, _)| *offset)
            .filter(|offset| private == Private::Free || !state.private.contains(offset))
            .collect();
        for offset in &gone {
            state.remove_pte(*offset);
        }
        self.unmap_runs(&gone);
        if private == Private::Free {
            state.forget_private(start, end);
        }
    }

    /// Unmaps the pages at `offsets`, in order, in runs of consecutive pages.
    fn unmap_runs(&self, offsets: &[u64]) {
        let ps = page_size();
        let mut runs: Vec<(u64, u64)> = Vec::new();
        for &offset in offsets {
            match runs.last_mut() {
                Some((start, len)) if *start + *len == offset => *len += ps,
                _ => runs.push((offset, ps)),
            }
        }
        for (start, len) in runs {
            match &self.translation {
                Translation::Stage(stage) => stage.unmap(start, len),
                Translation::Host { base } => {
                    let at = base + start as usize;
                    reserve(Some(at), len as usize).expect("the window's memory reserved again");
                }
            }
        }
    }

    /// Maps the `len` bytes at `offset` to the pages at `location`, read-only.
    fn map_run(&self, offset: u64, location: PhysLocation, len: u64) {
        match &self.translation {
            Translation::Stage(stage) => stage.map(offset, location, len),
            Translation::Host { base } => {
                let fd = crate::pages::arena_memory(location.arena).expect("the page's arena");
                let prot = libc::PROT_READ;
                let at = (base + offset as usize) as *mut libc::c_void;
                // SAFETY: Maps pages of physical memory, shared, over the window's reservation,
                // which only the guest, a test, accesses.
                let addr = unsafe {
                    use std::os::fd::AsRawFd as _;
                    let (flags, fd) = (libc::MAP_SHARED | libc::MAP_FIXED, fd.as_raw_fd());
                    libc::mmap(at, len as usize, prot, flags, fd, location.offset as libc::off_t)
                };
                assert_eq!(addr, at, "map pages: {}", std::io::Error::last_os_error());
            }
        }
    }

    /// Checks that `len` bytes at `offset` are page aligned and within the window.
    fn check_range(&self, offset: u64, len: u64) -> Result<()> {
        let end = offset.checked_add(len).ok_or(Errno::EINVAL)?;
        if len == 0
            || !offset.is_multiple_of(page_size())
            || !len.is_multiple_of(page_size())
            || end > self.size()
        {
            return Err(Errno::EINVAL);
        }
        Ok(())
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        if let Translation::Host { base } = self.translation {
            // SAFETY: The reservation is the window's own.
            let status = unsafe { libc::munmap(base as *mut libc::c_void, self.size as usize) };
            debug_assert_eq!(status, 0, "unreserve a window: {}", std::io::Error::last_os_error());
        }
    }
}

/// Reserves `len` bytes of host address space, at `at` if given, over what is there, inaccessible
/// until pages are mapped there.
fn reserve(at: Option<usize>, len: usize) -> std::io::Result<usize> {
    let flags = libc::MAP_PRIVATE
        | libc::MAP_ANON
        | libc::MAP_NORESERVE
        | if at.is_some() { libc::MAP_FIXED } else { 0 };
    let hint = at.unwrap_or(0) as *mut libc::c_void;
    // SAFETY: Either an address hint, or replacing part of a window's own reservation.
    let base = unsafe { libc::mmap(hint, len, libc::PROT_NONE, flags, -1, 0) };
    if base == libc::MAP_FAILED {
        return Err(std::io::Error::last_os_error());
    }
    Ok(base as usize)
}

/// The rules of how a window changes its mappings, each of which its Shuttle tests show to be
/// needed by turning it off (TASK-44.3).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rules {
    /// A freed extent's mappings map zeros rather than nothing: a store racing the free faults on
    /// them as on any read-only page.
    pub free_maps_zeros: bool,
    /// fxdax: a store's private copy is mapped only if the page still maps what it copied: a
    /// `MAP` meanwhile maps what the store is to (backlog doc-13).
    pub copy_rechecks_mapping: bool,
    /// fxdax: a private copy is counted against the window's bound as it is mapped, not only
    /// before it is made: two stores at once may each have found room for one.
    pub copy_counts_under_lock: bool,
    /// fxdax: a source evicting its pages leaves the private pages within its mappings.
    pub eviction_skips_private: bool,
}

impl Rules {
    pub const ALL: Self = Self {
        free_maps_zeros: true,
        copy_rechecks_mapping: true,
        copy_counts_under_lock: true,
        eviction_skips_private: true,
    };
}

#[cfg(not(feature = "shuttle"))]
fn rules() -> Rules {
    Rules::ALL
}

#[cfg(feature = "shuttle")]
static RULES: std::sync::Mutex<Rules> = std::sync::Mutex::new(Rules::ALL);

#[cfg(feature = "shuttle")]
fn rules() -> Rules {
    *RULES.lock().unwrap()
}

/// What keeps a window's mapping's memory alive (`Dax`).
pub(crate) trait Keep: Clone {
    /// Which memory it keeps alive, the same for each of its clones: what `Dax` finds the
    /// mappings it keeps by.
    fn identity(&self) -> usize;
}

impl<T: ?Sized> Keep for Arc<T> {
    fn identity(&self) -> usize {
        source_identity(self)
    }
}

impl Keep for () {
    fn identity(&self) -> usize {
        0
    }
}

/// A DAX window, and what is mapped into it: what keeps each mapping's memory alive, a `K`, such
/// as the sealed extent's cached pages that it maps.
pub(crate) struct Dax<K: Keep> {
    window: Arc<Window>,
    /// The mapped ranges of the window, by start: their ends and what keeps them alive. They do
    /// not overlap.
    mappings: BTreeMap<u64, (u64, K)>,
    /// The starts of the mappings, by the identity of what keeps them alive.
    kept: HashMap<usize, BTreeSet<u64>>,
    /// Zeros, which the window maps where nothing else is to be read ([`Dax::zeros`]): had before
    /// it maps anything, so that mapping them takes no memory.
    zeros: Arc<dyn Source>,
    /// The thread serving the faults on the host memory a test's window was created in
    /// ([`Dax::in_host_memory`]), which runs until the window is dropped.
    #[cfg(all(test, target_os = "linux"))]
    _faults: Option<uffd::FaultThread>,
}

impl<K: Keep> Dax<K> {
    /// A window of `size` bytes whose pages are mapped in `second_stage`, or, without one, in this
    /// process's memory, and a handle to it.
    pub fn new(size: u64, second_stage: Option<Arc<dyn SecondStage>>) -> Result<(Self, DaxWindow)> {
        Self::copying(size, second_stage, None)
    }

    /// fxdax: as [`Dax::new`], a window that copies on write as `copy_on_write` says, if given
    /// (backlog doc-13).
    pub fn copying(
        size: u64,
        second_stage: Option<Arc<dyn SecondStage>>,
        copy_on_write: Option<CopyOnWrite>,
    ) -> Result<(Self, DaxWindow)> {
        let window = Arc::new(Window::new(size, second_stage, copy_on_write)?);
        let dax = Self {
            window: window.clone(),
            mappings: BTreeMap::new(),
            kept: HashMap::new(),
            zeros: crate::cache::zeros()?,
            #[cfg(all(test, target_os = "linux"))]
            _faults: None,
        };
        Ok((dax, DaxWindow(window)))
    }

    /// A window whose second stage is the host memory `stage` takes over, and a handle to it. A
    /// thread of its own serves the faults on the memory, in this process, as tests have it: a
    /// VM's window is the extent daemon's, whose VM process serves them (`crate::extentd`).
    #[cfg(all(test, target_os = "linux"))]
    pub fn in_host_memory(
        stage: Arc<uffd::UffdSecondStage>,
    ) -> crate::error::Result<(Self, DaxWindow), crate::Error> {
        Self::in_host_memory_copying(stage, None)
    }

    /// fxdax: as [`Dax::in_host_memory`], a window that copies on write as `copy_on_write` says,
    /// if given, its stage told so.
    #[cfg(all(test, target_os = "linux"))]
    pub fn in_host_memory_copying(
        stage: Arc<uffd::UffdSecondStage>,
        copy_on_write: Option<CopyOnWrite>,
    ) -> crate::error::Result<(Self, DaxWindow), crate::Error> {
        if copy_on_write.is_some() {
            stage.copy_on_write()?;
        }
        let (mut dax, window) = Self::copying(stage.size(), Some(stage.clone()), copy_on_write)?;
        dax._faults = Some(stage.serve(dax.window.clone())?);
        Ok((dax, window))
    }

    /// fxdax: the private pages the window holds (backlog doc-13).
    pub fn private_pages(&self) -> u64 {
        self.window.private_pages()
    }

    /// Checks the window's range for a mapping of `len` bytes at `offset`.
    pub fn check_range(&self, offset: u64, len: u64) -> Result<()> {
        self.window.check_range(offset, len)
    }

    /// Maps `len` bytes of `source`, whose memory `keep` keeps alive, at `source_offset` into the
    /// window at `offset`, read-only: the guest's `MAP`, which ends the private pages there.
    pub fn map(
        &mut self,
        offset: u64,
        keep: K,
        source: &Arc<dyn Source>,
        source_offset: u64,
        len: u64,
    ) -> Result<()> {
        self.window.map_as(offset, source, source_offset, len, Private::Free)?;
        let end = offset + len;
        self.forget(offset, end);
        self.insert(offset, end, keep);
        Ok(())
    }

    /// Unmaps `len` bytes of the window at `offset`.
    pub fn unmap(&mut self, offset: u64, len: u64) -> Result<()> {
        self.window.unmap(offset, len);
        self.forget(offset, offset + len);
        Ok(())
    }

    /// Maps the window's ranges whose mappings keep alive what has the identity `kept`, what a
    /// freed extent mapped, from `zeros` instead, read-only, kept alive by `keep`: their memory
    /// goes with their old mappings. The guest may still touch such a range, as a store racing the
    /// move of what it maps, which faults as on any read-only page; unmapped, its fault would find
    /// nothing to map. fxdax: the private pages there stay, the guest's, which only its own `MAP`
    /// or `UNMAP` ends (backlog doc-13).
    pub fn replace_kept(&mut self, kept: usize, keep: K, zeros: &Arc<dyn Source>) -> Result<()> {
        let starts: Vec<u64> =
            self.kept.get(&kept).map(|starts| starts.iter().copied().collect()).unwrap_or_default();
        for start in starts {
            let end = self.mappings[&start].0;
            if rules().free_maps_zeros {
                self.window.map_as(start, zeros, 0, end - start, Private::Keep)?;
            } else {
                self.window.unmap_as(start, end - start, Private::Keep);
            }
            self.remove(start);
            self.insert(start, end, keep.clone());
        }
        Ok(())
    }

    /// The window's mappings that keep alive what has the identity `kept`, in order, as their
    /// starts, ends and what keeps them alive: as they are now, as maps, unmaps and remaps left
    /// them.
    pub fn kept(&self, kept: usize) -> Vec<(u64, u64, K)> {
        let Some(starts) = self.kept.get(&kept) else { return Vec::new() };
        (starts.iter())
            .map(|start| {
                let (end, keep) = &self.mappings[start];
                (*start, *end, keep.clone())
            })
            .collect()
    }

    /// Zeros, which the window maps where nothing else is to be read: one page of them, read-only,
    /// wherever it maps them (`crate::cache::zeros`).
    pub fn zeros(&self) -> Arc<dyn Source> {
        self.zeros.clone()
    }

    /// The window's mappings, in order, as their starts, ends and what keeps them alive.
    pub fn mappings(&self) -> impl Iterator<Item = (u64, u64, &K)> {
        self.mappings.iter().map(|(start, (end, keep))| (*start, *end, keep))
    }

    /// The mappings overlapping the window's range `start..end`, as their starts, ends and what
    /// keeps them alive: as they do not overlap each other, the last to start before it, and
    /// those starting in it.
    fn overlapping(&self, start: u64, end: u64) -> impl Iterator<Item = (u64, u64, &K)> {
        let before = self.mappings.range(..start).next_back();
        before
            .filter(|(_, (mapping_end, _))| *mapping_end > start)
            .into_iter()
            .chain(self.mappings.range(start..end))
            .map(|(start, (end, keep))| (*start, *end, keep))
    }

    /// Records a mapping of the window's range `start..end`, kept alive by `keep`.
    fn insert(&mut self, start: u64, end: u64, keep: K) {
        self.kept.entry(keep.identity()).or_default().insert(start);
        self.mappings.insert(start, (end, keep));
    }

    /// Forgets the mapping starting at `start`, returning its end and what kept it alive.
    fn remove(&mut self, start: u64) -> (u64, K) {
        let (end, keep) = self.mappings.remove(&start).unwrap();
        let identity = keep.identity();
        let starts = self.kept.get_mut(&identity).unwrap();
        starts.remove(&start);
        if starts.is_empty() {
            self.kept.remove(&identity);
        }
        (end, keep)
    }

    /// Forgets the mappings of the window's range `start..end`, keeping those of what is around it.
    fn forget(&mut self, start: u64, end: u64) {
        let overlapping: Vec<u64> =
            self.overlapping(start, end).map(|(mapping_start, _, _)| mapping_start).collect();
        for mapping_start in overlapping {
            let (mapping_end, keep) = self.remove(mapping_start);
            if mapping_start < start {
                self.insert(mapping_start, start, keep.clone());
            }
            if mapping_end > end {
                self.insert(end, mapping_end, keep);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use futures::FutureExt;
    use std::task::{Context, Poll, Waker};

    #[cfg(feature = "shuttle")]
    use shuttle::sync::Condvar;
    #[cfg(not(feature = "shuttle"))]
    use std::sync::Condvar;

    /// How a [`TestSource`]'s pages come into memory.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Kind {
        /// Each is in memory from the start.
        Filled,
        /// Each is asked for, and comes in once the test supplies it ([`TestSource::supply`]).
        Paged,
        /// Each comes in, zeros, as it is first brought in, as a file's blocks never written do.
        Anonymous,
    }

    /// A test's source of pages, of physical memory's default arena.
    pub(crate) struct TestSource {
        kind: Kind,
        pages: Mutex<Vec<Option<physmem::Page>>>,
        /// The pages asked for, and not yet taken ([`TestSource::request`]).
        requests: Mutex<Vec<u64>>,
        asked: Condvar,
        /// What waits for pages to be supplied, under the lock their supply takes, so that none
        /// misses it.
        wakers: Mutex<Vec<Waker>>,
    }

    impl TestSource {
        fn new(kind: Kind, pages: Vec<Option<physmem::Page>>) -> Arc<Self> {
            Arc::new(Self {
                kind,
                pages: Mutex::new(pages),
                requests: Mutex::new(Vec::new()),
                asked: Condvar::new(),
                wakers: Mutex::new(Vec::new()),
            })
        }

        /// A page of physical memory, its bytes `value`.
        fn page_of(value: u8) -> physmem::Page {
            let page = physmem::alloc(physmem::ArenaId::DEFAULT, physmem::Wait::No).unwrap();
            // SAFETY: The page is allocated, and only this test's.
            unsafe { std::ptr::write_bytes(page.as_ptr(), value, page_size() as usize) };
            page
        }

        /// `pages` pages in memory, each its number plus `first`.
        pub fn numbered(pages: u64, first: u8) -> Arc<Self> {
            let pages = (0..pages).map(|page| Some(Self::page_of(first + page as u8))).collect();
            Self::new(Kind::Filled, pages)
        }

        /// `pages` pages in memory, each its bytes `value`.
        #[cfg_attr(not(any(target_os = "linux", feature = "shuttle")), allow(dead_code))]
        pub fn filled(pages: u64, value: u8) -> Arc<Self> {
            Self::new(Kind::Filled, (0..pages).map(|_| Some(Self::page_of(value))).collect())
        }

        /// `pages` pages in memory, page `page`'s bytes `tag(page)`, scattered over physical
        /// memory: no two consecutive, so that a window maps a run of each.
        #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
        pub fn scattered(pages: u64, tag: impl Fn(u64) -> u8) -> Arc<Self> {
            let mut spacers = Vec::new();
            let pages = (0..pages)
                .map(|page| {
                    let frame = Self::page_of(tag(page));
                    spacers.push(Self::page_of(0));
                    Some(frame)
                })
                .collect();
            drop(spacers);
            Self::new(Kind::Filled, pages)
        }

        /// `pages` pages, none in memory, each brought in once the test supplies it.
        pub fn paged(pages: u64) -> Arc<Self> {
            Self::new(Kind::Paged, (0..pages).map(|_| None).collect())
        }

        /// `pages` pages, each zeros as it is first brought in.
        pub fn anonymous(pages: u64) -> Arc<Self> {
            Self::new(Kind::Anonymous, (0..pages).map(|_| None).collect())
        }

        /// It, as a window's mappings take it.
        pub fn source(self: &Arc<Self>) -> Arc<dyn Source> {
            self.clone()
        }

        /// Supplies `page` with bytes of `value`.
        pub fn supply(&self, page: u64, value: u8) {
            let frame = Self::page_of(value);
            let woken = {
                let mut pages = self.pages.lock().unwrap();
                pages[page as usize] = Some(frame);
                std::mem::take(&mut *self.wakers.lock().unwrap())
            };
            for waker in woken {
                waker.wake();
            }
        }

        /// The page asked for next, waiting for one.
        #[cfg_attr(not(any(target_os = "linux", feature = "shuttle")), allow(dead_code))]
        pub fn request(&self) -> u64 {
            let mut requests = self.requests.lock().unwrap();
            while requests.is_empty() {
                requests = self.asked.wait(requests).unwrap();
            }
            requests.remove(0)
        }

        /// The pages asked for, without waiting.
        pub fn requests(&self) -> Vec<u64> {
            std::mem::take(&mut *self.requests.lock().unwrap())
        }

    }

    impl Source for TestSource {
        fn each_present(&self, pages: Range<u64>, found: &mut dyn FnMut(u64, &physmem::Page)) {
            let held = self.pages.lock().unwrap();
            for page in pages {
                if let Some(Some(frame)) = held.get(page as usize) {
                    found(page, frame);
                }
            }
        }

        fn bring_in(self: Arc<Self>, page: u64) -> BoxFuture<'static, Result<bool>> {
            let index = page as usize;
            {
                let mut pages = self.pages.lock().unwrap();
                if pages[index].is_some() {
                    return Box::pin(std::future::ready(Ok(false)));
                }
                match self.kind {
                    Kind::Filled => panic!("page {page} of a filled source is not in memory"),
                    Kind::Anonymous => {
                        pages[index] = Some(Self::page_of(0));
                        return Box::pin(std::future::ready(Ok(false)));
                    }
                    Kind::Paged => {}
                }
            }
            self.requests.lock().unwrap().push(page);
            self.asked.notify_all();
            Box::pin(std::future::poll_fn(move |cx| {
                let pages = self.pages.lock().unwrap();
                if pages[index].is_some() {
                    return Poll::Ready(Ok(true));
                }
                self.wakers.lock().unwrap().push(cx.waker().clone());
                Poll::Pending
            }))
        }
    }

    /// The window's mappings are found by the identity of what keeps them alive, as mapping over
    /// parts of them splits them.
    #[test]
    fn replacing_what_keeps_mappings_finds_what_remains_of_them() {
        let ps = page_size();
        let source = TestSource::anonymous(8).source();
        let zeros = crate::cache::zeros().unwrap();
        let (mut dax, _window) = Dax::<Arc<u8>>::new(8 * ps, None).unwrap();
        let (a, b, z) = (Arc::new(0), Arc::new(1), Arc::new(2));
        dax.map(0, a.clone(), &source, 0, 4 * ps).unwrap();
        dax.map(4 * ps, b.clone(), &source, 4 * ps, 2 * ps).unwrap();
        // Over the middle of `a`'s mapping, which splits around it.
        dax.map(ps, b.clone(), &source, ps, ps).unwrap();

        dax.replace_kept(a.identity(), z.clone(), &zeros).unwrap();
        let kept_by = |keep: &Arc<u8>| -> Vec<(u64, u64)> {
            let mappings = dax.mappings.iter();
            mappings
                .filter(|(_, (_, k))| Arc::ptr_eq(k, keep))
                .map(|(s, (e, _))| (*s, *e))
                .collect()
        };
        assert_eq!(kept_by(&z), [(0, ps), (2 * ps, 4 * ps)]);
        assert_eq!(kept_by(&b), [(ps, 2 * ps), (4 * ps, 6 * ps)]);
        assert!(!dax.kept.contains_key(&a.identity()));
        assert_eq!(Arc::strong_count(&a), 1, "the window still keeps what it no longer maps");
        assert_eq!(dax.kept[&z.identity()], BTreeSet::from([0, 2 * ps]));
    }

    /// Faults wait for their pages together, on one thread, and each completes once its page is
    /// supplied.
    #[test]
    fn faults_wait_concurrently() {
        let pager = TestSource::paged(2);
        let (dax, window) = Dax::<()>::new(2 * page_size(), None).unwrap();
        dax.window.map(0, &pager.source(), 0, 2 * page_size()).unwrap();
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());

        let mut first = Box::pin(window.fault(0, Access::Read));
        let mut second = Box::pin(window.fault(page_size(), Access::Read));
        assert!(first.poll_unpin(&mut cx).is_pending());
        assert!(second.poll_unpin(&mut cx).is_pending());
        let mut requests = pager.requests();
        requests.sort();
        assert_eq!(requests, [0, 1]);

        pager.supply(1, 2);
        assert_eq!(second.poll_unpin(&mut cx), Poll::Ready(Ok(())));
        assert!(first.poll_unpin(&mut cx).is_pending());
        pager.supply(0, 1);
        assert_eq!(first.poll_unpin(&mut cx), Poll::Ready(Ok(())));
        assert!(window.is_mapped(0) && window.is_mapped(page_size()));
        // SAFETY: Both pages are mapped, and nothing else accesses them.
        let bytes = unsafe { [*window.host_base(), *window.host_base().add(page_size() as usize)] };
        assert_eq!(bytes, [1, 2]);
    }

    /// How a test's second stage has a page of the window mapped: read-only, a source's; or
    /// writable, a private page (backlog doc-13).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Protection {
        Read,
        ReadWrite,
    }

    /// A second stage that keeps the window's pages as the window maps them in it, and the ranges
    /// the window maps anew. fxdax: and its private pages, with what each was made of (backlog
    /// doc-13).
    #[derive(Default)]
    pub(crate) struct Recorded {
        pages: std::sync::Mutex<BTreeMap<u64, PhysLocation>>,
        private: std::sync::Mutex<BTreeMap<u64, Option<PhysLocation>>>,
        mapped: std::sync::Mutex<Vec<(u64, u64)>>,
    }

    impl Recorded {
        /// The pages mapped, by their numbers, read-only: fxdax: private pages too, writable.
        pub fn pages(&self) -> Vec<(u64, Protection)> {
            let pages = self.pages.lock().unwrap();
            let private = self.private.lock().unwrap();
            let mut all: Vec<(u64, Protection)> = (pages.keys())
                .map(|offset| (*offset, Protection::Read))
                .chain(private.keys().map(|offset| (*offset, Protection::ReadWrite)))
                .collect();
            all.sort_by_key(|(offset, _)| *offset);
            all.into_iter().map(|(offset, protection)| (offset / page_size(), protection)).collect()
        }

        /// Where the pages mapped are, by their numbers: those of physical memory alone.
        pub fn locations(&self) -> Vec<(u64, PhysLocation)> {
            let pages = self.pages.lock().unwrap();
            pages.iter().map(|(offset, location)| (offset / page_size(), *location)).collect()
        }

        /// fxdax: the private pages, by their numbers, with the page each copied, if any.
        pub fn private(&self) -> Vec<(u64, Option<PhysLocation>)> {
            let private = self.private.lock().unwrap();
            private.iter().map(|(offset, from)| (offset / page_size(), *from)).collect()
        }

        fn each_page(offset: u64, len: u64) -> impl Iterator<Item = u64> {
            (offset..offset + len).step_by(page_size() as usize)
        }
    }

    impl SecondStage for Recorded {
        fn map(&self, offset: u64, phys: PhysLocation, len: u64) {
            let mut pages = self.pages.lock().unwrap();
            let private = self.private.lock().unwrap();
            for page in Self::each_page(offset, len) {
                let at = PhysLocation { arena: phys.arena, offset: phys.offset + (page - offset) };
                let before = pages.insert(page, at);
                assert!(before.is_none(), "{page:#x} mapped twice");
                assert!(!private.contains_key(&page), "{page:#x} mapped over a private page");
            }
        }

        fn unmap(&self, offset: u64, len: u64) {
            let mut pages = self.pages.lock().unwrap();
            let mut private = self.private.lock().unwrap();
            for page in Self::each_page(offset, len) {
                let unmapped = pages.remove(&page).is_some() || private.remove(&page).is_some();
                assert!(unmapped, "{page:#x} unmapped, not mapped");
            }
        }

        fn window_mapped(&self, offset: u64, len: u64) {
            self.mapped.lock().unwrap().push((offset, len));
        }

        fn make_private(&self, offset: u64, from: Option<PhysLocation>) {
            // In place of what is mapped there.
            self.pages.lock().unwrap().remove(&offset);
            let before = self.private.lock().unwrap().insert(offset, from);
            assert!(before.is_none(), "{offset:#x} made private twice");
        }
    }

    /// The byte at the start of page `page` of the window, which a load faults in first: of a
    /// window with no second stage, whose host memory the window maps.
    fn load(window: &DaxWindow, page: u64) -> u8 {
        futures::executor::block_on(window.fault(page * page_size(), Access::Read)).unwrap();
        // SAFETY: The page is mapped, and nothing else accesses it.
        unsafe { *window.host_base().add((page * page_size()) as usize) }
    }

    /// The second stage, where alone the window's pages are mapped, maps the pages the window maps
    /// and unmaps those it unmaps; it is told of each range the window maps anew.
    #[test]
    fn second_stages_follow_the_windows_pages() {
        let ps = page_size();
        let stage = Arc::new(Recorded::default());
        let (mut dax, window) = Dax::<()>::new(4 * ps, Some(stage.clone())).unwrap();
        assert_eq!(dax.window.host_base(), None);
        let source = TestSource::paged(4);
        source.supply(0, 1);
        dax.map(0, (), &source.source(), 0, 4 * ps).unwrap();
        assert_eq!(*stage.mapped.lock().unwrap(), [(0, 4 * ps)]);
        assert_eq!(stage.pages(), [(0, Protection::Read)]);
        let mut fault = Box::pin(window.fault(2 * ps + 5, Access::Read));
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        assert!(fault.poll_unpin(&mut cx).is_pending());
        assert_eq!(source.requests(), [2]);
        source.supply(2, 3);
        assert_eq!(fault.poll_unpin(&mut cx), Poll::Ready(Ok(())));
        assert_eq!(stage.pages(), [(0, Protection::Read), (2, Protection::Read)]);
        let store = futures::executor::block_on(window.fault(ps, Access::Write));
        assert_eq!(store, Err(Errno::EACCES));
        dax.unmap(0, ps).unwrap();
        assert_eq!(stage.pages(), [(2, Protection::Read)]);
        assert!(!window.is_mapped(0) && window.is_mapped(ps));
        let unmapped = futures::executor::block_on(window.fault(0, Access::Read));
        assert_eq!(unmapped, Err(Errno::ENOENT));

        // Mapped anew where pages were, its pages in memory are mapped at once.
        dax.map(2 * ps, (), &TestSource::numbered(2, 1).source(), 0, 2 * ps).unwrap();
        assert_eq!(*stage.mapped.lock().unwrap(), [(0, 4 * ps), (2 * ps, 2 * ps)]);
        assert_eq!(stage.pages(), [(2, Protection::Read), (3, Protection::Read)]);
    }

    /// Pages consecutive in the window and in physical memory are mapped in one change of the
    /// second stage.
    #[test]
    fn consecutive_pages_are_mapped_together() {
        let ps = page_size();
        let first = physmem::alloc(physmem::ArenaId::DEFAULT, physmem::Wait::No).unwrap();
        let second = physmem::alloc_after(&first).expect("the page after a page just allocated");
        let at = first.location();
        let source = TestSource::new(Kind::Filled, vec![Some(first), Some(second)]);
        let stage = Arc::new(Recorded::default());
        let (mut dax, _window) = Dax::<()>::new(2 * ps, Some(stage.clone())).unwrap();
        dax.map(0, (), &source.source(), 0, 2 * ps).unwrap();
        let next = PhysLocation { arena: at.arena, offset: at.offset + ps };
        assert_eq!(stage.locations(), [(0, at), (1, next)]);
        assert_eq!(stage.pages(), [(0, Protection::Read), (1, Protection::Read)]);
    }

    /// Unmapping part of a mapping leaves what is around it.
    #[test]
    fn ranges_are_unmapped_where_they_are() {
        let ps = page_size();
        let pager = TestSource::paged(4);
        let stage = Arc::new(Recorded::default());
        let (mut dax, window) = Dax::<()>::new(4 * ps, Some(stage.clone())).unwrap();
        dax.map(0, (), &pager.source(), 0, 4 * ps).unwrap();
        assert_eq!(stage.pages(), []);
        pager.supply(2, 7);
        futures::executor::block_on(window.fault(2 * ps, Access::Read)).unwrap();
        assert_eq!(stage.pages(), [(2, Protection::Read)]);

        dax.unmap(2 * ps, ps).unwrap();
        assert_eq!(stage.pages(), []);
        assert!(!window.is_mapped(2 * ps) && window.is_mapped(ps) && window.is_mapped(3 * ps));
        let mappings: Vec<_> = dax.mappings().map(|(start, end, _)| (start, end)).collect();
        assert_eq!(mappings, [(0, 2 * ps), (3 * ps, 4 * ps)]);
    }

    /// The windows' gauges (`live`) follow their mappings, split and unmapped, and their pages,
    /// mapped and unmapped, and drop to nothing as the windows go. Each test runs in a process of
    /// its own (nextest), whose windows are its own.
    #[test]
    fn live_gauges_follow_the_windows_mappings_and_pages() {
        let ps = page_size();
        let gauges = |mappings, mapped_pages: u64, pages| Live {
            mappings,
            mapped_bytes: mapped_pages * ps,
            pages,
            private_pages: 0,
        };
        let (source, other) = (TestSource::numbered(4, 1), TestSource::paged(2));
        let (mut dax, window) =
            Dax::<()>::new(6 * ps, Some(Arc::new(Recorded::default()))).unwrap();
        let (mut second, _) = Dax::<()>::new(2 * ps, Some(Arc::new(Recorded::default()))).unwrap();
        assert_eq!(live(), Live::default());

        dax.map(0, (), &source.source(), 0, 4 * ps).unwrap();
        dax.map(4 * ps, (), &other.source(), 0, 2 * ps).unwrap();
        second.map(0, (), &TestSource::numbered(2, 5).source(), 0, 2 * ps).unwrap();
        assert_eq!(live(), gauges(3, 8, 6));

        other.supply(1, 3);
        futures::executor::block_on(window.fault(5 * ps, Access::Read)).unwrap();
        assert_eq!(live(), gauges(3, 8, 7));

        // The first mapping split in two around the page unmapped.
        dax.unmap(ps, ps).unwrap();
        assert_eq!(live(), gauges(4, 7, 6));

        drop(second);
        assert_eq!(live(), gauges(3, 5, 4));
        drop((dax, window));
        assert_eq!(live(), Live::default());
    }

    /// A source's pages unmapped by it, as it evicts them, are unmapped where a window maps them
    /// alone: the other pages, and other sources', stay mapped.
    #[test]
    fn a_sources_evicted_pages_are_unmapped_from_the_window() {
        let ps = page_size();
        let (source, other) = (TestSource::numbered(4, 1), TestSource::numbered(2, 9));
        let stage = Arc::new(Recorded::default());
        let (mut dax, _window) = Dax::<()>::new(6 * ps, Some(stage.clone())).unwrap();
        dax.map(0, (), &source.source(), 0, 4 * ps).unwrap();
        dax.map(4 * ps, (), &other.source(), 0, 2 * ps).unwrap();
        let frames: Vec<u32> = {
            let mut frames = Vec::new();
            source.each_present(1..3, &mut |_, frame| frames.push(frame.frame()));
            frames
        };
        dax.window.unmap_pages_of(source_identity(&source), &|frame| frames.contains(&frame));
        let pages: Vec<u64> = stage.pages().into_iter().map(|(page, _)| page).collect();
        assert_eq!(pages, [0, 3, 4, 5]);
    }

    /// The ranges a freed extent mapped map zeros, read-only; the others keep what they map.
    #[test]
    fn freed_extents_ranges_map_zeros() {
        let ps = page_size();
        let source = TestSource::numbered(8, 1).source();
        let zeros = crate::cache::zeros().unwrap();
        let (mut dax, window) = Dax::<Arc<u8>>::new(8 * ps, None).unwrap();
        let (freed, other, zero) = (Arc::new(0), Arc::new(1), Arc::new(2));
        dax.map(0, freed.clone(), &source, 0, 2 * ps).unwrap();
        dax.map(2 * ps, other.clone(), &source, 2 * ps, 2 * ps).unwrap();
        dax.map(4 * ps, freed.clone(), &source, 4 * ps, 2 * ps).unwrap();
        dax.replace_kept(freed.identity(), zero.clone(), &zeros).unwrap();

        let loaded: Vec<u8> = (0..6).map(|page| load(&window, page)).collect();
        assert_eq!(loaded, [0, 0, 3, 4, 0, 0]);
        let store = futures::executor::block_on(window.fault(4 * ps, Access::Write));
        assert_eq!(store, Err(Errno::EACCES));
        assert_eq!(dax.kept[&zero.identity()], BTreeSet::from([0, 4 * ps]));
    }

    /// Ranges of a window are page aligned, of some pages, and within it.
    #[test]
    fn ranges_are_whole_pages_within_the_window() {
        let ps = page_size();
        let (dax, window) = Dax::<()>::new(4 * ps, None).unwrap();
        assert_eq!(window.size(), 4 * ps);
        let shown =
            format!("DaxWindow {{ host_base: {:?}, size: {} }}", Some(window.host_base()), 4 * ps);
        assert_eq!(format!("{window:?}"), shown);
        assert_eq!(dax.check_range(0, 4 * ps), Ok(()));
        assert_eq!(dax.check_range(3 * ps, ps), Ok(()));
        for (offset, len) in
            [(0, 0), (1, ps), (0, ps + 1), (ps, 4 * ps), (4 * ps, ps), (u64::MAX - ps + 1, ps)]
        {
            assert_eq!(dax.check_range(offset, len), Err(Errno::EINVAL), "{offset:#x}+{len:#x}");
        }
        assert_eq!(Dax::<()>::new(ps + 1, None).err(), Some(Errno::EINVAL));
        assert_eq!(Dax::<()>::new(0, None).err(), Some(Errno::EINVAL));
    }

    /// fxdax: a window of `pages` pages that copies on write, holding `most` private pages at
    /// most, of the default arena, through `stage`, or in this process's memory without one.
    pub(crate) fn copying<K: Keep>(
        pages: u64,
        most: u64,
        stage: Option<Arc<dyn SecondStage>>,
    ) -> (Dax<K>, DaxWindow) {
        let copying = CopyOnWrite::reserve(crate::pages::ArenaId::DEFAULT, most).unwrap();
        Dax::copying(pages * page_size(), stage, Some(copying)).unwrap()
    }

    /// The byte at `at` in page `page` of a window with no second stage, as it maps it now, after
    /// storing `write` there, if given.
    fn byte_of(window: &DaxWindow, page: u64, at: usize, write: Option<u8>) -> u8 {
        // SAFETY: The page is mapped, as the test faulted it in, and nothing else accesses it.
        unsafe {
            let byte = window.host_base().add((page * page_size()) as usize + at);
            if let Some(value) = write {
                byte.write_volatile(value);
            }
            byte.read_volatile()
        }
    }

    /// The store fault at `offset` of `window`, resolved.
    fn store(window: &DaxWindow, offset: u64) -> Result<()> {
        futures::executor::block_on(window.fault(offset, Access::Write))
    }

    /// fxdax TASK-164.1 (backlog doc-13): in a window that copies on write, a store to a page
    /// mapped read-only, a sealed extent's, gives it a private copy, holding what the page held,
    /// mapped writable in its place, once; the source's page is as it was, as another mapping of
    /// it reads. A store to zeros gives the page a private page of zeros, the shared zeros as they
    /// were. A window that doesn't copy, a follower's, refuses the store (`EACCES`).
    #[test]
    fn stores_to_read_only_pages_copy_them_once() {
        let ps = page_size();
        let source = TestSource::numbered(2, 1);
        let zeros = crate::cache::zeros().unwrap();
        let (mut dax, window) = copying::<()>(4, 3, None);
        dax.map(0, (), &source.source(), 0, 2 * ps).unwrap();
        dax.map(2 * ps, (), &zeros, 0, ps).unwrap();
        assert_eq!(load(&window, 1), 2);

        assert_eq!(store(&window, ps + 5), Ok(()));
        assert_eq!(window.private_pages(), 1);
        assert_eq!(byte_of(&window, 1, 0, None), 2, "the copy holds other bytes");
        assert_eq!(byte_of(&window, 1, 7, Some(9)), 9);
        dax.map(3 * ps, (), &source.source(), ps, ps).unwrap();
        assert_eq!(load(&window, 3), 2);
        assert_eq!(byte_of(&window, 3, 7, None), 2, "the store reached the source's page");
        // A second store's fault finds the page private, and copies nothing.
        assert_eq!(store(&window, ps), Ok(()));
        assert_eq!(window.private_pages(), 1);
        assert_eq!(byte_of(&window, 1, 7, None), 9);

        assert_eq!(store(&window, 2 * ps), Ok(()));
        assert_eq!(window.private_pages(), 2);
        assert_eq!(byte_of(&window, 2, 100, Some(4)), 4);
        assert_eq!(byte_of(&window, 2, 101, None), 0);
        let mut shared = 1;
        // SAFETY: The zeros' page is in memory while its lock is held.
        zeros.each_present(0..1, &mut |_, frame| shared = unsafe { *frame.as_ptr().add(100) });
        assert_eq!(shared, 0, "the store reached the shared zeros");

        let (mut plain, plain_window) = Dax::<()>::new(2 * ps, None).unwrap();
        plain.map(0, (), &source.source(), 0, ps).unwrap();
        assert_eq!(store(&plain_window, 0), Err(Errno::EACCES));
        assert_eq!(store(&plain_window, ps), Err(Errno::ENOENT), "a store to nothing mapped");
        assert_eq!(plain_window.private_pages(), 0);
    }

    /// fxdax TASK-164.1: a window's private pages are the guest's until its own `MAP` or `UNMAP`
    /// over them ends them: a source evicting its pages leaves those within its mappings, which
    /// aren't its own, and so does a free mapping zeros where the freed extent was mapped. Shown
    /// red by unmap_pages_of taking private pages too (`Rules::eviction_skips_private`), or
    /// replace_kept mapping with `Private::Free`.
    #[test]
    fn private_pages_end_with_the_guests_maps_alone() {
        let ps = page_size();
        let (old, new) = (TestSource::numbered(4, 1), TestSource::numbered(4, 0x11));
        let stage = Arc::new(Recorded::default());
        let (mut dax, window) = copying::<Arc<u8>>(4, 4, Some(stage.clone()));
        let (kept, freed, zero) = (Arc::new(0), Arc::new(1), Arc::new(2));
        dax.map(0, kept.clone(), &old.source(), 0, 2 * ps).unwrap();
        dax.map(2 * ps, freed.clone(), &old.source(), 2 * ps, 2 * ps).unwrap();
        for page in 0..4 {
            assert_eq!(store(&window, page * ps), Ok(()), "page {page}");
        }
        assert_eq!(window.private_pages(), 4);
        let writable: Vec<_> = (0..4).map(|page| (page, Protection::ReadWrite)).collect();
        assert_eq!(stage.pages(), writable);
        // Each a copy of the source's page, which is mapped no more.
        let mut sources = Vec::new();
        old.each_present(0..4, &mut |page, frame| sources.push((page, Some(frame.location()))));
        assert_eq!(stage.private(), sources);
        assert_eq!(stage.locations(), [], "a source's page mapped");

        // Every page of the source evicted.
        dax.window.unmap_pages_of(source_identity(&old), &|_| true);
        assert_eq!(stage.private(), sources, "evicted");
        dax.replace_kept(freed.identity(), zero, &crate::cache::zeros().unwrap()).unwrap();
        assert_eq!(stage.private(), sources, "freed");
        assert_eq!(stage.locations(), [], "zeros mapped over private pages");
        assert_eq!(window.private_pages(), 4);

        dax.map(0, kept, &new.source(), 0, ps).unwrap();
        assert_eq!(window.private_pages(), 3);
        dax.unmap(3 * ps, ps).unwrap();
        assert_eq!(window.private_pages(), 2);
        let pages = [(0, Protection::Read), (1, Protection::ReadWrite), (2, Protection::ReadWrite)];
        assert_eq!(stage.pages(), pages);
        assert_eq!(stage.private(), sources[1..3]);
        let mut first = None;
        new.each_present(0..1, &mut |_, frame| first = Some(frame.location()));
        assert_eq!(stage.locations(), [(0, first.unwrap())]);
        assert_eq!(live().private_pages, 2);
        drop((dax, window));
        assert_eq!(live().private_pages, 0);
    }

    /// fxdax TASK-164.1: past its private pages, a window refuses a store to a read-only page, as
    /// one that doesn't copy does (`EACCES`, which stops the VM), the page read as it was; an
    /// `UNMAP` of a private page makes room for another.
    #[test]
    fn stores_past_the_private_pages_are_refused() {
        let ps = page_size();
        let source = TestSource::numbered(2, 1);
        let (mut dax, window) = copying::<()>(2, 1, None);
        dax.map(0, (), &source.source(), 0, 2 * ps).unwrap();
        assert_eq!(store(&window, 0), Ok(()));
        assert_eq!(store(&window, ps), Err(Errno::EACCES));
        assert_eq!(window.private_pages(), 1);
        assert_eq!(load(&window, 1), 2);
        dax.unmap(0, ps).unwrap();
        assert_eq!(store(&window, ps), Ok(()));
        assert_eq!(window.private_pages(), 1);
    }

    /// fxdax TASK-164.1: `PRIVATE` makes a range's pages private at once, mapped writable, those
    /// private already counted once; past the window's private pages it makes none
    /// (`ENOMEM`). A window that doesn't copy on write takes none (`EINVAL`), nor a range with a
    /// page mapped nothing at (`EINVAL`, as guest/sim's host has it).
    #[test]
    fn ranges_are_made_private_at_once_within_the_bound() {
        let ps = page_size();
        let source = TestSource::numbered(4, 1);
        let stage = Arc::new(Recorded::default());
        let (mut dax, window) = copying::<()>(4, 3, Some(stage.clone()));
        dax.map(0, (), &source.source(), 0, 3 * ps).unwrap();
        let private = |offset, len| futures::executor::block_on(window.make_private(offset, len));
        assert_eq!(private(0, 4 * ps), Err(Errno::EINVAL));
        dax.map(3 * ps, (), &source.source(), 3 * ps, ps).unwrap();
        assert_eq!(private(0, 4 * ps), Err(Errno::ENOMEM));
        assert_eq!(window.private_pages(), 0);

        assert_eq!(private(ps, 2 * ps), Ok(()));
        let (read, written) = (Protection::Read, Protection::ReadWrite);
        assert_eq!(stage.pages(), [(0, read), (1, written), (2, written), (3, read)]);
        assert_eq!(private(ps, 2 * ps), Ok(()));
        assert_eq!(window.private_pages(), 2);
        assert_eq!(private(0, ps), Ok(()));
        assert_eq!(private(3 * ps, ps), Err(Errno::ENOMEM));
        assert_eq!(private(4 * ps, ps), Err(Errno::EINVAL));

        let (mut plain, plain_window) = Dax::<()>::new(ps, None).unwrap();
        plain.map(0, (), &source.source(), 0, ps).unwrap();
        let refused = futures::executor::block_on(plain_window.make_private(0, ps));
        assert_eq!(refused, Err(Errno::EINVAL));
    }

    /// fxdax TASK-164.1: a window's private pages are reserved against its arena as it is made,
    /// all of them, and let go of as it goes, so that a store never waits for memory; a bound of
    /// none, or of more pages than the window has, is refused.
    #[test]
    fn private_pages_are_reserved_as_windows_are_made() {
        let (ps, arena) = (page_size(), crate::pages::ArenaId::DEFAULT);
        let before = physmem::pinned_bytes(arena);
        let (dax, window) = copying::<()>(8, 4, None);
        assert_eq!(physmem::pinned_bytes(arena) - before, 4 * ps);
        drop((dax, window));
        assert_eq!(physmem::pinned_bytes(arena), before);

        let too_many = CopyOnWrite::reserve(arena, 9).unwrap();
        assert_eq!(Dax::<()>::copying(8 * ps, None, Some(too_many)).err(), Some(Errno::EINVAL));
        assert_eq!(physmem::pinned_bytes(arena), before);
        assert_eq!(CopyOnWrite::reserve(arena, 0).err(), Some(Errno::EINVAL));
    }
}
