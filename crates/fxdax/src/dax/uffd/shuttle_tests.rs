// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The userfaultfd second stage under Shuttle (TASK-44.3): its real code, the window's and the
//! kernel's, over a model of the host memory it changes, [`ModelMemory`], which a guest's accesses
//! are made against as KVM makes them. Each of the stage's rules of the order it changes its
//! memory in, turned off, fails.

use super::super::shuttle_tests::{
    Event, Guest, Outcome, RUNS, SEALED, byte, reaching, stores_race_seals_and_frees,
};
use super::super::{Dax, Window};
use super::*;
use crate::kernel::PhysLocation;
use crate::shuttle_testing::{config, fails_with, runs};
use shuttle::scheduler::{PctScheduler, RandomScheduler};
use shuttle::{Runner, thread};
use std::collections::BTreeMap;
use std::os::fd::RawFd;

/// A page of host memory, as the second stage made it.
#[derive(Clone, Copy, Debug)]
struct Page {
    /// What it maps: anonymous memory, or a file's page.
    file: Option<(RawFd, u64)>,
    prot: libc::c_int,
    /// The faults userfaultfd reports on it.
    registered: u64,
    write_protected: bool,
}

/// The second stage's host memory, as `mmap`, `mprotect` and userfaultfd change it, and what the
/// window told the stage of each page, by its offset: its protection once the last change made to
/// it returned, and what a change being made gives it.
#[derive(Default)]
struct Model {
    pages: BTreeMap<usize, Page>,
    told: BTreeMap<u64, (Option<Protection>, Option<Option<Protection>>)>,
}

impl Model {
    /// Whether the window lets a store change the page at `offset`: as it was told it does, or
    /// as a change being made to it says.
    fn writable(&self, offset: u64) -> bool {
        let rw = Some(Protection::ReadWrite);
        self.told.get(&offset).is_some_and(|(made, making)| *made == rw || *making == Some(rw))
    }
}

#[derive(Clone, Default)]
struct ModelMemory(Arc<shuttle::sync::Mutex<Model>>);

impl ModelMemory {
    /// Changes each page of the `len` bytes at `addr` with `change`.
    fn pages(&self, addr: usize, len: usize, mut change: impl FnMut(&mut Page)) {
        let mut model = self.0.lock().unwrap();
        for at in (addr..addr + len).step_by(page_size() as usize) {
            change(model.pages.get_mut(&at).unwrap_or_else(|| panic!("no memory at {at:#x}")));
        }
    }
}

impl HostMemory for ModelMemory {
    fn map(&self, addr: usize, len: usize, prot: libc::c_int, memory: Memory) {
        let mut model = self.0.lock().unwrap();
        for at in (0..len).step_by(page_size() as usize) {
            let file = match memory {
                Memory::Anonymous => None,
                Memory::File { fd, offset, shared } => {
                    assert!(shared, "the window's pages are of this process's kernel");
                    Some((fd, offset + at as u64))
                }
            };
            let page = Page { file, prot, registered: 0, write_protected: false };
            model.pages.insert(addr + at, page);
        }
    }

    fn protect(&self, addr: usize, len: usize, prot: libc::c_int) {
        self.pages(addr, len, |page| page.prot = prot);
    }

    fn register(&self, addr: usize, len: usize, mode: u64) {
        self.pages(addr, len, |page| page.registered = mode);
    }

    fn write_protect(&self, addr: usize, len: usize, protect: bool) {
        self.pages(addr, len, |page| {
            assert_ne!(
                page.registered & UFFDIO_REGISTER_MODE_WP,
                0,
                "write-protected unregistered"
            );
            page.write_protected = protect;
        });
    }

    fn populate_write(&self, addr: usize, len: usize) {
        self.pages(addr, len, |page| assert!(page.file.is_some(), "populated anonymous memory"));
    }

    fn wake(&self, _addr: usize, _len: usize) {}
}

/// The stage, telling the model what the window tells it.
struct Telling {
    stage: Arc<UffdSecondStage<ModelMemory>>,
    memory: ModelMemory,
}

impl Telling {
    /// Tells the model of a change to `protection` of the `len` bytes at `offset`, made by `make`.
    fn change(&self, offset: u64, len: u64, protection: Option<Protection>, make: impl FnOnce()) {
        let offsets = (offset..offset + len).step_by(page_size() as usize);
        let mut model = self.memory.0.lock().unwrap();
        for at in offsets.clone() {
            model.told.entry(at).or_default().1 = Some(protection);
        }
        drop(model);
        make();
        let mut model = self.memory.0.lock().unwrap();
        for at in offsets {
            model.told.insert(at, (protection, None));
        }
    }
}

impl SecondStage for Telling {
    fn map(&self, offset: u64, host: *mut u8, phys: PhysLocation, len: u64, prot: Protection) {
        self.change(offset, len, Some(prot), || self.stage.map(offset, host, phys, len, prot));
    }

    fn protect(&self, offset: u64, len: u64, prot: Protection) {
        self.change(offset, len, Some(prot), || self.stage.protect(offset, len, prot));
    }

    fn unmap(&self, offset: u64, len: u64) {
        self.change(offset, len, None, || self.stage.unmap(offset, len));
    }
}

/// A guest whose memory slot is the stage's host memory, which KVM follows: an access the
/// memory's protection refuses fails with `EFAULT`; one userfaultfd reports faults, and the fault
/// thread resolves it.
struct HostGuest {
    memory: ModelMemory,
    stage: Arc<UffdSecondStage<ModelMemory>>,
    window: Arc<Window>,
    base: usize,
}

impl Guest for HostGuest {
    type Where = (RawFd, u64);

    fn access(&self, offset: u64, kind: Access, write: Option<u8>) -> Outcome<'_> {
        let addr = self.base + offset as usize;
        let model = self.memory.0.lock().unwrap();
        let page = model.pages[&addr];
        let needs = match kind {
            Access::Read => libc::PROT_READ,
            Access::Write => libc::PROT_WRITE,
        };
        if page.prot & needs == 0 {
            return Outcome::Failed;
        }
        let fault = match page.file {
            None => {
                let missing = page.registered & UFFDIO_REGISTER_MODE_MISSING != 0;
                assert!(missing, "a page of zeros faulted in at {offset:#x}");
                if kind == Access::Write { UFFD_PAGEFAULT_FLAG_WRITE } else { 0 }
            }
            Some(_)
                if kind == Access::Write
                    && page.registered & UFFDIO_REGISTER_MODE_WP != 0
                    && page.write_protected =>
            {
                UFFD_PAGEFAULT_FLAG_WP | UFFD_PAGEFAULT_FLAG_WRITE
            }
            Some((fd, at)) => {
                if kind == Access::Write {
                    assert!(model.writable(offset), "a store at {offset:#x} went untracked");
                }
                return Outcome::Made(byte(fd, at, write));
            }
        };
        drop(model);
        Outcome::Faulted(Box::pin(async move {
            self.stage.resolve(&*self.window, addr, fault).await;
            Ok(())
        }))
    }

    fn writable(&self) -> Vec<u64> {
        let model = self.memory.0.lock().unwrap();
        let tracked =
            |page: &Page| page.registered & UFFDIO_REGISTER_MODE_WP != 0 && page.write_protected;
        (model.pages.iter())
            .filter(|(_, page)| page.file.is_some() && page.prot & libc::PROT_WRITE != 0)
            .filter(|(_, page)| !tracked(page))
            .map(|(addr, _)| (addr - self.base) as u64)
            .collect()
    }

    fn mapped(&self) -> Vec<(u64, (RawFd, u64))> {
        let model = self.memory.0.lock().unwrap();
        (model.pages.iter())
            .filter_map(|(addr, page)| Some(((addr - self.base) as u64, page.file?)))
            .collect()
    }
}

fn seals_and_frees_through_userfaultfd() {
    let ps = page_size();
    let memory = ModelMemory::default();
    // Where the memory is doesn't matter to the model.
    let base = 64 * ps as usize;
    // SAFETY: The model changes no memory.
    let stage = unsafe {
        UffdSecondStage::with_host(base, 2 * ps, memory.clone(), Memories::InProcess, None)
    }
    .unwrap();
    let telling = Arc::new(Telling { stage: stage.clone(), memory: memory.clone() });
    let (dax, _) = Dax::<Arc<u8>>::new(2 * ps, Some(telling)).unwrap();
    let window = dax.window.clone();
    stores_race_seals_and_frees(dax, Arc::new(HostGuest { memory, stage, window, base }));
}

/// The window maps zeros over a page it mapped read-only, as a free does a sealed extent, while
/// the fault thread refuses a store to the page, as `resolve` does, and a vCPU then loads from it:
/// the window's change unmaps the page here, then maps the zeros. The load finds the page as the
/// window mapped it, or not yet registered, so inaccessible: never memory not registered.
fn refusal_races_changes() {
    let ps = page_size();
    let memory = ModelMemory::default();
    let base = 64 * ps as usize;
    // SAFETY: The model changes no memory.
    let stage =
        unsafe { UffdSecondStage::with_host(base, ps, memory.clone(), Memories::InProcess, None) }
            .unwrap();
    let telling = Arc::new(Telling { stage: stage.clone(), memory: memory.clone() });
    let (mut dax, _) = Dax::<Arc<u8>>::new(ps, Some(telling)).unwrap();
    let window = dax.window.clone();
    let vmo = |value| {
        let vmo = zx::Vmo::create(ps).unwrap();
        vmo.write(&vec![value; ps as usize], 0).unwrap();
        vmo
    };
    let (sealed, zeros) = (vmo(SEALED), vmo(0));
    dax.map(0, Arc::new(0), &sealed, 0, ps, false).unwrap();
    let guest = HostGuest { memory, stage: stage.clone(), window, base };

    let change = thread::spawn(move || dax.map(0, Arc::new(1), &zeros, 0, ps, false).unwrap());
    // A store faulted on the page, which the window refused, as it maps it read-only.
    let seen = stage.seen(base);
    stage.refuse(base, seen);
    // The load is made, or faults, or fails, as the page is.
    drop(guest.access(0, Access::Read, None));
    change.join().unwrap();
}

#[test]
fn refusal_races_changes_by_pct() {
    Runner::new(PctScheduler::new(3, runs(RUNS)), config()).run(refusal_races_changes);
}

const EVENTS: [Event; 6] = [
    Event::ReadOpen,
    Event::ReadSealed,
    Event::ReadZeros,
    Event::Faulted,
    Event::Failed,
    Event::StoreRefused,
];

#[test]
fn stores_race_seals_and_frees_randomly() {
    reaching(RandomScheduler::new(runs(RUNS)), seals_and_frees_through_userfaultfd, &EVENTS);
}

#[test]
fn stores_race_seals_and_frees_by_pct() {
    reaching(PctScheduler::new(3, runs(RUNS)), seals_and_frees_through_userfaultfd, &EVENTS);
}

/// Runs the race with `rules`, expecting a schedule to fail saying `expected`. A rule turned off
/// may open a window of one step, which takes more schedules to find than a test's own.
fn fails_without(rules: Rules, expected: &str) {
    *RULES.lock().unwrap() = rules;
    fails_with(expected, || {
        Runner::new(PctScheduler::new(3, runs(10 * RUNS)), config())
            .run(seals_and_frees_through_userfaultfd);
    });
}

/// A page mapped read-only, mapped writable until it is write-protected: a store meanwhile
/// changes the sealed copy, untracked.
#[test]
fn needs_maps_with_the_window_protection() {
    let rules = Rules { map_with_window_protection: false, ..Rules::ALL };
    fails_without(rules, "went untracked");
}

/// A page mapped read-only, made writable before it is write-protected.
#[test]
fn needs_write_protection_before_writable() {
    let rules = Rules { write_protect_before_writable: false, ..Rules::ALL };
    fails_without(rules, "went untracked");
}

/// Memory unmapped, accessible before it is registered for missing faults: an access meanwhile
/// faults a page of zeros in, rather than to the window.
#[test]
fn needs_registers_before_accessible() {
    let rules = Rules { register_before_accessible: false, ..Rules::ALL };
    fails_without(rules, "a page of zeros faulted in");
}

/// A store the window refuses made read-only whenever its fault is resolved: midway through the
/// window's unmapping of the page, the memory is left accessible before it is registered, and a
/// load faults in a page of zeros. A random schedule of the seals and frees found it (seed
/// 4939094435187741046) once in thousands; the race alone is reached at once.
#[test]
fn needs_refusals_between_changes() {
    *RULES.lock().unwrap() = Rules { refusal_between_changes: false, ..Rules::ALL };
    fails_with("a page of zeros faulted in", || {
        Runner::new(PctScheduler::new(3, runs(10 * RUNS)), config()).run(refusal_races_changes);
    });
}

/// A store the window refuses, left write-protected rather than made read-only: it faults again,
/// for ever.
#[test]
fn needs_refused_stores_read_only() {
    let rules = Rules { refused_store_read_only: false, ..Rules::ALL };
    fails_without(rules, "never completed");
}
