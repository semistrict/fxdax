// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Tests of the quarantine of freed pages a second stage of another process may map still
//! (`pmm::quarantine`, TASK-79.9, TASK-106).

use crate::{
    ArenaId, CHUNK_SIZE, NoMemory, Quarantined, Wait, alloc, count_free_pages, create_arena,
    keep_spare_chunks, quarantine_with, relieve_with, set_arena_cap,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A page a second stage mapped, dropped, is quarantined, not freed, and no other allocation's
/// until the quarantine drops it: then it is free, at the front of its arena's free list.
#[test]
fn staged_pages_are_quarantined_until_dropped() {
    let quarantined: Arc<Mutex<Vec<Quarantined>>> = Arc::default();
    quarantine_with({
        let quarantined = quarantined.clone();
        move |pages| quarantined.lock().unwrap().push(pages)
    });
    // A page of the chunk, which keeps it the arena's.
    let _keeper = alloc(ArenaId::DEFAULT, Wait::No).unwrap();
    let staged = alloc(ArenaId::DEFAULT, Wait::No).unwrap();
    let frame = staged.frame();
    staged.mark_staged();
    let free = count_free_pages();
    drop(staged);
    assert_eq!(count_free_pages(), free, "freed, not quarantined");
    let pages = quarantined.lock().unwrap().pop().expect("quarantined");
    assert_eq!(pages.len(), 1);
    assert!(!pages.is_empty());
    assert!(quarantined.lock().unwrap().is_empty());

    let next = alloc(ArenaId::DEFAULT, Wait::No).unwrap();
    assert_ne!(next.frame(), frame, "a quarantined page allocated");
    drop(pages);
    assert_eq!(count_free_pages(), free);
    let again = alloc(ArenaId::DEFAULT, Wait::No).unwrap();
    assert_eq!(again.frame(), frame, "freed at the front of its arena's list");
}

/// A page no second stage mapped is freed at once, quarantine or none.
#[test]
fn unstaged_pages_are_freed_at_once() {
    quarantine_with(|_| panic!("an unstaged page quarantined"));
    // A page of the chunk, which keeps it the arena's.
    let _keeper = alloc(ArenaId::DEFAULT, Wait::No).unwrap();
    let page = alloc(ArenaId::DEFAULT, Wait::No).unwrap();
    let (frame, free) = (page.frame(), count_free_pages());
    drop(page);
    assert_eq!(count_free_pages(), free + 1);
    assert_eq!(alloc(ArenaId::DEFAULT, Wait::No).unwrap().frame(), frame);
}

/// Without a quarantine, a page a second stage mapped is freed at once; allocated again, it is no
/// second stage's, and once a quarantine is given, freed at once still.
#[test]
fn staged_pages_are_freed_at_once_without_a_quarantine() {
    // A page of the chunk, which keeps it the arena's.
    let _keeper = alloc(ArenaId::DEFAULT, Wait::No).unwrap();
    let page = alloc(ArenaId::DEFAULT, Wait::No).unwrap();
    let (frame, free) = (page.frame(), count_free_pages());
    page.mark_staged();
    drop(page);
    assert_eq!(count_free_pages(), free + 1);
    quarantine_with(|_| panic!("a page freed since it was staged quarantined"));
    let again = alloc(ArenaId::DEFAULT, Wait::No).unwrap();
    assert_eq!(again.frame(), frame);
    drop(again);
    assert_eq!(count_free_pages(), free + 1);
}

/// TASK-106: an arena at its cap, whose page relief's evictor freed is quarantined, is not
/// exhausted by passes that free nothing while the page is to come back: its allocation waits for
/// it rather than fail, and takes it as it comes back.
#[test]
fn relief_waits_for_quarantined_pages_to_come_back() {
    keep_spare_chunks(false);
    let quarantined: Arc<Mutex<Vec<Quarantined>>> = Arc::default();
    quarantine_with({
        let quarantined = quarantined.clone();
        move |pages| quarantined.lock().unwrap().push(pages)
    });
    let arena = create_arena();
    set_arena_cap(arena, Some(CHUNK_SIZE as u64));
    let mut pages: Vec<_> = std::iter::from_fn(|| alloc(arena, Wait::No).ok()).collect();
    assert_eq!(alloc(arena, Wait::No).err(), Some(NoMemory));
    let evicted = pages.pop().unwrap();
    let frame = evicted.frame();
    evicted.mark_staged();
    drop(evicted);
    let pages_back = quarantined.lock().unwrap().pop().expect("quarantined");
    let (relieved, passes) = std::sync::mpsc::channel();
    relieve_with(move |from, _| {
        let _ = relieved.send(from);
        0
    })
    .unwrap();

    let waiter = std::thread::spawn(move || alloc(arena, Wait::Yes).map(|page| page.frame()));
    // Passes that free nothing, which leave it waiting.
    for _ in 0..3 {
        let pass = passes.recv_timeout(Duration::from_secs(10)).expect("relief's pass");
        assert_eq!(pass, Some(arena));
    }
    assert!(!waiter.is_finished(), "the allocation failed, or found room");
    drop(pages_back);
    assert_eq!(waiter.join().unwrap(), Ok(frame));
}
