// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The userfaultfd second stage under Shuttle (TASK-44.3): its real code, and the window's, over a model of the host memory it changes, [`ModelMemory`], which a guest's accesses
//! are made against as KVM makes them. Each of the stage's rules of the order it changes its
//! memory in, turned off, fails.

use super::super::shuttle_tests::{
    ATTEMPTS, Event, Guest, Outcome, RUNS, SEALED, byte, reaching, stores_race_frees,
};
use super::super::tests::TestSource;
use super::super::{Dax, Source as _, Window};
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
    /// fxdax: where the address space the stage reserves next is.
    reserved: usize,
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

    fn reserve(&self, len: usize) -> std::io::Result<usize> {
        // Where it is doesn't matter to the model, but that it is apart from the rest.
        let mut model = self.0.lock().unwrap();
        let addr = (1 << 40) + model.reserved;
        model.reserved += len.next_multiple_of(1 << 30);
        Ok(addr)
    }

    fn unreserve(&self, addr: usize, len: usize) {
        let mut model = self.0.lock().unwrap();
        model.pages.retain(|at, _| !(addr..addr + len).contains(at));
    }

    fn fill(&self, addr: usize, bytes: &[u8]) {
        let mut model = self.0.lock().unwrap();
        let page = model.pages[&addr];
        let registered = page.registered & UFFDIO_REGISTER_MODE_MISSING != 0;
        let Some((fd, at)) = page.file.filter(|_| registered) else {
            drop(model);
            panic!("filled {addr:#x}, which isn't registered for missing faults");
        };
        if !model.unfilled(&page) {
            drop(model);
            panic!("filled {addr:#x} twice");
        }
        model.filled.insert(at);
        // SAFETY: Writes `bytes` to the private memory, whose page it is.
        let written = unsafe { libc::pwrite(fd, bytes.as_ptr().cast(), bytes.len(), at as _) };
        assert_eq!(written, bytes.len() as isize, "{}", std::io::Error::last_os_error());
    }

    fn punch(&self, fd: RawFd, offset: u64, len: usize) {
        let mut model = self.0.lock().unwrap();
        model.filled.retain(|at| !(offset..offset + len as u64).contains(at));
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

    fn make_private(&self, offset: u64, from: Option<PhysLocation>) {
        self.change(offset, page_size(), true, || self.stage.make_private(offset, from));
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
/// makes them (TASK-91.16): what a thread makes in a change ([`SecondStage::begin_change`]) is
/// held back, as the daemon's sender holds a corked thread's messages, until the change ends, or
/// another thread makes something outside a change, which makes what was held back first, in
/// order; then each unmap is made with what follows it within what it unmaps, at once
/// ([`SecondStage::remap`]), as the VM process takes it (`wire::Receiver::take_change`).
struct Remapping {
    stage: Arc<Telling>,
    held: shuttle::sync::Mutex<Held>,
}

#[derive(Default)]
struct Held {
    /// The threads in a change, and how many changes deep.
    changing: Vec<(thread::ThreadId, usize)>,
    ops: Vec<Op>,
}

impl Remapping {
    /// Makes `op`: held back if this thread is in a change, or made after what was held back.
    fn make(&self, op: Op) {
        let mut held = self.held.lock().unwrap();
        let me = thread::current().id();
        held.ops.push(op);
        if !held.changing.iter().any(|(changing, _)| *changing == me) {
            self.flush(&mut held);
        }
    }

    /// Makes what was held back, in order, holding it so that what is made after waits.
    fn flush(&self, held: &mut Held) {
        let ps = page_size();
        let mut ops = std::mem::take(&mut held.ops).into_iter().peekable();
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
                Op::Private(offset, from) => self.stage.make_private(offset, from),
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

    fn make_private(&self, offset: u64, from: Option<PhysLocation>) {
        self.make(Op::Private(offset, from));
    }

    fn begin_change(&self) {
        let mut held = self.held.lock().unwrap();
        let me = thread::current().id();
        match held.changing.iter_mut().find(|(changing, _)| *changing == me) {
            Some((_, depth)) => *depth += 1,
            None => held.changing.push((me, 1)),
        }
    }

    fn end_change(&self) {
        let mut held = self.held.lock().unwrap();
        let me = thread::current().id();
        let at = held.changing.iter().position(|(changing, _)| *changing == me).unwrap();
        held.changing[at].1 -= 1;
        if held.changing[at].1 == 0 {
            held.changing.remove(at);
            self.flush(&mut held);
        }
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

/// fxdax: the window's changes made at once leave the host memory as made in steps (TASK-91.16),
/// with fewer `mmap`s: none for a map of the very pages installed, left in place; one for a map
/// of other memory, installed over the old; and pages no map covers, as a file's not in memory,
/// left out. A page a refused store restricted is installed anew, as in steps, which lifts it.
fn remaps_end_as_steps_do() {
    let ps = page_size();
    let (steps, steps_stage, mut steps_dax) = modelled(2, false);
    let (at_once, at_once_stage, mut at_once_dax) = modelled(2, true);
    let (sealed, other) = (TestSource::filled(2, 1).source(), TestSource::filled(2, 2).source());
    // Not yet written, it has no page in memory.
    let unwritten = TestSource::paged(2).source();
    let pages = |memory: &ModelMemory| memory.0.lock().unwrap().pages.clone();
    let maps = |memory: &ModelMemory| memory.0.lock().unwrap().maps;
    let mut change = |what: &str, mmaps: usize, make: &dyn Fn(&mut Dax)| {
        let before = maps(&at_once);
        make(&mut steps_dax);
        make(&mut at_once_dax);
        assert_eq!(pages(&at_once), pages(&steps), "{what}: the memory differs");
        assert_eq!(maps(&at_once) - before, mmaps, "{what}: mmaps");
    };
    change("sealed", 1, &|dax| dax.map(0, &sealed, 0, 2 * ps));
    change("the same pages", 0, &|dax| dax.map(0, &sealed, 0, 2 * ps));
    // A store faulted on the second page, which the window refused, as it doesn't copy on write.
    for stage in [&steps_stage, &at_once_stage] {
        let addr = 64 * ps as usize + ps as usize;
        stage.restrict(addr, stage.seen(addr), libc::PROT_READ);
    }
    // The first page in place, the second, restricted, installed anew.
    change("the same, one restricted", 1, &|dax| dax.map(0, &sealed, 0, 2 * ps));
    change("other memory", 1, &|dax| dax.map(0, &other, 0, 2 * ps));
    change("nothing in memory", 1, &|dax| dax.map(0, &unwritten, 0, 2 * ps));
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
        assert!(mapped.is_some_and(|mapped| !mapped.installed), "the first page wasn't left out");
    }

    // The worker's write, retried as get_user_pages retries it, until it fails or is made.
    for _ in 0..ATTEMPTS {
        match guest.access_by(WORKER, 0, Access::Write, None) {
            Outcome::Faulted(fault) => future::block_on(fault).unwrap(),
            Outcome::Failed | Outcome::Made(_) => break,
        }
    }
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
/// leaving the page writable here: the write, retried, faults as a store, and copies the page for
/// a load.
fn needs_worker_writes_restricted(not_here: NotHere) {
    *RULES.lock().unwrap() = Rules { worker_writes_restricted: false, ..Rules::ALL };
    fails_with("a worker's write copied the page", || {
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
/// read-only, write-protected, faults as a store, whoever's thread reports it, as only a store
/// faults so (KVM's worker's for the vCPU's store, which waits for it): the page is copied,
/// mapped writable here in its place, and holds the store; the extent's page is as it was.
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

    match guest.access_by(WORKER, 0, Access::Write, None) {
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
    assert_eq!(byte(arena.as_raw_fd(), shared.offset, None), SEALED, "the store reached the extent");
}

#[test]
fn stores_to_pages_here_copy_them_once() {
    Runner::new(RandomScheduler::new(1), config()).run(stores_to_pages_here_copy_them);
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
