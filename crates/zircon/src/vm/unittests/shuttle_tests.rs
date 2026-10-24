// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Mappings under Shuttle (awslabs/shuttle, the `shuttle` feature): mapping a mapping's pages
//! (`VmMapping::map_range`) on a thread of its own, racing a change to the mapping on another, in
//! the interleavings Shuttle's schedulers choose; an allocation of an arena's pages racing the
//! arena's removal; a chunk going back, outside the PMM's lock, racing an allocation it makes room
//! for, and its arena's removal; eviction of an arena's own pages racing a page it chose
//! supplied again; and a page freed racing its second stage's unmapping it, and its reuse.

use super::test_helper::StubPageProvider;
use crate::hypervisor::aspace::GuestPhysicalAspace;
use crate::kernel::deadline::Deadline;
use crate::sync::{Rules, set_rules};
use crate::vm::arch_vm_aspace::{ARCH_MMU_FLAG_PERM_READ, ARCH_MMU_FLAG_PERM_WRITE, ArchMmuFlags};
use crate::vm::page_source::PageSource;
use crate::vm::physmap::PhysLocation;
use crate::vm::pmm;
use crate::vm::vm_address_region::{VMAR_FLAG_SPECIFIC, VmAddressRegionOpChildren};
use crate::vm::vm_cow_pages::SupplyOptions;
use crate::vm::vm_mapping::VmMapping;
use crate::vm::vm_object_paged::VmObjectPaged;
use crate::vm::vm_page_list::VmPageSpliceList;
use shuttle::scheduler::{PctScheduler, RandomScheduler};
use shuttle::{Config, Runner, thread};
use std::sync::Arc;
use zx_status::Status;

const PAGE_SIZE: usize = page::SIZE;

/// The pages a test maps.
const PAGES: usize = 4;

/// The schedules a test runs with each scheduler.
const RUNS: usize = 100;

/// Runs `test` in schedules of each scheduler's, with threads' stacks as large as the kernel's
/// calls need.
fn check(test: fn()) {
    let config = || {
        let mut config = Config::new();
        config.stack_size = 4 << 20;
        config
    };
    Runner::new(RandomScheduler::new(RUNS), config()).run(test);
    Runner::new(PctScheduler::new(3, RUNS), config()).run(test);
}

/// A guest physical address space, and a mapping in it of `PAGES` committed pages of an anonymous
/// VMO, readable and writable, none of whose pages is mapped yet.
fn mapping() -> (GuestPhysicalAspace, Arc<VmMapping>) {
    let gpas = GuestPhysicalAspace::create(2 * PAGES * PAGE_SIZE).unwrap();
    let len = PAGES * PAGE_SIZE;
    let vmo = VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, len as u64).unwrap();
    vmo.commit_range(0, len as u64).unwrap();
    let flags: ArchMmuFlags = ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;
    let root = gpas.root_vmar();
    let (mapping, _) =
        root.create_vm_mapping(0, len, 0, VMAR_FLAG_SPECIFIC, vmo, 0, flags, "vmo").unwrap();
    (gpas, mapping)
}

/// Maps the pages of `mapping` on a thread of its own while `change` runs on this one.
fn map_pages_racing(mapping: &Arc<VmMapping>, change: impl FnOnce()) {
    let mapping = mapping.clone();
    let mapper = thread::spawn(move || {
        // It may find the mapping gone, and fail: what it leaves mapped is what the test checks.
        let _ = mapping.map_range(0, PAGES * PAGE_SIZE, false, true);
    });
    change();
    mapper.join().unwrap();
}

/// Mapping a mapping's pages racing its unmapping leaves none of them mapped. A page mapped after
/// the unmapping would stay mapped once the VMO frees it, as the mapping no longer hears of its
/// VMO's changes.
fn mapping_pages_races_unmapping() {
    let (gpas, mapping) = mapping();
    map_pages_racing(&mapping, || gpas.unmap_range(0, PAGES * PAGE_SIZE).unwrap());
    for page in 0..PAGES {
        let mapped = gpas.arch_aspace().query(page * PAGE_SIZE);
        assert_eq!(mapped, Err(Status::NOT_FOUND), "page {page} left mapped");
    }
}

#[test]
fn mapping_pages_races_unmapping_in_any_order() {
    check(mapping_pages_races_unmapping);
}

/// Mapping a mapping's pages racing a protect that takes write access away maps each of them
/// read-only: mapped before, the protect changes it; after, it is mapped as the protect left the
/// mapping.
fn mapping_pages_races_protecting() {
    let (gpas, mapping) = mapping();
    let root = gpas.root_vmar();
    let read_only = || {
        let children = VmAddressRegionOpChildren::No;
        root.protect(0, PAGES * PAGE_SIZE, ARCH_MMU_FLAG_PERM_READ, children).unwrap();
    };
    map_pages_racing(&mapping, read_only);
    for page in 0..PAGES {
        let flags = gpas.arch_aspace().query(page * PAGE_SIZE).map(|(_, flags)| flags);
        assert_eq!(flags, Ok(ARCH_MMU_FLAG_PERM_READ), "page {page}");
    }
}

#[test]
fn mapping_pages_races_protecting_in_any_order() {
    check(mapping_pages_races_protecting);
}

/// TASK-79.36: an allocation that waits for a page of an arena, racing the arena's removal, takes
/// one, or fails, as nothing makes a removed arena room; and either way leaves the node's gate to
/// the waiter after it, which a waiter that took it and failed would keep from every other. The
/// arena goes once the pages it gave are freed.
fn allocating_races_removing_the_arena() {
    let node = pmm::node();
    let arena = pmm::create_arena();
    // A chunk of the arena's, its pages free but this one.
    let held = node.alloc_page_in(arena, pmm::ALLOC_FLAG_ANY).unwrap();
    let allocator = thread::spawn(move || {
        pmm::node().wait_for_single_page_allocation_in(arena, Deadline::infinite()).ok()
    });
    pmm::remove_arena(arena);
    // What it takes, if anything, is freed below: the gate and the arena are what the test checks.
    let taken = allocator.join().unwrap();
    let now = Deadline::after_mono(0);
    let next = node.wait_for_single_page_allocation(now).expect("the gate lets the next by");
    for page in [held, next].into_iter().chain(taken) {
        // SAFETY: The test owns the pages.
        unsafe { pmm::free_page(page) };
    }
    assert!(node.phys_memory().arena_fd(arena).is_none(), "the arena stayed");
}

#[test]
fn allocating_races_removing_the_arena_in_any_order() {
    check(allocating_races_removing_the_arena);
}

/// TASK-92.35: an allocation that a chunk going back makes room for, as the host takes it back
/// outside the PMM's lock, waits for it, and takes a page: here one that may not wait, which would
/// fail at once otherwise, at the total cap, with no spare to give back, and no chunk under the cap
/// but the one going back.
fn allocating_races_a_chunk_going_back() {
    let node = pmm::node();
    node.keep_spare_chunks(false);
    let (going, waiting) = (pmm::create_arena(), pmm::create_arena());
    // A chunk of the arena's, which goes back as the page is freed.
    let page = node.alloc_page_in(going, pmm::ALLOC_FLAG_ANY).unwrap();
    node.set_total_cap(Some(node.arenas_bytes()));
    let free = node.count_free_pages();
    // SAFETY: The test owns the page.
    let freer = thread::spawn(move || unsafe { pmm::free_page(page) });
    // The page counted free under the lock it is put back under, so its chunk is going back, or
    // gone, once this takes the lock.
    while node.count_free_pages() == free {
        thread::yield_now();
    }
    let taken = node.alloc_page_in(waiting, pmm::ALLOC_FLAG_ANY);
    freer.join().unwrap();
    let page = taken.expect("a page, as the chunk went back");
    // SAFETY: As above.
    unsafe { pmm::free_page(page) };
    node.set_total_cap(None);
    for arena in [going, waiting] {
        pmm::remove_arena(arena);
        assert!(node.phys_memory().arena_fd(arena).is_none(), "{arena:?} stayed");
    }
}

#[test]
fn allocating_races_a_chunk_going_back_in_any_order() {
    check(allocating_races_a_chunk_going_back);
}

/// TASK-92.35: an arena removed as the last of its pages is freed goes, and its object, once its
/// chunks are back, its spare and the page's, whichever goes back first.
fn freeing_races_removing_the_arena() {
    let node = pmm::node();
    let arena = pmm::create_arena();
    // A chunk, and a spare.
    let page = node.alloc_page_in(arena, pmm::ALLOC_FLAG_ANY).unwrap();
    // SAFETY: The test owns the page.
    let freer = thread::spawn(move || unsafe { pmm::free_page(page) });
    pmm::remove_arena(arena);
    freer.join().unwrap();
    assert_eq!(node.arena_bytes(arena), 0);
    assert!(node.phys_memory().arena_fd(arena).is_none(), "the arena stayed");
}

#[test]
fn freeing_races_removing_the_arena_in_any_order() {
    check(freeing_races_removing_the_arena);
}

/// Pages of the current arena, `len` bytes of them, taken from an anonymous VMO, for a pager to
/// supply.
fn pages_to_supply(len: u64) -> VmPageSpliceList {
    let anonymous = VmObjectPaged::create(0, 0, len).unwrap();
    anonymous.commit_range(0, len).unwrap();
    let mut pages = VmPageSpliceList::new();
    anonymous.take_pages(0, len, &mut pages).unwrap();
    pages
}

/// TASK-79.76: eviction of an arena's own pages, racing another eviction of the page it chose, as
/// relief's evictor makes, and the page's supplying again, as its pager does as it is faulted on,
/// evicts not the page supplied in its place, the newest: the page it chose gone, it evicts the
/// oldest left, or nothing. Here the arena's pages are a pager-backed VMO's two, a queue older than
/// the page supplied again in the first's place, which is there at the end, whatever the order.
fn evicting_an_arenas_own_pages_races_a_page_supplied_again() {
    let node = pmm::node();
    let arena = pmm::create_arena();
    let len = PAGE_SIZE as u64;
    let source = PageSource::new(Arc::new(StubPageProvider::new(false, false)));
    let vmo =
        pmm::with_arena(arena, || VmObjectPaged::create_external(source, 0, 2 * len)).unwrap();
    let mut pages = pmm::with_arena(arena, || pages_to_supply(2 * len));
    vmo.supply_pages(0, 2 * len, &mut pages, SupplyOptions::PagerSupply).unwrap();
    node.page_queues().rotate_reclaim_queues();
    node.evictor().enable_eviction(false);
    let mut again = pmm::with_arena(arena, || pages_to_supply(len));

    let evictor = thread::spawn(move || pmm::node().evictor().evict_arena_on_demand(arena, len));
    vmo.evict_range(0, len).unwrap();
    vmo.supply_pages(0, len, &mut again, SupplyOptions::PagerSupply).unwrap();
    evictor.join().unwrap();
    assert!(vmo.debug_get_page(0).is_some(), "the page supplied again stays");
    drop(vmo);
    pmm::remove_arena(arena);
}

#[test]
fn evicting_an_arenas_own_pages_races_a_page_supplied_again_in_any_order() {
    check(evicting_an_arenas_own_pages_races_a_page_supplied_again);
}

/// A second stage of another process, as the extent daemon's connection to a VM process is: it maps
/// pages as the kernel maps them, but makes the changes taking them away later, on a thread of its
/// own ([`LaggingStage::make`]), as the VM process makes them: what a guest reads through it
/// meanwhile is what the pages hold.
#[derive(Default)]
struct LaggingStage(parking_lot::Mutex<Lagging>);

#[derive(Default)]
struct Lagging {
    /// What the guest reaches: where each page's memory is.
    mapped: std::collections::BTreeMap<usize, PhysLocation>,
    /// The unmaps sent, not made yet, and how many were sent, and made.
    unmaps: std::collections::VecDeque<(usize, usize)>,
    sent: u64,
    made: u64,
    /// The pages freed, quarantined until the unmaps sent before are made.
    parked: Vec<(u64, pmm::Quarantined)>,
}

impl crate::arch::host::mmu::SecondStage for LaggingStage {
    fn map(&self, vaddr: usize, phys: PhysLocation, len: usize, _prot: libc::c_int) {
        let mut stage = self.0.lock();
        for at in (0..len).step_by(PAGE_SIZE) {
            let page = PhysLocation { arena: phys.arena, offset: phys.offset + at as u64 };
            stage.mapped.insert(vaddr + at, page);
        }
    }

    fn protect(&self, _vaddr: usize, _len: usize, _prot: libc::c_int) {}

    fn unmap(&self, vaddr: usize, len: usize) {
        let mut stage = self.0.lock();
        stage.unmaps.push_back((vaddr, len));
        stage.sent += 1;
    }
}

impl LaggingStage {
    /// Makes the oldest unmap not made yet, and lets go of the pages that waited for it: whether
    /// there was one.
    fn make(&self) -> bool {
        let released: Vec<_> = {
            let mut stage = self.0.lock();
            let Some((vaddr, len)) = stage.unmaps.pop_front() else { return false };
            for at in (0..len).step_by(PAGE_SIZE) {
                stage.mapped.remove(&(vaddr + at));
            }
            stage.made += 1;
            let made = stage.made;
            stage.parked.extract_if(.., |(sent, _)| *sent <= made).collect()
        };
        // Freed without the stage's lock, which a quarantine takes.
        drop(released);
        true
    }

    /// Quarantines `pages` until the unmaps sent so far are made.
    fn park(&self, pages: pmm::Quarantined) {
        let mut stage = self.0.lock();
        if stage.made < stage.sent {
            let sent = stage.sent;
            stage.parked.push((sent, pages));
            return;
        }
        drop(stage);
        drop(pages);
    }

    /// The first word of what the guest reads at `vaddr` through the stage, if anything is mapped
    /// there, read as its VM process maps the page.
    fn read(&self, vaddr: usize) -> Option<u64> {
        use std::os::fd::AsRawFd;
        let stage = self.0.lock();
        let page = stage.mapped.get(&vaddr)?;
        let arena = pmm::node().phys_memory().arena_fd(page.arena).expect("the page's arena");
        let (len, prot) = (PAGE_SIZE, libc::PROT_READ);
        // SAFETY: A new mapping of the page's memory, which outlives it.
        let mapped = unsafe {
            let offset = page.offset as libc::off_t;
            libc::mmap(std::ptr::null_mut(), len, prot, libc::MAP_SHARED, arena.as_raw_fd(), offset)
        };
        assert_ne!(mapped, libc::MAP_FAILED, "{}", std::io::Error::last_os_error());
        // SAFETY: The mapping's first word, which the guest reads at once.
        let word = unsafe { mapped.cast::<u64>().read_volatile() };
        // SAFETY: The mapping made above.
        assert_eq!(unsafe { libc::munmap(mapped, len) }, 0);
        Some(word)
    }
}

/// The stage of the schedule running, which the quarantine parks the pages freed on.
static LAGGING: std::sync::Mutex<Option<Arc<LaggingStage>>> = std::sync::Mutex::new(None);

/// The quarantine of the tests of [`freeing_races_the_second_stage`], which parks the pages freed
/// on the stage of the schedule running, held for as long as the test runs.
fn lagging_quarantine() -> pmm::QuarantineForTest {
    pmm::quarantine_for_test(|pages| {
        LAGGING.lock().unwrap().as_ref().expect("a schedule's stage").park(pages)
    })
}

/// What a page freed of [`freeing_races_the_second_stage`] holds, and what its next owner writes.
const FREED: u64 = 0xf7ee_d000_f7ee_d000;
const REUSED: u64 = 0x0e05_ed00_0e05_ed00;

/// TASK-79.9: a page a second stage of another process maps, which makes the kernel's changes
/// after they return, freed, is no other object's until the stage has taken it away: a guest
/// reading it through the stage meanwhile reads what it held, never what an object made after
/// holds. A VMO's page mapped through a [`LaggingStage`], the mapping unmapped and the VMO
/// dropped, and another made and written, as its VM process makes the unmap and its guest reads.
fn freeing_races_the_second_stage() {
    let stage = Arc::new(LaggingStage::default());
    *LAGGING.lock().unwrap() = Some(stage.clone());
    let gpas = GuestPhysicalAspace::create_with_second_stage(PAGE_SIZE, stage.clone()).unwrap();
    let page = || VmObjectPaged::create(pmm::ALLOC_FLAG_ANY, 0, PAGE_SIZE as u64).unwrap();
    // A page of the chunk the next is of, which keeps it the arena's as the next is freed, and
    // so that page the one allocated next.
    let keeper = page();
    keeper.commit_range(0, PAGE_SIZE as u64).unwrap();
    let vmo = page();
    vmo.write(&FREED.to_ne_bytes(), 0).unwrap();
    let (mapping, _) = (gpas.root_vmar())
        .create_vm_mapping(
            0,
            PAGE_SIZE,
            0,
            VMAR_FLAG_SPECIFIC,
            vmo,
            0,
            ARCH_MMU_FLAG_PERM_READ,
            "vmo",
        )
        .unwrap();
    mapping.map_range(0, PAGE_SIZE, false, true).unwrap();
    assert_eq!(stage.read(0), Some(FREED));

    let vmm = {
        let stage = stage.clone();
        thread::spawn(move || {
            while !stage.make() {
                thread::yield_now();
            }
        })
    };
    let guest = {
        let stage = stage.clone();
        thread::spawn(move || {
            for _ in 0..3 {
                if let Some(read) = stage.read(0) {
                    assert_eq!(read, FREED, "the guest read a page another object holds");
                }
                thread::yield_now();
            }
        })
    };
    gpas.unmap_range(0, PAGE_SIZE).unwrap();
    drop(mapping);
    let next = page();
    next.write(&REUSED.to_ne_bytes(), 0).unwrap();
    guest.join().unwrap();
    vmm.join().unwrap();
    assert!(stage.0.lock().parked.is_empty(), "pages quarantined once the stage made the unmap");
    LAGGING.lock().unwrap().take();
    drop((keeper, next));
}

#[test]
fn freeing_races_the_second_stage_in_any_order() {
    let _quarantine = lagging_quarantine();
    check(freeing_races_the_second_stage);
}

/// The batches of schedules [`fails_without`] runs at most to find one that fails.
const BATCHES: usize = 8;

/// Runs `test` with `rules` in force, which turn one off, expecting one of its schedules to fail
/// with a panic saying `expected`. Its schedules are random: a batch may find none that fails,
/// which isn't that the rule is not needed, so the batches, new each time, run until one does, up
/// to [`BATCHES`]. The panics are collected by a hook installed for good, which Shuttle's chains
/// to, so each such test must be a process of its own, as nextest runs them.
fn fails_without(rules: Rules, expected: &str, test: fn()) {
    set_rules(rules);
    static PANICS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    std::panic::set_hook(Box::new(|info| {
        eprintln!("{info}");
        PANICS.lock().unwrap().push(info.payload_as_str().unwrap_or_default().to_owned());
    }));
    let failed = (0..BATCHES).any(|_| match std::panic::catch_unwind(|| check(test)) {
        Ok(()) => false,
        // Its payload may hold Shuttle's objects, which can't be dropped outside a run.
        Err(payload) => {
            std::mem::forget(payload);
            true
        }
    });
    let panics = std::mem::take(&mut *PANICS.lock().unwrap());
    assert!(failed, "no schedule failed, expecting {expected:?}");
    assert!(
        panics.iter().any(|panic| panic.contains(expected)),
        "expected {expected:?}, got {panics:#?}"
    );
}

/// A waited allocation failing as its arena was removed after it took the node's gate keeps the
/// gate: the next waiter finds it shut.
#[test]
fn needs_arena_failures_to_pass_the_gate() {
    let rules = Rules { arena_failures_pass_the_gate: false, ..Rules::ALL };
    fails_without(rules, "the gate lets the next by", allocating_races_removing_the_arena);
}

/// An allocation that does not wait for a chunk going back fails, out of room under the total cap,
/// though the chunk is about to be free.
#[test]
fn needs_allocations_to_wait_for_chunks_going_back() {
    let rules = Rules { allocations_wait_for_chunks_going_back: false, ..Rules::ALL };
    fails_without(rules, "a page, as the chunk went back", allocating_races_a_chunk_going_back);
}

/// Eviction of an arena's own pages that evicts whatever page it finds where it chose one evicts
/// the page supplied again in its place.
#[test]
fn needs_evictions_to_take_only_the_pages_chosen() {
    let rules = Rules { evictions_take_only_the_pages_chosen: false, ..Rules::ALL };
    let test = evicting_an_arenas_own_pages_races_a_page_supplied_again;
    fails_without(rules, "the page supplied again stays", test);
}

/// A page freed at once, though a second stage of another process may map it still, is another
/// object's that the guest reads.
#[test]
fn needs_staged_pages_to_wait_for_their_stages() {
    let rules = Rules { staged_pages_wait_for_their_stages: false, ..Rules::ALL };
    let (_quarantine, test) = (lagging_quarantine(), freeing_races_the_second_stage);
    fails_without(rules, "the guest read a page another object holds", test);
}
