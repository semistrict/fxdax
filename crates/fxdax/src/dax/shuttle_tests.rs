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
use super::*;
use crate::kernel::PhysLocation;
use crate::shuttle_testing::{config, fails_with, runs};
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
}

/// How often each event happened, over a test's runs (each test is a process of its own).
static REACHED: [std::sync::atomic::AtomicUsize; 6] =
    [const { std::sync::atomic::AtomicUsize::new(0) }; 6];

fn reached(event: Event) {
    REACHED[event as usize].fetch_add(1, Ordering::Relaxed);
}

/// Runs `test` on the schedules of `scheduler`, then checks they reached each of `events`:
/// schedules that miss the races a test is for fail it.
pub(super) fn reaching(scheduler: impl Scheduler + 'static, test: fn(), events: &[Event]) {
    Runner::new(scheduler, config()).run(test);
    let counts: Vec<_> = events
        .iter()
        .map(|&event| (event, REACHED[event as usize].load(Ordering::Relaxed)))
        .collect();
    assert!(counts.iter().all(|(_, count)| *count > 0), "unreached: {counts:?}");
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
    fn map(&self, offset: u64, _host: *mut u8, phys: PhysLocation, len: u64, prot: Protection) {
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

/// A guest whose second stage is a [`FakeStage`], such as HVF's: an access the stage doesn't
/// allow faults, and the VMM passes the fault to the window.
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
    reaching(RandomScheduler::new(runs(RUNS)), seals_and_frees_on_a_fake_stage, &STAGE_EVENTS);
}

#[test]
fn stores_race_seals_and_frees_by_pct() {
    reaching(PctScheduler::new(3, runs(RUNS)), seals_and_frees_on_a_fake_stage, &STAGE_EVENTS);
}

/// A freed extent's mappings unmapped, rather than mapping zeros: a read racing the free faults
/// on a page the window maps nothing at.
#[test]
fn needs_frees_to_map_zeros() {
    *RULES.lock().unwrap() = Rules { free_maps_zeros: false };
    fails_with("ENOENT", || {
        Runner::new(PctScheduler::new(3, runs(10 * RUNS)), config())
            .run(seals_and_frees_on_a_fake_stage);
    });
}
