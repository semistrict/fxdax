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

/// Of the frees kept, a view reads only those after its commit, which its commit holds: not one
/// freed before, which an older view keeps.
#[test]
fn views_read_only_what_their_commit_holds() {
    let store = create(OPTIONS);
    write(&store, 1, 2);
    store.commit(None).unwrap();
    let older = store.view();
    store.free(extent(1)).unwrap();
    store.commit(None).unwrap();
    let newer = store.view();
    assert_eq!(read(&store, &older, 1, 2).unwrap(), pattern(1, 2));
    error_is(read(&store, &newer, 1, 2), NoExtent(extent(1)));
    assert_eq!(store.view_extent(&older, extent(1)).unwrap(), 2 * BLOCK as u64);
    error_is(store.view_extent(&newer, extent(1)), NoExtent(extent(1)));
    // But for what it pins.
    newer.pin(extent(1)).unwrap();
    assert_eq!(read(&store, &newer, 1, 2).unwrap(), pattern(1, 2));
}

/// A view advanced keeping holds what the commits since the one it was as of held, those
/// between included, until it lets go of them: what a follower applying them reads.
#[test]
fn views_advanced_keeping_hold_the_commits_between() {
    let store = create(OPTIONS);
    write(&store, 1, 1);
    store.commit(None).unwrap();
    let mut view = store.view();
    write(&store, 2, 1);
    store.commit(None).unwrap();
    store.free(extent(1)).unwrap();
    store.free(extent(2)).unwrap();
    store.commit(None).unwrap();
    view.advance_keeping().unwrap();
    assert_eq!(view.epoch(), store.epoch().unwrap());
    assert_eq!(read(&store, &view, 1, 1).unwrap(), pattern(1, 1));
    assert_eq!(read(&store, &view, 2, 1).unwrap(), pattern(2, 1));
    view.let_go(view.epoch());
    assert_eq!(view.since(), view.epoch());
    error_is(read(&store, &view, 1, 1), NoExtent(extent(1)));
    error_is(read(&store, &view, 2, 1), NoExtent(extent(2)));
    store.commit(None).unwrap();
    assert_eq!(kept(&store), 0);
}

/// A view lets go of the commits before the one named, which a let-go arriving after its next
/// advance names: it holds the commits since that one still, and a let-go of an earlier one
/// changes nothing.
#[test]
fn views_let_go_of_the_commits_before_the_one_named() {
    let store = create(OPTIONS);
    write(&store, 1, 1);
    write(&store, 2, 1);
    store.commit(None).unwrap();
    let mut view = store.view();
    let first = view.epoch();
    store.free(extent(1)).unwrap();
    store.commit(None).unwrap();
    view.advance_keeping().unwrap();
    let second = view.epoch();
    store.free(extent(2)).unwrap();
    store.commit(None).unwrap();
    view.advance_keeping().unwrap();

    view.let_go(second);
    assert_eq!(view.since(), second);
    error_is(read(&store, &view, 1, 1), NoExtent(extent(1)));
    assert_eq!(read(&store, &view, 2, 1).unwrap(), pattern(2, 1));
    view.let_go(first);
    assert_eq!(view.since(), second);
    view.let_go(u64::MAX);
    assert_eq!(view.since(), view.epoch());
    error_is(read(&store, &view, 2, 1), NoExtent(extent(2)));
}

/// An extent's length as of a view: of one its commit holds, still or freed since; not of one
/// written after it.
#[test]
fn views_know_their_extents_lengths() {
    let store = create(OPTIONS);
    write(&store, 1, 3);
    store.commit(None).unwrap();
    let view = store.view();
    write(&store, 2, 1);
    store.free(extent(1)).unwrap();
    store.commit(None).unwrap();
    assert_eq!(store.view_extent(&view, extent(1)).unwrap(), 3 * BLOCK as u64);
    error_is(store.view_extent(&view, extent(2)), NoExtent(extent(2)));
}

/// A view waits for a commit after its own: what tells a follower to advance.
#[test]
fn views_wait_for_newer_commits() {
    let store = std::sync::Arc::new(create(OPTIONS));
    write(&store, 1, 1);
    store.commit(None).unwrap();
    let mut view = store.view();
    assert_eq!(view.newer(Duration::from_millis(10)).unwrap(), None);
    let committing = {
        let store = store.clone();
        std::thread::spawn(move || {
            write(&store, 2, 1);
            store.commit(None).unwrap();
            store.epoch().unwrap()
        })
    };
    let newer = view.newer(Duration::from_secs(60)).unwrap();
    let epoch = committing.join().unwrap();
    assert_eq!(newer, Some(epoch));
    view.advance().unwrap();
    assert_eq!(view.newer(Duration::from_millis(10)).unwrap(), None);
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
    assert_eq!(view.pinned(), vec![extent(1)]);

    view.unpin(extent(1));
    assert_eq!(view.pinned(), vec![]);
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

/// A store's views, as the followers' measurements report them: how many, how many commits the
/// furthest behind is, and that it has been behind since; none once cut off.
#[test]
fn views_report_how_far_behind_they_are() {
    let store = create(OPTIONS);
    write(&store, 1, 1);
    store.commit(None).unwrap();
    assert_eq!(store.view_stats(), extent_store::ViewStats::default());
    let mut behind = store.view();
    let _latest = store.view();
    store.commit(None).unwrap();
    store.commit(None).unwrap();
    let stats = store.view_stats();
    assert_eq!((stats.views, stats.behind), (2, 2));
    assert!(stats.stale_for > Duration::ZERO, "{stats:?}");
    behind.advance().unwrap();
    store.cut_views();
    assert_eq!(store.view_stats(), extent_store::ViewStats::default());
}

/// Views the store cuts off (its writer gone with writes uncommitted) fail from then on, and the
/// next commit keeps nothing for them; views opened after aren't cut off.
#[test]
fn views_cut_off_by_the_store_fail() {
    let store = create(OPTIONS);
    write(&store, 1, 1);
    store.commit(None).unwrap();
    let mut view = store.view();
    store.free(extent(1)).unwrap();
    store.cut_views();

    error_is(read(&store, &view, 1, 1), CutOff);
    error_is(view.advance(), CutOff);
    error_is(view.pin(extent(1)), CutOff);
    error_is(view.newer(Duration::ZERO), CutOff);
    store.commit(None).unwrap();
    assert_eq!(kept(&store), 0);
    let fresh = store.view();
    assert_eq!(fresh.epoch(), 2);
    assert_eq!(fresh.newer(Duration::ZERO).unwrap(), None);
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
