// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The store's behaviour, in memory and in a directory of the host's.

mod common;

use common::noise;
use extent_frame::segment::{Chain, Child, Contents, Location};
use extent_remote::wal::Fenced;
use extent_store::{
    Closed, Description, Dir, File as _, HostDir, MemDir, MemFile, NoSpace, Opening, Options, Part,
    Place, RemoteData, Spent, Store, TooLong, TooSmall,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use storage_device::{ExtentId, NoExtent, OtherBranch, SealedExtent};

const BLOCK: usize = 4096;
const BRANCH: extent_remote::layout::Branch = extent_remote::layout::test_branch(7);
/// The most bytes an extent holds: doc-4's `max_extent`, the store's default.
const MAX_EXTENT: u64 = 64 << 20;

/// Small segments and large extents, so that tests make several of each.
const OPTIONS: Options = Options {
    max_extent: MAX_EXTENT,
    segment_size: 64 << 10,
    segment_age: std::time::Duration::from_secs(3600),
    large_extent: 32 << 10,
    open_files: 2,
    view_lag: None,
    view_kept_bytes: None,
    held_most: 256 << 10,
};

/// The chain of a writer of the branch in object storage, whose segments the store closes: that of
/// a store opened where the bucket holds nothing of its branch (`Opening::new`).
const WRITER: Chain = Chain::new(BRANCH, 0, 0, None, 0);

fn extent(sequence: u64) -> ExtentId {
    ExtentId::new(BRANCH.id, sequence)
}

/// The branch's directory of `kind` (`segments`, `extents`, `index`).
fn of(kind: &str, branch: u64) -> String {
    format!("{kind}/{branch:016x}")
}

/// Bytes each their offset modulo 251, from `offset`, plus `seed`.
fn pattern(seed: u8, offset: usize, len: usize) -> Vec<u8> {
    (offset..offset + len).map(|i| (i % 251) as u8 ^ seed).collect()
}

fn create<D: Dir>(dir: D, capacity: u64) -> Store<D> {
    let description = Description { block_size: BLOCK as u32, capacity, branch: BRANCH };
    Store::create(dir, description, OPTIONS).unwrap()
}

/// fxdax: the store in `dir`, opened where the bucket holds `state` of its branch, its writer's
/// chain `chain` (TASK-165.9).
fn opened_on(dir: MemDir, state: extent_remote::State, chain: Chain) -> Store<MemDir> {
    let last = state.last_commit;
    let fenced = Fenced { recovered: last, last_commit: last, chain };
    let opening = Opening { state, fenced: Some(fenced), cache: None };
    Store::recover_unmade(dir).unwrap().open(OPTIONS, opening).unwrap()
}

/// fxdax: a new store in `dir`, of `capacity` bytes, made whole, opened as [`opened_on`] opens it.
fn created_on(
    dir: MemDir,
    capacity: u64,
    state: extent_remote::State,
    chain: Chain,
) -> Store<MemDir> {
    let description = Description { block_size: BLOCK as u32, capacity, branch: BRANCH };
    Store::make(&dir, description, None, None).unwrap();
    let store = opened_on(dir, state, chain);
    store.made().unwrap();
    store
}

fn read<D: Dir>(store: &Store<D>, extent: ExtentId, offset: u64, len: usize) -> Vec<u8> {
    let mut data = vec![0; len];
    store.read(extent, offset, &mut data).unwrap();
    data
}

fn error_is<E: std::error::Error + PartialEq + Send + Sync + 'static>(
    result: Result<(), anyhow::Error>,
    expected: E,
) {
    let error = result.expect_err("succeeded");
    assert_eq!(error.downcast_ref::<E>(), Some(&expected), "{error:?}");
}

/// Writes, reads, commits and reopens a store in `dir`, which `reopen` opens again.
fn reads_what_was_written<D: Dir>(dir: D, reopen: impl Fn(&D) -> D) {
    let store = create(dir, 64 << 20);
    // Small extents, in segments, one in pieces, and a large one, in a file of its own.
    store.write(extent(1), 0, &pattern(1, 0, 3 * BLOCK)).unwrap();
    store.write(extent(2), 2 * BLOCK as u64, &pattern(2, 0, BLOCK)).unwrap();
    store.write(extent(2), 0, &pattern(3, 0, BLOCK)).unwrap();
    store.write(extent(2), 2 * BLOCK as u64, &pattern(4, 0, BLOCK)).unwrap();
    store.write(extent(3), 0, &pattern(5, 0, 40 << 10)).unwrap();

    let check = |store: &Store<D>| {
        assert_eq!(read(store, extent(1), 0, 3 * BLOCK), pattern(1, 0, 3 * BLOCK));
        // Blocks never written read as zeros, within an extent and past its end; the later of
        // two writes of a block holds.
        assert_eq!(
            read(store, extent(2), 0, 4 * BLOCK),
            [pattern(3, 0, BLOCK), vec![0; BLOCK], pattern(4, 0, BLOCK), vec![0; BLOCK]].concat()
        );
        assert_eq!(
            read(store, extent(3), BLOCK as u64, 40 << 10),
            [pattern(5, BLOCK, 36 << 10), vec![0; BLOCK]].concat()
        );
        assert_eq!(
            store.extents().unwrap(),
            vec![
                (extent(1), 3 * BLOCK as u64),
                (extent(2), 3 * BLOCK as u64),
                (extent(3), 40 << 10)
            ]
        );
        assert_eq!(store.info().unwrap().used, 64 << 10);
        assert_eq!(store.info().unwrap().next_sequence, 4);
        error_is(store.read(extent(4), 0, &mut [0; BLOCK]), NoExtent(extent(4)));
    };
    check(&store);
    assert_eq!(store.usage().unwrap().own_bytes, 40 << 10);
    store.commit(Some(b"root")).unwrap();
    check(&store);

    // Uncommitted writes and frees are gone after reopening.
    store.write(extent(9), 0, &pattern(9, 0, BLOCK)).unwrap();
    store.free(extent(1)).unwrap();
    let dir = reopen(store.dir());
    drop(store);
    let store = Store::open(dir, OPTIONS).unwrap();
    check(&store);
    assert_eq!(store.root().unwrap(), b"root");
    assert!(store.epoch().unwrap() >= 1);
}

#[test]
fn reads_what_was_written_in_memory() {
    reads_what_was_written(MemDir::new(), MemDir::clone);
}

#[test]
fn reads_what_was_written_on_the_host() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().to_owned();
    reads_what_was_written(HostDir::new(&path, true).unwrap(), |dir| {
        HostDir::new(dir.path(), true).unwrap()
    });
}

/// Extents are written once: commits seal them. A store writes only its branch's extents.
#[test]
fn writes_once() {
    let store = create(MemDir::new(), 1 << 20);
    assert!(!store.has_open().unwrap());
    store.write(extent(1), 0, &[1; BLOCK]).unwrap();
    store.write(extent(1), BLOCK as u64, &[2; BLOCK]).unwrap();
    assert!(store.has_open().unwrap());
    store.commit(None).unwrap();
    assert!(!store.has_open().unwrap());
    error_is(store.write(extent(1), 2 * BLOCK as u64, &[3; BLOCK]), SealedExtent(extent(1)));
    error_is(
        store.write(ExtentId::new(BRANCH.id + 1, 1), 0, &[3; BLOCK]),
        OtherBranch(ExtentId::new(BRANCH.id + 1, 1)),
    );
    assert!(store.write(extent(2), 1, &[3; BLOCK]).is_err(), "unaligned");
    assert_eq!(read(&store, extent(1), 0, 2 * BLOCK), [[1; BLOCK], [2; BLOCK]].concat());
}

/// fxdax: an extent freed once sealed is written no more, nor opened, though the store holds it
/// no longer, nor once it opens again, from its deltas or its checkpoint: forks, views and caches
/// that hold it by its ID would take another's data for it (TASK-79.6). One freed while open,
/// which nothing but its writer saw, may be written again; and one never written, though below
/// the branch's next sequence number, as Fxfs's journal writes the tail it minted ahead in an
/// earlier mount.
#[test]
fn freed_extents_are_written_no_more() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    store.write(extent(1), 0, &[1; BLOCK]).unwrap();
    store.write(extent(2), 0, &[2; BLOCK]).unwrap();
    store.commit(None).unwrap();
    store.free(extent(1)).unwrap();
    error_is(store.write(extent(1), 0, &[3; BLOCK]), Spent(extent(1)));
    store.commit(None).unwrap();
    error_is(store.write(extent(1), 0, &[3; BLOCK]), Spent(extent(1)));
    error_is(store.write(extent(2), 0, &[3; BLOCK]), SealedExtent(extent(2)));

    store.write(extent(4), 0, &[4; BLOCK]).unwrap();
    store.free(extent(4)).unwrap();
    store.write(extent(4), 0, &[5; BLOCK]).unwrap();
    store.write(extent(5), 0, &[5; BLOCK]).unwrap();
    store.commit(None).unwrap();

    // From its deltas; then from the checkpoint that opening wrote.
    let store = reopened(store, &dir);
    error_is(store.write(extent(1), 0, &[6; BLOCK]), Spent(extent(1)));
    store.free(extent(2)).unwrap();
    store.commit(None).unwrap();
    let store = reopened(store, &dir);
    for sequence in [1, 2] {
        error_is(store.write(extent(sequence), 0, &[6; BLOCK]), Spent(extent(sequence)));
    }
    store.write(extent(3), 0, &[7; BLOCK]).unwrap();
    store.commit(None).unwrap();
    assert_eq!(read(&store, extent(3), 0, BLOCK), [7; BLOCK]);
    assert_eq!(read(&store, extent(4), 0, BLOCK), [5; BLOCK]);
}

/// The store holds at most its capacity, and frees make room.
#[test]
fn capacity() {
    let store = create(MemDir::new(), 64 << 10);
    store.write(extent(1), 0, &[1; 48 << 10]).unwrap();
    let full = store.write(extent(2), 0, &[2; 32 << 10]);
    // Which bound refused it, and its numbers (TASK-79.105).
    assert_eq!(
        format!("{:#}", full.as_ref().unwrap_err()),
        "49152 bytes used, 0 reserved and 32768 written past the capacity's 65536: the extent \
         store is full"
    );
    error_is(full, NoSpace);
    assert_eq!(store.info().unwrap().available, 16 << 10);
    store.free(extent(1)).unwrap();
    store.free(extent(1)).expect("freeing a freed extent failed");
    store.write(extent(2), 0, &[2; 32 << 10]).unwrap();
    // Freeing doesn't make sequence numbers available again.
    assert_eq!(store.info().unwrap().next_sequence, 3);
}

/// Freed extents' files, and segments whose extents are all freed, are removed once the frees are
/// committed, and fxdax: their segments acknowledged in the bucket (TASK-165.9).
#[test]
fn frees_remove_files() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    // Fills the first segment, so that the commit starts another.
    for sequence in 0..16 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
    }
    store.write(extent(16), 0, &pattern(16, 0, 32 << 10)).unwrap();
    store.commit(None).unwrap();
    settled(&store);
    // The first segment, closed, and the next, open.
    // fxdax: segments' files, not the next segment's, placed ahead of it (TASK-384).
    let segments = || {
        let count = |kind| segment_files(&store, &dir, kind).len();
        (count("segments"), count("open-segments"))
    };
    assert_eq!(segments(), (1, 1));
    assert_eq!(dir.list(&of("extents", BRANCH.id)).unwrap(), vec!["0000000000000010"]);
    assert_eq!(dir.list(&of("open-extents", BRANCH.id)).unwrap(), Vec::<String>::new());

    for sequence in 0..=16 {
        store.free(extent(sequence)).unwrap();
    }
    // fxdax: not until the frees are committed, and the segment is acknowledged in the bucket,
    // the write-ahead log keeping it until then (TASK-165.9).
    assert_eq!(segments(), (1, 1));
    assert_eq!(dir.list(&of("extents", BRANCH.id)).unwrap().len(), 1);
    store.commit(None).unwrap();
    assert_eq!(segments(), (1, 1));
    assert_eq!(dir.list(&of("extents", BRANCH.id)).unwrap().len(), 1);
    error_is(store.read(extent(16), 0, &mut [0; BLOCK]), NoExtent(extent(16)));
    let closed = closed_one(&store);
    store.acknowledged(closed.number).unwrap();
    store.commit(None).unwrap();
    assert_eq!(segments(), (0, 1));
    assert_eq!(dir.list(&of("extents", BRANCH.id)).unwrap(), Vec::<String>::new());
}

/// fxdax: a commit that fills the open segment closes it, and leaves its framing to a thread of
/// its own: its extents read from its raw file meanwhile, and the flush of the commit waits for
/// its framed file, which holds them from then on, the uploader takes, and its raw file goes for
/// (TASK-41).
#[test]
fn segments_close_framed_by_the_commits_flush() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    for sequence in 0..17 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
    }
    store.commit_ordered(None).unwrap();
    // fxdax: segments' files, not the next segment's, placed ahead of it (TASK-384).
    let count = |kind| segment_files(&store, &dir, kind).len();
    // Closed, and the next open: its raw file stays until the commit is durable.
    assert_eq!(count("open-segments"), 2);
    let reads = || (0..17).map(|sequence| read(&store, extent(sequence), 0, BLOCK));
    let written: Vec<_> = (0..17).map(|sequence| pattern(sequence as u8, 0, BLOCK)).collect();
    assert_eq!(reads().collect::<Vec<_>>(), written);
    // Not the uploader's until its commit is durable (TASK-165.9).
    assert!(store.closed(usize::MAX).unwrap().is_empty());

    // fxdax: framed, then its commit's flush: its raw file the spare, the segment after the next
    // one's placed ahead of it as the next opened, before, its raw file made anew (TASK-384).
    store.wait_framed().unwrap();
    store.flush().unwrap();
    assert_eq!((count("segments"), count("open-segments")), (1, 1));
    assert_eq!(reads().collect::<Vec<_>>(), written);
    let closed = store.closed(usize::MAX).unwrap();
    let [segment] = &closed[..] else { panic!("{} segments closed", closed.len()) };
    let data = extent_frame::data(&segment.bytes().unwrap()).unwrap();
    assert_eq!(&data[..BLOCK], &pattern(0, 0, BLOCK)[..]);
}

/// The one segment `store` closed whose commits are durable, which [`Store::closed`] gives.
fn closed_one(store: &Store<MemDir>) -> Closed<MemFile> {
    let closed = store.closed(usize::MAX).unwrap();
    let count = closed.len();
    let [closed] = <[_; 1]>::try_from(closed).unwrap_or_else(|_| panic!("{count} segments closed"));
    closed
}

/// fxdax: the uploader takes the segments of the write-ahead log once their commits are durable,
/// whose footers say what the commits wrote, with its lengths, freed, and set as the root; the
/// data of extents freed since stays in the log's files, which the uploader reads, until the
/// segments are acknowledged (TASK-165.9).
#[test]
fn the_write_ahead_log_keeps_freed_data_until_acknowledged() {
    use extent_frame::segment::{Location, extent_key};
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    store.commit(None).unwrap();
    assert!(store.closed(usize::MAX).unwrap().is_empty());

    for sequence in 2..=16 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
    }
    store.write(extent(17), 0, &pattern(17, 0, 32 << 10)).unwrap();
    // Filling its segment, which it closes; not taken until durable.
    store.commit_ordered(Some(b"root")).unwrap();
    assert!(store.closed(usize::MAX).unwrap().is_empty());
    store.flush().unwrap();
    let writing = closed_one(&store);
    let written: Vec<_> =
        writing.footer.contents.extents.iter().map(|&(extent, _, len)| (extent, len)).collect();
    let blocks: Vec<_> = (1..=16).map(|sequence| (extent(sequence), BLOCK as u64)).collect();
    assert_eq!(written, blocks);
    let own = Location {
        key: extent_key(BRANCH.cell, WRITER.epoch(), writing.footer.seq, extent(17)),
        offset: 0,
        len: 32 << 10,
    };
    assert_eq!(writing.footer.own().collect::<Vec<_>>(), [(extent(17), own)]);
    let (frees, root) = (&writing.footer.contents.frees, &writing.footer.contents.root);
    assert_eq!((frees.clone(), root.clone()), (vec![], Some(b"root".to_vec())));
    assert_eq!(store.wal_bytes().unwrap(), 16 * BLOCK as u64 + (32 << 10));

    for sequence in 1..=17 {
        store.free(extent(sequence)).unwrap();
    }
    store.commit(None).unwrap();
    assert!(store.close_now().unwrap());
    let freeing = closed_one(&store);
    assert_eq!(freeing.footer.contents.frees, (1..=17).map(extent).collect::<Vec<_>>());
    // Freed: the store's users read them not, the log holds them, which the uploader reads.
    error_is(store.read(extent(17), 0, &mut [0; BLOCK]), NoExtent(extent(17)));
    let [(_, own_bytes)] = &writing.own_bytes().unwrap()[..] else { panic!("no extent's own") };
    assert_eq!(extent_frame::data(own_bytes).unwrap(), pattern(17, 0, 32 << 10));
    let data = extent_frame::data(&writing.bytes().unwrap()).unwrap();
    assert_eq!(&data[2 * BLOCK..3 * BLOCK], &pattern(3, 0, BLOCK)[..]);
    store.commit(None).unwrap();
    assert_eq!(dir.list(&of("extents", BRANCH.id)).unwrap().len(), 1);
    let segments = dir.list(&of("segments", BRANCH.id)).unwrap().len();

    // Acknowledged, they go.
    for closed in [writing, freeing] {
        store.acknowledged(closed.number).unwrap();
    }
    assert_eq!(store.wal_bytes().unwrap(), 0);
    store.commit(None).unwrap();
    assert_eq!(dir.list(&of("extents", BRANCH.id)).unwrap(), Vec::<String>::new());
    assert!(dir.list(&of("segments", BRANCH.id)).unwrap().len() < segments);
}

/// fxdax: where in the bucket an extent of `len` bytes is: at `offset` in the object at `key`.
fn bucket_location(key: &str, offset: u64, len: u64) -> Location {
    Location { key: key.to_owned(), offset, len }
}

/// fxdax: a store holds the extents the bucket holds of its branch as of its opening: recovered
/// there, read there, at their locations there, which a read's error names, and freed like others
/// (TASK-165.8, TASK-165.9).
#[test]
fn extents_the_bucket_holds_are_held_without_their_data() {
    let dir = MemDir::new();
    let location =
        bucket_location("segments/0000000000000007/0000000000000003", 8192, 3 * BLOCK as u64);
    let other = bucket_location("extents/0000000000000008/1.2.9", 0, BLOCK as u64);
    let mut state = extent_remote::State::new(BRANCH);
    state.extents.insert(extent(5), location.clone());
    state.extents.insert(ExtentId::new(BRANCH.id + 1, 9), other);
    state.next_sequence = 6;
    let check = |store: &Store<MemDir>| {
        assert_eq!(store.extent(extent(5)).unwrap(), Some((3 * BLOCK as u64, false)));
        let remote = RemoteData { extent: extent(5), location: location.clone() };
        error_is(store.read(extent(5), 0, &mut [0; BLOCK]), remote);
        assert_eq!(store.place(extent(5)).unwrap(), Some(Place::Remote(location.clone())));
        assert_eq!(store.info().unwrap().used, 4 * BLOCK as u64);
        assert_eq!(store.info().unwrap().next_sequence, 6);
        // Sealed.
        error_is(store.write(extent(5), 0, &[0; BLOCK]), SealedExtent(extent(5)));
    };
    let store = created_on(dir.clone(), 1 << 20, state.clone(), WRITER);
    check(&store);
    drop(store);
    let store = opened_on(dir, state, WRITER);
    check(&store);
    store.free(extent(5)).unwrap();
    store.commit(None).unwrap();
    assert!(store.close_now().unwrap());
    assert_eq!(closed_one(&store).footer.contents.frees, [extent(5)]);
    error_is(store.read(extent(5), 0, &mut [0; BLOCK]), NoExtent(extent(5)));
}

/// fxdax: commits append their records to the open segment's log, its alone, and the store opened
/// again holds them all (TASK-165.9). The logs of the segments the commits closed go once their
/// framed files are durable, after the commits return (framings run off them, TASK-165.9, and
/// make their framed files durable on a thread of the store's, TASK-310): counted once the store
/// settled. Here none is made durable until the commits are made, as on a busy disk; counted at
/// once, as the test did, the closed segments' logs were still there, (12, 0), and on a CI
/// runner two had gone together, two spares kept, (1, 2); so too before TASK-310 with framings
/// as slow (TASK-370).
#[test]
fn commits_reopen_from_their_logs() {
    use std::sync::Mutex;
    use std::sync::mpsc::channel;
    let dir = MemDir::new();
    let store = create(dir.clone(), 64 << 20);
    let (go, gone) = channel::<()>();
    let gone = Mutex::new(gone);
    let held = fail::FailGuard::with_callback("segments::making_durable", move || {
        let _ = gone.lock().unwrap().recv();
    })
    .unwrap();
    for sequence in 0..200 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
        if sequence % 3 == 0 {
            store.free(extent(sequence)).unwrap();
        }
        store.commit(Some(format!("root {sequence}").as_bytes())).unwrap();
    }
    drop((go, held));
    settled(&store);
    store.prepared().unwrap();
    // The open segment's log, and under spares' names the logs of segments gone kept as spares,
    // two at most (TASK-301), and the next segment's, prepared (TASK-333), placed under its name
    // (TASK-384): a segment opened takes the one prepared, else a spare. The closed segments' logs
    // went together once settled, their framed files durable at last: two kept.
    let logs = |store: &Store<MemDir>| {
        let names = dir.list(&of("logs", BRANCH.id)).unwrap();
        let spares = names.iter().filter(|name| name.starts_with("spare-")).count();
        (segment_files(store, &dir, "logs").len(), store.placed().is_some(), spares)
    };
    assert_eq!(logs(&store), (1, true, 2));
    drop(store);
    let store = Store::open(dir.clone(), OPTIONS).unwrap();
    store.prepared().unwrap();
    // The opening's open segment took a spare; the log of the one it closed again went, and the
    // next's, placed ahead of it, holding nothing (TASK-384).
    assert_eq!(logs(&store), (1, false, 1));
    assert_eq!(store.root().unwrap(), b"root 199");
    assert_eq!(store.epoch().unwrap(), 200);
    let held: Vec<u64> =
        store.extents().unwrap().iter().map(|(extent, _)| extent.sequence).collect();
    assert_eq!(held, (0..200).filter(|sequence| sequence % 3 != 0).collect::<Vec<_>>());
    for sequence in held {
        assert_eq!(read(&store, extent(sequence), 0, BLOCK), pattern(sequence as u8, 0, BLOCK));
    }
}

/// What a store holds as of its last commit, as its users see it: its epoch, root and next
/// sequence number, its extents with their lengths and where they are, and what it spent.
type Held = (u64, Vec<u8>, u64, Vec<(ExtentId, u64, Place)>, extent_frame::spent::Spent);

/// What `store` holds ([`Held`]), its open segment closed by a commit.
fn closed_state<D: Dir>(store: &Store<D>) -> Held {
    store.close_segment().unwrap();
    store.commit(None).unwrap();
    settled(store);
    let next_sequence = store.info().unwrap().next_sequence;
    let (epoch, root) = (store.epoch().unwrap(), store.root().unwrap());
    (epoch, root, next_sequence, store.places().unwrap(), store.spent().unwrap())
}

/// fxdax: `store` once the segments its commits closed are framed, and the raw files and logs of
/// those whose commits are durable are gone: as a flush after their framings leaves it, none of
/// which a flush waits for (TASK-165.9).
fn settled<D: Dir>(store: &Store<D>) {
    store.wait_framed().unwrap();
    store.flush().unwrap();
}

/// What `store`, of `dir`, holds once opened again, its write-ahead log's footers replayed
/// (TASK-165.9): what [`closed_state`] said.
fn reopened_state(store: Store<MemDir>, dir: &MemDir) -> (Held, Store<MemDir>) {
    let store = reopened(store, dir);
    let next_sequence = store.info().unwrap().next_sequence;
    let (epoch, root) = (store.epoch().unwrap(), store.root().unwrap());
    let held = (epoch, root, next_sequence, store.places().unwrap(), store.spent().unwrap());
    (held, store)
}

/// fxdax: an extent freed while open, which no commit held, is not spent: the store writes it
/// again, and so does the store opened again, its write-ahead log's footers spending those freed
/// once sealed alone, as the store does (TASK-160.15, TASK-165.9). One sealed, and freed by a
/// later commit of the same segment, is spent, though no footer names it held.
#[test]
fn reopening_spends_what_the_store_spends() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    for (sequence, len) in [(1, BLOCK), (2, BLOCK), (3, 40 << 10)] {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, len)).unwrap();
    }
    // Freed open, in segments and in a raw file of its own.
    store.free(extent(2)).unwrap();
    store.free(extent(3)).unwrap();
    store.commit(None).unwrap();
    // Sealed, then freed.
    store.free(extent(1)).unwrap();
    let state = closed_state(&store);
    assert_eq!(state.4.runs().collect::<Vec<_>>(), [(1, 2)]);
    let (reopened, store) = reopened_state(store, &dir);
    // Its next sequence past what it holds, none: those freed, its next writer's floor is past.
    let (epoch, root, _, places, spent) = state;
    assert_eq!(reopened, (epoch, root, 0, places, spent));
    for sequence in [2, 3] {
        store.write(extent(sequence), 0, &pattern(9, 0, BLOCK)).unwrap();
    }
}

/// A commit seals each extent in one run of the open segment, however it was written, or, sealed
/// large, in a file of its own.
#[test]
fn sealed_extents_are_one_run_each() {
    let store = create(MemDir::new(), 1 << 20);
    // Interleaved, and rewritten, and with a hole.
    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    store.write(extent(2), 0, &pattern(2, 0, BLOCK)).unwrap();
    store.write(extent(1), BLOCK as u64, &pattern(1, BLOCK, BLOCK)).unwrap();
    store.write(extent(1), 0, &pattern(3, 0, BLOCK)).unwrap();
    store.write(extent(3), 2 * BLOCK as u64, &pattern(4, 0, BLOCK)).unwrap();
    // Small at first, large at its seal.
    store.write(extent(4), 0, &pattern(5, 0, BLOCK)).unwrap();
    store.write(extent(4), BLOCK as u64, &pattern(5, BLOCK, 32 << 10)).unwrap();
    assert_eq!(store.places().unwrap()[0].2, Place::Pieces);
    store.commit(None).unwrap();

    let places = store.places().unwrap();
    for (extent, len, place) in &places[..3] {
        let Place::Run { branch, offset, .. } = place else { panic!("{extent:?} is {place:?}") };
        assert_eq!((*branch, offset % BLOCK as u64), (BRANCH.id, 0), "{extent:?} at {offset}");
        assert!(*len > 0);
    }
    assert_eq!(places[3], (extent(4), (32 << 10) + BLOCK as u64, Place::Own));
    assert_eq!(
        read(&store, extent(1), 0, 2 * BLOCK),
        [pattern(3, 0, BLOCK), pattern(1, BLOCK, BLOCK)].concat()
    );
    assert_eq!(read(&store, extent(2), 0, BLOCK), pattern(2, 0, BLOCK));
    assert_eq!(
        read(&store, extent(3), 0, 3 * BLOCK),
        [vec![0; 2 * BLOCK], pattern(4, 0, BLOCK)].concat()
    );
    assert_eq!(read(&store, extent(4), BLOCK as u64, 32 << 10), pattern(5, BLOCK, 32 << 10));
    let check = |store: &Store<MemDir>| assert_eq!(store.places().unwrap(), places);
    check(&store);
    let dir = store.dir().clone();
    drop(store);
    check(&Store::open(dir, OPTIONS).unwrap());
}

/// Segments close when full, when old, when asked to, and at a fork, their footers after their
/// data; and the branch's state is the replay of its write-ahead log's footers, which the store
/// opened again holds (TASK-165.9), and which its audit checks at each closing.
#[test]
fn closed_segments_hold_the_branchs_history() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 64 << 20);
    // Closed, and open; fxdax: not the next segment's, placed ahead of it (TASK-384).
    let segments = |store: &Store<MemDir>| {
        dir.list(&of("segments", BRANCH.id)).unwrap().len()
            + segment_files(store, &dir, "open-segments").len()
    };
    store.write(extent(0), 0, &pattern(0, 0, BLOCK)).unwrap();
    store.commit(Some(b"one")).unwrap();
    assert_eq!(segments(&store), 1);
    // Asked to.
    let state = closed_state(&store);
    assert_eq!(segments(&store), 2);
    let (reopened, store) = reopened_state(store, &dir);
    assert_eq!(reopened, state);

    // Full: 64 KiB of extents, freed and kept.
    for sequence in 1..=40 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
        if sequence % 3 == 0 {
            store.free(extent(sequence)).unwrap();
        }
        store.commit(Some(format!("root {sequence}").as_bytes())).unwrap();
    }
    assert!(segments(&store) > 2, "no segment filled");
    store.free(extent(0)).unwrap();
    store.commit(None).unwrap();
    let state = closed_state(&store);
    let (reopened, store) = reopened_state(store, &dir);
    assert_eq!(reopened, state);
    // A run's data is in its segment, framed, where its footer says.
    let (_, _, place) = state.3.iter().find(|(id, ..)| *id == extent(40)).unwrap();
    let Place::Run { number, offset, .. } = *place else { panic!("{place:?}") };
    let file = dir.open(&format!("{}/{number:016x}", of("segments", BRANCH.id)), true).unwrap();
    let mut bytes = vec![0; extent_store::File::size(&file).unwrap() as usize];
    extent_store::File::read_at(&file, &mut bytes, 0).unwrap();
    let data = extent_frame::data(&bytes).unwrap();
    assert_eq!(data[offset as usize..][..BLOCK], pattern(40, 0, BLOCK));

    // Old.
    drop(store);
    let options = Options { segment_age: std::time::Duration::ZERO, ..OPTIONS };
    let store = Store::open(dir.clone(), options).unwrap();
    let before = segments(&store);
    store.write(extent(50), 0, &pattern(50, 0, BLOCK)).unwrap();
    store.commit(None).unwrap();
    settled(&store);
    assert_eq!(segments(&store), before + 1);

    // At a fork.
    store.write(extent(51), 0, &pattern(51, 0, BLOCK)).unwrap();
    store
        .commit_fork(Child {
            branch: extent_remote::layout::test_branch(BRANCH.id + 1),
            owner: 0,
            point: false,
        })
        .unwrap();
    let places = store.places().unwrap();
    let (reopened, _) = reopened_state(store, &dir);
    assert_eq!(reopened.3, places);
}

/// A branch with closed segments, and one not, reopens in a directory of the host's, whose
/// extents' files bypass its page cache where its file system allows: their trailers' and
/// footers' reads are aligned as such reads must be (`O_DIRECT` on Linux).
#[test]
fn closed_segments_reopen_on_the_host() {
    let temp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let open = || HostDir::new(temp.path(), true).unwrap();
    let store = create(open(), 64 << 20);
    store.write(extent(1), 0, &pattern(1, 0, 3 * BLOCK)).unwrap();
    let state = closed_state(&store);
    store.write(extent(2), 0, &pattern(2, 0, BLOCK)).unwrap();
    store.commit(Some(b"root")).unwrap();
    drop(store);
    let store = Store::open(open(), OPTIONS).unwrap();
    assert_eq!(read(&store, extent(1), 0, 3 * BLOCK), pattern(1, 0, 3 * BLOCK));
    assert_eq!(read(&store, extent(2), 0, BLOCK), pattern(2, 0, BLOCK));
    assert!(store.epoch().unwrap() > state.0);
    let state = closed_state(&store);
    drop(store);
    let store = Store::open(open(), OPTIONS).unwrap();
    assert_eq!(closed_state(&store).3, state.3);
}

/// Closed segments and sealed extents are stored compressed, but for parts that do not compress,
/// and read as written: while open, raw, once closed, framed, and once reopened.
#[test]
fn closed_segments_and_sealed_extents_are_compressed() {
    let dir = MemDir::new();
    let options = Options { segment_size: 4 << 20, large_extent: 8 << 20, ..OPTIONS };
    let description = Description { block_size: BLOCK as u32, capacity: 64 << 20, branch: BRANCH };
    let store = Store::create(dir.clone(), description, options).unwrap();
    let mib = 1 << 20;
    // In segments, compressible and not, across parts; and on their own, compressible and not.
    let contents = [
        (extent(1), pattern(1, 0, 3 * mib / 2)),
        (extent(2), noise(2, mib + 8 * BLOCK)),
        (extent(3), pattern(3, 0, 12 * BLOCK)),
        (extent(4), pattern(4, 0, 9 * mib)),
        (extent(5), [noise(5, 8 * mib), pattern(5, 0, mib)].concat()),
    ];
    for (extent, data) in &contents {
        store.write(*extent, 0, data).unwrap();
    }
    let reads = |store: &Store<MemDir>| {
        for (extent, data) in &contents {
            assert!(read(store, *extent, 0, data.len()) == *data, "{extent:?}");
            // A range across parts.
            let (at, len) = (mib - 2 * BLOCK, 5 * BLOCK);
            if data.len() >= at + len {
                let range = read(store, *extent, at as u64, len);
                assert!(range == data[at..at + len], "{extent:?} at {at}");
            }
        }
    };
    reads(&store);
    store.close_segment().unwrap();
    store.commit(None).unwrap();
    reads(&store);

    // Framed on a thread of its own, made durable on another: its framed file's size counts once
    // its framing ends, which the commit waits not for (TASK-310).
    store.wait_framed().unwrap();
    let usage = store.usage().unwrap();
    let in_segments: u64 = contents[..3].iter().map(|(_, data)| data.len() as u64).sum();
    assert_eq!((usage.segment_bytes, usage.own_bytes), (in_segments, 18 * mib as u64));
    // The noise takes what it is, the patterns next to nothing.
    let noise_bytes = (mib + 8 * BLOCK + 8 * mib) as u64;
    assert!(usage.disk_bytes >= noise_bytes, "{usage:?}");
    assert!(usage.disk_bytes < noise_bytes + 3 * mib as u64 / 10, "{usage:?}");
    drop(store);
    let store = Store::open(dir, options).unwrap();
    reads(&store);
    assert_eq!(store.usage().unwrap().disk_bytes, usage.disk_bytes);
}

/// A store full of data that compresses takes a fraction of it on disk.
#[test]
fn compressible_data_takes_less_disk() {
    let store = create(MemDir::new(), 64 << 20);
    for sequence in 0..64 {
        let len = if sequence % 8 == 7 { 64 << 10 } else { 4 * BLOCK };
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, len)).unwrap();
        store.commit(None).unwrap();
    }
    store.close_segment().unwrap();
    store.commit(None).unwrap();
    let usage = store.usage().unwrap();
    let held = usage.segment_bytes + usage.own_bytes;
    assert_eq!(held, 56 * 4 * BLOCK as u64 + 8 * (64 << 10));
    assert!(usage.disk_bytes * 4 < held, "{usage:?}");
}

/// fxdax: a segment a commit closes is that commit's record once framed, its framing having made
/// its framed file, which holds the commit's data, durable: a crash after leaves the branch as of
/// that commit, though no flush made it durable, the framed file read, its raw file gone once
/// opened (TASK-165.9). Before, a crash closes it again from its log (`wal`'s tests). The next
/// segment's raw file and log, made as it closed, are durable with the next flush, which syncs
/// their directories, not with the framing (TASK-310): a crash before may keep them or not, and
/// the opening opens the segment after the last it finds.
#[test]
fn segments_closed_by_commits_are_their_records_once_framed() {
    for seed in 0..16 {
        let dir = MemDir::serial();
        let store = create(dir.clone(), 64 << 20);
        store.write(extent(1), 0, &pattern(1, 0, 3 * BLOCK)).unwrap();
        store.commit(Some(b"one")).unwrap();
        store.write(extent(2), 0, &pattern(2, 0, BLOCK)).unwrap();
        store.close_segment().unwrap();
        store.commit_ordered(Some(b"two")).unwrap();
        let crashed = dir.crash(seed);
        drop(store);
        // The opening's open segment is past every one the crash kept a file of: segment 1's,
        // opened, and fxdax: segment 2's, placed ahead of it (TASK-384).
        let kept = ["segments", "open-segments", "logs"].iter().flat_map(|kind| {
            let names = crashed.list(&of(kind, BRANCH.id)).unwrap();
            names.into_iter().filter_map(|name| u64::from_str_radix(&name, 16).ok())
        });
        let open = format!("{:016x}", kept.max().unwrap() + 1);
        let store = Store::open(crashed.clone(), OPTIONS).unwrap();
        assert_eq!(store.root().unwrap(), b"two", "seed {seed}");
        assert_eq!(read(&store, extent(1), 0, 3 * BLOCK), pattern(1, 0, 3 * BLOCK));
        assert_eq!(read(&store, extent(2), 0, BLOCK), pattern(2, 0, BLOCK));
        assert_eq!(crashed.list(&of("open-segments", BRANCH.id)).unwrap(), [open], "seed {seed}");
    }
}

/// fxdax: a segment closed between commits, the last of whose commits is not durable, is closed
/// again with no footer by the branch's next opening: which, crashing as it does, leaves its data
/// whole, for the opening after (TASK-79.33).
#[test]
fn segments_closed_between_commits_survive_crashes_closing_them_again() {
    for seed in 0..64 {
        let dir = MemDir::serial();
        let store = create(dir.clone(), 1 << 20);
        store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
        store.commit(None).unwrap();
        store.write(extent(2), 0, &pattern(2, 0, BLOCK)).unwrap();
        store.commit_ordered(None).unwrap();
        assert!(store.close_now().unwrap());
        let crashed = dir.crash(seed);
        drop(store);
        // An opening stopped after each of its first operations, as by a crash, then another.
        for operations in 0.. {
            let opening = crashed.crash(seed);
            opening.fail_after(Some(operations));
            let opened = Store::open(opening.clone(), OPTIONS).map(drop);
            opening.fail_after(None);
            let reopened = Store::open(opening.crash(seed + operations), OPTIONS);
            let reopened = reopened.unwrap_or_else(|error| {
                panic!("seed {seed}, stopped after {operations} operations: {error:#}")
            });
            assert_eq!(read(&reopened, extent(1), 0, BLOCK), pattern(1, 0, BLOCK));
            if opened.is_ok() {
                break;
            }
        }
    }
}

/// fxdax: a segment closed between commits, the commits its footer names durable, whose framing a
/// crash stopped before its framed file was synced, is framed again by the branch's next opening,
/// from its raw file: its framed file may be whole in its trailer and footer, and torn in its data
/// (TASK-79.33).
#[test]
fn segments_closed_between_commits_survive_crashes_framing_them() {
    for seed in 0..64 {
        let dir = MemDir::serial();
        let store = create(dir.clone(), 1 << 20);
        for sequence in 0..4 {
            store.write(extent(sequence), 0, &noise(sequence, BLOCK)).unwrap();
        }
        store.commit(None).unwrap();
        // The next segment's raw file made, the framed file written, then its sync fails.
        dir.fail_after(Some(3));
        assert!(store.close_now().is_err(), "seed {seed}: the closing went on");
        dir.fail_after(None);
        let crashed = dir.crash(seed);
        drop(store);
        let store = Store::open(crashed, OPTIONS).unwrap();
        for sequence in 0..4 {
            let data = read(&store, extent(sequence), 0, BLOCK);
            assert!(data == noise(sequence, BLOCK), "seed {seed}: extent {sequence} differs");
        }
    }
}

/// fxdax: so too if the branch is opened again in the same process, its store failed as the
/// framed file's sync did: the opening makes the framed file durable before its raw file goes,
/// whatever a crash after it then keeps (TASK-79.33).
#[test]
fn segments_closed_between_commits_survive_failures_framing_them() {
    for seed in 0..64 {
        let dir = MemDir::serial();
        let store = create(dir.clone(), 1 << 20);
        for sequence in 0..4 {
            store.write(extent(sequence), 0, &noise(sequence, BLOCK)).unwrap();
        }
        store.commit(None).unwrap();
        dir.fail_after(Some(3));
        assert!(store.close_now().is_err(), "seed {seed}: the closing went on");
        dir.fail_after(None);
        drop(store);
        drop(Store::open(dir.clone(), OPTIONS).unwrap());
        let store = Store::open(dir.crash(seed), OPTIONS).unwrap();
        for sequence in 0..4 {
            let data = read(&store, extent(sequence), 0, BLOCK);
            assert!(data == noise(sequence, BLOCK), "seed {seed}: extent {sequence} differs");
        }
    }
}

/// fxdax: a segment closed between commits keeps its raw file and log until a commit after it is
/// durable, with the commits its footer names, though its framed file, durable once framed, holds
/// them (TASK-92.16, TASK-165.9): a crash then leaves the branch as of its last commit, its
/// framed file read, whose footer names it.
#[test]
fn segments_closed_between_commits_keep_their_raw_files_until_a_commit_after_is_durable() {
    use extent_frame::segment::{Footer, tag};
    let path = |kind| format!("{}/{:016x}", of(kind, BRANCH.id), 0);
    for seed in 0..16 {
        let dir = MemDir::serial();
        let store = create(dir.clone(), 1 << 20);
        store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
        store.commit(None).unwrap();
        store.write(extent(2), 0, &pattern(2, 0, BLOCK)).unwrap();
        store.commit_ordered(None).unwrap();
        assert!(store.close_now().unwrap());
        let crashed = dir.crash(seed);
        assert!(dir.open(&path("open-segments"), true).is_ok(), "seed {seed}: the raw file went");
        assert!(dir.open(&path("logs"), false).is_ok(), "seed {seed}: the log went");
        store.commit(None).unwrap();
        assert!(dir.open(&path("open-segments"), true).is_err(), "seed {seed}: the raw file stays");
        assert!(dir.open(&path("logs"), false).is_err(), "seed {seed}: the log stays");
        drop(store);
        let store = Store::open(crashed.clone(), OPTIONS).unwrap();
        assert_eq!(store.epoch().unwrap(), 2, "seed {seed}");
        assert_eq!(read(&store, extent(2), 0, BLOCK), pattern(2, 0, BLOCK));
        let framed = extent_store::read_whole(&crashed.open(&path("segments"), true).unwrap());
        let footer = extent_frame::footer(&framed.unwrap()).unwrap().to_vec();
        let footer = Footer::decode(&footer).unwrap();
        assert_eq!(footer.contents.commits, [tag(0, 2)], "seed {seed}");
    }
}

/// Reads find a part stored compressed packed, to be unpacked from its stored bytes, then, once
/// read, unpacked; a part stored raw, in place.
#[test]
fn locating_parts_stored_compressed() {
    let options = Options { segment_size: 4 << 20, large_extent: 8 << 20, ..OPTIONS };
    let description = Description { block_size: BLOCK as u32, capacity: 64 << 20, branch: BRANCH };
    let store = Store::create(MemDir::new(), description, options).unwrap();
    store.write(extent(1), 0, &pattern(1, 0, 16 * BLOCK)).unwrap();
    store.write(extent(2), 0, &noise(2, 16 * BLOCK)).unwrap();
    store.close_segment().unwrap();
    store.commit(None).unwrap();
    settled(&store);
    let kinds = |extent| -> Vec<&'static str> {
        (store.locate(extent, 4 * BLOCK as u64, 2 * BLOCK as u64).unwrap().iter())
            .map(|part| match part {
                Part::Data { .. } => "data",
                Part::Packed { .. } => "packed",
                Part::Unpacked { .. } => "unpacked",
                Part::Zeros(_) => "zeros",
            })
            .collect()
    };
    // Both in the segment's first part, which compresses.
    assert_eq!(kinds(extent(1)), ["packed"]);
    let [Part::Packed { part, skip, len }] = &store.locate(extent(1), 0, BLOCK as u64).unwrap()[..]
    else {
        panic!("not packed");
    };
    assert_eq!((*skip, *len, part.read_range().start % BLOCK as u64), (0, BLOCK as u64, 0));
    // Its range read, as a reader bypassing the store reads it, unpacks to its data.
    let range = part.read_range();
    let mut bytes = extent_store::Aligned::new((range.end - range.start) as usize);
    part.file.read_at(bytes.as_mut_slice(), range.start).unwrap();
    assert_eq!(store.unpack(part, bytes).unwrap()[..16 * BLOCK], pattern(1, 0, 16 * BLOCK));
    let unpacked = store.read_packed(part).unwrap();
    assert_eq!(unpacked[..16 * BLOCK], pattern(1, 0, 16 * BLOCK));
    assert_eq!(kinds(extent(1)), ["unpacked"]);
    assert_eq!(kinds(extent(2)), ["unpacked"]);
    assert_eq!(read(&store, extent(2), 0, 16 * BLOCK), noise(2, 16 * BLOCK));
}

/// fxdax: a resize is a commit of its own, which seals nothing, a segment's footer saying it, as
/// object storage keeps the branch's capacity: the last resize of its commits', or none if none
/// resized it (TASK-165.3, TASK-165.9). The extents open stay open.
#[test]
fn resizes_are_commits_of_their_own() {
    let store = create(MemDir::new(), 1 << 20);
    let capacity = |store: &Store<MemDir>| {
        store.close_now().unwrap();
        store.flush().unwrap();
        closed_one(store).footer.contents.capacity
    };
    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    let epoch = store.epoch().unwrap();
    store.set_capacity(2 << 20).unwrap();
    store.set_capacity(3 << 20).unwrap();
    assert_eq!(store.epoch().unwrap(), epoch + 2);
    // Open still: written on, sealed by a commit.
    assert_eq!(store.extent(extent(1)).unwrap(), Some((BLOCK as u64, true)));
    store.write(extent(1), BLOCK as u64, &pattern(1, BLOCK, BLOCK)).unwrap();
    assert_eq!(capacity(&store), Some(3 << 20));
    store.commit(None).unwrap();
    assert_eq!(capacity(&store), None);
    store.set_capacity(4 << 20).unwrap();
    assert_eq!(capacity(&store), Some(4 << 20));
    assert_eq!(store.info().unwrap().capacity, 4 << 20);
    assert_eq!(read(&store, extent(1), 0, 2 * BLOCK), pattern(1, 0, 2 * BLOCK));
}

/// fxdax: opened by a writer, the store closes its segments for the writer, as object storage
/// keeps them: numbered on from where the writer's chain is, each following the one before, its
/// commits tagged with the writer's epoch, its extents of their own at the writer's keys; and
/// keeps each, once its commits are durable, for the uploader, until it is uploaded. Opened as
/// another branch's, it refuses.
#[test]
fn segments_close_for_the_writer_of_their_chain() {
    use extent_frame::segment::{Footer, Location, extent_key, tag};
    let dir = MemDir::new();
    let state = extent_remote::State::new(BRANCH);
    let store =
        created_on(dir.clone(), 1 << 20, state, Chain::new(BRANCH, 3, 5, Some((4, 3)), 100));
    drop(store);
    let other = extent_remote::State::new(extent_remote::layout::test_branch(BRANCH.id + 1));
    let opening = Opening::new(extent_remote::layout::test_branch(BRANCH.id + 1));
    let refused =
        Store::recover(dir.clone()).unwrap().open(OPTIONS, Opening { state: other, ..opening });
    assert_eq!(refused.err().unwrap().to_string(), "branch 1/1/7 opened as 1/1/8");
    let store = opened_on(
        dir.clone(),
        extent_remote::State::new(BRANCH),
        Chain::new(BRANCH, 3, 5, Some((4, 3)), 100),
    );
    // Nothing committed since the last closing, nothing to close.
    assert!(!store.close_now().unwrap());

    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    store.write(extent(2), 0, &pattern(2, 0, 32 << 10)).unwrap();
    store.commit_ordered(Some(b"one")).unwrap();
    assert!(store.close_now().unwrap());
    // Not until its commit is durable.
    assert!(store.closed(usize::MAX).unwrap().is_empty());
    store.flush().unwrap();
    let closed = store.closed(usize::MAX).unwrap();
    let [first] = &closed[..] else { panic!("{} segments closed", closed.len()) };
    let own = Location { key: extent_key(BRANCH.cell, 3, 5, extent(2)), offset: 0, len: 32 << 10 };
    assert_eq!(
        first.footer,
        Footer {
            branch: BRANCH,
            seq: 5,
            epoch: 3,
            after: Some((4, 3)),
            contents: Contents {
                commits: vec![tag(3, 1)],
                extents: vec![(extent(1), 0, BLOCK as u64)],
                own: vec![(2, 32 << 10)],
                frees: vec![],
                root: Some(b"one".to_vec()),
                next_sequence: 100,
                forks: vec![],
                // Its first commit sets its capacity, which the bucket said none of.
                capacity: Some(1 << 20),
                continued: None,
            },
        }
    );
    assert_eq!(first.footer.own().collect::<Vec<_>>(), [(extent(2), own.clone())]);
    // Its file is the object, footer and all; its extent of its own's, the extent's object.
    let bytes = first.bytes().unwrap();
    let footer = Footer::decode(extent_frame::footer(&bytes).unwrap()).unwrap();
    assert_eq!(footer, first.footer);
    assert_eq!(extent_frame::data(&bytes).unwrap()[..BLOCK], pattern(1, 0, BLOCK));
    let [(key, own_bytes)] = &first.own_bytes().unwrap()[..] else { panic!("no extent's own") };
    assert_eq!((key, extent_frame::data(own_bytes).unwrap()), (&own.key, pattern(2, 0, 32 << 10)));

    // The next follows it; a fork's names the fork.
    store.write(extent(3), 0, &pattern(3, 0, BLOCK)).unwrap();
    store
        .commit_fork(Child {
            branch: extent_remote::layout::test_branch(BRANCH.id + 1),
            owner: 0,
            point: false,
        })
        .unwrap();
    let closed = store.closed(usize::MAX).unwrap();
    let [second] = &closed[..] else { panic!("{} segments closed", closed.len()) };
    assert_eq!((second.footer.seq, second.footer.after), (6, Some((5, 3))));
    assert_eq!(
        (second.footer.contents.commits.clone(), second.footer.contents.forks.clone()),
        (
            vec![tag(3, 2)],
            vec![Child {
                branch: extent_remote::layout::test_branch(BRANCH.id + 1),
                owner: 0,
                point: false
            }]
        )
    );

    // Freed, its extents' data goes once released, but the segment stays until uploaded.
    store.free(extent(1)).unwrap();
    store.commit(None).unwrap();
    store.commit(None).unwrap();
    let held = store.usage().unwrap().segments;
    store.acknowledged(first.number).unwrap();
    store.commit(None).unwrap();
    assert_eq!(store.usage().unwrap().segments, held - 1);
}

/// fxdax: a segment closed between commits holds what the commits made, as of the last: not
/// what was written or freed since, which the next segment holds.
#[test]
fn segments_closed_between_commits_hold_the_last_commits_state() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    store.write(extent(2), 0, &pattern(2, 0, BLOCK)).unwrap();
    store.commit(None).unwrap();
    // Since: one freed, another written.
    store.free(extent(1)).unwrap();
    store.write(extent(3), 0, &pattern(3, 0, BLOCK)).unwrap();
    assert!(store.close_now().unwrap());
    let closed = store.closed(usize::MAX).unwrap();
    let [first] = &closed[..] else { panic!("{} segments closed", closed.len()) };
    let listed: Vec<_> = first.footer.contents.extents.iter().map(|(extent, ..)| *extent).collect();
    assert_eq!((listed, first.footer.contents.frees.clone()), (vec![extent(1), extent(2)], vec![]));
    store.commit(None).unwrap();
    assert!(store.close_now().unwrap());
    let closed = store.closed(usize::MAX).unwrap();
    let [second] = &closed[..] else { panic!("{} segments closed", closed.len()) };
    let listed: Vec<_> =
        second.footer.contents.extents.iter().map(|(extent, ..)| *extent).collect();
    assert_eq!((listed, second.footer.contents.frees.clone()), (vec![extent(3)], vec![extent(1)]));
    assert_eq!(read(&store, extent(3), 0, BLOCK), pattern(3, 0, BLOCK));
}

/// fxdax (TASK-79.19): the uploader takes so many of the segments closed at once, oldest first;
/// the rest wait in the store for its next take.
#[test]
fn closed_segments_are_taken_so_many_at_once() {
    let store = create(MemDir::new(), 1 << 20);
    for sequence in 1..=3 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
        store.close_segment().unwrap();
        store.commit(None).unwrap();
    }
    let taken = |limit| -> Vec<u64> {
        store.closed(limit).unwrap().iter().map(|closed| closed.last()).collect()
    };
    assert_eq!(taken(2), [1, 2]);
    assert_eq!(taken(2), [3]);
    assert_eq!(taken(2), Vec::<u64>::new());
}

/// A store's capacity grows, and shrinks no lower than what it holds, durably: the store reopened
/// has it, and a fork inherits it.
#[test]
fn capacity_resizes() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 128 << 10);
    store.write(extent(1), 0, &pattern(1, 0, 64 << 10)).unwrap();
    store.commit(None).unwrap();
    error_is(store.write(extent(2), 0, &pattern(2, 0, 96 << 10)), NoSpace);
    store.set_capacity(256 << 10).unwrap();
    assert_eq!(
        (store.info().unwrap().capacity, store.info().unwrap().available),
        (256 << 10, 192 << 10)
    );
    store.write(extent(2), 0, &pattern(2, 0, 96 << 10)).unwrap();
    store.commit(None).unwrap();
    let held = (64 + 96) << 10;
    let refused = store.set_capacity(held - BLOCK as u64).unwrap_err();
    assert_eq!(
        refused.downcast_ref::<TooSmall>(),
        Some(&TooSmall { capacity: held - BLOCK as u64, held })
    );
    assert!(store.set_capacity(BLOCK as u64 - 1).is_err());
    store.set_capacity(held).unwrap();
    assert_eq!(store.info().unwrap().available, 0);
    // fxdax: durable, though no commit came since, the index holding it (TASK-165.7).
    drop(store);
    let store = Store::open(dir.clone(), OPTIONS).unwrap();
    assert_eq!(store.info().unwrap().capacity, held);
    store.set_capacity(2 * held).unwrap();
    store.commit(None).unwrap();
    drop(store);
    let store = Store::open(dir, OPTIONS).unwrap();
    assert_eq!(store.info().unwrap().capacity, 2 * held);
    assert_eq!(read(&store, extent(2), 0, 96 << 10), pattern(2, 0, 96 << 10));
}

/// fxdax: a framing writes its framed file back a MiB at a time as it writes it, so that its sync
/// leaves little for the host's disk to write at once: a framed segment's whole data, synced at
/// once, filled the disk's queue, which the syncs of a daemon's turns waited behind, up to a second
/// under pgbench (GCE r728, TASK-165.9). Each commit's sync, of 16 KiB extents in the open
/// segment, a MiB of them, writes back no more.
#[test]
fn framings_write_back_as_they_go() {
    let dir = MemDir::new();
    let options = Options { segment_size: 16 << 20, ..OPTIONS };
    let description = Description { block_size: BLOCK as u32, capacity: 64 << 20, branch: BRANCH };
    let store = Store::create(dir.clone(), description, options).unwrap();
    let len = 16 << 10;
    for sequence in 1..=512 {
        store.write(extent(sequence), 0, &noise(sequence, len)).unwrap();
        if sequence % 64 == 0 {
            store.commit(None).unwrap();
        }
    }
    store.close_segment().unwrap();
    store.commit(None).unwrap();
    store.wait_framed().unwrap();
    let framed = dir.open(&format!("{}/{:016x}", of("segments", BRANCH.id), 0), true).unwrap();
    let size = framed.size().unwrap();
    assert!(size > 8 << 20, "the framed segment holds 8 MiB of noise, not {size} bytes");
    assert_eq!(dir.io().most_unwritten, 1 << 20);
}

/// fxdax: what a guest writes to the open segment is written back as it writes it, its writing
/// back begun a MiB at a time and waited for 8 MiB behind, before any commit, so that the sync of
/// the flush after has little left for the host's disk to write at once: a guest's burst, up to
/// 300 MB a second under pgbench, waited in the host's page cache for the flush of extentd's next
/// turn, which synced 100 MB at once in 250-420 ms (GCE r954); its writing back begun but not
/// waited for filled the disk's queue as well, syncs waiting behind it up to 0.6 s (r956,
/// TASK-221). Red with nothing written back until the flush.
#[test]
fn writes_are_written_back_as_they_go() {
    let dir = MemDir::new();
    let options = Options { segment_size: 64 << 20, ..OPTIONS };
    let description = Description { block_size: BLOCK as u32, capacity: 64 << 20, branch: BRANCH };
    let store = Store::create(dir.clone(), description, options).unwrap();
    let len = 16 << 10;
    for sequence in 1..=2048 {
        store.write(extent(sequence), 0, &noise(sequence, len)).unwrap();
    }
    assert_eq!(dir.io().most_unwritten, 9 << 20);
    store.commit(None).unwrap();
    for sequence in 1..=2048 {
        assert_eq!(read(&store, extent(sequence), 0, len), noise(sequence, len));
    }
}

/// fxdax: the framing of segment 0, which the commit after `store`'s first closes, held where its
/// framed file is created, not yet written, until the returned sender says go or drops: its
/// framing begun once this returns.
fn framing_held(store: &Store<MemDir>) -> (std::sync::mpsc::Sender<()>, fail::FailGuard) {
    use std::sync::Mutex;
    use std::sync::mpsc::channel;
    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    store.commit(None).unwrap();
    store.flush().unwrap();
    let (waiting, waits) = channel();
    let (go, gone) = channel::<()>();
    let gone = Mutex::new(gone);
    let guard = fail::FailGuard::with_callback("segments::framing", move || {
        let _ = waiting.send(());
        let _ = gone.lock().unwrap().recv();
    })
    .unwrap();
    store.write(extent(2), 0, &pattern(2, 0, BLOCK)).unwrap();
    store.close_segment().unwrap();
    store.commit(None).unwrap();
    waits.recv().unwrap();
    (go, guard)
}

/// fxdax: a commit made in a segment as it closes, its framing not yet written, is durable once a
/// flush has synced the segment's raw file and appended the commit's record to the segment's log,
/// which it syncs: no flush waits for a framing (TASK-221), and an opening after a crash closes the
/// segment again from its raw file and log (TASK-165.9).
#[test]
fn commits_in_segments_being_framed_are_durable_in_their_raw_files_and_logs() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 64 << 20);
    let (go, guard) = framing_held(&store);
    store.flush().unwrap();
    for seed in 0..16 {
        let reopened = Store::open(dir.crash(seed), OPTIONS).unwrap();
        assert_eq!(reopened.epoch().unwrap(), 2, "seed {seed}");
        for sequence in 1..=2 {
            assert_eq!(
                read(&reopened, extent(sequence), 0, BLOCK),
                pattern(sequence as u8, 0, BLOCK),
                "seed {seed}, extent {sequence}"
            );
        }
    }
    go.send(()).unwrap();
    drop(guard);
}

/// fxdax: a segment's raw file and log stay until its framed file is synced, its commits durable:
/// an opening after a crash closes a segment whose framed file isn't whole again from them
/// (TASK-165.9); what goes once it is, the framed file holds, a crash after finding it whole.
#[test]
fn raw_files_and_logs_stay_until_their_framed_files_are_synced() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 64 << 20);
    let (go, guard) = framing_held(&store);
    store.flush().unwrap();
    let name = format!("{:016x}", 0);
    let held = |kind: &str| dir.list(&of(kind, BRANCH.id)).unwrap().contains(&name);
    assert!(held("open-segments") && held("logs"), "segment 0's raw file or log went, unframed");
    go.send(()).unwrap();
    drop(guard);
    settled(&store);
    assert!(!held("open-segments") && !held("logs"), "segment 0's raw file or log stayed, framed");
    for seed in 0..16 {
        let reopened = Store::open(dir.crash(seed), OPTIONS).unwrap();
        assert_eq!(reopened.epoch().unwrap(), 2, "seed {seed}");
        for sequence in 1..=2 {
            assert_eq!(
                read(&reopened, extent(sequence), 0, BLOCK),
                pattern(sequence as u8, 0, BLOCK),
                "seed {seed}, extent {sequence}"
            );
        }
    }
}

/// fxdax: the bytes the process's stores wrote of extents' own files, framed, since `before`
/// (`extent_store::written`): the tests of a process are its own (cargo nextest's).
fn own_written_since(before: extent_store::Written) -> usize {
    (extent_store::written().own - before.own) as usize
}

/// fxdax: the names in the branch's directory of `kind`.
fn names(dir: &MemDir, kind: &str) -> Vec<String> {
    dir.list(&of(kind, BRANCH.id)).unwrap()
}

/// fxdax: the names of the branch's files of `kind` in `dir`, `store`'s, but for spare logs
/// (TASK-301) and the next segment's raw file and log, placed ahead of it (`Store::placed`,
/// TASK-384), once the preparation of them ended.
fn segment_files(store: &Store<MemDir>, dir: &MemDir, kind: &str) -> Vec<String> {
    store.prepared().unwrap();
    let placed = store.placed().map(|number| format!("{number:016x}"));
    let ours = |name: &String| !name.starts_with("spare-") && placed.as_ref() != Some(name);
    names(dir, kind).into_iter().filter(ours).collect()
}

/// fxdax: a large extent none of whose parts compresses is framed in place: its raw file is its
/// own file then, its tail appended, renamed, its data written once, as its guest wrote it, not a
/// second time framed in a file of its own, as a sandbox host's daemon wrote it, some 2.3 times
/// what its guests wrote, its framings waiting seconds behind (TASK-310). What it wrote of own
/// files, its tail alone; through a crash after, it reads as written. Red with every large
/// extent framed in a copy: its data written again.
#[test]
fn incompressible_large_extents_are_framed_in_place() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 4 << 20);
    let data = noise(1, 3 * OPTIONS.large_extent as usize + BLOCK);
    store.write(extent(1), 0, &data).unwrap();
    let before = extent_store::written();
    store.commit(None).unwrap();
    let framed = extent_frame::framed(&data, &[]);
    assert_eq!(own_written_since(before), framed.len() - data.len());
    let name = format!("{:016x}", 1);
    assert_eq!(names(&dir, "open-extents"), Vec::<String>::new());
    assert_eq!(names(&dir, "extents"), vec![name]);
    assert!(read(&store, extent(1), 0, data.len()) == data, "extent 1 reads otherwise");
    drop(store);
    for seed in 0..16 {
        let reopened = Store::open(dir.crash(seed), OPTIONS).unwrap();
        assert_eq!(reopened.epoch().unwrap(), 1, "seed {seed}");
        assert!(read(&reopened, extent(1), 0, data.len()) == data, "seed {seed}");
    }
}

/// fxdax: a guest writing more than a segment of small extents between commits, as one writing a
/// large file before its fsync, after a commit: each segment it fills waits for the next commit,
/// which seals every extent where it was written, copying nothing and making no file of an
/// extent's own, and closes them, their footers naming their runs as the commit's, which the open
/// segment ends (TASK-92.41); none of their parts compressing, each is framed in place, its data
/// written once (TASK-310). Before, the commit copied out every extent of the segments set aside,
/// each into a file of its own past the open segment's room: store-bench's burst, 640 own files,
/// each its own create, frame, sync and upload. Then a crash keeps them all. Red with the extents
/// in segments filled copied out: 516 KB copied; with segments framed in copies: none in place.
#[test]
fn bursts_past_a_segment_between_commits_are_sealed_where_they_are() {
    use extent_frame::segment::tag;
    let dir = MemDir::new();
    // Segments full once they hold a segment's size and an extent's most, as small here.
    let options = Options { max_extent: OPTIONS.segment_size, ..OPTIONS };
    let description = Description { block_size: BLOCK as u32, capacity: 4 << 20, branch: BRANCH };
    let store = Store::create(dir.clone(), description, options).unwrap();
    store.write(extent(1), 0, &noise(1, BLOCK)).unwrap();
    store.commit(None).unwrap();
    let (extents, len) = (8 * OPTIONS.segment_size as usize / (2 * BLOCK), 2 * BLOCK);
    for sequence in 2..=extents as u64 {
        store.write(extent(sequence), 0, &noise(sequence, len)).unwrap();
    }
    let before = (extent_store::written(), extent_store::segments_framed_in_place());
    let (before, framed_before) = before;
    // The open segment closes with the commit too, which ends in it.
    store.close_segment().unwrap();
    store.commit(None).unwrap();
    settled(&store);
    let after = extent_store::written();
    let copied = (after.own + after.copied) - (before.own + before.copied);
    assert_eq!(copied, 0, "copied");
    assert_eq!((names(&dir, "extents"), names(&dir, "open-extents")), (vec![], vec![]));
    let closed = store.closed(usize::MAX).unwrap();
    let continued = |closed: &extent_store::Closed<MemFile>| {
        let contents = &closed.footer.contents;
        (contents.commits.clone(), contents.continued.as_ref().map(|c| c.commit))
    };
    let mut expected = vec![(vec![], Some(tag(0, 2))); closed.len()];
    expected[0].0 = vec![tag(0, 1)];
    expected[closed.len() - 1] = (vec![tag(0, 2)], None);
    assert_eq!(closed.iter().map(continued).collect::<Vec<_>>(), expected);
    assert_eq!(closed.len(), 4, "the three segments filled, and the open one");
    let in_place = extent_store::segments_framed_in_place() - framed_before;
    assert_eq!(in_place as usize, closed.len(), "framed in place");
    let framed = after.framed - before.framed;
    assert!(framed < (closed.len() * BLOCK) as u64, "framed {framed}");
    drop(store);
    for seed in 0..4 {
        let reopened = Store::open(dir.crash(seed), options).unwrap();
        assert_eq!(reopened.epoch().unwrap(), 2, "seed {seed}");
        for sequence in 2..=extents as u64 {
            let read = read(&reopened, extent(sequence), 0, len);
            assert!(read == noise(sequence, len), "seed {seed}, {sequence}");
        }
    }
}

/// fxdax: segments compressing their parts would save less than an eighth of are framed in place,
/// their parts stored raw, as a guest's filesystem fills them, its metadata compressing in a few
/// parts of a large file's data; those it would save more of are framed in copies, compressed
/// (TASK-310). Red with segments framed in place only if no part compresses: none was.
#[test]
fn segments_compressing_little_are_framed_in_place() {
    let options = Options { max_extent: OPTIONS.segment_size, ..OPTIONS };
    let description = Description { block_size: BLOCK as u32, capacity: 4 << 20, branch: BRANCH };
    for (every, in_place) in [(16, true), (1, false)] {
        let every = every as u64;
        let dir = MemDir::new();
        let store = Store::create(dir.clone(), description, options).unwrap();
        let (extents, len) = (4 * OPTIONS.segment_size as usize / (2 * BLOCK), 2 * BLOCK);
        // Every `every`th extent compresses, the first of each segment's.
        let data = |sequence: u64| match sequence % every == 1 % every {
            true => pattern(sequence as u8, 0, len),
            false => noise(sequence, len),
        };
        for sequence in 1..=extents as u64 {
            store.write(extent(sequence), 0, &data(sequence)).unwrap();
        }
        let framed = extent_store::segments_framed_in_place();
        store.close_segment().unwrap();
        store.commit(None).unwrap();
        settled(&store);
        let closed = store.closed(usize::MAX).unwrap().len() as u64;
        let framed = extent_store::segments_framed_in_place() - framed;
        assert_eq!(framed, if in_place { closed } else { 0 }, "one in {every} compressing");
        assert!(closed >= 2, "{closed} segments closed");
        drop(store);
        let reopened = Store::open(dir.crash(0), options).unwrap();
        for sequence in 1..=extents as u64 {
            let read = read(&reopened, extent(sequence), 0, len);
            assert!(read == data(sequence), "one in {every} compressing, {sequence}");
        }
    }
}

/// fxdax TASK-343: a segment holding noise, then extents' holes, as Hunt's hostile VM process's
/// did (3 MiB of its fills' noise, then 2.6 MB of zeros its sparse extents' commit sealed in one run
/// each), is framed in a copy, compressed, its holes taking nothing; not in place, raw, which kept
/// the holes, bytes no write of the tenant's was admitted for, on its disk past its limit (Hunt
/// run 42949975683). Red with in-place framing decided by every eighth MiB part's compression: the
/// first part alone, of noise. Its extents read as written, before and after a crash.
#[test]
fn segments_of_noise_then_holes_are_framed_compressed() {
    const MIB: usize = 1 << 20;
    let options = Options {
        max_extent: 8 * MIB as u64,
        segment_size: 8 * MIB as u64,
        large_extent: 8 * MIB as u64,
        ..OPTIONS
    };
    let description = Description { block_size: BLOCK as u32, capacity: 64 << 20, branch: BRANCH };
    let dir = MemDir::new();
    let store = Store::create(dir.clone(), description, options).unwrap();
    for sequence in 1..=3 {
        store.write(extent(sequence), 0, &noise(sequence, MIB)).unwrap();
    }
    // A block at the end of each of three extents of a MiB: a MiB's hole before each.
    for sequence in 4..=6 {
        store.write(extent(sequence), (MIB - BLOCK) as u64, &noise(sequence, BLOCK)).unwrap();
    }
    let framed = extent_store::segments_framed_in_place();
    store.close_segment().unwrap();
    store.commit(None).unwrap();
    settled(&store);
    assert!(!store.closed(usize::MAX).unwrap().is_empty(), "no segment closed");
    assert_eq!(extent_store::segments_framed_in_place() - framed, 0, "framed in place");
    let expect = |sequence: u64| match sequence {
        1..=3 => noise(sequence, MIB),
        _ => {
            let mut held = vec![0; MIB - BLOCK];
            held.extend(noise(sequence, BLOCK));
            held
        }
    };
    for sequence in 1..=6 {
        assert!(read(&store, extent(sequence), 0, MIB) == expect(sequence), "{sequence}");
    }
    drop(store);
    let reopened = Store::open(dir.crash(0), options).unwrap();
    for sequence in 1..=6 {
        assert!(read(&reopened, extent(sequence), 0, MIB) == expect(sequence), "{sequence}");
    }
}

/// fxdax: extents not one run in a segment filled since the last commit, written in pieces
/// interleaved with others', or across two segments, are copied by the commit into the open
/// segment, and past its room into the next, it full in turn, which the commit goes on with: none
/// into a file of its own (TASK-92.41); and all read as written, before and after a crash.
#[test]
fn copies_past_the_open_segments_room_go_on_in_the_next() {
    let dir = MemDir::new();
    let options = Options { max_extent: OPTIONS.segment_size, ..OPTIONS };
    let description = Description { block_size: BLOCK as u32, capacity: 4 << 20, branch: BRANCH };
    let store = Store::create(dir.clone(), description, options).unwrap();
    // Two extents at a time, a block of each in turn: each in pieces, other writes between.
    let (extents, blocks) = (8 * OPTIONS.segment_size as usize / (2 * BLOCK), 2);
    let data = |sequence: u64| noise(sequence, blocks * BLOCK);
    for pair in (1..=extents as u64).step_by(2) {
        for block in 0..blocks {
            for sequence in [pair, pair + 1] {
                let at = block * BLOCK;
                store.write(extent(sequence), at as u64, &data(sequence)[at..at + BLOCK]).unwrap();
            }
        }
    }
    let before = extent_store::written();
    store.commit(None).unwrap();
    settled(&store);
    let after = extent_store::written();
    assert_eq!(after.own - before.own, 0, "own files written");
    assert_eq!(after.copied - before.copied, (extents * blocks * BLOCK) as u64, "copied");
    assert_eq!(names(&dir, "extents"), Vec::<String>::new());
    for sequence in 1..=extents as u64 {
        assert!(read(&store, extent(sequence), 0, blocks * BLOCK) == data(sequence), "{sequence}");
    }
    drop(store);
    let reopened = Store::open(dir.crash(0), options).unwrap();
    for sequence in 1..=extents as u64 {
        let read = read(&reopened, extent(sequence), 0, blocks * BLOCK);
        assert!(read == data(sequence), "{sequence}");
    }
}

/// fxdax: a large extent that compresses is framed in a file of its own, as before, its raw file
/// gone once its commit is durable; and one whose parts compress past its first 16 MiB, which are
/// framed in place no more, is too, those copied there as they are ([`Store::seal_own`],
/// TASK-310): what each wrote of own files, its framed file's bytes.
#[test]
fn compressible_large_extents_are_framed_in_copies() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 64 << 20);
    let compressible = pattern(1, 0, 3 * OPTIONS.large_extent as usize + BLOCK);
    let mut mixed = noise(2, 16 << 20);
    mixed.extend(pattern(2, 0, 1 << 20));
    for (sequence, data) in [(1, &compressible), (2, &mixed)] {
        store.write(extent(sequence), 0, data).unwrap();
        let before = extent_store::written();
        store.commit(None).unwrap();
        let framed = extent_frame::framed(data, &[]);
        assert!(framed.len() < data.len() + framed.len() / 2, "extent {sequence} compresses");
        assert_eq!(own_written_since(before), framed.len(), "extent {sequence}");
    }
    assert_eq!(names(&dir, "open-extents"), Vec::<String>::new());
    assert_eq!(names(&dir, "extents"), vec![format!("{:016x}", 1), format!("{:016x}", 2)]);
    drop(store);
    let reopened = Store::open(dir.crash(0), OPTIONS).unwrap();
    for (sequence, data) in [(1, &compressible), (2, &mixed)] {
        assert!(read(&reopened, extent(sequence), 0, data.len()) == *data, "extent {sequence}");
    }
}

/// fxdax: a commit framing a large extent in place, its raw file renamed from one directory to
/// another (TASK-310), stopped after each of its operations, as by a crash: the branch opens, the
/// extent a commit before framed in place reading as written, and the commit's extent reading as
/// written exactly if the commit is durable.
#[test]
fn extents_framed_in_place_survive_crashes_framing_them() {
    let len = 3 * OPTIONS.large_extent as usize + BLOCK;
    let (first, second) = (noise(1, len), noise(2, len));
    for operations in 0.. {
        let dir = MemDir::serial();
        let store = create(dir.clone(), 4 << 20);
        store.write(extent(1), 0, &first).unwrap();
        store.commit(None).unwrap();
        store.write(extent(2), 0, &second).unwrap();
        dir.fail_after(Some(operations));
        let committed = store.commit(None);
        dir.fail_after(None);
        for seed in 0..4 {
            let reopened = Store::open(dir.crash(seed), OPTIONS);
            let reopened = reopened.unwrap_or_else(|error| {
                panic!("stopped after {operations} operations, seed {seed}: {error:#}")
            });
            let at = format!("stopped after {operations} operations, seed {seed}");
            assert!(read(&reopened, extent(1), 0, len) == first, "{at}: extent 1");
            let mut read = vec![0; len];
            let intact = reopened.read(extent(2), 0, &mut read).is_ok() && read == second;
            assert_eq!(reopened.epoch().unwrap() == 2, intact, "{at}: extent 2, durable or not");
        }
        drop(store);
        if committed.is_ok() {
            break;
        }
    }
}

/// fxdax: a commit closing segments filled since the commit before, sealing in them what is one
/// run there, each framed in place, none of its parts compressing, its raw file linked as its
/// framed file (TASK-310, TASK-92.41), stopped after each of its operations, as by a crash: the
/// branch opens, what the commit before made reading as written, and what the commit made reading
/// as written exactly if the commit is durable.
#[test]
fn segments_framed_in_place_survive_crashes_framing_them() {
    let options = Options { max_extent: OPTIONS.segment_size, ..OPTIONS };
    let (extents, len) = (3 * OPTIONS.segment_size as usize / (2 * BLOCK), 2 * BLOCK);
    let first = noise(1, BLOCK);
    for operations in 0.. {
        let dir = MemDir::serial();
        let description =
            Description { block_size: BLOCK as u32, capacity: 4 << 20, branch: BRANCH };
        let store = Store::create(dir.clone(), description, options).unwrap();
        store.write(extent(1), 0, &first).unwrap();
        store.commit(None).unwrap();
        for sequence in 2..=extents as u64 {
            store.write(extent(sequence), 0, &noise(sequence, len)).unwrap();
        }
        let framed = extent_store::segments_framed_in_place();
        dir.fail_after(Some(operations));
        store.close_segment().unwrap();
        let committed = store.commit(None);
        dir.fail_after(None);
        for seed in 0..4 {
            let reopened = Store::open(dir.crash(seed), options);
            let reopened = reopened.unwrap_or_else(|error| {
                panic!("stopped after {operations} operations, seed {seed}: {error:#}")
            });
            let at = format!("stopped after {operations} operations, seed {seed}");
            assert!(read(&reopened, extent(1), 0, BLOCK) == first, "{at}: extent 1");
            let durable = reopened.epoch().unwrap() == 2;
            for sequence in 2..=extents as u64 {
                let mut read = vec![0; len];
                let intact = reopened.read(extent(sequence), 0, &mut read).is_ok()
                    && read == noise(sequence, len);
                assert_eq!(durable, intact, "{at}: extent {sequence}, durable or not");
            }
        }
        drop(store);
        if committed.is_ok() {
            assert!(extent_store::segments_framed_in_place() > framed, "none framed in place");
            break;
        }
    }
}

/// fxdax: segments one commit closes, framed one after another, then made durable together, their
/// framed files synced, then their directory, once
/// (`segments::make_durable`, TASK-310), stopped after each of the commit's operations, as by a
/// crash, which may keep any of the framed files whole and any torn, a later one's whole and an
/// earlier one's not: the branch opens, what the commit before made reading as written, and what
/// the commit made reading as written exactly if it is durable: an opening taking the last's
/// footer, whole, the commit's record, though a crash lost the first's, which it closed again
/// without the commit, whose record its log lacked, read the commit with its runs there lost; and
/// one taking a framed file whose raw file's entry was never durable, a crash keeping its entry,
/// as unchecked, read it torn (`wal::recover`, `Segments::unsynced_entries`). Whole, the
/// commit made its segments durable together, at once.
#[test]
fn segments_made_durable_together_survive_crashes_making_them_durable() {
    let options = Options { max_extent: OPTIONS.segment_size, ..OPTIONS };
    let len = 2 * BLOCK;
    let per_segment = OPTIONS.segment_size as usize / len;
    // Six segments' worth, of bytes that compress: framed in copies of a few blocks each, which a
    // crash keeps whole, or not, as often (`MemDir::crash`), where a segment framed in place, a
    // segment's data unsynced, it never keeps whole (segments_framed_in_place_survive_...'s).
    let extents = 6 * per_segment as u64;
    let data = |sequence: u64| pattern(sequence as u8, 0, len);
    let first = noise(1, BLOCK);
    for operations in 0.. {
        let dir = MemDir::serial();
        let description =
            Description { block_size: BLOCK as u32, capacity: 4 << 20, branch: BRANCH };
        let store = Store::create(dir.clone(), description, options).unwrap();
        store.write(extent(1), 0, &first).unwrap();
        store.commit(None).unwrap();
        for sequence in 2..2 + extents {
            store.write(extent(sequence), 0, &data(sequence)).unwrap();
        }
        let before = extent_store::timings();
        dir.fail_after(Some(operations));
        store.close_segment().unwrap();
        let committed = store.commit(None);
        dir.fail_after(None);
        for seed in 0..16 {
            let reopened = Store::open(dir.crash(seed), options);
            let reopened = reopened.unwrap_or_else(|error| {
                panic!("stopped after {operations} operations, seed {seed}: {error:#}")
            });
            let at = format!("stopped after {operations} operations, seed {seed}");
            assert!(read(&reopened, extent(1), 0, BLOCK) == first, "{at}: extent 1");
            let durable = reopened.epoch().unwrap() == 2;
            for sequence in 2..2 + extents {
                let mut read = vec![0; len];
                let intact =
                    reopened.read(extent(sequence), 0, &mut read).is_ok() && read == data(sequence);
                assert_eq!(durable, intact, "{at}: extent {sequence}, durable or not");
            }
        }
        drop(store);
        if committed.is_ok() {
            let after = extent_store::timings();
            let batches = after.durable.count - before.durable.count;
            let segments = after.durable_segments - before.durable_segments;
            assert_eq!(batches, 1, "the commit's segments made durable apart");
            // Three filled, a segment and an extent each, then the open one.
            assert_eq!(segments, 4, "segments made durable");
            break;
        }
    }
}

/// fxdax: a segment's framing waits for no other's syncs: while the segments framed before it
/// are being made durable, held there, it is framed, its framed file written, and it is made
/// durable after, with those framed meanwhile; neither is durable, nor its raw file and log gone,
/// before it is (TASK-310). Each sync of a framing waited for those before it, on a disk VMs
/// share seconds each.
#[test]
fn framings_wait_for_no_other_framings_syncs() {
    use std::sync::Mutex;
    use std::sync::mpsc::channel;
    let dir = MemDir::new();
    let store = create(dir.clone(), 64 << 20);
    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    store.commit(None).unwrap();
    store.flush().unwrap();
    let before = extent_store::timings();
    // Each framing, as it makes its framed file; each making durable, held until told to go.
    let (framing, framings) = channel();
    let framing = Mutex::new(framing);
    let _framing = fail::FailGuard::with_callback("segments::framing", move || {
        let _ = framing.lock().unwrap().send(());
    })
    .unwrap();
    let (holding, held) = channel();
    let (go, gone) = channel::<()>();
    let (holding, gone) = (Mutex::new(holding), Mutex::new(gone));
    let _durable = fail::FailGuard::with_callback("segments::making_durable", move || {
        let _ = holding.lock().unwrap().send(());
        let _ = gone.lock().unwrap().recv();
    })
    .unwrap();
    store.write(extent(2), 0, &pattern(2, 0, BLOCK)).unwrap();
    store.close_segment().unwrap();
    store.commit(None).unwrap();
    framings.recv().unwrap();
    held.recv().unwrap();
    // Segment 0 being made durable, held: segment 1 closes, and is framed meanwhile.
    store.write(extent(3), 0, &pattern(3, 0, BLOCK)).unwrap();
    store.close_segment().unwrap();
    store.commit(None).unwrap();
    framings.recv().unwrap();
    assert_eq!(store.framing(), [0, 1], "framed, neither durable yet");
    let raw = |number: u64| {
        let name = format!("{number:016x}");
        let held = |kind: &str| dir.list(&of(kind, BRANCH.id)).unwrap().contains(&name);
        held("open-segments") && held("logs")
    };
    assert!(raw(0) && raw(1), "a raw file or log went before its framed file was durable");
    drop(go);
    settled(&store);
    assert!(store.framing().is_empty(), "framings still running, settled");
    assert!(!raw(0) && !raw(1), "a raw file or log stayed, its framed file durable");
    let after = extent_store::timings();
    assert_eq!(after.durable.count - before.durable.count, 2, "segment 1 waited for segment 0");
    assert_eq!(after.durable_segments - before.durable_segments, 2);
    for seed in 0..16 {
        let reopened = Store::open(dir.crash(seed), OPTIONS).unwrap();
        assert_eq!(reopened.epoch().unwrap(), 3, "seed {seed}");
        for sequence in 1..=3 {
            assert_eq!(
                read(&reopened, extent(sequence), 0, BLOCK),
                pattern(sequence as u8, 0, BLOCK),
                "seed {seed}, extent {sequence}"
            );
        }
    }
}

/// fxdax: making a segment's framed file durable syncs the directories of raw files and logs only
/// for its own entries, and only if no flush synced them since it opened: the commit's flush after
/// a segment opens syncs them, and says so. So a closing costs its commit's flush those two, and
/// the making durable the directory of framed files alone, made durable here before that flush,
/// the next segment's entries unsynced then, which its own don't need. Synced again for every
/// segment whose entries no making had synced, the one just opened always among them, both cost a
/// journal commit more each closing, behind which pgbench-dax's flushes waited seconds on a disk
/// VMs share (TASK-369): 5 a closing, not 3; red so with the flush's marking left out, or every
/// segment's synced.
#[test]
fn segments_made_durable_sync_no_entries_a_flush_synced() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 64 << 20);
    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    store.commit(None).unwrap();
    settled(&store);
    for sequence in 2..6 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
        store.commit(None).unwrap();
        settled(&store);
        let before = dir.io().synced_dirs;
        store.close_segment().unwrap();
        store.commit_ordered(None).unwrap();
        // Framed and made durable, then the commit's flush.
        store.wait_framed().unwrap();
        store.flush().unwrap();
        let synced = dir.io().synced_dirs - before;
        assert_eq!(synced, 3, "closing after extent {sequence}: directories synced");
    }
}

/// fxdax: a segment opened from its raw file and log placed ahead of it, their entries made durable
/// off the commits by the store's opener, costs the commits' flush no directory: before, each
/// segment opened had its commits' next flush sync the directories of raw files and logs, each a
/// commit of the host's journal, which a durable commit, closing a segment and opening the next
/// for its upload, waited for every time (TASK-384, `durable_commits_through_the_daemon`). Red so
/// with the entries left unsynced, or the segment opened unplaced.
#[test]
fn segments_opened_ahead_cost_their_commits_flushes_no_directory() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 64 << 20);
    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    store.commit(None).unwrap();
    settled(&store);
    for sequence in 2..6 {
        store.prepared().unwrap();
        let next = store.placed().expect("the next segment placed");
        store.close_segment().unwrap();
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
        store.commit_ordered(None).unwrap();
        // Framed and made durable, and the segment after placed, their syncs apart.
        store.wait_framed().unwrap();
        store.prepared().unwrap();
        assert_eq!(store.placed(), Some(next + 1), "the segment after the one opened, placed");
        let before = dir.io().synced_dirs;
        store.flush().unwrap();
        let synced = dir.io().synced_dirs - before;
        assert_eq!(synced, 0, "segment {next}'s commits' flush: directories synced");
    }
    for sequence in 1..6 {
        assert_eq!(read(&store, extent(sequence), 0, BLOCK), pattern(sequence as u8, 0, BLOCK));
    }
}

/// fxdax: a store prepares the next segment's log once a flush made commits durable, a store that
/// writes, not as it opens (TASK-333): one that only reads, as a fork a daemon serves, prepares
/// none, nor holds a file open for it, its flushes making nothing durable. Every fork the daemon
/// served took one, and a file held open (`serves_hundreds_of_forks_of_a_base`).
#[test]
fn stores_that_only_read_prepare_no_log() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 64 << 20);
    let logs = || dir.list(&of("logs", BRANCH.id)).unwrap().len();
    // Its open segment's log alone, flushed, nothing durable.
    store.flush().unwrap();
    store.prepared().unwrap();
    assert_eq!(logs(), 1, "the next segment's log prepared, nothing durable");
    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    store.commit(None).unwrap();
    store.prepared().unwrap();
    assert_eq!(logs(), 2, "the next segment's log not prepared, a commit durable");
}

/// fxdax: the uploader takes a segment once its framing wrote its framed file, not once it made it
/// durable, which its raw file and log stand for meanwhile, until the commits its footer names are
/// (TASK-384): a durable commit waited, one after another, for the framed file's sync and its
/// directory's, each a commit of the host's journal, before its upload began. A crash anywhere
/// before the framed file is durable keeps every commit: as the store's write-ahead log holds them,
/// if the bucket has not acknowledged the segment, or as the bucket does, once it has, the store
/// not told so yet. The store hears of the acknowledgement once the framed file is durable, which
/// it waits for, and lets the segment's files go then, as before; and a crash after keeps them too.
#[test]
fn segments_upload_before_their_framed_files_are_durable() {
    use std::sync::Mutex;
    use std::sync::mpsc::channel;
    let dir = MemDir::new();
    let store = created_on(dir.clone(), 1 << 20, extent_remote::State::new(BRANCH), WRITER);
    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    store.write(extent(2), 0, &pattern(2, 0, BLOCK)).unwrap();
    store.commit(None).unwrap();
    // Its making durable held until told to go.
    let (holding, held) = channel();
    let (go, gone) = channel::<()>();
    let (holding, gone) = (Mutex::new(holding), Mutex::new(gone));
    let durable = fail::FailGuard::with_callback("segments::making_durable", move || {
        let _ = holding.lock().unwrap().send(());
        let _ = gone.lock().unwrap().recv();
    })
    .unwrap();
    assert!(store.close_for_upload(false).unwrap());
    held.recv().unwrap();
    store.flush().unwrap();
    let closed = store.closed(usize::MAX).unwrap();
    let [segment] = &closed[..] else { panic!("{} segments closed", closed.len()) };
    assert_eq!(store.framing(), [0], "taken for the upload, its framed file not durable");
    let framed = format!("{:016x}", segment.number);
    assert!(!names(&dir, "segments").contains(&framed), "its framed file named its own");
    let mut landed = extent_remote::State::new(BRANCH);
    assert!(landed.apply(&segment.footer));
    let chain = WRITER.admit(&segment.footer).unwrap();
    // What a store opened on what the bucket holds, crashed at `seed`, holds: both extents, here,
    // or in the bucket, once it landed there.
    let check = |seed: u64, bucket: &extent_remote::State, chain: Chain| {
        let reopened = opened_on(dir.crash(seed), bucket.clone(), chain);
        for sequence in 1..=2 {
            match reopened.place(extent(sequence)).unwrap() {
                Some(Place::Remote(_)) => assert!(bucket.last_commit.is_some(), "seed {seed}"),
                Some(_) => assert_eq!(
                    read(&reopened, extent(sequence), 0, BLOCK),
                    pattern(sequence as u8, 0, BLOCK),
                    "seed {seed}, extent {sequence}"
                ),
                None => panic!("seed {seed}: extent {sequence} lost"),
            }
        }
    };
    for seed in 0..16 {
        // Uploaded, not landed.
        check(seed, &extent_remote::State::new(BRANCH), WRITER);
        // Landed, its acknowledgement not heard.
        check(seed, &landed, chain);
    }
    drop(go);
    drop(durable);
    store.acknowledged(segment.number).unwrap();
    assert!(store.framing().is_empty(), "its framing ended, durable, before it was heard");
    assert!(!names(&dir, "segments").contains(&framed), "acknowledged, it left");
    for seed in 0..16 {
        check(seed, &landed, chain);
    }
}

/// The files the process has open.
fn open_fds() -> usize {
    std::fs::read_dir("/dev/fd").unwrap().count()
}

/// A store keeps no more segments' files open than its options say, however many segments its
/// commits close: closing one each, as a store uploading to object storage does on every sync,
/// once ran a daemon out of file descriptors.
#[test]
fn closed_segments_keep_no_more_files_open_than_allowed() {
    let temp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let store = create(HostDir::new(temp.path(), true).unwrap(), 64 << 20);
    let mut open = Vec::new();
    for sequence in 1..=40 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
        store.close_segment().unwrap();
        store.commit(None).unwrap();
        // fxdax: framed, which no commit waits for: a segment being framed holds its raw file and
        // log open till then (TASK-165.9); and the next segment's placed ahead of it (TASK-384).
        settled(&store);
        store.prepared().unwrap();
        open.push(open_fds());
    }
    assert_eq!(open[39], open[9], "{open:?}");
    assert_eq!(read(&store, extent(1), 0, BLOCK), pattern(1, 0, BLOCK));
    assert_eq!(read(&store, extent(40), 0, BLOCK), pattern(40, 0, BLOCK));
}

/// What a store's files did, by the directory each is in: their syncs that let their pages go
/// from the host's page cache, of those of extents' data, as a host's do, written through it
/// (`HostFile::sync`); the bytes read; and of those, the bytes read of extents' data, which a
/// host's read bypassing its page cache, and the bytes read through that cache
/// (`File::read_cached`).
#[derive(Default)]
struct Counts {
    drops: BTreeMap<String, u64>,
    read: BTreeMap<String, u64>,
    direct: BTreeMap<String, u64>,
    cached: BTreeMap<String, u64>,
}

/// fxdax: a store's directory in memory whose files count what they do ([`Counts`]).
#[derive(Clone)]
struct Counting {
    dir: MemDir,
    counts: Arc<std::sync::Mutex<Counts>>,
}

struct CountedFile {
    file: MemFile,
    /// The directory it is in, and whether it holds extents' data.
    kind: String,
    data: bool,
    counts: Arc<std::sync::Mutex<Counts>>,
}

impl Counting {
    fn new() -> Self {
        Self { dir: MemDir::new(), counts: Default::default() }
    }

    fn file(&self, file: MemFile, path: &str, data: bool) -> CountedFile {
        let kind = path.split('/').next().unwrap().to_owned();
        CountedFile { file, kind, data, counts: self.counts.clone() }
    }

    /// The syncs that let files' pages go, so far.
    fn drops(&self) -> BTreeMap<String, u64> {
        self.counts.lock().unwrap().drops.clone()
    }

    /// The bytes read so far.
    fn read(&self) -> BTreeMap<String, u64> {
        self.counts.lock().unwrap().read.clone()
    }

    /// The bytes read so far of files of extents' data, which a host's read bypassing its page
    /// cache.
    fn direct(&self) -> BTreeMap<String, u64> {
        self.counts.lock().unwrap().direct.clone()
    }

    /// The bytes read so far through the page cache ([`extent_store::File::read_cached`]).
    fn cached(&self) -> BTreeMap<String, u64> {
        self.counts.lock().unwrap().cached.clone()
    }
}

/// Of `counts`, by directory, those of raw files' directories: open segments' and open extents'.
fn of_raw_files(counts: BTreeMap<String, u64>) -> BTreeMap<String, u64> {
    counts.into_iter().filter(|(kind, _)| kind.starts_with("open-")).collect()
}

impl extent_store::File for CountedFile {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
        let mut counts = self.counts.lock().unwrap();
        *counts.read.entry(self.kind.clone()).or_default() += buf.len() as u64;
        if self.data {
            *counts.direct.entry(self.kind.clone()).or_default() += buf.len() as u64;
        }
        drop(counts);
        self.file.read_at(buf, offset)
    }

    fn read_cached(&self, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
        let mut counts = self.counts.lock().unwrap();
        *counts.read.entry(self.kind.clone()).or_default() += buf.len() as u64;
        *counts.cached.entry(self.kind.clone()).or_default() += buf.len() as u64;
        drop(counts);
        self.file.read_cached(buf, offset)
    }

    fn write_at(&self, buf: &[u8], offset: u64) -> std::io::Result<()> {
        self.file.write_at(buf, offset)
    }

    fn size(&self) -> std::io::Result<u64> {
        self.file.size()
    }

    fn sync(&self) -> std::io::Result<()> {
        if self.data {
            *self.counts.lock().unwrap().drops.entry(self.kind.clone()).or_default() += 1;
        }
        self.file.sync()
    }

    fn sync_cached(&self) -> std::io::Result<()> {
        self.file.sync_cached()
    }
}

impl Dir for Counting {
    type File = CountedFile;

    fn create(&self, path: &str, data: bool) -> std::io::Result<CountedFile> {
        Ok(self.file(self.dir.create(path, data)?, path, data))
    }

    fn open(&self, path: &str, data: bool) -> std::io::Result<CountedFile> {
        Ok(self.file(self.dir.open(path, data)?, path, data))
    }

    fn remove(&self, path: &str) -> std::io::Result<()> {
        self.dir.remove(path)
    }

    fn rename(&self, from: &str, to: &str) -> std::io::Result<()> {
        self.dir.rename(from, to)
    }

    fn link(&self, from: &str, to: &str) -> std::io::Result<()> {
        self.dir.link(from, to)
    }

    fn create_dir(&self, path: &str) -> std::io::Result<()> {
        self.dir.create_dir(path)
    }

    fn list(&self, path: &str) -> std::io::Result<Vec<String>> {
        self.dir.list(path)
    }

    fn sync_dir(&self, path: &str) -> std::io::Result<()> {
        self.dir.sync_dir(path)
    }

    fn barrier(&self) -> std::io::Result<()> {
        self.dir.barrier()
    }
}

/// fxdax: durable commits let none of the open segment's pages go from the host's page cache: its
/// raw file, written and synced again and again, takes them with it once framed and removed; its
/// framed file, written once, lets them go once synced (TASK-92.23).
#[test]
fn durable_commits_keep_the_open_segments_pages_cached() {
    let dir = Counting::new();
    let store = create(dir.clone(), 1 << 20);
    for sequence in 1..=8 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
        store.commit(None).unwrap();
    }
    store.close_segment().unwrap();
    store.commit(None).unwrap();
    settled(&store);
    assert_eq!(dir.drops(), BTreeMap::from([("segments".to_owned(), 1)]));
}

/// fxdax: a store reads its raw files, the open segment's, those of segments being framed or set
/// aside, and open extents' own, through the host's page cache, which holds what was just written
/// to them: a read bypassing it writes the dirty pages it reads back first, waiting for the host's
/// disk. Under pgbench's load, a commit copying the extents written in pieces waited about 4 ms a
/// MiB for it, and up to 400 ms, under the store's state, holding every write back; and a closed
/// segment's framing took a median 100-190 ms to read it (TASK-91.17, TASK-79.93). Here a commit
/// copies two extents written in pieces, one after the other's, frames the segment it closes, and
/// frames an extent written in a file of its own; then reopened, the branch frames the open
/// segment left raw. Each reads its raw file through the cache.
#[test]
fn raw_files_are_read_through_the_page_cache() {
    let dir = Counting::new();
    let store = create(dir.clone(), 1 << 20);
    for half in 0..2 {
        for sequence in 1..=2 {
            let data = pattern(sequence as u8, half * BLOCK, BLOCK);
            store.write(extent(sequence), (half * BLOCK) as u64, &data).unwrap();
        }
    }
    let large = OPTIONS.large_extent as usize;
    store.write(extent(3), 0, &pattern(3, 0, large)).unwrap();
    store.close_segment().unwrap();
    store.commit(None).unwrap();
    // Framed, on a thread of its own, which no commit waits for (TASK-221).
    settled(&store);
    assert_eq!(of_raw_files(dir.direct()), BTreeMap::new());
    // The copies of the two extents, 4 blocks, and the segment framed, 8; the large extent framed.
    // The flush reads back none: the commit stored an extent on its own, whose data it syncs
    // before the records, which hold no runs then (TASK-301).
    let cached = |segments: usize| {
        let kinds = [("open-extents", large), ("open-segments", segments)];
        BTreeMap::from(kinds.map(|(kind, bytes)| (kind.to_owned(), bytes as u64)))
    };
    assert_eq!(of_raw_files(dir.cached()), cached(12 * BLOCK));
    store.write(extent(4), 0, &pattern(4, 0, BLOCK)).unwrap();
    store.commit(None).unwrap();
    drop(store);
    let store = Store::open(dir.clone(), OPTIONS).unwrap();
    assert_eq!(of_raw_files(dir.direct()), BTreeMap::new());
    // And extent 4's block, read back by its flush, whose record holds it (TASK-301), and the
    // open segment left raw, read once by the opening, which closes it again.
    assert_eq!(of_raw_files(dir.cached()), cached(14 * BLOCK));
    for (sequence, len) in [(1, 2 * BLOCK), (2, 2 * BLOCK), (3, large), (4, BLOCK)] {
        assert!(read(&store, extent(sequence), 0, len) == pattern(sequence as u8, 0, len));
    }
}

/// fxdax: every read of a raw file's data goes through the host's page cache, whoever asks, as the
/// store's copies and framings do (`raw_files_are_read_through_the_page_cache`): the guest's
/// (`Store::read`), the uploader's (`Store::read_kept`) and a follower's (`Store::read_view`) of an
/// extent committed in the open segment, and the guest's of an extent open in a file of its own.
/// What the store wrote lately is what they read most (TASK-91.17).
#[test]
fn reads_of_raw_files_go_through_the_page_cache() {
    let dir = Counting::new();
    let store = create(dir.clone(), 1 << 20);
    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    store.commit(None).unwrap();
    let large = OPTIONS.large_extent as usize;
    store.write(extent(2), 0, &pattern(2, 0, large)).unwrap();
    let view = store.view();
    // Extent 1's block, read back by the commit's flush, which checksums it (TASK-301).
    let flushed = BTreeMap::from([("open-segments".to_owned(), BLOCK as u64)]);
    assert_eq!(of_raw_files(dir.cached()), flushed);
    assert!(read(&store, extent(1), 0, BLOCK) == pattern(1, 0, BLOCK));
    let mut viewed = vec![0; BLOCK];
    store.read_view(&view, extent(1), 0, &mut viewed).unwrap();
    assert!(viewed == pattern(1, 0, BLOCK));
    assert!(read(&store, extent(2), 0, large) == pattern(2, 0, large));
    assert_eq!(of_raw_files(dir.direct()), BTreeMap::new());
    let kinds = [("open-extents", large), ("open-segments", 3 * BLOCK)];
    let cached = BTreeMap::from(kinds.map(|(kind, bytes)| (kind.to_owned(), bytes as u64)));
    assert_eq!(of_raw_files(dir.cached()), cached);
}

/// fxdax: a branch opening again reads of the segments of its write-ahead log their tails alone:
/// each's trailer and footer, which the replay of its footers needs, and its trailer and table,
/// which say how much data it holds, three blocks; none of their data, their raw files gone, their
/// commits durable (TASK-92.16, TASK-165.9).
#[test]
fn branches_open_reading_the_tails_of_their_logs_segments() {
    let dir = Counting::new();
    let store = create(dir.clone(), 1 << 20);
    for sequence in 0..16 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
        store.close_segment().unwrap();
        store.commit(None).unwrap();
    }
    drop(store);
    // None yet: commits closed them, from their raw files.
    let segments_read = || dir.read().get("segments").copied().unwrap_or(0);
    assert_eq!(segments_read(), 0);
    let store = Store::open(dir.clone(), OPTIONS).unwrap();
    assert_eq!(segments_read(), 16 * 3 * BLOCK as u64);
    assert_eq!(store.extents().unwrap().len(), 16);
}

/// The host's page size.
#[cfg(target_os = "linux")]
fn page_size() -> usize {
    // SAFETY: Reads a configuration value.
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

/// The pages of the file at `path` that the host's page cache holds.
#[cfg(target_os = "linux")]
fn cached_pages(path: &std::path::Path) -> usize {
    use std::os::fd::AsRawFd as _;
    let file = std::fs::File::open(path).unwrap();
    let len = file.metadata().unwrap().len() as usize;
    let (prot, flags) = (libc::PROT_READ, libc::MAP_SHARED);
    // SAFETY: Maps the file, unread, to ask the kernel which of its pages it caches, into a byte
    // for each, and unmaps it.
    unsafe {
        let addr = libc::mmap(std::ptr::null_mut(), len, prot, flags, file.as_raw_fd(), 0);
        assert_ne!(addr, libc::MAP_FAILED, "{}", std::io::Error::last_os_error());
        let mut cached = vec![0u8; len.div_ceil(page_size())];
        assert_eq!(libc::mincore(addr, len, cached.as_mut_ptr()), 0);
        libc::munmap(addr, len);
        cached.iter().filter(|page| **page & 1 == 1).count()
    }
}

/// fxdax: on Linux, durable commits leave the pages of the open segment's raw file, written through
/// the host's page cache, in it; its framed file, once synced, lets them go (TASK-92.23).
///
/// Synced, the raw file's pages are clean, which the kernel's reclaim may take under memory
/// pressure whenever it likes: a loaded host's took one of 8 in 2,000 runs (TASK-228). What this
/// guards against, a sync letting the file go (`POSIX_FADV_DONTNEED`, dir.rs), takes all of them,
/// so most staying cached is what tells the two apart.
#[cfg(target_os = "linux")]
#[test]
fn durable_commits_keep_the_open_segments_pages_cached_on_the_host() {
    let temp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let store = create(HostDir::new(temp.path(), true).unwrap(), 64 << 20);
    assert!(store.dir().is_direct(), "the host's file system bypasses no page cache");
    for sequence in 1..=8 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
        store.commit(None).unwrap();
    }
    let path = |kind| temp.path().join(of(kind, BRANCH.id)).join(format!("{:016x}", 0));
    let (written, cached) =
        ((8 * BLOCK).div_ceil(page_size()), cached_pages(&path("open-segments")));
    assert!(cached > written / 2, "{cached} of the open segment's {written} pages cached");
    store.close_segment().unwrap();
    store.commit(None).unwrap();
    // Framed, on a thread of its own, which no commit waits for, nothing written since the commit
    // before (TASK-165.9): its framing's sync lets the pages go.
    settled(&store);
    assert_eq!(cached_pages(&path("segments")), 0);
}

/// Segments of 64 KiB and extents of 64 KiB at most, so that a segment holds 128 KiB at most.
const SMALL_SEGMENTS: Options = Options { max_extent: 64 << 10, ..OPTIONS };

/// The most bytes of data a segment of [`SMALL_SEGMENTS`] holds: a segment's, and an extent's.
const SEGMENT_LIMIT: u64 = 128 << 10;

/// fxdax: the bytes of `store`'s segments' data no extent is in, no write being in progress, and
/// of the frees it keeps for the bucket alone, until acknowledged there (TASK-165.9).
fn dead_bytes(store: &Store<MemDir>) -> u64 {
    let usage = store.usage().unwrap();
    usage.segment_bytes - usage.live_bytes + usage.kept_bytes - usage.kept_for_views
}

/// The bytes of data of each of the branch's segments in `dir`, `store`'s: open ones' raw files,
/// closed ones' framed data. Not the framed file of a segment being framed behind what closed it,
/// which may not be written yet, and whose raw file holds its data until its framing ends
/// (TASK-189): named its own until it is durable (`framing-`, TASK-310).
fn segment_data(dir: &MemDir) -> Vec<u64> {
    let mut lens = Vec::new();
    for kind in ["open-segments", "segments"] {
        for name in dir.list(&of(kind, BRANCH.id)).unwrap() {
            if name.starts_with("framing-") {
                continue;
            }
            let file = dir.open(&format!("{}/{name}", of(kind, BRANCH.id)), true).unwrap();
            let mut bytes = vec![0; file.size().unwrap() as usize];
            file.read_at(&mut bytes, 0).unwrap();
            let data = if kind == "segments" { extent_frame::data(&bytes).unwrap() } else { bytes };
            lens.push(data.len() as u64);
        }
    }
    lens
}

/// TASK-189: reading the branch's segments beside the store between writes, as [`segment_data`]
/// does, reads no framed file a framing has created and not yet written: a commit closed the
/// segments writes filled, and framed them behind, on a thread of its own (TASK-92.41).
/// Each framing waits at the failpoint between its file's creation and its write while the helper
/// reads: read so, unskipped, the file was empty, `Corrupt("an object's trailer")`, as in CI run
/// 42105764695. The failpoint is the process's, as nextest gives each test.
#[test]
fn segments_read_beside_the_store_skip_framed_files_being_written() {
    use std::sync::Mutex;
    use std::sync::mpsc::channel;
    let dir = MemDir::new();
    let description = Description { block_size: BLOCK as u32, capacity: 64 << 20, branch: BRANCH };
    let store = Store::create(dir.clone(), description, SMALL_SEGMENTS).unwrap();
    let block = |sequence: u64| pattern(sequence as u8, 0, BLOCK);
    store.write(extent(96), 0, &block(96)).unwrap();
    store.commit(None).unwrap();
    // Each framing says it waits, its file created, and waits until let go: or until the test
    // stops, failing, its channels dropped, so that the store's drop, waiting for its framings,
    // ends.
    let (waiting, waits) = channel();
    let (go, gone) = channel::<()>();
    let gone = Mutex::new(gone);
    let guard = fail::FailGuard::with_callback("segments::framing", move || {
        let _ = waiting.send(());
        let _ = gone.lock().unwrap().recv();
    })
    .unwrap();
    let (mut seen, mut reads) = (BTreeSet::new(), 0);
    for sequence in 0..48 {
        store.write(extent(sequence), 0, &block(sequence)).unwrap();
    }
    // fxdax: the segments filled, framed behind the commit closing them (TASK-92.41).
    store.commit_ordered(None).unwrap();
    loop {
        let begun: Vec<u64> = (store.framing().into_iter()).filter(|n| seen.insert(*n)).collect();
        if begun.is_empty() {
            break;
        }
        for _ in &begun {
            waits.recv().unwrap();
            let lens = segment_data(&dir);
            assert!(lens.iter().all(|len| *len <= SEGMENT_LIMIT), "segments of {lens:?}");
            reads += 1;
            go.send(()).unwrap();
        }
    }
    drop(guard);
    assert!(reads > 1, "no segments framed behind a commit");
    store.commit(None).unwrap();
    for sequence in 0..48 {
        assert!(read(&store, extent(sequence), 0, BLOCK) == block(sequence), "{sequence}");
    }
}

/// TASK-150.2: a segment writes filled, holding a commit's data, goes to the uploader with no
/// commit after it: the uploader's closing between commits closes it, and frames it, as nothing
/// else would until the guest next commits, which a sync of the store, or a fork, waiting for that
/// commit's upload, may not live to see (TASK-92.41). It reads as before, and the next commit
/// copies out of it what was written since the commit before.
#[test]
fn segments_filled_go_to_the_uploader_with_no_commit_after() {
    let dir = MemDir::new();
    let description = Description { block_size: BLOCK as u32, capacity: 64 << 20, branch: BRANCH };
    let store = Store::create(dir, description, SMALL_SEGMENTS).unwrap();
    let block = |sequence: u64| pattern(sequence as u8, 0, BLOCK);
    store.write(extent(0), 0, &block(0)).unwrap();
    store.commit(None).unwrap();
    // Past what a segment holds, no commit with them.
    for sequence in 1..48 {
        store.write(extent(sequence), 0, &block(sequence)).unwrap();
    }

    store.flush().unwrap();
    assert_eq!(store.closed(8).unwrap().len(), 0, "a segment closed before the uploader's");
    assert!(store.close_now().unwrap(), "the segment filled, holding the commit, not closed");
    let lasts: Vec<u64> = store.closed(8).unwrap().iter().map(Closed::last).collect();
    assert_eq!(lasts, [1]);
    for sequence in 0..48 {
        assert!(read(&store, extent(sequence), 0, BLOCK) == block(sequence), "{sequence}");
    }
    store.commit(None).unwrap();
    for sequence in 0..48 {
        assert!(read(&store, extent(sequence), 0, BLOCK) == block(sequence), "{sequence}");
    }
}

/// fxdax: an uploader's interval leaves a segment the guest is filling for its next commit, which
/// seals in place what it wrote there (TASK-92.41): one filled since the last commit, or the open
/// one past 8 MiB written since; it closes one written little since, as a journal's commits leave
/// it, and a commit waiting for its upload closes it whatever it holds.
#[test]
fn interval_closings_leave_segments_being_filled() {
    let dir = MemDir::new();
    let description = Description { block_size: BLOCK as u32, capacity: 64 << 20, branch: BRANCH };
    let store = Store::create(dir, description, SMALL_SEGMENTS).unwrap();
    let block = |sequence: u64| pattern(sequence as u8, 0, BLOCK);
    store.write(extent(0), 0, &block(0)).unwrap();
    store.commit(None).unwrap();
    store.write(extent(1), 0, &block(1)).unwrap();
    assert!(store.close_now_unless_filling().unwrap(), "a segment written little since, open");
    store.write(extent(2), 0, &block(2)).unwrap();
    store.commit(None).unwrap();
    // Past what a segment holds: it fills.
    for sequence in 3..48 {
        store.write(extent(sequence), 0, &block(sequence)).unwrap();
    }
    assert!(!store.close_now_unless_filling().unwrap(), "a segment filled, closed");
    assert!(store.close_now().unwrap(), "a segment filled, holding a commit, left open");
    // The open one, past 8 MiB written since the last commit.
    let options = Options { max_extent: 16 << 20, large_extent: 16 << 20, ..OPTIONS };
    let store = Store::create(MemDir::new(), description, options).unwrap();
    store.write(extent(0), 0, &block(0)).unwrap();
    store.commit(None).unwrap();
    store.write(extent(1), 0, &vec![1; 9 << 20]).unwrap();
    assert!(!store.close_now_unless_filling().unwrap(), "a segment being filled, closed");
    assert!(store.close_now().unwrap(), "a segment being filled, holding a commit, left open");
}

/// fxdax: a store whose segment ending commit 1 and going on with commit 2 landed alone, its
/// writer then displaced and the branch taken again, live (`Store::rebase`): its log follows the
/// branch, whose last commit is 1, and keeps that segment, stamped again after the fence with its
/// runs of commit 2 alone, then the rest, stamped again too. Commit 1's extent, landed, is read
/// where it landed from then on, acknowledged or not; commit 2's in that segment, here, then where
/// it lands again (TASK-345). Before, the rebase failed `Diverged`, though no other writer moved
/// the branch on, and `fxdaxctl take` dropped commit 2, durable. Red with commit 1's extent left
/// to be read where the segment lands again, an object whose data the branch holds no more of but
/// commit 2's, which the collector may compact away.
#[test]
fn logs_whose_segment_ending_a_commit_and_going_on_landed_alone_go_on() {
    use extent_frame::segment::tag;
    let dir = MemDir::new();
    let options = Options { max_extent: OPTIONS.segment_size, ..OPTIONS };
    let description = Description { block_size: BLOCK as u32, capacity: 4 << 20, branch: BRANCH };
    let store = Store::create(dir.clone(), description, options).unwrap();
    store.write(extent(1), 0, &noise(1, BLOCK)).unwrap();
    store.commit(None).unwrap();
    let (extents, len) = (4 * OPTIONS.segment_size as usize / (2 * BLOCK), 2 * BLOCK);
    for sequence in 2..=extents as u64 {
        store.write(extent(sequence), 0, &noise(sequence, len)).unwrap();
    }
    store.close_segment().unwrap();
    store.commit(None).unwrap();
    settled(&store);
    let closed = store.closed(usize::MAX).unwrap();
    let first = closed[0].footer.clone();
    let continued = first.contents.continued.clone().expect("the first goes on with commit 2");
    assert_eq!((first.contents.commits.clone(), continued.commit), (vec![tag(0, 1)], tag(0, 2)));
    // It landed, alone; a writer of lease epoch 1 fenced the branch after it.
    let fence = first.seq + 1;
    let chain = Chain::new(BRANCH, 1, fence + 1, Some((fence, 1)), first.contents.next_sequence);
    let last = Some(tag(0, 1));
    store.rebase(&Fenced { recovered: last, last_commit: last, chain }).unwrap();
    let rebased = store.closed(usize::MAX).unwrap();
    let numbers = |closed: &[Closed<MemFile>]| -> Vec<u64> {
        closed.iter().map(|closed| closed.number).collect()
    };
    assert_eq!(numbers(&rebased), numbers(&closed));
    let again = &rebased[0];
    let place = (again.footer.seq, again.footer.epoch, again.footer.after, again.restamped);
    assert_eq!(place, (fence + 1, 1, Some((fence, 1)), true));
    let next_sequence = again.footer.contents.next_sequence;
    let continuation =
        Contents { continued: Some(continued.clone()), next_sequence, ..Contents::default() };
    assert_eq!(again.footer.contents, continuation);
    let seqs: Vec<u64> = rebased.iter().map(|closed| closed.footer.seq).collect();
    assert_eq!(seqs, (fence + 1..fence + 1 + rebased.len() as u64).collect::<Vec<_>>());
    // Commit 1's extent, where it landed; commit 2's, here.
    let landed = |run: &(ExtentId, u64, u64)| {
        let key = extent_remote::layout::segment(BRANCH, first.seq);
        Some(Place::Remote(Location { key, offset: run.1, len: run.2 }))
    };
    let run = first.contents.extents.iter().find(|run| run.0 == extent(1)).unwrap();
    assert_eq!(store.place(extent(1)).unwrap(), landed(run));
    for sequence in 2..=extents as u64 {
        assert!(read(&store, extent(sequence), 0, len) == noise(sequence, len), "{sequence}");
    }
    // Acknowledged, landed again: commit 2's runs in it are read where it landed again.
    for closed in &rebased {
        store.acknowledged(closed.number).unwrap();
    }
    assert_eq!(store.place(extent(1)).unwrap(), landed(run));
    let ran = &continued.extents[0];
    let key = extent_remote::layout::segment(BRANCH, fence + 1);
    let there = Some(Place::Remote(Location { key, offset: ran.1, len: ran.2 }));
    assert_eq!(store.place(ran.0).unwrap(), there);
}

/// The open segment takes writes until it holds a segment and an extent, `Options::segment_size`
/// and `Options::max_extent`, between commits: then the next goes to another, which no segment
/// grows past, however much is written between commits (TASK-79.2). Full, holding commits' data or
/// not, it waits for the next commit, which seals the extents in it where they are, none in a file
/// of its own, and closes it, the commit going on past it (TASK-92.41). Every extent reads as
/// written meanwhile, and once committed, and so after opening the store again.
#[test]
fn segments_hold_a_segment_and_an_extent_at_most() {
    let dir = MemDir::new();
    let description = Description { block_size: BLOCK as u32, capacity: 64 << 20, branch: BRANCH };
    let store = Store::create(dir.clone(), description, SMALL_SEGMENTS).unwrap();
    let block = |sequence: u64| pattern(sequence as u8, 0, BLOCK);
    let check = |store: &Store<MemDir>, sequences: std::ops::Range<u64>| {
        for sequence in sequences {
            assert!(read(store, extent(sequence), 0, BLOCK) == block(sequence), "{sequence}");
        }
    };
    // 192 KiB of blocks, no commit's data with them.
    for sequence in 0..48 {
        store.write(extent(sequence), 0, &block(sequence)).unwrap();
        let lens = segment_data(&dir);
        assert!(lens.iter().all(|len| *len <= SEGMENT_LIMIT), "segments of {lens:?}");
    }
    check(&store, 0..48);
    store.commit(None).unwrap();
    check(&store, 0..48);
    let lens = segment_data(&dir);
    assert!(lens.iter().all(|len| *len <= SEGMENT_LIMIT), "segments of {lens:?}");
    let own = dir.list(&of("extents", BRANCH.id)).unwrap();
    assert_eq!(own, Vec::<String>::new(), "extents sealed in files of their own");

    // A commit's data in the open segment, then another 192 KiB of blocks: the segment holding it
    // fills as they come, and closes with the commit after.
    store.write(extent(96), 0, &block(96)).unwrap();
    store.commit(None).unwrap();
    for sequence in 48..96 {
        store.write(extent(sequence), 0, &block(sequence)).unwrap();
        let lens = segment_data(&dir);
        assert!(lens.iter().all(|len| *len <= SEGMENT_LIMIT), "segments of {lens:?}");
    }
    store.commit(None).unwrap();
    check(&store, 0..97);
    let lasts: Vec<u64> = store.closed(usize::MAX).unwrap().iter().map(Closed::last).collect();
    assert_eq!(lasts, [1, 1, 3, 3]);
    let lens = segment_data(&dir);
    assert!(lens.iter().all(|len| *len <= SEGMENT_LIMIT), "segments of {lens:?}");
    drop(store);
    check(&Store::open(dir.clone(), SMALL_SEGMENTS).unwrap(), 0..97);
}

/// The segment the newest checkpoint names, whose footer would follow it, set aside full with no
/// commit's data, and so no footer: the branch is still its newest checkpoint and the footers of
/// its closed segments after it (TASK-79.2).
#[test]
fn segments_set_aside_leave_the_branch_its_footers() {
    let dir = MemDir::new();
    let description = Description { block_size: BLOCK as u32, capacity: 64 << 20, branch: BRANCH };
    let store = Store::create(dir.clone(), description, SMALL_SEGMENTS).unwrap();
    // 192 KiB of blocks after the checkpoint the branch opened with, which names segment 0.
    for sequence in 0..48 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
    }
    assert_eq!(dir.list(&of("segments", BRANCH.id)).unwrap(), Vec::<String>::new());
    let state = closed_state(&store);
    assert_eq!(state.0, 1);
    assert_eq!(reopened_state(store, &dir).0, state);
}

/// An extent written again and again between commits takes no segment past a segment and an
/// extent: its writes go to others (TASK-79.2). It reads as last written, and once committed,
/// where it was last written, in the open segment; the segments it filled, no extent's data in
/// them, closed with the commit, their footers alone (TASK-92.41).
#[test]
fn rewrites_take_no_segment_past_a_segment_and_an_extent() {
    let dir = MemDir::new();
    let description = Description { block_size: BLOCK as u32, capacity: 64 << 20, branch: BRANCH };
    let store = Store::create(dir.clone(), description, SMALL_SEGMENTS).unwrap();
    for seed in 0..64 {
        store.write(extent(1), 0, &pattern(seed, 0, 2 * BLOCK)).unwrap();
        let lens = segment_data(&dir);
        assert!(lens.iter().all(|len| *len <= SEGMENT_LIMIT), "segments of {lens:?}");
    }
    assert_eq!(read(&store, extent(1), 0, 2 * BLOCK), pattern(63, 0, 2 * BLOCK));
    store.commit(None).unwrap();
    settled(&store);
    assert_eq!(read(&store, extent(1), 0, 2 * BLOCK), pattern(63, 0, 2 * BLOCK));
    let place = Place::Run { branch: BRANCH.id, number: 4, offset: 0 };
    assert_eq!(store.place(extent(1)).unwrap(), Some(place));
    let segments: Vec<String> = (0..4).map(|number| format!("{number:016x}")).collect();
    assert_eq!(segment_files(&store, &dir, "open-segments"), ["0000000000000004"]);
    assert_eq!(dir.list(&of("segments", BRANCH.id)).unwrap(), segments);
    for closed in store.closed(usize::MAX).unwrap() {
        let data = extent_frame::data(&closed.bytes().unwrap()).unwrap();
        assert_eq!(data.len(), 0, "segment {}, no extent's data in it, framed", closed.number);
    }
    let store = reopened(store, &dir);
    assert_eq!(read(&store, extent(1), 0, 2 * BLOCK), pattern(63, 0, 2 * BLOCK));
}

/// Rewrites of an open extent take room in segments, which no extent holds once the extent is
/// sealed: past the capacity's worth, they count as used, until the commit sealing the extent
/// takes them back, which is the writer's to make (TASK-79.2); then they are dead space, which
/// takes no room, and which leaves the store with its segment, once acknowledged in the bucket
/// (TASK-165.8). So a guest rewriting a block between commits takes twice its capacity of the
/// host's disk at most until it commits.
#[test]
fn rewrites_past_the_capacity_count_as_used() {
    let capacity = 64 * BLOCK as u64;
    let store = create(MemDir::new(), capacity);
    // The first write, and the capacity's worth of rewrites, less the block the extent holds.
    for seed in 0..128 {
        store.write(extent(1), 0, &pattern(seed, 0, BLOCK)).unwrap();
    }
    assert_eq!(store.info().unwrap().available, 0);
    error_is(store.write(extent(1), 0, &pattern(128, 0, BLOCK)), NoSpace);
    error_is(store.write(extent(2), 0, &pattern(2, 0, BLOCK)), NoSpace);
    assert_eq!(read(&store, extent(1), 0, BLOCK), pattern(127, 0, BLOCK));
    // Sealed, the extent leaves them in a segment no extent holds but it, which takes none of the
    // room: it leaves the store once acknowledged in the bucket (TASK-165.8).
    store.commit(None).unwrap();
    assert_eq!(dead_bytes(&store), 128 * BLOCK as u64);
    assert_eq!(store.info().unwrap().available, capacity - BLOCK as u64);
    store.write(extent(2), 0, &pattern(2, 0, BLOCK)).unwrap();
    assert_eq!(read(&store, extent(1), 0, BLOCK), pattern(127, 0, BLOCK));
}

/// fxdax: what frees of extents open let go of, of what writes since the last commit wrote,
/// written over or not, in segments or in raw files of their own, counts as used past the
/// capacity's worth, as what writes wrote over does, until the commit takes it back (TASK-79.2):
/// so that a writer writing extents and freeing them, never committing, takes twice the capacity
/// of the host's disk at most, though its extents hold nothing.
#[test]
fn frees_of_open_extents_past_the_capacity_count_as_used() {
    let capacity = 64 * BLOCK as u64;
    let store = create(MemDir::new(), capacity);
    // Blocks written and freed; blocks written, written over and freed; and extents in raw files
    // of their own, of 8 blocks, written and freed: twice the capacity's worth, 512 KiB.
    for sequence in 0..32 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
        store.free(extent(sequence)).unwrap();
    }
    for sequence in 32..48 {
        store.write(extent(sequence), 0, &pattern(1, 0, BLOCK)).unwrap();
        store.write(extent(sequence), 0, &pattern(2, 0, BLOCK)).unwrap();
        store.free(extent(sequence)).unwrap();
    }
    for sequence in 48..56 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, 8 * BLOCK)).unwrap();
        store.free(extent(sequence)).unwrap();
    }
    error_is(store.write(extent(56), 0, &pattern(56, 0, BLOCK)), NoSpace);
    assert_eq!(store.info().unwrap().available, 0);
    // The commit takes it back: nothing holds it, as no commit did.
    store.commit(None).unwrap();
    assert_eq!(store.info().unwrap().available, capacity);
    store.write(extent(56), 0, &pattern(56, 0, BLOCK)).unwrap();
    store.commit(None).unwrap();
    assert_eq!(read(&store, extent(56), 0, BLOCK), pattern(56, 0, BLOCK));
}

/// Writes, a commit, and frees of all but one of the extents written, and a commit: a segment
/// closed, which one extent pins. Of data that does not compress, so that what no extent is in
/// takes as much of the host's disk as it is.
fn churn(store: &Store<MemDir>, first: u64) {
    for sequence in first..first + 16 {
        store.write(extent(sequence), 0, &noise(sequence, BLOCK)).unwrap();
    }
    store.commit(None).unwrap();
    for sequence in first + 1..first + 16 {
        store.free(extent(sequence)).unwrap();
    }
    store.commit(None).unwrap();
}

/// What the branch's segments hold that no extent is in, freed, is dead space until its segments
/// leave the store, acknowledged in the bucket, which takes none of the branch's room, past the
/// capacity's worth too: the bucket's collector's to take back, which the branch's writer can't
/// make room for (TASK-79.2, TASK-165.8).
#[test]
fn freed_space_is_dead_space_taking_no_room() {
    let capacity = 64 * BLOCK as u64;
    let store = create(MemDir::new(), capacity);
    for cycle in 0..4 {
        churn(&store, 16 * cycle);
    }
    // 240 KiB freed: within the capacity's worth.
    assert_eq!(dead_bytes(&store), 60 * BLOCK as u64);
    assert_eq!(store.info().unwrap().available, capacity - 4 * BLOCK as u64);
    churn(&store, 64);
    // 300 KiB freed: past the capacity's worth.
    assert_eq!(dead_bytes(&store), 75 * BLOCK as u64);
    assert_eq!(store.info().unwrap().available, capacity - 5 * BLOCK as u64);
    error_is(store.write(extent(100), 0, &vec![1; 60 * BLOCK]), NoSpace);
    store.write(extent(100), 0, &vec![1; 59 * BLOCK]).unwrap();
    store.free(extent(100)).unwrap();
    store.commit(None).unwrap();
    assert_eq!(store.info().unwrap().available, capacity - 5 * BLOCK as u64);
}

/// fxdax: what the branch's segments hold that no extent is in, freed, takes none of the room the
/// branch's writes need, however much it is: it leaves the store with its segments, acknowledged
/// in the bucket, whose collector takes it back (TASK-165.8). Here it gathers commit by commit,
/// past twice the capacity (TASK-79.2).
#[test]
fn dead_space_takes_none_of_the_room_writes_need() {
    let capacity = 64 * BLOCK as u64;
    let store = create(MemDir::new(), capacity);
    // Ten segments, each pinned by one extent: 600 KiB freed, more than twice the capacity.
    for cycle in 0..10 {
        churn(&store, 16 * cycle);
    }
    assert_eq!(dead_bytes(&store), 150 * BLOCK as u64);
    assert_eq!(store.info().unwrap().available, capacity - 10 * BLOCK as u64);
    // All the room left.
    store.write(extent(200), 0, &pattern(200, 0, 54 * BLOCK)).unwrap();
    store.commit(None).unwrap();
    assert_eq!(dead_bytes(&store), 150 * BLOCK as u64);
    assert_eq!(store.info().unwrap().available, 0);
    error_is(store.write(extent(201), 0, &pattern(201, 0, BLOCK)), NoSpace);
    for sequence in (0..160).step_by(16) {
        assert!(read(&store, extent(sequence), 0, BLOCK) == noise(sequence, BLOCK), "{sequence}");
    }
    assert_eq!(read(&store, extent(200), 0, 54 * BLOCK), pattern(200, 0, 54 * BLOCK));
}

/// fxdax: an uploader keeps the frees of a guest writing and freeing files until it has uploaded
/// the segments holding them, and lets go of them at once, the branch's dead space past twice its
/// capacity of a sudden. Counted as used past the capacity's worth, the dead space found the
/// guest's next write, a journal write of Fxfs's, refused (`ENOSPC`), the store a tenth used: the
/// guest's filesystem failed (TASK-79.2). The segments, acknowledged, leave the store, what is
/// live of them read in the bucket, and the dead space with them (TASK-165.8): the write takes
/// the room the extents leave it.
#[test]
fn dead_space_the_uploader_lets_go_takes_none_of_the_room() {
    let capacity = 64 * BLOCK as u64;
    let store = create(MemDir::new(), capacity);
    // Ten segments, each pinned by one extent, what is freed in them kept for the bucket.
    for cycle in 0..10 {
        churn(&store, 16 * cycle);
    }
    assert_eq!(dead_bytes(&store), 150 * BLOCK as u64);
    // Acknowledged, and released: 600 KiB let go of at once, more than twice the capacity, with
    // the segments.
    for closed in store.closed(usize::MAX).unwrap() {
        store.acknowledged(closed.number).unwrap();
    }
    // The write the next commit comes with, of all the room the extents leave.
    store.write(extent(200), 0, &pattern(200, 0, 54 * BLOCK)).unwrap();
    store.commit(None).unwrap();
    // But the frees of the last churn's commit, its segment open still, kept until the bucket
    // acknowledges it, their data there.
    assert_eq!(dead_bytes(&store), 15 * BLOCK as u64);
    assert_eq!(store.usage().unwrap().segments, 1);
    assert_eq!(store.info().unwrap().available, 0);
    for sequence in (0..160).step_by(16) {
        let place = store.place(extent(sequence)).unwrap();
        assert!(matches!(place, Some(Place::Remote(_))), "{sequence} at {place:?}");
    }
    assert_eq!(read(&store, extent(200), 0, 54 * BLOCK), pattern(200, 0, 54 * BLOCK));
}

/// fxdax: the bytes the branch's files of extents' data take of the host's disk: its segments',
/// open and closed, and its extents' own files and raw files (TASK-79.89).
fn data_files(dir: &MemDir) -> u64 {
    let mut bytes = 0;
    for kind in ["segments", "open-segments", "extents", "open-extents"] {
        for name in dir.list(&of(kind, BRANCH.id)).unwrap() {
            let file = dir.open(&format!("{}/{name}", of(kind, BRANCH.id)), true).unwrap();
            bytes += file.size().unwrap();
        }
    }
    bytes
}

/// fxdax: a guest writing a segment of blocks of data that does not compress at each commit, and
/// freeing all but two of them at the next, which pin the segment, until it lets go of them too,
/// twenty-one commits later; its uploader acknowledging each segment the store closes, and
/// releasing each commit, as it goes. Each segment acknowledged leaves the store, its extents read
/// in the bucket from then on: what the pins hold, and what the frees left, is the bucket's, whose
/// collector takes it back (TASK-165.8). Once the uploader has caught up, the branch's files are
/// none but its open segment's, which the commit that closed the last left empty. Before, they
/// took what cleaning behind the commits had yet to take back, which writes were refused past
/// three times the capacity for (TASK-79.89).
#[test]
fn acknowledged_segments_leave_a_churning_guests_files() {
    let capacity = 64 * BLOCK as u64;
    let dir = MemDir::new();
    let store = create(dir.clone(), capacity);
    let mut live = BTreeSet::new();
    let free = |live: &mut BTreeSet<u64>, sequence: u64| {
        store.free(extent(sequence)).unwrap();
        live.remove(&sequence);
    };
    for cycle in 0..100 {
        let first = 16 * cycle;
        if cycle > 0 {
            for sequence in first - 14..first {
                free(&mut live, sequence);
            }
        }
        if cycle > 20 {
            let pins = 16 * (cycle - 21);
            free(&mut live, pins);
            free(&mut live, pins + 1);
        }
        for sequence in first..first + 16 {
            store.write(extent(sequence), 0, &noise(sequence, BLOCK)).unwrap();
            live.insert(sequence);
        }
        store.commit(None).unwrap();
        store.flush().unwrap();
        for closed in store.closed(usize::MAX).unwrap() {
            store.acknowledged(closed.number).unwrap();
        }
        settled(&store);
        assert_eq!(data_files(&dir), 0, "the files after cycle {cycle}");
    }
    for sequence in live {
        let place = store.place(extent(sequence)).unwrap();
        assert!(matches!(place, Some(Place::Remote(_))), "{sequence} at {place:?}");
    }
}

/// fxdax: an honest guest whose uploader lags (TASK-79.89): forty commits, each of sixteen blocks
/// in segments and an extent of eight in a file of its own, of data that does not compress, all of
/// it freed by the next, its extents holding a third of its capacity at most. The uploader keeps
/// what the frees let go of until it releases their commits, and the segments until they are
/// acknowledged in the bucket, if they are as they close (`acknowledges`): returns what the
/// branch's files take then, which the upload lag bound bounds, making commits wait (fxdax's
/// `max_lag_bytes`). Once the uploader catches up, the next commit lets it all go, the bucket
/// holding what it kept (TASK-165.8).
fn uploader_lagging(acknowledges: bool) -> u64 {
    let capacity = 64 * BLOCK as u64;
    let dir = MemDir::new();
    let store = create(dir.clone(), capacity);
    for cycle in 0..40 {
        let first = 17 * cycle;
        for sequence in first..first + 16 {
            store.write(extent(sequence), 0, &noise(sequence, BLOCK)).unwrap();
        }
        store.write(extent(first + 16), 0, &noise(first + 16, 8 * BLOCK)).unwrap();
        store.commit(None).unwrap();
        for sequence in first..first + 17 {
            store.free(extent(sequence)).unwrap();
        }
        store.commit(None).unwrap();
        if acknowledges {
            for closed in store.closed(usize::MAX).unwrap() {
                store.acknowledged(closed.number).unwrap();
            }
        }
    }
    let files = data_files(&dir);
    for closed in store.closed(usize::MAX).unwrap() {
        store.acknowledged(closed.number).unwrap();
    }
    store.commit(None).unwrap();
    assert_eq!(data_files(&dir), 0);
    files
}

/// fxdax: `uploader_lagging`, its uploader acknowledging nothing, nor releasing anything: the
/// branch's files take ten times the capacity, which no bound of the store's refuses writes for
/// (TASK-79.89).
#[test]
fn what_an_uploader_keeps_stays_until_acknowledged() {
    let files = uploader_lagging(false);
    assert!(files > 10 * 64 * BLOCK as u64, "{files} bytes of files");
}

/// fxdax: `uploader_lagging`, its uploader acknowledging the segments as the store closes them,
/// and releasing nothing: the frees it keeps, in the bucket, hold no file of the branch's
/// (TASK-165.8).
#[test]
fn frees_kept_in_segments_acknowledged_hold_no_file() {
    assert_eq!(uploader_lagging(true), 0);
}

/// fxdax: a branch opened again in the same process lets go of the segments its last store set
/// aside, full, or left open, holding only what writes after its last commit wrote: they go, as
/// opening the store would remove them (TASK-79.2).
#[test]
fn segments_no_branch_holds_go_as_branches_open() {
    let dir = MemDir::new();
    let description = Description { block_size: BLOCK as u32, capacity: 64 << 20, branch: BRANCH };
    let store = Store::create(dir.clone(), description, SMALL_SEGMENTS).unwrap();
    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    store.commit(None).unwrap();
    // Two segments' worth after the commit, the first set aside.
    for sequence in 2..50 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
    }
    drop(store);
    let store = Store::open(dir.clone(), SMALL_SEGMENTS).unwrap();
    assert_eq!(store.extents().unwrap(), [(extent(1), BLOCK as u64)]);
    let segments = dir.list(&of("segments", BRANCH.id)).unwrap();
    assert_eq!(segments, ["0000000000000000"]);
    assert_eq!(read(&store, extent(1), 0, BLOCK), pattern(1, 0, BLOCK));
}

/// A write in progress reserves the room it may take, which counts against the capacity until it
/// is finished or abandoned; an abandoned write holds no commit back; and the capacity is room
/// enough to the byte (TASK-44.1).
#[test]
fn writes_reserve_room_until_done() {
    let store = create(MemDir::new(), 64 << 10);
    let first = store.begin_write(extent(1), 0, 24 << 10).unwrap();
    error_is(store.write(extent(2), 0, &[2; 44 << 10]), NoSpace);
    drop(first);
    store.write(extent(2), 0, &[2; 44 << 10]).unwrap();
    store.commit(None).unwrap();
    // To the byte: what is left, exactly.
    store.write(extent(3), 0, &[3; 16 << 10]).unwrap();
    store.write(extent(4), 0, &[4; 4 << 10]).unwrap();
    assert_eq!(store.info().unwrap().available, 0);
    error_is(store.write(extent(5), 0, &[5; BLOCK]), NoSpace);
}

/// Begins a write of `data` at `offset` in `extent`, and writes it to its file, unfinished: a
/// write in flight, which a free may race (TASK-79.1).
fn in_flight<'a>(
    store: &'a Store<MemDir>,
    extent: ExtentId,
    offset: u64,
    data: &[u8],
) -> extent_store::Write<'a, MemDir> {
    let write = store.begin_write(extent, offset, data.len() as u64).unwrap();
    write.file().write_at(data, write.offset()).unwrap();
    write
}

/// The store, reopened from `dir` once `store` is gone: what it made durable.
fn reopened(store: Store<MemDir>, dir: &MemDir) -> Store<MemDir> {
    drop(store);
    Store::open(dir.clone(), OPTIONS).unwrap()
}

/// A write in flight to an extent freed meanwhile, which the extent's next write placed in the
/// open segment rather than its file of its own, is dropped as it finishes: the extent holds what
/// was written after the free, and the store goes on, commits and opens again (TASK-79.1).
#[test]
fn writes_finishing_after_their_extents_free_and_rewrite_are_dropped() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    let freed = in_flight(&store, extent(1), 0, &pattern(1, 0, 40 << 10));
    store.free(extent(1)).unwrap();
    store.write(extent(1), BLOCK as u64, &pattern(2, 0, BLOCK)).unwrap();
    freed.finish();
    let written = [vec![0; BLOCK], pattern(2, 0, BLOCK)].concat();
    assert!(read(&store, extent(1), 0, 2 * BLOCK) == written, "the extent reads other data");
    store.commit(None).unwrap();
    let store = reopened(store, &dir);
    assert_eq!(store.extents().unwrap(), [(extent(1), 2 * BLOCK as u64)]);
    assert!(read(&store, extent(1), 0, 2 * BLOCK) == written, "the extent reads other data");
}

/// A write in flight to an extent freed meanwhile, and not written since, is dropped as it
/// finishes: the store holds no such extent, commits, and opens again (TASK-79.1).
#[test]
fn writes_finishing_after_their_extents_free_are_dropped() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    let freed = in_flight(&store, extent(1), 0, &pattern(1, 0, 40 << 10));
    store.free(extent(1)).unwrap();
    freed.finish();
    assert_eq!(store.extent(extent(1)).unwrap(), None);
    assert_eq!(store.info().unwrap().used, 0);
    store.commit(None).unwrap();
    let store = reopened(store, &dir);
    assert_eq!(store.extents().unwrap(), []);
    assert_eq!(dir.list(&of("open-extents", BRANCH.id)).unwrap(), Vec::<String>::new());
    assert_eq!(dir.list(&of("extents", BRANCH.id)).unwrap(), Vec::<String>::new());
}

/// Extents freed while open, and written anew, are held by the commit that seals them: the footer
/// of its segment names them among the extents it seals, in the segment or in files of their own,
/// and not among those it frees, which readers of footers apply after those, the store opened
/// again or object storage (TASK-79.1).
#[test]
fn footers_hold_extents_freed_open_and_written_anew() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    let lens = [BLOCK, 40 << 10];
    for (sequence, len) in (1..).zip(lens) {
        store.write(extent(sequence), 0, &pattern(1, 0, len)).unwrap();
        store.free(extent(sequence)).unwrap();
        store.write(extent(sequence), 0, &pattern(2, 0, len)).unwrap();
    }
    let state = closed_state(&store);
    assert_eq!(state.3.len(), 2);
    assert_eq!(reopened_state(store, &dir).0, state);
}

/// A write in flight to an extent freed meanwhile, written again in the open segment as it was
/// before, lands none of its bytes in the extent written again (TASK-79.1).
#[test]
fn writes_finishing_after_their_extents_free_land_nothing_in_its_next_writes() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    let freed = in_flight(&store, extent(1), 0, &pattern(1, 0, BLOCK));
    store.free(extent(1)).unwrap();
    store.write(extent(1), BLOCK as u64, &pattern(2, 0, BLOCK)).unwrap();
    freed.finish();
    let written = [vec![0; BLOCK], pattern(2, 0, BLOCK)].concat();
    assert!(read(&store, extent(1), 0, 2 * BLOCK) == written, "the extent reads other data");
    store.commit(None).unwrap();
    assert_eq!(store.usage().unwrap().live_bytes, 2 * BLOCK as u64);
    let store = reopened(store, &dir);
    assert!(read(&store, extent(1), 0, 2 * BLOCK) == written, "the extent reads other data");
}

/// An abandoned write to a large extent, never written since, leaves its raw file to no one: the
/// commit removes it, once durable, and the extent may be written anew (TASK-79.5).
#[test]
fn abandoned_writes_leave_no_raw_files() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    drop(store.begin_write(extent(1), 0, 40 << 10).unwrap());
    store.commit(None).unwrap();
    assert_eq!(dir.list(&of("open-extents", BRANCH.id)).unwrap(), Vec::<String>::new());
    store.write(extent(1), 0, &pattern(1, 0, 40 << 10)).unwrap();
    store.commit(None).unwrap();
    assert_eq!(read(&store, extent(1), 0, 40 << 10), pattern(1, 0, 40 << 10));
}

/// An abandoned write to a large extent, never written since, then a commit only ordered: the
/// extent may be written anew before the commit's flush, as the guest's next commit is, its raw
/// file gone with the commit's seal, not its flush (TASK-79.5).
#[test]
fn abandoned_writes_leave_no_raw_files_to_writes_before_the_flush() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    drop(store.begin_write(extent(1), 0, 40 << 10).unwrap());
    store.commit_ordered(None).unwrap();
    assert_eq!(dir.list(&of("open-extents", BRANCH.id)).unwrap(), Vec::<String>::new());
    store.write(extent(1), 0, &pattern(1, 0, 40 << 10)).unwrap();
    store.commit(None).unwrap();
    assert!(read(&store, extent(1), 0, 40 << 10) == pattern(1, 0, 40 << 10), "other data");
    let store = reopened(store, &dir);
    assert!(read(&store, extent(1), 0, 40 << 10) == pattern(1, 0, 40 << 10), "other data");
}

/// A large extent freed while open is written anew, large again, before any commit: the raw file
/// it had, which the free left to the next commit's flush to remove, makes way for another
/// (TASK-79.5).
#[test]
fn large_extents_freed_open_are_written_anew_at_once() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    store.write(extent(1), 0, &pattern(1, 0, 40 << 10)).unwrap();
    store.free(extent(1)).unwrap();
    store.write(extent(1), 0, &pattern(2, 0, 40 << 10)).unwrap();
    assert!(read(&store, extent(1), 0, 40 << 10) == pattern(2, 0, 40 << 10), "other data");
    store.commit(None).unwrap();
    assert_eq!(store.place(extent(1)).unwrap(), Some(Place::Own));
    let store = reopened(store, &dir);
    assert!(read(&store, extent(1), 0, 40 << 10) == pattern(2, 0, 40 << 10), "other data");
    assert_eq!(dir.list(&of("open-extents", BRANCH.id)).unwrap(), Vec::<String>::new());
}

/// A write in flight to a large extent freed meanwhile, which the extent's next write placed in a
/// file of its own again, is dropped as it finishes: the extent holds what was written after the
/// free, in a raw file of its own, not the one the write in flight wrote to (TASK-79.1,
/// TASK-79.5).
#[test]
fn writes_finishing_after_their_large_extents_free_and_rewrite_are_dropped() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    let freed = in_flight(&store, extent(1), 0, &pattern(1, 0, 40 << 10));
    store.free(extent(1)).unwrap();
    store.write(extent(1), 0, &pattern(2, 0, 40 << 10)).unwrap();
    freed.finish();
    assert!(read(&store, extent(1), 0, 40 << 10) == pattern(2, 0, 40 << 10), "other data");
    store.commit(None).unwrap();
    assert_eq!(store.place(extent(1)).unwrap(), Some(Place::Own));
    let store = reopened(store, &dir);
    assert!(read(&store, extent(1), 0, 40 << 10) == pattern(2, 0, 40 << 10), "other data");
}

/// A branch whose store failed, opened again in the same process, takes writes to the extents the
/// failed store had open, whose raw files go as it opens (TASK-79.5).
#[test]
fn branches_opened_again_after_a_failure_leave_no_raw_files() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    store.write(extent(1), 0, &pattern(1, 0, 40 << 10)).unwrap();
    dir.fail_after(Some(0));
    assert_eq!(store.commit(None).unwrap_err().to_string(), "committing the extent store");
    dir.fail_after(None);
    drop(store);
    let store = Store::open(dir.clone(), OPTIONS).unwrap();
    assert_eq!(dir.list(&of("open-extents", BRANCH.id)).unwrap(), Vec::<String>::new());
    store.write(extent(1), 0, &pattern(2, 0, 40 << 10)).unwrap();
    store.commit(None).unwrap();
    assert_eq!(read(&store, extent(1), 0, 40 << 10), pattern(2, 0, 40 << 10));
}

/// A branch whose flush failed, opened again in the same process as of its last durable commit,
/// keeps the files that commit's state holds, which the commit not durable let go: they go not as
/// the branch commits again (TASK-79.5).
#[test]
fn branches_opened_again_after_a_failed_flush_keep_what_they_hold() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    store.write(extent(1), 0, &pattern(1, 0, 40 << 10)).unwrap();
    store.commit(None).unwrap();
    store.free(extent(1)).unwrap();
    store.commit_ordered(None).unwrap();
    dir.fail_after(Some(0));
    assert_eq!(store.flush().unwrap_err().to_string(), "committing the extent store");
    dir.fail_after(None);
    drop(store);
    let store = Store::open(dir.clone(), OPTIONS).unwrap();
    assert_eq!(store.epoch().unwrap(), 1);
    store.commit(None).unwrap();
    assert_eq!(read(&store, extent(1), 0, 40 << 10), pattern(1, 0, 40 << 10));
}

/// fxdax: a store of eight closed segments, an extent in each, which keeps the files of
/// `open_files` of them open.
fn eight_segments(open_files: usize) -> (MemDir, Store<MemDir>) {
    let dir = MemDir::new();
    let options = Options { open_files, ..OPTIONS };
    let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: BRANCH };
    let store = Store::create(dir.clone(), description, options).unwrap();
    for sequence in 0..8 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
        store.close_segment().unwrap();
        store.commit(None).unwrap();
    }
    settled(&store);
    // fxdax: and the next segment's log prepared, whose spare its preparation opens (TASK-333).
    store.prepared().unwrap();
    (dir, store)
}

/// The files opened, and the bytes read, reading, through `store`, the extents of a store
/// [`eight_segments`] made: three again and again, and each of the others once between.
fn opened_reading(dir: &MemDir, store: &Store<MemDir>) -> (u64, u64) {
    let before = dir.io();
    for other in 3..8 {
        for sequence in [0, 1, 2, other] {
            assert_eq!(read(store, extent(sequence), 0, BLOCK), pattern(sequence as u8, 0, BLOCK));
        }
    }
    let after = dir.io();
    (after.opened - before.opened, after.read - before.read)
}

/// fxdax: a branch reading what it holds in more closed segments than it keeps the files of open
/// opens each of them once: as one more opens, the least lately read closes,
/// not all of them, which reads would open again; and opening one reads its table and trailer,
/// not its footer, a block, as each part's first read does (TASK-92.2).
#[test]
fn closed_segments_files_close_least_lately_read_first() {
    let (dir, store) = eight_segments(4);
    assert_eq!(opened_reading(&dir, &store), (8, 16 * BLOCK as u64));
}

/// An extent written past its end grows by what the write adds, no more.
#[test]
fn extending_an_extent_counts_its_growth() {
    let store = create(MemDir::new(), 1 << 20);
    store.write(extent(1), 0, &[1; 2 * BLOCK]).unwrap();
    store.write(extent(1), BLOCK as u64, &[2; 2 * BLOCK]).unwrap();
    assert_eq!(store.info().unwrap().used, 3 * BLOCK as u64);
}

/// A shrink counts the room writes in progress reserve, as held.
#[test]
fn shrinks_count_writes_in_progress() {
    let store = create(MemDir::new(), 1 << 20);
    store.write(extent(1), 0, &[1; 4 * BLOCK]).unwrap();
    let write = store.begin_write(extent(2), 0, 2 * BLOCK as u64).unwrap();
    let held = 6 * BLOCK as u64;
    let refused = store.set_capacity(held - BLOCK as u64).unwrap_err();
    assert_eq!(
        refused.downcast_ref::<TooSmall>(),
        Some(&TooSmall { capacity: held - BLOCK as u64, held })
    );
    store.set_capacity(held).unwrap();
    drop(write);
}

/// fxdax: a follower waiting for a commit after the one it has hears of the latest, or, if none
/// came, of none.
#[test]
fn newer_commits_are_heard_of() {
    let store = create(MemDir::new(), 1 << 20);
    for _ in 0..3 {
        store.commit(None).unwrap();
    }
    let epoch = store.epoch().unwrap();
    assert_eq!(store.newer(epoch, std::time::Duration::ZERO), None);
    store.commit(None).unwrap();
    store.commit(None).unwrap();
    assert_eq!(store.newer(epoch, std::time::Duration::ZERO), Some(epoch + 2));
}

/// fxdax: ordered commits wait for a flush to make them durable, and say so.
#[test]
fn ordered_commits_are_pending_until_flushed() {
    let store = create(MemDir::new(), 1 << 20);
    assert!(!store.flush_pending());
    store.write(extent(1), 0, &[1; BLOCK]).unwrap();
    store.commit_ordered(None).unwrap();
    assert!(store.flush_pending());
    store.flush().unwrap();
    assert!(!store.flush_pending());
}

/// Where an extent's data is: one run in a segment, once sealed in one write, fxdax: or in writes
/// one right after another (TASK-92.1); pieces while open or written in several otherwise; its own
/// file if large.
#[test]
fn places_of_extents() {
    let store = create(MemDir::new(), 1 << 20);
    store.write(extent(1), 0, &[1; 2 * BLOCK]).unwrap();
    // Its second block first.
    store.write(extent(2), BLOCK as u64, &[2; BLOCK]).unwrap();
    store.write(extent(2), 0, &[2; BLOCK]).unwrap();
    store.write(extent(3), BLOCK as u64, &[3; BLOCK]).unwrap();
    store.write(extent(4), 0, &[4; 40 << 10]).unwrap();
    store.write(extent(6), 0, &[6; BLOCK]).unwrap();
    store.write(extent(6), BLOCK as u64, &[6; BLOCK]).unwrap();
    assert_eq!(
        store.place(extent(1)).unwrap(),
        Some(Place::Run { branch: BRANCH.id, number: 0, offset: 0 })
    );
    assert_eq!(store.place(extent(2)).unwrap(), Some(Place::Pieces));
    // Not from its start.
    assert_eq!(store.place(extent(3)).unwrap(), Some(Place::Pieces));
    assert_eq!(store.place(extent(4)).unwrap(), Some(Place::Own));
    assert_eq!(store.place(extent(5)).unwrap(), None);
    assert_eq!(
        store.place(extent(6)).unwrap(),
        Some(Place::Run { branch: BRANCH.id, number: 0, offset: 5 * BLOCK as u64 })
    );
}

/// fxdax: the uploader's lag counts the bytes of the write-ahead log's segments not yet
/// acknowledged, their data and their extents of their own, the open segment's included:
/// acknowledged, they count no more (TASK-165.9).
#[test]
fn the_lag_counts_the_logs_bytes_not_acknowledged() {
    let store = create(MemDir::new(), 1 << 20);
    for (sequence, len) in [(1, BLOCK), (2, 2 * BLOCK), (3, 40 << 10)] {
        store.write(extent(sequence), 0, &vec![sequence as u8; len]).unwrap();
        store.close_segment().unwrap();
        store.commit(None).unwrap();
    }
    assert_eq!(store.wal_bytes().unwrap(), 3 * BLOCK as u64 + (40 << 10));
    let closed = store.closed(usize::MAX).unwrap();
    store.acknowledged(closed[0].number).unwrap();
    assert_eq!(store.wal_bytes().unwrap(), 2 * BLOCK as u64 + (40 << 10));
    // In order only.
    assert!(store.acknowledged(closed[2].number).is_err());
    // The open segment's, committed in it, and on their own.
    store.write(extent(4), 0, &[4; BLOCK]).unwrap();
    store.write(extent(5), 0, &[5; 40 << 10]).unwrap();
    store.commit(None).unwrap();
    assert_eq!(store.wal_bytes().unwrap(), 3 * BLOCK as u64 + (80 << 10));
}

/// fxdax: an uploader starting while commits wait to be made durable takes no segment of the
/// commits after it until they are: a crash may lose them here, which the bucket would hold
/// (TASK-165.9).
#[test]
fn uploads_started_with_commits_pending_take_only_durable_ones() {
    let store = create(MemDir::new(), 1 << 20);
    store.write(extent(1), 0, &[1; BLOCK]).unwrap();
    store.commit_ordered(None).unwrap();
    store.commit_ordered(None).unwrap();
    store.write(extent(2), 0, &[2; BLOCK]).unwrap();
    store.commit_ordered(None).unwrap();
    assert!(store.close_now().unwrap());
    assert!(store.closed(usize::MAX).unwrap().is_empty());
    store.flush().unwrap();
    assert_eq!(closed_one(&store).last(), 3);
}

/// A closed segment's last commit is the commit that closed it.
#[test]
fn closed_segments_know_their_last_commit() {
    let store = create(MemDir::new(), 1 << 20);
    store.write(extent(1), 0, &[1; BLOCK]).unwrap();
    store.commit(None).unwrap();
    store.write(extent(2), 0, &[2; BLOCK]).unwrap();
    store.commit(None).unwrap();
    assert!(store.close_now().unwrap());
    let closed = store.closed(usize::MAX).unwrap();
    let [closed] = &closed[..] else { panic!("{} segments closed", closed.len()) };
    assert_eq!(closed.last(), 2);
}

/// The raw file of an extent freed while open goes with the next durable commit, though the open
/// segment closed between.
#[test]
fn files_freed_between_commits_go_across_closings() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    store.write(extent(1), 0, &[1; BLOCK]).unwrap();
    store.commit(None).unwrap();
    store.write(extent(2), 0, &[2; 40 << 10]).unwrap();
    store.free(extent(2)).unwrap();
    assert!(store.close_now().unwrap());
    // Nothing committed since: nothing to close.
    assert!(!store.close_now().unwrap());
    store.commit(None).unwrap();
    assert_eq!(dir.list(&of("open-extents", BRANCH.id)).unwrap(), Vec::<String>::new());
}

/// A commit lists an extent it wrote in two writes once, at its length: its segment's footer names
/// it so, and the bytes it wrote count it so.
#[test]
fn commits_list_each_extent_written_once() {
    let store = create(MemDir::new(), 1 << 20);
    store.write(extent(1), 0, &[1; BLOCK]).unwrap();
    store.write(extent(1), BLOCK as u64, &[1; BLOCK]).unwrap();
    store.commit(None).unwrap();
    assert!(store.close_now().unwrap());
    assert_eq!(store.wal_bytes().unwrap(), 2 * BLOCK as u64);
    let written = closed_one(&store).footer.contents.extents;
    assert_eq!(
        written.iter().map(|&(extent, _, len)| (extent, len)).collect::<Vec<_>>(),
        [(extent(1), 2 * BLOCK as u64)]
    );
}

/// A fork's commit ends the segment that names the fork, and the fork gives its epoch.
#[test]
fn forks_give_their_commits_epoch() {
    let store = create(MemDir::new(), 1 << 20);
    store.write(extent(1), 0, &[1; BLOCK]).unwrap();
    store.commit(None).unwrap();
    let forked =
        Child { branch: extent_remote::layout::test_branch(BRANCH.id + 1), owner: 0, point: false };
    assert_eq!(store.commit_fork(forked).unwrap(), 2);
    assert_eq!(store.epoch().unwrap(), 2);
    let forking = closed_one(&store);
    assert_eq!((forking.last(), forking.footer.contents.forks.clone()), (2, vec![forked]));
}

/// fxdax: a fork's child in another cell takes no ID of a branch its parent holds extents of, its
/// parent's own among them: an extent's ID names its branch by the ID alone (TASK-364). It is
/// refused before anything is committed.
#[test]
fn forks_take_no_id_of_their_lineage() {
    let store = create(MemDir::new(), 1 << 20);
    store.write(extent(1), 0, &[1; BLOCK]).unwrap();
    store.commit(None).unwrap();
    let elsewhere = extent_remote::layout::Cell { tenant: 9, id: 9 };
    let child = extent_remote::layout::Branch { cell: elsewhere, id: BRANCH.id };
    let forked = Child { branch: child, owner: 0, point: false };
    let refused = store.commit_fork(forked).unwrap_err();
    assert_eq!(refused.downcast_ref(), Some(&extent_store::Inherited(child)));
    assert_eq!(store.epoch().unwrap(), 1);
    let other =
        Child { branch: extent_remote::layout::Branch { id: BRANCH.id + 1, ..child }, ..forked };
    assert_eq!(store.commit_fork(other).unwrap(), 2);
}

/// With segments closing as soon as they are old, an empty one closes not: a commit that wrote
/// nothing leaves no segment.
#[test]
fn empty_segments_close_not() {
    let options = Options { segment_age: std::time::Duration::ZERO, ..OPTIONS };
    let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: BRANCH };
    let store = Store::create(MemDir::new(), description, options).unwrap();
    store.commit(None).unwrap();
    store.commit(None).unwrap();
    assert_eq!(store.closed(usize::MAX).unwrap().len(), 0);
    store.write(extent(1), 0, &[1; BLOCK]).unwrap();
    store.commit(None).unwrap();
    assert_eq!(store.closed(usize::MAX).unwrap().len(), 1);
}

/// Sealing an extent written in pieces moves its data to one run: what is live in the segments is
/// its length, once.
#[test]
fn sealing_moves_pieces_live_once() {
    let store = create(MemDir::new(), 1 << 20);
    store.write(extent(1), BLOCK as u64, &[1; BLOCK]).unwrap();
    store.write(extent(1), 0, &[2; BLOCK]).unwrap();
    store.commit(None).unwrap();
    assert_eq!(store.usage().unwrap().live_bytes, 2 * BLOCK as u64);
    assert_eq!(read(&store, extent(1), 0, 2 * BLOCK), [[2; BLOCK], [1; BLOCK]].concat());
}

/// fxdax: an extent written in runs one after another in the open segment, as the guest's device
/// splits a write larger than its most I/O (1008 KiB) in two, is one run, which its commit seals
/// where it is, reading and writing none of it again, whichever of its writes finished first; and
/// is so once opened again (TASK-92.1).
#[test]
fn extents_written_in_runs_seal_where_they_are() {
    const MIB: usize = 1 << 20;
    const MAX_IO: usize = 1008 << 10;
    let options = Options { segment_size: 32 << 20, large_extent: 4 << 20, ..OPTIONS };
    let dir = MemDir::new();
    let description = Description { block_size: BLOCK as u32, capacity: 64 << 20, branch: BRANCH };
    let store = Store::create(dir.clone(), description, options).unwrap();
    let data = |sequence| pattern(sequence as u8, 0, MIB);
    store.write(extent(1), 0, &data(1)[..MAX_IO]).unwrap();
    store.write(extent(1), MAX_IO as u64, &data(1)[MAX_IO..]).unwrap();
    let first = in_flight(&store, extent(2), 0, &data(2)[..MAX_IO]);
    let second = in_flight(&store, extent(2), MAX_IO as u64, &data(2)[MAX_IO..]);
    second.finish();
    first.finish();
    let before = dir.io();
    store.commit_ordered(None).unwrap();
    let sealed = dir.io();
    assert_eq!((sealed.read - before.read, sealed.written - before.written), (0, 0));
    let runs = [(1, 0), (2, MIB as u64)].map(|(sequence, offset)| {
        (extent(sequence), MIB as u64, Place::Run { branch: BRANCH.id, number: 0, offset })
    });
    assert_eq!(store.places().unwrap(), runs);
    store.flush().unwrap();
    drop(store);
    let store = Store::open(dir, options).unwrap();
    assert_eq!(store.places().unwrap(), runs);
    for sequence in 1..=2 {
        assert!(read(&store, extent(sequence), 0, MIB) == data(sequence), "{sequence} differs");
    }
}

/// An extent larger than the part sealing frames at once (16 MiB) is framed whole.
#[test]
fn extents_larger_than_a_framing_chunk_seal_whole() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 64 << 20);
    let data = pattern(9, 0, (17 << 20) + BLOCK);
    store.write(extent(1), 0, &data).unwrap();
    store.commit(None).unwrap();
    assert_eq!(store.usage().unwrap().own_bytes, data.len() as u64);
    // Its file frames its data, no more.
    let file = dir.open(&format!("{}/{:016x}", of("extents", BRANCH.id), 1), true).unwrap();
    let mut framed = vec![0; file.size().unwrap() as usize];
    file.read_at(&mut framed, 0).unwrap();
    assert_eq!(extent_frame::data(&framed).unwrap().len(), data.len());
    drop(store);
    let store = Store::open(dir, OPTIONS).unwrap();
    assert!(read(&store, extent(1), 0, data.len()) == data, "the extent reads other data");
}

/// Zero bytes are no write.
#[test]
fn zero_byte_writes_are_refused() {
    let store = create(MemDir::new(), 1 << 20);
    assert!(store.write(extent(1), 0, &[]).is_err());
}

/// A write, or a read, ends within the most bytes an extent holds, doc-4's `max_extent`: one past
/// it, or so far that its end wraps, fails, and makes no extent, which its seal and its uploader
/// would hold whole in memory, however long the guest made it (TASK-79.2).
#[test]
fn io_ends_within_the_most_an_extent_holds() {
    let store = create(MemDir::new(), 1 << 40);
    let refused = |result: Result<(), anyhow::Error>| result.expect_err("succeeded").to_string();
    let past = refused(store.write(extent(1), MAX_EXTENT, &[1; BLOCK]));
    assert_eq!(past, format!("{BLOCK} bytes at {MAX_EXTENT}: past the most an extent holds"));
    let too_long = TooLong { offset: MAX_EXTENT, len: BLOCK as u64 };
    error_is(store.write(extent(1), MAX_EXTENT, &[1; BLOCK]), too_long);
    // Its end past the largest offset.
    let wrapping = u64::MAX - (BLOCK as u64 - 1);
    let wrapped = refused(store.write(extent(1), wrapping, &[1; 2 * BLOCK]));
    let expected = format!("{} bytes at {wrapping}: past the most an extent holds", 2 * BLOCK);
    assert_eq!(wrapped, expected);
    assert_eq!(refused(store.read(extent(1), wrapping, &mut [0; 2 * BLOCK])), expected);
    assert_eq!(store.extents().unwrap(), []);
    // To its end, the most.
    store.write(extent(1), MAX_EXTENT - BLOCK as u64, &[1; BLOCK]).unwrap();
    assert_eq!(store.extents().unwrap(), [(extent(1), MAX_EXTENT)]);
}

/// What is read past an extent's end, or where nothing was written, reads as zeros, whatever the
/// buffer held.
#[test]
fn reads_past_an_extents_end_are_zeros() {
    let store = create(MemDir::new(), 1 << 20);
    store.write(extent(1), BLOCK as u64, &[1; BLOCK]).unwrap();
    store.write(extent(2), 0, &[2; 40 << 10]).unwrap();
    for (extent, len) in [(extent(1), 2 * BLOCK), (extent(2), 40 << 10)] {
        let mut data = vec![0xff; len + 2 * BLOCK];
        store.read(extent, 0, &mut data).unwrap();
        assert!(data[len..].iter().all(|byte| *byte == 0), "past {extent:?}'s end");
    }
    let mut data = vec![0xff; 3 * BLOCK];
    store.read(extent(1), 0, &mut data).unwrap();
    assert_eq!(data, [vec![0; BLOCK], vec![1; BLOCK], vec![0; BLOCK]].concat());
}

/// A commit that fails part way, which leaves the store's state ahead of its files, fails the store:
/// it does nothing more, though its file system works again. Here the file system fails the
/// commit's first change to it: a seal's copy of an extent written in two writes, its second
/// block first, which are not one run, or the creation of the next open segment's file as the
/// commit closes the last. fxdax: not that of the index file a checkpoint begins, which the
/// commit's flush makes (`checkpoints_failing_in_their_flushes_fail_the_store`, TASK-92.56).
#[test]
fn commits_failing_part_way_fail_the_store() {
    let created = || {
        let dir = MemDir::new();
        let description =
            Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: BRANCH };
        let store = Store::create(dir.clone(), description, OPTIONS).unwrap();
        (dir, store)
    };
    let sealing = || {
        let (dir, store) = created();
        store.write(extent(1), BLOCK as u64, &[2; BLOCK]).unwrap();
        store.write(extent(1), 0, &[1; BLOCK]).unwrap();
        (dir, store)
    };
    let closing = || {
        let (dir, store) = created();
        store.close_segment().unwrap();
        (dir, store)
    };
    for (dir, store) in [sealing(), closing()] {
        dir.fail_after(Some(0));
        let failed = store.commit_ordered(None).unwrap_err();
        assert_eq!(failed.to_string(), "committing the extent store");
        for budget in [Some(0), None] {
            dir.fail_after(budget);
            let failure = |result: Result<(), anyhow::Error>| result.unwrap_err().to_string();
            // fxdax: saying why (TASK-165.9).
            let failed = "the extent store failed: the file system crashed";
            assert_eq!(failure(store.commit(None)), failed);
            assert_eq!(failure(store.read(extent(1), 0, &mut [0; BLOCK])), failed);
            assert_eq!(failure(store.epoch().map(drop)), failed);
        }
    }
}

/// fxdax: an ordered commit's record waits for its flush, which appends it to the open segment's
/// log: a crash before leaves the store as of the commit before; its flush failing fails the
/// store, as a flush failing does, its state ahead of its files (TASK-165.9).
#[test]
fn logs_failing_in_their_flushes_fail_the_store() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    store.commit_ordered(Some(b"ordered")).unwrap();
    assert_eq!(Store::open(dir.crash(0), OPTIONS).unwrap().epoch().unwrap(), 0);
    dir.fail_after(Some(0));
    let failure = |result: Result<(), anyhow::Error>| result.unwrap_err().to_string();
    assert_eq!(failure(store.flush()), "committing the extent store");
    dir.fail_after(None);
    let failed = "the extent store failed: committing the extent store: appending commits' \
                  records: the file system crashed";
    assert_eq!(failure(store.commit(None)), failed);
    assert_eq!(failure(store.flush()), failed);
}

/// Parts stored compressed are unpacked once while they are kept unpacked, the 32 read last.
#[test]
fn parts_read_again_are_unpacked_once() {
    let description = Description { block_size: BLOCK as u32, capacity: 64 << 20, branch: BRANCH };
    let store = Store::create(MemDir::new(), description, OPTIONS).unwrap();
    const PART: usize = 1 << 20;
    store.write(extent(1), 0, &pattern(1, 0, 34 * PART)).unwrap();
    store.commit(None).unwrap();
    let read_part = |part: usize| read(&store, extent(1), (part * PART) as u64, BLOCK);
    let unpacks = || extent_store::unpacks();
    let before = unpacks();
    for part in 0..32 {
        assert_eq!(read_part(part), pattern(1, part * PART, BLOCK));
    }
    assert_eq!(unpacks() - before, 32);
    // All 32 kept: none unpacked again.
    for part in 0..32 {
        read_part(part);
    }
    assert_eq!(unpacks() - before, 32);
    // Two more: the two read least recently go.
    read_part(32);
    read_part(33);
    assert_eq!(unpacks() - before, 34);
    read_part(5);
    assert_eq!(unpacks() - before, 34);
    read_part(0);
    assert_eq!(unpacks() - before, 35);
}

/// fxdax: a store's bytes are its branch's own, of the extents it wrote, or inherited from the
/// branches it was forked from, which it holds in the bucket (TASK-165.7), as they write and free,
/// and opened again (TASK-49).
#[test]
fn stores_tell_their_own_bytes_from_those_inherited() {
    let parent = BRANCH.id + 1;
    let dir = MemDir::new();
    let mut state = extent_remote::State::new(BRANCH);
    for (sequence, blocks) in [(1, 2), (2, 3)] {
        let key = "segments/0000000000000008/0000000000000001";
        let location = bucket_location(key, sequence * 8 * BLOCK as u64, blocks * BLOCK as u64);
        state.extents.insert(ExtentId::new(parent, sequence), location);
    }
    let store = created_on(dir.clone(), 1 << 20, state.clone(), WRITER);
    store.write(extent(1), 0, &[3; BLOCK]).unwrap();
    store.write(extent(2), 0, &[4; 40 << 10]).unwrap();
    store.free(ExtentId::new(parent, 1)).unwrap();
    store.free(extent(2)).unwrap();
    store.commit(None).unwrap();
    let bytes = |store: &Store<MemDir>| {
        let info = store.info().unwrap();
        (info.own, info.inherited)
    };
    let block = BLOCK as u64;
    assert_eq!(bytes(&store), (block, 3 * block));
    drop(store);
    assert_eq!(bytes(&opened_on(dir, state, WRITER)), (block, 3 * block));
}

/// TASK-295: a write-ahead log dropped, as `fxdaxctl take` drops one another writer's commits
/// moved the branch on from, stays dropped whatever a crash after it keeps: the store's branch is
/// what it committed since, and none of the log's.
#[test]
fn a_dropped_log_stays_dropped_through_a_crash() {
    for seed in 0..64 {
        let dir = MemDir::new();
        let store = create(dir.clone(), 64 << 20);
        for sequence in 1..=2 {
            store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
            store.close_segment().unwrap();
            store.commit(None).unwrap();
        }
        drop(store);
        let mut recovering = Store::recover(dir.clone()).unwrap();
        recovering.drop_wal().unwrap();
        // Its commit in its open segment: no segment framed since, whose directory's sync would
        // make the drop's removals durable too.
        let store = recovering.open(OPTIONS, Opening::new(BRANCH)).unwrap();
        store.write(extent(3), 0, &pattern(3, 0, BLOCK)).unwrap();
        store.commit(None).unwrap();
        let crashed = dir.crash(seed);
        drop(store);
        let reopened = Store::open(crashed, OPTIONS).unwrap();
        assert_eq!(reopened.extents().unwrap(), [(extent(3), BLOCK as u64)], "seed {seed}");
    }
}

/// TASK-359: a store whose directory is on another filesystem than the host's cache is refused
/// as it opens: what the bucket acknowledged goes into the cache by renames, which can't cross
/// filesystems, and every read of it would go to the bucket. On Linux, /dev/shm (tmpfs) and the
/// target directory (on disk) are two.
#[cfg(target_os = "linux")]
#[test]
fn stores_elsewhere_than_the_hosts_cache_are_refused() {
    use std::os::unix::fs::MetadataExt as _;
    let cached = tempfile::tempdir_in("/dev/shm").unwrap();
    let temp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let device = |path: &std::path::Path| std::fs::metadata(path).unwrap().dev();
    assert_ne!(device(cached.path()), device(temp.path()), "two filesystems");
    let cache = Arc::new(extent_remote::cache::Cache::open(cached.path(), 1 << 20, 0).unwrap());
    let cache_dir = cache.dir().to_owned();
    let dir = HostDir::new(temp.path(), true).unwrap();
    let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: BRANCH };
    Store::make(&dir, description, None, None).unwrap();
    let opening = Opening { cache: Some(cache), ..Opening::new(BRANCH) };
    let refused = Store::recover_unmade(dir).unwrap().open(OPTIONS, opening).err().unwrap();
    assert_eq!(
        refused.to_string(),
        format!(
            "the store at {}/ is on another filesystem than the host's cache at {}: the store's \
             segments go into the cache by renaming them",
            temp.path().display(),
            cache_dir.display()
        )
    );
}
