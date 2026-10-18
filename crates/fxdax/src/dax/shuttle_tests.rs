// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! DAX windows under Shuttle (awslabs/shuttle), with the kernel on Shuttle's locks (zircon's
//! `shuttle` feature): faults, the pager that supplies their pages, and the store that maps and
//! remaps them, on threads of their own, in every interleaving Shuttle's scheduler chooses
//! (TASK-44.3).

use super::tests::TestPager;
use super::*;
use crate::kernel::PhysLocation;
use shuttle::scheduler::{PctScheduler, RandomScheduler};
use shuttle::{Config, Runner, future, thread};

/// The schedules each test runs; `SHUTTLE_SOAK` multiplies them.
fn runs() -> usize {
    let soak: usize = std::env::var("SHUTTLE_SOAK").map_or(1, |soak| soak.parse().unwrap());
    100 * soak
}

fn config() -> Config {
    let mut config = Config::new();
    config.stack_size = 4 << 20;
    config
}

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
    Runner::new(RandomScheduler::new(runs()), config()).run(faults_wait_for_the_pager);
}

/// A guest's second stage, as the window makes it: each page's physical page and protection. An
/// access is made as the hardware makes it, at once, against the stage as it is.
#[derive(Default)]
struct FakeStage(shuttle::sync::Mutex<BTreeMap<u64, (PhysLocation, Protection)>>);

impl FakeStage {
    /// Runs `access` on the physical page mapped at `offset`, if it is mapped for `kind`.
    fn access<T>(
        &self,
        offset: u64,
        kind: Access,
        access: impl FnOnce(PhysLocation) -> T,
    ) -> Option<T> {
        let pages = self.0.lock().unwrap();
        let &(phys, protection) = pages.get(&offset)?;
        let allowed = match kind {
            Access::Read => protection != Protection::None,
            Access::Write => protection == Protection::ReadWrite,
        };
        allowed.then(|| access(phys))
    }

    /// The physical pages mapped, with their protections.
    fn pages(&self) -> BTreeMap<u64, (PhysLocation, Protection)> {
        self.0.lock().unwrap().clone()
    }
}

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

/// The first byte of the physical page `phys`, read, or written with `write`, as the guest would
/// through its mapping: through a mapping of the page's arena.
fn byte(phys: PhysLocation, write: Option<u8>) -> u8 {
    let arena = crate::kernel::arena_memory(phys.arena).expect("the page's arena");
    let fd = std::os::fd::AsRawFd::as_raw_fd(&*arena);
    let len = page_size() as usize;
    let prot = libc::PROT_READ | libc::PROT_WRITE;
    // SAFETY: A new mapping of a page of the arena's object, which outlives it.
    let page = unsafe {
        libc::mmap(std::ptr::null_mut(), len, prot, libc::MAP_SHARED, fd, phys.offset as _)
    };
    assert_ne!(page, libc::MAP_FAILED, "{}", std::io::Error::last_os_error());
    let page = page.cast::<u8>();
    // SAFETY: `page` is the mapping's first byte, which only the guest's accesses change, under the
    // stage's lock.
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

const OPEN: u8 = 0xa;
const SEALED: u8 = 0xb;

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

/// What a race test's schedules are to reach, each in some run: reads of each of a page's
/// versions, faults resolved, and stores refused once the seal began.
#[derive(Clone, Copy, Debug)]
enum Event {
    ReadOpen,
    ReadSealed,
    ReadZeros,
    Faulted,
    StoreRefused,
}

const EVENTS: [Event; 5] =
    [Event::ReadOpen, Event::ReadSealed, Event::ReadZeros, Event::Faulted, Event::StoreRefused];

/// How often each event happened, over a test's runs (each test is a process of its own).
static REACHED: [std::sync::atomic::AtomicUsize; EVENTS.len()] =
    [const { std::sync::atomic::AtomicUsize::new(0) }; EVENTS.len()];

fn reached(event: Event) {
    REACHED[event as usize].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Runs `test` on the schedules of `scheduler`, then checks they reached each event: schedules
/// that miss the races a test is for fail it.
fn reaching(scheduler: impl shuttle::scheduler::Scheduler + 'static, test: fn()) {
    Runner::new(scheduler, config()).run(test);
    let counts = EVENTS.map(|event| (event, REACHED[event as usize].load(Ordering::Relaxed)));
    assert!(counts.iter().all(|(_, count)| *count > 0), "unreached: {counts:?}");
}

/// Two vCPUs read and write two pages of an open extent's writable mapping while the store seals
/// it, mapping its sealed copy read-only in its place, then frees that, mapping zeros instead.
/// Each access the stage refuses faults, and is retried once the fault is resolved, as KVM's and
/// HVF's vCPUs do. A vCPU never reads a page older than one it read; once the seal returns, no
/// page of the open extent is writable; once the free returns, no page of the sealed copy is
/// mapped; a store faults only until the seal begins.
fn stores_race_seals_and_frees() {
    let ps = page_size();
    let stage = Arc::new(FakeStage::default());
    let (mut dax, window) = Dax::<Arc<u8>>::new(2 * ps, Some(stage.clone())).unwrap();
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
            let (stage, window, sealing) = (stage.clone(), window.clone(), sealing.clone());
            thread::spawn(move || {
                // Until it reads each page freed, as a guest polling them would.
                let (mut kinds, mut seen) = (kinds, [0; 2]);
                while seen != [rank(0); 2] {
                    for page in 0..2 {
                        let (offset, kind) = (page as u64 * ps, kinds[page]);
                        let read = loop {
                            let write = (kind == Access::Write).then_some(OPEN);
                            if let Some(read) = stage.access(offset, kind, |phys| byte(phys, write))
                            {
                                break Some(read);
                            }
                            match future::block_on(window.fault(offset, kind)) {
                                Ok(()) => reached(Event::Faulted),
                                // A store to what has been sealed, which the guest's seal protocol
                                // (specs/dax/DaxSeal.tla) keeps it from making: it reads the page
                                // from then on.
                                Err(_)
                                    if kind == Access::Write && sealing.load(Ordering::SeqCst) =>
                                {
                                    reached(Event::StoreRefused);
                                    kinds[page] = Access::Read;
                                    break None;
                                }
                                Err(err) => panic!("a {kind:?} fault at {offset:#x}: {err:?}"),
                            }
                        };
                        if let Some(read) = read {
                            assert!(rank(read) >= seen[page], "read {read:#x} after a newer page");
                            seen[page] = rank(read);
                            let events = [Event::ReadOpen, Event::ReadSealed, Event::ReadZeros];
                            reached(events[seen[page] as usize]);
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
        let after_seal = stage.pages();
        let writable: Vec<_> =
            after_seal.iter().filter(|(_, (_, prot))| *prot == Protection::ReadWrite).collect();
        assert!(writable.is_empty(), "writable after the seal: {writable:?}");
        dax.replace_kept(seal.identity(), free, &zeros).unwrap();
        let sealed_pages: Vec<PhysLocation> = after_seal.values().map(|(phys, _)| *phys).collect();
        let still: Vec<_> = stage
            .pages()
            .into_iter()
            .filter(|(_, (phys, _))| sealed_pages.contains(phys))
            .collect();
        assert!(still.is_empty(), "the sealed copy is mapped after it was freed: {still:?}");
    });
    for vcpu in vcpus {
        vcpu.join().unwrap();
    }
    store.join().unwrap();
}

#[test]
fn stores_race_seals_and_frees_randomly() {
    reaching(RandomScheduler::new(runs()), stores_race_seals_and_frees);
}

#[test]
fn stores_race_seals_and_frees_by_pct() {
    reaching(PctScheduler::new(3, runs()), stores_race_seals_and_frees);
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
