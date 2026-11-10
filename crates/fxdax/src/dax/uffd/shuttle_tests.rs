// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The userfaultfd second stage under Shuttle (TASK-44.3): its real code, and the window's, over a model of the host memory it changes, [`ModelMemory`], which a guest's accesses
//! are made against as KVM makes them. Each of the stage's rules of the order it changes its
//! memory in, turned off, fails.

use super::super::shuttle_tests::{
    ATTEMPTS, Event, Guest, Outcome, RUNS, SEALED, byte, reached, reaching, stores_race_frees,
};
use super::super::tests::TestSource;
use super::super::{Dax, DaxWindow, Source as _, Window};
use super::*;
use crate::pages::PhysLocation;
use crate::shuttle_testing::{config, fails_with, runs};
use shuttle::scheduler::{PctScheduler, RandomScheduler};
use shuttle::{Runner, future, thread};
use std::collections::{BTreeMap, BTreeSet};
use std::os::fd::RawFd;

/// A page of host memory, as the second stage made it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Page {
    /// What it maps: anonymous memory, or a file's page.
    file: Option<(RawFd, u64)>,
    prot: libc::c_int,
    /// The faults userfaultfd reports on it.
    registered: u64,
    write_protected: bool,
}

/// The second stage's host memory, as `mmap`, `mprotect` and userfaultfd change it, and what the
/// window told the stage of each page, by its offset: whether a store may change it, a private
/// page, once the last change made to it returned, and what a change being made gives it.
#[derive(Default)]
struct Model {
    pages: BTreeMap<usize, Page>,
    told: BTreeMap<u64, (bool, Option<bool>)>,
    /// fxdax: how many times memory was mapped (`mmap`): each over memory mapped before unmaps it,
    /// which flushes the host's TLBs and the hypervisor's mappings (TASK-91.16).
    maps: usize,
    /// fxdax: the private memory of a window that copies on write, if the stage made it, and the
    /// offsets of it filled: a page of it unfilled faults as anonymous memory (backlog doc-13).
    private: Option<RawFd>,
    filled: BTreeSet<u64>,
    /// fxdax: the pages of private memory emptied while memory mapped them accessibly, each a TLB
    /// flush of the process's CPUs and the hypervisor's (TASK-276.6).
    punched_mapped: usize,
}

impl Model {
    /// Whether the window lets a store change the page at `offset`, a private page: as it was told
    /// it does, or as a change being made to it says.
    fn writable(&self, offset: u64) -> bool {
        self.told.get(&offset).is_some_and(|(made, making)| *made || *making == Some(true))
    }

    /// fxdax: whether `page` is memory with nothing in it: anonymous, or private memory unfilled.
    fn unfilled(&self, page: &Page) -> bool {
        match page.file {
            None => true,
            Some((fd, at)) => self.private == Some(fd) && !self.filled.contains(&at),
        }
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
        model.maps += 1;
        for at in (0..len).step_by(page_size() as usize) {
            let file = match memory {
                Memory::Anonymous => None,
                Memory::File { fd, offset, shared } => {
                    assert!(shared, "the window's pages are of this process's physical memory");
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

    fn write_protect(&self, addr: usize, len: usize) {
        self.pages(addr, len, |page| {
            assert_ne!(
                page.registered & UFFDIO_REGISTER_MODE_WP,
                0,
                "write-protected unregistered"
            );
            page.write_protected = true;
        });
    }

    fn wake(&self, _addr: usize, _len: usize) {}

    fn private_memory(&self, len: usize) -> std::io::Result<OwnedFd> {
        let fd = Syscalls::private_memory_fd(len)?;
        self.0.lock().unwrap().private = Some(fd.as_raw_fd());
        Ok(fd)
    }

    /// fxdax: a page or a run of them (TASK-164.10), of the private memory alone, through no
    /// mapping (TASK-276.6).
    fn fill(&self, fd: RawFd, offset: u64, bytes: &[u8]) {
        let page_size = page_size() as usize;
        let mut model = self.0.lock().unwrap();
        assert_eq!(model.private, Some(fd), "filled memory other than the private memory");
        for (at, bytes) in (offset..).step_by(page_size).zip(bytes.chunks(page_size)) {
            if !model.filled.insert(at) {
                drop(model);
                panic!("filled private memory at {at:#x} twice");
            }
            Syscalls::write_at(fd, at, bytes);
        }
    }

    fn punch(&self, fd: RawFd, offset: u64, len: usize) {
        let mut model = self.0.lock().unwrap();
        let punched = offset..offset + len as u64;
        model.filled.retain(|at| !punched.contains(at));
        let mapped = (model.pages.values())
            .filter(|page| page.prot != libc::PROT_NONE)
            .filter(|page| {
                page.file.is_some_and(|(at_fd, at)| at_fd == fd && punched.contains(&at))
            })
            .count();
        model.punched_mapped += mapped;
        Syscalls::punch_hole(fd, offset, len);
    }
}

/// The stage, telling the model what the window tells it.
struct Telling {
    stage: Arc<UffdSecondStage<ModelMemory>>,
    memory: ModelMemory,
}

impl Telling {
    /// Tells the model of a change of the `len` bytes at `offset`, made by `make`, which makes
    /// them private pages, a store's to change, if `writable`.
    fn change(&self, offset: u64, len: u64, writable: bool, make: impl FnOnce()) {
        let offsets = (offset..offset + len).step_by(page_size() as usize);
        let mut model = self.memory.0.lock().unwrap();
        for at in offsets.clone() {
            model.told.entry(at).or_default().1 = Some(writable);
        }
        drop(model);
        make();
        let mut model = self.memory.0.lock().unwrap();
        for at in offsets {
            model.told.insert(at, (writable, None));
        }
    }
}

impl SecondStage for Telling {
    fn map(&self, offset: u64, phys: PhysLocation, len: u64) {
        self.change(offset, len, false, || self.stage.map(offset, phys, len));
    }

    fn unmap(&self, offset: u64, len: u64) {
        self.change(offset, len, false, || self.stage.unmap(offset, len));
    }

    fn window_mapped(&self, offset: u64, len: u64) {
        self.stage.window_mapped(offset, len);
    }

    fn make_private(&self, offset: u64, from: &[Option<PhysLocation>]) {
        let len = from.len() as u64 * page_size();
        self.change(offset, len, true, || self.stage.make_private(offset, from));
    }

    /// Every page of the range read-only once it is made, mapped or not: its private pages gone.
    fn remap(&self, offset: u64, len: u64, anew: &[Anew]) {
        self.change(offset, len, false, || self.stage.remap(offset, len, anew));
    }
}

/// A stage operation the window made, held back ([`Remapping`]).
#[derive(Clone, Copy, Debug)]
enum Op {
    Unmap(u64, u64),
    Mapped(u64, u64),
    Map(u64, PhysLocation, u64),
    Private(u64, Option<PhysLocation>),
}

/// The window's changes, held back and made as the extent daemon sends them and its VM process
/// makes them (TASK-91.16): what the window makes in a change ([`SecondStage::begin_change`]) is
/// held back, as the daemon's stage holds back its operations, until the change ends; then each
/// unmap is made with what follows it within what it unmaps, at once ([`SecondStage::remap`]), as
/// the VM process takes it (`wire::Receiver::take_change`). The window makes its changes, and
/// every other call, under its lock: one begun in another's, or anything made from another thread
/// meanwhile, fails (TASK-160.11).
struct Remapping {
    stage: Arc<Telling>,
    held: shuttle::sync::Mutex<Option<Held>>,
}

/// A change being made: the thread making it, and what it made.
struct Held {
    changing: thread::ThreadId,
    ops: Vec<Op>,
}

impl Remapping {
    /// Makes `op`: held back if the window is in a change, which this thread must be making.
    fn make(&self, op: Op) {
        let mut held = self.held.lock().unwrap();
        match held.as_mut() {
            Some(change) => {
                let me = thread::current().id();
                assert_eq!(change.changing, me, "{op:?} made from another thread in a change");
                change.ops.push(op);
            }
            None => self.flush(vec![op]),
        }
    }

    /// Makes what was held back, `ops`, in order, holding the lock, so that what is made after
    /// waits.
    fn flush(&self, ops: Vec<Op>) {
        let ps = page_size();
        let mut ops = ops.into_iter().peekable();
        while let Some(op) = ops.next() {
            match op {
                Op::Unmap(mut start, len) => {
                    let mut end = start + len;
                    while let Some(&Op::Unmap(next, len)) = ops.peek() {
                        if next > end || next + len < start {
                            break;
                        }
                        (start, end) = (start.min(next), end.max(next + len));
                        ops.next();
                    }
                    let within = |offset: u64, len: u64| offset >= start && offset + len <= end;
                    let mut anew = Vec::new();
                    loop {
                        match ops.peek() {
                            Some(&Op::Mapped(offset, len)) if within(offset, len) => {
                                anew.push(Anew::Mapped { offset, len })
                            }
                            Some(&Op::Map(offset, phys, len)) if within(offset, len) => {
                                anew.push(Anew::Map { offset, phys, len })
                            }
                            _ => break,
                        }
                        ops.next();
                    }
                    assert_eq!(start % ps, 0);
                    match anew.is_empty() {
                        true => self.stage.unmap(start, end - start),
                        false => self.stage.remap(start, end - start, &anew),
                    }
                }
                Op::Mapped(offset, len) => self.stage.window_mapped(offset, len),
                Op::Map(offset, phys, len) => self.stage.map(offset, phys, len),
                // fxdax: with the private pages following it on, a run at once, as the VM process
                // takes them (`wire::Receiver::take_privates`, TASK-164.10).
                Op::Private(offset, from) => {
                    let mut run = vec![from];
                    while let Some(&Op::Private(next, from)) = ops.peek() {
                        if next != offset + run.len() as u64 * ps {
                            break;
                        }
                        run.push(from);
                        ops.next();
                    }
                    self.stage.make_private(offset, &run);
                }
            }
        }
    }
}

impl SecondStage for Remapping {
    fn map(&self, offset: u64, phys: PhysLocation, len: u64) {
        self.make(Op::Map(offset, phys, len));
    }

    fn unmap(&self, offset: u64, len: u64) {
        self.make(Op::Unmap(offset, len));
    }

    fn window_mapped(&self, offset: u64, len: u64) {
        self.make(Op::Mapped(offset, len));
    }

    fn make_private(&self, offset: u64, from: &[Option<PhysLocation>]) {
        for (at, &from) in from.iter().enumerate() {
            self.make(Op::Private(offset + at as u64 * page_size(), from));
        }
    }

    fn begin_change(&self) {
        let mut held = self.held.lock().unwrap();
        assert!(held.is_none(), "a change begun in another");
        *held = Some(Held { changing: thread::current().id(), ops: Vec::new() });
    }

    fn end_change(&self) {
        let mut held = self.held.lock().unwrap();
        let change = held.take().expect("a change ended, not begun");
        assert_eq!(change.changing, thread::current().id(), "a change ended by another thread");
        self.flush(change.ops);
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

/// fxdax: the threads' IDs, as userfaultfd reports them, of a vCPU, the guest's own, which tests of
/// windows that copy on write say so of ([`guest_thread`]), and of KVM's asynchronous page fault's
/// worker, not the guest's.
const VCPU: u32 = 1;
const WORKER: u32 = 2;

impl HostGuest {
    /// fxdax: the host memory the guest reaches, the window's: not the stage's other mapping of
    /// its private memory, which it fills them through (backlog doc-13).
    fn window_memory(&self) -> std::ops::Range<usize> {
        self.base..self.base + self.window.size() as usize
    }

    /// fxdax: an access as [`Guest::access`] makes it, by thread `thread`: a vCPU's, or KVM's
    /// worker's for it, which makes a store with no bytes to write (`write` none).
    fn access_by(&self, thread: u32, offset: u64, kind: Access, write: Option<u8>) -> Outcome<'_> {
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
        // The violations the rules turned off make (`fails_with`) panic with the model unlocked:
        // Shuttle's mutex, its holder panicked, fails the other tasks' locks with a panic that
        // can't unwind, which aborts the process (TASK-55).
        let fault = match page.file {
            // fxdax: private memory unfilled too (backlog doc-13).
            _ if model.unfilled(&page) => {
                if page.registered & UFFDIO_REGISTER_MODE_MISSING == 0 {
                    drop(model);
                    panic!("a page of zeros faulted in at {offset:#x}");
                }
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
                if kind == Access::Write && !model.writable(offset) {
                    drop(model);
                    panic!("a store at {offset:#x} went untracked");
                }
                return Outcome::Made(byte(fd, at, write));
            }
            None => unreachable!("anonymous memory is unfilled"),
        };
        drop(model);
        Outcome::Faulted(Box::pin(async move {
            self.stage.resolve(&*self.window, addr, fault, thread).await;
            Ok(())
        }))
    }
}

impl Guest for HostGuest {
    type Where = (RawFd, u64);

    fn access(&self, offset: u64, kind: Access, write: Option<u8>) -> Outcome<'_> {
        self.access_by(VCPU, offset, kind, write)
    }

    fn writable(&self) -> Vec<u64> {
        let model = self.memory.0.lock().unwrap();
        let tracked =
            |page: &Page| page.registered & UFFDIO_REGISTER_MODE_WP != 0 && page.write_protected;
        (model.pages.range(self.window_memory()))
            .filter(|(_, page)| !model.unfilled(page) && page.prot & libc::PROT_WRITE != 0)
            .filter(|(_, page)| !tracked(page))
            .map(|(addr, _)| (addr - self.base) as u64)
            .collect()
    }

    fn mapped(&self) -> Vec<(u64, (RawFd, u64))> {
        let model = self.memory.0.lock().unwrap();
        (model.pages.range(self.window_memory()))
            .filter_map(|(addr, page)| Some(((addr - self.base) as u64, page.file?)))
            .collect()
    }

    fn place(&self, phys: PhysLocation) -> (RawFd, u64) {
        let (fd, _) = Memories::InProcess.fd(phys.arena).expect("the page's arena");
        (fd, phys.offset)
    }
}

/// fxdax: the race of stores and loads with a free ([`stores_race_frees`]) through the stage, over
/// the model of its memory, of a window of two pages that copies on write, one private page at
/// most, which the window's changes reach as the window makes them, or, `at_once`, as the extent
/// daemon's VM process makes them ([`Remapping`], TASK-91.16). Its vCPUs' threads are the guest's
/// own, whose stores to pages not here are stores.
fn frees_through_userfaultfd(at_once: bool) {
    let ps = page_size();
    let memory = ModelMemory::default();
    // Where the memory is doesn't matter to the model.
    let base = 64 * ps as usize;
    // SAFETY: The model changes no memory.
    let stage = unsafe {
        UffdSecondStage::with_host(
            base,
            2 * ps,
            memory.clone(),
            Memories::InProcess,
            None,
            usize::MAX,
        )
    }
    .unwrap();
    stage.copy_on_write().unwrap();
    guest_thread(VCPU);
    let telling = Arc::new(Telling { stage: stage.clone(), memory: memory.clone() });
    let second_stage: Arc<dyn SecondStage> = match at_once {
        true => Arc::new(Remapping { stage: telling, held: Default::default() }),
        false => telling,
    };
    let (dax, _) = super::super::tests::copying(2, 1, Some(second_stage));
    let window = dax.window.clone();
    stores_race_frees(dax, Arc::new(HostGuest { memory, stage, window, base }));
}

fn frees_in_steps() {
    frees_through_userfaultfd(false);
}

fn frees_remapped() {
    frees_through_userfaultfd(true);
}

/// fxdax: a stage over a model of its host memory, and a window of `pages` pages through it,
/// whose changes it makes in steps, or at once ([`Remapping`]).
fn modelled(pages: u64, at_once: bool) -> (ModelMemory, Arc<UffdSecondStage<ModelMemory>>, Dax) {
    let ps = page_size();
    let memory = ModelMemory::default();
    // SAFETY: The model changes no memory.
    let stage = unsafe {
        UffdSecondStage::with_host(
            64 * ps as usize,
            pages * ps,
            memory.clone(),
            Memories::InProcess,
            None,
            usize::MAX,
        )
    }
    .unwrap();
    let telling = Arc::new(Telling { stage: stage.clone(), memory: memory.clone() });
    let second_stage: Arc<dyn SecondStage> = match at_once {
        true => Arc::new(Remapping { stage: telling, held: Default::default() }),
        false => telling,
    };
    let (dax, _) = Dax::new(pages * ps, Some(second_stage)).unwrap();
    (memory, stage, dax)
}

/// fxdax: the window's changes made at once leave the host memory reading as made in steps
/// (TASK-91.16), with fewer `mmap`s: none for a map of the very pages installed, left in place;
/// one for a map of other memory over pages installed, left out, and named; and none for a change
/// of pages never touched since they were named (TASK-276.6: a map names pages, their first touch
/// installs them). A page a refused store restricted is taken away with its run, which lifts it.
fn remaps_end_as_steps_do() {
    let ps = page_size();
    let (steps, steps_stage, mut steps_dax) = modelled(2, false);
    let (at_once, at_once_stage, mut at_once_dax) = modelled(2, true);
    let (sealed, other) = (TestSource::filled(2, 1).source(), TestSource::filled(2, 2).source());
    // Not yet written, it has no page in memory.
    let unwritten = TestSource::paged(2).source();
    let maps = |memory: &ModelMemory| memory.0.lock().unwrap().maps;
    let guests = [(&steps, &steps_stage, &steps_dax), (&at_once, &at_once_stage, &at_once_dax)]
        .map(|(memory, stage, dax)| HostGuest {
            memory: memory.clone(),
            stage: stage.clone(),
            window: dax.window.clone(),
            base: 64 * ps as usize,
        });
    // What each guest reads of each page, its loads installing them.
    let reads = || guests.each_ref().map(|guest| [0, ps].map(|offset| load_byte(guest, offset)));
    let mut change = |what: &str, mmaps: usize, make: &dyn Fn(&mut Dax)| {
        let before = maps(&at_once);
        make(&mut steps_dax);
        make(&mut at_once_dax);
        assert_eq!(maps(&at_once) - before, mmaps, "{what}: mmaps");
    };
    change("sealed", 0, &|dax| dax.map(0, &sealed, 0, 2 * ps));
    assert_eq!(reads(), [[1, 1]; 2], "sealed");
    change("the same pages", 0, &|dax| dax.map(0, &sealed, 0, 2 * ps));
    assert_eq!(reads(), [[1, 1]; 2], "the same pages");
    // A store faulted on the second page, which the window refused, as it doesn't copy on write.
    for stage in [&steps_stage, &at_once_stage] {
        let addr = 64 * ps as usize + ps as usize;
        stage.restrict(addr, stage.seen(addr), libc::PROT_READ);
    }
    // The run, its second page restricted, left out.
    change("the same, one restricted", 1, &|dax| dax.map(0, &sealed, 0, 2 * ps));
    assert_eq!(reads(), [[1, 1]; 2], "the same, one restricted");
    change("other memory", 1, &|dax| dax.map(0, &other, 0, 2 * ps));
    assert_eq!(reads(), [[2, 2]; 2], "other memory");
    change("nothing in memory", 1, &|dax| dax.map(0, &unwritten, 0, 2 * ps));
    change("nothing in memory, never touched", 0, &|dax| dax.map(0, &other, 0, 2 * ps));
}

/// fxdax: a vCPU's store of [`STORED`] at `offset` through `guest`, retried as each fault is
/// resolved.
fn store_byte(guest: &HostGuest, offset: u64) {
    for _ in 0..ATTEMPTS {
        match guest.access(offset, Access::Write, Some(STORED)) {
            Outcome::Made(read) => return assert_eq!(read, STORED),
            Outcome::Faulted(fault) => future::block_on(fault).unwrap(),
            Outcome::Failed => panic!("the vCPU's store at {offset:#x} failed"),
        }
    }
    panic!("the vCPU's store at {offset:#x} never completed");
}

/// fxdax: what a vCPU's load at `offset` through `guest` reads, retried as each fault is resolved.
fn load_byte(guest: &HostGuest, offset: u64) -> u8 {
    for _ in 0..ATTEMPTS {
        match guest.access(offset, Access::Read, None) {
            Outcome::Made(read) => return read,
            Outcome::Faulted(fault) => future::block_on(fault).unwrap(),
            Outcome::Failed => panic!("the vCPU's load at {offset:#x} failed"),
        }
    }
    panic!("the vCPU's load at {offset:#x} never completed");
}

#[test]
fn remaps_end_as_steps_do_once() {
    Runner::new(RandomScheduler::new(1), config()).run(remaps_end_as_steps_do);
}

/// fxdax: the events the race reaches with the window's changes made at once: all but an access
/// failing for a moment, which the schedules reach no more (TASK-91.16). Made in steps, a free
/// leaves the pages out, inaccessible until registered, which an access finds; made at once, its
/// zeros, installed over the extent, refuse a store only until they are registered, a step no
/// schedule found in thousands.
const REMAPPED_EVENTS: [Event; 5] =
    [Event::ReadSealed, Event::ReadZeros, Event::ReadStored, Event::Faulted, Event::StoreRefused];

#[test]
fn remapped_stores_race_frees_randomly() {
    reaching(|| RandomScheduler::new(runs(RUNS)), frees_remapped, &REMAPPED_EVENTS);
}

#[test]
fn remapped_stores_race_frees_by_pct() {
    reaching(|| PctScheduler::new(3, runs(RUNS)), frees_remapped, &REMAPPED_EVENTS);
}

/// The window maps zeros over a page it mapped, as a free does a sealed extent, while the fault
/// thread refuses a store to the page, as `resolve` does of a window that doesn't copy on write,
/// and a vCPU then loads from it:
/// the window's change unmaps the page here, then maps the zeros. The load finds the page as the
/// window mapped it, or not yet registered, so inaccessible: never memory not registered.
fn refusal_races_changes() {
    let ps = page_size();
    let memory = ModelMemory::default();
    let base = 64 * ps as usize;
    // SAFETY: The model changes no memory.
    let stage = unsafe {
        UffdSecondStage::with_host(base, ps, memory.clone(), Memories::InProcess, None, usize::MAX)
    }
    .unwrap();
    let telling = Arc::new(Telling { stage: stage.clone(), memory: memory.clone() });
    let (mut dax, _) = Dax::new(ps, Some(telling)).unwrap();
    let window = dax.window.clone();
    let sealed = TestSource::filled(1, SEALED).source();
    dax.map(0, &sealed, 0, ps);
    let guest = HostGuest { memory, stage: stage.clone(), window, base };
    // Installed, as a load first touches it (TASK-276.6), for the change to take away.
    assert_eq!(load_byte(&guest, 0), SEALED);

    let change = thread::spawn(move || dax.map_zeros(0, ps));
    // A store faulted on the page, which the window refused, as it maps it read-only.
    let seen = stage.seen(base);
    stage.restrict(base, seen, libc::PROT_READ);
    // The load is made, or faults, or fails, as the page is.
    drop(guest.access(0, Access::Read, None));
    change.join().unwrap();
}

#[test]
fn refusal_races_changes_by_pct() {
    Runner::new(PctScheduler::new(3, runs(RUNS)), config()).run(refusal_races_changes);
}

/// The window maps a page while a fault on it finds nothing mapped, as KVM's may once the guest
/// has unmapped the page, and withholds the page, as `resolve` does. The page mapped has nothing in
/// memory yet, as a file's page before the pager supplies it, which the kernel maps here only as a
/// fault brings it in: a load once the map is done reads the page, which the fault brought in if
/// it found the page mapped, or faults, to the window; it never finds the page withheld.
fn withholding_races_maps() {
    let ps = page_size();
    let memory = ModelMemory::default();
    let base = 64 * ps as usize;
    // SAFETY: The model changes no memory.
    let stage = unsafe {
        UffdSecondStage::with_host(base, ps, memory.clone(), Memories::InProcess, None, usize::MAX)
    }
    .unwrap();
    let telling = Arc::new(Telling { stage: stage.clone(), memory: memory.clone() });
    let (mut dax, _) = Dax::new(ps, Some(telling)).unwrap();
    let window = dax.window.clone();
    // Not yet written, it has no page in memory, and is zeros as it is brought in.
    let file = TestSource::anonymous(1).source();
    let guest = HostGuest { memory, stage: stage.clone(), window: window.clone(), base };

    let map = thread::spawn(move || {
        dax.map(0, &file, 0, ps);
        dax
    });
    // A load's fault, as KVM makes for the guest.
    future::block_on(stage.resolve(&*window, base, 0, 0));
    let _dax = map.join().unwrap();
    match guest.access(0, Access::Read, None) {
        Outcome::Made(read) => assert_eq!(read, 0, "the page mapped isn't the file's zeros"),
        Outcome::Faulted(_) => {}
        Outcome::Failed => panic!("a page mapped was withheld"),
    }
}

#[test]
fn withholding_races_maps_by_pct() {
    Runner::new(PctScheduler::new(3, runs(RUNS)), config()).run(withholding_races_maps);
}

const EVENTS: [Event; 6] = [
    Event::ReadSealed,
    Event::ReadZeros,
    Event::ReadStored,
    Event::Faulted,
    Event::Failed,
    Event::StoreRefused,
];

#[test]
fn stores_race_frees_randomly() {
    reaching(|| RandomScheduler::new(runs(RUNS)), frees_in_steps, &EVENTS);
}

#[test]
fn stores_race_frees_by_pct() {
    reaching(|| PctScheduler::new(3, runs(RUNS)), frees_in_steps, &EVENTS);
}

/// Runs the race with `rules`, expecting a schedule to fail saying `expected`. A rule turned off
/// may open a window of one step, which takes more schedules to find than a test's own.
fn fails_without(rules: Rules, expected: &str) {
    *RULES.lock().unwrap() = rules;
    fails_with(expected, || {
        Runner::new(PctScheduler::new(3, runs(10 * RUNS)), config()).run(frees_in_steps);
    });
}

/// A page mapped writable until it is write-protected: a store meanwhile changes the extent's
/// page, untracked.
#[test]
fn needs_maps_with_the_window_protection() {
    let rules = Rules { map_with_window_protection: false, ..Rules::ALL };
    fails_without(rules, "went untracked");
}

/// A page mapped, made writable before it is write-protected.
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
/// 4939094435187741046) once in thousands, before copy on write; the race alone is reached at
/// once.
#[test]
fn needs_refusals_between_changes() {
    *RULES.lock().unwrap() = Rules { restrict_between_changes: false, ..Rules::ALL };
    fails_with("a page of zeros faulted in", || {
        Runner::new(PctScheduler::new(3, runs(10 * RUNS)), config()).run(refusal_races_changes);
    });
}

/// A page the window maps nothing at withheld whenever its fault is resolved: once the window has
/// mapped it, it is left inaccessible, and the guest's accesses fail for ever.
#[test]
fn needs_withholding_between_changes() {
    *RULES.lock().unwrap() = Rules { restrict_between_changes: false, ..Rules::ALL };
    fails_with("a page mapped was withheld", || {
        Runner::new(PctScheduler::new(3, runs(10 * RUNS)), config()).run(withholding_races_maps);
    });
}

/// A page withheld, left inaccessible when the window maps it anew: no fault maps it here, and
/// the guest's accesses fail for ever.
#[test]
fn needs_mapping_to_give_back_withheld() {
    *RULES.lock().unwrap() = Rules { mapping_gives_back_withheld: false, ..Rules::ALL };
    fails_with("a page mapped was withheld", || {
        Runner::new(PctScheduler::new(3, runs(10 * RUNS)), config()).run(withholding_races_maps);
    });
}

/// A store the window refuses, left write-protected rather than made read-only: it faults again,
/// for ever.
#[test]
fn needs_refused_stores_read_only() {
    let rules = Rules { refused_store_read_only: false, ..Rules::ALL };
    fails_without(rules, "never completed");
}

/// fxdax: how a page isn't here as KVM's worker writes it ([`worker_writes_copy_nothing`]).
#[derive(Clone, Copy, PartialEq, Eq)]
enum NotHere {
    /// Never faulted in.
    Never,
    /// Faulted in, then left out past the budget, which a fault installs again as it was, without
    /// the window ([`UffdSecondStage::reinstall`]).
    LeftOut,
}

/// KVM's worker's write of page 0, retried as get_user_pages retries it, until it fails or is
/// made: it must end, or the worker faults for good. fxdax: on a window that copies on write it
/// fails, its page restricted to reads, since taken as a load, never a store (TASK-191).
fn worker_writes(guest: &HostGuest) {
    worker_writes_at(guest, 0);
}

/// KVM's worker's write at `offset`, as [`worker_writes`] makes it.
fn worker_writes_at(guest: &HostGuest, offset: u64) {
    for _ in 0..ATTEMPTS {
        match guest.access_by(WORKER, offset, Access::Write, None) {
            Outcome::Faulted(fault) => future::block_on(fault).unwrap(),
            Outcome::Failed | Outcome::Made(_) => return,
        }
    }
    panic!("the worker's write never ended");
}

/// The vCPU's load at `offset`, retried as KVM retries it, until it is made.
fn load(guest: &HostGuest, offset: u64) {
    for _ in 0..ATTEMPTS {
        match guest.access(offset, Access::Read, None) {
            Outcome::Made(_) => return,
            Outcome::Faulted(fault) => future::block_on(fault).unwrap(),
            Outcome::Failed => panic!("the vCPU's load at {offset:#x} failed"),
        }
    }
    panic!("the vCPU's load at {offset:#x} never completed");
}

/// fxdax TASK-164.1 (backlog doc-13): on a window that copies on write, KVM's worker's write of a
/// page not here, as it makes for a vCPU's load and store alike, copies nothing, whether the page
/// was never here or was left out (`not_here`): resolved as a load, the page made read-only here,
/// the write, retried, fails. The vCPU's own access decides: a load is made, of the extent's page;
/// a store fails, as `KVM_RUN` does with `EFAULT`, until the VMM, retrying it, says so
/// (`guest_access_failed`), which lifts the restriction, and the store, retried, faults as one, and
/// is made, in a private copy. Red, left out, with the page installed again left writable here:
/// the worker's write, retried, faulted as a store, and copied the page for a load, uncounted by
/// the guest (GCE r582's jailed VM, its budget the default's, 21,396 pages so, TASK-170).
fn worker_writes_copy_nothing(vcpu_stores: bool, not_here: NotHere) {
    let ps = page_size();
    let memory = ModelMemory::default();
    let base = 64 * ps as usize;
    // Three pages, the first and the last runs of their own, of which one alone stays installed
    // if left out at all.
    let (pages, budget) = match not_here {
        NotHere::Never => (1, usize::MAX),
        NotHere::LeftOut => (3, 1),
    };
    // SAFETY: The model changes no memory.
    let stage = unsafe {
        UffdSecondStage::with_host(
            base,
            pages * ps,
            memory.clone(),
            Memories::InProcess,
            None,
            budget,
        )
    }
    .unwrap();
    stage.copy_on_write().unwrap();
    guest_thread(VCPU);
    let telling = Arc::new(Telling { stage: stage.clone(), memory: memory.clone() });
    let (mut dax, _) = super::super::tests::copying(pages, 1, Some(telling));
    let window = dax.window.clone();
    // Not in memory as it is mapped, and zeros as it is brought in: not here until a fault.
    let file = TestSource::anonymous(pages).source();
    dax.map(0, &file, 0, pages * ps);
    let guest = HostGuest { memory, stage: stage.clone(), window: window.clone(), base };
    if not_here == NotHere::LeftOut {
        load(&guest, 0);
        load(&guest, 2 * ps);
        let mapped = stage.pages.lock().unwrap().pages[0].mapped;
        assert!(mapped.is_some_and(|mapped| !mapped.installed()), "the first page wasn't left out");
    }

    // The worker's write, retried as get_user_pages retries it, until it fails or is made.
    worker_writes(&guest);
    assert_eq!(window.private_pages(), 0, "a worker's write copied the page");

    let (kind, write) = if vcpu_stores { (Access::Write, Some(7)) } else { (Access::Read, None) };
    let mut failed = false;
    let read = 'access: {
        for _ in 0..ATTEMPTS {
            match guest.access(0, kind, write) {
                Outcome::Made(read) => break 'access read,
                Outcome::Faulted(fault) => future::block_on(fault).unwrap(),
                // As fxdax's Firecracker says a vCPU's KVM_RUN failing, and retries it.
                Outcome::Failed => {
                    failed = true;
                    stage.lift_worker_restrictions();
                }
            }
        }
        panic!("the vCPU's {kind:?} never completed");
    };
    match vcpu_stores {
        true => {
            assert_eq!((read, failed, window.private_pages()), (7, true, 1));
            assert_eq!(guest.writable(), [0], "the private page isn't writable here");
        }
        false => assert_eq!((read, failed, window.private_pages()), (0, false, 0)),
    }
}

#[test]
fn worker_writes_before_loads_copy_nothing() {
    Runner::new(RandomScheduler::new(1), config())
        .run(|| worker_writes_copy_nothing(false, NotHere::Never));
}

#[test]
fn worker_writes_before_stores_copy_nothing() {
    Runner::new(RandomScheduler::new(1), config())
        .run(|| worker_writes_copy_nothing(true, NotHere::Never));
}

#[test]
fn worker_writes_of_pages_left_out_before_loads_copy_nothing() {
    Runner::new(RandomScheduler::new(1), config())
        .run(|| worker_writes_copy_nothing(false, NotHere::LeftOut));
}

#[test]
fn worker_writes_of_pages_left_out_before_stores_copy_nothing() {
    Runner::new(RandomScheduler::new(1), config())
        .run(|| worker_writes_copy_nothing(true, NotHere::LeftOut));
}

/// fxdax: a worker's write of a page `not_here`, resolved as a load, or installed again, left out,
/// leaving the page writable here: the write, retried, faults again, as a load, for good. It
/// faulted as a store and copied the page for a load, before such writes were loads alone
/// (TASK-191).
fn needs_worker_writes_restricted(not_here: NotHere) {
    *RULES.lock().unwrap() = Rules { worker_writes_restricted: false, ..Rules::ALL };
    fails_with("the worker's write never ended", || {
        Runner::new(RandomScheduler::new(1), config())
            .run(move || worker_writes_copy_nothing(false, not_here));
    });
}

#[test]
fn needs_worker_writes_restricted_of_pages_never_here() {
    needs_worker_writes_restricted(NotHere::Never);
}

#[test]
fn needs_worker_writes_restricted_of_pages_left_out() {
    needs_worker_writes_restricted(NotHere::LeftOut);
}

/// fxdax: a page `not_here` a worker's write restricted, left so as the guest's store fails: the
/// store fails for ever.
fn needs_failures_to_lift_worker_restrictions(not_here: NotHere) {
    *RULES.lock().unwrap() = Rules { failures_lift_worker_restrictions: false, ..Rules::ALL };
    fails_with("never completed", || {
        Runner::new(RandomScheduler::new(1), config())
            .run(move || worker_writes_copy_nothing(true, not_here));
    });
}

#[test]
fn needs_failures_to_lift_worker_restrictions_of_pages_never_here() {
    needs_failures_to_lift_worker_restrictions(NotHere::Never);
}

#[test]
fn needs_failures_to_lift_worker_restrictions_of_pages_left_out() {
    needs_failures_to_lift_worker_restrictions(NotHere::LeftOut);
}

/// fxdax TASK-164.1: on a window that copies on write, a vCPU's store to a page here, mapped
/// read-only, write-protected, faults as a store, reported by its own thread: the page is copied,
/// mapped writable here in its place, and holds the store; the extent's page is as it was. KVM's
/// worker's write faults so too, for a load as for a store, and copies nothing
/// ([`worker_writes_of_pages_here_copy_nothing`], TASK-191).
fn stores_to_pages_here_copy_them() {
    let ps = page_size();
    let memory = ModelMemory::default();
    let base = 64 * ps as usize;
    // SAFETY: The model changes no memory.
    let stage = unsafe {
        UffdSecondStage::with_host(base, ps, memory.clone(), Memories::InProcess, None, usize::MAX)
    }
    .unwrap();
    stage.copy_on_write().unwrap();
    let telling = Arc::new(Telling { stage: stage.clone(), memory: memory.clone() });
    let (mut dax, _) = super::super::tests::copying(1, 1, Some(telling));
    let window = dax.window.clone();
    let sealed = TestSource::filled(1, SEALED);
    dax.map(0, &sealed.source(), 0, ps);
    let guest = HostGuest { memory, stage, window: window.clone(), base };
    let mut shared = None;
    sealed.each_present(0..1, &mut |_, frame| shared = Some(frame.location()));
    let shared = shared.unwrap();
    // Installed here, as a load first touches it (TASK-276.6).
    assert_eq!(load_byte(&guest, 0), SEALED);

    guest_thread(VCPU);
    match guest.access_by(VCPU, 0, Access::Write, Some(7)) {
        Outcome::Faulted(fault) => future::block_on(fault).unwrap(),
        _ => panic!("a store to a page mapped read-only was made, or failed"),
    }
    assert_eq!(window.private_pages(), 1);
    match guest.access(0, Access::Write, Some(7)) {
        Outcome::Made(read) => assert_eq!(read, 7),
        _ => panic!("a store to a private page faulted, or failed"),
    }
    let arena = crate::pages::arena_memory(shared.arena).unwrap();
    use std::os::fd::AsRawFd as _;
    assert_eq!(
        byte(arena.as_raw_fd(), shared.offset, None),
        SEALED,
        "the store reached the extent"
    );
}

#[test]
fn stores_to_pages_here_copy_them_once() {
    Runner::new(RandomScheduler::new(1), config()).run(stores_to_pages_here_copy_them);
}

/// fxdax TASK-191: on a window that copies on write, KVM's worker's write of a page here, mapped
/// read-only and write-protected, copies nothing: the worker writes for a vCPU's load as for its
/// store (`async_pf_execute`'s `FOLL_WRITE`), and a page the vCPU's load found not here may be
/// installed before the worker runs, by another fault, the window's probe of the page after a
/// worker's write, or the guest's `MAP` of a page in memory. A guest makes a page private before
/// it stores to it (doc-13, TASK-164.9), so only a load gets here so: resolved as a load, the page
/// restricted to reads, the worker's write, retried, fails. The vCPU's load is made, of the
/// extent's page; its store, which a guest would have made private first, fails until a failure
/// lifts the restriction, and is a store then, made in a private copy. Red with the worker's
/// write taken as a store: a private page the guest never counted, which kept its frozen VM's
/// snapshot refused (GCE r685, r686, r688: 'the window has 2 private pages').
fn worker_writes_of_pages_here_copy_nothing(vcpu_stores: bool) {
    let ps = page_size();
    let memory = ModelMemory::default();
    let base = 64 * ps as usize;
    // SAFETY: The model changes no memory.
    let stage = unsafe {
        UffdSecondStage::with_host(base, ps, memory.clone(), Memories::InProcess, None, usize::MAX)
    }
    .unwrap();
    stage.copy_on_write().unwrap();
    guest_thread(VCPU);
    let telling = Arc::new(Telling { stage: stage.clone(), memory: memory.clone() });
    let (mut dax, _) = super::super::tests::copying(1, 1, Some(telling));
    let window = dax.window.clone();
    // In memory as it is mapped: installed here as a vCPU's load first touches it,
    // write-protected (TASK-276.6).
    let sealed = TestSource::filled(1, SEALED);
    dax.map(0, &sealed.source(), 0, ps);
    let guest = HostGuest { memory, stage: stage.clone(), window: window.clone(), base };
    assert_eq!(load_byte(&guest, 0), SEALED);
    assert!(stage.pages.lock().unwrap().pages[0].mapped.is_some_and(|mapped| mapped.installed()));

    // The worker's write, retried as get_user_pages retries it, until it fails or is made.
    worker_writes(&guest);
    assert_eq!(window.private_pages(), 0, "a worker's write copied the page");

    let (kind, write) = if vcpu_stores { (Access::Write, Some(7)) } else { (Access::Read, None) };
    let mut failed = false;
    let read = 'access: {
        for _ in 0..ATTEMPTS {
            match guest.access(0, kind, write) {
                Outcome::Made(read) => break 'access read,
                Outcome::Faulted(fault) => future::block_on(fault).unwrap(),
                Outcome::Failed => {
                    failed = true;
                    stage.lift_worker_restrictions();
                }
            }
        }
        panic!("the vCPU's {kind:?} never completed");
    };
    match vcpu_stores {
        true => assert_eq!((read, failed, window.private_pages()), (7, true, 1)),
        false => assert_eq!((read, failed, window.private_pages()), (SEALED, false, 0)),
    }
}

#[test]
fn worker_writes_of_pages_here_before_loads_copy_nothing() {
    Runner::new(RandomScheduler::new(1), config())
        .run(|| worker_writes_of_pages_here_copy_nothing(false));
}

#[test]
fn worker_writes_of_pages_here_before_stores_copy_nothing() {
    Runner::new(RandomScheduler::new(1), config())
        .run(|| worker_writes_of_pages_here_copy_nothing(true));
}

/// fxdax: a stage over a model of its host memory, of a window that copies on write, of `pages`
/// pages, holding as many private pages, and the window through it, which tells the model of its
/// changes in steps.
fn modelled_copying(pages: u64) -> (ModelMemory, Arc<UffdSecondStage<ModelMemory>>, Dax) {
    let ps = page_size();
    let memory = ModelMemory::default();
    // SAFETY: The model changes no memory.
    let stage = unsafe {
        UffdSecondStage::with_host(
            64 * ps as usize,
            pages * ps,
            memory.clone(),
            Memories::InProcess,
            None,
            usize::MAX,
        )
    }
    .unwrap();
    stage.copy_on_write().unwrap();
    let telling = Arc::new(Telling { stage: stage.clone(), memory: memory.clone() });
    let (dax, _) = super::super::tests::copying(pages, pages, Some(telling));
    (memory, stage, dax)
}

/// fxdax TASK-164.1 (backlog doc-13): a vCPU loads from a page that a window copying on write
/// maps read-only, a sealed extent's, while the window makes it private, as `PRIVATE` does: each
/// load reads the extent's bytes, from its page or its copy. The private memory mapped in place of
/// the page is filled first: mapped unfilled, before it is registered, a load meanwhile faults a
/// page of zeros in, as the kernel would.
fn private_pages_race_loads() {
    let ps = page_size();
    let (memory, stage, mut dax) = modelled_copying(1);
    let sealed = TestSource::filled(1, SEALED).source();
    dax.map(0, &sealed, 0, ps);
    let window = dax.window.clone();
    let guest = HostGuest { memory, stage, window: window.clone(), base: 64 * ps as usize };
    let guest = Arc::new(guest);
    let loading = guest.clone();
    let loads = thread::spawn(move || {
        for _ in 0..2 {
            let read = 'load: {
                for _ in 0..ATTEMPTS {
                    match loading.access(0, Access::Read, None) {
                        Outcome::Made(read) => break 'load read,
                        Outcome::Faulted(fault) => future::block_on(fault).unwrap(),
                        Outcome::Failed => thread::yield_now(),
                    }
                }
                panic!("a load never completed");
            };
            assert_eq!(read, SEALED, "a load read other bytes");
            thread::yield_now();
        }
    });
    future::block_on(window.make_private(0, ps)).unwrap();
    loads.join().unwrap();
    assert_eq!(window.private_pages(), 1);
    assert_eq!(guest.writable(), [0], "the private page isn't writable here");
}

#[test]
fn private_pages_race_loads_by_pct() {
    Runner::new(PctScheduler::new(3, runs(RUNS)), config()).run(private_pages_race_loads);
}

/// fxdax: a private page's memory mapped in place of the page before it is filled: a load
/// meanwhile finds it unfilled, and not yet registered.
#[test]
fn needs_private_pages_filled_before_mapped() {
    *RULES.lock().unwrap() = Rules { private_filled_before_mapped: false, ..Rules::ALL };
    fails_with("a page of zeros faulted in", || {
        Runner::new(PctScheduler::new(3, runs(10 * RUNS)), config()).run(private_pages_race_loads);
    });
}

/// fxdax TASK-164.10: a `PRIVATE`'s pages are made at once: a run of pages installed, as the
/// window installs a mapping's pages in memory as it maps them, maps its private memory over the
/// run in one `mmap`, where each page took one; a run none of whose pages is installed, a
/// mapping's whose pages came into memory only for the `PRIVATE`, maps nothing, its memory the
/// private memory already, which filling it makes the pages'. Each page reads what it copied, and
/// takes a store without a fault. Red with a map a page (four `mmap`s, then four more).
fn private_runs_map_at_once() {
    let ps = page_size();
    let (memory, stage, mut dax) = modelled_copying(8);
    let (installed, not_in_memory) =
        (TestSource::filled(4, SEALED).source(), TestSource::anonymous(4).source());
    dax.map(0, &installed, 0, 4 * ps);
    dax.map(4 * ps, &not_in_memory, 0, 4 * ps);
    let window = dax.window.clone();
    let base = 64 * ps as usize;
    let guest = HostGuest { memory: memory.clone(), stage, window: window.clone(), base };
    // The first run installed, as the guest's loads first touch it (TASK-276.6).
    assert_eq!((0..4).map(|page| load_byte(&guest, page * ps)).collect::<Vec<_>>(), [SEALED; 4]);
    let maps = || memory.0.lock().unwrap().maps;
    let before = maps();
    future::block_on(window.make_private(0, 4 * ps)).unwrap();
    assert_eq!(maps() - before, 1, "a run of pages installed");
    future::block_on(window.make_private(4 * ps, 4 * ps)).unwrap();
    assert_eq!(maps() - before, 1, "a run with none installed");
    assert_eq!(window.private_pages(), 8);
    assert_eq!(guest.writable(), (0..8).map(|page| page * ps).collect::<Vec<_>>());
    for page in 0..8 {
        let held = if page < 4 { SEALED } else { 0 };
        match guest.access(page * ps, Access::Read, None) {
            Outcome::Made(read) => assert_eq!(read, held, "page {page}"),
            _ => panic!("a load of private page {page} faulted, or failed"),
        }
        match guest.access(page * ps, Access::Write, Some(STORED)) {
            Outcome::Made(read) => assert_eq!(read, STORED, "page {page}"),
            _ => panic!("a store to private page {page} faulted, or failed"),
        }
    }
}

#[test]
fn private_runs_map_at_once_once() {
    Runner::new(RandomScheduler::new(1), config()).run(private_runs_map_at_once);
}

/// fxdax TASK-164.10: a vCPU loads from the pages of a run that a window copying on write makes
/// private at once, as `PRIVATE` does, through the VM process's taking of the run together
/// ([`Remapping`]): each load reads the extent's bytes, from its page or its copy, whether the
/// run's pages were installed, a load having faulted one in, or not; and every page of the run
/// is writable here at the end.
fn private_runs_race_loads() {
    let ps = page_size();
    let memory = ModelMemory::default();
    let base = 64 * ps as usize;
    // SAFETY: The model changes no memory.
    let stage = unsafe {
        UffdSecondStage::with_host(
            base,
            2 * ps,
            memory.clone(),
            Memories::InProcess,
            None,
            usize::MAX,
        )
    }
    .unwrap();
    stage.copy_on_write().unwrap();
    let telling = Arc::new(Telling { stage: stage.clone(), memory: memory.clone() });
    let remapping = Arc::new(Remapping { stage: telling, held: Default::default() });
    let (mut dax, _) = super::super::tests::copying(2, 2, Some(remapping));
    let sealed = TestSource::filled(2, SEALED).source();
    dax.map(0, &sealed, 0, 2 * ps);
    let window = dax.window.clone();
    let guest = Arc::new(HostGuest { memory, stage, window: window.clone(), base });
    let loading = guest.clone();
    let loads = thread::spawn(move || {
        for offset in [ps, 0, ps] {
            let read = 'load: {
                for _ in 0..ATTEMPTS {
                    match loading.access(offset, Access::Read, None) {
                        Outcome::Made(read) => break 'load read,
                        Outcome::Faulted(fault) => future::block_on(fault).unwrap(),
                        Outcome::Failed => thread::yield_now(),
                    }
                }
                panic!("a load never completed");
            };
            assert_eq!(read, SEALED, "a load read other bytes");
            thread::yield_now();
        }
    });
    future::block_on(window.make_private(0, 2 * ps)).unwrap();
    loads.join().unwrap();
    assert_eq!(window.private_pages(), 2);
    assert_eq!(guest.writable(), [0, ps], "the private pages aren't writable here");
}

#[test]
fn private_runs_race_loads_by_pct() {
    Runner::new(PctScheduler::new(3, runs(RUNS)), config()).run(private_runs_race_loads);
}

#[test]
fn private_runs_race_loads_randomly() {
    Runner::new(RandomScheduler::new(runs(RUNS)), config()).run(private_runs_race_loads);
}

/// fxdax: a run's private memory mapped in place of its pages before it is filled: a load
/// meanwhile finds it unfilled, and not yet registered.
#[test]
fn needs_private_runs_filled_before_mapped() {
    *RULES.lock().unwrap() = Rules { private_filled_before_mapped: false, ..Rules::ALL };
    fails_with("a page of zeros faulted in", || {
        Runner::new(PctScheduler::new(3, runs(10 * RUNS)), config()).run(private_runs_race_loads);
    });
}

/// fxdax: a guest's store to a page not here, on a window that copies on write, installs nothing
/// after it: the page is made private, a fault either way, and the pages after it stay as they
/// were, not here. The probe of the next page, which once came of every write to a page not here,
/// installed it read-only for a moment, once the faulting thread was woken to store on to it,
/// which a thread of the host's died of (SIGSEGV, 3 runs in 500 of
/// snapshots_after_private_pages_are_rebound_restore_their_bytes). Red with the probe back: page 1
/// installed. A worker's write still probes, for a run of loads.
fn stores_to_pages_not_here_install_nothing_after_them() {
    let ps = page_size();
    let (memory, stage, mut dax) = modelled_copying(4);
    guest_thread(VCPU);
    // Not in memory as it is mapped: not here until a fault.
    let file = TestSource::anonymous(4).source();
    dax.map(0, &file, 0, 4 * ps);
    let window = dax.window.clone();
    let base = 64 * ps as usize;
    let guest = HostGuest { memory, stage: stage.clone(), window: window.clone(), base };
    let installed = |page: usize| {
        let pages = stage.pages.lock().unwrap();
        pages.pages[page].mapped.is_some_and(|mapped| mapped.installed())
    };
    match guest.access(0, Access::Write, Some(STORED)) {
        Outcome::Faulted(fault) => future::block_on(fault).unwrap(),
        _ => panic!("a store to a page not here was made, or failed"),
    }
    assert_eq!(window.private_pages(), 1);
    assert!(!installed(1), "the store's fault installed the page after it");
    match guest.access(0, Access::Write, Some(STORED)) {
        Outcome::Made(read) => assert_eq!(read, STORED),
        _ => panic!("a store to a private page faulted, or failed"),
    }
    // A worker's write, a load's, of page 2, maps page 3 ahead.
    worker_writes_at(&guest, 2 * ps);
    assert!(installed(3), "a worker's write mapped nothing ahead");
}

#[test]
fn stores_to_pages_not_here_install_nothing_after_them_once() {
    Runner::new(RandomScheduler::new(1), config())
        .run(stores_to_pages_not_here_install_nothing_after_them);
}

/// fxdax: what a vCPU stores to a private page in [`private_pages_go_with_their_unmaps`].
const STORED: u8 = 0x5a;

/// fxdax TASK-164.1 (backlog doc-13): the guest's `UNMAP` of a private page ends it: an access to
/// the page after it finds nothing there, as the window maps nothing, and fails, withheld; never
/// what was stored to the private page, which its memory, mapped there anew, would hold for good
/// unless emptied.
fn private_pages_go_with_their_unmaps() {
    let ps = page_size();
    let (memory, stage, mut dax) = modelled_copying(1);
    let sealed = TestSource::filled(1, SEALED).source();
    dax.map(0, &sealed, 0, ps);
    let window = dax.window.clone();
    future::block_on(window.make_private(0, ps)).unwrap();
    let guest = HostGuest { memory, stage, window, base: 64 * ps as usize };
    match guest.access(0, Access::Write, Some(STORED)) {
        Outcome::Made(read) => assert_eq!(read, STORED),
        _ => panic!("a store to a private page faulted, or failed"),
    }
    dax.unmap(0, ps);
    for _ in 0..ATTEMPTS {
        match guest.access(0, Access::Read, None) {
            Outcome::Made(read) => panic!("an access found a private page gone: {read:#x}"),
            Outcome::Faulted(fault) => future::block_on(fault).unwrap(),
            Outcome::Failed => return,
        }
    }
    panic!("an access to a page unmapped never failed");
}

#[test]
fn private_pages_go_with_their_unmaps_once() {
    Runner::new(RandomScheduler::new(1), config()).run(private_pages_go_with_their_unmaps);
}

/// fxdax: a private page that went left filled: the memory mapped there anew holds it for good.
#[test]
fn needs_private_pages_emptied_as_they_go() {
    *RULES.lock().unwrap() = Rules { private_emptied_as_it_goes: false, ..Rules::ALL };
    fails_with("found a private page gone", || {
        Runner::new(RandomScheduler::new(1), config()).run(private_pages_go_with_their_unmaps);
    });
}

/// fxdax TASK-276.6: what races a store's first touch of a page the window named and the stage
/// never installed ([`first_stores_race_changes`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Racing {
    /// The guest's `MAP` of other bytes over the page, which ends its private page, if it has one.
    Map,
    /// The guest's `UNMAP` of the page, which ends its private page, if it has one.
    Unmap,
    /// The page's source evicting it, which leaves a private page.
    Evict,
}

/// What the other extent a [`Racing::Map`] maps holds.
const OTHER: u8 = 0xd;

/// fxdax TASK-276.6 (backlog doc-13: a `MAP` names pages, their first touch installs them): a
/// vCPU's store is the first touch of a page of a sealed extent's mapping, which the stage never
/// installed, on a window that copies on write, while the page is mapped over, unmapped or
/// evicted (`racing`): its fault goes to the window as a store, which makes the page private in
/// place of nothing, holding what the window maps there then, or is refused for a page unmapped,
/// and the store retried. Then a vCPU loads the page, and stores to it again. Checked: the store
/// is made, in a private page, unless the window maps nothing there by then; what a load reads
/// after both is what the window says the page holds: the store, while the page is private, else
/// the bytes it maps, or nothing, an unmapped page's load failing; a private page is the one page
/// writable here, and a store to it makes no fault; and nothing of the stage's memory faults in a
/// page of zeros, or takes a store untracked (`HostGuest::access_by`).
fn first_stores_race_changes(racing: Racing) {
    let ps = page_size();
    let (memory, stage, mut dax) = modelled_copying(1);
    guest_thread(VCPU);
    let sealed = TestSource::filled(1, SEALED);
    dax.map(0, &sealed.source(), 0, ps);
    assert!(stage.pages.lock().unwrap().pages[0].mapped.is_some_and(|mapped| !mapped.installed()));
    let window = DaxWindow(dax.window.clone());
    let base = 64 * ps as usize;
    let guest =
        Arc::new(HostGuest { memory, stage: stage.clone(), window: dax.window.clone(), base });
    let storing = guest.clone();
    let store = thread::spawn(move || {
        for _ in 0..ATTEMPTS {
            match storing.access(0, Access::Write, Some(STORED)) {
                Outcome::Made(read) => return Some(read),
                Outcome::Faulted(fault) => match future::block_on(fault) {
                    Ok(()) => thread::yield_now(),
                    Err(err) => panic!("a store's fault: {err:?}"),
                },
                // Unmapped meanwhile: the fault, withheld, fails, as KVM_RUN does.
                Outcome::Failed => return None,
            }
        }
        panic!("the store never completed");
    });
    let other = TestSource::filled(1, OTHER);
    let change = thread::spawn(move || {
        match racing {
            Racing::Map => dax.map(0, &other.source(), 0, ps),
            Racing::Unmap => dax.unmap(0, ps),
            Racing::Evict => {
                let identity = crate::dax::source_identity(&sealed.source());
                dax.window.unmap_pages_of(identity, &|_| true);
            }
        }
        (dax, other)
    });
    let stored = store.join().unwrap();
    let (_dax, _other) = change.join().unwrap();
    let private = window.private_pages() == 1;
    let mapped = window.is_mapped(0);
    match racing {
        // An eviction leaves the private page, and maps the page as it was.
        Racing::Evict => assert_eq!((stored, private), (Some(STORED), true), "evicted"),
        Racing::Map | Racing::Unmap if private => assert_eq!(stored, Some(STORED), "{racing:?}"),
        // The store before the change, which ended its private page, or none, unmapped first.
        Racing::Map | Racing::Unmap => {}
    }
    let read = (0..ATTEMPTS).find_map(|_| match guest.access(0, Access::Read, None) {
        Outcome::Made(read) => Some(Some(read)),
        Outcome::Faulted(fault) => {
            future::block_on(fault).unwrap();
            None
        }
        Outcome::Failed => Some(None),
    });
    let read = read.expect("a load never completed");
    let expected = match (private, mapped, racing) {
        (true, _, _) => Some(STORED),
        (false, false, _) => None,
        (false, true, Racing::Map) => Some(OTHER),
        (false, true, _) => Some(SEALED),
    };
    assert_eq!(read, expected, "{racing:?}: a load after the race (private {private})");
    match private {
        true => {
            assert_eq!(guest.writable(), [0], "the private page isn't writable here");
            match guest.access(0, Access::Write, Some(STORED)) {
                Outcome::Made(read) => assert_eq!(read, STORED),
                _ => panic!("a store to the private page faulted, or failed"),
            }
        }
        false => {
            assert_eq!(guest.writable(), Vec::<u64>::new(), "a page writable but private ones");
        }
    }
    // The store made and the page private still; made, and its copy ended by the change after
    // it; or refused, the page unmapped first.
    reached(match (stored, private) {
        (Some(_), true) => Event::ReadStored,
        (Some(_), false) => Event::MapEndedCopy,
        (None, _) => Event::Failed,
    });
}

#[test]
fn first_stores_race_maps_by_pct() {
    let events = [Event::ReadStored, Event::MapEndedCopy];
    reaching(
        || PctScheduler::new(3, runs(RUNS)),
        || first_stores_race_changes(Racing::Map),
        &events,
    );
}

#[test]
fn first_stores_race_unmaps_by_pct() {
    let events = [Event::MapEndedCopy, Event::Failed];
    reaching(
        || PctScheduler::new(3, runs(RUNS)),
        || first_stores_race_changes(Racing::Unmap),
        &events,
    );
}

#[test]
fn first_stores_race_evictions_by_pct() {
    reaching(
        || PctScheduler::new(3, runs(RUNS)),
        || first_stores_race_changes(Racing::Evict),
        &[Event::ReadStored],
    );
}

#[test]
fn first_stores_race_maps_randomly() {
    let events = [Event::ReadStored, Event::MapEndedCopy];
    reaching(
        || RandomScheduler::new(runs(RUNS)),
        || first_stores_race_changes(Racing::Map),
        &events,
    );
}

/// fxdax TASK-276.6: a vCPU's load is the first touch of a page the window named, which the stage
/// installs, while the window maps other bytes over the page, then unmaps it: each load reads the
/// page as the window mapped it as the load was made, never older bytes once newer were read;
/// and once the changes are made, the page reads as the window maps it, not at all once unmapped.
/// A page installed as a fault found it, after a change named it anew, read the bytes the window
/// mapped before for good; a change leaving the page installed, the same.
fn first_loads_race_changes() {
    let ps = page_size();
    let (memory, stage, mut dax) = modelled_copying(1);
    guest_thread(VCPU);
    let sealed = TestSource::filled(1, SEALED);
    dax.map(0, &sealed.source(), 0, ps);
    let base = 64 * ps as usize;
    let guest = Arc::new(HostGuest { memory, stage, window: dax.window.clone(), base });
    let loading = guest.clone();
    let loads = thread::spawn(move || {
        let mut last = None;
        for _ in 0..3 {
            for _ in 0..ATTEMPTS {
                match loading.access(0, Access::Read, None) {
                    Outcome::Made(read) => {
                        let rank = |read: u8| [SEALED, OTHER].iter().position(|&held| held == read);
                        let ranked = rank(read).unwrap_or_else(|| panic!("a load read {read:#x}"));
                        assert!(last <= Some(ranked), "a load read {read:#x} after newer bytes");
                        last = Some(ranked);
                        break;
                    }
                    Outcome::Faulted(fault) => future::block_on(fault).unwrap(),
                    // Unmapped.
                    Outcome::Failed => return,
                }
                thread::yield_now();
            }
            thread::yield_now();
        }
    });
    let other = TestSource::filled(1, OTHER);
    dax.map(0, &other.source(), 0, ps);
    assert_eq!(load_byte(&guest, 0), OTHER, "a load after the map read other bytes");
    dax.unmap(0, ps);
    loads.join().unwrap();
    let after = (0..ATTEMPTS).find_map(|_| match guest.access(0, Access::Read, None) {
        Outcome::Made(read) => Some(Some(read)),
        Outcome::Faulted(fault) => {
            future::block_on(fault).unwrap();
            None
        }
        Outcome::Failed => Some(None),
    });
    assert_eq!(after, Some(None), "a load of the page unmapped read it");
}

#[test]
fn first_loads_race_changes_by_pct() {
    Runner::new(PctScheduler::new(3, runs(RUNS)), config()).run(first_loads_race_changes);
}

#[test]
fn first_loads_race_changes_randomly() {
    Runner::new(RandomScheduler::new(runs(RUNS)), config()).run(first_loads_race_changes);
}

/// fxdax TASK-276.6: a change of pages installed leaving their memory as it was, the window's next
/// page named alone: the stage reads the bytes the window mapped before.
#[test]
fn needs_changes_to_take_away_installed() {
    *RULES.lock().unwrap() = Rules { changes_take_away_installed: false, ..Rules::ALL };
    fails_with("a load", || {
        Runner::new(PctScheduler::new(3, runs(10 * RUNS)), config()).run(first_loads_race_changes);
    });
}

/// fxdax TASK-276.6: a page installed as a fault found it, its lock let go of meanwhile: a map
/// between, of other bytes, is undone, the page installed with the bytes mapped before.
#[test]
fn needs_installs_as_named_now() {
    *RULES.lock().unwrap() = Rules { install_as_named_now: false, ..Rules::ALL };
    fails_with("a load", || {
        Runner::new(PctScheduler::new(3, runs(10 * RUNS)), config()).run(first_loads_race_changes);
    });
}

/// fxdax TASK-276.6 (backlog doc-13: a `MAP` names pages, their first touch installs them): pages
/// the window maps take no `mmap`, nor does a `PRIVATE` of a page never touched, nor a store's
/// first touch, each private copy put in place of nothing; a load's first touch installs its run,
/// one `mmap`; a `PRIVATE` of a page installed, one over it; and the pages read and take stores as
/// they should. Red with the window's maps installing: an `mmap` a map, and one over each page
/// made private, the TLB flush of each.
fn pages_never_touched_change_with_no_mmap() {
    let ps = page_size();
    let (memory, stage, mut dax) = modelled_copying(4);
    guest_thread(VCPU);
    let sealed = TestSource::filled(4, SEALED).source();
    let maps = || memory.0.lock().unwrap().maps;
    let before = maps();
    dax.map(0, &sealed, 0, 4 * ps);
    assert_eq!(maps() - before, 0, "a map");
    let window = dax.window.clone();
    let base = 64 * ps as usize;
    let guest =
        HostGuest { memory: memory.clone(), stage: stage.clone(), window: window.clone(), base };
    future::block_on(window.make_private(0, ps)).unwrap();
    assert_eq!(maps() - before, 0, "a PRIVATE of a page never touched");
    assert!(matches!(guest.access(ps, Access::Write, Some(STORED)), Outcome::Faulted(_)));
    store_byte(&guest, ps);
    assert_eq!(maps() - before, 0, "a store's first touch");
    assert_eq!(load_byte(&guest, 2 * ps), SEALED);
    assert_eq!(maps() - before, 1, "a load's first touch");
    future::block_on(window.make_private(3 * ps, ps)).unwrap();
    assert_eq!(maps() - before, 2, "a PRIVATE of a page installed");
    assert_eq!(window.private_pages(), 3);
    assert_eq!(guest.writable(), [0, ps, 3 * ps], "the private pages aren't writable here");
    for (page, held) in [(0, SEALED), (ps, STORED), (2 * ps, SEALED), (3 * ps, SEALED)] {
        assert_eq!(load_byte(&guest, page), held, "page {page:#x}");
    }
    assert_eq!(stage.taken(), [1, 0], "a change took a page installed away");
}

#[test]
fn pages_never_touched_change_with_no_mmap_once() {
    Runner::new(RandomScheduler::new(1), config()).run(pages_never_touched_change_with_no_mmap);
}

/// fxdax TASK-276.6: the guest's `UNMAP` of a range of pages installed and private pages among
/// them takes one `mmap`, and empties the private pages once nothing maps them, which flushes no
/// TLB; one of a private page alone, nothing installed, empties it in place, where the window
/// maps it, one flush, with no `mmap`; and one of pages never touched changes nothing of the
/// memory at all. Red with the private pages emptied first, as they were: each emptied where the
/// window mapped it, a flush each besides the `mmap`'s.
fn unmaps_empty_private_pages_unmapped() {
    let ps = page_size();
    let (memory, stage, mut dax) = modelled_copying(8);
    guest_thread(VCPU);
    // Its pages scattered, a run each, which a load installs alone.
    let sealed = TestSource::scattered(8, |_| SEALED).source();
    dax.map(0, &sealed, 0, 8 * ps);
    let window = dax.window.clone();
    let base = 64 * ps as usize;
    let guest = HostGuest { memory: memory.clone(), stage, window: window.clone(), base };
    // Pages 0 to 3 installed, 0 and 2 of them made private over them; 5 private alone.
    for page in 0..4 {
        assert_eq!(load_byte(&guest, page * ps), SEALED);
    }
    for page in [0, 2, 5] {
        future::block_on(window.make_private(page * ps, ps)).unwrap();
    }
    let counts = || {
        let model = memory.0.lock().unwrap();
        (model.maps, model.punched_mapped, model.filled.len())
    };
    let (maps, punched, filled) = counts();
    assert_eq!(filled, 3);
    dax.unmap(0, 4 * ps);
    assert_eq!(counts(), (maps + 1, punched, 1), "an unmap of pages installed and private");
    dax.unmap(4 * ps, 2 * ps);
    assert_eq!(counts(), (maps + 1, punched + 1, 0), "an unmap of a private page alone");
    dax.unmap(6 * ps, 2 * ps);
    assert_eq!(counts(), (maps + 1, punched + 1, 0), "an unmap of pages never touched");
    assert_eq!(window.private_pages(), 0);
    for page in 0..8 {
        assert!(matches!(guest.access(page * ps, Access::Read, None), Outcome::Faulted(_)));
    }
}

#[test]
fn unmaps_empty_private_pages_unmapped_once() {
    Runner::new(RandomScheduler::new(1), config()).run(unmaps_empty_private_pages_unmapped);
}
