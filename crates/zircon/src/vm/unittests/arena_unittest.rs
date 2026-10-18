// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Tests of physical memory in arenas (`vm::physmap`, TASK-43), which Zircon lacks.

use super::test_helper::make_committed_pager_vmo;
use crate::kernel::deadline::Deadline;
use crate::vm::page::VmPagePtr;
use crate::vm::page_queues::PageQueues;
use crate::vm::physmap::{ArenaId, CHUNK_SIZE};
use crate::vm::pmm::{self, ALLOC_FLAG_CAN_WAIT};
use crate::vm::vm_object_paged::VmObjectPaged;
use std::os::fd::AsRawFd;
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

/// Pages supplied to a pager-backed object of an arena from another's, as a pager's buffers in
/// the default arena are, are copied into pages of its own, and the others cleared and freed.
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
