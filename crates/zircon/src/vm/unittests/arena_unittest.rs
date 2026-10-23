// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Tests of physical memory in arenas (`vm::physmap`, TASK-43), which Zircon lacks.

use super::test_helper::make_committed_pager_vmo;
use crate::kernel::deadline::Deadline;
use crate::kernel::event::Event;
use crate::vm::page::{VmPageDoublyLinkedList, VmPagePtr};
use crate::vm::page_queues::PageQueues;
use crate::vm::physmap::{ArenaId, CHUNK_SIZE};
use crate::vm::pmm::{self, ALLOC_FLAG_CAN_WAIT};
use crate::vm::vm_object_paged::VmObjectPaged;
use futures::executor::block_on;
use std::os::fd::AsRawFd;
use std::pin::pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use zx_status::Status;

const PAGE_SIZE: u64 = page::SIZE as u64;
const CHUNK_PAGES: usize = CHUNK_SIZE / page::SIZE;

/// The page at `offset` in arena `arena`'s shared memory object, as another process mapping the
/// object sees it.
fn read_arena(arena: ArenaId, offset: u64) -> Vec<u8> {
    let fd = pmm::node().phys_memory().arena_fd(arena).unwrap();
    // SAFETY: Maps a page of the object, read-only, which is unmapped below.
    let addr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            page::SIZE,
            libc::PROT_READ,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            offset as libc::off_t,
        )
    };
    assert_ne!(addr, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
    // SAFETY: The mapping is a page long.
    let page = unsafe { std::slice::from_raw_parts(addr.cast::<u8>(), page::SIZE) }.to_vec();
    // SAFETY: Unmaps the mapping made above.
    unsafe { libc::munmap(addr, page::SIZE) };
    page
}

/// A VMO's pages come from the arena current when it was created, whose object holds them, and
/// another arena's VMO's from that one.
#[test]
fn vmos_take_pages_from_their_arena() {
    let arena = pmm::create_arena();
    let ours = pmm::with_arena(arena, || VmObjectPaged::create(0, 0, 2 * PAGE_SIZE)).unwrap();
    let default = VmObjectPaged::create(0, 0, 2 * PAGE_SIZE).unwrap();
    ours.write(&[0xa5; page::SIZE], PAGE_SIZE).unwrap();
    default.write(&[0x5a; page::SIZE], PAGE_SIZE).unwrap();

    let phys = pmm::node().phys_memory();
    let our_page = ours.debug_get_page(PAGE_SIZE).unwrap();
    let our_location = phys.location(our_page.paddr());
    assert_eq!(our_location.arena, arena);
    assert_eq!(read_arena(arena, our_location.offset), [0xa5; page::SIZE]);
    let default_page = default.debug_get_page(PAGE_SIZE).unwrap();
    let default_location = phys.location(default_page.paddr());
    assert_eq!(default_location.arena, ArenaId::DEFAULT);
    assert_eq!(read_arena(ArenaId::DEFAULT, default_location.offset), [0x5a; page::SIZE]);
    // Neither page is in the other's chunk.
    assert_ne!(phys.chunk_of(our_page.paddr()), phys.chunk_of(default_page.paddr()));
}

/// An arena takes a chunk for its first page, and gives it back, its memory freed, once all of
/// its pages are free; the chunk then serves another arena.
#[test]
fn chunks_go_back_once_their_pages_are_free() {
    let (first, second) = (pmm::create_arena(), pmm::create_arena());
    let node = pmm::node();
    let phys = node.phys_memory();
    let free_before = node.count_free_pages();
    let mut pages: Vec<VmPagePtr> =
        (0..CHUNK_PAGES).map(|_| node.alloc_page_in(first, 0).unwrap()).collect();
    let chunk = phys.chunk_of(pages[0].paddr());
    assert!(pages.iter().all(|page| phys.chunk_of(page.paddr()) == chunk), "one chunk");
    assert_eq!(phys.chunk_arena(chunk), Some(first));
    assert_eq!(node.count_free_pages(), free_before - CHUNK_PAGES as u64);
    // SAFETY: Writes a page this test owns.
    unsafe { *crate::vm::physmap::paddr_to_physmap(pages[0].paddr()) = 7 };

    let last = pages.pop().unwrap();
    for page in pages {
        // SAFETY: The test owns the page.
        unsafe { pmm::free_page(page) };
    }
    assert_eq!(phys.chunk_arena(chunk), Some(first), "a page of it is allocated");
    // SAFETY: As above.
    unsafe { pmm::free_page(last) };
    assert_eq!(phys.chunk_arena(chunk), None);
    assert_eq!(node.count_free_pages(), free_before);

    // The next arena to take a chunk takes it, freed chunks going last in, first out, with
    // nothing of the first's in it.
    let page = node.alloc_page_in(second, 0).unwrap();
    assert_eq!(phys.chunk_of(page.paddr()), chunk);
    assert_eq!(phys.chunk_arena(chunk), Some(second));
    // SAFETY: Reads a page this test owns.
    assert_eq!(unsafe { *crate::vm::physmap::paddr_to_physmap(page.paddr()) }, 0);
    pmm::remove_arena(first);
}

/// An arena removed while pages of it are allocated stays, its memory with them, until the last
/// of them is freed and its chunk goes back; one holding none goes at once.
#[test]
fn removed_arenas_go_once_their_chunks_do() {
    let (empty, holding) = (pmm::create_arena(), pmm::create_arena());
    let node = pmm::node();
    let phys = node.phys_memory();
    pmm::remove_arena(empty);
    assert!(phys.arena_fd(empty).is_none());

    let pages: Vec<VmPagePtr> = (0..2).map(|_| node.alloc_page_in(holding, 0).unwrap()).collect();
    let chunk = phys.chunk_of(pages[0].paddr());
    pmm::remove_arena(holding);
    assert!(phys.arena_fd(holding).is_some(), "removed with pages allocated");
    let [first, last] = pages.try_into().unwrap();
    // SAFETY: The test owns the pages.
    unsafe { pmm::free_page(first) };
    assert!(phys.arena_fd(holding).is_some(), "removed with a page allocated");
    // SAFETY: As above.
    unsafe { pmm::free_page(last) };
    assert_eq!(phys.chunk_arena(chunk), None);
    assert!(phys.arena_fd(holding).is_none());
}

/// Boots the kernel with the memory watchdog's thread never started (`oom_enabled` off), for a
/// test that sets the node's free memory signal itself (`PmmNode::set_free_memory_signal`), and
/// with it the level its allocations are delayed at, as the thread does, the signal's only setter
/// in the kernel; the watchdog itself is still created with the node. Started, the thread sets its
/// own signal as it first waits for memory to change, at whatever point of the test it gets to
/// then: its level, which never delays allocations, lifts the test's delay, and opens the node's
/// gate. nextest runs each test in its own process, so the kernel has not started.
fn boot_without_watchdog_thread() {
    let options = crate::boot_options::BootOptions { oom_enabled: false, ..Default::default() };
    crate::boot_options::BootOptions::set(options).expect("the kernel has not started");
}

/// Asserts that every way of allocating a page of `arena`, removed, fails with `NO_MEMORY` at
/// once, as nothing makes a removed arena room, whether or not the allocation may wait, and while
/// the node delays allocations that may, taking no chunk; and that the node's gate still lets the
/// next waiter by. Its test boots with the watchdog's thread never started
/// (`boot_without_watchdog_thread`), which would lift the delay.
fn assert_gives_no_page(arena: ArenaId) {
    let booted = crate::boot_options::BootOptions::get();
    assert!(!booted.oom_enabled, "the watchdog's thread lifts the delay");
    let node = pmm::node();
    let free_chunks = node.count_free_chunks();
    let mut list = VmPageDoublyLinkedList::new();
    for flags in [0, ALLOC_FLAG_CAN_WAIT] {
        assert_eq!(node.alloc_page_in(arena, flags).err(), Some(Status::NO_MEMORY), "{flags}");
        let pages = node.alloc_pages_in(arena, 2, flags, &mut list);
        assert_eq!(pages, Err(Status::NO_MEMORY), "{flags}");
    }
    // Passed: a wait would time out.
    let now = Deadline::after_mono(0);
    let waited = node.wait_for_single_page_allocation_in(arena, now);
    assert_eq!(waited.err(), Some(Status::NO_MEMORY));
    let waited = block_on(node.wait_for_single_page_allocation_async(arena, now));
    assert_eq!(waited.err(), Some(Status::NO_MEMORY));

    // Until its level is reset, the node delays allocations that may wait.
    let event = Arc::new(Event::new(false));
    assert!(node.set_free_memory_signal(0, u64::MAX, u64::MAX, event.clone()));
    let delayed = node.alloc_page_in(arena, ALLOC_FLAG_CAN_WAIT);
    assert_eq!(delayed.err(), Some(Status::NO_MEMORY));
    let waited = node.wait_for_single_page_allocation_in(arena, now);
    assert_eq!(waited.err(), Some(Status::NO_MEMORY));
    // The delay held, as a removed arena's allocations fail alike without it: the default arena's
    // wait times out at the gate.
    let waited = node.wait_for_single_page_allocation(now);
    assert_eq!(waited.err(), Some(Status::TIMED_OUT), "the delay lifted");
    assert!(node.set_free_memory_signal(0, u64::MAX, 0, event));
    assert_eq!(node.count_free_chunks(), free_chunks, "a chunk taken");

    let next = node.wait_for_single_page_allocation(now).expect("the gate lets the next by");
    // SAFETY: The test owns the page.
    unsafe { pmm::free_page(next) };
}

/// TASK-79.36: an arena removed holding no chunk, gone, gives no page, and takes no chunk.
#[test]
fn arenas_gone_give_no_page() {
    boot_without_watchdog_thread();
    let node = pmm::node();
    let arena = pmm::create_arena();
    pmm::remove_arena(arena);
    assert_gives_no_page(arena);
    assert!(node.phys_memory().arena_fd(arena).is_none());
}

/// TASK-79.36: an arena removed while a page of it is allocated gives no page either, of its
/// chunk's free pages or of a chunk it would take, nor takes a spare, until it goes with its last
/// chunk.
#[test]
fn retiring_arenas_give_no_page() {
    boot_without_watchdog_thread();
    let node = pmm::node();
    let arena = pmm::create_arena();
    let page = node.alloc_page_in(arena, 0).unwrap();
    pmm::remove_arena(arena);
    assert_eq!(node.arena_bytes(arena), CHUNK_SIZE as u64, "its spare went back");
    assert_gives_no_page(arena);
    assert_eq!(node.arena_bytes(arena), CHUNK_SIZE as u64);
    // SAFETY: The test owns the page.
    unsafe { pmm::free_page(page) };
    assert_eq!(node.arena_bytes(arena), 0);
    assert!(node.phys_memory().arena_fd(arena).is_none(), "gone with its last chunk");
}

/// An arena with no free page of its own, when every chunk is another's, waits for one to go
/// back, and takes it then.
#[test]
fn starved_arenas_wait_for_a_chunk() {
    let (hog, starved) = (pmm::create_arena(), pmm::create_arena());
    let node = pmm::node();
    let phys = node.phys_memory();
    // The hog takes every chunk, a page of each.
    let mut held = Vec::new();
    let mut spare = Vec::new();
    while let Ok(page) = node.alloc_page_in(hog, 0) {
        if held.last().is_none_or(|last: &VmPagePtr| {
            phys.chunk_of(last.paddr()) != phys.chunk_of(page.paddr())
        }) {
            held.push(page);
        } else {
            spare.push(page);
        }
    }
    // Free pages, all the hog's: its chunks', which hold a page of its each.
    for page in spare {
        // SAFETY: The test owns the page.
        unsafe { pmm::free_page(page) };
    }
    assert!(node.count_free_pages() > 0);
    assert_eq!(node.alloc_page_in(starved, 0), Err(Status::NO_MEMORY));
    assert_eq!(node.alloc_page_in(starved, ALLOC_FLAG_CAN_WAIT), Err(Status::SHOULD_WAIT));

    let waiter = std::thread::spawn(move || {
        let deadline = Deadline::after_mono(10_000_000_000);
        loop {
            match pmm::node().wait_for_single_page_allocation_in(starved, deadline) {
                Err(Status::SHOULD_WAIT) => continue,
                result => break result.map(|page| page.paddr()),
            }
        }
    });
    let freed = held.pop().unwrap();
    let chunk = phys.chunk_of(freed.paddr());
    // SAFETY: As above.
    unsafe { pmm::free_page(freed) };
    let paddr = waiter.join().unwrap().expect("allocated once a chunk went back");
    assert_eq!(phys.chunk_of(paddr), chunk);
    assert_eq!(phys.chunk_arena(chunk), Some(starved));
}

/// One arena's cache is evicted for another's allocation: with every chunk taken by one arena's
/// clean, pager-backed pages, an allocation of another arena's waits while eviction frees pages
/// until a chunk goes back, and then takes it.
#[test]
fn starved_arenas_take_chunks_eviction_frees() {
    const CHUNKS: usize = 16;
    let options = crate::boot_options::BootOptions {
        memory_bytes: (CHUNKS * CHUNK_SIZE) as u64,
        ..Default::default()
    };
    crate::boot_options::BootOptions::set(options).expect("the kernel has not started");
    let _scanner = crate::vm::scanner::AutoVmScannerDisable::new();
    let node = pmm::node();
    // Eviction alone frees a chunk here: no arena keeps a spare one to take instead.
    node.keep_spare_chunks(false);
    let phys = node.phys_memory();
    let (cache, starved) = (pmm::create_arena(), pmm::create_arena());

    // The default arena has a chunk, with free pages, which no other arena may take; every
    // chunk no arena has yet holds the cache's pages.
    let _default = node.alloc_page(0).unwrap();
    let cached = node.count_free_chunks() * CHUNK_PAGES;
    let (vmo, pages) =
        pmm::with_arena(cache, || make_committed_pager_vmo(cached, false, false)).unwrap();
    assert!(pages.iter().all(|page| phys.location(page.paddr()).arena == cache));
    assert_eq!(node.alloc_page_in(starved, 0), Err(Status::NO_MEMORY), "no chunk is free");

    // Old enough to evict.
    let page_queues = node.page_queues();
    for _ in 0..100 {
        if pages.iter().all(|page| PageQueues::is_page_reclaimable(*page)) {
            break;
        }
        page_queues.rotate_reclaim_queues();
        drop(page_queues.peek_isolate(0));
    }
    node.evictor().enable_eviction(false);

    let deadline = Deadline::after_mono(10_000_000_000);
    let page = loop {
        match node.wait_for_single_page_allocation_in(starved, deadline) {
            Err(Status::SHOULD_WAIT) => continue,
            result => break result.expect("allocated once eviction freed a chunk"),
        }
    };
    let chunk = phys.chunk_of(page.paddr());
    assert_eq!(phys.chunk_arena(chunk), Some(starved));
    let evicted = (0..cached as u64).filter(|i| vmo.debug_get_page(i * PAGE_SIZE).is_none());
    assert!(evicted.count() >= CHUNK_PAGES, "evicted less than a chunk's worth");
}

/// Pages supplied to a pager-backed object of an arena from another's are copied into pages of its
/// own, and the others cleared and freed.
#[test]
fn supplied_pages_are_copied_into_the_objects_arena() {
    let arena = pmm::create_arena();
    let (vmo, _) = pmm::with_arena(arena, || {
        super::test_helper::make_partially_committed_pager_vmo(2, 0, false, false, true)
    })
    .unwrap();
    let len = 2 * PAGE_SIZE;
    let buffer = VmObjectPaged::create(0, 0, len).unwrap();
    buffer.write(&[0x11; page::SIZE], 0).unwrap();
    buffer.write(&[0x22; page::SIZE], PAGE_SIZE).unwrap();
    let phys = pmm::node().phys_memory();
    let originals: Vec<u64> = (0..2)
        .map(|index| phys.location(buffer.debug_get_page(index * PAGE_SIZE).unwrap().paddr()))
        .inspect(|location| assert_eq!(location.arena, ArenaId::DEFAULT))
        .map(|location| location.offset)
        .collect();
    let mut pages = crate::vm::vm_page_list::VmPageSpliceList::new();
    buffer.take_pages(0, len, &mut pages).unwrap();
    let free_before = pmm::node().count_free_pages();

    vmo.supply_pages(0, len, &mut pages, crate::vm::vm_cow_pages::SupplyOptions::PagerSupply)
        .unwrap();
    for (index, byte) in [(0, 0x11u8), (1, 0x22)] {
        let page = vmo.debug_get_page(index * PAGE_SIZE).unwrap();
        assert_eq!(phys.location(page.paddr()).arena, arena);
        let mut data = vec![0; page::SIZE];
        vmo.read(&mut data, index * PAGE_SIZE).unwrap();
        assert_eq!(data, [byte; page::SIZE]);
    }
    // The buffer's pages went, as the copies came, holding nothing of the arena's data.
    assert_eq!(pmm::node().count_free_pages(), free_before);
    for offset in originals {
        assert_eq!(read_arena(ArenaId::DEFAULT, offset), [0; page::SIZE]);
    }
}

/// Pages supplied to an object of the default arena from another's are copied into the default
/// arena's, as any other arena's object's are, and the others cleared and freed.
#[test]
fn supplied_pages_are_copied_into_the_default_arena() {
    let arena = pmm::create_arena();
    let (vmo, _) =
        super::test_helper::make_partially_committed_pager_vmo(2, 0, false, false, true).unwrap();
    let len = 2 * PAGE_SIZE;
    // A page more than is supplied, which stays the buffer's, so that the arena keeps the chunk
    // the others are in when they are freed, and with it what they hold.
    let buffer = pmm::with_arena(arena, || VmObjectPaged::create(0, 0, len + PAGE_SIZE)).unwrap();
    for (index, byte) in [(0, 0x11u8), (1, 0x22), (2, 0x33)] {
        buffer.write(&[byte; page::SIZE], index * PAGE_SIZE).unwrap();
    }
    let phys = pmm::node().phys_memory();
    let originals: Vec<u64> = (0..2)
        .map(|index| phys.location(buffer.debug_get_page(index * PAGE_SIZE).unwrap().paddr()))
        .inspect(|location| assert_eq!(location.arena, arena))
        .map(|location| location.offset)
        .collect();
    let mut pages = crate::vm::vm_page_list::VmPageSpliceList::new();
    buffer.take_pages(0, len, &mut pages).unwrap();
    let free_before = pmm::node().count_free_pages();

    vmo.supply_pages(0, len, &mut pages, crate::vm::vm_cow_pages::SupplyOptions::PagerSupply)
        .unwrap();
    for (index, byte) in [(0, 0x11u8), (1, 0x22)] {
        let page = vmo.debug_get_page(index * PAGE_SIZE).unwrap();
        assert_eq!(phys.location(page.paddr()).arena, ArenaId::DEFAULT);
        let mut data = vec![0; page::SIZE];
        vmo.read(&mut data, index * PAGE_SIZE).unwrap();
        assert_eq!(data, [byte; page::SIZE]);
    }
    assert_eq!(pmm::node().count_free_pages(), free_before);
    for offset in originals {
        assert_eq!(read_arena(arena, offset), [0; page::SIZE]);
    }
}

/// Pages supplied to an object from its own arena are taken as they are: the object holds the very
/// pages supplied.
#[test]
fn supplied_pages_of_the_objects_arena_are_taken_as_they_are() {
    let arena = pmm::create_arena();
    let (vmo, _) = pmm::with_arena(arena, || {
        super::test_helper::make_partially_committed_pager_vmo(2, 0, false, false, true)
    })
    .unwrap();
    let len = 2 * PAGE_SIZE;
    let buffer = pmm::with_arena(arena, || VmObjectPaged::create(0, 0, len)).unwrap();
    buffer.commit_range(0, len).unwrap();
    let supplied: Vec<_> =
        (0..2).map(|index| buffer.debug_get_page(index * PAGE_SIZE).unwrap().paddr()).collect();
    let mut pages = crate::vm::vm_page_list::VmPageSpliceList::new();
    buffer.take_pages(0, len, &mut pages).unwrap();

    vmo.supply_pages(0, len, &mut pages, crate::vm::vm_cow_pages::SupplyOptions::PagerSupply)
        .unwrap();
    let held: Vec<_> =
        (0..2).map(|index| vmo.debug_get_page(index * PAGE_SIZE).unwrap().paddr()).collect();
    assert_eq!(held, supplied);
}

/// An arena keeps a spare chunk, which it takes its pages from once its others' are allocated,
/// and takes another then; another arena takes it when no chunk is free; and it goes back with its
/// arena (TASK-26.18.3).
#[test]
fn arenas_keep_a_spare_chunk() {
    let (arena, other) = (pmm::create_arena(), pmm::create_arena());
    let node = pmm::node();
    let phys = node.phys_memory();
    let free_chunks = node.count_free_chunks();
    let first = node.alloc_page_in(arena, 0).unwrap();
    let chunk = phys.chunk_of(first.paddr());
    assert_eq!(node.count_free_chunks(), free_chunks - 2, "a chunk, and a spare");
    let mut pages: Vec<VmPagePtr> =
        (1..CHUNK_PAGES).map(|_| node.alloc_page_in(arena, 0).unwrap()).collect();
    assert!(pages.iter().all(|page| phys.chunk_of(page.paddr()) == chunk), "its first chunk first");
    pages.push(first);

    // The spare's first page: another spare taken.
    let spare_page = node.alloc_page_in(arena, 0).unwrap();
    let spare = phys.chunk_of(spare_page.paddr());
    assert_ne!(spare, chunk);
    assert_eq!(phys.chunk_arena(spare), Some(arena));
    assert_eq!(node.count_free_chunks(), free_chunks - 3);
    pages.push(spare_page);
    for page in pages {
        // SAFETY: The test owns the page.
        unsafe { pmm::free_page(page) };
    }
    assert_eq!(node.count_free_chunks(), free_chunks - 1, "the spare stays");

    // Removed, the arena gives its spare back.
    pmm::remove_arena(arena);
    assert_eq!(node.count_free_chunks(), free_chunks);
    assert!(phys.arena_fd(arena).is_none());
    let _ = other;
}

/// With no chunk free, an arena takes another's spare.
#[test]
fn starved_arenas_take_a_spare_chunk() {
    const CHUNKS: usize = 4;
    let options = crate::boot_options::BootOptions {
        memory_bytes: (CHUNKS * CHUNK_SIZE) as u64,
        ..Default::default()
    };
    crate::boot_options::BootOptions::set(options).expect("the kernel has not started");
    let node = pmm::node();
    let phys = node.phys_memory();
    let (hog, starved) = (pmm::create_arena(), pmm::create_arena());
    // The hog takes pages until no chunk is free, the last it took its spare.
    let mut held = Vec::new();
    while node.count_free_chunks() > 0 {
        held.push(node.alloc_page_in(hog, 0).unwrap());
    }
    let spare = (0..CHUNKS)
        .find(|&chunk| {
            phys.chunk_arena(chunk) == Some(hog)
                && held.iter().all(|page| phys.chunk_of(page.paddr()) != chunk)
        })
        .expect("the hog has a spare chunk");
    let page = node.alloc_page_in(starved, 0).expect("the hog's spare, taken");
    assert_eq!(phys.chunk_of(page.paddr()), spare);
    assert_eq!(phys.chunk_arena(spare), Some(starved));
}

/// Runs `f` with the host mapping no chunk of `arena`'s, as when the process makes as many mappings
/// as it may (`vm.max_map_count`): the arena's object made a read-only file under the kernel, which
/// the host does not map writable. The object must have a slot free for each chunk `f` maps: on
/// Linux, the object grown for one, which it can't be meanwhile, would fail otherwise.
fn with_chunks_unmapped<R>(arena: ArenaId, f: impl FnOnce() -> R) -> R {
    let object = pmm::node().phys_memory().arena_fd(arena).unwrap();
    let writable = object.try_clone().unwrap();
    let read_only = std::fs::File::open(std::env::current_exe().unwrap()).unwrap();
    // SAFETY: Makes the descriptor of the arena's object a read-only file's, until it is made the
    // object's again below.
    let made = unsafe { libc::dup2(read_only.as_raw_fd(), object.as_raw_fd()) };
    assert_eq!(made, object.as_raw_fd(), "dup2: {}", std::io::Error::last_os_error());
    let result = f();
    // SAFETY: Makes the descriptor the arena's object's again.
    let made = unsafe { libc::dup2(writable.as_raw_fd(), object.as_raw_fd()) };
    assert_eq!(made, object.as_raw_fd(), "dup2: {}", std::io::Error::last_os_error());
    result
}

/// TASK-79.37: a chunk the host does not map stays free, and so does the slot of the arena's
/// object it would have taken: every way of allocating the page it was for fails with `NO_MEMORY`
/// at once, whether or not it may wait, leaving the node's gate to the next waiter; and the next
/// allocation, once the host maps it, takes both.
#[test]
fn chunks_the_host_does_not_map_stay_free() {
    let node = pmm::node();
    node.keep_spare_chunks(false);
    let phys = node.phys_memory();
    let arena = pmm::create_arena();
    // The arena's object's slot free, which its next chunk takes.
    let page = node.alloc_page_in(arena, 0).unwrap();
    // SAFETY: The test owns the page.
    unsafe { pmm::free_page(page) };
    let free_chunks = node.count_free_chunks();

    let now = Deadline::after_mono(0);
    with_chunks_unmapped(arena, || {
        let mut list = VmPageDoublyLinkedList::new();
        for flags in [0, ALLOC_FLAG_CAN_WAIT] {
            assert_eq!(node.alloc_page_in(arena, flags).err(), Some(Status::NO_MEMORY), "{flags}");
            let pages = node.alloc_pages_in(arena, 2, flags, &mut list);
            assert_eq!(pages, Err(Status::NO_MEMORY), "{flags}");
        }
        let waited = node.wait_for_single_page_allocation_in(arena, now);
        assert_eq!(waited.err(), Some(Status::NO_MEMORY));
    });
    assert_eq!(node.count_free_chunks(), free_chunks, "a chunk lost");
    assert_eq!(node.arena_bytes(arena), 0);

    let page = node.alloc_page_in(arena, 0).expect("a chunk the host maps");
    assert_eq!(phys.location(page.paddr()).offset, 0, "the slot, free still");
    assert_eq!(node.count_free_chunks(), free_chunks - 1);
    let next = node.wait_for_single_page_allocation(now).expect("the gate lets the next by");
    for page in [page, next] {
        // SAFETY: As above.
        unsafe { pmm::free_page(page) };
    }
}

/// TASK-79.37: an arena starting on its spare chunk while the host maps no chunk takes its page,
/// and does without a spare. Nor do arenas ask the host for one again, which it would refuse too,
/// each allocation in vain, until it maps, or takes back, a chunk: then the arena takes its spare.
#[test]
fn spares_the_host_does_not_map_are_done_without() {
    let node = pmm::node();
    let phys = node.phys_memory();
    let (arena, other) = (pmm::create_arena(), pmm::create_arena());
    // A chunk and a spare; the chunk gone back, its slot of the arena's object free, which the
    // arena's next spare takes.
    let page = node.alloc_page_in(arena, 0).unwrap();
    // SAFETY: The test owns the page.
    unsafe { pmm::free_page(page) };
    assert_eq!(node.arena_bytes(arena), CHUNK_SIZE as u64, "its spare");
    let free_chunks = node.count_free_chunks();

    let page =
        with_chunks_unmapped(arena, || node.alloc_page_in(arena, 0)).expect("a page of its spare");
    assert_eq!(node.arena_bytes(arena), CHUNK_SIZE as u64, "a spare taken");
    assert_eq!(node.count_free_chunks(), free_chunks, "a chunk lost");

    // The host would map one now, and is not asked to.
    let next = node.alloc_page_in(arena, 0).unwrap();
    assert_eq!(phys.chunk_of(next.paddr()), phys.chunk_of(page.paddr()));
    assert_eq!(node.arena_bytes(arena), CHUNK_SIZE as u64, "a spare asked of a refusing host");
    // It maps a chunk, another arena's.
    let others = node.alloc_page_in(other, 0).unwrap();
    let last = node.alloc_page_in(arena, 0).unwrap();
    assert_eq!(phys.chunk_of(last.paddr()), phys.chunk_of(page.paddr()));
    assert_eq!(node.arena_bytes(arena), 2 * CHUNK_SIZE as u64, "a spare once the host maps one");
    for page in [page, next, others, last] {
        // SAFETY: As above.
        unsafe { pmm::free_page(page) };
    }
}

/// The mappings the process may make beside those it has (`vm.max_map_count`), taken while this
/// lives, until the host maps nothing more: pages of a shared mapping of its own, which merges with
/// none, every other one made readable, each so a mapping of its own, until the host splits it no
/// more; then a page each, shared memory of its own, until it maps no more, as Linux may map one
/// mapping more than it splits one for (`do_mmap` refusing past its limit, `split_vma` at it). The
/// host then maps no chunk, whether its reservation is split around it or shortened, nor reserves
/// one again in the middle of its arena's mapping.
#[cfg(target_os = "linux")]
struct MappingsTaken {
    split: *mut libc::c_void,
    len: usize,
    pages: Vec<*mut libc::c_void>,
}

#[cfg(target_os = "linux")]
impl MappingsTaken {
    fn new() -> Self {
        let max = std::fs::read_to_string("/proc/sys/vm/max_map_count").unwrap();
        // Two pages a mapping: more than the process may make.
        let len = 2 * max.trim().parse::<usize>().unwrap() * page::SIZE;
        let split = map_shared(len).expect("a mapping to split");
        // The thread's heap grown, and its memory freed, before the mappings run out: what the
        // thread allocates meanwhile, as the PMM logs, it takes from the heap as it is, rather than
        // grow it, which the host would refuse, and the process abort.
        drop(std::hint::black_box((0..16).map(|_| vec![1u8; 64 << 10]).collect::<Vec<_>>()));
        // Each page made readable splits what is left of the mapping: two mappings more, until the
        // host refuses, at its limit.
        let mut offset = page::SIZE;
        // SAFETY: Changes the protection of a page of the mapping, which this owns.
        while unsafe { libc::mprotect(split.byte_add(offset), page::SIZE, libc::PROT_READ) } == 0 {
            offset += 2 * page::SIZE;
        }
        let error = std::io::Error::last_os_error();
        assert_eq!(error.raw_os_error(), Some(libc::ENOMEM), "mprotect: {error}");
        assert!(offset < len, "the mapping is too small to take every mapping");
        // Room for the few the host may map still, so that none grows the heap.
        let mut pages = Vec::with_capacity(16);
        let refused = (0..pages.capacity()).find_map(|_| match map_shared(page::SIZE) {
            Ok(page) => {
                pages.push(page);
                None
            }
            Err(error) => Some(error),
        });
        let refused = refused.unwrap_or_else(|| panic!("the host maps on: {} pages", pages.len()));
        assert_eq!(refused.raw_os_error(), Some(libc::ENOMEM), "mmap: {refused}");
        Self { split, len, pages }
    }
}

/// A new shared mapping of `len` bytes, of memory of its own, which merges with no other, never
/// touched.
#[cfg(target_os = "linux")]
fn map_shared(len: usize) -> std::io::Result<*mut libc::c_void> {
    // SAFETY: Maps memory of its own, with no address hint.
    let addr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_NONE,
            libc::MAP_SHARED | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
            -1,
            0,
        )
    };
    if addr == libc::MAP_FAILED { Err(std::io::Error::last_os_error()) } else { Ok(addr) }
}

#[cfg(target_os = "linux")]
impl Drop for MappingsTaken {
    fn drop(&mut self) {
        // SAFETY: Unmaps the mappings, which nothing else uses: whole mappings, none split.
        let status = unsafe { libc::munmap(self.split, self.len) };
        assert_eq!(status, 0, "munmap: {}", std::io::Error::last_os_error());
        for &page in &self.pages {
            // SAFETY: As above.
            let status = unsafe { libc::munmap(page, page::SIZE) };
            assert_eq!(status, 0, "munmap: {}", std::io::Error::last_os_error());
        }
    }
}

/// TASK-79.37: a chunk the host does not reserve again as it goes back, as reserving it splits
/// the mapping of its arena's chunks around it when the process makes as many mappings as it may
/// (`vm.max_map_count`), stays its arena's, its pages free, as its spare, and goes back with the
/// spares once the host takes it. Meanwhile another arena takes no chunk, failing at once: neither
/// one free, which the host does not map, nor, at the total cap, that spare, which it does not take
/// back.
#[cfg(target_os = "linux")]
#[test]
fn chunks_the_host_does_not_take_back_stay_their_arenas() {
    let node = pmm::node();
    node.keep_spare_chunks(false);
    let phys = node.phys_memory();
    let (arena, other) = (pmm::create_arena(), pmm::create_arena());
    // Three chunks, next to one another, in the first three slots of the arena's object: one
    // mapping, which the middle one going back splits.
    let mut pages: Vec<VmPagePtr> =
        (0..3 * CHUNK_PAGES).map(|_| node.alloc_page_in(arena, 0).unwrap()).collect();
    let chunks: Vec<usize> =
        pages.iter().step_by(CHUNK_PAGES).map(|page| phys.chunk_of(page.paddr())).collect();
    assert_eq!(chunks, [chunks[0], chunks[0] + 1, chunks[0] + 2]);
    let offsets: Vec<u64> =
        pages.iter().step_by(CHUNK_PAGES).map(|page| phys.location(page.paddr()).offset).collect();
    assert_eq!(offsets, [0, CHUNK_SIZE as u64, 2 * CHUNK_SIZE as u64]);
    let free_chunks = node.count_free_chunks();

    let taken = MappingsTaken::new();
    for page in pages.drain(CHUNK_PAGES..2 * CHUNK_PAGES) {
        // SAFETY: The test owns the page.
        unsafe { pmm::free_page(page) };
    }
    assert_eq!(phys.chunk_arena(chunks[1]), Some(arena), "the middle chunk went back");
    assert_eq!(node.arena_bytes(arena), 3 * CHUNK_SIZE as u64);
    assert_eq!(node.alloc_page_in(other, ALLOC_FLAG_CAN_WAIT).err(), Some(Status::NO_MEMORY));
    node.set_total_cap(Some(node.arenas_bytes()));
    assert_eq!(node.alloc_page_in(other, ALLOC_FLAG_CAN_WAIT).err(), Some(Status::NO_MEMORY));
    assert_eq!(phys.chunk_arena(chunks[1]), Some(arena), "the spare went back");
    node.set_total_cap(None);
    assert_eq!(node.count_free_chunks(), free_chunks, "a chunk lost");
    drop(taken);

    node.release_spares(Some(arena));
    assert_eq!(phys.chunk_arena(chunks[1]), None, "the middle chunk, its spare, stayed");
    assert_eq!(node.arena_bytes(arena), 2 * CHUNK_SIZE as u64);
    pages.push(node.alloc_page_in(other, 0).expect("a chunk the host maps"));
    for page in pages {
        // SAFETY: As above.
        unsafe { pmm::free_page(page) };
    }
}

/// TASK-50.3: an arena takes no chunk past its cap: its allocations fail, or wait, once its own
/// pages are taken, until one of them is freed, or its cap lifted.
#[test]
fn arenas_take_no_chunk_past_their_cap() {
    let arena = pmm::create_arena();
    let node = pmm::node();
    node.set_arena_cap(arena, Some(CHUNK_SIZE as u64));
    let mut pages: Vec<VmPagePtr> =
        (0..CHUNK_PAGES).map(|_| node.alloc_page_in(arena, 0).unwrap()).collect();
    assert_eq!(node.arena_bytes(arena), CHUNK_SIZE as u64);
    assert_eq!(node.alloc_page_in(arena, 0).err(), Some(Status::NO_MEMORY));
    assert_eq!(node.alloc_page_in(arena, ALLOC_FLAG_CAN_WAIT).err(), Some(Status::SHOULD_WAIT));

    // SAFETY: The test owns the page.
    unsafe { pmm::free_page(pages.pop().unwrap()) };
    pages.push(node.alloc_page_in(arena, 0).unwrap());
    node.set_arena_cap(arena, None);
    pages.push(node.alloc_page_in(arena, 0).unwrap());
    // Another chunk for the page, and a spare.
    assert_eq!(node.arena_bytes(arena), 3 * CHUNK_SIZE as u64);
    for page in pages {
        // SAFETY: As above.
        unsafe { pmm::free_page(page) };
    }
    pmm::remove_arena(arena);
}

/// TASK-50.3: the arenas together take no chunk past the total cap.
#[test]
fn arenas_take_no_chunk_past_the_total_cap() {
    let node = pmm::node();
    let arena = pmm::create_arena();
    node.set_total_cap(Some(node.arenas_bytes()));
    assert_eq!(node.alloc_page_in(arena, 0).err(), Some(Status::NO_MEMORY));
    node.set_total_cap(None);
    let page = node.alloc_page_in(arena, 0).unwrap();
    // SAFETY: The test owns the page.
    unsafe { pmm::free_page(page) };
    pmm::remove_arena(arena);
}

/// TASK-50.3: a total cap lowered under what the arenas hold, as many of their spare chunks, which
/// nothing uses, go back for an allocation as bring them under it.
#[test]
fn a_total_cap_lowered_under_the_arenas_gives_their_spares_back() {
    let node = pmm::node();
    let arenas = [pmm::create_arena(), pmm::create_arena(), pmm::create_arena()];
    // A chunk for each page, and a spare.
    let mut pages: Vec<VmPagePtr> =
        arenas.iter().map(|&arena| node.alloc_page_in(arena, 0).unwrap()).collect();
    let cap = node.arenas_bytes() - 2 * CHUNK_SIZE as u64;
    node.set_total_cap(Some(cap));
    let arena = pmm::create_arena();
    pages.push(node.alloc_page_in(arena, 0).expect("three spares gone back"));
    assert_eq!(node.arenas_bytes(), cap);
    for page in pages {
        // SAFETY: The test owns the page.
        unsafe { pmm::free_page(page) };
    }
}

/// A waker that counts its wakes.
#[derive(Default)]
struct Woken(AtomicUsize);

impl Wake for Woken {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// TASK-86: an allocation of an arena at its cap but not starved, as it has free pages, waits at
/// the node's gate while the node delays allocations, as any allocation does, and takes its page
/// once the level is reset. Told to wait without its waiting, as it was, its caller tried again at
/// once, and again, for as long as the node delayed allocations.
#[test]
fn arenas_at_a_cap_wait_at_the_gate_while_the_node_delays_allocations() {
    boot_without_watchdog_thread();
    let node = pmm::node();
    let arena = pmm::create_arena();
    node.set_arena_cap(arena, Some(CHUNK_SIZE as u64));
    // A chunk, its free pages but this one's: the arena is at its cap, and not starved.
    let page = node.alloc_page_in(arena, 0).unwrap();
    assert_eq!(node.arena_bytes(arena), CHUNK_SIZE as u64);

    // Until its level is reset, the node delays allocations that may wait.
    let event = Arc::new(Event::new(false));
    assert!(node.set_free_memory_signal(0, u64::MAX, u64::MAX, event.clone()));
    // Passed: a wait times out.
    let now = Deadline::after_mono(0);
    let waited = node.wait_for_single_page_allocation_in(arena, now);
    assert_eq!(waited.err(), Some(Status::TIMED_OUT));
    let woken = Arc::new(Woken::default());
    let waker = Waker::from(woken.clone());
    let mut cx = Context::from_waker(&waker);
    let mut wait = pin!(node.wait_for_single_page_allocation_async(arena, Deadline::infinite()));
    assert!(wait.as_mut().poll(&mut cx).is_pending(), "told to wait without waiting");
    assert!(node.set_free_memory_signal(0, u64::MAX, 0, event));
    assert_eq!(woken.0.load(Ordering::Relaxed), 1, "the level reset woke it");
    let Poll::Ready(next) = wait.as_mut().poll(&mut cx) else { panic!("woken, and waiting") };
    for page in [page, next.expect("a page of the arena's")] {
        // SAFETY: The test owns the page.
        unsafe { pmm::free_page(page) };
    }
}

/// TASK-86: while the node delays allocations, one of an arena starved at its cap waits at the
/// node's gate too, which the level reset opens. Waiting for room of its own instead, as it did,
/// it waited on through the reset: its allocation was delayed before its arena was found starved,
/// and so asked neither relief nor eviction for room.
#[test]
fn starved_arenas_wait_at_the_gate_while_the_node_delays_allocations() {
    boot_without_watchdog_thread();
    let node = pmm::node();
    let arena = pmm::create_arena();
    node.set_arena_cap(arena, Some(CHUNK_SIZE as u64));
    let pages: Vec<VmPagePtr> =
        (0..CHUNK_PAGES).map(|_| node.alloc_page_in(arena, 0).unwrap()).collect();

    // Until its level is reset, the node delays allocations that may wait.
    let event = Arc::new(Event::new(false));
    assert!(node.set_free_memory_signal(0, u64::MAX, u64::MAX, event.clone()));
    let woken = Arc::new(Woken::default());
    let waker = Waker::from(woken.clone());
    let mut wait = pin!(node.wait_for_single_page_allocation_async(arena, Deadline::infinite()));
    assert!(wait.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
    assert!(node.set_free_memory_signal(0, u64::MAX, 0, event));
    assert_eq!(woken.0.load(Ordering::Relaxed), 1, "the level reset woke it");
    for page in pages {
        // SAFETY: The test owns the page.
        unsafe { pmm::free_page(page) };
    }
}

/// Waits for a page of `arena`, as an anonymous page request does, trying again as told to.
fn wait_for_page(arena: ArenaId) -> Result<VmPagePtr, Status> {
    let deadline = Deadline::after_mono(10_000_000_000);
    loop {
        match pmm::node().wait_for_single_page_allocation_in(arena, deadline) {
            Err(Status::SHOULD_WAIT) => continue,
            result => break result,
        }
    }
}

/// TASK-50.3: an arena at its cap is relieved, its allocations waiting meanwhile, by its own pages
/// freed, while relief frees some; once it frees none, they fail, until a page of the arena is
/// freed. At the total's, any arena's pages are.
#[test]
fn relief_frees_an_arenas_pages_then_fails_its_allocations() {
    let node = pmm::node();
    // Spares would be freed for it as relief is not.
    node.keep_spare_chunks(false);
    let arena = pmm::create_arena();
    node.set_arena_cap(arena, Some(CHUNK_SIZE as u64));
    let mut pages: Vec<VmPagePtr> =
        (0..CHUNK_PAGES).map(|_| node.alloc_page_in(arena, 0).unwrap()).collect();
    // What relief is asked to free; and a page of the arena's it frees, once.
    let asked = Arc::new(Mutex::new(Vec::new()));
    let to_free = Arc::new(Mutex::new(pages.pop()));
    pmm::relieve_with({
        let (asked, to_free) = (asked.clone(), to_free.clone());
        move |from, bytes| {
            asked.lock().unwrap().push((from, bytes));
            let Some(page) = to_free.lock().unwrap().take() else { return 0 };
            // SAFETY: The test handed the page over to be freed.
            unsafe { pmm::free_page(page) };
            PAGE_SIZE
        }
    })
    .unwrap();

    assert_eq!(node.alloc_page_in(arena, ALLOC_FLAG_CAN_WAIT).err(), Some(Status::SHOULD_WAIT));
    pages.push(wait_for_page(arena).expect("the page relief freed"));
    assert_eq!(wait_for_page(arena).err(), Some(Status::NO_MEMORY), "nothing left to free");
    let asked_for: Vec<_> = asked.lock().unwrap().drain(..).collect();
    assert_eq!(asked_for, vec![(Some(arena), 2 * CHUNK_SIZE as u64); asked_for.len()]);
    // SAFETY: The test owns the page.
    unsafe { pmm::free_page(pages.pop().unwrap()) };
    pages.push(node.alloc_page_in(arena, 0).expect("a page of the arena freed"));

    node.set_arena_cap(arena, None);
    node.set_total_cap(Some(node.arenas_bytes()));
    let other = pmm::create_arena();
    assert_eq!(wait_for_page(other).err(), Some(Status::NO_MEMORY), "nothing left to free");
    assert_eq!(asked.lock().unwrap().last(), Some(&(None, 2 * CHUNK_SIZE as u64)));
    for page in pages {
        // SAFETY: As above.
        unsafe { pmm::free_page(page) };
    }
}

/// Pager-backed VMOs' clean pages, two chunks of each: another arena's and an arena's older ones,
/// aged, and then the arena's newer ones, which fill the arena to its cap, as no arena keeps a
/// spare chunk. The scanner, which ages pages, is disabled while the test lives, and eviction on.
struct AgedPages {
    arena: ArenaId,
    others: Arc<VmObjectPaged>,
    older: Arc<VmObjectPaged>,
    newer: Arc<VmObjectPaged>,
    _scanner: crate::vm::scanner::AutoVmScannerDisable,
}

impl AgedPages {
    fn new() -> Self {
        let scanner = crate::vm::scanner::AutoVmScannerDisable::new();
        let node = pmm::node();
        node.keep_spare_chunks(false);
        let (arena, other) = (pmm::create_arena(), pmm::create_arena());
        node.set_arena_cap(arena, Some(4 * CHUNK_SIZE as u64));
        let pager_vmo = || make_committed_pager_vmo(2 * CHUNK_PAGES, false, false).unwrap().0;
        let others = pmm::with_arena(other, pager_vmo);
        let older = pmm::with_arena(arena, pager_vmo);
        node.page_queues().rotate_reclaim_queues();
        let newer = pmm::with_arena(arena, pager_vmo);
        assert_eq!(node.arena_bytes(arena), 4 * CHUNK_SIZE as u64);
        node.evictor().enable_eviction(false);
        Self { arena, others, older, newer, _scanner: scanner }
    }

    /// The pages left of the other arena's, the arena's older and its newer.
    fn resident(&self) -> [usize; 3] {
        [&self.others, &self.older, &self.newer].map(|vmo| {
            let pages = 0..2 * CHUNK_PAGES as u64;
            pages.filter(|i| vmo.debug_get_page(i * PAGE_SIZE).is_some()).count()
        })
    }
}

/// TASK-79.76: eviction of an arena's pages on demand evicts its own, the oldest first, as many as
/// it is asked for, whatever their age, and none of another arena's, older though they are.
#[test]
fn arenas_own_pages_are_evicted_on_demand_the_oldest_first() {
    let pages = AgedPages::new();
    let evictor = pmm::node().evictor();
    let evicted = evictor.evict_arena_on_demand(pages.arena, 2 * CHUNK_SIZE as u64);
    assert_eq!(evicted.pager_backed, 2 * CHUNK_PAGES as u64);
    assert_eq!(pages.resident(), [2 * CHUNK_PAGES, 0, 2 * CHUNK_PAGES]);
    let evicted = evictor.evict_arena_on_demand(pages.arena, CHUNK_SIZE as u64);
    assert_eq!(evicted.pager_backed, CHUNK_PAGES as u64);
    assert_eq!(pages.resident(), [2 * CHUNK_PAGES, 0, CHUNK_PAGES]);
}

/// TASK-79.76: an arena at its cap, full of clean pages of its own, which relief's evictor leaves,
/// is relieved by the kernel's evictor freeing them on demand, the oldest first (relief's pass
/// frees two chunks' worth, as many as the older), and none of another arena's, older still. It
/// was exhausted, and its allocation failed.
#[test]
fn relief_evicts_an_arenas_own_clean_pages_its_evictor_leaves() {
    let pages = AgedPages::new();
    let node = pmm::node();
    pmm::relieve_with(|_, _| 0).unwrap();

    let allocated = node.alloc_page_in(pages.arena, ALLOC_FLAG_CAN_WAIT);
    assert_eq!(allocated.err(), Some(Status::SHOULD_WAIT));
    let page = wait_for_page(pages.arena).expect("a page of the arena's, evicted");
    // The older pages, the pass's, may be evicting still.
    let [others, _, newer] = pages.resident();
    assert_eq!([others, newer], [2 * CHUNK_PAGES; 2]);
    // SAFETY: The test owns the page.
    unsafe { pmm::free_page(page) };
}

/// TASK-50.3: relief gone, as its evictor panicked, the allocations it would make room for fail,
/// those waiting for it too.
#[test]
fn allocations_fail_once_relief_is_gone() {
    let node = pmm::node();
    node.keep_spare_chunks(false);
    let arena = pmm::create_arena();
    node.set_arena_cap(arena, Some(CHUNK_SIZE as u64));
    let pages: Vec<VmPagePtr> =
        (0..CHUNK_PAGES).map(|_| node.alloc_page_in(arena, 0).unwrap()).collect();
    pmm::relieve_with(|_, _| panic!("an evictor that fails")).unwrap();
    assert_eq!(node.alloc_page_in(arena, ALLOC_FLAG_CAN_WAIT).err(), Some(Status::SHOULD_WAIT));
    assert_eq!(wait_for_page(arena).err(), Some(Status::NO_MEMORY));
    for page in pages {
        // SAFETY: The test owns the page.
        unsafe { pmm::free_page(page) };
    }
}

/// TASK-50.3: a pager-backed VMO's clean pages are evicted on demand, whatever their age; other
/// VMOs have none to evict.
#[test]
fn clean_pages_of_pager_vmos_are_evicted_on_demand() {
    let (vmo, _pages) = make_committed_pager_vmo(4, false, false).unwrap();
    assert_eq!(vmo.evict_range(0, 4 * PAGE_SIZE), Ok(4));
    assert!(vmo.debug_get_page(0).is_none());
    let anonymous = VmObjectPaged::create(0, 0, PAGE_SIZE).unwrap();
    assert_eq!(anonymous.evict_range(0, PAGE_SIZE), Err(Status::NOT_SUPPORTED));
}
