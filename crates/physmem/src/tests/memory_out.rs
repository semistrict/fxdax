// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Tests of memory out, lifted from fxdax's port of the Zircon kernel
//! (`vm::unittests::memory_out_unittest`, TASK-50.3, TASK-79.70): an allocation that finds no free
//! page at all fails, as the node's failure, unless relief makes it room; and fails once relief
//! makes none, rather than wait for ever.

use super::configure_chunks;
use crate::{
    ArenaId, CHUNK_SIZE, NoMemory, Page, RELIEF_BYTES, Wait, alloc, count_free_pages, create_arena,
    has_alloc_failed_no_mem, keep_spare_chunks, page_size, relieve_with, set_arena_cap,
};
use std::sync::{Arc, Mutex};

/// Takes every free page with the default arena's pages, and returns them.
fn fill_memory() -> Vec<Page> {
    configure_chunks(16);
    let free = count_free_pages();
    let pages = (0..free).map(|_| alloc(ArenaId::DEFAULT, Wait::No).unwrap()).collect();
    assert_eq!(count_free_pages(), 0);
    assert!(!has_alloc_failed_no_mem(), "filling memory failed");
    pages
}

/// With every free page taken, an allocation fails, whether or not it can wait, rather than wait
/// for room nothing makes: the node's failure, as memory is out, which it records.
#[test]
fn allocations_that_find_memory_out_fail_as_the_nodes_failure() {
    let _pages = fill_memory();
    for wait in [Wait::No, Wait::Yes] {
        assert_eq!(alloc(ArenaId::DEFAULT, wait).err(), Some(NoMemory), "{wait:?}");
    }
    assert!(has_alloc_failed_no_mem(), "the failure was not recorded");
}

/// An arena's own failure, at its cap while other arenas' chunks hold free pages, is not the
/// node's: memory is not out.
#[test]
fn an_arenas_own_failure_is_not_the_nodes() {
    configure_chunks(16);
    keep_spare_chunks(false);
    let arena = create_arena();
    set_arena_cap(arena, Some(CHUNK_SIZE as u64));
    let _pages: Vec<Page> =
        (0..CHUNK_SIZE as u64 / page_size()).map(|_| alloc(arena, Wait::No).unwrap()).collect();
    assert!(count_free_pages() > 0);
    for wait in [Wait::No, Wait::Yes] {
        assert_eq!(alloc(arena, wait).err(), Some(NoMemory), "{wait:?}");
    }
    assert!(!has_alloc_failed_no_mem(), "an arena's failure recorded as memory out");
}

/// With relief, an allocation that finds every free page taken by pages relief's evictor frees
/// none of waits for relief's pass, and then fails, its arena exhausted: the node's failure, as
/// memory is out, which it records.
#[test]
fn relief_failures_finding_memory_out_are_the_nodes() {
    let _pages = fill_memory();
    let asked = Arc::new(Mutex::new(Vec::new()));
    relieve_with({
        let asked = asked.clone();
        move |from, bytes| {
            asked.lock().unwrap().push((from, bytes));
            0
        }
    })
    .unwrap();

    assert_eq!(alloc(ArenaId::DEFAULT, Wait::Yes).err(), Some(NoMemory));
    assert!(has_alloc_failed_no_mem(), "the failure was not recorded");
    assert_eq!(asked.lock().unwrap().first(), Some(&(None, RELIEF_BYTES)));
}

/// With relief, an allocation that can wait, finding every free page taken, waits while relief's
/// evictor frees one, and takes it: no failure.
#[test]
fn relief_makes_room_when_memory_is_out() {
    let pages = Arc::new(Mutex::new(fill_memory()));
    relieve_with({
        let pages = pages.clone();
        move |_, _| {
            drop(pages.lock().unwrap().pop());
            page_size()
        }
    })
    .unwrap();

    let page = alloc(ArenaId::DEFAULT, Wait::Yes).expect("a page relief freed");
    assert!(!has_alloc_failed_no_mem(), "a failure recorded");
    drop(page);
}

/// An allocation that may not wait, finding every free page taken, fails, as memory out, having
/// asked relief for room for those after it, which find what it freed.
#[test]
fn allocations_that_may_not_wait_ask_for_relief_for_those_after() {
    let pages = Arc::new(Mutex::new(fill_memory()));
    let (relieved, passes) = std::sync::mpsc::channel();
    relieve_with({
        let pages = pages.clone();
        move |from, bytes| {
            drop(pages.lock().unwrap().pop());
            relieved.send((from, bytes)).unwrap();
            page_size()
        }
    })
    .unwrap();

    assert_eq!(alloc(ArenaId::DEFAULT, Wait::No).err(), Some(NoMemory));
    assert!(has_alloc_failed_no_mem(), "the failure was not recorded");
    assert_eq!(passes.recv().unwrap(), (None, RELIEF_BYTES));
    drop(alloc(ArenaId::DEFAULT, Wait::No).expect("the page relief freed"));
}
