// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A DAX window's second stage in host memory that a hypervisor follows, such as the memory
//! backing a KVM memory slot, whose faults userfaultfd reports.
//!
//! Nothing here is particular to a VMM: any whose hypervisor maps guest memory from host memory
//! and follows the host's mappings of it can hand the memory over.
//!
//! KVM maps a guest's memory from the host memory backing its memory slot, and follows changes to
//! the host's mappings. On arm64 it reports a fault it cannot resolve there only as `EFAULT`, with
//! no address, so the memory must never fault that way. What the window has not mapped, or maps
//! read-only, is instead registered with userfaultfd, which stops the faulting thread (a vCPU, or
//! KVM on its behalf) and reports the address:
//!
//! - An unmapped page is anonymous memory registered for missing faults.
//! - A mapped page maps the window's page again, read-write, from the kernel's physical memory
//!   (a memfd, which the process that owns the kernel may share with this one), registered for
//!   write-protect faults, and is write-protected while the window maps it read-only.
//!
//! A fault thread passes the faults to the window, which maps the page (and so maps it here), and
//! wakes the faulting thread. It resolves faults concurrently, as futures it polls as they make
//! progress, so that a fault waiting for Fxfs's pager does not hold up the others. A fault the
//! window cannot resolve is fatal, as it is to a Zircon guest: the faulting thread cannot go on.
//!
//! A fault's access is not always the guest's. x86 KVM hands a fault it cannot resolve without
//! waiting, as it cannot one userfaultfd reports, to a worker that faults the page in for writing
//! (`async_pf_execute`), and lets the vCPU retry its own access once the page is in. So a page a
//! writable mapping maps may be dirtied by a load, and written back needlessly; and a write to a
//! page the window maps read-only is resolved as a read, and the page made read-only here, so that
//! the worker's write fails and the vCPU's own access decides.
//!
//! Nor is a fault always for a page the guest still maps. While the worker faults the page in, the
//! vCPU runs on, taking the guest's interrupts, and the guest may unmap the page meanwhile, as a
//! follower drops the ranges its batches change; once the worker is done, KVM faults the page
//! again itself, before the vCPU next runs (`kvm_arch_async_page_ready`). A fault on a page the
//! window maps nothing at withholds the page, which is made inaccessible here, so that the fault
//! fails with `EFAULT`, which KVM drops. The guest's own access, made only to pages it maps, would
//! fail as KVM's run of the vCPU does, which the VMM gives up on. The window gives the page back
//! when it maps it anew (`SecondStage::window_mapped`): the kernel maps a page here only once a
//! fault brings it in, which a page withheld would never take.
//!
//! A page restricted by a fault, made read-only for a store the window refuses or withheld, is
//! restricted by the fault thread beside the window's own changes to the memory: only between
//! them, and only as the page was when it faulted. Restricted midway through a change, it would
//! leave memory accessible that is not yet registered; restricted once the page has changed, it
//! would refuse the new page's stores, or withhold a page mapped.
//!
//! Changing a page's mapping here replaces its memory, which takes several system calls, and a fault
//! between them finds memory not yet registered. Until it is registered, new memory is inaccessible,
//! or read-only where a store would go untracked, so that such a fault fails with `EFAULT` rather
//! than resolving wrongly: a page of zeros faulted in, or a store not dirtying its page. A VMM must
//! then let the guest retry its access, as fxdax's Firecracker retries a vCPU's `KVM_RUN` that
//! fails with `EFAULT` for a moment.
//!
//! Each run of pages mapped here, consecutive pages of one arena's memory, is a mapping of the
//! process's own, of which Linux allows `vm.max_map_count` (65,530 by default); a window's pages
//! scattered over its arenas would take one each (TASK-12). The stage keeps a budget of runs
//! installed, half of what the process may map beyond what it keeps for itself: past it, it leaves
//! out the runs installed or faulted on longest ago, memory registered for missing faults again,
//! whose pages it remembers, and installs one again, as it was, when the guest faults on it,
//! without the window. The kernel's mappings of the window are unchanged.
//!
//! The userfaultfd must handle faults taken in the kernel, such as KVM's. That needs access to
//! `/dev/userfaultfd`, or the `vm.unprivileged_userfaultfd` sysctl.

use super::{Access, Anew, Faults, Prefault, Protection, SecondStage};
use crate::error::{Errno, Error, Result};
use crate::kernel::page_size;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, Wake, Waker};

#[cfg(feature = "shuttle")]
use shuttle::sync::Mutex;
#[cfg(not(feature = "shuttle"))]
use std::sync::Mutex;

// From `include/uapi/linux/userfaultfd.h`.
const UFFD_API: u64 = 0xaa;
const UFFD_EVENT_PAGEFAULT: u8 = 0x12;
const UFFD_PAGEFAULT_FLAG_WRITE: u64 = 1 << 0;
const UFFD_PAGEFAULT_FLAG_WP: u64 = 1 << 1;
const UFFD_FEATURE_WP_HUGETLBFS_SHMEM: u64 = 1 << 12;
const UFFDIO_REGISTER_MODE_MISSING: u64 = 1 << 0;
const UFFDIO_REGISTER_MODE_WP: u64 = 1 << 1;
const UFFDIO_WRITEPROTECT_MODE_WP: u64 = 1 << 0;

const fn ioc(dir: u64, nr: u64, size: usize) -> u64 {
    (dir << 30) | ((size as u64) << 16) | (0xaa << 8) | nr
}
const IOC_NONE: u64 = 0;
const IOC_READ: u64 = 2;
const IOC_READ_WRITE: u64 = 3;
const USERFAULTFD_IOC_NEW: u64 = ioc(IOC_NONE, 0x00, 0);
const UFFDIO_API: u64 = ioc(IOC_READ_WRITE, 0x3f, size_of::<UffdioApi>());
const UFFDIO_REGISTER: u64 = ioc(IOC_READ_WRITE, 0x00, size_of::<UffdioRegister>());
const UFFDIO_WAKE: u64 = ioc(IOC_READ, 0x02, size_of::<UffdioRange>());
const UFFDIO_WRITEPROTECT: u64 = ioc(IOC_READ_WRITE, 0x06, size_of::<UffdioWriteprotect>());

#[repr(C)]
struct UffdioApi {
    api: u64,
    features: u64,
    ioctls: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UffdioRange {
    start: u64,
    len: u64,
}

#[repr(C)]
struct UffdioRegister {
    range: UffdioRange,
    mode: u64,
    ioctls: u64,
}

#[repr(C)]
struct UffdioWriteprotect {
    range: UffdioRange,
    mode: u64,
}

/// `struct uffd_msg`, for page faults.
#[repr(C)]
struct UffdMsg {
    event: u8,
    reserved1: u8,
    reserved2: u16,
    reserved3: u32,
    flags: u64,
    address: u64,
    ptid: u32,
    reserved4: u32,
}

const _: () = assert!(size_of::<UffdMsg>() == 32);

/// A DAX window's second stage in host memory that a hypervisor follows. See the module docs.
pub(crate) struct UffdSecondStage<H: HostMemory = Syscalls> {
    base: usize,
    size: usize,
    /// What changes the memory: system calls, but in tests.
    host: H,
    /// The kernel's physical memory, which the window's pages are mapped from.
    memories: Memories,
    /// The hypervisor, told of the pages mapped here, if it maps them ahead of the guest.
    prefault: Option<Arc<dyn Prefault>>,
    /// What the window has done to each page here, held while it makes a change: a page a fault
    /// restricts, as a store the window refuses makes it read-only, waits for a change being made,
    /// and is restricted only if it hasn't changed since the fault.
    pages: Mutex<Pages>,
    /// The runs of pages it keeps installed at most (see the module docs).
    budget: usize,
    /// fxdax: the changes that took access away from pages it had installed (TASK-91.16).
    taken: Taken,
}

/// fxdax: the changes a second stage made that took access away from pages it had installed, by
/// how: memory installed over theirs, theirs left out, or theirs write-protected. Each flushes the
/// host's TLBs of the process's threads and the hypervisor's of the guest's vCPUs, interrupting
/// them, which those that took access from no installed page needn't (TASK-91.16).
#[derive(Default)]
struct Taken {
    overwritten: AtomicU64,
    left_out: AtomicU64,
    write_protected: AtomicU64,
}

/// fxdax: as [`Taken`] counts them: installed over, left out, write-protected.
pub(crate) type TakenCounts = [u64; 3];

/// How many mappings a VM process keeps for itself beside its window's: Firecracker's, its
/// libraries', guest memory's, its threads' stacks'.
const MAPPINGS_RESERVED: usize = 4096;

/// The runs of pages a second stage keeps installed at most: half of the mappings the process may
/// make (`vm.max_map_count`) beyond [`MAPPINGS_RESERVED`], as each run, a mapping of its own,
/// splits the memory around it, a mapping too.
fn default_budget() -> usize {
    let max = std::fs::read_to_string("/proc/sys/vm/max_map_count")
        .ok()
        .and_then(|max| max.trim().parse::<usize>().ok())
        .unwrap_or(65_530);
    (max.saturating_sub(MAPPINGS_RESERVED) / 2).max(1)
}

/// Logs a fault the window refused, `what` at window offset `offset`: each of the first thousand,
/// a run's few, and every power of two after: a vCPU whose access stays refused for a second stops
/// its VM with only `EFAULT` to say so, from `KVM_RUN` (TASK-99, TASK-101).
fn refused(what: &str, offset: u64) {
    static REFUSED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = REFUSED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if count <= 1000 || count.is_power_of_two() {
        log::warn!("fxdax: the DAX window refused {what}, at offset {offset:#x} ({count} so far)");
    }
}

/// fxdax: logs a restriction of a page at window offset `offset` to reads, or its lifting, `what`,
/// as its changes stood (`made`, and what the fault saw, `seen`), each of the first thousand and
/// every power of two after: which side was wrong, the window or the guest, when a store stays
/// refused (TASK-102).
fn changed_protection(what: &str, offset: u64, made: u32, seen: Option<u32>) {
    static CHANGED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = CHANGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if count <= 1000 || count.is_power_of_two() {
        log::warn!("fxdax: DAX window offset {offset:#x} {what} (made {made}, seen {seen:?})");
    }
}

/// What the window has done to a page of a second stage, as the faults on it see it, and what it
/// maps there.
#[derive(Clone, Copy, Default)]
struct PageChanges {
    /// The changes it has made to the page, and its mappings of it anew.
    made: u32,
    /// Whether a fault that found nothing mapped there withheld the page, which the window's
    /// mapping it anew gives back.
    withheld: bool,
    /// fxdax: whether a fault whose store the window refused made the page read-only here, which
    /// the window's making it writable in place lifts, and its memory replaced does away with
    /// (TASK-102): left so, its stores failed, KVM_RUN with EFAULT, for ever.
    restricted: bool,
    /// What the window maps at the page, if anything.
    mapped: Option<Mapped>,
}

/// A page the window maps here: its memory, the window's protection of it, whether it is
/// installed in host memory or left out, and when it was installed or faulted on last.
#[derive(Clone, Copy)]
struct Mapped {
    phys: crate::kernel::PhysLocation,
    protection: Protection,
    installed: bool,
    used: u64,
}

/// A second stage's pages, and the runs of them installed.
struct Pages {
    pages: Vec<PageChanges>,
    /// The runs of pages installed: consecutive pages of one arena's memory, each a mapping.
    runs: usize,
    /// Counts installations and faults, the ones longest ago left out first.
    clock: u64,
}

impl Pages {
    /// Whether page `at` starts a run installed: it is installed, and its memory doesn't follow
    /// on from its predecessor's, installed.
    fn starts_run(&self, at: usize) -> bool {
        let Some(Mapped { phys, installed: true, .. }) = self.pages[at].mapped else {
            return false;
        };
        let follows = at > 0
            && matches!(self.pages[at - 1].mapped, Some(Mapped { phys: before, installed: true, .. })
                if before.arena == phys.arena && before.offset + page_size() == phys.offset);
        !follows
    }

    /// The runs starting in `range`, or at the page after it: what a change to it may change.
    fn runs_around(&self, range: &std::ops::Range<usize>) -> usize {
        let end = (range.end + 1).min(self.pages.len());
        (range.start..end).filter(|&at| self.starts_run(at)).count()
    }

    /// Changes the pages of `range` with `change`, counting the runs installed anew.
    fn update(&mut self, range: std::ops::Range<usize>, change: impl FnOnce(&mut [PageChanges])) {
        let before = self.runs_around(&range);
        change(&mut self.pages[range.clone()]);
        self.runs = self.runs + self.runs_around(&range) - before;
    }

    /// The next tick of the clock.
    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// fxdax: whether a page of `range` is installed, which a change taking it away flushes.
    fn any_installed(&self, range: std::ops::Range<usize>) -> bool {
        self.pages[range].iter().any(|page| page.mapped.is_some_and(|mapped| mapped.installed))
    }

    /// fxdax: the protection the pages of `range` are installed with, if each is installed, in one
    /// run of memory from `phys` on, all with one protection, and none restricted by a fault: what
    /// a map of the very same pages there changes in place (TASK-91.16).
    fn installed_as(
        &self,
        range: std::ops::Range<usize>,
        phys: crate::kernel::PhysLocation,
    ) -> Option<Protection> {
        let mut protection = None;
        for (at, page) in self.pages[range].iter().enumerate() {
            let mapped = page.mapped.filter(|mapped| mapped.installed)?;
            let offset = phys.offset + at as u64 * page_size();
            if page.restricted || mapped.phys != (crate::kernel::PhysLocation { offset, ..phys }) {
                return None;
            }
            if *protection.get_or_insert(mapped.protection) != mapped.protection {
                return None;
            }
        }
        protection
    }
}

/// What a second stage maps its host memory with.
pub(crate) enum Memory {
    /// Anonymous memory, private, of zeros until written.
    Anonymous,
    /// `fd`'s memory at `offset`, shared with it or private to this process.
    File { fd: std::os::fd::RawFd, offset: u64, shared: bool },
}

/// What a second stage changes its host memory with: system calls ([`Syscalls`]), or, in its
/// Shuttle tests, a model of what they do, against which a guest's accesses are made.
pub(crate) trait HostMemory: Send + Sync {
    /// Maps `len` bytes of `memory` at `addr` with protection `prot`, replacing what was mapped
    /// there, and its registration with userfaultfd (`mmap` with `MAP_FIXED`).
    fn map(&self, addr: usize, len: usize, prot: libc::c_int, memory: Memory);

    /// Changes the protection of the `len` bytes at `addr` (`mprotect`).
    fn protect(&self, addr: usize, len: usize, prot: libc::c_int);

    /// Registers the `len` bytes at `addr` with userfaultfd for faults of `mode`.
    fn register(&self, addr: usize, len: usize, mode: u64);

    /// Write-protects the `len` bytes at `addr` with userfaultfd, or removes their write
    /// protection and wakes the threads faulting on it.
    fn write_protect(&self, addr: usize, len: usize, protect: bool);

    /// Faults in the `len` bytes at `addr` for writing, without changing them.
    fn populate_write(&self, addr: usize, len: usize);

    /// Wakes the threads faulting on the `len` bytes at `addr`.
    fn wake(&self, addr: usize, len: usize);
}

/// The system calls that change a second stage's host memory, with its userfaultfd.
pub(crate) struct Syscalls {
    uffd: OwnedFd,
}

impl HostMemory for Syscalls {
    fn map(&self, addr: usize, len: usize, prot: libc::c_int, memory: Memory) {
        let (flags, fd, offset) = match memory {
            Memory::Anonymous => {
                (libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE, -1, 0)
            }
            Memory::File { fd, offset, shared } => {
                let share = if shared { libc::MAP_SHARED } else { libc::MAP_PRIVATE };
                (share, fd, offset as libc::off_t)
            }
        };
        // SAFETY: Replaces the second stage's memory at `addr`, which only the guest accesses.
        let mapped =
            unsafe { libc::mmap(addr as *mut _, len, prot, flags | libc::MAP_FIXED, fd, offset) };
        assert_eq!(mapped as usize, addr, "map {addr:#x}: {}", std::io::Error::last_os_error());
    }

    fn protect(&self, addr: usize, len: usize, prot: libc::c_int) {
        // SAFETY: Changes the protection of the second stage's memory.
        let status = unsafe { libc::mprotect(addr as *mut _, len, prot) };
        assert_eq!(status, 0, "protect {addr:#x}: {}", std::io::Error::last_os_error());
    }

    fn register(&self, addr: usize, len: usize, mode: u64) {
        let range = UffdioRange { start: addr as u64, len: len as u64 };
        let mut register = UffdioRegister { range, mode, ioctls: 0 };
        ioctl(&self.uffd, UFFDIO_REGISTER, &mut register)
            .unwrap_or_else(|err| panic!("register {addr:#x} with userfaultfd: {err}"));
    }

    fn write_protect(&self, addr: usize, len: usize, protect: bool) {
        let range = UffdioRange { start: addr as u64, len: len as u64 };
        let mode = if protect { UFFDIO_WRITEPROTECT_MODE_WP } else { 0 };
        let mut write_protect = UffdioWriteprotect { range, mode };
        ioctl(&self.uffd, UFFDIO_WRITEPROTECT, &mut write_protect)
            .unwrap_or_else(|err| panic!("write-protect {addr:#x} with userfaultfd: {err}"));
    }

    fn populate_write(&self, addr: usize, len: usize) {
        // SAFETY: Faults in the second stage's pages, without changing them.
        let status = unsafe { libc::madvise(addr as *mut _, len, libc::MADV_POPULATE_WRITE) };
        assert_eq!(status, 0, "populate {addr:#x}: {}", std::io::Error::last_os_error());
    }

    fn wake(&self, addr: usize, len: usize) {
        let mut range = UffdioRange { start: addr as u64, len: len as u64 };
        ioctl(&self.uffd, UFFDIO_WAKE, &mut range).expect("wake a faulting thread");
    }
}

/// The rules of the order in which the second stage changes its memory, each of which its Shuttle
/// tests show to be needed by turning it off (TASK-44.3).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rules {
    /// A page mapped here has the window's protection until its stores are tracked.
    pub map_with_window_protection: bool,
    /// A page the window maps read-only is write-protected before it is made writable.
    pub write_protect_before_writable: bool,
    /// Memory unmapped here is inaccessible until it is registered for missing faults.
    pub register_before_accessible: bool,
    /// A store the window refuses makes its page read-only here.
    pub refused_store_read_only: bool,
    /// A refused store's page is made read-only, and a page withheld inaccessible, between the
    /// window's changes to the memory, and only if the page hasn't changed since the fault; a page
    /// withheld is recorded so, to be given back.
    pub restrict_between_changes: bool,
    /// A page withheld is given back when the window maps it anew.
    pub mapping_gives_back_withheld: bool,
    /// fxdax: a page a refused store made read-only is made writable again when the window makes
    /// it writable in place (TASK-102).
    pub protection_lifts_restriction: bool,
}

impl Rules {
    pub const ALL: Self = Self {
        map_with_window_protection: true,
        write_protect_before_writable: true,
        register_before_accessible: true,
        refused_store_read_only: true,
        restrict_between_changes: true,
        mapping_gives_back_withheld: true,
        protection_lifts_restriction: true,
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

/// The kernel's physical memory, by arena (TASK-43), which a window's pages are mapped from.
pub(crate) enum Memories {
    /// This process's kernel's, every arena's, as tests have it.
    #[cfg(test)]
    InProcess,
    /// The arenas' a daemon gave this process: its tenant's, and the default, read-only, or the
    /// default alone.
    Given(Vec<(crate::kernel::ArenaId, OwnedFd)>),
}

impl Memories {
    /// The descriptor of `arena`'s memory, which the window maps pages of, and whether it is
    /// read-only, as a tenant's VM process holds the default arena.
    fn fd(&self, arena: crate::kernel::ArenaId) -> Option<(std::os::fd::RawFd, bool)> {
        let fd = match self {
            #[cfg(test)]
            Self::InProcess => crate::kernel::arena_memory(arena)?.as_raw_fd(),
            Self::Given(memories) => {
                memories.iter().find(|(given, _)| *given == arena)?.1.as_raw_fd()
            }
        };
        // SAFETY: Plain system call on a descriptor the memories hold.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        Some((fd, flags >= 0 && flags & libc::O_ACCMODE == libc::O_RDONLY))
    }
}

impl UffdSecondStage {
    /// Takes over the `size` bytes of host memory at `base`, replacing them with memory whose
    /// faults userfaultfd reports, as nothing is mapped yet: the window's pages are mapped there
    /// from `phys`, the kernel's physical memory. `prefault`, if any, is told of the pages mapped
    /// there.
    ///
    /// # Safety
    ///
    /// The memory must be a page-aligned mapping that nothing else in the process uses.
    pub unsafe fn new(
        base: usize,
        size: u64,
        memories: Memories,
        prefault: Option<Arc<dyn Prefault>>,
    ) -> Result<Arc<Self>, Error> {
        // SAFETY: As the caller promises.
        unsafe { Self::with_budget(base, size, memories, prefault, default_budget()) }
    }

    /// As [`new`](Self::new), keeping at most `budget` runs of pages installed.
    ///
    /// # Safety
    ///
    /// As [`new`](Self::new).
    pub(crate) unsafe fn with_budget(
        base: usize,
        size: u64,
        memories: Memories,
        prefault: Option<Arc<dyn Prefault>>,
        budget: usize,
    ) -> Result<Arc<Self>, Error> {
        let uffd = open_userfaultfd()?;
        let mut api =
            UffdioApi { api: UFFD_API, features: UFFD_FEATURE_WP_HUGETLBFS_SHMEM, ioctls: 0 };
        ioctl(&uffd, UFFDIO_API, &mut api)
            .map_err(|err| Error::new(anyhow::Error::new(err).context("UFFDIO_API")))?;
        // SAFETY: As the caller promises.
        unsafe { Self::with_host(base, size, Syscalls { uffd }, memories, prefault, budget) }
    }

    /// As [`new`](Self::new), for a window of this process's kernel, as tests have it.
    ///
    /// # Safety
    ///
    /// As [`new`](Self::new).
    #[cfg(test)]
    pub unsafe fn in_process(
        base: usize,
        size: u64,
        prefault: Option<Arc<dyn Prefault>>,
    ) -> Result<Arc<Self>, Error> {
        // SAFETY: As the caller promises.
        unsafe { Self::new(base, size, Memories::InProcess, prefault) }
    }

    /// Serves the faults on the memory with `window`, on a thread of its own, until the returned
    /// thread is dropped.
    pub fn serve(self: &Arc<Self>, window: Arc<dyn Faults>) -> Result<FaultThread, Error> {
        assert_eq!(window.size(), self.size as u64, "the window is not the second stage's size");
        let stop = Arc::new(event_fd()?);
        let wake = Arc::new(EventFdWaker(event_fd()?));
        let thread_stop = stop.clone();
        let stage = self.clone();
        let thread = crate::thread::spawn("dax faults", move || {
            stage.serve_faults(&*window, &thread_stop, wake)
        })
        .map_err(Error::new)?;
        Ok(FaultThread { stop, thread: Some(thread) })
    }

    /// Passes the memory's faults to `window` until `stop` is signalled. Faults are resolved
    /// concurrently: each is a future, which this thread polls once `wake`, its waker, signals that
    /// it can make progress, as when Fxfs's pager supplies the page it waits for.
    fn serve_faults(&self, window: &dyn Faults, stop: &File, wake: Arc<EventFdWaker>) {
        let waker = Waker::from(wake.clone());
        let mut cx = Context::from_waker(&waker);
        let mut faults = FuturesUnordered::new();
        let mut fds = [
            libc::pollfd { fd: self.host.uffd.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: stop.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: wake.0.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        loop {
            // Make what progress the faults can, and then wait for new faults or for a fault's
            // waker.
            while let Poll::Ready(Some(())) = faults.poll_next_unpin(&mut cx) {}
            // SAFETY: `fds` is an array of `pollfd`s of its length.
            let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
            if ready < 0 {
                let err = std::io::Error::last_os_error();
                assert_eq!(err.kind(), std::io::ErrorKind::Interrupted, "poll userfaultfd: {err}");
                continue;
            }
            if fds[1].revents != 0 {
                return;
            }
            if fds[2].revents != 0 {
                wake.reset();
            }
            while let Some(msg) = self.read_event() {
                if msg.event == UFFD_EVENT_PAGEFAULT {
                    faults.push(self.resolve(window, msg.address as usize, msg.flags));
                }
            }
        }
    }

    /// Reads the next event, if there is one.
    fn read_event(&self) -> Option<UffdMsg> {
        let mut msg = std::mem::MaybeUninit::<UffdMsg>::uninit();
        // SAFETY: Reads one message into `msg`, which is its size.
        let read = unsafe {
            libc::read(self.host.uffd.as_raw_fd(), msg.as_mut_ptr().cast(), size_of::<UffdMsg>())
        };
        if read < 0 {
            let err = std::io::Error::last_os_error();
            assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock, "read userfaultfd: {err}");
            return None;
        }
        assert_eq!(read as usize, size_of::<UffdMsg>(), "short read of a userfaultfd message");
        // SAFETY: The kernel wrote a whole message.
        Some(unsafe { msg.assume_init() })
    }
}

impl<H: HostMemory> UffdSecondStage<H> {
    /// As [`new`](UffdSecondStage::new), changing the memory with `host`.
    ///
    /// # Safety
    ///
    /// As [`new`](UffdSecondStage::new), if `host` changes it.
    pub(crate) unsafe fn with_host(
        base: usize,
        size: u64,
        host: H,
        memories: Memories,
        prefault: Option<Arc<dyn Prefault>>,
        budget: usize,
    ) -> Result<Arc<Self>, Error> {
        let size = usize::try_from(size).map_err(|_| Errno::EINVAL)?;
        let page_size = page_size() as usize;
        if !base.is_multiple_of(page_size) || size == 0 || !size.is_multiple_of(page_size) {
            return Err(Error::msg("DAX window memory must be whole pages"));
        }
        let pages = vec![PageChanges::default(); size / page_size];
        let pages = Mutex::new(Pages { pages, runs: 0, clock: 0 });
        let budget = budget.max(1);
        let taken = Taken::default();
        let stage = Arc::new(Self { base, size, host, memories, prefault, pages, budget, taken });
        stage.unmap(0, size as u64);
        Ok(stage)
    }

    /// The index of the page at `addr`, and of the pages of the `len` bytes there.
    fn page_range(&self, addr: usize, len: usize) -> std::ops::Range<usize> {
        let page_size = page_size() as usize;
        let first = (addr - self.base) / page_size;
        first..first + len.div_ceil(page_size)
    }

    /// Makes the window's change to the `len` bytes at `addr`: `record`s it in the pages, and
    /// makes it to the memory with `change`, given what `record` returns, holding the pages' lock,
    /// and counts it for each page, which is withheld no more: the change replaces its memory.
    fn change<T>(
        &self,
        addr: usize,
        len: usize,
        record: impl FnOnce(&mut Pages) -> T,
        change: impl FnOnce(T),
    ) {
        let mut pages = self.pages.lock().unwrap();
        let recorded = record(&mut pages);
        if !rules().restrict_between_changes {
            drop(pages);
            return change(recorded);
        }
        for page in &mut pages.pages[self.page_range(addr, len)] {
            page.made = page.made.wrapping_add(1);
            page.withheld = false;
        }
        change(recorded);
    }

    /// What a fault finds of the page at `addr`, which a restriction of the page checks: the
    /// changes the window has made to it, as the rules have restrictions check them.
    fn seen(&self, addr: usize) -> Option<u32> {
        let page = self.page_range(addr, 1).start;
        rules().restrict_between_changes.then(|| self.pages.lock().unwrap().pages[page].made)
    }

    /// Restricts the page at `addr` to `prot`, for a fault the window did not resolve, and so
    /// withholds it if it is made inaccessible: unless the window changed it since it had made
    /// `seen` changes to it, whose new memory the access finds when it retries. Without `seen`, at
    /// once.
    fn restrict(&self, addr: usize, seen: Option<u32>, prot: libc::c_int) {
        let page_size = page_size() as usize;
        let page = self.page_range(addr, 1).start;
        let withheld = prot == libc::PROT_NONE;
        let offset = (addr - self.base) as u64;
        let mut pages = self.pages.lock().unwrap();
        let made = pages.pages[page].made;
        if seen.is_some_and(|seen| made != seen) {
            return;
        }
        self.host.protect(addr, page_size, prot);
        pages.pages[page].withheld = withheld;
        pages.pages[page].restricted = prot == libc::PROT_READ;
        changed_protection("restricted to reads", offset, made, seen);
    }

    /// fxdax: the changes made so far that took access away from pages installed, by how
    /// ([`Taken`]).
    pub(crate) fn taken(&self) -> TakenCounts {
        let taken = &self.taken;
        [&taken.overwritten, &taken.left_out, &taken.write_protected]
            .map(|count| count.load(Ordering::Relaxed))
    }

    /// fxdax: counts a change that took access away from pages installed, `count` by how.
    fn took(count: &AtomicU64, took: bool) {
        if took {
            count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The size of the memory: its window's, as a test's window in it takes it
    /// ([`Dax::in_host_memory`](crate::dax::Dax::in_host_memory)), or a VM process's of the
    /// extent daemon's.
    pub fn size(&self) -> u64 {
        self.size as u64
    }

    /// Resolves the fault at host address `address` described by `flags`, and wakes the faulting
    /// thread.
    pub(crate) async fn resolve(&self, window: &dyn Faults, address: usize, flags: u64) {
        let page_size = page_size() as usize;
        let addr = address - address % page_size;
        let offset = (addr - self.base) as u64;
        let access = if flags & (UFFD_PAGEFAULT_FLAG_WRITE | UFFD_PAGEFAULT_FLAG_WP) != 0 {
            Access::Write
        } else {
            Access::Read
        };
        // A store to a page mapped here read-only is the guest's: KVM faults loads of pages mapped
        // here without userfaultfd. A fault on a page not mapped here may be for writing whatever
        // the guest's access (see the module docs), and resolves that page alone.
        let store = flags & UFFD_PAGEFAULT_FLAG_WP != 0;
        // A page the window maps, left out past the budget: installed again, as it was.
        if !store && self.reinstall(addr) {
            self.host.wake(addr, page_size);
            return;
        }
        // The page as the access found it, which a fault the window does not resolve restricts.
        let seen = self.seen(addr);
        let resolved = if store {
            window.fault(offset, access).await
        } else {
            window.fault_page(offset, access).await
        };
        let resolved = match resolved {
            // Where the window refuses a write, the page is mapped read-only: make it read-only
            // here too, so that the write fails in the kernel rather than faulting here again. A
            // store from the guest then fails as KVM's would.
            Err(Errno::EACCES) if access == Access::Write => {
                refused("a store to a page the window maps read-only", offset);
                if rules().refused_store_read_only {
                    self.restrict(addr, seen, libc::PROT_READ);
                }
                Ok(())
            }
            // Where the window maps nothing, the fault is KVM's, for a page the guest has
            // unmapped (see the module docs): it is withheld, so that the fault fails.
            Err(Errno::ENOENT) => {
                refused("an access to a page the window maps nothing at", offset);
                self.restrict(addr, seen, libc::PROT_NONE);
                Ok(())
            }
            resolved => resolved,
        };
        if let Err(err) = resolved {
            let why =
                format!("unresolved DAX window fault at offset {offset:#x} ({access:?}): {err}");
            log::error!("{why}");
            // fxdax: on standard error too, which a process without a logger, a test's, keeps:
            // its abort said nothing else (TASK-100).
            eprintln!("fxdax: {why}");
            std::process::abort();
        }
        self.host.wake(addr, page_size);
        // A write to a page not mapped here may start a run a sequential writer stores to: the pages
        // after it are mapped read-only, the next read in if need be, so that stores to them fault
        // as stores, which dirty runs of pages, and loads do not fault at all. Past the file, or
        // the window, there is nothing to map.
        let next = offset + page_size as u64;
        if !store && access == Access::Write && next < window.size() {
            match window.fault_page(next, Access::Read).await {
                Ok(()) | Err(Errno::ENOENT | Errno::EINVAL) => {}
                Err(err) => log::warn!("mapping DAX window offset {next:#x} ahead: {err}"),
            }
        }
    }

    /// The host address of the `len` bytes at `offset`, as a length in host terms.
    fn addr(&self, offset: u64, len: u64) -> (usize, usize) {
        let (offset, len) = (offset as usize, len as usize);
        assert!(offset <= self.size && len <= self.size - offset, "out of the second stage");
        (self.base + offset, len)
    }
}

impl<H: HostMemory> SecondStage for UffdSecondStage<H> {
    fn map(
        &self,
        offset: u64,
        phys: crate::kernel::PhysLocation,
        len: u64,
        protection: Protection,
    ) {
        let (addr, len) = self.addr(offset, len);
        assert_ne!(protection, Protection::None, "a guest's pages are always readable");
        let memory = self.memory(phys, protection);
        let range = self.page_range(addr, len);
        let page_size = page_size();
        self.change(
            addr,
            len,
            |pages| {
                let used = pages.tick();
                Self::took(&self.taken.overwritten, pages.any_installed(range.clone()));
                pages.update(range, |pages| {
                    for (at, page) in pages.iter_mut().enumerate() {
                        let offset = phys.offset + at as u64 * page_size;
                        let phys = crate::kernel::PhysLocation { arena: phys.arena, offset };
                        page.mapped = Some(Mapped { phys, protection, installed: true, used });
                        page.restricted = false;
                    }
                })
            },
            |()| self.install(addr, len, protection, memory),
        );
        self.trim();
        if let Some(prefault) = &self.prefault {
            prefault.prefault(offset, len as u64);
        }
    }

    fn protect(&self, offset: u64, len: u64, protection: Protection) {
        assert_ne!(protection, Protection::None, "a guest's pages are always readable");
        let (addr, len) = self.addr(offset, len);
        let range = self.page_range(addr, len);
        let page_size = page_size() as usize;
        // The pages left out take the protection when they are installed again. fxdax: those a
        // refused store made read-only made writable again, if the window makes them writable.
        let lifts = protection == Protection::ReadWrite && rules().protection_lifts_restriction;
        let installed = |pages: &mut Pages| {
            let mut installed: Vec<std::ops::Range<usize>> = Vec::new();
            let mut lifted = Vec::new();
            for at in range.clone() {
                let page = &mut pages.pages[at];
                // Installed or not: memory left out, made writable, faults to the window.
                if lifts && page.restricted {
                    page.restricted = false;
                    lifted.push((at, page.made));
                }
                let Some(mapped) = &mut page.mapped else { continue };
                mapped.protection = protection;
                if !mapped.installed {
                    continue;
                }
                match installed.last_mut() {
                    Some(run) if run.end == at => run.end = at + 1,
                    _ => installed.push(at..at + 1),
                }
            }
            (installed, lifted)
        };
        self.change(addr, len, installed, |(installed, lifted)| {
            for (at, made) in lifted {
                self.host.protect(
                    self.base + at * page_size,
                    page_size,
                    libc::PROT_READ | libc::PROT_WRITE,
                );
                changed_protection("made writable again", (at * page_size) as u64, made, None);
            }
            for run in installed {
                let addr = self.base + run.start * page_size;
                Self::took(&self.taken.write_protected, protection == Protection::Read);
                self.host.write_protect(
                    addr,
                    run.len() * page_size,
                    protection == Protection::Read,
                );
            }
        });
    }

    fn unmap(&self, offset: u64, len: u64) {
        let (addr, len) = self.addr(offset, len);
        let range = self.page_range(addr, len);
        let forget = |pages: &mut Pages| {
            Self::took(&self.taken.left_out, pages.any_installed(range.clone()));
            pages.update(range, |pages| {
                pages.iter_mut().for_each(|page| (page.mapped, page.restricted) = (None, false))
            })
        };
        self.change(addr, len, forget, |()| self.leave_out(addr, len));
    }

    /// fxdax: the window's change of a range it mapped, made at once (TASK-91.16): each map of the
    /// very pages installed where it maps them, as a seal's of the open extent's pages, read-only
    /// now, changes their protection in place, the memory staying; another's memory is installed
    /// over what was there, in one `mmap`; and only what no map covers is left out. Made in steps,
    /// each page would be left out, then installed again: an unmap of the old memory, flushing
    /// the host's TLBs and the hypervisor's mappings, then a map, its registration and its
    /// protection, then the guest's accesses faulting the pages in again.
    fn remap(&self, offset: u64, len: u64, anew: &[Anew]) {
        let (addr, len) = self.addr(offset, len);
        let range = self.page_range(addr, len);
        let page_size = page_size() as usize;
        let maps: Vec<(usize, usize, crate::kernel::PhysLocation, Protection)> = (anew.iter())
            .filter_map(|anew| match *anew {
                Anew::Map { offset, phys, len, protection } => {
                    assert_ne!(protection, Protection::None, "a guest's pages are always readable");
                    let (addr, len) = self.addr(offset, len);
                    Some((addr, len, phys, protection))
                }
                Anew::Mapped { .. } => None,
            })
            .collect();
        let record = |pages: &mut Pages| {
            let used = pages.tick();
            let in_place: Vec<Option<Protection>> = (maps.iter())
                .map(|&(addr, len, phys, _)| pages.installed_as(self.page_range(addr, len), phys))
                .collect();
            for (&(addr, len, ..), in_place) in maps.iter().zip(&in_place) {
                let over = in_place.is_none() && pages.any_installed(self.page_range(addr, len));
                Self::took(&self.taken.overwritten, over);
            }
            let installed_before: Vec<bool> = (range.clone())
                .map(|at| pages.pages[at].mapped.is_some_and(|mapped| mapped.installed))
                .collect();
            pages.update(range.clone(), |pages| {
                pages.iter_mut().for_each(|page| (page.mapped, page.restricted) = (None, false))
            });
            let mut covered = vec![false; range.len()];
            for &(addr, len, phys, protection) in &maps {
                let mapping = self.page_range(addr, len);
                covered[mapping.start - range.start..mapping.end - range.start].fill(true);
                pages.update(mapping, |pages| {
                    for (at, page) in pages.iter_mut().enumerate() {
                        let offset = phys.offset + at as u64 * page_size as u64;
                        let phys = crate::kernel::PhysLocation { offset, ..phys };
                        page.mapped = Some(Mapped { phys, protection, installed: true, used });
                    }
                });
            }
            // The runs of pages no map covers, which are left out.
            let mut left_out: Vec<std::ops::Range<usize>> = Vec::new();
            for (at, covered) in (range.clone()).zip(covered) {
                match left_out.last_mut() {
                    _ if covered => {}
                    Some(run) if run.end == at => run.end = at + 1,
                    _ => left_out.push(at..at + 1),
                }
            }
            for run in &left_out {
                let at = run.start - range.start..run.end - range.start;
                Self::took(&self.taken.left_out, installed_before[at].contains(&true));
            }
            (in_place, left_out)
        };
        // Whether each map was made in place.
        let mut reprotected = Vec::with_capacity(maps.len());
        self.change(addr, len, record, |(in_place, left_out)| {
            for (&(addr, len, phys, protection), was) in maps.iter().zip(in_place) {
                reprotected.push(was.is_some());
                match was {
                    Some(was) => self.reprotect(addr, len, was, protection),
                    None => self.install(addr, len, protection, self.memory(phys, protection)),
                }
            }
            for run in left_out {
                self.leave_out(self.base + run.start * page_size, run.len() * page_size);
            }
        });
        for anew in anew {
            if let Anew::Mapped { offset, len } = *anew {
                self.window_mapped(offset, len);
            }
        }
        self.trim();
        // The memory installed is mapped for the guest ahead of its accesses, as `map` has it;
        // the pages reprotected in place aren't: the hypervisor maps those the guest touches
        // again as it does, which a seal's, written and done with, mostly aren't, rather than
        // each at once (r116 against r117: 87 against 83 us of host time a window change).
        if let Some(prefault) = &self.prefault {
            for (&(addr, len, ..), reprotected) in maps.iter().zip(reprotected) {
                if !reprotected {
                    prefault.prefault((addr - self.base) as u64, len as u64);
                }
            }
        }
    }

    fn window_mapped(&self, offset: u64, len: u64) {
        let (addr, len) = self.addr(offset, len);
        let page_size = page_size() as usize;
        let range = self.page_range(addr, len);
        let first = range.start;
        let mut pages = self.pages.lock().unwrap();
        // Each page counts the mapping as a change, so that a fault that found nothing there
        // before withholds nothing now. A page withheld is unmapped memory, registered for
        // missing faults, made accessible again.
        for (at, page) in pages.pages[range].iter_mut().enumerate() {
            page.made = page.made.wrapping_add(1);
            if page.withheld && rules().mapping_gives_back_withheld {
                page.withheld = false;
                let addr = self.base + (first + at) * page_size;
                self.host.protect(addr, page_size, libc::PROT_READ | libc::PROT_WRITE);
            }
        }
    }
}

impl<H: HostMemory> UffdSecondStage<H> {
    /// The memory of the page at `phys`, which the window maps with `protection`.
    fn memory(&self, phys: crate::kernel::PhysLocation, protection: Protection) -> Memory {
        // A page of an arena the window may not map is the daemon's error, which the guest must
        // not get past.
        let (fd, read_only) =
            self.memories.fd(phys.arena).expect("a window's page is of its arenas");
        // Memory this process holds read-only, the default arena's, public bases' pages, is mapped
        // private: userfaultfd tracks stores to a mapping that may be made writable alone, and a
        // store to a private one could only copy the page into this process, which the window
        // never lets it make, as it maps such pages read-only. Its loads read the very pages.
        assert!(!read_only || protection == Protection::Read, "a read-only page mapped writably");
        Memory::File { fd, offset: phys.offset, shared: !read_only }
    }

    /// Installs `memory` at the `len` bytes at `addr`, which the window maps with `protection`.
    fn install(&self, addr: usize, len: usize, protection: Protection, memory: Memory) {
        let writable = libc::PROT_READ | libc::PROT_WRITE;
        let prot = match protection {
            Protection::Read if rules().map_with_window_protection => libc::PROT_READ,
            _ => writable,
        };
        self.host.map(addr, len, prot, memory);
        // Stores are tracked by write-protecting the page, rather than by its protection, whose
        // faults userfaultfd does not report. The new mapping has the window's protection until it
        // is write-protected, so that a store meanwhile fails rather than going untracked.
        self.host.register(addr, len, UFFDIO_REGISTER_MODE_WP);
        let write_protect = protection == Protection::Read;
        // Mapped writable already, it needs no protection changed (TASK-26.18.3).
        let make_writable = prot != writable;
        if make_writable && !rules().write_protect_before_writable {
            self.host.protect(addr, len, writable);
        }
        if write_protect {
            self.host.write_protect(addr, len, true);
        }
        if make_writable && rules().write_protect_before_writable {
            self.host.protect(addr, len, writable);
        }
        // The pages are mapped for the guest ahead of its accesses, as when a file is mapped, an
        // open extent's pages are mapped for the guest to write, or a fault maps the pages after
        // its own. A writable page is mapped writable here first: its first access through the new
        // mapping would map it read-only, as the mapping tracks stores, and the guest's first store
        // would then fault anyway.
        if protection == Protection::ReadWrite {
            self.host.populate_write(addr, len);
        }
    }

    /// fxdax: changes the protection of the pages installed at the `len` bytes at `addr`, which
    /// the window mapped with `was`, to `now`, their memory staying, as [`install`](Self::install)
    /// leaves memory it installs: tracked by write protection while read-only, and faulted in for
    /// writing once writable.
    fn reprotect(&self, addr: usize, len: usize, was: Protection, now: Protection) {
        match now {
            _ if now == was => {}
            Protection::Read => {
                Self::took(&self.taken.write_protected, true);
                self.host.write_protect(addr, len, true)
            }
            Protection::ReadWrite => {
                self.host.write_protect(addr, len, false);
                self.host.populate_write(addr, len);
            }
            Protection::None => unreachable!("a guest's pages are always readable"),
        }
    }

    /// Leaves the `len` bytes at `addr` out: memory registered for missing faults.
    fn leave_out(&self, addr: usize, len: usize) {
        // The new memory is inaccessible until it is registered, so that an access meanwhile fails
        // rather than faulting in a page of zeros.
        let writable = libc::PROT_READ | libc::PROT_WRITE;
        let prot = if rules().register_before_accessible { libc::PROT_NONE } else { writable };
        self.host.map(addr, len, prot, Memory::Anonymous);
        self.host.register(addr, len, UFFDIO_REGISTER_MODE_MISSING);
        self.host.protect(addr, len, writable);
    }

    /// While more runs than the budget are installed, leaves out those installed or faulted on
    /// longest ago, down to seven eighths of it, so that it is not done at each mapping.
    fn trim(&self) {
        let mut pages = self.pages.lock().unwrap();
        if pages.runs <= self.budget {
            return;
        }
        let page_size = page_size() as usize;
        // Each run, with when it was used last.
        let mut runs = Vec::new();
        let mut at = 0;
        while at < pages.pages.len() {
            if !pages.starts_run(at) {
                at += 1;
                continue;
            }
            let start = at;
            let mut used = 0;
            loop {
                used = used.max(pages.pages[at].mapped.map_or(0, |mapped| mapped.used));
                at += 1;
                let installed = at < pages.pages.len()
                    && pages.pages[at].mapped.is_some_and(|mapped| mapped.installed);
                if !installed || pages.starts_run(at) {
                    break;
                }
            }
            runs.push((used, start..at));
        }
        runs.sort_by_key(|(used, _)| *used);
        let low = self.budget - self.budget / 8;
        for (_, run) in runs {
            if pages.runs <= low {
                break;
            }
            pages.update(run.clone(), |pages| {
                for page in pages {
                    page.made = page.made.wrapping_add(1);
                    if let Some(mapped) = &mut page.mapped {
                        mapped.installed = false;
                        page.restricted = false;
                    }
                }
            });
            Self::took(&self.taken.left_out, true);
            self.leave_out(self.base + run.start * page_size, run.len() * page_size);
        }
    }

    /// Installs the page at `addr` again, if the window maps it and it was left out, with those
    /// left out around it whose memory follows on from it, as it was, and returns whether it did.
    /// A page installed is counted as used.
    fn reinstall(&self, addr: usize) -> bool {
        let page_size = page_size() as usize;
        let at = self.page_range(addr, 1).start;
        let mut pages = self.pages.lock().unwrap();
        let used = pages.tick();
        let Some(mapped) = &mut pages.pages[at].mapped else { return false };
        if mapped.installed {
            mapped.used = used;
            return false;
        }
        let (phys, protection) = (mapped.phys, mapped.protection);
        // The pages around it left out with it, of memory following on, as far as a chunk's.
        let left_out_with = |page: &PageChanges, distance: i64| {
            page.mapped.is_some_and(|other| {
                !other.installed
                    && other.protection == protection
                    && other.phys.arena == phys.arena
                    && other.phys.offset as i64 - phys.offset as i64 == distance * page_size as i64
            })
        };
        const MOST: usize = 512;
        let mut start = at;
        while start > 0
            && at - start < MOST
            && left_out_with(&pages.pages[start - 1], start as i64 - 1 - at as i64)
        {
            start -= 1;
        }
        let mut end = at + 1;
        while end < pages.pages.len()
            && end - start < MOST
            && left_out_with(&pages.pages[end], end as i64 - at as i64)
        {
            end += 1;
        }
        let first = crate::kernel::PhysLocation {
            arena: phys.arena,
            offset: phys.offset - ((at - start) * page_size) as u64,
        };
        pages.update(start..end, |pages| {
            for page in pages {
                page.made = page.made.wrapping_add(1);
                if let Some(mapped) = &mut page.mapped {
                    mapped.installed = true;
                    mapped.used = used;
                }
            }
        });
        let len = (end - start) * page_size;
        self.install(
            self.base + start * page_size,
            len,
            protection,
            self.memory(first, protection),
        );
        drop(pages);
        self.trim();
        true
    }
}

/// A new event file descriptor, which does not block reads.
fn event_fd() -> Result<File, Error> {
    // SAFETY: Creates an event file descriptor, which is owned below.
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if fd < 0 {
        return Err(Error::new(std::io::Error::last_os_error()));
    }
    // SAFETY: `fd` was just created, and is owned here.
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// Wakes the fault thread by signalling an event file descriptor it polls.
struct EventFdWaker(File);

impl EventFdWaker {
    /// Clears the signal, once the thread has woken.
    fn reset(&self) {
        let mut count = [0u8; 8];
        // A read fails with `EAGAIN` if a racing reset cleared the signal first.
        let _ = (&self.0).read(&mut count);
    }
}

impl Wake for EventFdWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        // A write fails with `EAGAIN` only if the counter would overflow, when it is signalled.
        let _ = (&self.0).write(&1u64.to_ne_bytes());
    }
}

/// The thread serving a second stage's faults, which stops when this is dropped.
pub(crate) struct FaultThread {
    stop: Arc<File>,
    thread: Option<crate::thread::Thread<()>>,
}

impl Drop for FaultThread {
    fn drop(&mut self) {
        (&*self.stop).write_all(&1u64.to_ne_bytes()).expect("stop the fault thread");
        if let Err(panic) = self.thread.take().unwrap().join() {
            std::panic::resume_unwind(panic);
        }
    }
}

/// Opens a userfaultfd that handles faults taken in the kernel as well as in user mode.
fn open_userfaultfd() -> Result<OwnedFd, Error> {
    let flags = libc::O_CLOEXEC | libc::O_NONBLOCK;
    // `/dev/userfaultfd`'s permissions grant handling kernel faults, where the system call needs
    // the `vm.unprivileged_userfaultfd` sysctl or CAP_SYS_PTRACE.
    let fd = match File::options().read(true).write(true).open("/dev/userfaultfd") {
        // SAFETY: Creates a userfaultfd, which is owned below.
        Ok(dev) => unsafe { libc::ioctl(dev.as_raw_fd(), USERFAULTFD_IOC_NEW as _, flags) },
        // SAFETY: As above.
        Err(_) => unsafe { libc::syscall(libc::SYS_userfaultfd, flags) as libc::c_int },
    };
    if fd < 0 {
        return Err(Error::new(anyhow::Error::new(std::io::Error::last_os_error()).context(
            "userfaultfd: handling a guest's faults needs access to /dev/userfaultfd, or \
             vm.unprivileged_userfaultfd=1",
        )));
    }
    // SAFETY: `fd` was just created, and is owned here.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Issues userfaultfd ioctl `request` with `arg`.
fn ioctl<T>(uffd: &OwnedFd, request: u64, arg: &mut T) -> std::io::Result<()> {
    // SAFETY: `arg` is the structure `request` takes.
    if unsafe { libc::ioctl(uffd.as_raw_fd(), request as _, arg as *mut T) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(all(test, feature = "shuttle"))]
mod shuttle_tests;

#[cfg(test)]
mod tests {
    use super::super::Dax;
    use super::super::tests::TestPager;
    use super::*;

    /// The fault thread resolves faults concurrently: a fault waiting for the pager does not hold
    /// up the next, whose page request reaches the pager first.
    #[test]
    fn faults_are_resolved_concurrently() {
        let ps = page_size() as usize;
        // SAFETY: Maps anonymous memory, which this test owns.
        let memory = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                2 * ps,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(memory, libc::MAP_FAILED);
        let base = memory as usize;
        let pager = TestPager::new(2);
        // SAFETY: Only the test uses the memory, which outlives the window.
        let stage = unsafe { UffdSecondStage::in_process(base, 2 * ps as u64, None) }.unwrap();
        let (dax, _window) = Dax::<()>::in_host_memory(stage).unwrap();
        dax.window.map(0, &pager.vmo, 0, 2 * ps as u64, false).unwrap();

        // SAFETY: The memory is the window's, whose faults the fault thread serves.
        let load =
            move |offset: usize| move || unsafe { ((base + offset) as *const u8).read_volatile() };
        let first = std::thread::spawn(load(0));
        assert_eq!(pager.read_request(zx::MonotonicInstant::INFINITE), 0..ps as u64);
        let second = std::thread::spawn(load(ps));
        assert_eq!(pager.read_request(zx::MonotonicInstant::INFINITE), ps as u64..2 * ps as u64);

        pager.supply(1, 2);
        assert_eq!(second.join().unwrap(), 2);
        assert!(!first.is_finished());
        pager.supply(0, 1);
        assert_eq!(first.join().unwrap(), 1);

        drop(dax);
        // SAFETY: Unmaps the memory, which nothing uses any more.
        assert_eq!(unsafe { libc::munmap(memory, 2 * ps) }, 0);
    }

    /// Stores to pages mapped read-only, of a file whose pager tracks its dirty pages, fault as
    /// stores, which the window resolves once the pager dirties the pages, and the stores land in
    /// the file. A store following on from the last dirties the file's pages after it too.
    #[test]
    fn stores_to_clean_pages_wait_for_the_pager_to_dirty_them() {
        let ps = page_size() as usize;
        // SAFETY: Maps anonymous memory, which this test owns.
        let memory = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                4 * ps,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(memory, libc::MAP_FAILED);
        let base = memory as usize;
        let port = zx::Port::create();
        let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
        let vmo = pager.create_vmo(zx::VmoOptions::TRAP_DIRTY, &port, 7, 4 * ps as u64).unwrap();
        let aux = zx::Vmo::create(4 * ps as u64).unwrap();
        aux.write(&vec![5; 4 * ps], 0).unwrap();
        pager.supply_pages(&vmo, 0..4 * ps as u64, &aux, 0).unwrap();
        // SAFETY: Only the test uses the memory, which outlives the window.
        let stage = unsafe { UffdSecondStage::in_process(base, 4 * ps as u64, None) }.unwrap();
        let (dax, _window) = Dax::<()>::in_host_memory(stage).unwrap();
        dax.window.map(0, &vmo, 0, 4 * ps as u64, true).unwrap();

        // SAFETY: The memory is the window's, whose faults the fault thread serves.
        let load = move |page: usize| unsafe { ((base + page * ps) as *const u8).read_volatile() };
        let store = move |page: usize| {
            // SAFETY: As for loads.
            std::thread::spawn(move || unsafe { ((base + page * ps) as *mut u8).write_volatile(9) })
        };
        let dirtied = || {
            let packet = port.wait(zx::MonotonicInstant::INFINITE).unwrap();
            let zx::PacketContents::Pager(request) = packet.contents() else { panic!() };
            assert_eq!(request.command(), zx::sys::zx_page_request_command_t::ZX_PAGER_VMO_DIRTY);
            pager.op_range(zx::PagerOp::Dirty, &vmo, request.range()).unwrap();
            request.range()
        };
        assert_eq!((load(0), load(1)), (5, 5));
        let first = store(0);
        assert_eq!(dirtied(), 0..ps as u64);
        first.join().unwrap();
        let second = store(1);
        assert_eq!(dirtied(), ps as u64..4 * ps as u64);
        second.join().unwrap();
        let mut bytes = [0; 2];
        vmo.read(&mut bytes[..1], 0).unwrap();
        vmo.read(&mut bytes[1..], ps as u64).unwrap();
        assert_eq!(bytes, [9, 9]);

        drop(dax);
        // SAFETY: Unmaps the memory, which nothing uses any more.
        assert_eq!(unsafe { libc::munmap(memory, 4 * ps) }, 0);
    }

    /// A window whose pages are scattered over its arena's memory keeps no more runs installed
    /// than its budget, and so few mappings, leaving out those used longest ago, which read as
    /// they were when the guest touches them again (TASK-12).
    #[test]
    fn windows_keep_their_mappings_within_the_budget() {
        const PAGES: usize = 64;
        const BUDGET: usize = 8;
        let ps = page_size() as usize;
        // SAFETY: Maps anonymous memory, which this test owns.
        let memory = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                PAGES * ps,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(memory, libc::MAP_FAILED);
        let base = memory as usize;
        // The file's pages, written turn about with another's, are scattered: a run each.
        let (file, other) = (
            zx::Vmo::create((PAGES * ps) as u64).unwrap(),
            zx::Vmo::create((PAGES * ps) as u64).unwrap(),
        );
        for page in 0..PAGES {
            file.write(&vec![page as u8 + 1; ps], (page * ps) as u64).unwrap();
            other.write(&vec![0xff; ps], (page * ps) as u64).unwrap();
        }
        // SAFETY: Only the test uses the memory, which outlives the window.
        let stage = unsafe {
            UffdSecondStage::with_budget(
                base,
                (PAGES * ps) as u64,
                Memories::InProcess,
                None,
                BUDGET,
            )
        }
        .unwrap();
        let (dax, _window) = Dax::<()>::in_host_memory(stage.clone()).unwrap();
        dax.window.map(0, &file, 0, (PAGES * ps) as u64, false).unwrap();

        // The mappings in the window's memory: its runs installed, and what lies between them.
        let mappings = || {
            let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
            maps.lines()
                .filter(|line| {
                    let range = line.split_whitespace().next().unwrap();
                    let (start, end) = range.split_once('-').unwrap();
                    let (start, end) = (
                        usize::from_str_radix(start, 16).unwrap(),
                        usize::from_str_radix(end, 16).unwrap(),
                    );
                    start < base + PAGES * ps && end > base
                })
                .count()
        };
        let left_out = || {
            let pages = stage.pages.lock().unwrap();
            pages
                .pages
                .iter()
                .filter(|page| page.mapped.is_some_and(|mapped| !mapped.installed))
                .count()
        };
        let within = |when: &str| {
            let runs = stage.pages.lock().unwrap().runs;
            assert!(runs <= BUDGET, "{when}: {runs} runs installed");
            assert!(mappings() <= 2 * BUDGET + 1, "{when}: {} mappings", mappings());
        };
        within("mapped");
        assert!(left_out() >= PAGES - BUDGET, "{} pages left out", left_out());

        // SAFETY: The memory is the window's, whose faults the fault thread serves.
        let load = |page: usize| unsafe { ((base + page * ps) as *const u8).read_volatile() };
        for round in 0..2 {
            for page in 0..PAGES {
                assert_eq!(load(page), page as u8 + 1, "round {round}, page {page}");
                within(&format!("round {round}, page {page}"));
            }
        }

        drop(dax);
        // SAFETY: Unmaps the memory, which nothing uses any more.
        assert_eq!(unsafe { libc::munmap(memory, PAGES * ps) }, 0);
    }

    /// TASK-91.16: the changes that take access away from pages installed are counted by how,
    /// each a flush of the host's TLBs and the hypervisor's: a map of other memory over them, a map
    /// of the very same pages read-only, made in place, and an unmap. A map where nothing is
    /// installed takes nothing away.
    #[test]
    fn changes_taking_access_from_pages_installed_are_counted() {
        let ps = page_size() as usize;
        // SAFETY: Maps anonymous memory, which this test owns.
        let memory = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                2 * ps,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(memory, libc::MAP_FAILED);
        let base = memory as usize;
        let file = zx::Vmo::create((2 * ps) as u64).unwrap();
        file.write(&vec![1; 2 * ps], 0).unwrap();
        // SAFETY: Only the test uses the memory, which outlives the window.
        let stage =
            unsafe { UffdSecondStage::new(base, (2 * ps) as u64, Memories::InProcess, None) }
                .unwrap();
        let (dax, _window) = Dax::<()>::in_host_memory(stage.clone()).unwrap();
        dax.window.map(0, &file, 0, (2 * ps) as u64, true).unwrap();
        assert_eq!(stage.taken(), [0, 0, 0], "mapped where nothing was");
        let phys = |page: usize| stage.pages.lock().unwrap().pages[page].mapped.unwrap().phys;
        let first = phys(0);
        // The first page's own read-only, in place; and the first page's again at the second's.
        stage.remap(
            0,
            (2 * ps) as u64,
            &[
                Anew::Map { offset: 0, phys: first, len: ps as u64, protection: Protection::Read },
                Anew::Map {
                    offset: ps as u64,
                    phys: first,
                    len: ps as u64,
                    protection: Protection::Read,
                },
            ],
        );
        assert_eq!(stage.taken(), [1, 0, 1], "the second page installed over, the first protected");
        stage.unmap(0, (2 * ps) as u64);
        assert_eq!(stage.taken(), [1, 1, 1], "both left out");
        stage.unmap(0, (2 * ps) as u64);
        assert_eq!(stage.taken(), [1, 1, 1], "nothing installed to leave out");

        drop(dax);
        // SAFETY: Unmaps the memory, which nothing uses any more.
        assert_eq!(unsafe { libc::munmap(memory, 2 * ps) }, 0);
    }

    /// TASK-12: a window of more scattered pages than the process may make mappings
    /// (`vm.max_map_count`), each a run of its own, read through whole, twice: within the default
    /// budget, every page reads as it should, where a mapping each would fail.
    #[test]
    fn windows_larger_than_the_map_count_read_whole() {
        let max = std::fs::read_to_string("/proc/sys/vm/max_map_count").unwrap();
        let pages = max.trim().parse::<usize>().unwrap() + MAPPINGS_RESERVED;
        let ps = page_size() as usize;
        // Room for the file and another, and the watchdog's thresholds.
        let memory = (4 * pages * ps).next_multiple_of(1 << 30) as u64;
        crate::kernel::configure(crate::kernel::Config::with_memory(memory)).unwrap();
        // SAFETY: Maps anonymous memory, which this test owns.
        let window = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                pages * ps,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
                -1,
                0,
            )
        };
        assert_ne!(window, libc::MAP_FAILED);
        let base = window as usize;
        // The file's pages, written turn about with another's, are scattered: a run each. (Mapped
        // in pieces instead, the VMO's mappings take long to remove: TASK-53.)
        let (file, other) = (
            zx::Vmo::create((pages * ps) as u64).unwrap(),
            zx::Vmo::create((pages * ps) as u64).unwrap(),
        );
        let tag = |page: usize| (page % 251) as u8 + 1;
        for page in 0..pages {
            file.write(&vec![tag(page); ps], (page * ps) as u64).unwrap();
            other.write(&[0xff], (page * ps) as u64).unwrap();
        }
        drop(other);
        // SAFETY: Only the test uses the memory, which outlives the window.
        let stage =
            unsafe { UffdSecondStage::new(base, (pages * ps) as u64, Memories::InProcess, None) }
                .unwrap();
        let (dax, _window) = Dax::<()>::in_host_memory(stage.clone()).unwrap();
        dax.window.map(0, &file, 0, (pages * ps) as u64, false).unwrap();
        // SAFETY: The memory is the window's, whose faults the fault thread serves.
        let load = |page: usize| unsafe { ((base + page * ps) as *const u8).read_volatile() };
        for round in 0..2 {
            for page in 0..pages {
                assert_eq!(load(page), tag(page), "round {round}, page {page}");
            }
            let runs = stage.pages.lock().unwrap().runs;
            assert!(runs <= default_budget(), "round {round}: {runs} runs installed");
        }
        drop(dax);
        // SAFETY: Unmaps the memory, which nothing uses any more.
        assert_eq!(unsafe { libc::munmap(window, pages * ps) }, 0);
    }

    /// An access to a page the window maps nothing at, as x86 KVM makes for a guest that has
    /// unmapped the page (see the module docs), fails with `EFAULT`, rather than ending the
    /// process. Mapped again, from memory not yet in memory, as a file's pages before Fxfs's pager
    /// supplies them, the page is accessible: it is mapped here only as a fault brings it in. The
    /// accesses are the kernel's, as KVM's are: a pipe's writes from the page.
    #[test]
    fn accesses_to_pages_unmapped_fail() {
        let ps = page_size() as usize;
        // SAFETY: Maps anonymous memory, which this test owns.
        let memory = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                ps,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(memory, libc::MAP_FAILED);
        let base = memory as usize;
        // SAFETY: Only the test uses the memory, which outlives the window.
        let stage = unsafe { UffdSecondStage::in_process(base, ps as u64, None) }.unwrap();
        let (dax, _window) = Dax::<()>::in_host_memory(stage).unwrap();
        let mut pipe = [0; 2];
        // SAFETY: Creates a pipe, which the test closes.
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        // SAFETY: Writes from the window's memory, which outlives the call.
        let write = || unsafe { libc::write(pipe[1], memory, ps) };

        assert_eq!(write(), -1);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EFAULT));

        // Not yet written, the VMO has no page in memory, and its first is zeros.
        let vmo = zx::Vmo::create(ps as u64).unwrap();
        dax.window.map(0, &vmo, 0, ps as u64, false).unwrap();
        assert_eq!(write(), ps as isize);
        let mut read = vec![1u8; ps];
        // SAFETY: Reads into `read`, which is `ps` bytes.
        assert_eq!(unsafe { libc::read(pipe[0], read.as_mut_ptr().cast(), ps) }, ps as isize);
        assert!(read.iter().all(|byte| *byte == 0), "the page mapped again reads otherwise");

        drop(dax);
        // SAFETY: Closes the pipe and unmaps the memory, which nothing uses any more.
        unsafe {
            libc::close(pipe[0]);
            libc::close(pipe[1]);
            assert_eq!(libc::munmap(memory, ps), 0);
        }
    }
}
