// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! DAX windows: guest physical memory into which an extent-store device maps extents, so that the
//! guest's Fxfs reads and writes their pages directly instead of copying them through requests.
//!
//! A window is a range of guest physical memory, whose mappings each map a range of a [`Source`]'s
//! pages (`crate::cache`): a sealed extent's cached pages, shared by every window that maps it,
//! which are read in from the store as they are first needed; an open extent's, the memory the
//! guest writes it in until a commit seals it; or zeros. The guest's loads and stores fault pages
//! in. The window keeps the pages it has mapped, each a page of physical memory (`crate::pages`),
//! as page tables do; a source evicting a page unmaps it from each window that maps it first.
//!
//! The window maps its pages in its [`SecondStage`], if it has one: its translation outside the
//! daemon. A VM's window is the extent daemon's (`crate::extentd`), whose second stage is its
//! connection to the VM process, which makes each change in the VM's memory. KVM maps guest memory
//! from the host memory backing it, and follows changes to it: the VM process's own stage
//! (`dax::uffd`) takes that memory over, maps the window's pages into it, and reports the faults
//! KVM takes there itself, with userfaultfd, to the daemon, which passes them to the window
//! ([`DaxWindow::fault`]). A window without a second stage, as tests' are, has its pages mapped at
//! its host address.

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

/// The guest's access to a page of a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Protection {
    /// Loads and instruction fetches. Stores must fault.
    Read,
    /// Loads, stores and instruction fetches.
    ReadWrite,
}

/// A window's second stage: a translation of it outside the daemon, which follows its mappings,
/// such as the extent daemon's connection to a VM process, which makes them in the VM's memory.
///
/// Each change the window makes to its pages is made here, rather than in this process's memory,
/// with offsets into the window. Calls for a window are serialized. They cannot fail: if the
/// hypervisor refuses one, the guest's memory no longer matches the window's, and the
/// implementation must panic.
pub trait SecondStage: Send + Sync {
    /// Maps the `len` bytes at `offset`, where nothing is mapped, to the pages at `phys` of
    /// physical memory, consecutive in one arena's object
    /// ([`arena_memory`](crate::pages::arena_memory)), from which another process can map them.
    fn map(&self, offset: u64, phys: PhysLocation, len: u64, protection: Protection);

    /// Changes the protection of the mapped `len` bytes at `offset`.
    fn protect(&self, offset: u64, len: u64, protection: Protection);

    /// Unmaps the mapped `len` bytes at `offset`.
    fn unmap(&self, offset: u64, len: u64);

    /// fxdax: the window maps the `len` bytes at `offset` anew; called once it has, before any of
    /// the pages it maps there now are mapped here, as the window maps a page here only once it is
    /// in memory. A stage that withholds pages a fault found nothing at (`dax::uffd`) gives them
    /// back, so that accesses to them fault again.
    fn window_mapped(&self, _offset: u64, _len: u64) {}

    /// fxdax: gathers what waits for the changes this thread makes from now on that take access
    /// away, until [`end_deferring`](Self::end_deferring). Only a stage that makes them after it
    /// returns, as one in another process does (`crate::extentd`), defers: the pages it took away,
    /// the guest may reach still, which mustn't be shared with another guest until then. Freed,
    /// they wait for it themselves (`physmem::quarantine_with`, TASK-79.9).
    fn begin_deferring(&self) {}

    /// fxdax: ends [`begin_deferring`](Self::begin_deferring): what waits for the changes it
    /// deferred, if there were any.
    fn end_deferring(&self) -> Option<Arc<dyn Deferred>> {
        None
    }

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
                Anew::Map { offset, phys, len, protection } => {
                    self.map(offset, phys, len, protection)
                }
            }
        }
    }
}

/// fxdax: what a window's change makes of a range it unmapped ([`SecondStage::remap`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anew {
    /// The window maps the range anew ([`SecondStage::window_mapped`]).
    Mapped { offset: u64, len: u64 },
    /// It maps the pages there ([`SecondStage::map`]).
    Map { offset: u64, phys: PhysLocation, len: u64, protection: Protection },
}

/// fxdax: changes to a [`SecondStage`] that take access away, deferred: until they are made, the
/// guest may still reach the pages, which mustn't be shared with another guest, or freed, until
/// then.
pub trait Deferred: Send + Sync {
    /// Waits for the changes to be made.
    fn wait(&self);

    /// Keeps `keep`, memory the guest may still reach, until the changes are made.
    fn keep_until_made(&self, keep: Box<dyn Send>);
}

/// A hypervisor that maps a window's host memory for the guest itself, such as KVM, which the
/// window tells of the pages it maps there, so that it can map them for the guest ahead of the
/// guest's accesses rather than on a fault on each page, as KVM's `KVM_PRE_FAULT_MEMORY` does.
#[cfg(target_os = "linux")]
pub trait Prefault: Send + Sync {
    /// Maps the `len` bytes at `offset` into the window for the guest, which the window has just
    /// mapped in its host memory, writable (fxdax: pages read-only are left to the guest's
    /// accesses, TASK-109, but those it asks for to read next, TASK-112.1). It is called with the
    /// window's mappings locked, and must not wait on the window, or for the guest.
    fn prefault(&self, offset: u64, len: u64);
}

/// What resolves the faults on a window's second stage in host memory: the VM process's connection
/// to the extent daemon, which holds the window (`crate::extentd`), or the window itself, in tests
/// whose window is in this process's host memory (`Dax::in_host_memory`).
#[cfg(target_os = "linux")]
pub(crate) trait Faults: Send + Sync {
    /// The window's size in bytes.
    fn size(&self) -> u64;

    /// As [`DaxWindow::fault`].
    fn fault(&self, offset: u64, access: Access) -> BoxFuture<'_, Result<()>>;

    /// Resolves a fault at `offset` for `access` on that page alone, for a fault whose access may
    /// not be the guest's.
    fn fault_page(&self, offset: u64, access: Access) -> BoxFuture<'_, Result<()>>;
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
        Box::pin(Window::fault_page(self, offset, access))
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

/// What a window's mapping maps pages of (`crate::cache`): an extent's pages, sealed or open, or
/// zeros. Its pages are numbered from its first, each a page of physical memory once it is in
/// memory, which the source keeps until it lets go of it: a page it evicts, it first unmaps from
/// each window it was told maps it ([`Window::unmap_pages_of`]).
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
    /// the access.
    ///
    /// Completes once the page is in memory, holding no thread meanwhile. Fails if nothing is
    /// mapped there, with `ENOENT`, or with `EACCES` for a store to a read-only mapping; the guest
    /// cannot go on.
    pub async fn fault(&self, offset: u64, access: Access) -> Result<()> {
        self.0.fault(offset, access).await
    }

    /// Resolves a fault at `offset` for `access` on that page alone, for a fault whose access may
    /// not be the guest's.
    #[cfg(target_os = "linux")]
    pub(crate) async fn fault_page(&self, offset: u64, access: Access) -> Result<()> {
        self.0.fault_page(offset, access).await
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
}

impl std::fmt::Debug for DaxWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaxWindow")
            .field("host_base", &self.0.host_base().map(|base| base as *mut u8))
            .field("size", &self.size())
            .finish()
    }
}

/// How many pages a store fault that follows on from the last brings in: its own and those after
/// it, where a sequential writer stores next, mapped together rather than one fault each.
const SEQUENTIAL_STORE_PAGES: usize = 16;

/// Where a window's pages are mapped: in exactly one place.
enum Translation {
    /// In its second stage alone, another process's (a VMM's) or another range of this one's.
    Stage(Arc<dyn SecondStage>),
    /// In host memory: a reservation of host address space, at `base`, as long as the window.
    Host { base: usize },
}

/// A mapping of a window: a range of its source's pages, from `page`, up to the window's `end`.
struct Mapping {
    end: u64,
    source: Arc<dyn Source>,
    page: u64,
    writable: bool,
}

/// A page the window has mapped: where it is in physical memory, and which page of it it is.
#[derive(Clone, Copy)]
struct Pte {
    location: PhysLocation,
    frame: u32,
}

/// A window's mappings and the pages it has mapped, under its lock.
#[derive(Default)]
struct WindowState {
    /// The mapped ranges of the window, by start. They do not overlap.
    mappings: BTreeMap<u64, Mapping>,
    /// The starts of the mappings, by the identity of their source ([`source_identity`]).
    by_source: HashMap<usize, BTreeSet<u64>>,
    /// The pages mapped, by offset: each within a mapping, and its source's.
    ptes: BTreeMap<u64, Pte>,
}

/// The process's windows' mappings, their bytes, and the pages they have mapped, all windows'
/// together, as their states change: the extent daemon's gauges (`crate::extentd::metrics`,
/// TASK-131.6).
static LIVE_MAPPINGS: AtomicU64 = AtomicU64::new(0);
static LIVE_MAPPED_BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE_PTES: AtomicU64 = AtomicU64::new(0);

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
}

/// The process's windows' mappings, as of now.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn live() -> Live {
    Live {
        mappings: LIVE_MAPPINGS.load(Ordering::Relaxed),
        mapped_bytes: LIVE_MAPPED_BYTES.load(Ordering::Relaxed),
        pages: LIVE_PTES.load(Ordering::Relaxed),
    }
}

impl Drop for WindowState {
    fn drop(&mut self) {
        let bytes: u64 = self.mappings.iter().map(|(start, mapping)| mapping.end - start).sum();
        LIVE_MAPPINGS.fetch_sub(self.mappings.len() as u64, Ordering::Relaxed);
        LIVE_MAPPED_BYTES.fetch_sub(bytes, Ordering::Relaxed);
        LIVE_PTES.fetch_sub(self.ptes.len() as u64, Ordering::Relaxed);
    }
}

impl WindowState {
    /// Records that the window has mapped `pte` at `offset`, where it had mapped nothing.
    fn insert_pte(&mut self, offset: u64, pte: Pte) {
        let previous = self.ptes.insert(offset, pte);
        debug_assert!(previous.is_none(), "a page mapped over another at {offset:#x}");
        LIVE_PTES.fetch_add(1, Ordering::Relaxed);
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
    /// Where a store fault following on from the last would be: the page after those it brought
    /// in.
    next_store: AtomicU64,
}

impl Window {
    /// A window of `size` bytes, which must be a multiple of the page size, whose pages are mapped
    /// in `second_stage`, or, without one, in this process's memory.
    fn new(size: u64, second_stage: Option<Arc<dyn SecondStage>>) -> Result<Self> {
        if size == 0 || !size.is_multiple_of(page_size()) {
            return Err(Errno::EINVAL);
        }
        let len = usize::try_from(size).map_err(|_| Errno::EINVAL)?;
        let translation = match second_stage {
            Some(stage) => Translation::Stage(stage),
            None => Translation::Host { base: reserve(None, len).map_err(|_| Errno::ENOMEM)? },
        };
        let (state, next_store) = (Mutex::new(WindowState::default()), AtomicU64::new(u64::MAX));
        Ok(Self { size, translation, state, next_store })
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

    /// Resolves the guest's fault at `offset` for `access`. A store that follows on from the last
    /// store fault brings in the pages after it too, up to `SEQUENTIAL_STORE_PAGES` in all.
    pub(crate) async fn fault(&self, offset: u64, access: Access) -> Result<()> {
        let page = offset - offset % page_size();
        if access == Access::Read {
            return self.fault_pages(page, access, 1).await;
        }
        let pages = if self.next_store.load(Ordering::Relaxed) == page {
            SEQUENTIAL_STORE_PAGES
        } else {
            1
        };
        self.fault_pages(page, access, pages).await?;
        self.next_store.store(page + (pages as u64) * page_size(), Ordering::Relaxed);
        Ok(())
    }

    /// Resolves a fault at `offset` for `access` on that page alone, for a fault whose access may
    /// not be the guest's. A store fault on the next page follows on from it.
    #[cfg(target_os = "linux")]
    pub(crate) async fn fault_page(&self, offset: u64, access: Access) -> Result<()> {
        let page = offset - offset % page_size();
        self.fault_pages(page, access, 1).await?;
        if access == Access::Write {
            self.next_store.store(page + page_size(), Ordering::Relaxed);
        }
        Ok(())
    }

    /// Resolves a fault at `offset` for `access` on that page and the `pages - 1` after it, as far
    /// as what is mapped there goes.
    async fn fault_pages(&self, offset: u64, access: Access, pages: usize) -> Result<()> {
        use crate::stats::{self, Counter};
        let start = std::time::Instant::now();
        let faulted = self.resolve(offset, access, pages).await;
        stats::count_since(Counter::Faults, Counter::FaultNs, start);
        if faulted == Ok(true) {
            stats::add(Counter::FaultWaits, 1);
        }
        faulted.map(|_| ())
    }

    /// Maps the page at `offset`, which a fault for `access` was on, and the `pages - 1` after it
    /// within its mapping, bringing them into memory first, and those its source brought in with
    /// it (`Source::brought_in_with`), and returns whether it waited for the page to be brought
    /// in. A page evicted, or mapped anew, meanwhile is resolved as it is then.
    async fn resolve(&self, offset: u64, access: Access, pages: usize) -> Result<bool> {
        let ps = page_size();
        let mut waited = false;
        loop {
            let (source, page, count, mapped) = {
                let state = self.state.lock().unwrap();
                let (start, mapping) = state.mapping_at(offset).ok_or(Errno::ENOENT)?;
                if access == Access::Write && !mapping.writable {
                    return Err(Errno::EACCES);
                }
                let left = (mapping.end - offset) / ps;
                let count = left.min(pages as u64);
                if count == 1 && state.ptes.contains_key(&offset) {
                    return Ok(waited);
                }
                let page = mapping.page + (offset - start) / ps;
                let mapped = left.min(mapping.source.brought_in_with(page).max(count));
                (mapping.source.clone(), page, count, mapped)
            };
            waited |= source.clone().bring_in(page).await?;
            for next in 1..count {
                // Those after it are brought in as they can be: a fault on one shows what fails.
                if let Err(errno) = source.clone().bring_in(page + next).await {
                    log::debug!("fxdax: bringing in a page after a store fault: {errno:?}");
                    break;
                }
            }
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

    /// Maps `len` bytes of `source`'s pages from `source_offset` into the window at `offset`,
    /// replacing what was mapped there, and then its pages in memory: one change to the second
    /// stage, if there is one (`SecondStage::begin_change`).
    fn map(
        self: &Arc<Self>,
        offset: u64,
        source: &Arc<dyn Source>,
        source_offset: u64,
        len: u64,
        writable: bool,
    ) -> Result<()> {
        let _change = self.stage().map(|stage| Change::begin(stage.as_ref()));
        let end = offset + len;
        let mut state = self.state.lock().unwrap();
        self.unmap_ptes(&mut state, offset, end);
        let forgotten = state.forget(offset, end);
        let identity = source_identity(source);
        let first = !state.by_source.contains_key(&identity);
        let page = source_offset / page_size();
        state.insert(offset, Mapping { end, source: source.clone(), page, writable });
        if first {
            source.mapped(self);
        }
        if let Some(stage) = self.stage() {
            stage.window_mapped(offset, len);
        }
        // Its pages in memory are mapped now, so that the guest's first accesses to them do not
        // fault.
        self.install(&mut state, offset, end);
        self.unmapped(&state, forgotten);
        Ok(())
    }

    /// Maps the pages in memory of what is mapped at `len` bytes of the window at `offset`, so
    /// that the guest's first accesses to them do not fault.
    fn populate(&self, offset: u64, len: u64) {
        let mut state = self.state.lock().unwrap();
        self.install(&mut state, offset, offset + len);
    }

    /// Unmaps `len` bytes of the window at `offset`.
    fn unmap(&self, offset: u64, len: u64) {
        let end = offset + len;
        let mut state = self.state.lock().unwrap();
        self.unmap_ptes(&mut state, offset, end);
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
    /// are of the frames it evicts: before it lets go of them.
    pub(crate) fn unmap_pages_of(&self, source: usize, evicted: &dyn Fn(u32) -> bool) {
        let mut state = self.state.lock().unwrap();
        let Some(starts) = state.by_source.get(&source) else { return };
        let ranges: Vec<(u64, u64)> =
            starts.iter().map(|start| (*start, state.mappings[start].end)).collect();
        let mut gone = Vec::new();
        for (start, end) in ranges {
            gone.extend(
                (state.ptes.range(start..end))
                    .filter(|(_, pte)| evicted(pte.frame))
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
        // The pages to map, in order: their offsets, where they are, and their protections.
        let mut pages: Vec<(u64, PhysLocation, Protection)> = Vec::new();
        for mapping_start in state.overlapping(start, end) {
            let mapping = &state.mappings[&mapping_start];
            let (from, to) = (start.max(mapping_start), end.min(mapping.end));
            let first = mapping.page + (from - mapping_start) / ps;
            let protection =
                if mapping.writable { Protection::ReadWrite } else { Protection::Read };
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
                found.push((offset, Pte { location: frame.location(), frame: frame.frame() }));
            });
            for (offset, pte) in found {
                state.insert_pte(offset, pte);
                pages.push((offset, pte.location, protection));
            }
        }
        let mut run: Option<(u64, PhysLocation, u64, Protection)> = None;
        for (offset, location, protection) in pages {
            match &mut run {
                Some((at, first, len, prot))
                    if *at + *len == offset
                        && first.arena == location.arena
                        && first.offset + *len == location.offset
                        && *prot == protection =>
                {
                    *len += ps;
                }
                _ => {
                    if let Some((at, first, len, prot)) =
                        run.replace((offset, location, ps, protection))
                    {
                        self.map_run(at, first, len, prot);
                    }
                }
            }
        }
        if let Some((at, first, len, prot)) = run {
            self.map_run(at, first, len, prot);
        }
    }

    /// Unmaps the pages the window has mapped of `start..end`.
    fn unmap_ptes(&self, state: &mut WindowState, start: u64, end: u64) {
        let gone: Vec<u64> = state.ptes.range(start..end).map(|(offset, _)| *offset).collect();
        for offset in &gone {
            state.remove_pte(*offset);
        }
        self.unmap_runs(&gone);
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

    /// Maps the `len` bytes at `offset` to the pages at `location`, with `protection`.
    fn map_run(&self, offset: u64, location: PhysLocation, len: u64, protection: Protection) {
        match &self.translation {
            Translation::Stage(stage) => stage.map(offset, location, len, protection),
            Translation::Host { base } => {
                let fd = crate::pages::arena_memory(location.arena).expect("the page's arena");
                let prot = match protection {
                    Protection::Read => libc::PROT_READ,
                    Protection::ReadWrite => libc::PROT_READ | libc::PROT_WRITE,
                };
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
}

impl Rules {
    pub const ALL: Self = Self { free_maps_zeros: true };
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
/// as the open extent's memory or the sealed extent's cached pages that it maps.
pub(crate) struct Dax<K: Keep> {
    window: Arc<Window>,
    /// The mapped ranges of the window, by start: their ends and what keeps them alive. They do
    /// not overlap.
    mappings: BTreeMap<u64, (u64, K)>,
    /// The starts of the mappings, by the identity of what keeps them alive.
    kept: HashMap<usize, BTreeSet<u64>>,
    /// The second stage, if the window has one.
    stage: Option<Arc<dyn SecondStage>>,
    /// The thread serving the faults on the host memory a test's window was created in
    /// ([`Dax::in_host_memory`]), which runs until the window is dropped.
    #[cfg(all(test, target_os = "linux"))]
    _faults: Option<uffd::FaultThread>,
}

impl<K: Keep> Dax<K> {
    /// A window of `size` bytes whose pages are mapped in `second_stage`, or, without one, in this
    /// process's memory, and a handle to it.
    pub fn new(size: u64, second_stage: Option<Arc<dyn SecondStage>>) -> Result<(Self, DaxWindow)> {
        let window = Arc::new(Window::new(size, second_stage.clone())?);
        let dax = Self {
            window: window.clone(),
            mappings: BTreeMap::new(),
            kept: HashMap::new(),
            stage: second_stage,
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
        let (mut dax, window) = Self::new(stage.size(), Some(stage.clone()))?;
        dax._faults = Some(stage.serve(dax.window.clone())?);
        Ok((dax, window))
    }

    /// fxdax: runs `f`, whose changes to the window's mappings are one change to the second stage
    /// ([`SecondStage::begin_change`]): a stage in another process, sent it whole, makes it at once
    /// ([`SecondStage::remap`]), as a range mapped anew and its pages mapped again there are made in
    /// place (TASK-91.16).
    pub fn changing<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let stage = self.stage.clone();
        let _change = stage.as_deref().map(Change::begin);
        f(self)
    }

    /// fxdax: runs `f`, whose changes to the second stage that take access away aren't waited for:
    /// returns what waits for them, if any. Until they are made, the guest may still reach the
    /// pages they take away, which the caller mustn't free or share meanwhile.
    pub fn deferring<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> T,
    ) -> (T, Option<Arc<dyn Deferred>>) {
        /// A deferral, which ends however `f` does.
        struct Deferring(Option<Arc<dyn SecondStage>>);
        impl Deferring {
            fn end(mut self) -> Option<Arc<dyn Deferred>> {
                self.0.take()?.end_deferring()
            }
        }
        impl Drop for Deferring {
            fn drop(&mut self) {
                if let Some(stage) = self.0.take() {
                    stage.end_deferring();
                }
            }
        }
        let deferring = Deferring(self.stage.clone());
        if let Some(stage) = &deferring.0 {
            stage.begin_deferring();
        }
        let result = f(self);
        (result, deferring.end())
    }

    /// Checks the window's range for a mapping of `len` bytes at `offset`.
    pub fn check_range(&self, offset: u64, len: u64) -> Result<()> {
        self.window.check_range(offset, len)
    }

    /// Maps `len` bytes of `source`, whose memory `keep` keeps alive, at `source_offset` into the
    /// window at `offset`.
    pub fn map(
        &mut self,
        offset: u64,
        keep: K,
        source: &Arc<dyn Source>,
        source_offset: u64,
        len: u64,
        writable: bool,
    ) -> Result<()> {
        self.window.map(offset, source, source_offset, len, writable)?;
        let end = offset + len;
        self.forget(offset, end);
        self.insert(offset, end, keep);
        Ok(())
    }

    /// Maps the parts of the window's `len` bytes at `offset` that mappings `is_kept` says are
    /// kept alive by what was mapped there still map, read-only, from `source` at `source_offset`,
    /// kept alive by `keep`: a writable mapping of what has become immutable since, and moved.
    /// Returns the parts, as the window's offsets and lengths.
    pub fn make_read_only(
        &mut self,
        offset: u64,
        len: u64,
        keep: K,
        source: &Arc<dyn Source>,
        source_offset: u64,
        is_kept: impl Fn(&K) -> bool,
    ) -> Result<Vec<(u64, u64)>> {
        let end = offset + len;
        let parts: Vec<(u64, u64)> = self
            .overlapping(offset, end)
            .filter(|(_, _, keep)| is_kept(keep))
            .map(|(start, mapping_end, _)| (start.max(offset), mapping_end.min(end)))
            .collect();
        let mut mapped = Vec::with_capacity(parts.len());
        for (start, part_end) in parts {
            let (source_offset, len) = (source_offset + (start - offset), part_end - start);
            self.map(start, keep.clone(), source, source_offset, len, false)?;
            mapped.push((start, len));
        }
        Ok(mapped)
    }

    /// Unmaps `len` bytes of the window at `offset`.
    pub fn unmap(&mut self, offset: u64, len: u64) -> Result<()> {
        self.window.unmap(offset, len);
        self.forget(offset, offset + len);
        Ok(())
    }

    /// Maps the pages in memory of what is mapped at `len` bytes of the window at `offset`.
    pub fn populate(&self, offset: u64, len: u64) -> Result<()> {
        self.window.populate(offset, len);
        Ok(())
    }

    /// Maps the window's ranges whose mappings keep alive what has the identity `kept`, what a
    /// freed extent mapped, from `zeros` instead, read-only, kept alive by `keep`: their memory
    /// goes with their old mappings. The guest may still touch such a range, as a store racing the
    /// move of what it maps, which faults as on any read-only page; unmapped, its fault would find
    /// nothing to map.
    pub fn replace_kept(&mut self, kept: usize, keep: K, zeros: &Arc<dyn Source>) -> Result<()> {
        let starts: Vec<u64> =
            self.kept.get(&kept).map(|starts| starts.iter().copied().collect()).unwrap_or_default();
        for start in starts {
            let end = self.mappings[&start].0;
            if rules().free_maps_zeros {
                self.window.map(start, zeros, 0, end - start, false)?;
            } else {
                self.window.unmap(start, end - start);
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
        crate::cache::zeros()
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
        /// Each comes in, zeros, as it is first brought in, as an open extent's do.
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

        /// How many of its pages are in memory.
        pub fn present(&self) -> u64 {
            self.pages.lock().unwrap().iter().filter(|page| page.is_some()).count() as u64
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
        let zeros = crate::cache::zeros();
        let (mut dax, _window) = Dax::<Arc<u8>>::new(8 * ps, None).unwrap();
        let (a, b, z) = (Arc::new(0), Arc::new(1), Arc::new(2));
        dax.map(0, a.clone(), &source, 0, 4 * ps, true).unwrap();
        dax.map(4 * ps, b.clone(), &source, 4 * ps, 2 * ps, true).unwrap();
        // Over the middle of `a`'s mapping, which splits around it.
        dax.map(ps, b.clone(), &source, ps, ps, true).unwrap();

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
        dax.window.map(0, &pager.source(), 0, 2 * page_size(), false).unwrap();
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

    /// Changes deferred, which tests need not wait for.
    struct Waited;

    impl Deferred for Waited {
        fn wait(&self) {}

        fn keep_until_made(&self, _keep: Box<dyn Send>) {}
    }

    /// A second stage that keeps the window's pages as the window maps them in it, the ranges the
    /// window maps anew, and its deferrals, each of which defers changes.
    #[derive(Default)]
    pub(crate) struct Recorded {
        pages: std::sync::Mutex<BTreeMap<u64, (PhysLocation, Protection)>>,
        mapped: std::sync::Mutex<Vec<(u64, u64)>>,
        deferrals: std::sync::Mutex<Vec<&'static str>>,
    }

    impl Recorded {
        /// The pages mapped, by their numbers.
        pub fn pages(&self) -> Vec<(u64, Protection)> {
            let pages = self.pages.lock().unwrap();
            pages.iter().map(|(offset, (_, protection))| (offset / page_size(), *protection)).collect()
        }

        /// Where the pages mapped are, by their numbers.
        pub fn locations(&self) -> Vec<(u64, PhysLocation)> {
            let pages = self.pages.lock().unwrap();
            pages.iter().map(|(offset, (location, _))| (offset / page_size(), *location)).collect()
        }

        fn each_page(offset: u64, len: u64) -> impl Iterator<Item = u64> {
            (offset..offset + len).step_by(page_size() as usize)
        }
    }

    impl SecondStage for Recorded {
        fn map(&self, offset: u64, phys: PhysLocation, len: u64, protection: Protection) {
            let mut pages = self.pages.lock().unwrap();
            for page in Self::each_page(offset, len) {
                let at = PhysLocation { arena: phys.arena, offset: phys.offset + (page - offset) };
                let before = pages.insert(page, (at, protection));
                assert!(before.is_none(), "{page:#x} mapped twice");
            }
        }

        fn protect(&self, offset: u64, len: u64, protection: Protection) {
            let mut pages = self.pages.lock().unwrap();
            for page in Self::each_page(offset, len) {
                pages.get_mut(&page).expect("a page mapped").1 = protection;
            }
        }

        fn unmap(&self, offset: u64, len: u64) {
            let mut pages = self.pages.lock().unwrap();
            for page in Self::each_page(offset, len) {
                pages.remove(&page).expect("a page mapped");
            }
        }

        fn window_mapped(&self, offset: u64, len: u64) {
            self.mapped.lock().unwrap().push((offset, len));
        }

        fn begin_deferring(&self) {
            self.deferrals.lock().unwrap().push("begin");
        }

        fn end_deferring(&self) -> Option<Arc<dyn Deferred>> {
            self.deferrals.lock().unwrap().push("end");
            Some(Arc::new(Waited))
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
        dax.map(0, (), &source.source(), 0, 4 * ps, false).unwrap();
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
        dax.map(2 * ps, (), &TestSource::numbered(2, 1).source(), 0, 2 * ps, false).unwrap();
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
        dax.map(0, (), &source.source(), 0, 2 * ps, true).unwrap();
        let next = PhysLocation { arena: at.arena, offset: at.offset + ps };
        assert_eq!(stage.locations(), [(0, at), (1, next)]);
        assert_eq!(stage.pages(), [(0, Protection::ReadWrite), (1, Protection::ReadWrite)]);
    }

    /// A store fault that follows on from the last brings in the pages after it too; a load, or a
    /// store elsewhere, does not.
    #[test]
    fn stores_that_follow_on_bring_in_the_pages_after_them() {
        let ps = page_size();
        let source = TestSource::anonymous(64);
        let (mut dax, window) = Dax::<()>::new(64 * ps, None).unwrap();
        dax.map(0, (), &source.source(), 0, 64 * ps, true).unwrap();
        let fault = |offset: u64, access: Access| {
            futures::executor::block_on(window.fault(offset, access)).unwrap();
            source.present()
        };
        assert_eq!(fault(5, Access::Write), 1);
        assert_eq!(fault(ps + 7, Access::Write), 17);
        assert_eq!(fault(17 * ps + 1, Access::Write), 33);
        assert_eq!(fault(40 * ps, Access::Write), 34);
        // A load brings in its page, an open extent's, as a store would.
        assert_eq!(fault(41 * ps, Access::Read), 35);
        assert_eq!(fault(41 * ps, Access::Write), 50);
    }

    /// A fault on a page alone brings in that page alone, and a store on the next follows on from
    /// it.
    #[cfg(target_os = "linux")]
    #[test]
    fn faults_on_a_page_alone_are_followed_on_from() {
        let ps = page_size();
        let source = TestSource::anonymous(32);
        let (mut dax, window) = Dax::<()>::new(32 * ps, None).unwrap();
        dax.map(0, (), &source.source(), 0, 32 * ps, true).unwrap();
        futures::executor::block_on(window.fault_page(ps + 3, Access::Write)).unwrap();
        assert_eq!(source.present(), 1);
        futures::executor::block_on(window.fault(2 * ps, Access::Write)).unwrap();
        assert_eq!(source.present(), 17);
        futures::executor::block_on(window.fault_page(20 * ps, Access::Read)).unwrap();
        futures::executor::block_on(window.fault(21 * ps, Access::Write)).unwrap();
        assert_eq!(source.present(), 19);
    }

    /// Populating maps the pages of a range that are in memory; unmapping part of a mapping
    /// leaves what is around it.
    #[test]
    fn ranges_are_populated_and_unmapped_where_they_are() {
        let ps = page_size();
        let pager = TestSource::paged(4);
        let stage = Arc::new(Recorded::default());
        let (mut dax, window) = Dax::<()>::new(4 * ps, Some(stage.clone())).unwrap();
        dax.map(0, (), &pager.source(), 0, 4 * ps, false).unwrap();
        assert_eq!(stage.pages(), []);
        pager.supply(2, 7);
        dax.populate(2 * ps, ps).unwrap();
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
        };
        let (source, other) = (TestSource::numbered(4, 1), TestSource::paged(2));
        let (mut dax, window) =
            Dax::<()>::new(6 * ps, Some(Arc::new(Recorded::default()))).unwrap();
        let (mut second, _) = Dax::<()>::new(2 * ps, Some(Arc::new(Recorded::default()))).unwrap();
        assert_eq!(live(), Live::default());

        dax.map(0, (), &source.source(), 0, 4 * ps, false).unwrap();
        dax.map(4 * ps, (), &other.source(), 0, 2 * ps, false).unwrap();
        second.map(0, (), &TestSource::numbered(2, 5).source(), 0, 2 * ps, false).unwrap();
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
        dax.map(0, (), &source.source(), 0, 4 * ps, false).unwrap();
        dax.map(4 * ps, (), &other.source(), 0, 2 * ps, false).unwrap();
        let frames: Vec<u32> = {
            let mut frames = Vec::new();
            source.each_present(1..3, &mut |_, frame| frames.push(frame.frame()));
            frames
        };
        dax.window.unmap_pages_of(source_identity(&source), &|frame| frames.contains(&frame));
        let pages: Vec<u64> = stage.pages().into_iter().map(|(page, _)| page).collect();
        assert_eq!(pages, [0, 3, 4, 5]);
    }

    /// Making a range read-only remaps the parts of it that what `is_kept` says still maps, from
    /// the new source at the offsets they are at in the range, and no others.
    #[test]
    fn ranges_made_read_only_map_their_kept_parts_anew() {
        let ps = page_size();
        let (old, new) = (TestSource::numbered(7, 0xa0).source(), TestSource::numbered(8, 0xb0));
        let new = new.source();
        let (mut dax, window) = Dax::<Arc<u8>>::new(8 * ps, None).unwrap();
        let (other, kept, after, moved) = (Arc::new(0), Arc::new(1), Arc::new(2), Arc::new(3));
        dax.map(0, other.clone(), &old, 0, 2 * ps, true).unwrap();
        dax.map(2 * ps, kept.clone(), &old, 2 * ps, 3 * ps, true).unwrap();
        dax.map(5 * ps, after.clone(), &old, 5 * ps, 2 * ps, true).unwrap();

        // The range from the second page, at the new source's second.
        let is = |keep: Arc<u8>| move |k: &Arc<u8>| Arc::ptr_eq(k, &keep);
        let parts =
            dax.make_read_only(ps, 5 * ps, moved.clone(), &new, ps, is(kept.clone())).unwrap();
        assert_eq!(parts, [(2 * ps, 3 * ps)]);
        let mappings: Vec<_> =
            dax.mappings().map(|(start, end, keep)| (start, end, **keep)).collect();
        assert_eq!(mappings, [(0, 2 * ps, 0), (2 * ps, 5 * ps, 3), (5 * ps, 7 * ps, 2)]);
        let loaded: Vec<u8> = (0..7).map(|page| load(&window, page)).collect();
        assert_eq!(loaded, [0xa0, 0xa1, 0xb2, 0xb3, 0xb4, 0xa5, 0xa6]);
        let store = futures::executor::block_on(window.fault(3 * ps, Access::Write));
        assert_eq!(store, Err(Errno::EACCES));

        // A mapping that ends where the range starts is not in it.
        let parts =
            dax.make_read_only(5 * ps, ps, kept.clone(), &new, 0, is(moved.clone())).unwrap();
        assert_eq!(parts, []);
        // A range that ends within a mapping takes only its part of it.
        let parts =
            dax.make_read_only(5 * ps, ps, moved.clone(), &new, 0, is(after.clone())).unwrap();
        assert_eq!(parts, [(5 * ps, ps)]);
        let mappings: Vec<_> =
            dax.mappings().map(|(start, end, keep)| (start, end, **keep)).collect();
        assert_eq!(mappings[2..], [(5 * ps, 6 * ps, 3), (6 * ps, 7 * ps, 2)]);
    }

    /// The ranges a freed extent mapped map zeros, read-only; the others keep what they map.
    #[test]
    fn freed_extents_ranges_map_zeros() {
        let ps = page_size();
        let source = TestSource::numbered(8, 1).source();
        let zeros = crate::cache::zeros();
        let (mut dax, window) = Dax::<Arc<u8>>::new(8 * ps, None).unwrap();
        let (freed, other, zero) = (Arc::new(0), Arc::new(1), Arc::new(2));
        dax.map(0, freed.clone(), &source, 0, 2 * ps, true).unwrap();
        dax.map(2 * ps, other.clone(), &source, 2 * ps, 2 * ps, true).unwrap();
        dax.map(4 * ps, freed.clone(), &source, 4 * ps, 2 * ps, true).unwrap();
        dax.replace_kept(freed.identity(), zero.clone(), &zeros).unwrap();

        let loaded: Vec<u8> = (0..6).map(|page| load(&window, page)).collect();
        assert_eq!(loaded, [0, 0, 3, 4, 0, 0]);
        let store = futures::executor::block_on(window.fault(4 * ps, Access::Write));
        assert_eq!(store, Err(Errno::EACCES));
        assert_eq!(dax.kept[&zero.identity()], BTreeSet::from([0, 4 * ps]));
    }

    /// A deferral ends however what runs in it does; one that ends gives what waits for what it
    /// deferred.
    #[test]
    fn deferrals_end_even_when_what_runs_in_them_panics() {
        let stage = Arc::new(Recorded::default());
        let (mut dax, _window) = Dax::<()>::new(page_size(), Some(stage.clone())).unwrap();
        let ((), deferred) = dax.deferring(|_| ());
        assert!(deferred.is_some());
        assert_eq!(*stage.deferrals.lock().unwrap(), ["begin", "end"]);
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dax.deferring(|_| panic!("deferring"));
        }));
        assert!(panicked.is_err());
        assert_eq!(*stage.deferrals.lock().unwrap(), ["begin", "end", "begin", "end"]);
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
}
