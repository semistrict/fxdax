// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! DAX windows: guest physical memory into which an extent-store device maps extents, so that the
//! guest's Fxfs reads and writes their pages directly instead of copying them through requests.
//!
//! A window is a guest physical address space of the kernel, as a Zircon guest's memory is: `MAP`
//! maps a range of an extent's VMO into it, and the guest's loads and stores fault pages in. A
//! sealed extent's VMO is pager-backed, shared by every window that maps it, whose pager reads
//! pages in from the store; an open extent's is the memory the guest writes it in, until a commit
//! seals it.
//!
//! The kernel maps the pages at the window's host address, and makes the same changes to the
//! hypervisor's [`SecondStage`] translation of the window, if it has one. How the guest's faults
//! reach the window depends on the hypervisor:
//!
//! - One whose second stage the VMM maintains, such as HVF, reports the guest's faults on the
//!   window to the VMM, which passes them to [`DaxWindow::fault`].
//! - On Linux, KVM maps guest memory from the host memory backing it, and follows changes to it.
//!   The store takes that memory over, maps the window's pages into it, and serves the faults KVM
//!   takes there itself, with userfaultfd (`ExtentStore::create_dax_window_in_host_memory`), in
//!   the extent daemon's VM processes too, which the daemon tells what to map (`crate::extentd`).

#[cfg(all(test, feature = "shuttle"))]
mod shuttle_tests;
#[cfg(target_os = "linux")]
pub(crate) mod uffd;

use crate::error::{Errno, IntoErrno, Result};
use crate::kernel::page_size;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use zircon::hypervisor::aspace::GuestPhysicalAspace;

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
    /// None: the kernel tracks the page's accesses, and the next must fault.
    None,
    /// Loads and instruction fetches. Stores must fault.
    Read,
    /// Loads, stores and instruction fetches.
    ReadWrite,
}

/// A hypervisor's second stage translation of a window, which a VMM maintains for a hypervisor
/// that does not follow the host's mappings, such as HVF's `hv_vm_map`.
///
/// Each change the kernel makes to its mappings of the window is also made here, with offsets into
/// the window. Calls for a window are serialized. They cannot fail: if the hypervisor refuses one,
/// the guest's memory no longer matches the window's, and the implementation must panic.
pub trait SecondStage: Send + Sync {
    /// Maps the `len` bytes of host memory at `host_addr` at `offset`, where nothing is mapped.
    /// The memory stays mapped at `host_addr` until it is unmapped here. It is the pages at
    /// `phys` in the kernel's physical memory, consecutive in one arena's object
    /// ([`arena_memory`](crate::kernel::arena_memory)), from which another process can map them
    /// too.
    fn map(
        &self,
        offset: u64,
        host_addr: *mut u8,
        phys: crate::kernel::PhysLocation,
        len: u64,
        protection: Protection,
    );

    /// Changes the protection of the mapped `len` bytes at `offset`.
    fn protect(&self, offset: u64, len: u64, protection: Protection);

    /// Unmaps the mapped `len` bytes at `offset`.
    fn unmap(&self, offset: u64, len: u64);

    /// fxdax: the window maps the `len` bytes at `offset` anew; called once it has, before any of
    /// the pages it maps there now are mapped here, as the kernel maps a page here only once it is
    /// in memory. A stage that withholds pages a fault found nothing at (`dax::uffd`) gives them
    /// back, so that accesses to them fault again.
    fn window_mapped(&self, _offset: u64, _len: u64) {}

    /// fxdax: the changes this thread makes from now on that take access away, until
    /// [`end_deferring`](Self::end_deferring), return before they are made. Only a stage that
    /// waits for them, as one in another process does, defers.
    fn begin_deferring(&self) {}

    /// fxdax: ends [`begin_deferring`](Self::begin_deferring): what waits for the changes it
    /// deferred, if there were any.
    fn end_deferring(&self) -> Option<Arc<dyn Deferred>> {
        None
    }
}

/// fxdax: changes to a [`SecondStage`] that take access away, deferred: until they are made, the
/// guest may still reach the pages, which mustn't be freed, or shared with another guest, until
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
    /// mapped read-only in its host memory. It is called with the window's mappings locked, and must not
    /// wait on the window, or for the guest.
    fn prefault(&self, offset: u64, len: u64);
}

/// What resolves the faults on a window's second stage in host memory: the window, or, for a
/// window in another process, a connection to it (`crate::extentd`).
#[cfg(target_os = "linux")]
pub(crate) trait Faults: Send + Sync {
    /// The window's size in bytes.
    fn size(&self) -> u64;

    /// As [`DaxWindow::fault`].
    fn fault(&self, offset: u64, access: Access) -> futures::future::BoxFuture<'_, Result<()>>;

    /// Resolves a fault at `offset` for `access` on that page alone, for a fault whose access may
    /// not be the guest's.
    fn fault_page(&self, offset: u64, access: Access)
    -> futures::future::BoxFuture<'_, Result<()>>;
}

#[cfg(target_os = "linux")]
impl Faults for Window {
    fn size(&self) -> u64 {
        Window::size(self)
    }

    fn fault(&self, offset: u64, access: Access) -> futures::future::BoxFuture<'_, Result<()>> {
        Box::pin(Window::fault(self, offset, access))
    }

    fn fault_page(
        &self,
        offset: u64,
        access: Access,
    ) -> futures::future::BoxFuture<'_, Result<()>> {
        Box::pin(Window::fault_page(self, offset, access))
    }
}

/// A [`SecondStage`] as the kernel's address spaces take it.
struct KernelSecondStage(Arc<dyn SecondStage>);

impl KernelSecondStage {
    fn protection(prot: libc::c_int) -> Protection {
        debug_assert_eq!(prot & libc::PROT_EXEC, 0, "DAX mappings are not executable");
        match prot & (libc::PROT_READ | libc::PROT_WRITE) {
            libc::PROT_NONE => Protection::None,
            libc::PROT_READ => Protection::Read,
            _ => {
                assert_ne!(prot & libc::PROT_READ, 0, "a writable page is readable");
                Protection::ReadWrite
            }
        }
    }
}

impl zircon::hypervisor::aspace::SecondStage for KernelSecondStage {
    fn map(
        &self,
        vaddr: usize,
        host_addr: usize,
        phys: crate::kernel::PhysLocation,
        len: usize,
        prot: libc::c_int,
    ) {
        let protection = Self::protection(prot);
        self.0.map(vaddr as u64, host_addr as *mut u8, phys, len as u64, protection);
    }

    fn protect(&self, vaddr: usize, len: usize, prot: libc::c_int) {
        self.0.protect(vaddr as u64, len as u64, Self::protection(prot));
    }

    fn unmap(&self, vaddr: usize, len: usize) {
        self.0.unmap(vaddr as u64, len as u64);
    }
}

/// A DAX window of an extent-store device. See the module docs.
///
/// Handles to a window are cheap to clone, and may be used from any thread.
#[derive(Clone)]
pub struct DaxWindow(Arc<Window>);

impl DaxWindow {
    /// Resolves the guest's fault at `offset` into the window, for `access`: maps the page, at the
    /// window's host address and in its second stage, so that the guest can retry the access.
    ///
    /// Completes once Fxfs's pager supplies or dirties the page, holding no thread meanwhile.
    /// Fails if no file is mapped there, with `ENOENT`, or with `EACCES` for a store to a
    /// read-only mapping; the guest cannot go on.
    pub async fn fault(&self, offset: u64, access: Access) -> Result<()> {
        self.0.fault(offset, access).await
    }

    /// Resolves a fault at `offset` for `access` on that page alone, for a fault whose access may
    /// not be the guest's.
    #[cfg(target_os = "linux")]
    pub(crate) async fn fault_page(&self, offset: u64, access: Access) -> Result<()> {
        self.0.fault_page(offset, access).await
    }

    /// The window's memory in the host, where the kernel maps its pages.
    pub fn host_base(&self) -> *mut u8 {
        self.0.gpas.host_base() as *mut u8
    }

    /// The window's size in bytes.
    pub fn size(&self) -> u64 {
        self.0.size()
    }

    /// Whether a file is mapped at `offset`.
    pub fn is_mapped(&self, offset: u64) -> bool {
        self.0.gpas.is_mapped(offset as usize)
    }
}

impl std::fmt::Debug for DaxWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaxWindow")
            .field("host_base", &self.host_base())
            .field("size", &self.size())
            .finish()
    }
}

/// How many pages a store fault that follows on from the last dirties: its own and those after
/// it, where a sequential writer stores next. Dirtying them together costs Fxfs's pager one
/// request rather than one a page.
const SEQUENTIAL_STORE_PAGES: usize = 16;

/// A window: a guest physical address space of the kernel.
pub(crate) struct Window {
    gpas: GuestPhysicalAspace,
    /// The root VMAR of `gpas`, which files' VMOs are mapped into.
    vmar: zx::Vmar,
    /// `vmar`'s base address.
    base: usize,
    /// Where a store fault following on from the last would be: the page after those it dirtied.
    next_store: AtomicU64,
    /// The second stage, told of the window's mappings anew.
    stage: Option<Arc<dyn SecondStage>>,
}

impl Window {
    /// A window of `size` bytes, which must be a multiple of the page size, whose mappings are
    /// also made to `second_stage`.
    fn new(size: u64, second_stage: Option<Arc<dyn SecondStage>>) -> Result<Self> {
        if size == 0 || !size.is_multiple_of(page_size()) {
            return Err(Errno::EINVAL);
        }
        let size = usize::try_from(size).map_err(|_| Errno::EINVAL)?;
        let gpas = match &second_stage {
            Some(second_stage) => GuestPhysicalAspace::create_with_second_stage(
                size,
                Arc::new(KernelSecondStage(second_stage.clone())),
            ),
            None => GuestPhysicalAspace::create(size),
        }
        .errno()?;
        let (vmar, rights) = gpas.create_vmar_dispatcher().errno()?;
        let handle = vmar.make_and_add_handle(rights).errno()?;
        // SAFETY: The handle was just added to the handle table, and is owned by nothing else.
        let vmar = zx::Vmar::from(unsafe { zx::Handle::from_raw(handle) }.unwrap());
        let base = vmar.info().errno()?.base;
        let next_store = AtomicU64::new(u64::MAX);
        Ok(Self { gpas, vmar, base, next_store, stage: second_stage })
    }

    fn size(&self) -> u64 {
        self.gpas.size() as u64
    }

    /// Resolves the guest's fault at `offset` for `access`. A store that follows on from the last
    /// store fault dirties the pages after it too, up to `SEQUENTIAL_STORE_PAGES` in all.
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
    /// as the file mapped there goes.
    async fn fault_pages(&self, offset: u64, access: Access, pages: usize) -> Result<()> {
        use crate::stats::{self, Counter};
        let start = std::time::Instant::now();
        let offset = usize::try_from(offset).map_err(|_| Errno::EINVAL)?;
        let faulted = self.gpas.page_fault_async(offset, access == Access::Write, pages - 1).await;
        stats::count_since(Counter::Faults, Counter::FaultNs, start);
        faulted.errno()
    }

    /// Maps `len` bytes of `vmo` at `vmo_offset` into the window at `offset`, replacing what was
    /// mapped there.
    fn map(
        &self,
        offset: u64,
        vmo: &zx::Vmo,
        vmo_offset: u64,
        len: u64,
        writable: bool,
    ) -> Result<()> {
        let mut flags =
            zx::VmarFlagsExtended::PERM_READ | zx::VmarFlagsExtended::SPECIFIC_OVERWRITE;
        if writable {
            flags |= zx::VmarFlagsExtended::PERM_WRITE;
        }
        // SAFETY: The mappings replaced are the window's, which only the guest accesses.
        unsafe { self.vmar.map_unsafe(offset as usize, vmo, vmo_offset, len as usize, flags) }
            .inspect_err(|status| {
                log::warn!(
                    "fxdax: mapping {len:#x} bytes into the DAX window at {offset:#x}: {status}"
                )
            })
            .errno()?;
        if let Some(stage) = &self.stage {
            stage.window_mapped(offset, len);
        }
        // The pages the file has in memory are mapped now, so that the guest's first accesses to
        // them do not fault here.
        self.vmar
            .op_range(zx::VmarOp::MAP_RANGE, self.base + offset as usize, len as usize)
            .inspect_err(|status| {
                log::warn!(
                    "fxdax: populating {len:#x} bytes of the DAX window at {offset:#x}: {status}"
                )
            })
            .errno()?;
        Ok(())
    }

    /// Maps the pages in memory of what is mapped at `len` bytes of the window at `offset`, so
    /// that the guest's first accesses to them do not fault.
    fn populate(&self, offset: u64, len: u64) -> Result<()> {
        self.vmar.op_range(zx::VmarOp::MAP_RANGE, self.base + offset as usize, len as usize).errno()
    }

    /// Unmaps `len` bytes of the window at `offset`.
    fn unmap(&self, offset: u64, len: u64) -> Result<()> {
        // SAFETY: Only the guest accesses the window's memory, which the window hands out only to
        // the VMM.
        unsafe { self.vmar.unmap(self.base + offset as usize, len as usize) }.errno()
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
        Arc::as_ptr(self) as *const () as usize
    }
}

impl Keep for () {
    fn identity(&self) -> usize {
        0
    }
}

/// A DAX window, and what is mapped into it: what keeps each mapping's memory alive, a `K`, such
/// as a filesystem's connection to the file mapped, which keeps it open.
pub(crate) struct Dax<K: Keep> {
    window: Arc<Window>,
    /// The mapped ranges of the window, by start: their ends and what keeps them alive. They do
    /// not overlap.
    mappings: BTreeMap<u64, (u64, K)>,
    /// The starts of the mappings, by the identity of what keeps them alive.
    kept: HashMap<usize, BTreeSet<u64>>,
    /// The second stage, if the window has one.
    stage: Option<Arc<dyn SecondStage>>,
    /// The thread serving the faults on the host memory the window was created in, which runs
    /// until the filesystem is dropped.
    #[cfg(target_os = "linux")]
    _faults: Option<uffd::FaultThread>,
}

impl<K: Keep> Dax<K> {
    /// A window of `size` bytes whose mappings are also made to `second_stage`, and a handle to it.
    pub fn new(size: u64, second_stage: Option<Arc<dyn SecondStage>>) -> Result<(Self, DaxWindow)> {
        let window = Arc::new(Window::new(size, second_stage.clone())?);
        let dax = Self {
            window: window.clone(),
            mappings: BTreeMap::new(),
            kept: HashMap::new(),
            stage: second_stage,
            #[cfg(target_os = "linux")]
            _faults: None,
        };
        Ok((dax, DaxWindow(window)))
    }

    /// A window whose second stage is the host memory `stage` takes over, and a handle to it. A
    /// thread of its own serves the faults on the memory.
    #[cfg(target_os = "linux")]
    pub fn in_host_memory(
        stage: Arc<uffd::UffdSecondStage>,
    ) -> crate::error::Result<(Self, DaxWindow), crate::Error> {
        let (mut dax, window) = Self::new(stage.size(), Some(stage.clone()))?;
        dax._faults = Some(stage.serve(dax.window.clone())?);
        Ok((dax, window))
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

    /// Maps `len` bytes of `vmo`, whose memory `keep` keeps alive, at `vmo_offset` into the
    /// window at `offset`.
    pub fn map(
        &mut self,
        offset: u64,
        keep: K,
        vmo: &zx::Vmo,
        vmo_offset: u64,
        len: u64,
        writable: bool,
    ) -> Result<()> {
        self.window.map(offset, vmo, vmo_offset, len, writable)?;
        let end = offset + len;
        self.forget(offset, end);
        self.insert(offset, end, keep);
        Ok(())
    }

    /// Maps the parts of the window's `len` bytes at `offset` that mappings `is_kept` says are
    /// kept alive by what was mapped there still map, read-only, from `vmo` at `vmo_offset`, kept
    /// alive by `keep`: a writable mapping of what has become immutable since, and moved. Returns
    /// the parts, as the window's offsets and lengths.
    pub fn make_read_only(
        &mut self,
        offset: u64,
        len: u64,
        keep: K,
        vmo: &zx::Vmo,
        vmo_offset: u64,
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
            let (vmo_offset, len) = (vmo_offset + (start - offset), part_end - start);
            self.map(start, keep.clone(), vmo, vmo_offset, len, false)?;
            mapped.push((start, len));
        }
        Ok(mapped)
    }

    /// Unmaps `len` bytes of the window at `offset`.
    pub fn unmap(&mut self, offset: u64, len: u64) -> Result<()> {
        self.window.unmap(offset, len)?;
        self.forget(offset, offset + len);
        Ok(())
    }

    /// Maps the pages in memory of what is mapped at `len` bytes of the window at `offset`.
    pub fn populate(&self, offset: u64, len: u64) -> Result<()> {
        self.window.populate(offset, len)
    }

    /// Maps the window's ranges whose mappings keep alive what has the identity `kept`, what a
    /// freed extent mapped, from `zeros` instead, read-only, kept alive by `keep`: their memory
    /// goes with their old mappings. The guest may still touch such a range, as a store racing the
    /// move of what it maps, which faults as on any read-only page; unmapped, its fault would find
    /// nothing to map.
    pub fn replace_kept(&mut self, kept: usize, keep: K, zeros: &zx::Vmo) -> Result<()> {
        let starts: Vec<u64> =
            self.kept.get(&kept).map(|starts| starts.iter().copied().collect()).unwrap_or_default();
        for start in starts {
            let end = self.mappings[&start].0;
            if rules().free_maps_zeros {
                self.window.map(start, zeros, start, end - start, false)?;
            } else {
                self.window.unmap(start, end - start)?;
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

    /// The window's size.
    pub fn size(&self) -> u64 {
        self.window.size()
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
    use std::task::{Context, Poll};

    const KEY: u64 = 3;

    /// A pager-backed VMO of `pages` pages, served by the test, and its pager and port.
    pub(crate) struct TestPager {
        pub port: zx::Port,
        pub pager: zx::Pager,
        pub vmo: zx::Vmo,
    }

    impl TestPager {
        pub fn new(pages: u64) -> Self {
            let port = zx::Port::create();
            let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
            let vmo =
                pager.create_vmo(zx::VmoOptions::empty(), &port, KEY, pages * page_size()).unwrap();
            Self { port, pager, vmo }
        }

        /// The range of the read request queued on the port, waiting up to `deadline` for it.
        pub fn read_request(&self, deadline: zx::MonotonicInstant) -> std::ops::Range<u64> {
            let packet = self.port.wait(deadline).expect("no page request");
            let zx::PacketContents::Pager(request) = packet.contents() else {
                panic!("Unexpected packet");
            };
            assert_eq!(request.command(), zx::sys::zx_page_request_command_t::ZX_PAGER_VMO_READ);
            request.range()
        }

        /// Supplies `page` with bytes of `value`.
        pub fn supply(&self, page: u64, value: u8) {
            let aux = zx::Vmo::create(page_size()).unwrap();
            aux.write(&vec![value; page_size() as usize], 0).unwrap();
            self.pager
                .supply_pages(&self.vmo, page * page_size()..(page + 1) * page_size(), &aux, 0)
                .unwrap();
        }
    }

    /// The window's mappings are found by the identity of what keeps them alive, as mapping over
    /// parts of them splits them.
    #[test]
    fn replacing_what_keeps_mappings_finds_what_remains_of_them() {
        let ps = page_size();
        let vmo = zx::Vmo::create(8 * ps).unwrap();
        let zeros = zx::Vmo::create(8 * ps).unwrap();
        let (mut dax, _window) = Dax::<Arc<u8>>::new(8 * ps, None).unwrap();
        let (a, b, z) = (Arc::new(0), Arc::new(1), Arc::new(2));
        dax.map(0, a.clone(), &vmo, 0, 4 * ps, true).unwrap();
        dax.map(4 * ps, b.clone(), &vmo, 4 * ps, 2 * ps, true).unwrap();
        // Over the middle of `a`'s mapping, which splits around it.
        dax.map(ps, b.clone(), &vmo, ps, ps, true).unwrap();

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
        let pager = TestPager::new(2);
        let (dax, window) = Dax::<()>::new(2 * page_size(), None).unwrap();
        dax.window.map(0, &pager.vmo, 0, 2 * page_size(), false).unwrap();
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());

        let mut first = Box::pin(window.fault(0, Access::Read));
        let mut second = Box::pin(window.fault(page_size(), Access::Read));
        assert!(first.poll_unpin(&mut cx).is_pending());
        assert!(second.poll_unpin(&mut cx).is_pending());
        let now = zx::MonotonicInstant::INFINITE_PAST;
        let mut requests = [pager.read_request(now), pager.read_request(now)];
        requests.sort_by_key(|range| range.start);
        assert_eq!(requests, [0..page_size(), page_size()..2 * page_size()]);

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

    /// A second stage that keeps the window's pages as the kernel maps them in it, the ranges the
    /// window maps anew, and its deferrals, each of which defers changes.
    #[derive(Default)]
    struct Recorded {
        pages: std::sync::Mutex<BTreeMap<u64, Protection>>,
        mapped: std::sync::Mutex<Vec<(u64, u64)>>,
        deferrals: std::sync::Mutex<Vec<&'static str>>,
    }

    impl Recorded {
        /// The pages mapped, by their numbers.
        fn pages(&self) -> Vec<(u64, Protection)> {
            let pages = self.pages.lock().unwrap();
            pages.iter().map(|(offset, protection)| (offset / page_size(), *protection)).collect()
        }

        fn each_page(offset: u64, len: u64) -> impl Iterator<Item = u64> {
            (offset..offset + len).step_by(page_size() as usize)
        }
    }

    impl SecondStage for Recorded {
        fn map(
            &self,
            offset: u64,
            _host_addr: *mut u8,
            _phys: crate::kernel::PhysLocation,
            len: u64,
            protection: Protection,
        ) {
            let mut pages = self.pages.lock().unwrap();
            for page in Self::each_page(offset, len) {
                assert_eq!(pages.insert(page, protection), None, "{page:#x} mapped twice");
            }
        }

        fn protect(&self, offset: u64, len: u64, protection: Protection) {
            let mut pages = self.pages.lock().unwrap();
            for page in Self::each_page(offset, len) {
                *pages.get_mut(&page).expect("a page mapped") = protection;
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

    /// An anonymous VMO of `pages` pages, each its number plus `first`.
    fn numbered(pages: u64, first: u8) -> zx::Vmo {
        let vmo = zx::Vmo::create(pages * page_size()).unwrap();
        for page in 0..pages {
            let bytes = vec![first + page as u8; page_size() as usize];
            vmo.write(&bytes, page * page_size()).unwrap();
        }
        vmo
    }

    /// The byte at the start of page `page` of the window, which a load faults in first: of a
    /// window with no second stage, whose host memory the kernel maps.
    fn load(window: &DaxWindow, page: u64) -> u8 {
        futures::executor::block_on(window.fault(page * page_size(), Access::Read)).unwrap();
        // SAFETY: The page is mapped, and nothing else accesses it.
        unsafe { *window.host_base().add((page * page_size()) as usize) }
    }

    /// The kernel's protections are the second stage's.
    #[test]
    fn protections_are_the_kernels() {
        let protection = KernelSecondStage::protection;
        assert_eq!(protection(libc::PROT_NONE), Protection::None);
        assert_eq!(protection(libc::PROT_READ), Protection::Read);
        assert_eq!(protection(libc::PROT_READ | libc::PROT_WRITE), Protection::ReadWrite);
    }

    /// The second stage maps the pages the window maps, as the window does, and unmaps those it
    /// unmaps; it is told of each range the window maps anew.
    #[test]
    fn second_stages_follow_the_windows_pages() {
        let ps = page_size();
        let stage = Arc::new(Recorded::default());
        let (mut dax, window) = Dax::<()>::new(4 * ps, Some(stage.clone())).unwrap();
        let vmo = zx::Vmo::create(4 * ps).unwrap();
        vmo.write(&[1], 0).unwrap();
        dax.map(0, (), &vmo, 0, 4 * ps, false).unwrap();
        assert_eq!(*stage.mapped.lock().unwrap(), [(0, 4 * ps)]);
        assert_eq!(stage.pages(), [(0, Protection::Read)]);
        futures::executor::block_on(window.fault(2 * ps + 5, Access::Read)).unwrap();
        assert_eq!(stage.pages(), [(0, Protection::Read), (2, Protection::Read)]);
        let store = futures::executor::block_on(window.fault(ps, Access::Write));
        assert_eq!(store, Err(Errno::EACCES));
        dax.unmap(0, ps).unwrap();
        assert_eq!(stage.pages(), [(2, Protection::Read)]);
        assert!(!window.is_mapped(0) && window.is_mapped(ps));
        let unmapped = futures::executor::block_on(window.fault(0, Access::Read));
        assert_eq!(unmapped, Err(Errno::ENOENT));

        // Mapped anew where pages were, its pages in memory are mapped at once.
        dax.map(2 * ps, (), &numbered(2, 1), 0, 2 * ps, false).unwrap();
        assert_eq!(*stage.mapped.lock().unwrap(), [(0, 4 * ps), (2 * ps, 2 * ps)]);
        assert_eq!(stage.pages(), [(2, Protection::Read), (3, Protection::Read)]);
    }

    /// A store to a clean page of a file whose pager tracks its dirty pages maps it writable once
    /// the pager dirties it; writeback takes the write access away again.
    #[test]
    fn stores_dirty_pages_and_writeback_cleans_them() {
        let ps = page_size();
        let port = zx::Port::create();
        let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
        let vmo = pager.create_vmo(zx::VmoOptions::TRAP_DIRTY, &port, KEY, ps).unwrap();
        let aux = zx::Vmo::create(ps).unwrap();
        pager.supply_pages(&vmo, 0..ps, &aux, 0).unwrap();
        let stage = Arc::new(Recorded::default());
        let (mut dax, window) = Dax::<()>::new(ps, Some(stage.clone())).unwrap();
        dax.map(0, (), &vmo, 0, ps, true).unwrap();
        futures::executor::block_on(window.fault(0, Access::Read)).unwrap();
        assert_eq!(stage.pages(), [(0, Protection::Read)]);

        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        let mut store = Box::pin(window.fault(0, Access::Write));
        assert!(store.poll_unpin(&mut cx).is_pending());
        let packet = port.wait(zx::MonotonicInstant::INFINITE_PAST).unwrap();
        let zx::PacketContents::Pager(request) = packet.contents() else { panic!("{packet:?}") };
        assert_eq!(request.command(), zx::sys::zx_page_request_command_t::ZX_PAGER_VMO_DIRTY);
        pager.op_range(zx::PagerOp::Dirty, &vmo, request.range()).unwrap();
        assert_eq!(store.poll_unpin(&mut cx), Poll::Ready(Ok(())));
        assert_eq!(stage.pages(), [(0, Protection::ReadWrite)]);

        let begin = zx::PagerOp::WritebackBegin(zx::PagerWritebackBeginOptions::empty());
        pager.op_range(begin, &vmo, 0..ps).unwrap();
        assert_eq!(stage.pages(), [(0, Protection::Read)]);
    }

    /// A store fault that follows on from the last dirties the pages after it too; a load, or a
    /// store elsewhere, does not.
    #[test]
    fn stores_that_follow_on_dirty_the_pages_after_them() {
        let ps = page_size();
        let vmo = zx::Vmo::create(64 * ps).unwrap();
        let (mut dax, window) = Dax::<()>::new(64 * ps, None).unwrap();
        dax.map(0, (), &vmo, 0, 64 * ps, true).unwrap();
        let committed = || vmo.info().unwrap().committed_bytes / ps;
        let fault = |offset: u64, access: Access| {
            futures::executor::block_on(window.fault(offset, access)).unwrap();
            committed()
        };
        assert_eq!(fault(5, Access::Write), 1);
        assert_eq!(fault(ps + 7, Access::Write), 17);
        assert_eq!(fault(17 * ps + 1, Access::Write), 33);
        assert_eq!(fault(40 * ps, Access::Write), 34);
        assert_eq!(fault(41 * ps, Access::Read), 34);
        assert_eq!(fault(41 * ps, Access::Write), 50);
    }

    /// A fault on a page alone dirties that page alone, and a store on the next follows on from
    /// it.
    #[cfg(target_os = "linux")]
    #[test]
    fn faults_on_a_page_alone_are_followed_on_from() {
        let ps = page_size();
        let vmo = zx::Vmo::create(32 * ps).unwrap();
        let (mut dax, window) = Dax::<()>::new(32 * ps, None).unwrap();
        dax.map(0, (), &vmo, 0, 32 * ps, true).unwrap();
        let committed = || vmo.info().unwrap().committed_bytes / ps;
        futures::executor::block_on(window.fault_page(ps + 3, Access::Write)).unwrap();
        assert_eq!(committed(), 1);
        futures::executor::block_on(window.fault(2 * ps, Access::Write)).unwrap();
        assert_eq!(committed(), 17);
        futures::executor::block_on(window.fault_page(20 * ps, Access::Read)).unwrap();
        futures::executor::block_on(window.fault(21 * ps, Access::Write)).unwrap();
        assert_eq!(committed(), 18);
    }

    /// Populating maps the pages of a range that are in memory; unmapping part of a mapping
    /// leaves what is around it.
    #[test]
    fn ranges_are_populated_and_unmapped_where_they_are() {
        let ps = page_size();
        let pager = TestPager::new(4);
        let stage = Arc::new(Recorded::default());
        let (mut dax, window) = Dax::<()>::new(4 * ps, Some(stage.clone())).unwrap();
        dax.map(0, (), &pager.vmo, 0, 4 * ps, false).unwrap();
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

    /// Making a range read-only remaps the parts of it that what `is_kept` says still maps, from
    /// the new VMO at the offsets they are at in the range, and no others.
    #[test]
    fn ranges_made_read_only_map_their_kept_parts_anew() {
        let ps = page_size();
        let (old, new) = (numbered(7, 0xa0), numbered(8, 0xb0));
        let (mut dax, window) = Dax::<Arc<u8>>::new(8 * ps, None).unwrap();
        let (other, kept, after, moved) = (Arc::new(0), Arc::new(1), Arc::new(2), Arc::new(3));
        dax.map(0, other.clone(), &old, 0, 2 * ps, true).unwrap();
        dax.map(2 * ps, kept.clone(), &old, 2 * ps, 3 * ps, true).unwrap();
        dax.map(5 * ps, after.clone(), &old, 5 * ps, 2 * ps, true).unwrap();

        // The range from the second page, at the new VMO's second.
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
        let vmo = numbered(8, 1);
        let zeros = zx::Vmo::create(8 * ps).unwrap();
        let (mut dax, window) = Dax::<Arc<u8>>::new(8 * ps, None).unwrap();
        let (freed, other, zero) = (Arc::new(0), Arc::new(1), Arc::new(2));
        dax.map(0, freed.clone(), &vmo, 0, 2 * ps, true).unwrap();
        dax.map(2 * ps, other.clone(), &vmo, 2 * ps, 2 * ps, true).unwrap();
        dax.map(4 * ps, freed.clone(), &vmo, 4 * ps, 2 * ps, true).unwrap();
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
            format!("DaxWindow {{ host_base: {:?}, size: {} }}", window.host_base(), 4 * ps);
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
