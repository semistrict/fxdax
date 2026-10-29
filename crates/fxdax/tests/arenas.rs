// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TASK-79.36: an arena of physical memory of its own (TASK-43), a tenant's, held by
//! what allocates in it: its tenant, the stores opened in it, and what reads into the extent cache
//! for them. It goes once none of them holds it, and nothing allocates in it after.

use futures::executor::block_on;
use fxdax::pages::Arena;
use fxdax::{Access, ExtentId, ExtentStore, VolatileSlice};

/// A store opened in an arena pages its extents in, through its window, once what made the arena
/// for it lets go of it, as a tenant whose VM processes are gone does; and the arena goes with the
/// store: it gives no page then.
#[test]
fn stores_hold_their_arenas() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let ps = fxdax::pages::page_size();
    let store = ExtentStore::create(&path, 1 << 24).unwrap();
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    let bytes = vec![0xa5; ps as usize];
    store.write(extent, 0, &[VolatileSlice::from(&bytes[..])]).unwrap();
    store.commit(None).unwrap();
    drop(store);

    let held = Arena::new();
    let arena = held.id();
    let store = ExtentStore::open_in(&path, Some(held.clone())).unwrap();
    // What made it lets go of it, as a tenant's going does.
    drop(held);
    let window = store.create_dax_window(ps, None).unwrap();
    store.map(extent, 0, ps, 0, false).unwrap();
    block_on(window.fault(0, Access::Read)).expect("the extent paged in");
    assert!(physmem::arena_bytes(arena) > 0, "the page is another arena's");
    let page = physmem::alloc(arena, physmem::Wait::No).expect("a page of the store's arena");
    drop(page);

    drop((window, store));
    assert_eq!(physmem::alloc(arena, physmem::Wait::No).err(), Some(physmem::NoMemory));
}
