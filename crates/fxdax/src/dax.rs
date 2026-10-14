//! DAX windows: guest physical memory into which a virtio-fs device maps files, so that the guest
//! reads and writes their pages directly instead of copying them through FUSE requests.
//!
//! A window is a guest physical address space of the in-process kernel, as a Zircon guest's memory
//! is: `FUSE_SETUPMAPPING` maps a range of a file's pager-backed VMO into it, and the guest's loads
//! and stores fault pages in. A fault is resolved like any fault on the VMO: a page not in memory
//! is read by Fxfs's pager, and a store to a clean page has the pager mark it dirty first, so that
//! it is written back when the file is flushed (on `fsync`, when the file is closed, periodically,
//! or under memory pressure).
//!
//! The kernel maps the pages at the window's host address, and makes the same changes to the
//! hypervisor's [`SecondStage`] translation of the window, if it has one. How the guest's faults
//! reach the window depends on the hypervisor:
//!
//! - One whose second stage the VMM maintains, such as HVF, reports the guest's faults on the
//!   window to the VMM, which passes them to [`DaxWindow::fault`].
//! - On Linux, KVM maps guest memory from the host memory backing it, and follows changes to it.
//!   The filesystem takes that memory over, maps the window's pages into it, and serves the faults
//!   KVM takes there itself, with userfaultfd
//!   ([`create_dax_window_in_host_memory`](crate::Filesystem::create_dax_window_in_host_memory)).
//!
//! Past the end of a file, the file's VMO has pages of zeros, which the guest may read: Linux's
//! FUSE client maps files in 2 MiB ranges, which extend past a file's end, and bounds accesses by
//! the file's size itself (it faults a mapping past its last page with `SIGBUS`, and extends files
//! with `FUSE_WRITE`). Stores past the end in the last page never reach the file: Fxfs writes back
//! only what is before its end, and zeroes the rest of the last block it writes.

#[cfg(target_os = "linux")]
pub(crate) mod uffd;

use crate::error::{Errno, IntoErrno, Result};
use crate::kernel::page_size;
use std::collections::BTreeMap;
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
    /// The memory stays mapped at `host_addr` until it is unmapped here.
    fn map(&self, offset: u64, host_addr: *mut u8, len: u64, protection: Protection);

    /// Changes the protection of the mapped `len` bytes at `offset`.
    fn protect(&self, offset: u64, len: u64, protection: Protection);

    /// Unmaps the mapped `len` bytes at `offset`.
    fn unmap(&self, offset: u64, len: u64);
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
    fn map(&self, vaddr: usize, host_addr: usize, len: usize, prot: libc::c_int) {
        self.0.map(vaddr as u64, host_addr as *mut u8, len as u64, Self::protection(prot));
    }

    fn protect(&self, vaddr: usize, len: usize, prot: libc::c_int) {
        self.0.protect(vaddr as u64, len as u64, Self::protection(prot));
    }

    fn unmap(&self, vaddr: usize, len: usize) {
        self.0.unmap(vaddr as u64, len as u64);
    }
}

/// A DAX window of a virtio-fs device. See the module docs.
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
}

impl Window {
    /// A window of `size` bytes, which must be a multiple of the page size, whose mappings are
    /// also made to `second_stage`.
    fn new(size: u64, second_stage: Option<Arc<dyn SecondStage>>) -> Result<Self> {
        if size == 0 || !size.is_multiple_of(page_size()) {
            return Err(Errno::EINVAL);
        }
        let size = usize::try_from(size).map_err(|_| Errno::EINVAL)?;
        let gpas = match second_stage {
            Some(second_stage) => GuestPhysicalAspace::create_with_second_stage(
                size,
                Arc::new(KernelSecondStage(second_stage)),
            ),
            None => GuestPhysicalAspace::create(size),
        }
        .errno()?;
        let (vmar, rights) = gpas.create_vmar_dispatcher().errno()?;
        let handle = vmar.make_and_add_handle(rights).errno()?;
        // SAFETY: The handle was just added to the handle table, and is owned by nothing else.
        let vmar = zx::Vmar::from(unsafe { zx::Handle::from_raw(handle) }.unwrap());
        let base = vmar.info().errno()?.base;
        Ok(Self { gpas, vmar, base, next_store: AtomicU64::new(u64::MAX) })
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
        let offset = usize::try_from(offset).map_err(|_| Errno::EINVAL)?;
        self.gpas.page_fault_async(offset, access == Access::Write, pages - 1).await.errno()
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
            .errno()?;
        // The pages the file has in memory are mapped now, so that the guest's first accesses to
        // them do not fault here.
        self.vmar
            .op_range(zx::VmarOp::MAP_RANGE, self.base + offset as usize, len as usize)
            .errno()?;
        Ok(())
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

/// A DAX window, and what is mapped into it: what keeps each mapping's memory alive, a `K`, such
/// as a filesystem's connection to the file mapped, which keeps it open.
pub(crate) struct Dax<K: Clone> {
    window: Arc<Window>,
    /// The mapped ranges of the window, by start: their ends and what keeps them alive.
    mappings: BTreeMap<u64, (u64, K)>,
    /// The thread serving the faults on the host memory the window was created in, which runs
    /// until the filesystem is dropped.
    #[cfg(target_os = "linux")]
    _faults: Option<uffd::FaultThread>,
}

impl<K: Clone> Dax<K> {
    /// A window of `size` bytes whose mappings are also made to `second_stage`, and a handle to it.
    pub fn new(size: u64, second_stage: Option<Arc<dyn SecondStage>>) -> Result<(Self, DaxWindow)> {
        let window = Arc::new(Window::new(size, second_stage)?);
        let dax = Self {
            window: window.clone(),
            mappings: BTreeMap::new(),
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
        self.mappings.insert(offset, (end, keep));
        Ok(())
    }

    /// Maps the parts of the window's `len` bytes at `offset` that it still maps from `vmo` at
    /// `vmo_offset`, by mappings `is_kept` says are kept alive by what keeps it, again, read-only:
    /// a writable mapping of what has become immutable since.
    pub fn make_read_only(
        &mut self,
        offset: u64,
        len: u64,
        vmo: &zx::Vmo,
        vmo_offset: u64,
        is_kept: impl Fn(&K) -> bool,
    ) -> Result<()> {
        let end = offset + len;
        let parts: Vec<(u64, u64, K)> = self
            .mappings
            .range(..end)
            .filter(|(start, (mapping_end, keep))| {
                *mapping_end > offset && **start < end && is_kept(keep)
            })
            .map(|(start, (mapping_end, keep))| {
                ((*start).max(offset), (*mapping_end).min(end), keep.clone())
            })
            .collect();
        for (start, part_end, keep) in parts {
            self.map(start, keep, vmo, vmo_offset + (start - offset), part_end - start, false)?;
        }
        Ok(())
    }

    /// Unmaps `len` bytes of the window at `offset`.
    pub fn unmap(&mut self, offset: u64, len: u64) -> Result<()> {
        self.window.unmap(offset, len)?;
        self.forget(offset, offset + len);
        Ok(())
    }

    /// Unmaps the whole window.
    pub fn unmap_all(&mut self) -> Result<()> {
        self.unmap(0, self.window.size())
    }

    /// Forgets the mappings of the window's range `start..end`, keeping those of what is around it.
    fn forget(&mut self, start: u64, end: u64) {
        let overlapping: Vec<u64> = self
            .mappings
            .range(..end)
            .filter(|(_, (mapping_end, _))| *mapping_end > start)
            .map(|(mapping_start, _)| *mapping_start)
            .collect();
        for mapping_start in overlapping {
            let (mapping_end, keep) = self.mappings.remove(&mapping_start).unwrap();
            if mapping_start < start {
                self.mappings.insert(mapping_start, (start, keep.clone()));
            }
            if mapping_end > end {
                self.mappings.insert(end, (mapping_end, keep));
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
}
