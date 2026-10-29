// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Physical memory under Shuttle (awslabs/shuttle, the `shuttle` feature), lifted from fxdax's
//! port of the Zircon kernel (`vm::unittests::shuttle_tests`): an allocation of an arena's pages
//! racing the arena's removal; a chunk going back, outside the PMM's lock, racing an allocation it
//! makes room for, and its arena's removal; and a page freed racing its second stage's unmapping
//! it, and its reuse; in the interleavings Shuttle's schedulers choose.

use super::configure_chunks;
use crate::sync::{Rules, set_rules};
use crate::{
    ArenaId, PhysLocation, Quarantined, Wait, alloc, arena_bytes, arena_fd, arenas_bytes,
    count_free_pages, create_arena, keep_spare_chunks, page_bytes, remove_arena, set_total_cap,
};
use shuttle::scheduler::{PctScheduler, RandomScheduler};
use shuttle::{Config, Runner, thread};
use std::sync::Arc;

/// The schedules a test runs with each scheduler.
const RUNS: usize = 100;

/// Runs `test` in schedules of each scheduler's, in physical memory of a few chunks.
fn check(test: fn()) {
    static CONFIGURED: std::sync::Once = std::sync::Once::new();
    CONFIGURED.call_once(|| configure_chunks(16));
    let config = || {
        let mut config = Config::new();
        config.stack_size = 4 << 20;
        config
    };
    Runner::new(RandomScheduler::new(RUNS), config()).run(test);
    Runner::new(PctScheduler::new(3, RUNS), config()).run(test);
}

/// TASK-79.36: an allocation that may wait for a page of an arena, racing the arena's removal,
/// takes one, or fails, as nothing makes a removed arena room. The arena goes once the pages it
/// gave are freed.
fn allocating_races_removing_the_arena() {
    let arena = create_arena();
    // A chunk of the arena's, its pages free but this one.
    let held = alloc(arena, Wait::No).unwrap();
    let allocator = thread::spawn(move || alloc(arena, Wait::Yes).ok());
    remove_arena(arena);
    // What it takes, if anything, is freed below: the arena is what the test checks.
    let taken = allocator.join().unwrap();
    drop((held, taken));
    assert!(arena_fd(arena).is_none(), "the arena stayed");
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
    keep_spare_chunks(false);
    let (going, waiting) = (create_arena(), create_arena());
    // A chunk of the arena's, which goes back as the page is freed.
    let page = alloc(going, Wait::No).unwrap();
    set_total_cap(Some(arenas_bytes()));
    let free = count_free_pages();
    let freer = thread::spawn(move || drop(page));
    // The page counted free under the lock it is put back under, so its chunk is going back, or
    // gone, once this takes the lock.
    while count_free_pages() == free {
        thread::yield_now();
    }
    let taken = alloc(waiting, Wait::No);
    freer.join().unwrap();
    let page = taken.expect("a page, as the chunk went back");
    drop(page);
    set_total_cap(None);
    for arena in [going, waiting] {
        remove_arena(arena);
        assert!(arena_fd(arena).is_none(), "{arena:?} stayed");
    }
}

#[test]
fn allocating_races_a_chunk_going_back_in_any_order() {
    check(allocating_races_a_chunk_going_back);
}

/// TASK-92.35: an arena removed as the last of its pages is freed goes, and its object, once its
/// chunks are back, its spare and the page's, whichever goes back first.
fn freeing_races_removing_the_arena() {
    let arena = create_arena();
    // A chunk, and a spare.
    let page = alloc(arena, Wait::No).unwrap();
    let freer = thread::spawn(move || drop(page));
    remove_arena(arena);
    freer.join().unwrap();
    assert_eq!(arena_bytes(arena), 0);
    assert!(arena_fd(arena).is_none(), "the arena stayed");
}

#[test]
fn freeing_races_removing_the_arena_in_any_order() {
    check(freeing_races_removing_the_arena);
}

/// A second stage of another process, as the extent daemon's connection to a VM process is: it maps
/// pages as they are mapped, but makes the changes taking them away later, on a thread of its own
/// ([`LaggingStage::make`]), as the VM process makes them: what a guest reads through it meanwhile
/// is what the pages hold.
#[derive(Default)]
struct LaggingStage(parking_lot::Mutex<Lagging>);

#[derive(Default)]
struct Lagging {
    /// What the guest reaches: where each page's memory is.
    mapped: std::collections::BTreeMap<usize, PhysLocation>,
    /// The unmaps sent, not made yet, and how many were sent, and made.
    unmaps: std::collections::VecDeque<usize>,
    sent: u64,
    made: u64,
    /// The pages freed, quarantined until the unmaps sent before are made.
    parked: Vec<(u64, Quarantined)>,
}

impl LaggingStage {
    /// Maps the page at `location` at `vaddr`.
    fn map(&self, vaddr: usize, location: PhysLocation) {
        self.0.lock().mapped.insert(vaddr, location);
    }

    /// Sends the unmap of the page at `vaddr`, which it makes later.
    fn unmap(&self, vaddr: usize) {
        let mut stage = self.0.lock();
        stage.unmaps.push_back(vaddr);
        stage.sent += 1;
    }

    /// Makes the oldest unmap not made yet, and lets go of the pages that waited for it: whether
    /// there was one.
    fn make(&self) -> bool {
        let released: Vec<_> = {
            let mut stage = self.0.lock();
            let Some(vaddr) = stage.unmaps.pop_front() else { return false };
            stage.mapped.remove(&vaddr);
            stage.made += 1;
            let made = stage.made;
            stage.parked.extract_if(.., |(sent, _)| *sent <= made).collect()
        };
        // Freed without the stage's lock, which a quarantine takes.
        drop(released);
        true
    }

    /// Quarantines `pages` until the unmaps sent so far are made.
    fn park(&self, pages: Quarantined) {
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
        let arena = arena_fd(page.arena).expect("the page's arena");
        let (len, prot) = (page_bytes(), libc::PROT_READ);
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
fn lagging_quarantine() -> crate::pmm::quarantine::QuarantineForTest {
    crate::pmm::quarantine::quarantine_for_test(|pages| {
        LAGGING.lock().unwrap().as_ref().expect("a schedule's stage").park(pages)
    })
}

/// What a page freed of [`freeing_races_the_second_stage`] holds, and what its next holder writes.
const FREED: u64 = 0xf7ee_d000_f7ee_d000;
const REUSED: u64 = 0x0e05_ed00_0e05_ed00;

/// Writes `word` at the start of `page`.
fn write_word(page: &crate::Page, word: u64) {
    // SAFETY: The page is the test's.
    unsafe { page.as_ptr().cast::<u64>().write_volatile(word) };
}

/// TASK-79.9: a page a second stage of another process maps, which makes the changes it is sent
/// after they return, freed, is no other allocation's until the stage has taken it away: a guest
/// reading it through the stage meanwhile reads what it held, never what its next holder writes. A
/// page mapped through a [`LaggingStage`], the mapping sent unmapping and the page dropped, and
/// another allocated and written, as its VM process makes the unmap and its guest reads.
fn freeing_races_the_second_stage() {
    let stage = Arc::new(LaggingStage::default());
    *LAGGING.lock().unwrap() = Some(stage.clone());
    // A page of the chunk the next is of, which keeps it the arena's as the next is freed, and
    // so that page the one allocated next.
    let keeper = alloc(ArenaId::DEFAULT, Wait::No).unwrap();
    let page = alloc(ArenaId::DEFAULT, Wait::No).unwrap();
    write_word(&page, FREED);
    page.mark_staged();
    stage.map(0, page.location());
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
                    assert_eq!(read, FREED, "the guest read a page another holds");
                }
                thread::yield_now();
            }
        })
    };
    stage.unmap(0);
    drop(page);
    let next = alloc(ArenaId::DEFAULT, Wait::No).unwrap();
    write_word(&next, REUSED);
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

/// An allocation that does not wait for a chunk going back fails, out of room under the total cap,
/// though the chunk is about to be free.
#[test]
fn needs_allocations_to_wait_for_chunks_going_back() {
    let rules = Rules { allocations_wait_for_chunks_going_back: false, ..Rules::ALL };
    fails_without(rules, "a page, as the chunk went back", allocating_races_a_chunk_going_back);
}

/// A page freed at once, though a second stage of another process may map it still, is another's
/// that the guest reads.
#[test]
fn needs_staged_pages_to_wait_for_their_stages() {
    let rules = Rules { staged_pages_wait_for_their_stages: false, ..Rules::ALL };
    let (_quarantine, test) = (lagging_quarantine(), freeing_races_the_second_stage);
    fails_without(rules, "the guest read a page another holds", test);
}
