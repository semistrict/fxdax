// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Tests of physical memory in arenas (`physmap`, TASK-43), lifted from fxdax's port of the Zircon
//! kernel (`vm::unittests::arena_unittest`): those of the arenas themselves, which allocate pages
//! now rather than VMOs, and starved arenas relieved by relief's evictor rather than the kernel's.

use super::{chunk_arena, chunk_of, chunk_pages, configure_chunks, fill, first_byte, read_arena};
use crate::{
    ArenaId, CHUNK_SIZE, NoMemory, Page, RELIEF_BYTES, Wait, alloc, alloc_after, arena_bytes,
    arena_fd, arenas_bytes, count_free_chunks, count_free_pages, create_arena, default_fd,
    has_alloc_failed_no_mem, keep_spare_chunks, page_bytes, page_size, relieve_with, remove_arena,
    set_arena_cap, set_total_cap,
};
use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const CHUNK: u64 = CHUNK_SIZE as u64;

/// `count` pages of `arena`'s.
fn alloc_pages(arena: ArenaId, count: usize) -> Vec<Page> {
    (0..count).map(|_| alloc(arena, Wait::No).unwrap()).collect()
}

/// An arena's pages are of its object, and another arena's of that one's: what is written to
/// them is what another process mapping the object reads.
#[test]
fn pages_are_of_their_arenas_object() {
    let arena = create_arena();
    let ours = alloc(arena, Wait::No).unwrap();
    let default = alloc(ArenaId::DEFAULT, Wait::No).unwrap();
    fill(&ours, 0xa5);
    fill(&default, 0x5a);

    let our_location = ours.location();
    assert_eq!(our_location.arena, arena);
    assert_eq!(ours.arena(), arena);
    assert_eq!(read_arena(arena, our_location.offset), vec![0xa5; page_bytes()]);
    let default_location = default.location();
    assert_eq!(default_location.arena, ArenaId::DEFAULT);
    assert_eq!(read_arena(ArenaId::DEFAULT, default_location.offset), vec![0x5a; page_bytes()]);
    let default_object = arena_fd(ArenaId::DEFAULT).unwrap();
    assert_eq!(default_fd().as_raw_fd(), default_object.as_raw_fd());
    // Neither page is in the other's chunk.
    assert_ne!(chunk_of(&ours), chunk_of(&default));
}

/// An arena takes a chunk for its first page, and gives it back, its memory freed, once all of
/// its pages are free; the chunk then serves another arena.
#[test]
fn chunks_go_back_once_their_pages_are_free() {
    let (first, second) = (create_arena(), create_arena());
    let free_before = count_free_pages();
    let mut pages = alloc_pages(first, chunk_pages());
    let chunk = chunk_of(&pages[0]);
    assert!(pages.iter().all(|page| chunk_of(page) == chunk), "one chunk");
    assert_eq!(chunk_arena(chunk), Some(first));
    assert_eq!(count_free_pages(), free_before - chunk_pages() as u64);
    fill(&pages[0], 7);

    let last = pages.pop().unwrap();
    drop(pages);
    assert_eq!(chunk_arena(chunk), Some(first), "a page of it is allocated");
    drop(last);
    assert_eq!(chunk_arena(chunk), None);
    assert_eq!(count_free_pages(), free_before);

    // The next arena to take a chunk takes it, freed chunks going last in, first out, with
    // nothing of the first's in it.
    let page = alloc(second, Wait::No).unwrap();
    assert_eq!(chunk_of(&page), chunk);
    assert_eq!(chunk_arena(chunk), Some(second));
    assert_eq!(first_byte(&page), 0);
    remove_arena(first);
}

/// An arena removed while pages of it are allocated stays, its memory with them, until the last
/// of them is freed and its chunk goes back; one holding none goes at once.
#[test]
fn removed_arenas_go_once_their_chunks_do() {
    let (empty, holding) = (create_arena(), create_arena());
    remove_arena(empty);
    assert!(arena_fd(empty).is_none());

    let [first, last]: [Page; 2] = alloc_pages(holding, 2).try_into().unwrap();
    let chunk = chunk_of(&first);
    remove_arena(holding);
    assert!(arena_fd(holding).is_some(), "removed with pages allocated");
    drop(first);
    assert!(arena_fd(holding).is_some(), "removed with a page allocated");
    drop(last);
    assert_eq!(chunk_arena(chunk), None);
    assert!(arena_fd(holding).is_none());
}

/// Asserts that every way of allocating a page of `arena`, removed, fails at once, as nothing
/// makes a removed arena room, whether or not the allocation may wait, taking no chunk; and not as
/// memory out.
fn assert_gives_no_page(arena: ArenaId) {
    let free_chunks = count_free_chunks();
    for wait in [Wait::No, Wait::Yes] {
        assert_eq!(alloc(arena, wait).err(), Some(NoMemory), "{wait:?}");
    }
    assert_eq!(count_free_chunks(), free_chunks, "a chunk taken");
    assert!(!has_alloc_failed_no_mem(), "a removed arena's failure is memory out");
}

/// TASK-79.36: an arena removed holding no chunk, gone, gives no page, and takes no chunk.
#[test]
fn arenas_gone_give_no_page() {
    let arena = create_arena();
    remove_arena(arena);
    assert_gives_no_page(arena);
    assert!(arena_fd(arena).is_none());
}

/// TASK-79.36: an arena removed while a page of it is allocated gives no page either, of its
/// chunk's free pages or of a chunk it would take, nor takes a spare, until it goes with its last
/// chunk.
#[test]
fn retiring_arenas_give_no_page() {
    let arena = create_arena();
    let page = alloc(arena, Wait::No).unwrap();
    remove_arena(arena);
    assert_eq!(arena_bytes(arena), CHUNK, "its spare went back");
    assert_gives_no_page(arena);
    assert_eq!(arena_bytes(arena), CHUNK);
    drop(page);
    assert_eq!(arena_bytes(arena), 0);
    assert!(arena_fd(arena).is_none(), "gone with its last chunk");
}

/// An arena with no free page of its own, when every chunk is another's, fails without relief,
/// and, with it, waits for relief's evictor to free one of the other's pages, any arena's, until a
/// chunk goes back, and takes it then.
#[test]
fn starved_arenas_wait_for_relief_to_free_a_chunk() {
    configure_chunks(16);
    let (hog, starved) = (create_arena(), create_arena());
    // The hog takes every chunk, a page of each.
    let mut held: Vec<Page> = Vec::new();
    let mut spare = Vec::new();
    while let Ok(page) = alloc(hog, Wait::No) {
        if held.last().is_none_or(|last| chunk_of(last) != chunk_of(&page)) {
            held.push(page);
        } else {
            spare.push(page);
        }
    }
    assert_eq!(held.len(), 16);
    // Free pages, all the hog's: its chunks', which hold a page of its each.
    drop(spare);
    assert!(count_free_pages() > 0);
    for wait in [Wait::No, Wait::Yes] {
        assert_eq!(alloc(starved, wait).err(), Some(NoMemory), "{wait:?}, without relief");
    }

    let held = Arc::new(Mutex::new(held));
    let (asked, freed) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(None)));
    relieve_with({
        let (held, asked, freed) = (held.clone(), asked.clone(), freed.clone());
        move |from, bytes| {
            asked.lock().unwrap().push((from, bytes));
            let Some(page) = held.lock().unwrap().pop() else { return 0 };
            *freed.lock().unwrap() = Some(chunk_of(&page));
            drop(page);
            page_size()
        }
    })
    .unwrap();
    let page = alloc(starved, Wait::Yes).expect("allocated once a chunk went back");
    assert_eq!(Some(chunk_of(&page)), *freed.lock().unwrap());
    assert_eq!(chunk_arena(chunk_of(&page)), Some(starved));
    assert_eq!(*asked.lock().unwrap(), [(None, RELIEF_BYTES)]);
}

/// An arena keeps a spare chunk, which it takes its pages from once its others' are allocated,
/// and takes another then; and it goes back with its arena (TASK-26.18.3).
#[test]
fn arenas_keep_a_spare_chunk() {
    let arena = create_arena();
    let free_chunks = count_free_chunks();
    let first = alloc(arena, Wait::No).unwrap();
    let chunk = chunk_of(&first);
    assert_eq!(count_free_chunks(), free_chunks - 2, "a chunk, and a spare");
    let mut pages = alloc_pages(arena, chunk_pages() - 1);
    assert!(pages.iter().all(|page| chunk_of(page) == chunk), "its first chunk first");
    pages.push(first);

    // The spare's first page: another spare taken.
    let spare_page = alloc(arena, Wait::No).unwrap();
    let spare = chunk_of(&spare_page);
    assert_ne!(spare, chunk);
    assert_eq!(chunk_arena(spare), Some(arena));
    assert_eq!(count_free_chunks(), free_chunks - 3);
    pages.push(spare_page);
    drop(pages);
    assert_eq!(count_free_chunks(), free_chunks - 1, "the spare stays");

    // Removed, the arena gives its spare back.
    remove_arena(arena);
    assert_eq!(count_free_chunks(), free_chunks);
    assert!(arena_fd(arena).is_none());
}

/// With no chunk free, an arena takes another's spare.
#[test]
fn starved_arenas_take_a_spare_chunk() {
    const CHUNKS: usize = 4;
    configure_chunks(CHUNKS);
    let (hog, starved) = (create_arena(), create_arena());
    // The hog takes pages until no chunk is free, the last it took its spare.
    let mut held = Vec::new();
    while count_free_chunks() > 0 {
        held.push(alloc(hog, Wait::No).unwrap());
    }
    let spare = (0..CHUNKS)
        .find(|&chunk| {
            chunk_arena(chunk) == Some(hog) && held.iter().all(|page| chunk_of(page) != chunk)
        })
        .expect("the hog has a spare chunk");
    let page = alloc(starved, Wait::No).expect("the hog's spare, taken");
    assert_eq!(chunk_of(&page), spare);
    assert_eq!(chunk_arena(spare), Some(starved));
}

/// Runs `f` with the host mapping no chunk of `arena`'s, as when the process makes as many mappings
/// as it may (`vm.max_map_count`): the arena's object made a read-only file, which the host does
/// not map writable. The object must have a slot free for each chunk `f` maps: on Linux, the object
/// grown for one, which it can't be meanwhile, would fail otherwise.
fn with_chunks_unmapped<R>(arena: ArenaId, f: impl FnOnce() -> R) -> R {
    let object = arena_fd(arena).unwrap();
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
/// object it would have taken: every way of allocating the page it was for fails at once, whether
/// or not it may wait, as memory out; and the next allocation, once the host maps it, takes both.
#[test]
fn chunks_the_host_does_not_map_stay_free() {
    keep_spare_chunks(false);
    let arena = create_arena();
    // The arena's object's slot free, which its next chunk takes.
    drop(alloc(arena, Wait::No).unwrap());
    let free_chunks = count_free_chunks();

    with_chunks_unmapped(arena, || {
        for wait in [Wait::No, Wait::Yes] {
            assert_eq!(alloc(arena, wait).err(), Some(NoMemory), "{wait:?}");
        }
    });
    assert_eq!(count_free_chunks(), free_chunks, "a chunk lost");
    assert_eq!(arena_bytes(arena), 0);
    assert!(has_alloc_failed_no_mem(), "the host's refusal is not memory out");

    let page = alloc(arena, Wait::No).expect("a chunk the host maps");
    assert_eq!(page.location().offset, 0, "the slot, free still");
    assert_eq!(count_free_chunks(), free_chunks - 1);
}

/// TASK-79.37: an arena starting on its spare chunk while the host maps no chunk takes its page,
/// and does without a spare. Nor do arenas ask the host for one again, which it would refuse too,
/// each allocation in vain, until it maps, or takes back, a chunk: then the arena takes its spare.
#[test]
fn spares_the_host_does_not_map_are_done_without() {
    let (arena, other) = (create_arena(), create_arena());
    // A chunk and a spare; the chunk gone back, its slot of the arena's object free, which the
    // arena's next spare takes.
    drop(alloc(arena, Wait::No).unwrap());
    assert_eq!(arena_bytes(arena), CHUNK, "its spare");
    let free_chunks = count_free_chunks();

    let page = with_chunks_unmapped(arena, || alloc(arena, Wait::No)).expect("a page of its spare");
    assert_eq!(arena_bytes(arena), CHUNK, "a spare taken");
    assert_eq!(count_free_chunks(), free_chunks, "a chunk lost");

    // The host would map one now, and is not asked to.
    let next = alloc(arena, Wait::No).unwrap();
    assert_eq!(chunk_of(&next), chunk_of(&page));
    assert_eq!(arena_bytes(arena), CHUNK, "a spare asked of a refusing host");
    // It maps a chunk, another arena's.
    let others = alloc(other, Wait::No).unwrap();
    let last = alloc(arena, Wait::No).unwrap();
    assert_eq!(chunk_of(&last), chunk_of(&page));
    assert_eq!(arena_bytes(arena), 2 * CHUNK, "a spare once the host maps one");
    drop([page, next, others, last]);
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
        let len = 2 * max.trim().parse::<usize>().unwrap() * page_bytes();
        let split = map_shared(len).expect("a mapping to split");
        // The thread's heap grown, and its memory freed, before the mappings run out: what the
        // thread allocates meanwhile, as the PMM logs, it takes from the heap as it is, rather than
        // grow it, which the host would refuse, and the process abort.
        drop(std::hint::black_box((0..16).map(|_| vec![1u8; 64 << 10]).collect::<Vec<_>>()));
        // Each page made readable splits what is left of the mapping: two mappings more, until the
        // host refuses, at its limit.
        let mut offset = page_bytes();
        // SAFETY: Changes the protection of a page of the mapping, which this owns.
        while unsafe { libc::mprotect(split.byte_add(offset), page_bytes(), libc::PROT_READ) } == 0
        {
            offset += 2 * page_bytes();
        }
        let error = std::io::Error::last_os_error();
        assert_eq!(error.raw_os_error(), Some(libc::ENOMEM), "mprotect: {error}");
        assert!(offset < len, "the mapping is too small to take every mapping");
        // Room for the few the host may map still, so that none grows the heap.
        let mut pages = Vec::with_capacity(16);
        let refused = (0..pages.capacity()).find_map(|_| match map_shared(page_bytes()) {
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
            let status = unsafe { libc::munmap(page, page_bytes()) };
            assert_eq!(status, 0, "munmap: {}", std::io::Error::last_os_error());
        }
    }
}

/// TASK-79.37: a chunk the host does not reserve again as it goes back, as reserving it splits
/// the mapping of its arena's chunks around it when the process makes as many mappings as it may
/// (`vm.max_map_count`), stays its arena's, its pages free, as its spare, and goes back with the
/// spares once the host takes it. Meanwhile another arena takes no chunk, failing at once, though
/// it may wait, and relief is not asked for: neither one free, which the host does not map, nor,
/// at the total cap, that spare, which it does not take back.
#[cfg(target_os = "linux")]
#[test]
fn chunks_the_host_does_not_take_back_stay_their_arenas() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    keep_spare_chunks(false);
    let (arena, other) = (create_arena(), create_arena());
    // Relief started before the mappings are taken, its thread's stack one of them.
    let relieved = Arc::new(AtomicUsize::new(0));
    relieve_with({
        let relieved = relieved.clone();
        move |_, _| {
            relieved.fetch_add(1, Ordering::Relaxed);
            0
        }
    })
    .unwrap();
    // Three chunks, next to one another, in the first three slots of the arena's object: one
    // mapping, which the middle one going back splits.
    let mut pages = alloc_pages(arena, 3 * chunk_pages());
    let chunks: Vec<usize> = pages.iter().step_by(chunk_pages()).map(chunk_of).collect();
    assert_eq!(chunks, [chunks[0], chunks[0] + 1, chunks[0] + 2]);
    let offsets: Vec<u64> =
        pages.iter().step_by(chunk_pages()).map(|page| page.location().offset).collect();
    assert_eq!(offsets, [0, CHUNK, 2 * CHUNK]);
    let free_chunks = count_free_chunks();

    let taken = MappingsTaken::new();
    drop(pages.drain(chunk_pages()..2 * chunk_pages()));
    assert_eq!(chunk_arena(chunks[1]), Some(arena), "the middle chunk went back");
    assert_eq!(arena_bytes(arena), 3 * CHUNK);
    assert_eq!(alloc(other, Wait::Yes).err(), Some(NoMemory));
    set_total_cap(Some(arenas_bytes()));
    assert_eq!(alloc(other, Wait::Yes).err(), Some(NoMemory));
    assert_eq!(chunk_arena(chunks[1]), Some(arena), "the spare went back");
    set_total_cap(None);
    assert_eq!(count_free_chunks(), free_chunks, "a chunk lost");
    drop(taken);
    assert_eq!(relieved.load(Ordering::Relaxed), 0, "relief asked for");

    crate::release_spares(Some(arena));
    assert_eq!(chunk_arena(chunks[1]), None, "the middle chunk, its spare, stayed");
    assert_eq!(arena_bytes(arena), 2 * CHUNK);
    pages.push(alloc(other, Wait::No).expect("a chunk the host maps"));
}

/// TASK-50.3: an arena takes no chunk past its cap: its allocations fail, without relief, whether
/// or not they may wait, once its own pages are taken, until one of them is freed, or its cap
/// lifted.
#[test]
fn arenas_take_no_chunk_past_their_cap() {
    let arena = create_arena();
    set_arena_cap(arena, Some(CHUNK));
    let mut pages = alloc_pages(arena, chunk_pages());
    assert_eq!(arena_bytes(arena), CHUNK);
    for wait in [Wait::No, Wait::Yes] {
        assert_eq!(alloc(arena, wait).err(), Some(NoMemory), "{wait:?}");
    }

    drop(pages.pop());
    pages.push(alloc(arena, Wait::No).unwrap());
    set_arena_cap(arena, None);
    pages.push(alloc(arena, Wait::No).unwrap());
    // Another chunk for the page, and a spare.
    assert_eq!(arena_bytes(arena), 3 * CHUNK);
    drop(pages);
    remove_arena(arena);
}

/// TASK-50.3: the arenas together take no chunk past the total cap.
#[test]
fn arenas_take_no_chunk_past_the_total_cap() {
    let arena = create_arena();
    set_total_cap(Some(arenas_bytes()));
    assert_eq!(crate::total_cap(), Some(arenas_bytes()));
    assert_eq!(alloc(arena, Wait::No).err(), Some(NoMemory));
    set_total_cap(None);
    assert_eq!(crate::total_cap(), None);
    drop(alloc(arena, Wait::No).unwrap());
    remove_arena(arena);
}

/// TASK-50.3: caps are whole chunks, rounded down.
#[test]
fn caps_are_rounded_down_to_chunks() {
    set_total_cap(Some(3 * CHUNK - 1));
    assert_eq!(crate::total_cap(), Some(2 * CHUNK));
    let arena = create_arena();
    set_arena_cap(arena, Some(CHUNK + CHUNK / 2));
    let pages = alloc_pages(arena, chunk_pages());
    assert_eq!(arena_bytes(arena), CHUNK, "its spare taken past its cap");
    drop(pages);
}

/// TASK-50.3: a total cap lowered under what the arenas hold, as many of their spare chunks, which
/// nothing uses, go back for an allocation as bring them under it.
#[test]
fn a_total_cap_lowered_under_the_arenas_gives_their_spares_back() {
    let arenas = [create_arena(), create_arena(), create_arena()];
    // A chunk for each page, and a spare.
    let mut pages: Vec<Page> =
        arenas.iter().map(|&arena| alloc(arena, Wait::No).unwrap()).collect();
    let cap = arenas_bytes() - 2 * CHUNK;
    set_total_cap(Some(cap));
    let arena = create_arena();
    pages.push(alloc(arena, Wait::No).expect("three spares gone back"));
    assert_eq!(arenas_bytes(), cap);
}

/// TASK-50.3: an arena at its cap is relieved, its allocations waiting meanwhile, by its own pages
/// freed, while relief frees some; once it frees none, they fail, until a page of the arena is
/// freed. At the total's, any arena's pages are.
#[test]
fn relief_frees_an_arenas_pages_then_fails_its_allocations() {
    // Spares would be freed for it as relief is not.
    keep_spare_chunks(false);
    let arena = create_arena();
    set_arena_cap(arena, Some(CHUNK));
    let mut pages = alloc_pages(arena, chunk_pages());
    // What relief is asked to free; and a page of the arena's it frees, once.
    let asked = Arc::new(Mutex::new(Vec::new()));
    let to_free = Arc::new(Mutex::new(pages.pop()));
    relieve_with({
        let (asked, to_free) = (asked.clone(), to_free.clone());
        move |from, bytes| {
            asked.lock().unwrap().push((from, bytes));
            let Some(page) = to_free.lock().unwrap().take() else { return 0 };
            drop(page);
            page_size()
        }
    })
    .unwrap();

    pages.push(alloc(arena, Wait::Yes).expect("the page relief freed"));
    assert_eq!(alloc(arena, Wait::Yes).err(), Some(NoMemory), "nothing left to free");
    let asked_for: Vec<_> = asked.lock().unwrap().drain(..).collect();
    assert_eq!(asked_for, vec![(Some(arena), RELIEF_BYTES); asked_for.len()]);
    drop(pages.pop());
    pages.push(alloc(arena, Wait::No).expect("a page of the arena freed"));

    set_arena_cap(arena, None);
    set_total_cap(Some(arenas_bytes()));
    let other = create_arena();
    assert_eq!(alloc(other, Wait::Yes).err(), Some(NoMemory), "nothing left to free");
    assert_eq!(asked.lock().unwrap().last(), Some(&(None, RELIEF_BYTES)));
}

/// TASK-79.76: relief asked for an arena again as its pass runs, by an allocation the pass then
/// makes room for, runs no pass of its own, as the arena is starved no longer: one would evict as
/// much again of the arena's pages for no allocation. Another arena, starved at its cap, asked for
/// after, and so relieved after, shows when the pass would have run.
#[test]
fn relief_asked_for_as_a_pass_relieves_the_arena_runs_no_pass_more() {
    keep_spare_chunks(false);
    let (arena, after) = (create_arena(), create_arena());
    assert!(arena < after, "the arena's ask taken first");
    for of in [arena, after] {
        set_arena_cap(of, Some(CHUNK));
    }
    let mut pages = alloc_pages(arena, chunk_pages());
    let held = alloc_pages(after, chunk_pages());
    // What the evictor frees of the arena's, once: four pages.
    let to_free = Arc::new(Mutex::new(pages.split_off(chunk_pages() - 4)));
    // The arenas relief's evictor is called for; and what the arena's allocation asking again, as
    // its first pass runs, the arena's pages not yet freed, was told.
    let (called, calls) = std::sync::mpsc::channel();
    let asked_again = Arc::new(Mutex::new(None));
    relieve_with({
        let (to_free, asked_again) = (to_free.clone(), asked_again.clone());
        move |from, _| {
            let mut asked = asked_again.lock().unwrap();
            if from == Some(arena) && asked.is_none() {
                *asked = Some(alloc(arena, Wait::No).err());
            }
            drop(asked);
            let freed = std::mem::take(&mut *to_free.lock().unwrap());
            let bytes = freed.len() as u64 * page_size();
            drop(freed);
            // Calls after the test's are let be.
            let _ = called.send(from);
            bytes
        }
    })
    .unwrap();

    pages.push(alloc(arena, Wait::Yes).expect("a page of the arena's, freed"));
    assert_eq!(alloc(after, Wait::Yes).err(), Some(NoMemory), "nothing of its own freed");
    let mut called_for = Vec::new();
    while called_for.last() != Some(&Some(after)) {
        called_for.push(calls.recv_timeout(Duration::from_secs(10)).expect("relief's pass"));
    }
    assert_eq!(*asked_again.lock().unwrap(), Some(Some(NoMemory)));
    assert_eq!(called_for, [Some(arena), Some(after)]);
    drop((pages, held));
}

/// TASK-50.3: relief gone, as its evictor panicked, the allocations it would make room for fail,
/// those waiting for it too.
#[test]
fn allocations_fail_once_relief_is_gone() {
    keep_spare_chunks(false);
    let arena = create_arena();
    set_arena_cap(arena, Some(CHUNK));
    let pages = alloc_pages(arena, chunk_pages());
    relieve_with(|_, _| panic!("an evictor that fails")).unwrap();
    assert_eq!(alloc(arena, Wait::Yes).err(), Some(NoMemory));
    assert_eq!(alloc(arena, Wait::Yes).err(), Some(NoMemory), "relief gone");
    drop(pages);
}

/// Relief starts once.
#[test]
fn relief_starts_once() {
    relieve_with(|_, _| 0).unwrap();
    let again = relieve_with(|_, _| 0).unwrap_err();
    assert_eq!(again.kind(), std::io::ErrorKind::AlreadyExists);
}

/// `count` pages of `arena`'s, each the one after the last, physically, where that one is free, as
/// an allocation of many pages takes them (TASK-108.1).
fn alloc_run(arena: ArenaId, count: usize) -> Vec<Page> {
    let mut pages: Vec<Page> = Vec::new();
    for _ in 0..count {
        let next = pages.last().and_then(alloc_after);
        pages.push(next.unwrap_or_else(|| alloc(arena, Wait::No).unwrap()));
    }
    pages
}

/// The runs of consecutive frames of `frames`.
fn runs(frames: &[u32]) -> usize {
    1 + frames.windows(2).filter(|pair| pair[1] != pair[0] + 1).count()
}

/// TASK-108.1: pages allocated together come in runs of consecutive pages where those free allow,
/// though the free list holds them scattered, freed in another order, as evictions free them: a
/// window maps each run with a mapping of its own in a VM process, not each page.
#[test]
fn pages_allocated_together_follow_on_from_one_another() {
    const PAGES: usize = 64;
    let arena = create_arena();
    let mut pages = alloc_run(arena, 2 * PAGES);
    pages.sort_by_key(Page::frame);
    let frames: Vec<u32> = pages.iter().map(Page::frame).collect();
    assert_eq!(runs(&frames), 1, "a new chunk's pages are consecutive");
    // The first half freed scattered, the second kept, so that runs end at it.
    let kept = pages.split_off(PAGES);
    let mut pages: Vec<Option<Page>> = pages.into_iter().map(Some).collect();
    for at in 0..PAGES {
        drop(pages[at * 37 % PAGES].take().expect("freed once"));
    }
    let again = alloc_run(arena, PAGES);
    let mut sorted: Vec<u32> = again.iter().map(Page::frame).collect();
    sorted.sort();
    assert_eq!(sorted, frames[..PAGES], "the pages freed, again");
    // In the order taken: a run from each page the list gave, up to one taken before.
    let taken: Vec<u32> = again.iter().map(Page::frame).collect();
    let taken_runs = runs(&taken);
    assert!(taken_runs <= 8, "{taken_runs} runs of {PAGES} pages, taken a page at a time");
    drop((again, kept));
}

/// TASK-108.1: the page after another is none that is allocated, nor past its chunk.
#[test]
fn pages_after_others_are_free_and_of_their_chunk() {
    let arena = create_arena();
    let pages = alloc_pages(arena, chunk_pages());
    assert_eq!(alloc_after(&pages[0]).map(|page| page.frame()), None, "allocated");
    let last = pages.last().unwrap();
    assert_eq!(chunk_of(last), chunk_of(&pages[0]));
    // The next chunk, the arena's spare, is free, but another chunk.
    let spare = alloc(arena, Wait::No).unwrap();
    assert_eq!(spare.frame(), last.frame() + 1);
    drop(spare);
    assert_eq!(alloc_after(last).map(|page| page.frame()), None, "past the chunk");
}
