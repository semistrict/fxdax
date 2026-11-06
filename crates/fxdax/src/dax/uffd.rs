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
//! - A mapped page maps the window's page again, read-write, from physical memory
//!   (a memfd, which the process that owns the kernel may share with this one), registered for
//!   write-protect faults, and write-protected, as the window maps every page read-only.
//!
//! A fault thread passes the faults to the window, which maps the page (and so maps it here), and
//! wakes the faulting thread. It resolves faults concurrently, as futures it polls as they make
//! progress, so that a fault waiting for Fxfs's pager does not hold up the others. A fault the
//! window cannot resolve is fatal: the faulting thread cannot go on.
//!
//! A fault's access is not always the guest's. x86 KVM hands a fault it cannot resolve without
//! waiting, as it cannot one userfaultfd reports, to a worker that faults the page in for writing
//! (`async_pf_execute`), and lets the vCPU retry its own access once the page is in. So a write to
//! a page a window that doesn't copy on write maps, read-only, is resolved as a read, and the page
//! made read-only here, so that the worker's write fails and the vCPU's own access decides.
//!
//! fxdax: a window that copies on write (backlog doc-13, `CopyOnWrite`) makes a page private for a
//! store, which a load mustn't: the worker's write, of a page not here, is a load's as much as a
//! store's. So a fault for writing is the guest's store if it write-protected the page here, as a
//! load of a page here takes no fault, and if the faulting thread is one of the guest's own (its
//! vCPUs', [`guest_thread_started`]); a worker's write of a page not here is resolved as a load,
//! the page made read-only here so that the worker's write fails. The vCPU's own access decides: a
//! load is made; a store fails, `KVM_RUN` with `EFAULT`, and the VMM, which retries it, says so
//! ([`guest_access_failed`]), which lifts the restrictions such writes made: the store, retried,
//! write-protected here now, faults as one.
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
//! fails with `EFAULT` for a moment, giving up on one that keeps failing
//! ([`AccessFailures`](crate::AccessFailures)).
//!
//! Each run of pages mapped here, consecutive pages of one arena's memory, is a mapping of the
//! process's own, of which Linux allows `vm.max_map_count` (65,530 by default); a window's pages
//! scattered over its arenas would take one each (TASK-12). The stage keeps a budget of runs
//! installed, half of what the process may map beyond what it keeps for itself: past it, it leaves
//! out the runs installed or faulted on longest ago, memory registered for missing faults again,
//! whose pages it remembers, and installs one again, as it was, when the guest faults on it,
//! without the window. The window's mappings are unchanged.
//!
//! fxdax: the private pages of a window that copies on write (backlog doc-13) are none of those
//! runs. Its memory where nothing is installed is not anonymous but its private memory
//! ([`PrivateMemory`]): an object as large as the window, laid out as the window is, mapped shared
//! at the window's offsets, of which only the private pages are filled, so that the rest faults as
//! anonymous memory would. A page made private is filled there, its source's bytes copied in, and
//! the memory around it mapped over the run installed at it: it merges with the memory around it,
//! however many private pages there are, one mapping between two runs installed; a trim never
//! leaves it out, as it leaves out runs alone. Each held its own mapping of an arena's page once,
//! and a VM writing more pages than the budget had them left out and installed again by the
//! thousand, the fault thread trimming in place of serving faults (r544: 110,857 private pages
//! against a budget of some 30,000 runs).
//!
//! The userfaultfd must handle faults taken in the kernel, such as KVM's. That needs access to
//! `/dev/userfaultfd`, or the `vm.unprivileged_userfaultfd` sysctl.

use super::{Access, Anew, Faults, Prefault, SecondStage};
use crate::error::{Errno, Error, Result};
use crate::pages::page_size;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};
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
const UFFD_FEATURE_THREAD_ID: u64 = 1 << 8;
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
const UFFDIO_COPY: u64 = ioc(IOC_READ_WRITE, 0x03, size_of::<UffdioCopy>());
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

/// `struct uffdio_copy`.
#[repr(C)]
struct UffdioCopy {
    dst: u64,
    src: u64,
    len: u64,
    mode: u64,
    copy: i64,
}

const _: () = assert!(size_of::<UffdioCopy>() == 40);

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
    /// Physical memory, which the window's pages are mapped from.
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
    /// fxdax: its window's private memory, if the window copies on write (backlog doc-13): a
    /// worker's write of a page not here is resolved as a load then (see the module docs).
    private: OnceLock<PrivateMemory>,
    /// fxdax: when it was made, which its pages' last changes are told from
    /// ([`describe_windows`]).
    start: std::time::Instant,
    /// fxdax: what it is doing to its memory, if anything, and since when
    /// ([`describe_windows`]).
    busy: Busy,
}

/// fxdax: the private memory of a window that copies on write (backlog doc-13; see the module
/// docs): a memory object of the window's size, the window's memory where nothing is installed, at
/// the window's offsets, which holds the private pages alone.
pub(crate) struct PrivateMemory {
    fd: OwnedFd,
    /// Where it is mapped besides, all of it, registered for missing faults: what its pages are
    /// filled through (`UFFDIO_COPY`, which only fills memory registered so), and nothing
    /// accesses.
    view: usize,
}

/// fxdax: what a second stage is doing to its memory, holding its pages' lock, and since when, in
/// nanoseconds from its start: what a vCPU whose access keeps failing tells of
/// ([`describe_windows`]).
#[derive(Default)]
struct Busy {
    doing: AtomicU8,
    since: AtomicU64,
    /// When its fault thread began resolving the faults and lifting the restrictions it is on
    /// now, one past, or none while it waits for more: the lifts a vCPU's failing access asks for
    /// wait for it (`guest_access_failed`).
    serving: AtomicU64,
}

/// fxdax: what [`Busy`] says a second stage is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Doing {
    Nothing = 0,
    /// A change the window made.
    Change = 1,
    /// Leaving out the runs installed or faulted on longest ago.
    Trim = 2,
    /// Installing a page left out again, for a fault.
    Reinstall = 3,
}

impl Doing {
    fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Change,
            2 => Self::Trim,
            3 => Self::Reinstall,
            _ => Self::Nothing,
        }
    }
}

impl Busy {
    /// Says the stage does `doing` from `since`, until what it returns is dropped: under its
    /// pages' lock, so that nothing else says otherwise meanwhile.
    fn doing(&self, doing: Doing, since: u64) -> Busied<'_> {
        self.since.store(since, Ordering::Relaxed);
        self.doing.store(doing as u8, Ordering::Relaxed);
        Busied(self)
    }

    /// What the stage is doing, and since when.
    fn now(&self) -> (Doing, u64) {
        let doing = Doing::from_code(self.doing.load(Ordering::Relaxed));
        (doing, self.since.load(Ordering::Relaxed))
    }
}

/// fxdax: a stage's [`Busy`] doing something, until dropped.
struct Busied<'a>(&'a Busy);

impl Drop for Busied<'_> {
    fn drop(&mut self) {
        self.0.doing.store(Doing::Nothing as u8, Ordering::Relaxed);
    }
}

/// fxdax: the threads whose faults are the guest's own accesses, its vCPUs' ([`guest_thread_started`]),
/// by their IDs, as userfaultfd reports them.
static GUEST_THREADS: std::sync::RwLock<Vec<u32>> = std::sync::RwLock::new(Vec::new());

/// fxdax: the calling thread's accesses to guest memory are the guest's own, as a vCPU's are: a
/// fault of it for writing, on a window that copies on write, is a store (backlog doc-13). A VMM
/// calls it on each vCPU's thread as it starts. Any other thread's, as x86 KVM's asynchronous page
/// fault's worker's, may be for a load.
pub fn guest_thread_started() {
    // SAFETY: Plain system call.
    let thread = unsafe { libc::syscall(libc::SYS_gettid) } as u32;
    guest_thread(thread);
}

/// fxdax: thread `thread`'s accesses are the guest's own ([`guest_thread_started`]).
pub(crate) fn guest_thread(thread: u32) {
    GUEST_THREADS.write().unwrap_or_else(std::sync::PoisonError::into_inner).push(thread);
}

/// fxdax: whether thread `thread`'s accesses are the guest's own.
fn is_guests(thread: u32) -> bool {
    GUEST_THREADS.read().unwrap_or_else(std::sync::PoisonError::into_inner).contains(&thread)
}

/// fxdax: the lifts of the stages serving faults ([`guest_access_failed`]), each its fault
/// thread's while it runs.
static LIFTS: std::sync::Mutex<Vec<Arc<Lift>>> = std::sync::Mutex::new(Vec::new());

/// fxdax: how long a guest's access failing waits for the fault threads' lifts at most
/// ([`guest_access_failed`]): less than a run of failures' gap (`AccessFailures::IN_A_ROW`), so
/// that a fault thread slow to lift leaves the VMM retrying the access, one run of failures.
const LIFT_PATIENCE: std::time::Duration = std::time::Duration::from_millis(5);

/// fxdax: a fault thread's lifts of what its stage restricted for workers' writes, which a guest's
/// access failing asks for, and waits for ([`guest_access_failed`]).
struct Lift {
    /// What the fault thread polls, signalled at each ask.
    fd: File,
    /// The asks made so far.
    asked: AtomicU64,
    /// The asks the fault thread has lifted for, all those made before its last lift began.
    lifted: std::sync::Mutex<u64>,
    done: std::sync::Condvar,
}

impl Lift {
    fn new() -> Result<Self, Error> {
        let (asked, lifted) = (AtomicU64::new(0), std::sync::Mutex::new(0));
        Ok(Self { fd: event_fd()?, asked, lifted, done: std::sync::Condvar::new() })
    }

    /// Asks the fault thread to lift: the ask's number, which [`Self::wait`] waits for.
    fn ask(&self) -> u64 {
        let ask = self.asked.fetch_add(1, Ordering::AcqRel) + 1;
        // A write fails with `EAGAIN` only if the counter would overflow, when it is signalled.
        let _ = (&self.fd).write(&1u64.to_ne_bytes());
        ask
    }

    /// Waits until the fault thread has lifted for ask `ask`, or `deadline` has passed.
    fn wait(&self, ask: u64, deadline: std::time::Instant) {
        let mut lifted = self.lifted.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        while *lifted < ask {
            let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) else {
                return;
            };
            lifted = (self.done.wait_timeout(lifted, left))
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }

    /// On the fault thread, signalled: lifts with `lift`, for the asks made before it began.
    fn serve(&self, lift: impl FnOnce()) {
        let mut count = [0u8; 8];
        // A read fails with `EAGAIN` only if nothing signalled it since the last.
        let _ = (&self.fd).read(&mut count);
        let asked = self.asked.load(Ordering::Acquire);
        lift();
        let mut lifted = self.lifted.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        *lifted = (*lifted).max(asked);
        self.done.notify_all();
    }
}

/// fxdax: a guest's access failed, as `KVM_RUN` does with `EFAULT`, which the VMM retries: each
/// stage lifts the restrictions it made for workers' writes, on windows that copy on write, so that
/// the access, a store, retried, faults to the window (see the module docs). Called on a vCPU's
/// thread: it signals the fault threads, which lift them, and waits until they have, for
/// [`LIFT_PATIENCE`] at most. It waited for nothing once, and the vCPU, retrying at once, failed
/// again and again until the fault thread had lifted: 140 times a store after a worker's write,
/// 357 while a lift walked every page of the window (TASK-187,
/// `a_store_after_a_workers_write_waits_for_one_lift`).
pub fn guest_access_failed() {
    let lifts: Vec<Arc<Lift>> =
        LIFTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    let asks: Vec<u64> = lifts.iter().map(|lift| lift.ask()).collect();
    let deadline = std::time::Instant::now() + LIFT_PATIENCE;
    for (lift, ask) in lifts.iter().zip(asks) {
        lift.wait(ask, deadline);
    }
}

/// fxdax: the changes a second stage made that took access away from pages it had installed, by
/// how: memory installed over theirs, or theirs left out. Each flushes the host's TLBs of the
/// process's threads and the hypervisor's of the guest's vCPUs, interrupting them, which those
/// that took access from no installed page needn't (TASK-91.16).
#[derive(Default)]
struct Taken {
    overwritten: AtomicU64,
    left_out: AtomicU64,
}

/// fxdax: as [`Taken`] counts them: installed over, left out.
pub(crate) type TakenCounts = [u64; 2];

/// How many mappings a VM process keeps for itself beside its window's: Firecracker's, its
/// libraries', guest memory's, its threads' stacks'.
const MAPPINGS_RESERVED: usize = 4096;

/// The runs of pages a second stage keeps installed at most, in a process the host lets make
/// `max_map_count` mappings, if it limits them (`physmem::max_map_count`): half of those beyond
/// [`MAPPINGS_RESERVED`], as each run, a mapping of its own, splits the memory around it, a mapping
/// too. fxdax: told the limit, as a VM process in a jail without /proc can't read it, which once
/// had it budget by Linux's default, a seventeenth of the hosts' (TASK-172).
pub(crate) fn budget(max_map_count: Option<u64>) -> usize {
    let Some(max) = max_map_count else {
        return usize::MAX / 2;
    };
    let max = usize::try_from(max).unwrap_or(usize::MAX);
    (max.saturating_sub(MAPPINGS_RESERVED) / 2).max(1)
}

/// Logs a fault the window refused, `what` at window offset `offset`, of `thread`: each of the
/// first thousand, a run's few, and every power of two after: a vCPU whose access stays refused
/// for a second stops its VM with only `EFAULT` to say so, from `KVM_RUN` (TASK-99, TASK-101).
fn refused(what: &str, offset: u64, thread: u32) {
    static REFUSED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = REFUSED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if count <= 1000 || count.is_power_of_two() {
        // fxdax: whose, by the VMM's own registry of its vCPU threads, as a jailed VMM has no
        // /proc to name a thread by, and a thread gone is named nothing (TASK-167).
        let whose = if is_guests(thread) { "a vCPU" } else { "not a vCPU" };
        log::warn!(
            "fxdax: the DAX window refused {what}, at offset {offset:#x}, by thread {thread} ({}, \
             {whose}) ({count} so far)",
            thread_name(thread)
        );
    }
}

/// fxdax: the name of thread `thread`, as the kernel has it: whether a fault the window refused was
/// a vCPU's, the guest's own access, a device's, the VMM's on its behalf, or a kernel thread's, as
/// KVM's asynchronous page faults are, a vCPU's access faulted in for it as its guest goes on
/// (TASK-44.12): this process's threads, then any. fxdax: "gone" only if /proc is there to say
/// so: a jailed VMM has none, where every thread would read as gone (TASK-172).
fn thread_name(thread: u32) -> String {
    if !std::path::Path::new("/proc/self").exists() {
        return "unnamed: no /proc".to_owned();
    }
    [format!("/proc/self/task/{thread}/comm"), format!("/proc/{thread}/comm")]
        .iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .map_or_else(|| "gone".to_owned(), |name| name.trim_end().to_owned())
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
    /// its memory replaced does away with (TASK-102).
    restricted: bool,
    /// fxdax: whether it was restricted for a worker's write, on a window that copies on write,
    /// which a guest's access failing lifts ([`guest_access_failed`]): meaningful while
    /// `restricted`.
    for_worker: bool,
    /// fxdax: whether it is a private page (backlog doc-13): its private memory, filled, mapped
    /// here as the memory around it is ([`PrivateMemory`]). `mapped` is none then: it is no run.
    private: bool,
    /// fxdax: when the window last changed it, in nanoseconds from the stage's start: what a vCPU
    /// whose access keeps failing tells of ([`describe_windows`]).
    changed: u64,
    /// What the window maps at the page, if anything.
    mapped: Option<Mapped>,
}

/// A page the window maps here, read-only: its memory, whether it is installed in host memory or
/// left out, and when it was installed or faulted on last.
#[derive(Clone, Copy)]
struct Mapped {
    phys: crate::pages::PhysLocation,
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
    /// fxdax: whether the memory was let go of ([`UffdSecondStage::release`]): nothing changes
    /// it from now on.
    released: bool,
    /// fxdax: the pages restricted for workers' writes, some perhaps unrestricted since, which a
    /// guest's access failing lifts ([`UffdSecondStage::lift_worker_restrictions`]): each of a
    /// vCPU's `KVM_RUN`s failing lifts them, thousands a second while the guest strays, which a
    /// look at every page of the window took the fault thread from serving faults for (TASK-187).
    for_workers: Vec<usize>,
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

    /// fxdax: the private pages of `range`, which a change of it ends, private no more: the runs
    /// of them, to be emptied ([`UffdSecondStage::empty_private`]).
    fn take_private(&mut self, range: std::ops::Range<usize>) -> Vec<std::ops::Range<usize>> {
        let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
        for at in range {
            if !std::mem::take(&mut self.pages[at].private) {
                continue;
            }
            match runs.last_mut() {
                Some(run) if run.end == at => run.end = at + 1,
                _ => runs.push(at..at + 1),
            }
        }
        runs
    }

    /// fxdax: whether the pages of `range` are each installed, in one run of memory from `phys`
    /// on, none restricted by a fault: what a map of the very same pages there leaves in place
    /// (TASK-91.16).
    fn installed_at(&self, range: std::ops::Range<usize>, phys: crate::pages::PhysLocation) -> bool {
        self.pages[range].iter().enumerate().all(|(at, page)| {
            let offset = phys.offset + at as u64 * page_size();
            !page.restricted
                && page.mapped.is_some_and(|mapped| {
                    mapped.installed
                        && mapped.phys == (crate::pages::PhysLocation { offset, ..phys })
                })
        })
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

    /// Write-protects the `len` bytes at `addr` with userfaultfd.
    fn write_protect(&self, addr: usize, len: usize);

    /// Wakes the threads faulting on the `len` bytes at `addr`.
    fn wake(&self, addr: usize, len: usize);

    /// fxdax: a new memory object of `len` bytes, none of them filled: a window's private memory
    /// (`memfd_create`).
    fn private_memory(&self, len: usize) -> std::io::Result<OwnedFd>;

    /// fxdax: reserves `len` bytes of address space, inaccessible, where nothing else is.
    fn reserve(&self, len: usize) -> std::io::Result<usize>;

    /// fxdax: lets go of the `len` bytes of address space at `addr`, which [`reserve`] reserved.
    ///
    /// [`reserve`]: HostMemory::reserve
    fn unreserve(&self, addr: usize, len: usize);

    /// fxdax: fills the page at `addr`, of memory registered for missing faults where nothing is
    /// filled, with `bytes`, and wakes the threads faulting on it (`UFFDIO_COPY`).
    fn fill(&self, addr: usize, bytes: &[u8]);

    /// fxdax: empties the `len` bytes of memory object `fd` at `offset`: faults on them, where they
    /// are mapped, find them unfilled again (`fallocate`'s `FALLOC_FL_PUNCH_HOLE`).
    fn punch(&self, fd: RawFd, offset: u64, len: usize);
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

    fn write_protect(&self, addr: usize, len: usize) {
        let range = UffdioRange { start: addr as u64, len: len as u64 };
        let mut write_protect = UffdioWriteprotect { range, mode: UFFDIO_WRITEPROTECT_MODE_WP };
        ioctl(&self.uffd, UFFDIO_WRITEPROTECT, &mut write_protect)
            .unwrap_or_else(|err| panic!("write-protect {addr:#x} with userfaultfd: {err}"));
    }

    fn wake(&self, addr: usize, len: usize) {
        let mut range = UffdioRange { start: addr as u64, len: len as u64 };
        ioctl(&self.uffd, UFFDIO_WAKE, &mut range).expect("wake a faulting thread");
    }

    fn private_memory(&self, len: usize) -> std::io::Result<OwnedFd> {
        Self::private_memory_fd(len)
    }

    fn reserve(&self, len: usize) -> std::io::Result<usize> {
        let flags = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE;
        // SAFETY: A new mapping, wherever the kernel puts it.
        let addr = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_NONE, flags, -1, 0) };
        if addr == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        Ok(addr as usize)
    }

    fn unreserve(&self, addr: usize, len: usize) {
        // SAFETY: The address space `reserve` reserved, which nothing uses any more.
        let status = unsafe { libc::munmap(addr as *mut _, len) };
        assert_eq!(status, 0, "unreserve {addr:#x}: {}", std::io::Error::last_os_error());
    }

    fn fill(&self, addr: usize, bytes: &[u8]) {
        let len = bytes.len() as u64;
        let (dst, src) = (addr as u64, bytes.as_ptr() as u64);
        let mut copy = UffdioCopy { dst, src, len, mode: 0, copy: 0 };
        ioctl(&self.uffd, UFFDIO_COPY, &mut copy)
            .unwrap_or_else(|err| panic!("fill {addr:#x} ({} copied): {err}", copy.copy));
        assert_eq!(copy.copy, len as i64, "fill {addr:#x}: a short copy");
    }

    fn punch(&self, fd: RawFd, offset: u64, len: usize) {
        Self::punch_hole(fd, offset, len);
    }
}

impl Syscalls {
    /// fxdax: a new memory object of `len` bytes, none of them filled
    /// ([`HostMemory::private_memory`]).
    fn private_memory_fd(len: usize) -> std::io::Result<OwnedFd> {
        // SAFETY: Creates a memory object, which is owned below.
        let fd = unsafe { libc::memfd_create(c"fxdax-private-pages".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `fd` was just created, and is owned here.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: Plain system call on the descriptor just made: no page is filled.
        if unsafe { libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(fd)
    }

    /// fxdax: empties the `len` bytes of memory object `fd` at `offset` ([`HostMemory::punch`]).
    fn punch_hole(fd: RawFd, offset: u64, len: usize) {
        let mode = libc::FALLOC_FL_PUNCH_HOLE | libc::FALLOC_FL_KEEP_SIZE;
        // SAFETY: Plain system call on a memory object's descriptor.
        let status =
            unsafe { libc::fallocate(fd, mode, offset as libc::off_t, len as libc::off_t) };
        assert_eq!(status, 0, "punch {offset:#x}: {}", std::io::Error::last_os_error());
    }
}

/// The rules of the order in which the second stage changes its memory, each of which its Shuttle
/// tests show to be needed by turning it off (TASK-44.3).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rules {
    /// A page mapped here is read-only until its stores are tracked.
    pub map_with_window_protection: bool,
    /// A page mapped here is write-protected before it is made writable.
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
    /// fxdax: a worker's write of a page not here, on a window that copies on write, resolved as a
    /// load, makes the page read-only here, so that the worker's write, retried, fails rather than
    /// copying it for a load (backlog doc-13).
    pub worker_writes_restricted: bool,
    /// fxdax: a guest's access failing lifts what workers' writes restricted, so that a store,
    /// retried, faults to the window.
    pub failures_lift_worker_restrictions: bool,
    /// fxdax: a private page is filled before its memory is mapped in place of what was there, so
    /// that the memory is never accessible unfilled before it is registered (backlog doc-13).
    pub private_filled_before_mapped: bool,
    /// fxdax: a private page that goes is emptied, so that the memory around it, mapped there anew,
    /// faults again rather than holding the private page for good.
    pub private_emptied_as_it_goes: bool,
}

impl Rules {
    pub const ALL: Self = Self {
        map_with_window_protection: true,
        write_protect_before_writable: true,
        register_before_accessible: true,
        refused_store_read_only: true,
        restrict_between_changes: true,
        mapping_gives_back_withheld: true,
        worker_writes_restricted: true,
        failures_lift_worker_restrictions: true,
        private_filled_before_mapped: true,
        private_emptied_as_it_goes: true,
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

/// Physical memory, by arena (TASK-43), which a window's pages are mapped from.
pub(crate) enum Memories {
    /// This process's, every arena's, as tests have it.
    #[cfg(test)]
    InProcess,
    /// The arenas' a daemon gave this process: its tenant's, and the default, read-only, or the
    /// default alone.
    Given(Vec<(crate::pages::ArenaId, OwnedFd)>),
}

impl Memories {
    /// The descriptor of `arena`'s memory, which the window maps pages of, and whether it is
    /// read-only, as a tenant's VM process holds the default arena.
    fn fd(&self, arena: crate::pages::ArenaId) -> Option<(std::os::fd::RawFd, bool)> {
        let fd = match self {
            #[cfg(test)]
            Self::InProcess => crate::pages::arena_memory(arena)?.as_raw_fd(),
            Self::Given(memories) => {
                memories.iter().find(|(given, _)| *given == arena)?.1.as_raw_fd()
            }
        };
        // SAFETY: Plain system call on a descriptor the memories hold.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        Some((fd, flags >= 0 && flags & libc::O_ACCMODE == libc::O_RDONLY))
    }
}

/// How many mappings the host lets a test's process make, which a test, with /proc, reads.
#[cfg(test)]
pub(crate) fn host_max_map_count() -> Option<u64> {
    physmem::max_map_count().expect("the host's vm.max_map_count")
}

impl UffdSecondStage {
    /// Takes over the `size` bytes of host memory at `base`, replacing them with memory whose
    /// faults userfaultfd reports, as nothing is mapped yet: the window's pages are mapped there
    /// from `phys`, physical memory. `prefault`, if any, is told of the pages mapped
    /// there. It keeps as many runs of pages installed as a process the host lets make
    /// `max_map_count` mappings may ([`budget`]).
    ///
    /// # Safety
    ///
    /// The memory must be a page-aligned mapping that nothing else in the process uses.
    pub unsafe fn new(
        base: usize,
        size: u64,
        memories: Memories,
        prefault: Option<Arc<dyn Prefault>>,
        max_map_count: Option<u64>,
    ) -> Result<Arc<Self>, Error> {
        let budget = budget(max_map_count);
        log::info!(
            "fxdax: a DAX window of {size} bytes keeps at most {budget} runs of pages installed, \
             the host letting a process make {} mappings",
            max_map_count.map_or_else(|| "any number of".to_owned(), |max| max.to_string())
        );
        // SAFETY: As the caller promises.
        unsafe { Self::with_budget(base, size, memories, prefault, budget) }
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
            UffdioApi {
                api: UFFD_API,
                features: UFFD_FEATURE_WP_HUGETLBFS_SHMEM | UFFD_FEATURE_THREAD_ID,
                ioctls: 0,
            };
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
        unsafe { Self::new(base, size, Memories::InProcess, prefault, host_max_map_count()) }
    }

    /// The runs of pages it keeps installed at most: fxdax: which its VM process tells the
    /// daemon of (TASK-111.5).
    pub(crate) fn budget(&self) -> usize {
        self.budget
    }

    /// fxdax: the runs of pages it has installed now, each a mapping of the process's, which
    /// [`budget`](Self::budget) bounds (TASK-111.5).
    pub(crate) fn installed(&self) -> u64 {
        self.pages.lock().unwrap().runs as u64
    }

    /// Serves the faults on the memory with `window`, on a thread of its own, until the returned
    /// thread is dropped.
    pub fn serve(self: &Arc<Self>, window: Arc<dyn Faults>) -> Result<FaultThread, Error> {
        assert_eq!(window.size(), self.size as u64, "the window is not the second stage's size");
        let stop = Arc::new(event_fd()?);
        let wake = Arc::new(EventFdWaker(event_fd()?));
        // fxdax: a guest's access failing, said to every stage (`guest_access_failed`).
        let lift = Arc::new(Lift::new()?);
        LIFTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(lift.clone());
        let thread_stop = stop.clone();
        let (stage, thread_lift) = (self.clone(), lift.clone());
        let thread = crate::thread::spawn("dax faults", move || {
            stage.serve_faults(&*window, &thread_stop, wake, &thread_lift)
        });
        let thread = match thread {
            Ok(thread) => thread,
            Err(err) => {
                forget_lift(&lift);
                return Err(Error::new(err));
            }
        };
        // fxdax: told of by a vCPU whose access keeps failing (`describe_windows`).
        let mut serving = SERVING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        serving.retain(|stage| stage.strong_count() > 0);
        serving.push(Arc::downgrade(self));
        Ok(FaultThread { stop, lift, thread: Some(thread) })
    }

    /// Passes the memory's faults to `window` until `stop` is signalled. Faults are resolved
    /// concurrently: each is a future, which this thread polls once `wake`, its waker, signals that
    /// it can make progress, as when Fxfs's pager supplies the page it waits for. fxdax: and lifts
    /// what workers' writes restricted as `lift` is signalled ([`guest_access_failed`]).
    fn serve_faults(&self, window: &dyn Faults, stop: &File, wake: Arc<EventFdWaker>, lift: &Lift) {
        let waker = Waker::from(wake.clone());
        let mut cx = Context::from_waker(&waker);
        let mut faults = FuturesUnordered::new();
        let mut fds = [
            libc::pollfd { fd: self.host.uffd.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: stop.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: wake.0.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: lift.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        loop {
            // Make what progress the faults can, and then wait for new faults or for a fault's
            // waker.
            while let Poll::Ready(Some(())) = faults.poll_next_unpin(&mut cx) {}
            self.busy.serving.store(0, Ordering::Relaxed);
            // SAFETY: `fds` is an array of `pollfd`s of its length.
            let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
            self.busy.serving.store(self.now() + 1, Ordering::Relaxed);
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
            if fds[3].revents != 0 {
                lift.serve(|| self.lift_worker_restrictions());
            }
            while let Some(msg) = self.read_event() {
                if msg.event == UFFD_EVENT_PAGEFAULT {
                    faults.push(self.resolve(window, msg.address as usize, msg.flags, msg.ptid));
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
        let for_workers = Vec::new();
        let pages = Mutex::new(Pages { pages, runs: 0, clock: 0, released: false, for_workers });
        let budget = budget.max(1);
        let taken = Taken::default();
        let stage = Arc::new(Self {
            base,
            size,
            host,
            memories,
            prefault,
            pages,
            budget,
            taken,
            private: OnceLock::new(),
            start: std::time::Instant::now(),
            busy: Busy::default(),
        });
        stage.unmap(0, size as u64);
        Ok(stage)
    }

    /// fxdax: its window copies on write (backlog doc-13), from now on: told before the window is
    /// made, as it faults nothing before then. Its memory, all of it, is its private memory from
    /// now on, none of it filled, which faults as the anonymous memory it replaces did (see the
    /// module docs).
    pub(crate) fn copy_on_write(&self) -> Result<(), Error> {
        let fd = self.host.private_memory(self.size).map_err(Error::new)?;
        let view = self.host.reserve(self.size).map_err(Error::new)?;
        let memory = Memory::File { fd: fd.as_raw_fd(), offset: 0, shared: true };
        self.host.map(view, self.size, libc::PROT_READ | libc::PROT_WRITE, memory);
        self.host.register(view, self.size, UFFDIO_REGISTER_MODE_MISSING);
        if let Err(private) = self.private.set(PrivateMemory { fd, view }) {
            self.host.unreserve(private.view, self.size);
            return Err(Error::msg("the window copies on write already"));
        }
        self.unmap(0, self.size as u64);
        Ok(())
    }

    /// fxdax: whether its window copies on write (backlog doc-13).
    pub(crate) fn copying(&self) -> bool {
        self.private.get().is_some()
    }

    /// fxdax: the time from its start, in nanoseconds.
    fn now(&self) -> u64 {
        self.start.elapsed().as_nanos() as u64
    }

    /// fxdax: the memory where the window installs nothing, at `addr`: its private memory, if it
    /// copies on write (see the module docs), or anonymous memory.
    fn base_memory(&self, addr: usize) -> Memory {
        match self.private.get() {
            Some(private) => {
                let offset = (addr - self.base) as u64;
                Memory::File { fd: private.fd.as_raw_fd(), offset, shared: true }
            }
            None => Memory::Anonymous,
        }
    }

    /// fxdax: empties the private pages of `runs`, ranges of pages that went: the memory around
    /// them, mapped there anew, faults again, as their private memory is unfilled.
    fn empty_private(&self, runs: &[std::ops::Range<usize>]) {
        let Some(private) = self.private.get() else { return };
        if !rules().private_emptied_as_it_goes {
            return;
        }
        let page_size = page_size() as usize;
        for run in runs {
            let offset = (run.start * page_size) as u64;
            self.host.punch(private.fd.as_raw_fd(), offset, run.len() * page_size);
        }
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
    /// fxdax: returns whether it did: not once the memory was let go of ([`Self::release`]).
    fn change<T>(
        &self,
        addr: usize,
        len: usize,
        record: impl FnOnce(&mut Pages) -> T,
        change: impl FnOnce(T),
    ) -> bool {
        let mut pages = self.pages.lock().unwrap();
        if pages.released {
            return false;
        }
        let now = self.now();
        let recorded = record(&mut pages);
        for page in &mut pages.pages[self.page_range(addr, len)] {
            page.changed = now;
        }
        if !rules().restrict_between_changes {
            drop(pages);
            change(recorded);
            return true;
        }
        for page in &mut pages.pages[self.page_range(addr, len)] {
            page.made = page.made.wrapping_add(1);
            page.withheld = false;
        }
        let _busy = self.busy.doing(Doing::Change, now);
        change(recorded);
        true
    }

    /// fxdax: lets go of the memory, as the connection to the daemon whose pages it maps goes,
    /// after which the daemon frees them, for other VMs' extents (TASK-79.8): all of it replaced
    /// with memory nothing accesses, whose faults userfaultfd reports not, the guest's or this
    /// process's; and no change made to it from now on, as the thread receiving from the daemon
    /// may still make those it sent before it went.
    pub fn release(&self) {
        let mut pages = self.pages.lock().unwrap();
        pages.released = true;
        let all = 0..pages.pages.len();
        pages.update(all, |pages| {
            pages.iter_mut().for_each(|page| {
                (page.mapped, page.restricted, page.private) = (None, false, false)
            })
        });
        self.host.map(self.base, self.size, libc::PROT_NONE, Memory::Anonymous);
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
        if pages.released || seen.is_some_and(|seen| made != seen) {
            return;
        }
        self.host.protect(addr, page_size, prot);
        pages.pages[page].withheld = withheld;
        pages.pages[page].restricted = prot == libc::PROT_READ;
        pages.pages[page].for_worker = false;
        let what = if withheld { "withheld" } else { "restricted to reads" };
        changed_protection(what, offset, made, seen);
    }

    /// fxdax: restricts the page at `addr` to reads for a worker's write, resolved as a load, on a
    /// window that copies on write (see the module docs), if the window maps it here, read-only,
    /// so that the worker's write fails; until a guest's access fails ([`guest_access_failed`]). A
    /// private page takes the write.
    fn restrict_for_worker(&self, addr: usize) {
        if !rules().worker_writes_restricted {
            return;
        }
        let page_size = page_size() as usize;
        let at = self.page_range(addr, 1).start;
        let mut pages = self.pages.lock().unwrap();
        let installed = pages.pages[at].mapped.is_some_and(|mapped| mapped.installed);
        if pages.released || !installed {
            return;
        }
        self.host.protect(addr, page_size, libc::PROT_READ);
        let Pages { pages: all, for_workers, .. } = &mut *pages;
        // Pages unrestricted since stay listed till a lift, of which a VM may take none: dropped
        // once the list is as long as the window, each page listed once while it is restricted.
        if for_workers.len() >= all.len() {
            for_workers.retain(|&at| all[at].restricted && all[at].for_worker);
        }
        if !(all[at].restricted && all[at].for_worker) {
            for_workers.push(at);
        }
        let page = &mut pages.pages[at];
        (page.restricted, page.for_worker) = (true, true);
        let offset = (addr - self.base) as u64;
        changed_protection("restricted to reads for a worker's write", offset, page.made, None);
    }

    /// fxdax: lifts the restrictions made for workers' writes ([`Self::restrict_for_worker`]), as
    /// a guest's access failed ([`guest_access_failed`]): the page is writable here again, and
    /// write-protected still, so that a store to it faults to the window.
    pub(crate) fn lift_worker_restrictions(&self) {
        if !rules().failures_lift_worker_restrictions {
            return;
        }
        let page_size = page_size() as usize;
        let mut pages = self.pages.lock().unwrap();
        if pages.released {
            return;
        }
        for at in std::mem::take(&mut pages.for_workers) {
            let page = &mut pages.pages[at];
            if !(page.restricted && page.for_worker) {
                continue;
            }
            (page.restricted, page.for_worker) = (false, false);
            let writable = libc::PROT_READ | libc::PROT_WRITE;
            self.host.protect(self.base + at * page_size, page_size, writable);
            let offset = (at * page_size) as u64;
            changed_protection("lifted, a guest's access having failed", offset, page.made, None);
        }
    }

    /// fxdax: the changes made so far that took access away from pages installed, by how
    /// ([`Taken`]).
    pub(crate) fn taken(&self) -> TakenCounts {
        let taken = &self.taken;
        [&taken.overwritten, &taken.left_out].map(|count| count.load(Ordering::Relaxed))
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
    pub(crate) async fn resolve(
        &self,
        window: &dyn Faults,
        address: usize,
        flags: u64,
        thread: u32,
    ) {
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
        // fxdax: a worker's write of a page not here, on a window that copies on write, which a
        // load may have made, is resolved as a load, and the page restricted to reads here once
        // it is (see the module docs).
        let for_worker =
            !store && access == Access::Write && self.copying() && !is_guests(thread);
        // A page the window maps, left out past the budget, is installed again, as it was, without
        // the window; any other fault is the window's to resolve.
        let reinstalled = !store && self.reinstall(addr);
        if !reinstalled {
            self.resolve_through(window, addr, access, store, for_worker, thread).await;
        }
        // Before the worker retries its write, however its page came to be here. fxdax: one
        // installed again, as it was, write-protected, and left writable, took the retried write
        // as a store, which copied the page for a load, uncounted by the guest, past its private
        // pages: a jailed VM process, its budget the default's for want of /proc, left out pages by
        // the thousand, 21,396 copied so, and the guest's writes were refused (GCE r582, TASK-170).
        if for_worker {
            self.restrict_for_worker(addr);
        }
        self.host.wake(addr, page_size);
        // A write to a page not mapped here may start a run a sequential writer stores to: the pages
        // after it are mapped read-only, the next read in if need be, so that stores to them fault
        // as stores, which dirty runs of pages, and loads do not fault at all. Past the file, or
        // the window, there is nothing to map. fxdax: a probe, whose page the guest may not map,
        // which the window refuses then without telling of it as of the guest's access.
        let next = offset + page_size as u64;
        if !reinstalled && !store && access == Access::Write && next < window.size() {
            match window.probe(next).await {
                Ok(()) | Err(Errno::ENOENT | Errno::EINVAL) => {}
                Err(err) => log::warn!("mapping DAX window offset {next:#x} ahead: {err}"),
            }
        }
    }

    /// Resolves the fault at `addr`, an `access`, a `store` to a page here if so, of `thread`'s,
    /// through `window`, as [`resolve`](Self::resolve) does all but a page left out: a worker's
    /// write as a load, `for_worker`. A fault the window refuses restricts the page as the access
    /// found it; one it can't resolve aborts the process, as its faulting thread can't go on.
    async fn resolve_through(
        &self,
        window: &dyn Faults,
        addr: usize,
        access: Access,
        store: bool,
        for_worker: bool,
        thread: u32,
    ) {
        let offset = (addr - self.base) as u64;
        // The page as the access found it, which a fault the window does not resolve restricts.
        let seen = self.seen(addr);
        let resolved = if store {
            window.fault(offset, access).await
        } else if for_worker {
            window.fault_page(offset, Access::Read).await
        } else {
            window.fault_page(offset, access).await
        };
        let resolved = match resolved {
            // Where the window refuses a write, the page is mapped read-only: make it read-only
            // here too, so that the write fails in the kernel rather than faulting here again. A
            // store from the guest then fails as KVM's would.
            Err(Errno::EACCES) if access == Access::Write => {
                refused("a store to a page the window maps read-only", offset, thread);
                if rules().refused_store_read_only {
                    self.restrict(addr, seen, libc::PROT_READ);
                }
                Ok(())
            }
            // Where the window maps nothing, the fault is KVM's, for a page the guest has
            // unmapped (see the module docs): it is withheld, so that the fault fails.
            Err(Errno::ENOENT) => {
                refused("an access to a page the window maps nothing at", offset, thread);
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
    }

    /// fxdax: maps the pages installed of the `len` bytes at `offset` for the guest ahead of its
    /// accesses (`Prefault`), as the guest asks for the pages it maps to read (doc-4's
    /// `MAP_AHEAD`): each one's first access otherwise takes the hypervisor a fault, which nested
    /// virtualization makes dear (TASK-112.1). Pages not installed are left to the guest's
    /// faults, which this mustn't wait for: what waits for the daemon's replies may be holding
    /// them up. Nothing outside the second stage, or without a `Prefault`, is mapped.
    pub(crate) fn prefault_installed(&self, offset: u64, len: u64) {
        let Some(prefault) = &self.prefault else { return };
        let page_size = page_size();
        let end = offset.saturating_add(len).min(self.size as u64);
        if offset >= end {
            return;
        }
        let (first, last) = (offset / page_size, end.div_ceil(page_size));
        let runs: Vec<std::ops::Range<u64>> = {
            let pages = self.pages.lock().unwrap();
            if pages.released {
                return;
            }
            // fxdax: private pages too, their memory filled here (backlog doc-13).
            let installed = |at: u64| {
                let page = &pages.pages[at as usize];
                !page.withheld
                    && (page.private || page.mapped.is_some_and(|mapped| mapped.installed))
            };
            let mut runs: Vec<std::ops::Range<u64>> = Vec::new();
            for at in (first..last).filter(|&at| installed(at)) {
                match runs.last_mut() {
                    Some(run) if run.end == at => run.end = at + 1,
                    _ => runs.push(at..at + 1),
                }
            }
            runs
        };
        for run in runs {
            prefault.prefault(run.start * page_size, (run.end - run.start) * page_size);
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
    fn map(&self, offset: u64, phys: crate::pages::PhysLocation, len: u64) {
        let (addr, len) = self.addr(offset, len);
        let memory = self.memory(phys);
        let range = self.page_range(addr, len);
        let page_size = page_size();
        self.change(
            addr,
            len,
            |pages| {
                let used = pages.tick();
                Self::took(&self.taken.overwritten, pages.any_installed(range.clone()));
                // fxdax: a private page mapped over goes (backlog doc-13).
                let private = pages.take_private(range.clone());
                pages.update(range, |pages| {
                    for (at, page) in pages.iter_mut().enumerate() {
                        let offset = phys.offset + at as u64 * page_size;
                        let phys = crate::pages::PhysLocation { arena: phys.arena, offset };
                        page.mapped = Some(Mapped { phys, installed: true, used });
                        page.restricted = false;
                    }
                });
                private
            },
            |private| {
                self.empty_private(&private);
                self.install(addr, len, memory)
            },
        );
        self.trim();
    }

    fn unmap(&self, offset: u64, len: u64) {
        let (addr, len) = self.addr(offset, len);
        let range = self.page_range(addr, len);
        let forget = |pages: &mut Pages| {
            Self::took(&self.taken.left_out, pages.any_installed(range.clone()));
            // fxdax: the private pages there go (backlog doc-13).
            let private = pages.take_private(range.clone());
            pages.update(range, |pages| {
                pages.iter_mut().for_each(|page| (page.mapped, page.restricted) = (None, false))
            });
            private
        };
        // fxdax: emptied first, so that an access finds the page gone as soon as it can't reach
        // it: it faults, to the window, which resolves it as its change left it.
        self.change(addr, len, forget, |private| {
            self.empty_private(&private);
            self.leave_out(addr, len)
        });
    }

    /// fxdax: the window's change of a range it mapped, made at once (TASK-91.16): each map of the
    /// very pages installed where it maps them, as a `MAP` of what a range maps already, leaves
    /// them in place, the memory staying; another's memory is installed over what was there, in
    /// one `mmap`; and only what no map covers is left out. Made in steps, each page would be left
    /// out, then installed again: an unmap of the old memory, flushing the host's TLBs and the
    /// hypervisor's mappings, then a map, its registration and its protection, then the guest's
    /// accesses faulting the pages in again.
    fn remap(&self, offset: u64, len: u64, anew: &[Anew]) {
        let (addr, len) = self.addr(offset, len);
        let range = self.page_range(addr, len);
        let page_size = page_size() as usize;
        let maps: Vec<(usize, usize, crate::pages::PhysLocation)> = (anew.iter())
            .filter_map(|anew| match *anew {
                Anew::Map { offset, phys, len } => {
                    let (addr, len) = self.addr(offset, len);
                    Some((addr, len, phys))
                }
                Anew::Mapped { .. } => None,
            })
            .collect();
        let record = |pages: &mut Pages| {
            let used = pages.tick();
            let in_place: Vec<bool> = (maps.iter())
                .map(|&(addr, len, phys)| pages.installed_at(self.page_range(addr, len), phys))
                .collect();
            for (&(addr, len, _), in_place) in maps.iter().zip(&in_place) {
                let over = !in_place && pages.any_installed(self.page_range(addr, len));
                Self::took(&self.taken.overwritten, over);
            }
            let installed_before: Vec<bool> = (range.clone())
                .map(|at| pages.pages[at].mapped.is_some_and(|mapped| mapped.installed))
                .collect();
            // fxdax: the private pages there go, mapped over or left out (backlog doc-13).
            let private = pages.take_private(range.clone());
            pages.update(range.clone(), |pages| {
                pages.iter_mut().for_each(|page| (page.mapped, page.restricted) = (None, false))
            });
            let mut covered = vec![false; range.len()];
            for &(addr, len, phys) in &maps {
                let mapping = self.page_range(addr, len);
                covered[mapping.start - range.start..mapping.end - range.start].fill(true);
                pages.update(mapping, |pages| {
                    for (at, page) in pages.iter_mut().enumerate() {
                        let offset = phys.offset + at as u64 * page_size as u64;
                        let phys = crate::pages::PhysLocation { offset, ..phys };
                        page.mapped = Some(Mapped { phys, installed: true, used });
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
            (in_place, left_out, private)
        };
        self.change(addr, len, record, |(in_place, left_out, private)| {
            self.empty_private(&private);
            // The maps made in place need nothing: their memory stays, as it is.
            for (&(addr, len, phys), in_place) in maps.iter().zip(in_place) {
                if !in_place {
                    self.install(addr, len, self.memory(phys));
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
    }

    fn window_mapped(&self, offset: u64, len: u64) {
        let (addr, len) = self.addr(offset, len);
        let page_size = page_size() as usize;
        let range = self.page_range(addr, len);
        let first = range.start;
        let mut pages = self.pages.lock().unwrap();
        if pages.released {
            return;
        }
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

    /// fxdax: the page at `offset` is private from now on (backlog doc-13): its private memory
    /// filled with the bytes of the page at `from`, its source's, or zeros, then mapped in place of
    /// what was there, as the memory around it is, with which it merges (see the module docs):
    /// accessible as soon as it is mapped, as nothing about it faults any more.
    fn make_private(&self, offset: u64, from: Option<crate::pages::PhysLocation>) {
        let (addr, len) = self.addr(offset, page_size());
        let private = self.private.get().expect("a private page of a window that doesn't copy");
        let mut bytes = vec![0u8; len];
        if let Some(from) = from {
            // The source's page stays as it is until this is made, as it takes access away from
            // the page installed here: the daemon frees the page only once it is acknowledged.
            let (fd, _) = self.memories.fd(from.arena).expect("a window's page is of its arenas");
            read_at(fd, &mut bytes, from.offset);
        }
        let at = self.page_range(addr, len).start;
        let record = |pages: &mut Pages| {
            Self::took(&self.taken.overwritten, pages.any_installed(at..at + 1));
            let again = pages.pages[at].private;
            pages.update(at..at + 1, |pages| {
                let page = &mut pages[0];
                (page.mapped, page.restricted, page.private) = (None, false, true);
            });
            again
        };
        let made = self.change(addr, len, record, |again| {
            // The window makes a page private once, until a change of it ends it.
            assert!(!again, "DAX window offset {offset:#x} made private again");
            let (memory, writable) = (self.base_memory(addr), libc::PROT_READ | libc::PROT_WRITE);
            let view = private.view + offset as usize;
            if rules().private_filled_before_mapped {
                self.host.fill(view, &bytes);
            }
            self.host.map(addr, len, writable, memory);
            self.host.register(addr, len, UFFDIO_REGISTER_MODE_MISSING);
            if !rules().private_filled_before_mapped {
                self.host.fill(view, &bytes);
            }
            // What faulted on it meanwhile finds it now.
            self.host.wake(addr, len);
        });
        if made && let Some(prefault) = &self.prefault {
            prefault.prefault(offset, len as u64);
        }
    }
}

/// fxdax: reads `bytes.len()` bytes of `fd`'s memory at `offset` into `bytes`.
fn read_at(fd: RawFd, bytes: &mut [u8], offset: u64) {
    let mut done = 0;
    while done < bytes.len() {
        let rest = &mut bytes[done..];
        let at = (offset + done as u64) as libc::off_t;
        // SAFETY: Reads into `rest`, which is as long as asked.
        let read = unsafe { libc::pread(fd, rest.as_mut_ptr().cast(), rest.len(), at) };
        if read < 0 {
            let err = std::io::Error::last_os_error();
            assert_eq!(err.kind(), std::io::ErrorKind::Interrupted, "read {offset:#x}: {err}");
            continue;
        }
        assert!(read > 0, "read {offset:#x}: past the memory's end");
        done += read as usize;
    }
}

impl<H: HostMemory> UffdSecondStage<H> {
    /// The memory of the page at `phys`, which the window maps read-only. Not mapped for the guest
    /// ahead of its accesses (`Prefault`) but those it asks for ([`Self::prefault_installed`]):
    /// mapped for it, the hypervisor's pre-fault took a fifth of the VM process's time changing the
    /// window, and its pages, in the host's page tables, made each later unmap of them flush the
    /// host's and the hypervisor's TLBs on every CPU (TASK-109).
    fn memory(&self, phys: crate::pages::PhysLocation) -> Memory {
        // A page of an arena the window may not map is the daemon's error, which the guest must
        // not get past.
        let (fd, read_only) =
            self.memories.fd(phys.arena).expect("a window's page is of its arenas");
        // Memory this process holds read-only, the default arena's, public bases' pages, is mapped
        // private: userfaultfd tracks stores to a mapping that may be made writable alone, and a
        // store to a private one could only copy the page into this process, which the window
        // never lets it make, as it write-protects every page it maps. Its loads read the very
        // pages.
        Memory::File { fd, offset: phys.offset, shared: !read_only }
    }

    /// Installs `memory` at the `len` bytes at `addr`, which the window maps read-only.
    fn install(&self, addr: usize, len: usize, memory: Memory) {
        let writable = libc::PROT_READ | libc::PROT_WRITE;
        let prot = if rules().map_with_window_protection { libc::PROT_READ } else { writable };
        self.host.map(addr, len, prot, memory);
        // Stores are tracked by write-protecting the page, rather than by its protection, whose
        // faults userfaultfd does not report. The new mapping is read-only until it is
        // write-protected, so that a store meanwhile fails rather than going untracked.
        self.host.register(addr, len, UFFDIO_REGISTER_MODE_WP);
        // Mapped writable already, it needs no protection changed (TASK-26.18.3).
        let make_writable = prot != writable;
        if make_writable && !rules().write_protect_before_writable {
            self.host.protect(addr, len, writable);
        }
        self.host.write_protect(addr, len);
        if make_writable && rules().write_protect_before_writable {
            self.host.protect(addr, len, writable);
        }
    }

    /// Leaves the `len` bytes at `addr` out: memory registered for missing faults. fxdax: its
    /// private memory, of a window that copies on write, unfilled there.
    fn leave_out(&self, addr: usize, len: usize) {
        // The new memory is inaccessible until it is registered, so that an access meanwhile fails
        // rather than faulting in a page of zeros.
        let writable = libc::PROT_READ | libc::PROT_WRITE;
        let prot = if rules().register_before_accessible { libc::PROT_NONE } else { writable };
        self.host.map(addr, len, prot, self.base_memory(addr));
        self.host.register(addr, len, UFFDIO_REGISTER_MODE_MISSING);
        self.host.protect(addr, len, writable);
    }

    /// While more runs than the budget are installed, leaves out those installed or faulted on
    /// longest ago, down to seven eighths of it, so that it is not done at each mapping.
    fn trim(&self) {
        let mut pages = self.pages.lock().unwrap();
        if pages.released || pages.runs <= self.budget {
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
        let now = self.now();
        let _busy = self.busy.doing(Doing::Trim, now);
        for (_, run) in runs {
            if pages.runs <= low {
                break;
            }
            pages.update(run.clone(), |pages| {
                for page in pages {
                    page.made = page.made.wrapping_add(1);
                    page.changed = now;
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
    /// A page installed is counted as used. fxdax: a private page needs nothing (backlog doc-13):
    /// filled, its fault came before it was, and its access finds it now.
    fn reinstall(&self, addr: usize) -> bool {
        let page_size = page_size() as usize;
        let at = self.page_range(addr, 1).start;
        let mut pages = self.pages.lock().unwrap();
        if pages.pages[at].private {
            return true;
        }
        let used = pages.tick();
        let Some(mapped) = &mut pages.pages[at].mapped else { return false };
        if mapped.installed {
            mapped.used = used;
            return false;
        }
        let phys = mapped.phys;
        // The pages around it left out with it, of memory following on, as far as a chunk's.
        let left_out_with = |page: &PageChanges, distance: i64| {
            page.mapped.is_some_and(|other| {
                !other.installed
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
        let first = crate::pages::PhysLocation {
            arena: phys.arena,
            offset: phys.offset - ((at - start) * page_size) as u64,
        };
        let now = self.now();
        pages.update(start..end, |pages| {
            for page in pages {
                page.made = page.made.wrapping_add(1);
                page.changed = now;
                if let Some(mapped) = &mut page.mapped {
                    mapped.installed = true;
                    mapped.used = used;
                }
            }
        });
        let len = (end - start) * page_size;
        let busy = self.busy.doing(Doing::Reinstall, now);
        self.install(self.base + start * page_size, len, self.memory(first));
        drop(busy);
        drop(pages);
        self.trim();
        true
    }

    /// fxdax: what it holds at window offset `offset` and around it, and what it is doing: for a
    /// vCPU whose access keeps failing to tell of ([`describe_windows`]). Waits for its pages'
    /// lock no more than a moment, as what holds it may be what holds the vCPU up.
    fn describe_page(&self, offset: u64) -> String {
        let now = self.now();
        let ago = |at: u64| std::time::Duration::from_nanos(now.saturating_sub(at));
        let (doing, since) = self.busy.now();
        let doing = match doing {
            Doing::Nothing => "nothing".to_owned(),
            doing => format!("{doing:?} for {:?}", ago(since)),
        };
        let serving = match self.busy.serving.load(Ordering::Relaxed) {
            0 => "waiting for faults".to_owned(),
            since => format!("busy for {:?}", ago(since - 1)),
        };
        let mut said = format!(
            "the second stage at {:#x}, {} bytes, is doing {doing}; its fault thread is {serving}",
            self.base, self.size
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(100);
        let pages = loop {
            match self.pages.try_lock() {
                Ok(pages) => break Some(pages),
                Err(_) if std::time::Instant::now() >= deadline => break None,
                Err(_) => std::hint::spin_loop(),
            }
        };
        let Some(pages) = pages else {
            said.push_str("; its pages' lock stayed held for 100ms");
            return said;
        };
        let count = |what: fn(&PageChanges) -> bool| pages.pages.iter().filter(|p| what(p)).count();
        let installed = |page: &PageChanges| page.mapped.is_some_and(|mapped| mapped.installed);
        let left_out = |page: &PageChanges| page.mapped.is_some_and(|mapped| !mapped.installed);
        said.push_str(&format!(
            "; {} runs installed of a budget of {}, {} pages installed, {} left out, {} private, \
             {} withheld, {} restricted ({} for workers' writes)",
            pages.runs,
            self.budget,
            count(installed),
            count(left_out),
            count(|page| page.private),
            count(|page| page.withheld),
            count(|page| page.restricted),
            count(|page| page.restricted && page.for_worker),
        ));
        let at = (offset / page_size()) as usize;
        if let Some(page) = pages.pages.get(at) {
            let state = match page.mapped {
                _ if page.private => "private".to_owned(),
                Some(mapped) => format!(
                    "{} of {:?} at {:#x}",
                    if mapped.installed { "installed" } else { "left out" },
                    mapped.phys.arena,
                    mapped.phys.offset
                ),
                None => "nothing mapped".to_owned(),
            };
            said.push_str(&format!(
                "; offset {offset:#x}: {state}{}{}{}, changed {} times, last {:?} ago",
                if page.withheld { ", withheld" } else { "" },
                if page.restricted { ", restricted to reads" } else { "" },
                if page.restricted && page.for_worker { " for a worker's write" } else { "" },
                page.made,
                ago(page.changed),
            ));
        }
        said
    }
}

impl<H: HostMemory> Drop for UffdSecondStage<H> {
    fn drop(&mut self) {
        // fxdax: the private memory's other mapping, its own (backlog doc-13).
        if let Some(private) = self.private.get() {
            self.host.unreserve(private.view, self.size);
        }
    }
}

/// fxdax: the second stages serving faults ([`UffdSecondStage::serve`]), which a vCPU whose access
/// keeps failing tells of ([`describe_windows`]).
static SERVING: std::sync::Mutex<Vec<Weak<UffdSecondStage>>> = std::sync::Mutex::new(Vec::new());

/// fxdax: what the DAX windows' second stages serving faults hold and are doing, for a vCPU whose
/// access keeps failing, `KVM_RUN` with `EFAULT`, which says no more, to tell of: of each, what it
/// holds and does, and, of the one whose memory host address `addr` is in, if given, the page
/// there: its window offset, and whether it is installed, left out, withheld, restricted or
/// private, and when it last changed. Waits for no stage more than a moment.
pub fn describe_windows(addr: Option<usize>) -> String {
    let serving: Vec<Arc<UffdSecondStage>> = (SERVING.lock())
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter_map(Weak::upgrade)
        .collect();
    if serving.is_empty() {
        return "no DAX window's second stage serves faults".to_owned();
    }
    let said: Vec<String> = (serving.iter())
        .map(|stage| {
            let memory = stage.base..stage.base + stage.size;
            let offset = match addr.filter(|addr| memory.contains(addr)) {
                Some(addr) => (addr - stage.base) as u64,
                // None of its pages.
                None => u64::MAX,
            };
            stage.describe_page(offset - offset % page_size())
        })
        .collect();
    said.join("; and ")
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
    /// fxdax: what it polls for a guest's access failing ([`guest_access_failed`]).
    lift: Arc<Lift>,
    thread: Option<crate::thread::Thread<()>>,
}

impl Drop for FaultThread {
    fn drop(&mut self) {
        forget_lift(&self.lift);
        (&*self.stop).write_all(&1u64.to_ne_bytes()).expect("stop the fault thread");
        if let Err(panic) = self.thread.take().unwrap().join() {
            std::panic::resume_unwind(panic);
        }
    }
}

/// fxdax: a fault thread polls `lift` no more ([`LIFTS`]).
fn forget_lift(lift: &Arc<Lift>) {
    let mut lifts = LIFTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    lifts.retain(|registered| !Arc::ptr_eq(registered, lift));
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
    use super::super::tests::TestSource;
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
        let pager = TestSource::paged(2);
        // SAFETY: Only the test uses the memory, which outlives the window.
        let stage = unsafe { UffdSecondStage::in_process(base, 2 * ps as u64, None) }.unwrap();
        let (dax, _window) = Dax::in_host_memory(stage).unwrap();
        dax.window.map(0, &pager.source(), 0, 2 * ps as u64);

        // SAFETY: The memory is the window's, whose faults the fault thread serves.
        let load =
            move |offset: usize| move || unsafe { ((base + offset) as *const u8).read_volatile() };
        let first = std::thread::spawn(load(0));
        assert_eq!(pager.request(), 0);
        let second = std::thread::spawn(load(ps));
        assert_eq!(pager.request(), 1);

        pager.supply(1, 2);
        assert_eq!(second.join().unwrap(), 2);
        assert!(!first.is_finished());
        pager.supply(0, 1);
        assert_eq!(first.join().unwrap(), 1);

        drop(dax);
        // SAFETY: Unmaps the memory, which nothing uses any more.
        assert_eq!(unsafe { libc::munmap(memory, 2 * ps) }, 0);
    }

    /// fxdax TASK-79.8: a second stage let go of maps nothing from then on, and no change the
    /// window makes reaches its memory, which nothing can access: what the thread receiving from a
    /// daemon gone makes still.
    #[test]
    fn released_stages_change_nothing() {
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
        let file = TestSource::filled(2, 7).source();
        // SAFETY: Only the test uses the memory, which outlives the window.
        let stage = unsafe { UffdSecondStage::in_process(base, 2 * ps as u64, None) }.unwrap();
        let (dax, _window) = Dax::in_host_memory(stage.clone()).unwrap();
        dax.window.map(0, &file, 0, ps as u64);
        // SAFETY: The memory is the window's, whose faults the fault thread serves.
        assert_eq!(unsafe { (base as *const u8).read_volatile() }, 7);

        stage.release();
        dax.window.map(ps as u64, &file, ps as u64, ps as u64);
        stage.unmap(0, ps as u64);
        let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
        let window: Vec<&str> = (maps.lines())
            .filter(|line| {
                let (start, end) = line.split_whitespace().next().unwrap().split_once('-').unwrap();
                let start = usize::from_str_radix(start, 16).unwrap();
                let end = usize::from_str_radix(end, 16).unwrap();
                start < base + 2 * ps && end > base
            })
            .map(|line| line.split_whitespace().nth(1).unwrap())
            .collect();
        assert_eq!(window, ["---p"]);

        drop(dax);
        // SAFETY: Unmaps the memory, which nothing uses any more.
        assert_eq!(unsafe { libc::munmap(memory, 2 * ps) }, 0);
    }

    /// fxdax TASK-112.1: the pages the guest asks for ahead of its reads, of what the window maps,
    /// are those the hypervisor is told of, the runs of them installed; not those it maps nothing
    /// at, nor past the window.
    #[test]
    fn pages_asked_for_ahead_are_prefaulted_as_installed() {
        struct Told(Mutex<Vec<(u64, u64)>>);
        impl Prefault for Told {
            fn prefault(&self, offset: u64, len: u64) {
                self.0.lock().unwrap().push((offset, len));
            }
        }
        let ps = page_size();
        // SAFETY: Maps anonymous memory, which this test owns.
        let memory = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                4 * ps as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(memory, libc::MAP_FAILED);
        let file = TestSource::filled(4, 7).source();
        let told = Arc::new(Told(Mutex::new(Vec::new())));
        // SAFETY: Only the test uses the memory, which outlives the window.
        let stage =
            unsafe { UffdSecondStage::in_process(memory as usize, 4 * ps, Some(told.clone())) }
                .unwrap();
        let (dax, _window) = Dax::in_host_memory(stage.clone()).unwrap();
        dax.window.map(0, &file, 0, 2 * ps);
        dax.window.map(3 * ps, &file, 3 * ps, ps);
        assert_eq!(*told.0.lock().unwrap(), []);

        stage.prefault_installed(0, 4 * ps);
        assert_eq!(std::mem::take(&mut *told.0.lock().unwrap()), [(0, 2 * ps), (3 * ps, ps)]);
        stage.prefault_installed(2 * ps, 8 * ps);
        assert_eq!(std::mem::take(&mut *told.0.lock().unwrap()), [(3 * ps, ps)]);
        stage.prefault_installed(4 * ps, ps);
        assert_eq!(*told.0.lock().unwrap(), []);

        drop(dax);
        // SAFETY: Unmaps the memory, which nothing uses any more.
        assert_eq!(unsafe { libc::munmap(memory, 4 * ps as usize) }, 0);
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
        // The file's pages are scattered: a run each.
        let file = TestSource::scattered(PAGES as u64, |page| page as u8 + 1).source();
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
        let (dax, _window) = Dax::in_host_memory(stage.clone()).unwrap();
        dax.window.map(0, &file, 0, (PAGES * ps) as u64);

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
            let runs = stage.installed() as usize;
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
    /// each a flush of the host's TLBs and the hypervisor's: a map of other memory over them, and
    /// an unmap. A map where nothing is installed takes nothing away, nor does a map of the very
    /// same pages, left in place.
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
        let file = TestSource::filled(2, 1).source();
        // SAFETY: Only the test uses the memory, which outlives the window.
        let stage =
            unsafe { UffdSecondStage::in_process(base, (2 * ps) as u64, None) }.unwrap();
        let (dax, _window) = Dax::in_host_memory(stage.clone()).unwrap();
        dax.window.map(0, &file, 0, (2 * ps) as u64);
        assert_eq!(stage.taken(), [0, 0], "mapped where nothing was");
        let phys = |page: usize| stage.pages.lock().unwrap().pages[page].mapped.unwrap().phys;
        let first = phys(0);
        // The first page's own, in place; and the first page's again at the second's.
        stage.remap(
            0,
            (2 * ps) as u64,
            &[
                Anew::Map { offset: 0, phys: first, len: ps as u64 },
                Anew::Map { offset: ps as u64, phys: first, len: ps as u64 },
            ],
        );
        assert_eq!(stage.taken(), [1, 0], "the second page installed over, the first in place");
        stage.unmap(0, (2 * ps) as u64);
        assert_eq!(stage.taken(), [1, 1], "both left out");
        stage.unmap(0, (2 * ps) as u64);
        assert_eq!(stage.taken(), [1, 1], "nothing installed to leave out");

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
        // Room for the file and the pages between its pages.
        let memory = (4 * pages * ps).next_multiple_of(1 << 30) as u64;
        crate::pages::configure(crate::pages::Config::with_memory(memory)).unwrap();
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
        // The file's pages are scattered: a run each.
        let tag = |page: usize| (page % 251) as u8 + 1;
        let file = TestSource::scattered(pages as u64, |page| tag(page as usize)).source();
        // SAFETY: Only the test uses the memory, which outlives the window.
        let stage =
            unsafe { UffdSecondStage::in_process(base, (pages * ps) as u64, None) }.unwrap();
        let (dax, _window) = Dax::in_host_memory(stage.clone()).unwrap();
        dax.window.map(0, &file, 0, (pages * ps) as u64);
        // SAFETY: The memory is the window's, whose faults the fault thread serves.
        let load = |page: usize| unsafe { ((base + page * ps) as *const u8).read_volatile() };
        for round in 0..2 {
            for page in 0..pages {
                assert_eq!(load(page), tag(page), "round {round}, page {page}");
            }
            let runs = stage.installed() as usize;
            let budget = budget(host_max_map_count());
            assert!(runs <= budget, "round {round}: {runs} runs installed");
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
        let (dax, _window) = Dax::in_host_memory(stage).unwrap();
        let mut pipe = [0; 2];
        // SAFETY: Creates a pipe, which the test closes.
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        // SAFETY: Writes from the window's memory, which outlives the call.
        let write = || unsafe { libc::write(pipe[1], memory, ps) };

        assert_eq!(write(), -1);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EFAULT));

        // Not yet written, the source has no page in memory, and its first is zeros.
        let source = TestSource::anonymous(1).source();
        dax.window.map(0, &source, 0, ps as u64);
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

    /// fxdax TASK-164.1 (backlog doc-13): on a window that copies on write, a write the kernel
    /// makes to a page not here for a thread that isn't the guest's, as KVM's asynchronous page
    /// fault's worker makes for a vCPU's load and store alike, copies nothing: resolved as a load,
    /// the page restricted to reads here, the write fails (`EFAULT`). The guest's own thread's
    /// write fails too then, until a guest's access failing lifts the restriction
    /// (`guest_access_failed`): it faults as a store, and is made, in a private copy, the
    /// extent's page as it was. The kernel's writes are a pipe's reads into the window.
    #[test]
    fn worker_writes_copy_nothing_and_the_guests_stores_copy() {
        use crate::dax::Source as _;
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
        let copying = crate::dax::CopyOnWrite::reserve(crate::pages::ArenaId::DEFAULT, 1).unwrap();
        let (mut dax, window) =
            Dax::in_host_memory_copying(stage.clone(), Some(copying)).unwrap();
        // Not in memory as it is mapped, and zeros as it is brought in: not here until a fault.
        let file = TestSource::anonymous(1);
        dax.map(0, &file.source(), 0, ps as u64);
        let mut pipe = [0; 2];
        // SAFETY: Creates a pipe, which the test closes.
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        // A byte of the pipe's read into the window's page, as the kernel writes it.
        let read_into = move |base: usize| -> std::result::Result<(), i32> {
            // SAFETY: Writes a byte to the pipe, and reads one into the window's memory.
            unsafe {
                assert_eq!(libc::write(pipe[1], [7u8].as_ptr().cast(), 1), 1);
                match libc::read(pipe[0], base as *mut libc::c_void, 1) {
                    1 => Ok(()),
                    _ => Err(std::io::Error::last_os_error().raw_os_error().unwrap()),
                }
            }
        };

        let worker = std::thread::spawn(move || read_into(base)).join().unwrap();
        assert_eq!(worker, Err(libc::EFAULT));
        assert_eq!(window.private_pages(), 0, "a worker's write copied the page");
        guest_thread_started();
        assert_eq!(read_into(base), Err(libc::EFAULT));
        guest_access_failed();
        // Lifted by the fault thread, as it is signalled.
        while stage.pages.lock().unwrap().pages[0].restricted {
            std::thread::yield_now();
        }
        assert_eq!(read_into(base), Ok(()));
        assert_eq!(window.private_pages(), 1);
        // SAFETY: The window's page, private now, which nothing else accesses.
        assert_eq!(unsafe { (base as *const u8).read_volatile() }, 7);
        let mut shared = 1;
        // SAFETY: The extent's page is in memory while its lock is held.
        file.each_present(0..1, &mut |_, frame| shared = unsafe { *frame.as_ptr() });
        assert_eq!(shared, 0, "the write reached the extent's page");

        drop(dax);
        // SAFETY: Closes the pipe and unmaps the memory, which nothing uses any more.
        unsafe {
            libc::close(pipe[0]);
            libc::close(pipe[1]);
            assert_eq!(libc::munmap(memory, ps), 0);
        }
    }

    /// fxdax: on a window that copies on write, a write to a page installed here, write-protected,
    /// is a store, whichever thread makes it: the page is made private, the write made there, as
    /// KVM's worker writes only pages not here, a load of a page here taking no fault. A test of a
    /// worker's write must keep its page from here: `a_store_after_a_workers_write_waits_for_one_lift`
    /// mapped zeros, which a host's `vm.max_map_count` past OrbStack's kept installed (GCE r691).
    #[test]
    fn writes_to_pages_here_are_stores_whoever_makes_them() {
        use crate::dax::Source as _;
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
        let copying = crate::dax::CopyOnWrite::reserve(crate::pages::ArenaId::DEFAULT, 1).unwrap();
        let (mut dax, window) =
            Dax::in_host_memory_copying(stage.clone(), Some(copying)).unwrap();
        // In memory as it is mapped, so installed here at once.
        let file = TestSource::filled(1, 9);
        dax.map(0, &file.source(), 0, ps as u64);
        let installed = stage.pages.lock().unwrap().pages[0].mapped.map(|mapped| mapped.installed);
        assert_eq!(installed, Some(true));
        let mut pipe = [0; 2];
        // SAFETY: Creates a pipe, which the test closes.
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        let read_into = move |base: usize| -> std::result::Result<(), i32> {
            // SAFETY: Writes a byte to the pipe, and reads one into the window's memory.
            unsafe {
                assert_eq!(libc::write(pipe[1], [7u8].as_ptr().cast(), 1), 1);
                match libc::read(pipe[0], base as *mut libc::c_void, 1) {
                    1 => Ok(()),
                    _ => Err(std::io::Error::last_os_error().raw_os_error().unwrap()),
                }
            }
        };

        // No thread of this process is the guest's.
        let worker = std::thread::spawn(move || read_into(base)).join().unwrap();
        assert_eq!(worker, Ok(()));
        assert_eq!(window.private_pages(), 1);
        // SAFETY: The window's page, private now, which nothing else accesses.
        let page = unsafe { std::slice::from_raw_parts(base as *const u8, ps) };
        assert_eq!(page[0], 7);
        assert!(page[1..].iter().all(|&byte| byte == 9), "the private page holds other data");
        let mut shared = 0;
        // SAFETY: The extent's page is in memory while its lock is held.
        file.each_present(0..1, &mut |_, frame| shared = unsafe { *frame.as_ptr() });
        assert_eq!(shared, 9, "the write reached the extent's page");

        drop(dax);
        // SAFETY: Closes the pipe and unmaps the memory, which nothing uses any more.
        unsafe {
            libc::close(pipe[0]);
            libc::close(pipe[1]);
            assert_eq!(libc::munmap(memory, ps), 0);
        }
    }
    /// The lines of /proc/self/maps of the mappings in the `len` bytes at `base`.
    fn mappings_within(base: usize, len: usize) -> Vec<String> {
        let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
        (maps.lines())
            .filter(|line| {
                let range = line.split_whitespace().next().unwrap();
                let (start, end) = range.split_once('-').unwrap();
                let start = usize::from_str_radix(start, 16).unwrap();
                let end = usize::from_str_radix(end, 16).unwrap();
                start < base + len && end > base
            })
            .map(str::to_owned)
            .collect()
    }

    /// `pages` pages of anonymous memory, for a window's second stage to take over.
    fn window_memory(pages: usize) -> usize {
        let len = pages * page_size() as usize;
        let prot = libc::PROT_READ | libc::PROT_WRITE;
        let flags = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS;
        // SAFETY: Maps anonymous memory, which the test owns.
        let memory = unsafe { libc::mmap(std::ptr::null_mut(), len, prot, flags, -1, 0) };
        assert_ne!(memory, libc::MAP_FAILED);
        memory as usize
    }

    /// fxdax TASK-164.1 (backlog doc-13): a window's private pages cost its VM process no mapping
    /// each: made private between pages installed of an extent scattered over its arena, a run of
    /// its own each, and in an order no allocator's runs follow, the last page first, those
    /// adjacent in the window merge into one mapping, with the memory around them, the window's
    /// private memory, laid out as the window is. Each reads what it copied, and holds what is
    /// stored to it. Red with each private page a page of the tenant's arena mapped on its own, as
    /// r544's 110,857 were: 64 mappings.
    #[test]
    fn adjacent_private_pages_take_one_mapping() {
        const PAGES: usize = 64;
        let ps = page_size() as usize;
        let base = window_memory(PAGES);
        let size = (PAGES * ps) as u64;
        // SAFETY: Only the test uses the memory, which outlives the window.
        let stage = unsafe { UffdSecondStage::in_process(base, size, None) }.unwrap();
        let arena = crate::pages::ArenaId::DEFAULT;
        let copying = crate::dax::CopyOnWrite::reserve(arena, PAGES as u64).unwrap();
        let (mut dax, window) =
            Dax::in_host_memory_copying(stage.clone(), Some(copying)).unwrap();
        let tag = |page: usize| (page % 251) as u8 + 1;
        let file = TestSource::scattered(PAGES as u64, |page| tag(page as usize)).source();
        dax.map(0, &file, 0, size);
        for page in (0..PAGES).rev() {
            let (offset, len) = ((page * ps) as u64, ps as u64);
            futures::executor::block_on(window.make_private(offset, len)).unwrap();
        }
        assert_eq!(window.private_pages(), PAGES as u64);
        let mappings = mappings_within(base, PAGES * ps);
        assert_eq!(mappings.len(), 1, "the private pages' mappings: {mappings:#?}");
        assert!(mappings[0].contains("fxdax-private-pages"), "{mappings:#?}");
        for page in 0..PAGES {
            let at = (base + page * ps) as *mut u8;
            // SAFETY: The window's memory, private pages, whose faults the fault thread serves.
            let (copied, stored) = unsafe {
                at.add(1).write_volatile(0xee);
                (at.read_volatile(), at.add(1).read_volatile())
            };
            assert_eq!((copied, stored), (tag(page), 0xee), "page {page}");
        }

        drop(dax);
        // SAFETY: Unmaps the memory, which nothing uses any more.
        assert_eq!(unsafe { libc::munmap(base as *mut libc::c_void, PAGES * ps) }, 0);
    }

    /// fxdax TASK-164.1 (backlog doc-13): a window installing more runs than its budget trims
    /// them, but never its private pages: each stays in place, its private memory's, which the
    /// kernel reaches without a fault (a pipe's write from it), holding what was copied and stored
    /// there. Red with each private page a run, which a trim left out (r544: the private pages
    /// thrice the budget, the fault thread trimming, its lifts of the stores it refused waiting).
    #[test]
    fn trims_leave_private_pages_in_place() {
        const PAGES: usize = 32;
        const BUDGET: usize = 4;
        let ps = page_size() as usize;
        let base = window_memory(PAGES);
        let size = (PAGES * ps) as u64;
        // SAFETY: Only the test uses the memory, which outlives the window.
        let stage = unsafe {
            UffdSecondStage::with_budget(base, size, Memories::InProcess, None, BUDGET)
        }
        .unwrap();
        let arena = crate::pages::ArenaId::DEFAULT;
        let copying = crate::dax::CopyOnWrite::reserve(arena, PAGES as u64).unwrap();
        let (mut dax, window) =
            Dax::in_host_memory_copying(stage.clone(), Some(copying)).unwrap();
        let tag = |page: usize| (page % 251) as u8 + 1;
        let file = TestSource::scattered(PAGES as u64, |page| tag(page as usize)).source();
        dax.map(0, &file, 0, size);
        // Every other page private, stored to.
        for page in (0..PAGES).step_by(2) {
            let (offset, len) = ((page * ps) as u64, ps as u64);
            futures::executor::block_on(window.make_private(offset, len)).unwrap();
            // SAFETY: The window's private page, which nothing else accesses.
            unsafe { ((base + page * ps + 1) as *mut u8).write_volatile(0xee) };
        }
        // The others loaded, each a run installed, many times the budget, twice.
        for _ in 0..2 {
            for page in (1..PAGES).step_by(2) {
                // SAFETY: The window's memory, whose faults the fault thread serves.
                let loaded = unsafe { ((base + page * ps) as *const u8).read_volatile() };
                assert_eq!(loaded, tag(page), "page {page}");
                let runs = stage.installed() as usize;
                assert!(runs <= BUDGET, "page {page}: {runs} runs installed");
            }
        }
        let mut pipe = [0; 2];
        // SAFETY: Creates a pipe, which the test closes.
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        for page in (0..PAGES).step_by(2) {
            let at = base + page * ps;
            let mapped = mappings_within(at, ps);
            assert!(mapped[0].contains("fxdax-private-pages"), "page {page}: {mapped:#?}");
            let mut read = [0u8; 2];
            // SAFETY: Writes the page's first bytes to the pipe, and reads them back.
            unsafe {
                assert_eq!(libc::write(pipe[1], at as *const libc::c_void, 2), 2, "page {page}");
                assert_eq!(libc::read(pipe[0], read.as_mut_ptr().cast(), 2), 2);
            }
            assert_eq!(read, [tag(page), 0xee], "page {page}");
        }
        assert_eq!(window.private_pages(), (PAGES / 2) as u64);

        drop(dax);
        // SAFETY: Closes the pipe and unmaps the memory, which nothing uses any more.
        unsafe {
            libc::close(pipe[0]);
            libc::close(pipe[1]);
            assert_eq!(libc::munmap(base as *mut libc::c_void, PAGES * ps), 0);
        }
    }

    /// fxdax: a vCPU whose access keeps failing, `KVM_RUN` with `EFAULT`, tells what the stages
    /// serving faults hold at the page it failed at (`describe_windows`): a private page, a page
    /// installed, what of the window's pages is installed, left out, private, withheld and
    /// restricted, and what the stage is doing.
    #[test]
    fn stages_describe_their_pages() {
        let ps = page_size() as usize;
        let base = window_memory(2);
        // SAFETY: Only the test uses the memory, which outlives the window.
        let stage = unsafe { UffdSecondStage::in_process(base, (2 * ps) as u64, None) }.unwrap();
        let arena = crate::pages::ArenaId::DEFAULT;
        let copying = crate::dax::CopyOnWrite::reserve(arena, 2).unwrap();
        let (mut dax, window) =
            Dax::in_host_memory_copying(stage.clone(), Some(copying)).unwrap();
        let file = TestSource::filled(2, 7).source();
        dax.map(0, &file, 0, (2 * ps) as u64);
        futures::executor::block_on(window.make_private(0, ps as u64)).unwrap();

        let told = describe_windows(Some(base + 5));
        assert!(told.contains("is doing nothing"), "{told}");
        assert!(told.contains("1 pages installed, 0 left out, 1 private, 0 withheld"), "{told}");
        assert!(told.contains("; offset 0x0: private, changed "), "{told}");
        let told = describe_windows(Some(base + ps));
        let installed = format!("; offset {ps:#x}: installed of {arena:?} at ");
        assert!(told.contains(&installed), "{told}");
        let told = describe_windows(Some(base + 2 * ps));
        assert!(!told.contains("; offset"), "a page outside the window told of: {told}");

        drop(dax);
        // SAFETY: Unmaps the memory, which nothing uses any more.
        assert_eq!(unsafe { libc::munmap(base as *mut libc::c_void, 2 * ps) }, 0);
    }
}
