// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: how long a store's hot paths take a call, each looking up an extent's life: a page-in's
//! `locate`, a view's `locate_view`, a write's opening and finish, and a free; the store holding
//! tens of thousands of extents, keeping as many freed, and having spent their numbers
//! (TASK-160.18). A measurement, not a check: ignored, run in release,
//! `cargo nextest run --release -p extent_store --test hot_path --run-ignored only --no-capture`.

use extent_store::{Description, HostDir, Options, Store};
use std::time::Instant;
use storage_device::ExtentId;

const BLOCK: usize = 4096;
const BRANCH: extent_remote::layout::Branch = extent_remote::layout::test_branch(7);
/// The extents the store holds, and as many it keeps, freed.
const EXTENTS: u64 = 20_000;
/// The rounds of each call timed, over every extent.
const ROUNDS: u32 = 5;

fn extent(sequence: u64) -> ExtentId {
    ExtentId::new(BRANCH.id, sequence)
}

/// The nanoseconds a call of `call` took, on average, over `calls` calls.
fn timed(calls: u64, mut call: impl FnMut()) -> f64 {
    let started = Instant::now();
    call();
    started.elapsed().as_nanos() as f64 / calls as f64
}

#[test]
#[ignore = "a benchmark: --run-ignored only, in release"]
fn hot_paths() {
    let temp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let description = Description { block_size: BLOCK as u32, capacity: 4 << 30, branch: BRANCH };
    let dir = HostDir::new(temp.path(), true).unwrap();
    let store = Store::create(dir, description, Options::default()).unwrap();
    let data = vec![7u8; BLOCK];
    // Held: 1..=EXTENTS; kept, freed: EXTENTS+1..=2*EXTENTS, their numbers spent.
    for sequence in 1..=2 * EXTENTS {
        store.write(extent(sequence), 0, &data).unwrap();
        if sequence % 1000 == 0 {
            store.commit(None).unwrap();
        }
    }
    for sequence in EXTENTS + 1..=2 * EXTENTS {
        store.free(extent(sequence)).unwrap();
    }
    store.commit(None).unwrap();
    let view = store.view();
    let held = || (1..=EXTENTS).map(extent);
    let mut results = Vec::new();
    for _ in 0..ROUNDS {
        let locate = timed(EXTENTS, || {
            for extent in held() {
                store.locate(extent, 0, BLOCK as u64).unwrap();
            }
        });
        let viewed = timed(EXTENTS, || {
            for extent in held() {
                store.locate_view(&view, extent, 0, BLOCK as u64).unwrap();
            }
        });
        // Refused: sealed, or spent, as a write to a sealed extent is.
        let refused = timed(2 * EXTENTS, || {
            for sequence in 1..=2 * EXTENTS {
                assert!(store.begin_write(extent(sequence), 0, BLOCK as u64).is_err());
            }
        });
        results.push((locate, viewed, refused));
    }
    // Writes of new extents, opened and finished, and frees of them, open: once each.
    let first = 2 * EXTENTS + 1;
    let written = timed(EXTENTS, || {
        for sequence in first..first + EXTENTS {
            store.write(extent(sequence), 0, &data).unwrap();
        }
    });
    let freed = timed(EXTENTS, || {
        for sequence in first..first + EXTENTS {
            store.free(extent(sequence)).unwrap();
        }
    });
    let best =
        |pick: fn(&(f64, f64, f64)) -> f64| results.iter().map(pick).fold(f64::INFINITY, f64::min);
    eprintln!(
        "ns a call, best of {ROUNDS}: locate {:.0}, locate_view {:.0}, begin_write refused {:.0}; \
         once: write (open, write, finish) {written:.0}, free (open) {freed:.0}",
        best(|r| r.0),
        best(|r| r.1),
        best(|r| r.2),
    );
}
