// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! DAX windows under Shuttle (awslabs/shuttle), with the kernel on Shuttle's locks (zircon's
//! `shuttle` feature): faults, the pager that supplies their pages, and the store that maps and
//! remaps them, on threads of their own, in every interleaving Shuttle's scheduler chooses
//! (TASK-44.3).
//!
//! The race of a guest's stores with a seal and a free is run against a [`Guest`]: a guest's view
//! of the window through a second stage, here one that records what the window maps
//! ([`FakeStage`]), and, on Linux, the userfaultfd second stage over a model of its host memory
//! (`uffd::shuttle_tests`).

use super::tests::TestPager;
use super::unmapped::{Change, Unmapped};
use super::*;
use crate::kernel::PhysLocation;
use crate::shuttle_testing::{BATCHES, config, fails_with, runs};
use futures::future::LocalBoxFuture;
use shuttle::scheduler::{PctScheduler, RandomScheduler, Scheduler};
use shuttle::{Runner, future, thread};
use std::os::fd::{AsRawFd, RawFd};

/// The schedules each test runs, before `SHUTTLE_SOAK`.
pub(super) const RUNS: usize = 100;

/// Faults on two pages, each on a thread of its own, wait for the pager, which supplies each page
/// asked for, its bytes naming it; each completes with its page mapped.
fn faults_wait_for_the_pager() {
    let ps = page_size();
    let pager = TestPager::new(2);
    let (dax, window) = Dax::<()>::new(2 * ps, None).unwrap();
    dax.window.map(0, &pager.vmo, 0, 2 * ps, false).unwrap();
    let faults: Vec<_> = (0..2)
        .map(|page| {
            let window = window.clone();
            thread::spawn(move || future::block_on(window.fault(page * ps, Access::Read)))
        })
        .collect();
    for _ in 0..2 {
        let request = pager.read_request(zx::MonotonicInstant::INFINITE);
        let page = request.start / ps;
        pager.supply(page, page as u8 + 1);
    }
    for fault in faults {
        assert_eq!(fault.join().unwrap(), Ok(()));
    }
    // SAFETY: Both pages are mapped, and nothing else accesses them.
    let bytes = unsafe { [*window.host_base(), *window.host_base().add(ps as usize)] };
    assert_eq!(bytes, [1, 2]);
}

#[test]
fn faults_wait_for_the_pager_randomly() {
    Runner::new(RandomScheduler::new(runs(RUNS)), config()).run(faults_wait_for_the_pager);
}

/// The kernel's waits are Shuttle's: a wait on a port for a request no fault makes is a
/// deadlock Shuttle reports, rather than a test that never ends.
#[test]
#[should_panic(expected = "deadlock")]
fn kernel_waits_are_shuttles() {
    Runner::new(RandomScheduler::new(1), config()).run(|| {
        let pager = TestPager::new(1);
        pager.read_request(zx::MonotonicInstant::INFINITE);
    });
}

/// How a vCPU's access to a page of the window goes.
pub(super) enum Outcome<'a> {
    /// It was made: the page's first byte after it.
    Made(u8),
    /// It faulted: the future resolves the fault, after which the vCPU retries the access.
    Faulted(LocalBoxFuture<'a, Result<()>>),
    /// It failed, as KVM's run of a vCPU fails with `EFAULT`, and the vCPU retries it, as fxdax's
    /// Firecracker does: the userfaultfd second stage's, on Linux.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Failed,
}

/// A guest's view of a window through its second stage: how its vCPUs' accesses go, each made at
/// once, against the stage as it is, as the hardware makes it.
pub(super) trait Guest: Send + Sync + 'static {
    /// Where a page's memory is, which tells a page's copies apart.
    type Where: Copy + PartialEq + std::fmt::Debug + Send;

    /// A vCPU's access of `kind` to the page at `offset`, storing `write` if it is a store.
    fn access(&self, offset: u64, kind: Access, write: Option<u8>) -> Outcome<'_>;

    /// The pages a store would change without faulting.
    fn writable(&self) -> Vec<u64>;

    /// Where the memory of each page mapped is.
    fn mapped(&self) -> Vec<(u64, Self::Where)>;
}

/// The first byte of the page at `offset` of `fd`'s memory, read, or written with `write` first,
/// as the guest would through its mapping.
pub(super) fn byte(fd: RawFd, offset: u64, write: Option<u8>) -> u8 {
    let len = page_size() as usize;
    let prot = libc::PROT_READ | libc::PROT_WRITE;
    // SAFETY: A new mapping of a page of the memory, which outlives it.
    let page =
        unsafe { libc::mmap(std::ptr::null_mut(), len, prot, libc::MAP_SHARED, fd, offset as _) };
    assert_ne!(page, libc::MAP_FAILED, "{}", std::io::Error::last_os_error());
    let page = page.cast::<u8>();
    // SAFETY: `page` is the mapping's first byte, which only the guest's accesses change, each at
    // once.
    let value = unsafe {
        if let Some(value) = write {
            page.write_volatile(value);
        }
        page.read_volatile()
    };
    // SAFETY: The mapping made above.
    assert_eq!(unsafe { libc::munmap(page.cast(), len) }, 0);
    value
}

pub(super) const OPEN: u8 = 0xa;
pub(super) const SEALED: u8 = 0xb;

/// What a page of the window holds, in the order the store changes it: an open extent's memory,
/// its sealed copy, then, freed, zeros.
fn rank(value: u8) -> u8 {
    match value {
        OPEN => 0,
        SEALED => 1,
        0 => 2,
        other => panic!("the guest read {other:#x}, which nothing mapped held"),
    }
}

/// What a race test's schedules are to reach, each in some run.
#[derive(Clone, Copy, Debug)]
pub(super) enum Event {
    ReadOpen,
    ReadSealed,
    ReadZeros,
    /// A fault resolved.
    Faulted,
    /// An access failed, and was retried.
    Failed,
    /// A store failed once the seal began.
    StoreRefused,
    /// fxdax: a page a `MAP` replaced read once the `MAP` returned, before its VM process made the
    /// change (TASK-92.50).
    ReadReplaced,
    /// fxdax: the page that replaced it read.
    ReadReplacing,
    /// fxdax: a fault mapped a page that a thread deferring its waits had unmapped, before the VM
    /// process was told of the unmap (TASK-91.16).
    FaultedUnmapped,
}

/// How often each event happened, over a test's runs (each test is a process of its own).
static REACHED: [std::sync::atomic::AtomicUsize; 9] =
    [const { std::sync::atomic::AtomicUsize::new(0) }; 9];

fn reached(event: Event) {
    REACHED[event as usize].fetch_add(1, Ordering::Relaxed);
}

/// Runs `test` on the schedules of `scheduler`, then checks they reached each of `events`:
/// schedules that miss the races a test is for fail it. They are random: a batch may miss one,
/// which isn't that it can't be reached (TASK-55), so batches of new schedules are run, from
/// `scheduler`, until they have reached each, up to [`BATCHES`].
pub(super) fn reaching<S: Scheduler + 'static>(
    scheduler: impl Fn() -> S,
    test: fn(),
    events: &[Event],
) {
    let counts = || -> Vec<_> {
        (events.iter())
            .map(|&event| (event, REACHED[event as usize].load(Ordering::Relaxed)))
            .collect()
    };
    for _ in 0..BATCHES {
        Runner::new(scheduler(), config()).run(test);
        if counts().iter().all(|(_, count)| *count > 0) {
            return;
        }
    }
    panic!("unreached in {BATCHES} batches: {:?}", counts());
}

/// The attempts an access makes before it is taken never to complete, as a vCPU faulting on a
/// page for ever: a few races' worth.
const ATTEMPTS: usize = 100;

/// Two vCPUs read and write two pages of an open extent's writable mapping while the store seals
/// it, mapping its sealed copy read-only in its place, then frees that, mapping zeros instead.
/// Each access `guest` refuses faults, and is retried once the fault is resolved, or fails, and is
/// retried. Each vCPU goes on until it reads both pages freed, as a guest polling them would.
/// Checked: a vCPU never reads a page older than one it read; once the seal returns, no page is
/// writable; once the free returns, no page of the sealed copy is mapped; a store fails only once
/// the seal began, and every access completes.
pub(super) fn stores_race_seals_and_frees<G: Guest>(mut dax: Dax<Arc<u8>>, guest: Arc<G>) {
    let ps = page_size();
    let vmo = |value| {
        let vmo = zx::Vmo::create(2 * ps).unwrap();
        vmo.write(&vec![value; 2 * ps as usize], 0).unwrap();
        vmo
    };
    let (open, sealed, zeros) = (vmo(OPEN), vmo(SEALED), vmo(0));
    let (opened, seal, free) = (Arc::new(0), Arc::new(1), Arc::new(2));
    dax.map(0, opened.clone(), &open, 0, 2 * ps, true).unwrap();
    let sealing = Arc::new(shuttle::sync::atomic::AtomicBool::new(false));

    let vcpus: Vec<_> = [[Access::Write, Access::Read], [Access::Read, Access::Write]]
        .into_iter()
        .map(|kinds| {
            let (guest, sealing) = (guest.clone(), sealing.clone());
            thread::spawn(move || {
                let (mut kinds, mut seen) = (kinds, [0; 2]);
                while seen != [rank(0); 2] {
                    for page in 0..2 {
                        let (offset, kind) = (page as u64 * ps, kinds[page]);
                        // A store to what has been sealed fails, which the guest's seal protocol
                        // (specs/dax/DaxSeal.tla) keeps it from making: it reads the page from
                        // then on.
                        let refused = || kind == Access::Write && sealing.load(Ordering::SeqCst);
                        let read = 'access: {
                            for _ in 0..ATTEMPTS {
                                let write = (kind == Access::Write).then_some(OPEN);
                                match guest.access(offset, kind, write) {
                                    Outcome::Made(read) => break 'access Some(read),
                                    Outcome::Faulted(fault) => match future::block_on(fault) {
                                        Ok(()) => reached(Event::Faulted),
                                        Err(_) if refused() => break 'access None,
                                        Err(err) => {
                                            panic!("a {kind:?} fault at {offset:#x}: {err:?}")
                                        }
                                    },
                                    Outcome::Failed if refused() => break 'access None,
                                    Outcome::Failed => {
                                        reached(Event::Failed);
                                        thread::yield_now();
                                    }
                                }
                            }
                            panic!("a {kind:?} at {offset:#x} never completed");
                        };
                        match read {
                            Some(read) => {
                                assert!(
                                    rank(read) >= seen[page],
                                    "read {read:#x} after a newer page"
                                );
                                seen[page] = rank(read);
                                let events = [Event::ReadOpen, Event::ReadSealed, Event::ReadZeros];
                                reached(events[seen[page] as usize]);
                            }
                            None => {
                                reached(Event::StoreRefused);
                                kinds[page] = Access::Read;
                            }
                        }
                    }
                    thread::yield_now();
                }
            })
        })
        .collect();

    let store = thread::spawn(move || {
        sealing.store(true, Ordering::SeqCst);
        let is_open = |keep: &Arc<u8>| Arc::ptr_eq(keep, &opened);
        dax.make_read_only(0, 2 * ps, seal.clone(), &sealed, 0, is_open).unwrap();
        let writable = guest.writable();
        assert!(writable.is_empty(), "writable after the seal: {writable:#x?}");
        let sealed: Vec<G::Where> = guest.mapped().into_iter().map(|(_, at)| at).collect();
        dax.replace_kept(seal.identity(), free, &zeros).unwrap();
        let still: Vec<_> =
            guest.mapped().into_iter().filter(|(_, at)| sealed.contains(at)).collect();
        assert!(still.is_empty(), "the sealed copy is mapped after it was freed: {still:#x?}");
    });
    for vcpu in vcpus {
        vcpu.join().unwrap();
    }
    store.join().unwrap();
}

/// A guest's second stage, as the window makes it: each page's physical page and protection.
#[derive(Default)]
struct FakeStage(shuttle::sync::Mutex<BTreeMap<u64, (PhysLocation, Protection)>>);

impl SecondStage for FakeStage {
    fn map(&self, offset: u64, phys: PhysLocation, len: u64, prot: Protection) {
        let mut pages = self.0.lock().unwrap();
        for at in (0..len).step_by(page_size() as usize) {
            let page = PhysLocation { arena: phys.arena, offset: phys.offset + at };
            let replaced = pages.insert(offset + at, (page, prot));
            assert!(replaced.is_none(), "mapped over the page at {:#x}", offset + at);
        }
    }

    fn protect(&self, offset: u64, len: u64, prot: Protection) {
        let mut pages = self.0.lock().unwrap();
        for at in (offset..offset + len).step_by(page_size() as usize) {
            pages.get_mut(&at).unwrap_or_else(|| panic!("protected unmapped {at:#x}")).1 = prot;
        }
    }

    fn unmap(&self, offset: u64, len: u64) {
        let mut pages = self.0.lock().unwrap();
        for at in (offset..offset + len).step_by(page_size() as usize) {
            assert!(pages.remove(&at).is_some(), "unmapped unmapped {at:#x}");
        }
    }
}

/// A guest whose second stage is a [`FakeStage`], as the extent daemon's connection to a VM
/// process is: an access the stage doesn't allow faults, and the VM process passes the fault to
/// the window.
struct StageGuest {
    stage: Arc<FakeStage>,
    window: DaxWindow,
}

impl Guest for StageGuest {
    type Where = PhysLocation;

    fn access(&self, offset: u64, kind: Access, write: Option<u8>) -> Outcome<'_> {
        let pages = self.stage.0.lock().unwrap();
        match pages.get(&offset) {
            Some(&(phys, protection))
                if protection == Protection::ReadWrite
                    || (kind == Access::Read && protection == Protection::Read) =>
            {
                let arena = crate::kernel::arena_memory(phys.arena).expect("the page's arena");
                Outcome::Made(byte(arena.as_raw_fd(), phys.offset, write))
            }
            _ => Outcome::Faulted(Box::pin(self.window.fault(offset, kind))),
        }
    }

    fn writable(&self) -> Vec<u64> {
        let pages = self.stage.0.lock().unwrap();
        pages
            .iter()
            .filter(|(_, (_, prot))| *prot == Protection::ReadWrite)
            .map(|(at, _)| *at)
            .collect()
    }

    fn mapped(&self) -> Vec<(u64, PhysLocation)> {
        let pages = self.stage.0.lock().unwrap();
        pages.iter().map(|(at, (phys, _))| (*at, *phys)).collect()
    }
}

fn seals_and_frees_on_a_fake_stage() {
    let stage = Arc::new(FakeStage::default());
    let (dax, window) = Dax::<Arc<u8>>::new(2 * page_size(), Some(stage.clone())).unwrap();
    stores_race_seals_and_frees(dax, Arc::new(StageGuest { stage, window }));
}

const STAGE_EVENTS: [Event; 5] =
    [Event::ReadOpen, Event::ReadSealed, Event::ReadZeros, Event::Faulted, Event::StoreRefused];

#[test]
fn stores_race_seals_and_frees_randomly() {
    reaching(|| RandomScheduler::new(runs(RUNS)), seals_and_frees_on_a_fake_stage, &STAGE_EVENTS);
}

#[test]
fn stores_race_seals_and_frees_by_pct() {
    reaching(|| PctScheduler::new(3, runs(RUNS)), seals_and_frees_on_a_fake_stage, &STAGE_EVENTS);
}

/// A freed extent's mappings unmapped, rather than mapping zeros: a read racing the free faults
/// on a page the window maps nothing at.
#[test]
fn needs_frees_to_map_zeros() {
    *RULES.lock().unwrap() = Rules { free_maps_zeros: false, ..Rules::ALL };
    fails_with("ENOENT", || {
        Runner::new(PctScheduler::new(3, runs(10 * RUNS)), config())
            .run(seals_and_frees_on_a_fake_stage);
    });
}

/// fxdax: zircon's `ZX_VMO_OP_FXDAX_EVICT`, which evicts a range's clean pages now.
const VMO_OP_EVICT: u32 = 0x8000_0001;

/// fxdax: what a page a `MAP` replaces, its replacement, and a page reused once freed hold.
const REPLACED: u8 = 0x1a;
const REPLACING: u8 = 0x1b;
const REUSED: u8 = 0x1c;

/// fxdax: what a test's window's mapping keeps alive: the VMO it maps, held from eviction while a
/// change deferred that replaced the mapping is made, as an extent store's mappings are.
#[derive(Clone)]
struct Kept(Arc<zx::Vmo>);

impl Keep for Kept {
    fn identity(&self) -> usize {
        self.0.identity()
    }

    fn hold(&self) -> Box<dyn Send> {
        Box::new(Held::new(self.0.clone()))
    }
}

/// fxdax: the changes sent to a [`RemoteFakeStage`], and what waits for them.
#[derive(Default)]
struct Sent {
    changes: std::collections::VecDeque<Change>,
    /// How many were sent, and how many made.
    sent: u64,
    made: u64,
    /// What is kept until the change of each ID is made (`Deferred::keep_until_made`).
    kept: Vec<(u64, Box<dyn Send>)>,
    /// Whether the VM process stops once it has made those sent.
    stopping: bool,
    /// The last change sent that takes access away.
    last_taking: u64,
    /// The pages unmapped while a thread deferred its waits, which the VM process still maps, as
    /// the extent daemon's stage keeps them (`dax::unmapped`).
    unmapped: Unmapped,
}

shuttle::thread_local! {
    /// While this thread defers its waits, the last change it would have waited for.
    static DEFERRING: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
    /// The pages this thread unmapped while it deferred its waits, kept per thread rather than
    /// per stage, as de7a0d2b kept them ([`RemoteFakeStage::per_thread`]).
    static UNMAPPED_HERE: std::cell::RefCell<Unmapped> = std::cell::RefCell::new(Unmapped::default());
}

/// fxdax: a window's second stage in another process, as the extent daemon's connection to a VM
/// process is (`extentd`'s `RemoteStage`): its changes are made in order, by the VM process's own
/// thread ([`RemoteFakeStage::serve`]), into a [`FakeStage`], what the guest's vCPUs access; one
/// that takes access away is waited for, but while the thread sending it defers its waits, whose
/// unmaps it tells of only as the kernel maps their pages again, or as the thread stops deferring
/// ([`Unmapped`]), deciding what to send and sending it holding one lock.
struct RemoteFakeStage {
    this: std::sync::Weak<Self>,
    made: Arc<FakeStage>,
    sent: shuttle::sync::Mutex<Sent>,
    changed: shuttle::sync::Condvar,
    /// Whether it keeps the pages unmapped per thread, as de7a0d2b did: a fault's map on another
    /// thread then reaches the VM process before the deferring thread's unmap of the page.
    per_thread: bool,
}

impl RemoteFakeStage {
    fn new() -> Arc<Self> {
        Self::keeping_unmapped(false)
    }

    fn keeping_unmapped(per_thread: bool) -> Arc<Self> {
        Arc::new_cyclic(|this| Self {
            this: this.clone(),
            made: Arc::default(),
            sent: shuttle::sync::Mutex::new(Sent::default()),
            changed: shuttle::sync::Condvar::new(),
            per_thread,
        })
    }

    /// Sends `change`, its ID.
    fn push(&self, sent: &mut Sent, change: Change) -> u64 {
        let taking = change.takes_access();
        sent.changes.push_back(change);
        sent.sent += 1;
        if taking {
            sent.last_taking = sent.sent;
        }
        self.changed.notify_all();
        sent.sent
    }

    /// What the pages unmapped decide, `decide` of them, sent: none waited for.
    fn decided(&self, decide: impl FnOnce(&mut Unmapped) -> Vec<Change>) {
        let mut sent = self.sent.lock().unwrap();
        let changes = if self.per_thread {
            UNMAPPED_HERE.with(|unmapped| decide(&mut unmapped.borrow_mut()))
        } else {
            decide(&mut sent.unmapped)
        };
        let deferring = DEFERRING.with(std::cell::Cell::get).is_some();
        if !deferring && changes.iter().any(Change::takes_access) {
            reached(Event::FaultedUnmapped);
        }
        for change in changes {
            self.push(&mut sent, change);
        }
    }

    fn send(&self, change: Change, wait: bool) {
        let mut sent = self.sent.lock().unwrap();
        let id = self.push(&mut sent, change);
        if !wait {
            return;
        }
        match DEFERRING.with(std::cell::Cell::get) {
            Some(last) => DEFERRING.with(|deferring| deferring.set(Some(last.max(id)))),
            None => self.wait_made(sent, id),
        }
    }

    fn wait_made(&self, mut sent: shuttle::sync::MutexGuard<'_, Sent>, id: u64) {
        while sent.made < id {
            sent = self.changed.wait(sent).unwrap();
        }
    }

    /// The VM process: makes the changes sent, in order, until stopped once it made them all.
    fn serve(&self) {
        loop {
            let mut sent = self.sent.lock().unwrap();
            while sent.changes.is_empty() && !sent.stopping {
                sent = self.changed.wait(sent).unwrap();
            }
            let Some(change) = sent.changes.pop_front() else { return };
            drop(sent);
            match change {
                Change::Map { offset, phys, len, protection } => {
                    self.made.map(offset, phys, len, protection)
                }
                Change::Protect { offset, len, protection } => {
                    self.made.protect(offset, len, protection)
                }
                Change::Unmap { offset, len } => self.made.unmap(offset, len),
            }
            let mut sent = self.sent.lock().unwrap();
            sent.made += 1;
            let made = sent.made;
            let (released, kept): (Vec<_>, Vec<_>) =
                std::mem::take(&mut sent.kept).into_iter().partition(|(id, _)| *id <= made);
            sent.kept = kept;
            self.changed.notify_all();
            drop(sent);
            drop(released);
        }
    }

    /// Stops the VM process once it has made the changes sent.
    fn stop(&self) {
        self.sent.lock().unwrap().stopping = true;
        self.changed.notify_all();
    }
}

impl SecondStage for RemoteFakeStage {
    fn map(&self, offset: u64, phys: PhysLocation, len: u64, protection: Protection) {
        self.decided(|unmapped| unmapped.map(offset, phys, len, protection));
    }

    fn protect(&self, offset: u64, len: u64, protection: Protection) {
        self.decided(|unmapped| unmapped.take_within(offset..offset + len));
        let change = Change::Protect { offset, len, protection };
        self.send(change, protection != Protection::ReadWrite);
    }

    fn unmap(&self, offset: u64, len: u64) {
        self.decided(|unmapped| unmapped.take_within(offset..offset + len));
        self.send(Change::Unmap { offset, len }, true);
    }

    fn unmap_pages(&self, offset: u64, len: u64, phys: &[PhysLocation]) {
        if DEFERRING.with(std::cell::Cell::get).is_none() {
            return self.unmap(offset, len);
        }
        self.decided(|unmapped| {
            unmapped.unmapped(offset, phys);
            Vec::new()
        });
    }

    fn begin_deferring(&self) {
        DEFERRING.with(|deferring| deferring.set(Some(0)));
    }

    fn end_deferring(&self) -> Option<Arc<dyn Deferred>> {
        DEFERRING.with(std::cell::Cell::get)?;
        self.decided(Unmapped::take);
        DEFERRING.with(std::cell::Cell::take);
        let stage = self.this.upgrade().unwrap();
        let sent = self.sent.lock().unwrap();
        let id = sent.last_taking;
        (id > sent.made).then(|| Arc::new(RemoteDeferral { stage, id }) as _)
    }
}

/// fxdax: the changes a [`RemoteFakeStage`] deferred, up to `id`.
struct RemoteDeferral {
    stage: Arc<RemoteFakeStage>,
    id: u64,
}

impl Deferred for RemoteDeferral {
    fn wait(&self) {
        self.stage.wait_made(self.stage.sent.lock().unwrap(), self.id);
    }

    fn keep_until_made(&self, keep: Box<dyn Send>) {
        let mut sent = self.stage.sent.lock().unwrap();
        if sent.made < self.id {
            sent.kept.push((self.id, keep));
        }
    }
}

/// fxdax (TASK-92.50): a vCPU reads a page of the window while the store maps another over it,
/// deferring the change, as a `MAP` does, whose VM process makes it in its own time; then evicts
/// what the replaced mapping mapped, a pager-backed page, and reuses what that frees for another
/// page at once, written. The vCPU reads the page replaced until the VM process unmaps it, then its
/// replacement, never the page reused: what the mapping replaced is held until the change is
/// made, from eviction too, which would free it without the second stage, as the window maps it
/// no more.
fn maps_race_evictions() {
    let ps = page_size();
    let stage = RemoteFakeStage::new();
    let (mut dax, window) = Dax::<Kept>::new(ps, Some(stage.clone())).unwrap();
    let vm = thread::spawn({
        let stage = stage.clone();
        move || stage.serve()
    });
    let replaced = TestPager::new(1);
    replaced.supply(0, REPLACED);
    let replacing = Arc::new(zx::Vmo::create(ps).unwrap());
    replacing.write(&vec![REPLACING; ps as usize], 0).unwrap();
    let was = Kept(Arc::new(replaced.vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap()));
    dax.map(0, was, &replaced.vmo, 0, ps, false).unwrap();
    let mapped = Arc::new(shuttle::sync::atomic::AtomicBool::new(false));

    let vcpu = thread::spawn({
        let guest = StageGuest { stage: stage.made.clone(), window };
        let mapped = mapped.clone();
        move || {
            for _ in 0..ATTEMPTS {
                match guest.access(0, Access::Read, None) {
                    Outcome::Made(REPLACING) => return reached(Event::ReadReplacing),
                    Outcome::Made(REPLACED) => {
                        if mapped.load(Ordering::SeqCst) {
                            reached(Event::ReadReplaced);
                        }
                    }
                    Outcome::Made(other) => panic!("the guest read {other:#x}, a page reused"),
                    Outcome::Faulted(fault) => future::block_on(fault).unwrap(),
                    Outcome::Failed => unreachable!("a fake stage's accesses fault"),
                }
                thread::yield_now();
            }
            panic!("the guest never read the page that replaced the first");
        }
    });
    let store = thread::spawn(move || {
        let is = Kept(replacing.clone());
        let (mapping, _) = dax.deferring(|dax| dax.map(0, is, &replacing, 0, ps, false));
        mapping.unwrap();
        mapped.store(true, Ordering::SeqCst);
        replaced.vmo.op_range(zx::VmoOp::from_raw(VMO_OP_EVICT), 0, ps).unwrap();
        // What eviction freed, taken at once by another page, written.
        let reused = zx::Vmo::create(ps).unwrap();
        reused.write(&vec![REUSED; ps as usize], 0).unwrap();
        (dax, replaced, reused)
    });
    vcpu.join().unwrap();
    let (dax, replaced, _reused) = store.join().unwrap();
    // The window goes, and its mappings with it, while the VM process still makes the changes.
    drop(dax);
    stage.stop();
    vm.join().unwrap();
    // Once the change is made, nothing holds what the mapping replaced.
    replaced.vmo.op_range(zx::VmoOp::from_raw(VMO_OP_EVICT), 0, ps).unwrap();
    assert_eq!(
        replaced.vmo.info().unwrap().committed_bytes,
        0,
        "still held once the change was made"
    );
}

const MAP_EVENTS: [Event; 2] = [Event::ReadReplaced, Event::ReadReplacing];

/// Random schedules seldom have the VM process wait out the rest of a `MAP` once it has the change
/// to make: the guest reads what the `MAP` replaced once it returned in those PCT's priorities
/// give.
#[test]
fn maps_race_evictions_randomly() {
    reaching(|| RandomScheduler::new(runs(RUNS)), maps_race_evictions, &MAP_EVENTS[1..]);
}

#[test]
fn maps_race_evictions_by_pct() {
    reaching(|| PctScheduler::new(3, runs(RUNS)), maps_race_evictions, &MAP_EVENTS);
}

/// fxdax (TASK-92.50): what a deferred change replaces not held, eviction frees the page the guest
/// still reaches, which another page takes, and the guest reads it.
#[test]
fn needs_deferred_changes_to_hold_what_they_replace() {
    *RULES.lock().unwrap() = Rules { deferred_changes_hold_what_they_replace: false, ..Rules::ALL };
    fails_with("a page reused", || {
        Runner::new(PctScheduler::new(3, runs(10 * RUNS)), config()).run(maps_race_evictions);
    });
}

/// fxdax (TASK-91.16): a fault served on a thread of its own, as a vCPU's store to a page its VM
/// process write-protects, or the prefetch of the page after a miss, maps a page of the window
/// while the store maps another over it, deferring the change, as a `MAP` does; the VM process
/// makes the changes in its own time. Once it has made them all it maps what the kernel maps, the
/// page that replaced the first, which the vCPU then loads without a fault.
fn faults_race_deferred_unmaps(stage: Arc<RemoteFakeStage>) {
    let ps = page_size();
    let (mut dax, window) = Dax::<Kept>::new(ps, Some(stage.clone())).unwrap();
    let vm = thread::spawn({
        let stage = stage.clone();
        move || stage.serve()
    });
    let replaced = Arc::new(zx::Vmo::create(ps).unwrap());
    replaced.write(&vec![REPLACED; ps as usize], 0).unwrap();
    let replacing = Arc::new(zx::Vmo::create(ps).unwrap());
    replacing.write(&vec![REPLACING; ps as usize], 0).unwrap();
    dax.map(0, Kept(replaced.clone()), &replaced, 0, ps, false).unwrap();
    // The page mapped, as the guest's first load maps it.
    future::block_on(window.fault(0, Access::Read)).unwrap();

    let fault = thread::spawn({
        let window = window.clone();
        move || future::block_on(window.fault(0, Access::Read)).unwrap()
    });
    let store = thread::spawn(move || {
        let is = Kept(replacing.clone());
        let (mapping, _) = dax.deferring(|dax| dax.map(0, is, &replacing, 0, ps, false));
        mapping.unwrap();
        dax
    });
    fault.join().unwrap();
    let dax = store.join().unwrap();
    // Once the VM process has made every change sent.
    let sent = stage.sent.lock().unwrap();
    let last = sent.sent;
    stage.wait_made(sent, last);
    let guest = StageGuest { stage: stage.made.clone(), window };
    let Outcome::Made(read) = guest.access(0, Access::Read, None) else {
        panic!("the VM process maps nothing where the kernel maps a page")
    };
    assert_eq!(read, REPLACING, "the VM process maps another page than the kernel");
    // The window goes, and its mappings with it, while the VM process still makes the changes.
    drop((dax, guest));
    stage.stop();
    vm.join().unwrap();
}

fn faults_race_deferred_unmaps_per_stage() {
    faults_race_deferred_unmaps(RemoteFakeStage::new());
}

#[test]
fn faults_race_deferred_unmaps_randomly() {
    reaching(
        || RandomScheduler::new(runs(RUNS)),
        faults_race_deferred_unmaps_per_stage,
        &[Event::FaultedUnmapped],
    );
}

#[test]
fn faults_race_deferred_unmaps_by_pct() {
    reaching(
        || PctScheduler::new(3, runs(RUNS)),
        faults_race_deferred_unmaps_per_stage,
        &[Event::FaultedUnmapped],
    );
}

/// fxdax (TASK-91.16): the pages unmapped kept per thread, as de7a0d2b kept them, a fault's map on
/// another thread reaches the VM process before the deferring thread's unmap of the page it
/// replaced, which it maps over.
#[test]
fn needs_deferred_unmaps_kept_per_stage() {
    fails_with("mapped over the page", || {
        Runner::new(PctScheduler::new(3, runs(10 * RUNS)), config())
            .run(|| faults_race_deferred_unmaps(RemoteFakeStage::keeping_unmapped(true)));
    });
}
