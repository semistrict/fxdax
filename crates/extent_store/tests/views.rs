// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: views of a branch as of a commit (TASK-42.2): what they read while the writer goes on,
//! what they keep of its frees, and for how long, their pins, and their cutting off. Their
//! concurrency with the writer is tests/shuttle.rs's.

use extent_store::{CutOff, Description, MemDir, Options, Store, View};
use std::time::Duration;
use storage_device::{ExtentId, NoExtent};

const BLOCK: usize = 4096;
const BRANCH: u64 = 7;

/// Small segments and large extents, so that tests make several of each.
const OPTIONS: Options = Options {
    segment_size: 64 << 10,
    segment_age: Duration::from_secs(3600),
    large_extent: 32 << 10,
    checkpoint_after: 4 << 10,
    open_files: 2,
    view_lag: None,
    view_kept_bytes: None,
};

fn extent(sequence: u64) -> ExtentId {
    ExtentId::new(BRANCH, sequence)
}

/// `blocks` blocks of bytes each their offset modulo 251, plus `seed`.
fn pattern(seed: u8, blocks: usize) -> Vec<u8> {
    (0..blocks * BLOCK).map(|i| (i % 251) as u8 ^ seed).collect()
}

fn create(options: Options) -> Store<MemDir> {
    let description = Description { block_size: BLOCK as u32, capacity: 64 << 20, branch: BRANCH };
    Store::create(MemDir::new(), description, options).unwrap()
}

/// Writes extent `sequence`, of `blocks` blocks of its pattern.
fn write(store: &Store<MemDir>, sequence: u64, blocks: usize) {
    store.write(extent(sequence), 0, &pattern(sequence as u8, blocks)).unwrap();
}

/// Extent `sequence`, as `view` reads `blocks` blocks of it.
fn read(
    store: &Store<MemDir>,
    view: &View,
    sequence: u64,
    blocks: usize,
) -> Result<Vec<u8>, anyhow::Error> {
    let mut data = vec![0; blocks * BLOCK];
    store.read_view(view, extent(sequence), 0, &mut data)?;
    Ok(data)
}

fn error_is<E: std::error::Error + PartialEq + Send + Sync + 'static, T>(
    result: Result<T, anyhow::Error>,
    expected: E,
) {
    let Err(error) = result else { panic!("succeeded, expecting {expected:?}") };
    assert_eq!(error.downcast_ref::<E>(), Some(&expected), "{error:?}");
}

/// The bytes of frees views keep, which count as used.
fn kept(store: &Store<MemDir>) -> u64 {
    let kept = store.usage().unwrap().kept_for_views;
    assert_eq!(store.info().unwrap().used, store_bytes(store) + kept);
    kept
}

/// The bytes the store's extents hold.
fn store_bytes(store: &Store<MemDir>) -> u64 {
    store.extents().unwrap().iter().map(|(_, len)| len).sum()
}

/// A view reads the commit it is of, and its root, while the writer frees what it holds, in
/// segments and in files of their own, writes more, cleans and commits.
#[test]
fn views_read_the_commit_they_are_of() {
    let store = create(OPTIONS);
    write(&store, 1, 2);
    // Large: in a file of its own.
    write(&store, 2, 10);
    write(&store, 3, 1);
    store.commit(Some(b"one")).unwrap();
    let view = store.view();
    assert_eq!((view.epoch(), view.root()), (1, &b"one"[..]));

    store.free(extent(1)).unwrap();
    store.free(extent(2)).unwrap();
    write(&store, 4, 3);
    store.commit(Some(b"two")).unwrap();
    store.clean(0.99).unwrap();
    write(&store, 5, 1);
    store.commit(None).unwrap();

    assert_eq!(read(&store, &view, 1, 2).unwrap(), pattern(1, 2));
    assert_eq!(read(&store, &view, 2, 10).unwrap(), pattern(2, 10));
    assert_eq!(read(&store, &view, 3, 1).unwrap(), pattern(3, 1));
    assert_eq!((view.epoch(), view.root()), (1, &b"one"[..]));
    // A view opened now is of the latest commit, and its root.
    let latest = store.view();
    assert_eq!((latest.epoch(), latest.root()), (4, &b"two"[..]));
    assert_eq!(read(&store, &latest, 4, 3).unwrap(), pattern(4, 3));
}

/// The frees after a view's commit are kept, and count as used, until it advances past them, or
/// goes: then the next commit lets them go.
#[test]
fn kept_frees_go_once_views_advance_or_drop() {
    let store = create(OPTIONS);
    write(&store, 1, 3);
    store.commit(None).unwrap();
    let mut view = store.view();
    store.free(extent(1)).unwrap();
    store.commit(None).unwrap();
    assert_eq!(kept(&store), 3 * BLOCK as u64);

    view.advance().unwrap();
    assert_eq!(view.epoch(), 2);
    store.commit(None).unwrap();
    assert_eq!(kept(&store), 0);
    error_is(read(&store, &view, 1, 3), NoExtent(extent(1)));

    drop(view);
    write(&store, 2, 1);
    store.commit(None).unwrap();
    let other = store.view();
    store.free(extent(2)).unwrap();
    store.commit(None).unwrap();
    assert_eq!(kept(&store), BLOCK as u64);
    drop(other);
    store.commit(None).unwrap();
    assert_eq!(kept(&store), 0);
    assert_eq!(store.usage().unwrap().kept_bytes, 0);
}

/// What views keep takes room as writes in progress do: it is used, and not available.
#[test]
fn kept_frees_take_room() {
    let store = create(OPTIONS);
    write(&store, 1, 3);
    store.commit(None).unwrap();
    let _view = store.view();
    store.free(extent(1)).unwrap();
    store.commit(None).unwrap();
    let write = store.begin_write(extent(2), 0, 2 * BLOCK as u64).unwrap();
    let info = store.info().unwrap();
    assert_eq!(info.used, 3 * BLOCK as u64);
    assert_eq!(info.available, info.capacity - 5 * BLOCK as u64);
    drop(write);
}

/// A view locates the parts of what it holds, freed since, as [`Store::locate`] does what the
/// store holds; an open extent it holds not.
#[test]
fn views_locate_what_they_hold() {
    let store = create(OPTIONS);
    write(&store, 1, 3);
    store.commit(None).unwrap();
    let view = store.view();
    let held = store.locate(extent(1), 0, 3 * BLOCK as u64).unwrap();
    store.free(extent(1)).unwrap();
    store.commit(None).unwrap();
    let parts = store.locate_view(&view, extent(1), 0, 3 * BLOCK as u64).unwrap();
    assert_eq!(parts.len(), held.len());
    assert_eq!(parts.iter().map(|part| part.len()).sum::<u64>(), 3 * BLOCK as u64);
    write(&store, 2, 1);
    error_is(store.locate_view(&view, extent(2), 0, BLOCK as u64), NoExtent(extent(2)));
}

/// A view opened after a free, but before the commit that frees it, is of the commit before,
/// which holds it: it is kept. One opened after that commit keeps nothing of it.
#[test]
fn views_keep_what_their_commit_holds() {
    let store = create(OPTIONS);
    write(&store, 1, 1);
    write(&store, 2, 1);
    store.commit(None).unwrap();
    store.free(extent(1)).unwrap();
    let before = store.view();
    store.commit(None).unwrap();
    assert_eq!(read(&store, &before, 1, 1).unwrap(), pattern(1, 1));
    drop(before);
    store.free(extent(2)).unwrap();
    store.commit(None).unwrap();
    let after = store.view();
    store.commit(None).unwrap();
    assert_eq!(kept(&store), 0);
    error_is(read(&store, &after, 2, 1), NoExtent(extent(2)));
}

/// A view refuses extents its commit doesn't hold, sealed by a later one: a follower's journal
/// replay reads on until it finds nothing, and must end at its view's commit. Those sealed while
/// no view lived are older than any view.
#[test]
fn views_refuse_extents_of_later_commits() {
    let store = create(OPTIONS);
    write(&store, 1, 1);
    store.commit(None).unwrap();
    let mut view = store.view();
    write(&store, 2, 1);
    store.commit(None).unwrap();
    error_is(read(&store, &view, 2, 1), NoExtent(extent(2)));
    view.advance().unwrap();
    assert_eq!(read(&store, &view, 2, 1).unwrap(), pattern(2, 1));
    drop(view);
    write(&store, 3, 1);
    store.commit(None).unwrap();
    let view = store.view();
    assert_eq!(read(&store, &view, 3, 1).unwrap(), pattern(3, 1));
}

/// An extent freed while open, before a commit sealed it, was in no view's commit: it is not kept.
#[test]
fn open_extents_freed_are_not_kept() {
    let store = create(OPTIONS);
    let _view = store.view();
    write(&store, 1, 2);
    store.free(extent(1)).unwrap();
    store.commit(None).unwrap();
    assert_eq!(kept(&store), 0);
}

/// A duplicate of a view is as of its commit, and keeps its frees when it goes, but not its pins.
#[test]
fn duplicates_keep_what_their_commit_holds() {
    let store = create(OPTIONS);
    write(&store, 1, 1);
    write(&store, 2, 1);
    store.commit(None).unwrap();
    let view = store.view();
    view.pin(extent(2)).unwrap();
    let copy = view.duplicate().unwrap();
    assert_eq!((copy.epoch(), copy.root()), (view.epoch(), view.root()));
    store.free(extent(1)).unwrap();
    store.free(extent(2)).unwrap();
    store.commit(None).unwrap();
    drop(view);
    store.commit(None).unwrap();
    assert_eq!(read(&store, &copy, 1, 1).unwrap(), pattern(1, 1));
    assert_eq!(read(&store, &copy, 2, 1).unwrap(), pattern(2, 1));
}

/// A pinned extent is kept, whatever commit its view advances to, until unpinned.
#[test]
fn pins_keep_extents_past_advances() {
    let store = create(OPTIONS);
    write(&store, 1, 2);
    store.commit(None).unwrap();
    let mut view = store.view();
    view.pin(extent(1)).unwrap();
    store.free(extent(1)).unwrap();
    store.commit(None).unwrap();
    view.advance().unwrap();
    store.commit(None).unwrap();
    assert_eq!(read(&store, &view, 1, 2).unwrap(), pattern(1, 2));
    assert_eq!(kept(&store), 2 * BLOCK as u64);

    view.unpin(extent(1));
    store.commit(None).unwrap();
    assert_eq!(kept(&store), 0);
    error_is(read(&store, &view, 1, 2), NoExtent(extent(1)));
}

/// A view stale for `view_lag` is cut off: its reads, advances and pins fail, and it keeps
/// nothing. Views opened since are not.
#[test]
fn views_lagging_too_long_are_cut_off() {
    let store = create(Options { view_lag: Some(Duration::ZERO), ..OPTIONS });
    write(&store, 1, 1);
    store.commit(None).unwrap();
    let mut view = store.view();
    assert_eq!(read(&store, &view, 1, 1).unwrap(), pattern(1, 1));
    store.free(extent(1)).unwrap();
    store.commit(None).unwrap();

    error_is(read(&store, &view, 1, 1), CutOff);
    error_is(view.advance(), CutOff);
    error_is(view.pin(extent(1)), CutOff);
    assert_eq!(kept(&store), 0);
    let fresh = store.view();
    write(&store, 2, 1);
    assert_eq!(fresh.epoch(), 2);
    error_is(read(&store, &fresh, 2, 1), NoExtent(extent(2)));
}

/// Views keeping more than `view_kept_bytes` of frees are cut off, the oldest first, until those
/// left keep no more.
#[test]
fn views_keeping_too_much_are_cut_off() {
    let store = create(Options { view_kept_bytes: Some(2 * BLOCK as u64), ..OPTIONS });
    write(&store, 1, 1);
    write(&store, 2, 2);
    store.commit(None).unwrap();
    let old = store.view();
    store.free(extent(1)).unwrap();
    store.commit(None).unwrap();
    assert_eq!(kept(&store), BLOCK as u64);
    let young = store.view();
    store.free(extent(2)).unwrap();
    store.commit(None).unwrap();

    // The old view would keep both, three blocks; the young one, the second's two.
    error_is(read(&store, &old, 1, 1), CutOff);
    assert_eq!(read(&store, &young, 2, 2).unwrap(), pattern(2, 2));
    assert_eq!(kept(&store), 2 * BLOCK as u64);
}
