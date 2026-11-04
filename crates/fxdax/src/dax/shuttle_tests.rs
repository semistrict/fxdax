// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! DAX windows under Shuttle (awslabs/shuttle), on Shuttle's locks: faults, the source that
//! brings their pages in, and the store that maps and remaps them, on threads of their own, in
//! every interleaving Shuttle's scheduler chooses (TASK-44.3).
//!
//! The race of a guest's stores with a seal and a free is run against a [`Guest`]: a guest's view
//! of the window through a second stage, here one that records what the window maps
//! ([`FakeStage`]), and, on Linux, the userfaultfd second stage over a model of its host memory
//! (`uffd::shuttle_tests`).

use super::tests::TestSource;
use super::*;
use crate::pages::PhysLocation;
use crate::shuttle_testing::{BATCHES, config, fails_with, runs};
use futures::future::LocalBoxFuture;
use shuttle::scheduler::{PctScheduler, RandomScheduler, Scheduler};
use shuttle::{Runner, future, thread};
use std::os::fd::{AsRawFd, RawFd};

/// The schedules each test runs, before `SHUTTLE_SOAK`.
pub(super) const RUNS: usize = 100;

/// Faults on two pages, each on a thread of its own, wait for the source, which supplies each page
/// asked for, its bytes naming it; each completes with its page mapped.
fn faults_wait_for_the_pager() {
    let ps = page_size();
    let pager = TestSource::paged(2);
    let (dax, window) = Dax::<()>::new(2 * ps, None).unwrap();
    dax.window.map(0, &pager.source(), 0, 2 * ps, false).unwrap();
    let faults: Vec<_> = (0..2)
        .map(|page| {
            let window = window.clone();
            thread::spawn(move || future::block_on(window.fault(page * ps, Access::Read)))
        })
        .collect();
    for _ in 0..2 {
        let page = pager.request();
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

/// The test source's waits are Shuttle's: a wait for a request no fault makes is a deadlock
/// Shuttle reports, rather than a test that never ends.
#[test]
#[should_panic(expected = "deadlock")]
fn source_waits_are_shuttles() {
    Runner::new(RandomScheduler::new(1), config()).run(|| {
        let pager = TestSource::paged(1);
        pager.request();
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
    /// fxdax: a store's private copy that a `MAP` after it ended.
    MapEndedCopy,
    /// fxdax: a store's private copy of what a `MAP` before it mapped.
    CopiedAfterMap,
}

/// How often each event happened, over a test's runs (each test is a process of its own).
static REACHED: [std::sync::atomic::AtomicUsize; 8] =
    [const { std::sync::atomic::AtomicUsize::new(0) }; 8];

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
pub(super) const ATTEMPTS: usize = 100;

/// Two vCPUs read and write two pages of an open extent's writable mapping while the store seals
/// it, mapping its sealed copy read-only in its place, then frees that, mapping zeros instead.
/// Each access `guest` refuses faults, and is retried once the fault is resolved, or fails, and is
/// retried. Each vCPU goes on until it reads both pages freed, as a guest polling them would.
/// Checked: a vCPU never reads a page older than one it read; once the seal returns, no page is
/// writable; once the free returns, no page of the sealed copy is mapped; a store fails only once
/// the seal began, and every access completes.
pub(super) fn stores_race_seals_and_frees<G: Guest>(mut dax: Dax<Arc<u8>>, guest: Arc<G>) {
    let ps = page_size();
    let (open, sealed) = (TestSource::filled(2, OPEN).source(), TestSource::filled(2, SEALED));
    let (sealed, zeros) = (sealed.source(), crate::cache::zeros().unwrap());
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

/// Where a page of a [`FakeStage`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Frame {
    /// A page of physical memory.
    Phys(PhysLocation),
    /// fxdax: a private page (backlog doc-13), the stage's own, by the number it was made as,
    /// which tells copies apart.
    Private(u64),
}

/// A guest's second stage, as the window makes it.
#[derive(Default)]
struct Fake {
    /// Each page's memory and protection.
    pages: BTreeMap<u64, (Frame, Protection)>,
    /// fxdax: the private pages' bytes, by their numbers.
    private: BTreeMap<u64, Vec<u8>>,
}

#[derive(Default)]
struct FakeStage(shuttle::sync::Mutex<Fake>);

impl SecondStage for FakeStage {
    fn map(&self, offset: u64, phys: PhysLocation, len: u64, prot: Protection) {
        let mut fake = self.0.lock().unwrap();
        for at in (0..len).step_by(page_size() as usize) {
            let page = PhysLocation { arena: phys.arena, offset: phys.offset + at };
            let replaced = fake.pages.insert(offset + at, (Frame::Phys(page), prot));
            assert!(replaced.is_none(), "mapped over the page at {:#x}", offset + at);
        }
    }

    fn protect(&self, offset: u64, len: u64, prot: Protection) {
        let mut fake = self.0.lock().unwrap();
        for at in (offset..offset + len).step_by(page_size() as usize) {
            let page = fake.pages.get_mut(&at);
            page.unwrap_or_else(|| panic!("protected unmapped {at:#x}")).1 = prot;
        }
    }

    fn unmap(&self, offset: u64, len: u64) {
        let mut fake = self.0.lock().unwrap();
        for at in (offset..offset + len).step_by(page_size() as usize) {
            match fake.pages.remove(&at) {
                Some((Frame::Private(number), _)) => drop(fake.private.remove(&number)),
                Some((Frame::Phys(_), _)) => {}
                None => panic!("unmapped unmapped {at:#x}"),
            }
        }
    }

    /// fxdax: the page at `offset` a private page, in place of what was there, holding the page's
    /// at `from`, or zeros.
    fn make_private(&self, offset: u64, from: Option<PhysLocation>) {
        let bytes = from.map_or_else(|| vec![0; page_size() as usize], page_bytes);
        let mut fake = self.0.lock().unwrap();
        let number = fake.private.keys().next_back().map_or(0, |last| last + 1);
        fake.private.insert(number, bytes);
        if let Some((Frame::Private(was), _)) =
            fake.pages.insert(offset, (Frame::Private(number), Protection::ReadWrite))
        {
            panic!("{offset:#x} made private over private page {was}");
        }
    }
}

/// fxdax: the bytes of the page at `phys`.
fn page_bytes(phys: PhysLocation) -> Vec<u8> {
    let arena = crate::pages::arena_memory(phys.arena).expect("the page's arena");
    let len = page_size() as usize;
    let (prot, flags, fd) = (libc::PROT_READ, libc::MAP_SHARED, arena.as_raw_fd());
    // SAFETY: A new mapping of a page of the memory, which outlives it.
    let page =
        unsafe { libc::mmap(std::ptr::null_mut(), len, prot, flags, fd, phys.offset as _) };
    assert_ne!(page, libc::MAP_FAILED, "{}", std::io::Error::last_os_error());
    // SAFETY: `page` is the mapping, `len` bytes.
    let bytes = unsafe { std::slice::from_raw_parts(page.cast::<u8>(), len) }.to_vec();
    // SAFETY: The mapping made above.
    assert_eq!(unsafe { libc::munmap(page, len) }, 0);
    bytes
}

/// A guest whose second stage is a [`FakeStage`], as the extent daemon's connection to a VM
/// process is: an access the stage doesn't allow faults, and the VM process passes the fault to
/// the window.
struct StageGuest {
    stage: Arc<FakeStage>,
    window: DaxWindow,
}

impl Guest for StageGuest {
    type Where = Frame;

    fn access(&self, offset: u64, kind: Access, write: Option<u8>) -> Outcome<'_> {
        let mut fake = self.stage.0.lock().unwrap();
        match fake.pages.get(&offset).copied() {
            Some((frame, protection))
                if protection == Protection::ReadWrite
                    || (kind == Access::Read && protection == Protection::Read) =>
            {
                match frame {
                    Frame::Phys(phys) => {
                        let arena =
                            crate::pages::arena_memory(phys.arena).expect("the page's arena");
                        Outcome::Made(byte(arena.as_raw_fd(), phys.offset, write))
                    }
                    Frame::Private(number) => {
                        let bytes = fake.private.get_mut(&number).expect("a private page's bytes");
                        if let Some(value) = write {
                            bytes[0] = value;
                        }
                        Outcome::Made(bytes[0])
                    }
                }
            }
            _ => Outcome::Faulted(Box::pin(self.window.fault(offset, kind))),
        }
    }

    fn writable(&self) -> Vec<u64> {
        let fake = self.stage.0.lock().unwrap();
        (fake.pages.iter())
            .filter(|(_, (_, prot))| *prot == Protection::ReadWrite)
            .map(|(at, _)| *at)
            .collect()
    }

    fn mapped(&self) -> Vec<(u64, Frame)> {
        let fake = self.stage.0.lock().unwrap();
        fake.pages.iter().map(|(at, (frame, _))| (*at, *frame)).collect()
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

/// fxdax: what pages of a window that copies on write hold (backlog doc-13): an extent's, another
/// mapped after it, and what a vCPU stores.
const FIRST: u8 = 0x1a;
const SECOND: u8 = 0x1b;
const STORED: u8 = 0x1c;

/// fxdax: a vCPU's store of [`STORED`] to the page at `offset`, retried as each fault is resolved:
/// whether it was made, or refused (`EACCES`).
fn stored(guest: &StageGuest, offset: u64) -> bool {
    for _ in 0..ATTEMPTS {
        match guest.access(offset, Access::Write, Some(STORED)) {
            Outcome::Made(read) => {
                assert_eq!(read, STORED, "the store at {offset:#x} read back otherwise");
                return true;
            }
            Outcome::Faulted(fault) => match future::block_on(fault) {
                Ok(()) => {}
                Err(Errno::EACCES) => return false,
                Err(err) => panic!("a store's fault at {offset:#x}: {err:?}"),
            },
            Outcome::Failed => unreachable!("a fake stage's accesses fault"),
        }
    }
    panic!("a store at {offset:#x} never completed");
}

/// fxdax TASK-164.1: a vCPU stores to a page that a window copying on write maps read-only, an
/// extent's, while the guest's `MAP` maps another's there: as the guest doesn't (it holds its
/// file's semaphore), but the host stays whole whatever it does. Checked once both are done: the
/// page maps the `MAP`'s, read-only, the copy the store made before ended; or a private copy of
/// it, holding the store: never a copy of what the `MAP` replaced. The window counts the private
/// pages the stage maps writable.
fn copies_race_maps() {
    let ps = page_size();
    let stage = Arc::new(FakeStage::default());
    let (mut dax, window) = super::tests::copying::<()>(1, 1, Some(stage.clone()));
    let (first, second) = (TestSource::filled(1, FIRST), TestSource::filled(1, SECOND));
    dax.map(0, (), &first.source(), 0, ps, false).unwrap();
    let guest = Arc::new(StageGuest { stage: stage.clone(), window: window.clone() });
    let storing = guest.clone();
    let vcpu = thread::spawn(move || assert!(stored(&storing, 0), "the store was refused"));
    let mapped = second.source();
    let map = thread::spawn(move || {
        dax.map(0, (), &mapped, 0, ps, false).unwrap();
        dax
    });
    vcpu.join().unwrap();
    let _dax = map.join().unwrap();

    let mut shared = None;
    second.each_present(0..1, &mut |_, frame| shared = Some(Frame::Phys(frame.location())));
    let (frame, protection) = stage.0.lock().unwrap().pages[&0];
    let writable = guest.writable();
    assert_eq!(window.private_pages(), writable.len() as u64, "private pages miscounted");
    match (frame, protection) {
        (_, Protection::Read) => {
            assert_eq!(Some(frame), shared, "the page maps neither the MAP's nor a copy");
            reached(Event::MapEndedCopy);
        }
        (Frame::Private(number), Protection::ReadWrite) => {
            let bytes = stage.0.lock().unwrap().private[&number].clone();
            assert_eq!(bytes[0], STORED, "the private page lost the store");
            assert_eq!(bytes[1], SECOND, "a copy of what the MAP replaced");
            reached(Event::CopiedAfterMap);
        }
        (Frame::Phys(_), Protection::ReadWrite) => panic!("the MAP's page made writable"),
    }
}

const COPY_EVENTS: [Event; 2] = [Event::MapEndedCopy, Event::CopiedAfterMap];

/// For the invariants alone: a random schedule practically never lets the store's copy finish
/// before the `MAP` (none of 800), which the PCT schedules below reach.
#[test]
fn copies_race_maps_randomly() {
    Runner::new(RandomScheduler::new(runs(RUNS)), config()).run(copies_race_maps);
}

#[test]
fn copies_race_maps_by_pct() {
    reaching(|| PctScheduler::new(3, runs(RUNS)), copies_race_maps, &COPY_EVENTS);
}

/// fxdax: a store's copy mapped whatever the window maps by then: one of what a `MAP` replaced
/// meanwhile is mapped in place of the `MAP`'s pages.
#[test]
fn needs_copies_to_recheck_their_mappings() {
    *RULES.lock().unwrap() = Rules { copy_rechecks_mapping: false, ..Rules::ALL };
    fails_with("a copy of what the MAP replaced", || {
        Runner::new(PctScheduler::new(3, runs(10 * RUNS)), config()).run(copies_race_maps);
    });
}

/// fxdax TASK-164.1: two vCPUs store to two pages of a window that holds one private page at
/// most: one store is made, in a private copy, and the other refused, never both copied.
fn copies_race_their_bound() {
    let ps = page_size();
    let stage = Arc::new(FakeStage::default());
    let (mut dax, window) = super::tests::copying::<()>(2, 1, Some(stage.clone()));
    let source = TestSource::filled(2, FIRST);
    dax.map(0, (), &source.source(), 0, 2 * ps, false).unwrap();
    let guest = Arc::new(StageGuest { stage, window: window.clone() });
    let vcpus: Vec<_> = (0..2)
        .map(|page| {
            let guest = guest.clone();
            thread::spawn(move || stored(&guest, page * ps))
        })
        .collect();
    let made: Vec<bool> = vcpus.into_iter().map(|vcpu| vcpu.join().unwrap()).collect();
    let (private, writable) = (window.private_pages(), guest.writable().len());
    assert!(private <= 1 && writable <= 1, "past the window's private pages: {private}, {writable}");
    assert_eq!(made.iter().filter(|made| **made).count(), 1, "stores made: {made:?}");
    drop(dax);
}

#[test]
fn copies_race_their_bound_by_pct() {
    Runner::new(PctScheduler::new(3, runs(RUNS)), config()).run(copies_race_their_bound);
}

/// fxdax: copies counted against the bound before they are made alone: two stores each find room
/// for one, and both are mapped.
#[test]
fn needs_copies_counted_as_they_are_mapped() {
    *RULES.lock().unwrap() = Rules { copy_counts_under_lock: false, ..Rules::ALL };
    fails_with("past the window's private pages", || {
        Runner::new(PctScheduler::new(3, runs(10 * RUNS)), config()).run(copies_race_their_bound);
    });
}
