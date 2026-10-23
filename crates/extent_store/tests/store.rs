// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The store's behaviour, in memory and in a directory of the host's.

use extent_frame::segment::Chain;
use extent_store::{
    Closed, Description, Dir, File as _, Footprint, HostDir, Lineage, MemDir, MemFile, NoSpace,
    Options, Part, Place, RemoteData, Store, TooLong, TooSmall,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use storage_device::{ExtentId, NoExtent, OtherBranch, SealedExtent};

const BLOCK: usize = 4096;
const BRANCH: u64 = 7;
/// The most bytes an extent holds: doc-4's `max_extent`, the store's default.
const MAX_EXTENT: u64 = 64 << 20;

/// Small segments and large extents, so that tests make several of each.
const OPTIONS: Options = Options {
    max_extent: MAX_EXTENT,
    segment_size: 64 << 10,
    segment_age: std::time::Duration::from_secs(3600),
    large_extent: 32 << 10,
    checkpoint_after: 4 << 10,
    open_files: 2,
    view_lag: None,
    view_kept_bytes: None,
};

/// The chain of a writer of the branch in object storage, whose segments the store closes.
const WRITER: Chain = Chain::new(BRANCH, 1, 0, None, 0);

fn extent(sequence: u64) -> ExtentId {
    ExtentId::new(BRANCH, sequence)
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
        store.write(ExtentId::new(BRANCH + 1, 1), 0, &[3; BLOCK]),
        OtherBranch(ExtentId::new(BRANCH + 1, 1)),
    );
    assert!(store.write(extent(2), 1, &[3; BLOCK]).is_err(), "unaligned");
    assert_eq!(read(&store, extent(1), 0, 2 * BLOCK), [[1; BLOCK], [2; BLOCK]].concat());
}

/// The store holds at most its capacity, and frees make room.
#[test]
fn capacity() {
    let store = create(MemDir::new(), 64 << 10);
    store.write(extent(1), 0, &[1; 48 << 10]).unwrap();
    error_is(store.write(extent(2), 0, &[2; 32 << 10]), NoSpace);
    assert_eq!(store.info().unwrap().available, 16 << 10);
    store.free(extent(1)).unwrap();
    store.free(extent(1)).expect("freeing a freed extent failed");
    store.write(extent(2), 0, &[2; 32 << 10]).unwrap();
    // Freeing doesn't make sequence numbers available again.
    assert_eq!(store.info().unwrap().next_sequence, 3);
}

/// Freed extents' files, and segments whose extents are all freed, are removed once the frees are
/// committed.
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
    // The first segment, closed, and the next, open.
    let segments = || {
        let count = |kind| dir.list(&of(kind, BRANCH)).unwrap().len();
        (count("segments"), count("open-segments"))
    };
    assert_eq!(segments(), (1, 1));
    assert_eq!(dir.list(&of("extents", BRANCH)).unwrap(), vec!["0000000000000010"]);
    assert_eq!(dir.list(&of("open-extents", BRANCH)).unwrap(), Vec::<String>::new());

    for sequence in 0..=16 {
        store.free(extent(sequence)).unwrap();
    }
    // Not until the frees are committed.
    assert_eq!(segments(), (1, 1));
    assert_eq!(dir.list(&of("extents", BRANCH)).unwrap().len(), 1);
    store.commit(None).unwrap();
    assert_eq!(segments(), (0, 1));
    assert_eq!(dir.list(&of("extents", BRANCH)).unwrap(), Vec::<String>::new());
    error_is(store.read(extent(16), 0, &mut [0; BLOCK]), NoExtent(extent(16)));
}

/// fxdax: a commit that fills the open segment closes it, and leaves its framing to a thread of
/// its own: its extents read from its raw file meanwhile, and the flush of the commit waits for
/// its framed file, which holds them from then on, the uploader takes, and its raw file goes for
/// (TASK-41).
#[test]
fn segments_close_framed_by_the_commits_flush() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    assert!(store.upload(WRITER, 0).unwrap());
    for sequence in 0..17 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
    }
    store.commit_ordered(None).unwrap();
    let count = |kind| dir.list(&of(kind, BRANCH)).unwrap().len();
    // Closed, and the next open: its raw file stays until the commit is durable.
    assert_eq!(count("open-segments"), 2);
    let reads = || (0..17).map(|sequence| read(&store, extent(sequence), 0, BLOCK));
    let written: Vec<_> = (0..17).map(|sequence| pattern(sequence as u8, 0, BLOCK)).collect();
    assert_eq!(reads().collect::<Vec<_>>(), written);
    assert!(store.closed().unwrap().is_empty());

    store.flush().unwrap();
    assert_eq!((count("segments"), count("open-segments")), (1, 1));
    assert_eq!(reads().collect::<Vec<_>>(), written);
    let closed = store.closed().unwrap();
    let [segment] = &closed[..] else { panic!("{} segments closed", closed.len()) };
    let data = extent_frame::data(&segment.bytes().unwrap()).unwrap();
    assert_eq!(&data[..BLOCK], &pattern(0, 0, BLOCK)[..]);
}

/// The one segment `store` closed whose commits are durable, which [`Store::closed`] gives.
fn closed_one(store: &Store<MemDir>) -> Closed<MemFile> {
    let closed = store.closed().unwrap();
    let count = closed.len();
    let [closed] = <[_; 1]>::try_from(closed).unwrap_or_else(|_| panic!("{count} segments closed"));
    closed
}

/// fxdax: an uploader takes the segments closed once their commits are durable, whose footers say
/// what the commits wrote, with its lengths, freed, and set as the root; and reads the data of
/// extents freed since, whose files stay until it releases them.
#[test]
fn uploads_keep_freed_data_until_released() {
    use extent_frame::segment::{Location, extent_key};
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    store.commit(None).unwrap();
    // Before uploading began: the uploader starts from the store's extents.
    let snapshot = store.snapshot().unwrap();
    assert_eq!((snapshot.epoch, snapshot.extents), (1, vec![(extent(1), BLOCK as u64)]));
    assert!(!store.upload(WRITER, 0).unwrap());
    assert!(store.upload(WRITER, 1).unwrap());
    assert!(store.closed().unwrap().is_empty());

    for sequence in 2..=16 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
    }
    store.write(extent(17), 0, &pattern(17, 0, 32 << 10)).unwrap();
    // Filling its segment, which it closes; not taken until durable.
    store.commit_ordered(Some(b"root")).unwrap();
    assert!(store.closed().unwrap().is_empty());
    store.flush().unwrap();
    let writing = closed_one(&store);
    let written: Vec<_> =
        writing.footer.extents.iter().map(|&(extent, _, len)| (extent, len)).collect();
    // The segment opened before uploading began holds what the commit before it wrote too.
    let blocks: Vec<_> = (1..=16).map(|sequence| (extent(sequence), BLOCK as u64)).collect();
    assert_eq!(written, blocks);
    let own =
        Location { key: extent_key(1, writing.footer.seq, extent(17)), offset: 0, len: 32 << 10 };
    assert_eq!(writing.footer.own, [(extent(17), own)]);
    let (frees, root) = (&writing.footer.frees, &writing.footer.root);
    assert_eq!((frees.clone(), root.clone()), (vec![], Some(b"root".to_vec())));
    assert_eq!(store.unreleased_bytes().unwrap(), 15 * BLOCK as u64 + (32 << 10));

    for sequence in 1..=17 {
        store.free(extent(sequence)).unwrap();
    }
    store.commit(None).unwrap();
    assert!(store.close_now().unwrap());
    let freeing = closed_one(&store);
    assert_eq!(freeing.footer.frees, (1..=17).map(extent).collect::<Vec<_>>());
    // Freed, and kept: the uploader reads them, the store's users don't.
    error_is(store.read(extent(17), 0, &mut [0; BLOCK]), NoExtent(extent(17)));
    let mut data = vec![0; 32 << 10];
    store.read_kept(extent(17), 0, &mut data).unwrap();
    assert_eq!(data, pattern(17, 0, 32 << 10));
    store.read_kept(extent(3), 0, &mut data[..BLOCK]).unwrap();
    assert_eq!(&data[..BLOCK], pattern(3, 0, BLOCK));
    store.commit(None).unwrap();
    assert_eq!(dir.list(&of("extents", BRANCH)).unwrap().len(), 1);
    let segments = dir.list(&of("segments", BRANCH)).unwrap().len();

    // Released, and their segments uploaded, they go with the next commit.
    store.release(3).unwrap();
    for closed in [writing, freeing] {
        store.uploaded(closed.number).unwrap();
    }
    assert_eq!(store.unreleased_bytes().unwrap(), 0);
    error_is(store.read_kept(extent(17), 0, &mut data), NoExtent(extent(17)));
    store.commit(None).unwrap();
    assert_eq!(dir.list(&of("extents", BRANCH)).unwrap(), Vec::<String>::new());
    assert!(dir.list(&of("segments", BRANCH)).unwrap().len() < segments);
}

/// fxdax: a store holds extents whose data is elsewhere: committed and recovered like others, and
/// freed so, but read from elsewhere.
#[test]
fn adopted_extents_are_held_without_their_data() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    assert!(store.upload(WRITER, store.epoch().unwrap()).unwrap());
    store.adopt(extent(5), 3 * BLOCK as u64).unwrap();
    store.adopt(ExtentId::new(BRANCH + 1, 9), BLOCK as u64).unwrap();
    assert!(store.adopt(extent(5), BLOCK as u64).is_err());
    store.commit(None).unwrap();
    // Not uploaded again: a checkpoint names them, no footer.
    assert!(store.close_now().unwrap());
    let adopting = closed_one(&store);
    assert_eq!((adopting.footer.extents.len(), adopting.footer.own.len()), (0, 0));
    assert_eq!(store.unreleased_bytes().unwrap(), 0);
    let check = |store: &Store<MemDir>| {
        assert_eq!(store.extent(extent(5)).unwrap(), Some((3 * BLOCK as u64, false)));
        error_is(store.read(extent(5), 0, &mut [0; BLOCK]), RemoteData(extent(5)));
        assert_eq!(store.info().unwrap().used, 4 * BLOCK as u64);
        assert_eq!(store.info().unwrap().next_sequence, 6);
        // Sealed.
        error_is(store.write(extent(5), 0, &[0; BLOCK]), SealedExtent(extent(5)));
    };
    check(&store);
    drop(store);
    let store = Store::open(dir.clone(), OPTIONS).unwrap();
    check(&store);
    assert!(store.upload(WRITER, store.epoch().unwrap()).unwrap());
    store.free(extent(5)).unwrap();
    store.commit(None).unwrap();
    assert!(store.close_now().unwrap());
    assert_eq!(closed_one(&store).footer.frees, [extent(5)]);
    error_is(store.read(extent(5), 0, &mut [0; BLOCK]), NoExtent(extent(5)));
}

/// Commits append deltas to the index, which a checkpoint in a new file replaces once they are
/// many.
#[test]
fn checkpoints() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 64 << 20);
    for sequence in 0..200 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
        if sequence % 3 == 0 {
            store.free(extent(sequence)).unwrap();
        }
        store.commit(Some(format!("root {sequence}").as_bytes())).unwrap();
    }
    assert_eq!(dir.list(&of("index", BRANCH)).unwrap().len(), 1);
    let first_index = dir.list(&of("index", BRANCH)).unwrap();
    drop(store);
    let store = Store::open(dir.clone(), OPTIONS).unwrap();
    assert_ne!(dir.list(&of("index", BRANCH)).unwrap(), first_index);
    assert_eq!(dir.list(&of("index", BRANCH)).unwrap().len(), 1);
    assert_eq!(store.root().unwrap(), b"root 199");
    assert_eq!(store.epoch().unwrap(), 200);
    let held: Vec<u64> =
        store.extents().unwrap().iter().map(|(extent, _)| extent.sequence).collect();
    assert_eq!(held, (0..200).filter(|sequence| sequence % 3 != 0).collect::<Vec<_>>());
    for sequence in held {
        assert_eq!(read(&store, extent(sequence), 0, BLOCK), pattern(sequence as u8, 0, BLOCK));
    }
}

/// Cleaning copies the extents of segments mostly freed to the open segment, and removes them.
fn cleaning<D: Dir>(dir: D, reopen: impl Fn(&D) -> D) {
    let store = create(dir, 64 << 20);
    // 8 segments of 16 extents.
    for sequence in 0..128 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
        if sequence % 16 == 15 {
            store.commit(None).unwrap();
        }
    }
    let full = store.usage().unwrap();
    assert_eq!((full.segments, full.segment_bytes), (9, 128 * BLOCK as u64));
    // Keep one extent in four in the first four segments, and every other in the rest.
    let kept: Vec<u64> = (0..128)
        .filter(|sequence| if *sequence < 64 { sequence % 4 == 0 } else { sequence % 2 == 0 })
        .collect();
    for sequence in (0..128).filter(|sequence| !kept.contains(sequence)) {
        store.free(extent(sequence)).unwrap();
    }
    store.commit(None).unwrap();
    assert_eq!(store.usage().unwrap().live_bytes, kept.len() as u64 * BLOCK as u64);

    // The first four segments are a quarter live, and the others half.
    let cleaned = store.clean(0.4).unwrap();
    assert_eq!(cleaned.segments, 4);
    assert_eq!(cleaned.bytes, 64 * BLOCK as u64);
    assert_eq!(cleaned.copied, 16 * BLOCK as u64);
    let usage = store.usage().unwrap();
    assert_eq!(usage.segment_bytes, full.segment_bytes - cleaned.bytes + cleaned.copied);
    assert_eq!(usage.live_bytes, kept.len() as u64 * BLOCK as u64);

    let check = |store: &Store<D>| {
        let held: Vec<u64> =
            store.extents().unwrap().iter().map(|(extent, _)| extent.sequence).collect();
        assert_eq!(held, kept);
        for sequence in &kept {
            assert_eq!(
                read(store, extent(*sequence), 0, BLOCK),
                pattern(*sequence as u8, 0, BLOCK)
            );
        }
    };
    check(&store);
    let dir = reopen(store.dir());
    drop(store);
    let store = Store::open(dir, OPTIONS).unwrap();
    check(&store);
    assert_eq!(store.usage().unwrap().segment_bytes, usage.segment_bytes);
}

/// fxdax: cleaning behind the commits copies, and commits nothing: the next commit makes the
/// copies their extents' places, which removes the segments cleaned, once durable (TASK-79.2). A
/// pass copies so many bytes at most, those of the least live segments first, and none whose
/// copies wait for a commit; and no pass is due until a commit has made those.
#[test]
fn cleaning_behind_the_commits() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 64 << 20);
    // 8 segments of 16 extents: of the first four, one extent in four kept, of the rest half.
    for sequence in 0..128 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
        if sequence % 16 == 15 {
            store.commit(None).unwrap();
        }
    }
    let kept: Vec<u64> = (0..128)
        .filter(|sequence| if *sequence < 64 { sequence % 4 == 0 } else { sequence % 2 == 0 })
        .collect();
    for sequence in (0..128).filter(|sequence| !kept.contains(sequence)) {
        store.free(extent(sequence)).unwrap();
    }
    store.commit(None).unwrap();
    let check = |store: &Store<MemDir>| {
        for sequence in &kept {
            let data = read(store, extent(*sequence), 0, BLOCK);
            assert!(data == pattern(*sequence as u8, 0, BLOCK), "{sequence}");
        }
    };
    let epoch = store.epoch().unwrap();
    assert!(store.cleaning_due(64 * BLOCK as u64).unwrap());
    assert!(!store.cleaning_due(96 * BLOCK as u64).unwrap());
    // Those of one of the four a quarter live.
    assert_eq!(store.clean_behind(0.4, 4 * BLOCK as u64).unwrap(), 4 * BLOCK as u64);
    assert_eq!(store.epoch().unwrap(), epoch);
    assert!(!store.cleaning_due(0).unwrap());
    check(&store);
    store.commit(None).unwrap();
    assert_eq!(store.usage().unwrap().segments, 8);
    // Those of the others, once.
    assert_eq!(store.clean_behind(0.4, u64::MAX).unwrap(), 12 * BLOCK as u64);
    assert_eq!(store.clean_behind(0.4, u64::MAX).unwrap(), 0);
    store.commit(None).unwrap();
    let usage = store.usage().unwrap();
    // The four half live, the one the copies filled, which the commit closed, and the open one.
    assert_eq!((usage.segments, usage.segment_bytes), (6, (128 - 64 + 16) * BLOCK as u64));
    assert_eq!(usage.live_bytes, kept.len() as u64 * BLOCK as u64);
    check(&store);
    check(&reopened(store, &dir));
}

#[test]
fn cleaning_in_memory() {
    cleaning(MemDir::new(), MemDir::clone);
}

#[test]
fn cleaning_on_the_host() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().to_owned();
    cleaning(HostDir::new(&path, true).unwrap(), |dir| HostDir::new(dir.path(), true).unwrap());
}

/// Every file in `dir`, with its size: the store's description, branches' records, and each
/// branch's segments, extents' files and index files.
fn files(dir: &MemDir) -> std::collections::BTreeMap<String, u64> {
    let mut files = std::collections::BTreeMap::new();
    let mut add = |path: String| {
        let size = extent_store::File::size(&dir.open(&path, false).unwrap()).unwrap();
        files.insert(path, size);
    };
    let kinds = ["segments", "extents", "index", "open-segments", "open-extents"];
    let dirs = [&["branches"][..], &kinds].concat();
    for name in dir.list("").unwrap() {
        if !dirs.contains(&name.as_str()) {
            add(name);
        }
    }
    for name in dir.list("branches").unwrap() {
        add(format!("branches/{name}"));
    }
    for kind in kinds {
        for branch in dir.list(kind).unwrap_or_default() {
            for name in dir.list(&format!("{kind}/{branch}")).unwrap() {
                add(format!("{kind}/{branch}/{name}"));
            }
        }
    }
    files
}

/// A store of `count` small extents and `count / 8` large ones, committed.
fn holding(count: u64) -> (MemDir, Store<MemDir>) {
    let dir = MemDir::new();
    let store = create(dir.clone(), 64 << 20);
    for sequence in 0..count {
        let len = if sequence % 8 == 7 { 32 << 10 } else { BLOCK };
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, len)).unwrap();
    }
    store.commit(None).unwrap();
    (dir, store)
}

/// A fork writes its branch's record, whatever the store holds: no extent's data, nor its index.
#[test]
fn forks_take_constant_time_and_space() {
    let mut added = Vec::new();
    for count in [16, 1024] {
        let (dir, store) = holding(count);
        // What the parent holds in segments of its, closed.
        store.close_segment().unwrap();
        store.commit(None).unwrap();
        let before = files(&dir);
        store.fork(BRANCH + 1).unwrap();
        let after = files(&dir);
        let mut new: Vec<&String> =
            after.keys().filter(|path| !before.contains_key(*path)).collect();
        // The fork's record, the segment it closes, at whose end it is, framed, and the one after
        // it, open, raw; the open one's raw file goes.
        let open = before.keys().rfind(|path| path.starts_with("open-segments/")).unwrap();
        let number = u64::from_str_radix(open.rsplit('/').next().unwrap(), 16).unwrap();
        let closed = format!("segments/{BRANCH:016x}/{number:016x}");
        let next = format!("open-segments/{BRANCH:016x}/{:016x}", number + 1);
        new.sort();
        assert_eq!(
            new,
            ["branches/0000000000000008.0000000000000000", next.as_str(), closed.as_str()]
        );
        assert!(!after.contains_key(open));
        // And the fork's commit, which seals nothing, in the parent's index.
        let grown: Vec<(&String, u64)> = (before.iter())
            .filter(|(path, size)| after.get(*path).is_some_and(|after| after != *size))
            .map(|(path, size)| (path, after[path] - size))
            .collect();
        added.push((grown.len(), grown.iter().map(|(_, bytes)| bytes).sum::<u64>()));
    }
    assert_eq!(added[0], added[1]);
}

/// A fork the host crashed while making: its record created, its contents not yet durable, as a
/// crash leaves a file a journal committed the creation of and nothing else. The fork never
/// happened, as [`Store::fork`] never returned: the store opens without it, and may make it again.
/// (Found by tests/shuttle/main.rs, TASK-44.4: the store refused to open.)
#[test]
fn a_fork_whose_record_never_became_whole_never_happened() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    store.commit(None).unwrap();
    drop(store);
    let child = BRANCH + 1;
    let record = format!("branches/{child:016x}.{:016x}", 0);
    dir.create(&record, false).unwrap().sync().unwrap();
    dir.sync_dir("branches").unwrap();

    let store = Store::open(dir.clone(), OPTIONS).unwrap();
    assert_eq!(read(&store, extent(1), 0, BLOCK), pattern(1, 0, BLOCK));
    assert!(!dir.list("branches").unwrap().contains(&record.replace("branches/", "")));
    store.fork(child).unwrap();
    let forked = store.lineage().open_branch(child).unwrap();
    assert_eq!(read(&forked, extent(1), 0, BLOCK), pattern(1, 0, BLOCK));
}

/// A fork seals its parent's open extents: the parent writes them no more, and the fork reads
/// them.
#[test]
fn forks_seal_their_parents_open_extents() {
    let store = create(MemDir::new(), 1 << 20);
    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    store.fork(BRANCH + 1).unwrap();
    error_is(store.write(extent(1), BLOCK as u64, &[0; BLOCK]), SealedExtent(extent(1)));
    let child = store.lineage().open_branch(BRANCH + 1).unwrap();
    assert_eq!(read(&child, extent(1), 0, BLOCK), pattern(1, 0, BLOCK));
    assert_eq!(child.extents().unwrap(), vec![(extent(1), BLOCK as u64)]);
}

/// A parent and its fork read every extent the parent held at the fork, and free them each on
/// its own; an extent both free goes. So it is whether the fork is opened before the parent frees
/// them or after, and across reopening the store.
fn forks_free_on_their_own(open_child_first: bool) {
    let child_branch = BRANCH + 1;
    let (dir, store) = holding(48);
    // 3 segments' worth, and 6 extents of their own.
    let all: Vec<u64> = (0..48).collect();
    let len = |sequence: u64| if sequence % 8 == 7 { 32 << 10 } else { BLOCK };
    store.fork(child_branch).unwrap();
    let lineage = store.lineage().clone();
    let child = open_child_first.then(|| lineage.open_branch(child_branch).unwrap());

    // The parent frees the even extents, and writes one of its own.
    for sequence in all.iter().filter(|sequence| *sequence % 2 == 0) {
        store.free(extent(*sequence)).unwrap();
    }
    store.write(extent(100), 0, &pattern(100, 0, BLOCK)).unwrap();
    store.commit(None).unwrap();
    let child = child.unwrap_or_else(|| lineage.open_branch(child_branch).unwrap());
    let check_inherited = |child: &Store<MemDir>, held: &[u64]| {
        let extents: Vec<u64> =
            child.extents().unwrap().iter().map(|(extent, _)| extent.sequence).collect();
        assert_eq!(extents, held);
        for &sequence in held {
            assert_eq!(
                read(child, extent(sequence), 0, len(sequence)),
                pattern(sequence as u8, 0, len(sequence))
            );
        }
    };
    check_inherited(&child, &all);
    error_is(child.read(extent(100), 0, &mut [0; BLOCK]), NoExtent(extent(100)));

    // The child frees the extents divisible by three, and writes one of its own.
    for sequence in all.iter().filter(|sequence| *sequence % 3 == 0) {
        child.free(extent(*sequence)).unwrap();
    }
    child.write(ExtentId::new(child_branch, 0), 0, &pattern(200, 0, BLOCK)).unwrap();
    child.commit(None).unwrap();
    let parent_held: Vec<u64> =
        all.iter().copied().filter(|sequence| sequence % 2 == 1).chain([100]).collect();
    let child_held: Vec<u64> = all.iter().copied().filter(|sequence| sequence % 3 != 0).collect();
    let check_parent = |store: &Store<MemDir>| {
        let extents: Vec<u64> =
            store.extents().unwrap().iter().map(|(extent, _)| extent.sequence).collect();
        assert_eq!(extents, parent_held);
        for &sequence in &parent_held {
            let len = if sequence == 100 { BLOCK } else { len(sequence) };
            assert_eq!(read(store, extent(sequence), 0, len), pattern(sequence as u8, 0, len));
        }
    };
    check_parent(&store);
    let check_child = |child: &Store<MemDir>| {
        let extents: Vec<ExtentId> =
            child.extents().unwrap().iter().map(|(extent, _)| *extent).collect();
        // Its own extents' IDs, of its own branch, sort after its parent's.
        let mut expected: Vec<ExtentId> =
            child_held.iter().map(|sequence| extent(*sequence)).collect();
        expected.push(ExtentId::new(child_branch, 0));
        expected.sort();
        assert_eq!(extents, expected);
        for &sequence in &child_held {
            assert_eq!(
                read(child, extent(sequence), 0, len(sequence)),
                pattern(sequence as u8, 0, len(sequence))
            );
        }
    };
    check_child(&child);

    // The extents of their own both freed are gone; those either holds stay.
    let own_files: Vec<String> = dir.list(&of("extents", BRANCH)).unwrap();
    let expected: Vec<String> = (all.iter())
        .filter(|sequence| *sequence % 8 == 7 && (*sequence % 2 == 1 || *sequence % 3 != 0))
        .map(|sequence| format!("{sequence:016x}"))
        .collect();
    assert_eq!(own_files, expected);

    // And so after reopening the store.
    drop((store, child, lineage));
    let lineage = Lineage::open(dir.clone(), OPTIONS).unwrap();
    assert_eq!(lineage.branches(), vec![(BRANCH, None), (child_branch, Some(BRANCH))]);
    check_parent(&lineage.open_branch(BRANCH).unwrap());
    check_child(&lineage.open_branch(child_branch).unwrap());
}

#[test]
fn forks_opened_at_once_free_on_their_own() {
    forks_free_on_their_own(true);
}

#[test]
fn forks_opened_later_free_on_their_own() {
    forks_free_on_their_own(false);
}

/// A fork's parent keeps what the fork needs, across reopening the store, and cleaning: and its
/// index file as of the fork, which the fork's is over, while the fork lives (TASK-31).
#[test]
fn forks_not_yet_opened_keep_what_they_inherited() {
    let child_branch = BRANCH + 1;
    let (dir, store) = holding(64);
    store.fork(child_branch).unwrap();
    // The parent lets every extent go, and cleans, and checkpoints its index anew.
    for sequence in 0..64 {
        store.free(extent(sequence)).unwrap();
    }
    for _ in 0..8 {
        store.commit(Some(b"later")).unwrap();
    }
    store.clean(0.5).unwrap();
    assert_eq!(store.extents().unwrap(), vec![]);
    drop(store);
    let lineage = Lineage::open(dir.clone(), OPTIONS).unwrap();
    let child = lineage.open_branch(child_branch).unwrap();
    assert_eq!(child.extents().unwrap().len(), 64);
    for sequence in 0..64 {
        let len = if sequence % 8 == 7 { 32 << 10 } else { BLOCK };
        assert_eq!(read(&child, extent(sequence), 0, len), pattern(sequence as u8, 0, len));
    }
    assert_eq!(child.root().unwrap(), b"");
    // Opened, its parent keeps its index as of the fork; deleted, that goes.
    drop(child);
    let parent = lineage.open_branch(BRANCH).unwrap();
    assert_eq!(parent.root().unwrap(), b"later");
    assert_eq!(dir.list(&of("index", BRANCH)).unwrap().len(), 2);
    drop(parent);
    lineage.delete(child_branch).unwrap();
    let parent = lineage.open_branch(BRANCH).unwrap();
    assert_eq!(parent.root().unwrap(), b"later");
    assert_eq!(dir.list(&of("index", BRANCH)).unwrap().len(), 1);
}

/// A fork's first opening reads and writes the same bytes whatever its parent holds: it opens over
/// its parent's state at the fork, which the store holds, and its index holds what it changed over
/// that, nothing yet. So does its opening after (TASK-31).
#[test]
fn forks_open_in_the_same_bytes_whatever_their_parent_holds() {
    let mut io = Vec::new();
    // As many digits each, which the fork's record holds.
    for count in [200, 800] {
        let (dir, store) = holding(count);
        store.fork(BRANCH + 1).unwrap();
        let lineage = store.lineage().clone();
        let before = dir.io();
        let child = lineage.open_branch(BRANCH + 1).unwrap();
        let first = dir.io();
        assert_eq!(child.extents().unwrap().len() as u64, count);
        drop(child);
        let before_again = dir.io();
        let child = lineage.open_branch(BRANCH + 1).unwrap();
        let again = dir.io();
        assert_eq!(child.extents().unwrap().len() as u64, count);
        io.push((
            (first.read - before.read, first.written - before.written),
            (again.read - before_again.read, again.written - before_again.written),
        ));
    }
    assert_eq!(io[0], io[1]);
    assert_eq!(io[0].0.0, 0, "the first opening read {io:?}");
}

/// Forks of a branch opened in one process share its state at the fork, one copy, as the branch
/// does, going on over it: each holds in memory what it changed over it, whatever the branch holds
/// (TASK-31).
#[test]
fn forks_share_their_parents_state() {
    for count in [200, 800] {
        let (_dir, store) = holding(count);
        let lineage = store.lineage().clone();
        let children: Vec<Store<MemDir>> = (1..=3)
            .map(|n| {
                store.fork(BRANCH + n).unwrap();
                lineage.open_branch(BRANCH + n).unwrap()
            })
            .collect();
        assert_eq!(store.footprint().unwrap().entries, 0);
        for child in &children {
            // Its open segment, and nothing of what it inherited.
            assert_eq!(child.footprint().unwrap(), Footprint { entries: 0, segments: 1 });
            assert_eq!(child.extents().unwrap().len() as u64, count);
        }
        // An extent written, and one inherited freed, whose segment it holds otherwise from then
        // on.
        let child = &children[0];
        child.write(ExtentId::new(BRANCH + 1, 0), 0, &pattern(9, 0, BLOCK)).unwrap();
        child.free(extent(0)).unwrap();
        child.commit(None).unwrap();
        assert_eq!(child.footprint().unwrap(), Footprint { entries: 2, segments: 2 });
        assert_eq!(read(&children[1], extent(0), 0, BLOCK), pattern(0, 0, BLOCK));
        error_is(child.read(extent(0), 0, &mut [0; BLOCK]), NoExtent(extent(0)));
    }
}

/// Opening a store with forks of its first branch reads, per fork, the same bytes whatever the
/// branch holds: the branch's index once, for all of them, and each fork's own (TASK-31).
#[test]
fn stores_open_their_forks_in_bytes_independent_of_their_base() {
    let options = Options { checkpoint_after: 16 << 20, ..OPTIONS };
    let mut per_fork = Vec::new();
    // As many digits each, which the forks' records hold.
    for count in [200, 800] {
        let mut read = Vec::new();
        for forks in [50, 100] {
            let dir = MemDir::new();
            let description =
                Description { block_size: BLOCK as u32, capacity: 64 << 20, branch: BRANCH };
            let store = Store::create(dir.clone(), description, options).unwrap();
            for sequence in 0..count {
                let len = if sequence % 8 == 7 { 32 << 10 } else { BLOCK };
                store.write(extent(sequence), 0, &pattern(sequence as u8, 0, len)).unwrap();
            }
            store.commit(None).unwrap();
            for n in 1..=forks {
                store.fork(BRANCH + n).unwrap();
            }
            // Opened once each, as forks run.
            let lineage = store.lineage().clone();
            for n in 1..=forks {
                drop(lineage.open_branch(BRANCH + n).unwrap());
            }
            drop((store, lineage));

            let before = dir.io();
            let lineage = Lineage::open(dir.clone(), options).unwrap();
            for n in 1..=forks {
                let child = lineage.open_branch(BRANCH + n).unwrap();
                assert_eq!(child.extents().unwrap().len() as u64, count);
            }
            read.push(dir.io().read - before.read);
        }
        // What 50 more forks read.
        per_fork.push(read[1] - read[0]);
    }
    assert_eq!(per_fork[0], per_fork[1]);
}

/// A deleted branch's extents go, but for those another branch holds.
#[test]
fn deleting_a_branch_lets_its_extents_go() {
    let child_branch = BRANCH + 1;
    let (dir, store) = holding(16);
    store.fork(child_branch).unwrap();
    let lineage = store.lineage().clone();
    let child = lineage.open_branch(child_branch).unwrap();
    child.write(ExtentId::new(child_branch, 0), 0, &pattern(1, 0, 32 << 10)).unwrap();
    child.commit(None).unwrap();
    assert_eq!(dir.list(&of("extents", child_branch)).unwrap().len(), 1);
    assert!(lineage.delete(child_branch).is_err(), "deleted an open branch");
    drop(child);
    lineage.delete(child_branch).unwrap();
    assert_eq!(lineage.branches(), vec![(BRANCH, None)]);
    assert_eq!(dir.list(&of("extents", child_branch)).unwrap(), Vec::<String>::new());
    assert_eq!(dir.list(&of("index", child_branch)).unwrap(), Vec::<String>::new());
    // The parent's extents stay; freed, they go.
    let own = vec!["0000000000000007", "000000000000000f"];
    assert_eq!(dir.list(&of("extents", BRANCH)).unwrap(), own);
    store.free(extent(7)).unwrap();
    store.free(extent(15)).unwrap();
    store.commit(None).unwrap();
    assert_eq!(dir.list(&of("extents", BRANCH)).unwrap(), Vec::<String>::new());
    assert!(lineage.open_branch(child_branch).is_err());
}

/// Deleting the store, its first branch, while a fork of it lives (TASK-39.4): its own extents go,
/// what the fork holds of it stays, and the fork reads, writes and forks as before, across
/// reopening the store, until it lets that go too.
#[test]
fn deleting_a_store_keeps_what_its_forks_hold() {
    let (child_branch, grandchild) = (BRANCH + 1, BRANCH + 2);
    let large = |sequence: u64| pattern(sequence as u8, 0, 32 << 10);
    let (dir, store) = holding(16);
    store.fork(child_branch).unwrap();
    // The store's own, after the fork.
    store.write(extent(16), 0, &large(16)).unwrap();
    store.commit(None).unwrap();
    let lineage = store.lineage().clone();
    assert!(lineage.delete(BRANCH).is_err(), "deleted an open branch");
    drop(store);
    lineage.delete(BRANCH).unwrap();
    assert!(lineage.is_deleted());
    assert_eq!(lineage.branches(), vec![(child_branch, Some(BRANCH))]);
    let inherited = vec!["0000000000000007", "000000000000000f"];
    assert_eq!(dir.list(&of("extents", BRANCH)).unwrap(), inherited);
    let error = lineage.open_branch(BRANCH).err().expect("opened a deleted store");
    assert_eq!(
        error.to_string(),
        "the store was deleted: its directory holds its forks' data alone"
    );
    let error = lineage.delete(BRANCH).expect_err("deleted a store twice");
    assert_eq!(
        error.to_string(),
        "the store was deleted: its directory holds its forks' data alone"
    );

    let child = lineage.open_branch(child_branch).unwrap();
    for sequence in 0..16 {
        let len = if sequence % 8 == 7 { 32 << 10 } else { BLOCK };
        assert_eq!(read(&child, extent(sequence), 0, len), pattern(sequence as u8, 0, len));
    }
    let own = ExtentId::new(child_branch, 0);
    child.write(own, 0, &large(99)).unwrap();
    child.commit(None).unwrap();
    child.fork(grandchild).unwrap();
    drop(child);

    // Across reopening the store.
    drop(lineage);
    let lineage = Lineage::open(dir.clone(), OPTIONS).unwrap();
    assert!(lineage.is_deleted());
    assert_eq!(
        lineage.branches(),
        vec![(child_branch, Some(BRANCH)), (grandchild, Some(child_branch))]
    );
    let child = lineage.open_branch(child_branch).unwrap();
    assert_eq!(read(&child, extent(7), 0, 32 << 10), large(7));
    assert_eq!(read(&child, own, 0, 32 << 10), large(99));
    // The fork lets the store's extents go; its fork, not yet opened, holds them still.
    for sequence in 0..16 {
        child.free(extent(sequence)).unwrap();
    }
    child.commit(None).unwrap();
    drop(child);
    assert_eq!(dir.list(&of("extents", BRANCH)).unwrap(), inherited);
    let grandchild_store = lineage.open_branch(grandchild).unwrap();
    assert_eq!(read(&grandchild_store, extent(15), 0, 32 << 10), large(15));
    drop(grandchild_store);
    // Deleted, nothing holds them: the store's files go.
    lineage.delete(grandchild).unwrap();
    assert_eq!(dir.list(&of("extents", BRANCH)).unwrap(), Vec::<String>::new());
    assert_eq!(dir.list(&of("segments", BRANCH)).unwrap(), Vec::<String>::new());
    assert_eq!(lineage.branches(), vec![(child_branch, Some(BRANCH))]);
}

/// The branch's state, as it holds it, which rebuilding it from its checkpoint and its closed
/// segments' footers must give once its open segment is closed.
fn closed_state<D: Dir>(store: &Store<D>) -> extent_store::Rebuilt {
    store.close_segment().unwrap();
    store.commit(None).unwrap();
    extent_store::Rebuilt {
        epoch: store.epoch().unwrap(),
        root: store.root().unwrap(),
        next_sequence: store.info().unwrap().next_sequence,
        extents: store.places().unwrap(),
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
        assert_eq!((*branch, offset % BLOCK as u64), (BRANCH, 0), "{extent:?} at {offset}");
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
/// data; and the branch's state is its checkpoint and its closed segments' footers.
#[test]
fn closed_segments_hold_the_branchs_history() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 64 << 20);
    // Closed, and open.
    let segments = || {
        dir.list(&of("segments", BRANCH)).unwrap().len()
            + dir.list(&of("open-segments", BRANCH)).unwrap().len()
    };
    store.write(extent(0), 0, &pattern(0, 0, BLOCK)).unwrap();
    store.commit(Some(b"one")).unwrap();
    assert_eq!(segments(), 1);
    // Asked to.
    let state = closed_state(&store);
    assert_eq!(segments(), 2);
    assert_eq!(store.rebuild().unwrap(), state);

    // Full: 64 KiB of extents, freed and kept, moved by cleaning, adopted.
    for sequence in 1..=40 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
        if sequence % 3 == 0 {
            store.free(extent(sequence)).unwrap();
        }
        store.commit(Some(format!("root {sequence}").as_bytes())).unwrap();
    }
    assert!(segments() > 2, "no segment filled");
    store.free(extent(0)).unwrap();
    store.adopt(ExtentId::new(BRANCH + 9, 3), BLOCK as u64).unwrap();
    store.commit(None).unwrap();
    store.clean(0.9).unwrap();
    let state = closed_state(&store);
    assert_eq!(store.rebuild().unwrap(), state);
    // A run's data is in its segment, framed, where its footer says.
    let (_, _, place) = state.extents.iter().find(|(id, ..)| *id == extent(40)).unwrap();
    let Place::Run { number, offset, .. } = *place else { panic!("{place:?}") };
    let file = dir.open(&format!("{}/{number:016x}", of("segments", BRANCH)), true).unwrap();
    let mut bytes = vec![0; extent_store::File::size(&file).unwrap() as usize];
    extent_store::File::read_at(&file, &mut bytes, 0).unwrap();
    let data = extent_frame::data(&bytes).unwrap();
    assert_eq!(data[offset as usize..][..BLOCK], pattern(40, 0, BLOCK));

    // Old.
    drop(store);
    let options = Options { segment_age: std::time::Duration::ZERO, ..OPTIONS };
    let store = Store::open(dir.clone(), options).unwrap();
    let before = segments();
    store.write(extent(50), 0, &pattern(50, 0, BLOCK)).unwrap();
    store.commit(None).unwrap();
    assert_eq!(segments(), before + 1);
    assert_eq!(store.rebuild().unwrap().extents, store.places().unwrap());

    // At a fork.
    store.write(extent(51), 0, &pattern(51, 0, BLOCK)).unwrap();
    store.fork(BRANCH + 1).unwrap();
    assert_eq!(store.rebuild().unwrap().extents, store.places().unwrap());
}

/// A branch rebuilt from a checkpoint the index wrote since, and the footers after it, is the
/// branch; footers of closed segments no extent is in go once a checkpoint covers them.
#[test]
fn checkpoints_cover_closed_segments() {
    let dir = MemDir::new();
    let options = Options { checkpoint_after: 1 << 10, ..OPTIONS };
    let store = Store::create(
        dir.clone(),
        Description { block_size: BLOCK as u32, capacity: 64 << 20, branch: BRANCH },
        options,
    )
    .unwrap();
    for round in 0..6 {
        for sequence in round * 20..(round + 1) * 20 {
            store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
            store.commit(None).unwrap();
        }
        // The rounds before last freed: their segments hold no extent.
        if round > 0 {
            for sequence in (round - 1) * 20..round * 20 {
                store.free(extent(sequence)).unwrap();
            }
        }
        let state = closed_state(&store);
        assert_eq!(store.rebuild().unwrap(), state, "round {round}");
    }
    // Only the last rounds' segments, and the open one, are left.
    assert!(dir.list(&of("segments", BRANCH)).unwrap().len() <= 4);
    drop(store);
    let store = Store::open(dir, options).unwrap();
    let state = closed_state(&store);
    assert_eq!(store.rebuild().unwrap(), state);
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
    // As of the checkpoint opening wrote, past the closed segment's footer.
    let rebuilt = store.rebuild().unwrap();
    assert!(rebuilt.epoch > state.epoch);
    assert_eq!((rebuilt.epoch, rebuilt.extents), (store.epoch().unwrap(), store.places().unwrap()));
    let state = closed_state(&store);
    assert_eq!(store.rebuild().unwrap(), state);
}

/// Bytes from a xorshift generator, which do not compress.
fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
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

/// A segment's raw file goes only once the commit closing it is durable: a crash before leaves the
/// branch as of the commits before, its data read from the raw file, which opening it closes again,
/// with no footer.
#[test]
fn segments_closed_by_commits_not_durable_close_again() {
    for seed in 0..16 {
        let dir = MemDir::new();
        let store = create(dir.clone(), 64 << 20);
        store.write(extent(1), 0, &pattern(1, 0, 3 * BLOCK)).unwrap();
        store.commit(Some(b"one")).unwrap();
        store.write(extent(2), 0, &pattern(2, 0, BLOCK)).unwrap();
        store.close_segment().unwrap();
        store.commit_ordered(Some(b"two")).unwrap();
        let crashed = dir.crash(seed);
        drop(store);
        let store = Store::open(crashed, OPTIONS).unwrap();
        assert_eq!(store.root().unwrap(), b"one", "seed {seed}");
        assert_eq!(read(&store, extent(1), 0, 3 * BLOCK), pattern(1, 0, 3 * BLOCK));
        assert_eq!(store.extents().unwrap(), vec![(extent(1), 3 * BLOCK as u64)]);
        let rebuilt = store.rebuild().unwrap();
        assert_eq!((rebuilt.root, rebuilt.extents), (b"one".to_vec(), store.places().unwrap()));
    }
}

/// fxdax: a segment closed between commits, the last of whose commits is not durable, is closed
/// again with no footer by the branch's next opening: which, crashing as it does, leaves its data
/// whole, for the opening after (TASK-79.33).
#[test]
fn segments_closed_between_commits_survive_crashes_closing_them_again() {
    for seed in 0..64 {
        let dir = MemDir::new();
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

/// fxdax: a segment closed between commits keeps its raw file until a commit after it is durable,
/// with the commits its footer names: so that the branch's next opening, a crash having lost them,
/// finds it, and closes it again, with no footer; and reads no segment whose raw file is gone
/// (TASK-92.16).
#[test]
fn segments_closed_between_commits_keep_their_raw_files_until_a_commit_after_is_durable() {
    let path = |kind| format!("{}/{:016x}", of(kind, BRANCH), 0);
    for seed in 0..16 {
        let dir = MemDir::new();
        let store = create(dir.clone(), 1 << 20);
        store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
        store.commit(None).unwrap();
        store.write(extent(2), 0, &pattern(2, 0, BLOCK)).unwrap();
        store.commit_ordered(None).unwrap();
        assert!(store.close_now().unwrap());
        let crashed = dir.crash(seed);
        assert!(dir.open(&path("open-segments"), true).is_ok(), "seed {seed}: the raw file went");
        store.commit(None).unwrap();
        assert!(dir.open(&path("open-segments"), true).is_err(), "seed {seed}: the raw file stays");
        drop(store);
        let store = Store::open(crashed.clone(), OPTIONS).unwrap();
        assert_eq!(store.epoch().unwrap(), 1, "seed {seed}");
        assert_eq!(read(&store, extent(1), 0, BLOCK), pattern(1, 0, BLOCK));
        let framed = extent_store::read_whole(&crashed.open(&path("segments"), true).unwrap());
        let footer = extent_frame::footer(&framed.unwrap()).unwrap().to_vec();
        assert_eq!(footer, b"", "seed {seed}: its footer names commits lost");
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
    let mut bytes = vec![0; (range.end - range.start) as usize];
    part.file.read_at(&mut bytes, range.start).unwrap();
    assert_eq!(store.unpack(part, &bytes).unwrap()[..16 * BLOCK], pattern(1, 0, 16 * BLOCK));
    let unpacked = store.read_packed(part).unwrap();
    assert_eq!(unpacked[..16 * BLOCK], pattern(1, 0, 16 * BLOCK));
    assert_eq!(kinds(extent(1)), ["unpacked"]);
    assert_eq!(kinds(extent(2)), ["unpacked"]);
    assert_eq!(read(&store, extent(2), 0, 16 * BLOCK), noise(2, 16 * BLOCK));
}

/// fxdax: given a writer's chain, the store closes its segments for the writer, as object storage
/// keeps them: numbered on from where the chain is, each following the one before, its commits
/// tagged with the writer's epoch, its extents of their own at the writer's keys; and keeps each,
/// once its commits are durable, for the uploader, until it is uploaded. The chain of another
/// branch's writer it refuses.
#[test]
fn segments_close_for_the_writer_of_their_chain() {
    use extent_frame::segment::{Footer, Location, extent_key, tag};
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    let refused = store.upload(Chain::new(BRANCH + 1, 3, 5, Some((4, 3)), 100), 0).unwrap_err();
    assert_eq!(refused.to_string(), "branch 0x7 uploaded by a writer of branch 0x8");
    assert!(store.upload(Chain::new(BRANCH, 3, 5, Some((4, 3)), 100), 0).unwrap());
    // Nothing committed since the last closing, nothing to close.
    assert!(!store.close_now().unwrap());

    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    store.write(extent(2), 0, &pattern(2, 0, 32 << 10)).unwrap();
    store.commit_ordered(Some(b"one")).unwrap();
    assert!(store.close_now().unwrap());
    // Not until its commit is durable.
    assert!(store.closed().unwrap().is_empty());
    store.flush().unwrap();
    let closed = store.closed().unwrap();
    let [first] = &closed[..] else { panic!("{} segments closed", closed.len()) };
    let own = Location { key: extent_key(3, 5, extent(2)), offset: 0, len: 32 << 10 };
    assert_eq!(
        first.footer,
        Footer {
            branch: BRANCH,
            seq: 5,
            epoch: 3,
            after: Some((4, 3)),
            commits: vec![tag(3, 1)],
            extents: vec![(extent(1), 0, BLOCK as u64)],
            own: vec![(extent(2), own.clone())],
            frees: vec![],
            root: Some(b"one".to_vec()),
            next_sequence: 100,
            forks: vec![],
        }
    );
    // Its file is the object, footer and all; its extent of its own's, the extent's object.
    let bytes = first.bytes().unwrap();
    let footer = Footer::decode(extent_frame::footer(&bytes).unwrap()).unwrap();
    assert_eq!(footer, first.footer);
    assert_eq!(extent_frame::data(&bytes).unwrap()[..BLOCK], pattern(1, 0, BLOCK));
    let [(key, own_bytes)] = &first.own_bytes().unwrap()[..] else { panic!("no extent's own") };
    assert_eq!((key, extent_frame::data(own_bytes).unwrap()), (&own.key, pattern(2, 0, 32 << 10)));

    // The next follows it; a fork's names the fork.
    store.write(extent(3), 0, &pattern(3, 0, BLOCK)).unwrap();
    store.fork(BRANCH + 1).unwrap();
    let closed = store.closed().unwrap();
    let [second] = &closed[..] else { panic!("{} segments closed", closed.len()) };
    assert_eq!((second.footer.seq, second.footer.after), (6, Some((5, 3))));
    assert_eq!(
        (second.footer.commits.clone(), second.footer.forks.clone()),
        (vec![tag(3, 2)], vec![BRANCH + 1])
    );

    // Freed, its extents' data goes once released, but the segment stays until uploaded.
    store.free(extent(1)).unwrap();
    store.commit(None).unwrap();
    store.release(3).unwrap();
    store.commit(None).unwrap();
    let held = store.usage().unwrap().segments;
    store.uploaded(first.number).unwrap();
    store.commit(None).unwrap();
    assert_eq!(store.usage().unwrap().segments, held - 1);
}

/// fxdax: a segment closed between commits holds what the commits made, as of the last: not
/// what was written or freed since, which the next segment holds.
#[test]
fn segments_closed_between_commits_hold_the_last_commits_state() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    assert!(store.upload(WRITER, 0).unwrap());
    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    store.write(extent(2), 0, &pattern(2, 0, BLOCK)).unwrap();
    store.commit(None).unwrap();
    // Since: one freed, another written.
    store.free(extent(1)).unwrap();
    store.write(extent(3), 0, &pattern(3, 0, BLOCK)).unwrap();
    assert!(store.close_now().unwrap());
    let closed = store.closed().unwrap();
    let [first] = &closed[..] else { panic!("{} segments closed", closed.len()) };
    let listed: Vec<_> = first.footer.extents.iter().map(|(extent, ..)| *extent).collect();
    assert_eq!((listed, first.footer.frees.clone()), (vec![extent(1), extent(2)], vec![]));
    store.commit(None).unwrap();
    assert!(store.close_now().unwrap());
    let closed = store.closed().unwrap();
    let [second] = &closed[..] else { panic!("{} segments closed", closed.len()) };
    let listed: Vec<_> = second.footer.extents.iter().map(|(extent, ..)| *extent).collect();
    assert_eq!((listed, second.footer.frees.clone()), (vec![extent(3)], vec![extent(1)]));
    assert_eq!(read(&store, extent(3), 0, BLOCK), pattern(3, 0, BLOCK));
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
    store.fork(BRANCH + 1).unwrap();
    let child = store.lineage().open_branch(BRANCH + 1).unwrap();
    assert_eq!(child.description().capacity, held);
    drop((child, store));
    let store = Store::open(dir, OPTIONS).unwrap();
    assert_eq!(store.info().unwrap().capacity, held);
    assert_eq!(read(&store, extent(2), 0, 96 << 10), pattern(2, 0, 96 << 10));
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
        open.push(open_fds());
    }
    assert_eq!(open[39], open[9], "{open:?}");
    assert_eq!(read(&store, extent(1), 0, BLOCK), pattern(1, 0, BLOCK));
    assert_eq!(read(&store, extent(40), 0, BLOCK), pattern(40, 0, BLOCK));
}

/// What a store's files did, by the directory each is in: their syncs that let their pages go
/// from the host's page cache, of those of extents' data, as a host's do, written through it
/// (`HostFile::sync`); and the bytes read.
#[derive(Default)]
struct Counts {
    drops: BTreeMap<String, u64>,
    read: BTreeMap<String, u64>,
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
}

impl extent_store::File for CountedFile {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
        *self.counts.lock().unwrap().read.entry(self.kind.clone()).or_default() += buf.len() as u64;
        self.file.read_at(buf, offset)
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
    assert_eq!(dir.drops(), BTreeMap::from([("segments".to_owned(), 1)]));
}

/// fxdax: a branch opening again reads of its closed segments but a block of each whose extents it
/// holds, its trailer and table, which say how much data it holds: not the footers of those whose
/// commits are durable, which it needs not close again, their raw files gone (TASK-92.16).
#[test]
fn branches_open_reading_a_block_of_each_closed_segment() {
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
    assert_eq!(segments_read(), 16 * BLOCK as u64);
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
    let path = |kind| temp.path().join(of(kind, BRANCH)).join(format!("{:016x}", 0));
    assert_eq!(cached_pages(&path("open-segments")), (8 * BLOCK).div_ceil(page_size()));
    store.close_segment().unwrap();
    store.commit(None).unwrap();
    assert_eq!(cached_pages(&path("segments")), 0);
}

/// Segments of 64 KiB and extents of 64 KiB at most, so that a segment holds 128 KiB at most.
const SMALL_SEGMENTS: Options = Options { max_extent: 64 << 10, ..OPTIONS };

/// The most bytes of data a segment of [`SMALL_SEGMENTS`] holds: a segment's, and an extent's.
const SEGMENT_LIMIT: u64 = 128 << 10;

/// The bytes of data of each of the branch's segments in `dir`: open ones' raw files, closed ones'
/// framed data.
fn segment_data(dir: &MemDir) -> Vec<u64> {
    let mut lens = Vec::new();
    for kind in ["open-segments", "segments"] {
        for name in dir.list(&of(kind, BRANCH)).unwrap() {
            let file = dir.open(&format!("{}/{name}", of(kind, BRANCH)), true).unwrap();
            let mut bytes = vec![0; file.size().unwrap() as usize];
            file.read_at(&mut bytes, 0).unwrap();
            let data = if kind == "segments" { extent_frame::data(&bytes).unwrap() } else { bytes };
            lens.push(data.len() as u64);
        }
    }
    lens
}

/// The open segment takes writes until it holds a segment and an extent, `Options::segment_size`
/// and `Options::max_extent`, between commits: then the next goes to another, which no segment
/// grows past, however much is written between commits (TASK-79.2). One holding no commit's data
/// is set aside raw, the extents in it copied out by the next commit, to the open segment while it
/// has room and to files of their own after; one holding commits' closes as it would between
/// commits ([`Store::close_now`]). Every extent reads as written meanwhile, and once committed,
/// and so after opening the store again.
#[test]
fn segments_hold_a_segment_and_an_extent_at_most() {
    let dir = MemDir::new();
    let description = Description { block_size: BLOCK as u32, capacity: 64 << 20, branch: BRANCH };
    let store = Store::create(dir.clone(), description, SMALL_SEGMENTS).unwrap();
    assert!(store.upload(WRITER, 0).unwrap());
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
    let own = dir.list(&of("extents", BRANCH)).unwrap().len();
    assert!(own > 0, "no extent sealed in a file of its own");

    // A commit's data in the open segment, then another 192 KiB of blocks: the segment holding it
    // closes as they come.
    store.write(extent(96), 0, &block(96)).unwrap();
    store.commit(None).unwrap();
    for sequence in 48..96 {
        store.write(extent(sequence), 0, &block(sequence)).unwrap();
        let lens = segment_data(&dir);
        assert!(lens.iter().all(|len| *len <= SEGMENT_LIMIT), "segments of {lens:?}");
    }
    store.commit(None).unwrap();
    check(&store, 0..97);
    let lasts: Vec<u64> = store.closed().unwrap().iter().map(Closed::last).collect();
    assert_eq!(lasts, [1, 2, 3]);
    let lens = segment_data(&dir);
    assert!(lens.iter().all(|len| *len <= SEGMENT_LIMIT), "segments of {lens:?}");
    drop(store);
    check(&Store::open(dir.clone(), SMALL_SEGMENTS).unwrap(), 0..97);
}

/// An extent written again and again between commits takes no segment past a segment and an
/// extent: its writes go to others, which go once the commit has sealed it (TASK-79.2). It reads
/// as last written, and once committed, in a file of its own, the open segment being full.
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
    assert_eq!(read(&store, extent(1), 0, 2 * BLOCK), pattern(63, 0, 2 * BLOCK));
    assert_eq!(store.place(extent(1)).unwrap(), Some(Place::Own));
    // The four it filled: three set aside, gone; the last closed by the commit.
    assert_eq!(dir.list(&of("open-segments", BRANCH)).unwrap(), ["0000000000000004"]);
    assert_eq!(dir.list(&of("segments", BRANCH)).unwrap(), ["0000000000000003"]);
    let store = reopened(store, &dir);
    assert_eq!(read(&store, extent(1), 0, 2 * BLOCK), pattern(63, 0, 2 * BLOCK));
}

/// Rewrites of an open extent take room in segments, which no extent holds once the extent is
/// sealed: past the capacity's worth, they count as used, until the commit sealing the extent,
/// then cleaning, takes them back (TASK-79.2). So a guest rewriting a block between commits fills
/// the host's disk with twice its capacity at most.
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
    // Sealed, the extent leaves them in a segment no extent holds but it, which cleaning removes.
    store.commit(None).unwrap();
    error_is(store.write(extent(2), 0, &pattern(2, 0, BLOCK)), NoSpace);
    store.clean(0.5).unwrap();
    assert_eq!(store.info().unwrap().available, capacity - BLOCK as u64);
    store.write(extent(2), 0, &pattern(2, 0, BLOCK)).unwrap();
    assert_eq!(read(&store, extent(1), 0, BLOCK), pattern(127, 0, BLOCK));
}

/// Writes, a commit, and frees of all but one of the extents written, and a commit: a segment
/// closed, which one extent pins.
fn churn(store: &Store<MemDir>, first: u64) {
    for sequence in first..first + 16 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
    }
    store.commit(None).unwrap();
    for sequence in first + 1..first + 16 {
        store.free(extent(sequence)).unwrap();
    }
    store.commit(None).unwrap();
}

/// What the branch's segments hold that no extent is in, freed, counts as used past the
/// capacity's worth, until cleaning takes it back (TASK-79.2): so that a guest freeing all but a
/// block of each segment it writes fills the host's disk with twice its capacity at most. What a
/// fork shares of it, the branch alone can't take back, nor counts.
#[test]
fn freed_space_past_the_capacity_counts_as_used() {
    let capacity = 64 * BLOCK as u64;
    let store = create(MemDir::new(), capacity);
    for cycle in 0..4 {
        churn(&store, 16 * cycle);
    }
    // 240 KiB freed: within the capacity's worth.
    assert_eq!(store.usage().unwrap().dead_bytes, 60 * BLOCK as u64);
    assert_eq!(store.info().unwrap().available, capacity - 4 * BLOCK as u64);
    churn(&store, 64);
    // 300 KiB freed, 44 KiB past it.
    assert_eq!(store.usage().unwrap().dead_bytes, 75 * BLOCK as u64);
    assert_eq!(store.info().unwrap().available, capacity - 5 * BLOCK as u64 - 11 * BLOCK as u64);
    error_is(store.write(extent(100), 0, &vec![1; 49 * BLOCK]), NoSpace);
    store.write(extent(100), 0, &vec![1; 48 * BLOCK]).unwrap();
    store.free(extent(100)).unwrap();
    store.commit(None).unwrap();
    // A fork holds the segments, and what is freed in them.
    store.fork(BRANCH + 1).unwrap();
    assert_eq!(store.usage().unwrap().dead_bytes, 0);
    assert_eq!(store.info().unwrap().available, capacity - 5 * BLOCK as u64);
    // Deleted, it holds them no more: cleaning finds them the branch's alone again.
    store.lineage().delete(BRANCH + 1).unwrap();
    store.clean(0.0).unwrap();
    assert_eq!(store.usage().unwrap().dead_bytes, 75 * BLOCK as u64);
}

/// fxdax: a segment a fork inherited and let go of goes, though a segment of its own numbered as
/// it is waits for its uploader: what the uploader keeps is the fork's own (TASK-85).
#[test]
fn inherited_segments_go_though_own_ones_numbered_so_are_kept() {
    let child = BRANCH + 1;
    let store = create(MemDir::new(), 1 << 20);
    // The parent's segment 0, holding extent 1.
    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    store.close_segment().unwrap();
    store.commit(None).unwrap();
    store.fork(child).unwrap();
    let forked = store.lineage().open_branch(child).unwrap();
    assert!(forked.upload(Chain::new(child, 1, 0, None, 0), 0).unwrap());
    // The fork's own segment 0, closed, which its uploader has yet to upload.
    forked.write(ExtentId::new(child, 0), 0, &pattern(2, 0, BLOCK)).unwrap();
    forked.close_segment().unwrap();
    forked.commit(None).unwrap();
    assert_eq!(forked.footprint().unwrap().segments, 2);
    // Freed, and its free released by the uploader, which keeps its data no more.
    forked.free(extent(1)).unwrap();
    forked.commit(None).unwrap();
    forked.release(forked.epoch().unwrap()).unwrap();
    forked.commit(None).unwrap();
    // Its own two, the one kept for its uploader and the open one.
    assert_eq!(forked.footprint().unwrap().segments, 2);
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
    let lineage = store.lineage().clone();
    drop(store);
    let store = lineage.open_branch(BRANCH).unwrap();
    assert_eq!(store.extents().unwrap(), [(extent(1), BLOCK as u64)]);
    let segments = dir.list(&of("segments", BRANCH)).unwrap();
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
    assert_eq!(dir.list(&of("open-extents", BRANCH)).unwrap(), Vec::<String>::new());
    assert_eq!(dir.list(&of("extents", BRANCH)).unwrap(), Vec::<String>::new());
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
    assert_eq!(dir.list(&of("open-extents", BRANCH)).unwrap(), Vec::<String>::new());
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
    assert_eq!(dir.list(&of("open-extents", BRANCH)).unwrap(), Vec::<String>::new());
    store.write(extent(1), 0, &pattern(1, 0, 40 << 10)).unwrap();
    store.commit(None).unwrap();
    assert!(read(&store, extent(1), 0, 40 << 10) == pattern(1, 0, 40 << 10), "other data");
    let store = reopened(store, &dir);
    assert!(read(&store, extent(1), 0, 40 << 10) == pattern(1, 0, 40 << 10), "other data");
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
    let lineage = store.lineage().clone();
    drop(store);
    let store = lineage.open_branch(BRANCH).unwrap();
    assert_eq!(dir.list(&of("open-extents", BRANCH)).unwrap(), Vec::<String>::new());
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
    let lineage = store.lineage().clone();
    drop(store);
    let store = lineage.open_branch(BRANCH).unwrap();
    assert_eq!(store.epoch().unwrap(), 1);
    store.commit(None).unwrap();
    assert_eq!(read(&store, extent(1), 0, 40 << 10), pattern(1, 0, 40 << 10));
}

/// fxdax: a store of eight closed segments, an extent in each, which keeps the files of
/// `open_files` of them open; and a fork of it, open.
fn eight_segments(open_files: usize) -> (MemDir, Store<MemDir>, Store<MemDir>) {
    let dir = MemDir::new();
    let options = Options { open_files, ..OPTIONS };
    let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: BRANCH };
    let store = Store::create(dir.clone(), description, options).unwrap();
    for sequence in 0..8 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
        store.close_segment().unwrap();
        store.commit(None).unwrap();
    }
    store.fork(BRANCH + 1).unwrap();
    let child = store.lineage().open_branch(BRANCH + 1).unwrap();
    (dir, store, child)
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

/// fxdax: a branch reading what it holds in more closed segments than it keeps the files of open,
/// inherited or its own, opens each of them once: as one more opens, the least lately read closes,
/// not all of them, which reads would open again; and opening one reads its table and trailer,
/// not its footer, a block, as each part's first read does (TASK-92.2).
#[test]
fn closed_segments_files_close_least_lately_read_first() {
    let (dir, store, child) = eight_segments(4);
    let expected = (8, 16 * BLOCK as u64);
    assert_eq!(opened_reading(&dir, &child), expected, "inherited");
    assert_eq!(opened_reading(&dir, &store), expected, "the branch's own");
}

/// A segment the lineage's forks read, removed once no branch holds it, is read no more from the
/// file it was: a fork of a branch whose ID is taken again reads its new segments, though numbered
/// as the old ones were (TASK-79.38).
#[test]
fn forks_of_branches_taken_again_read_their_new_segments() {
    let (child, grandchild) = (BRANCH + 1, BRANCH + 2);
    let store = create(MemDir::new(), 1 << 20);
    let lineage = store.lineage().clone();
    let segment_of = |data: &[u8]| {
        store.fork(child).unwrap();
        let forked = lineage.open_branch(child).unwrap();
        forked.write(ExtentId::new(child, 0), 0, data).unwrap();
        forked.close_segment().unwrap();
        forked.commit(None).unwrap();
        forked.fork(grandchild).unwrap();
        drop(forked);
        let read =
            read(&lineage.open_branch(grandchild).unwrap(), ExtentId::new(child, 0), 0, BLOCK);
        lineage.delete(grandchild).unwrap();
        lineage.delete(child).unwrap();
        read
    };
    assert_eq!(segment_of(&pattern(1, 0, BLOCK)), pattern(1, 0, BLOCK));
    assert!(segment_of(&pattern(2, 0, BLOCK)) == pattern(2, 0, BLOCK), "read the old segment");
}

/// A lineage this process keeps, its branches closed, while another process forks one of them,
/// opens a branch again knowing of the fork: its opening leaves the fork what it needs of its
/// parent, its index file as of the fork (TASK-79.5).
#[test]
fn lineages_kept_meanwhile_know_what_other_processes_did() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    store.commit(None).unwrap();
    let kept = store.lineage().clone();
    drop(store);
    // Another process's, which forks the branch.
    let other = Lineage::open(dir.clone(), OPTIONS).unwrap();
    other.open_branch(BRANCH).unwrap().fork(BRANCH + 1).unwrap();
    drop(other);

    let store = kept.open_branch(BRANCH).unwrap();
    assert_eq!(kept.branches(), [(BRANCH, None), (BRANCH + 1, Some(BRANCH))]);
    store.commit(None).unwrap();
    drop((store, kept));
    let fork = Lineage::open(dir, OPTIONS).unwrap().open_branch(BRANCH + 1).unwrap();
    assert_eq!(read(&fork, extent(1), 0, BLOCK), pattern(1, 0, BLOCK));
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

/// A fork names the branch it was forked from; a branch forked from none, none.
#[test]
fn forks_name_their_parent() {
    let store = create(MemDir::new(), 1 << 20);
    assert_eq!(store.parent(), None);
    store.fork(BRANCH + 1).unwrap();
    let child = store.lineage().open_branch(BRANCH + 1).unwrap();
    assert_eq!(child.parent(), Some(BRANCH));
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
        Some(Place::Run { branch: BRANCH, number: 0, offset: 0 })
    );
    assert_eq!(store.place(extent(2)).unwrap(), Some(Place::Pieces));
    // Not from its start.
    assert_eq!(store.place(extent(3)).unwrap(), Some(Place::Pieces));
    assert_eq!(store.place(extent(4)).unwrap(), Some(Place::Own));
    assert_eq!(store.place(extent(5)).unwrap(), None);
    assert_eq!(
        store.place(extent(6)).unwrap(),
        Some(Place::Run { branch: BRANCH, number: 0, offset: 5 * BLOCK as u64 })
    );
}

/// fxdax: a release lets the commits up to its epoch go, and not those after it.
#[test]
fn releases_let_their_commits_go() {
    let store = create(MemDir::new(), 1 << 20);
    assert!(store.upload(WRITER, 0).unwrap());
    store.write(extent(1), 0, &[1; BLOCK]).unwrap();
    store.commit(None).unwrap();
    store.write(extent(2), 0, &[2; 2 * BLOCK]).unwrap();
    store.commit(None).unwrap();
    store.write(extent(3), 0, &[3; 4 * BLOCK]).unwrap();
    store.commit(None).unwrap();
    assert_eq!(store.unreleased_bytes().unwrap(), 7 * BLOCK as u64);
    store.release(2).unwrap();
    assert_eq!(store.unreleased_bytes().unwrap(), 4 * BLOCK as u64);
}

/// fxdax: an uploader starting while commits wait to be made durable takes no segment of the
/// commits after it until they are.
#[test]
fn uploads_started_with_commits_pending_take_only_durable_ones() {
    let store = create(MemDir::new(), 1 << 20);
    store.write(extent(1), 0, &[1; BLOCK]).unwrap();
    store.commit_ordered(None).unwrap();
    store.commit_ordered(None).unwrap();
    assert!(store.upload(WRITER, store.epoch().unwrap()).unwrap());
    store.write(extent(2), 0, &[2; BLOCK]).unwrap();
    store.commit_ordered(None).unwrap();
    assert!(store.close_now().unwrap());
    assert!(store.closed().unwrap().is_empty());
    store.flush().unwrap();
    assert_eq!(closed_one(&store).last(), 3);
}

/// A closed segment's last commit is the commit that closed it.
#[test]
fn closed_segments_know_their_last_commit() {
    let store = create(MemDir::new(), 1 << 20);
    assert!(store.upload(WRITER, 0).unwrap());
    store.write(extent(1), 0, &[1; BLOCK]).unwrap();
    store.commit(None).unwrap();
    store.write(extent(2), 0, &[2; BLOCK]).unwrap();
    store.commit(None).unwrap();
    assert!(store.close_now().unwrap());
    let closed = store.closed().unwrap();
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
    assert_eq!(dir.list(&of("open-extents", BRANCH)).unwrap(), Vec::<String>::new());
}

/// Cleaning a fraction of 1 takes the segments with any dead data, not those wholly live.
#[test]
fn cleaning_takes_no_wholly_live_segment() {
    let store = create(MemDir::new(), 1 << 20);
    for sequence in 1..=4 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, 4 * BLOCK)).unwrap();
    }
    store.commit(None).unwrap();
    store.close_segment().unwrap();
    store.commit(None).unwrap();
    assert_eq!(store.clean(1.0).unwrap().segments, 0);
    store.free(extent(1)).unwrap();
    store.commit(None).unwrap();
    assert_eq!(store.clean(1.0).unwrap().segments, 1);
}

/// A commit lists an extent it wrote in two writes once, at its length: its segment's footer names
/// it so, and the bytes it wrote count it so.
#[test]
fn commits_list_each_extent_written_once() {
    let store = create(MemDir::new(), 1 << 20);
    assert!(store.upload(WRITER, 0).unwrap());
    store.write(extent(1), 0, &[1; BLOCK]).unwrap();
    store.write(extent(1), BLOCK as u64, &[1; BLOCK]).unwrap();
    store.commit(None).unwrap();
    assert_eq!(store.unreleased_bytes().unwrap(), 2 * BLOCK as u64);
    assert!(store.close_now().unwrap());
    let written = closed_one(&store).footer.extents;
    assert_eq!(
        written.iter().map(|&(extent, _, len)| (extent, len)).collect::<Vec<_>>(),
        [(extent(1), 2 * BLOCK as u64)]
    );
}

/// A fork's commit ends the segment that names the fork, and the fork gives its epoch.
#[test]
fn forks_give_their_commits_epoch() {
    let store = create(MemDir::new(), 1 << 20);
    assert!(store.upload(WRITER, 0).unwrap());
    store.write(extent(1), 0, &[1; BLOCK]).unwrap();
    store.commit(None).unwrap();
    assert_eq!(store.fork(BRANCH + 1).unwrap(), 2);
    assert_eq!(store.epoch().unwrap(), 2);
    let forking = closed_one(&store);
    assert_eq!((forking.last(), forking.footer.forks.clone()), (2, vec![BRANCH + 1]));
}

/// With segments closing as soon as they are old, an empty one closes not: a commit that wrote
/// nothing leaves no segment.
#[test]
fn empty_segments_close_not() {
    let options = Options { segment_age: std::time::Duration::ZERO, ..OPTIONS };
    let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: BRANCH };
    let store = Store::create(MemDir::new(), description, options).unwrap();
    assert!(store.upload(WRITER, 0).unwrap());
    store.commit(None).unwrap();
    store.commit(None).unwrap();
    assert_eq!(store.closed().unwrap().len(), 0);
    store.write(extent(1), 0, &[1; BLOCK]).unwrap();
    store.commit(None).unwrap();
    assert_eq!(store.closed().unwrap().len(), 1);
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
        (extent(sequence), MIB as u64, Place::Run { branch: BRANCH, number: 0, offset })
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
    let file = dir.open(&format!("{}/{:016x}", of("extents", BRANCH), 1), true).unwrap();
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

/// Checkpoints come as the index's deltas grow, not only as segments close.
#[test]
fn deltas_make_checkpoints() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    let index = dir.list(&of("index", BRANCH)).unwrap();
    for sequence in 1..=100 {
        store.write(extent(sequence), 0, &[1; BLOCK]).unwrap();
        store.commit(None).unwrap();
    }
    assert_ne!(dir.list(&of("index", BRANCH)).unwrap(), index);
}

/// A fork that frees what it inherited holds the rest, and so when opened again.
#[test]
fn forks_freeing_inherited_extents_hold_the_rest() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    store.write(extent(1), 0, &[1; 2 * BLOCK]).unwrap();
    store.write(extent(2), 0, &[2; 3 * BLOCK]).unwrap();
    store.commit(None).unwrap();
    store.fork(BRANCH + 1).unwrap();
    let lineage = store.lineage().clone();
    let child = lineage.open_branch(BRANCH + 1).unwrap();
    child.free(extent(1)).unwrap();
    child.commit(None).unwrap();
    assert_eq!(child.info().unwrap().used, 3 * BLOCK as u64);
    drop(child);
    let child = lineage.open_branch(BRANCH + 1).unwrap();
    assert_eq!(child.info().unwrap().used, 3 * BLOCK as u64);
}

/// A lineage knows which of its branches are open, and whether the store was deleted.
#[test]
fn lineages_know_their_open_branches_and_deletion() {
    let store = create(MemDir::new(), 1 << 20);
    store.fork(BRANCH + 1).unwrap();
    let lineage = store.lineage().clone();
    assert!(lineage.is_open(BRANCH));
    assert!(!lineage.is_open(BRANCH + 1));
    assert!(!lineage.is_deleted());
    drop(store);
    assert!(!lineage.is_open(BRANCH));
    lineage.delete(BRANCH).unwrap();
    assert!(lineage.is_deleted());
}

/// Forks of two points of a branch each hold their own point's files: one holds what was written
/// after the other's point, which the branch then freed.
#[test]
fn forks_of_different_points_hold_their_own_files() {
    let (first, second) = (BRANCH + 1, BRANCH + 2);
    let store = create(MemDir::new(), 1 << 20);
    store.write(extent(1), 0, &pattern(1, 0, 32 << 10)).unwrap();
    store.commit(None).unwrap();
    store.fork(first).unwrap();
    store.write(extent(2), 0, &pattern(2, 0, 32 << 10)).unwrap();
    store.commit(None).unwrap();
    store.fork(second).unwrap();
    let lineage = store.lineage().clone();
    let (first, second) =
        (lineage.open_branch(first).unwrap(), lineage.open_branch(second).unwrap());
    store.free(extent(1)).unwrap();
    store.free(extent(2)).unwrap();
    store.commit(None).unwrap();
    store.flush().unwrap();
    assert_eq!(read(&first, extent(1), 0, 32 << 10), pattern(1, 0, 32 << 10));
    assert_eq!(read(&second, extent(1), 0, 32 << 10), pattern(1, 0, 32 << 10));
    assert_eq!(read(&second, extent(2), 0, 32 << 10), pattern(2, 0, 32 << 10));
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

/// A record of the index damaged after it was written is not one: the store opens as of the
/// commit before it.
#[test]
fn damaged_index_records_are_none() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    store.commit(Some(b"one")).unwrap();
    store.commit(Some(b"two")).unwrap();
    drop(store);
    let index = of("index", BRANCH);
    let name = dir.list(&index).unwrap().pop().unwrap();
    let file = dir.open(&format!("{index}/{name}"), false).unwrap();
    let mut bytes = vec![0; file.size().unwrap() as usize];
    file.read_at(&mut bytes, 0).unwrap();
    // The last record's root, its last bytes: "two" as "twp".
    let at = bytes.windows(3).rposition(|window| window == b"two").unwrap();
    file.write_at(b"twp", at as u64).unwrap();
    let store = Store::open(dir, OPTIONS).unwrap();
    assert_eq!(store.root().unwrap(), b"one");
}

/// A commit that fails part way, which leaves the store's state ahead of its files, fails the store:
/// it does nothing more, though its file system works again. Here the file system fails the
/// commit's first change to it: a seal's copy of an extent written in two writes, its second
/// block first, which are not one run, the creation of the next open segment's file as the commit
/// closes the last, or that of the index file a checkpoint begins.
#[test]
fn commits_failing_part_way_fail_the_store() {
    let created = |options| {
        let dir = MemDir::new();
        let description =
            Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: BRANCH };
        let store = Store::create(dir.clone(), description, options).unwrap();
        (dir, store)
    };
    let sealing = || {
        let (dir, store) = created(OPTIONS);
        store.write(extent(1), BLOCK as u64, &[2; BLOCK]).unwrap();
        store.write(extent(1), 0, &[1; BLOCK]).unwrap();
        (dir, store)
    };
    let closing = || {
        let (dir, store) = created(OPTIONS);
        store.close_segment().unwrap();
        (dir, store)
    };
    let checkpointing = || created(Options { checkpoint_after: 0, ..OPTIONS });
    for (dir, store) in [sealing(), closing(), checkpointing()] {
        dir.fail_after(Some(0));
        let failed = store.commit_ordered(None).unwrap_err();
        assert_eq!(failed.to_string(), "committing the extent store");
        for budget in [Some(0), None] {
            dir.fail_after(budget);
            let failure = |result: Result<(), anyhow::Error>| result.unwrap_err().to_string();
            assert_eq!(failure(store.commit(None)), "the extent store failed");
            assert_eq!(
                failure(store.read(extent(1), 0, &mut [0; BLOCK])),
                "the extent store failed"
            );
            assert_eq!(failure(store.epoch().map(drop)), "the extent store failed");
        }
    }
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
/// branches it was forked from: in a base, a fork and a fork of it, as they write and free, and
/// opened again (TASK-49).
#[test]
fn stores_tell_their_own_bytes_from_those_inherited() {
    let (child, grandchild) = (BRANCH + 1, BRANCH + 2);
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    store.write(extent(1), 0, &[1; 2 * BLOCK]).unwrap();
    store.write(extent(2), 0, &[2; 3 * BLOCK]).unwrap();
    store.commit(None).unwrap();
    store.fork(child).unwrap();
    let lineage = store.lineage().clone();
    let forked = lineage.open_branch(child).unwrap();
    forked.write(ExtentId::new(child, 1), 0, &[3; BLOCK]).unwrap();
    forked.write(ExtentId::new(child, 2), 0, &[4; 40 << 10]).unwrap();
    forked.free(extent(1)).unwrap();
    forked.free(ExtentId::new(child, 2)).unwrap();
    forked.commit(None).unwrap();
    forked.fork(grandchild).unwrap();
    let grandforked = lineage.open_branch(grandchild).unwrap();
    grandforked.write(ExtentId::new(grandchild, 1), 0, &[5; 4 * BLOCK]).unwrap();
    grandforked.free(extent(2)).unwrap();
    grandforked.commit(None).unwrap();
    let bytes = |store: &Store<MemDir>| {
        let info = store.info().unwrap();
        (info.own, info.inherited)
    };
    let block = BLOCK as u64;
    let expected = [(5 * block, 0), (block, 3 * block), (4 * block, block)];
    assert_eq!([bytes(&store), bytes(&forked), bytes(&grandforked)], expected);
    drop((store, forked, grandforked, lineage));
    let lineage = Lineage::open(dir, OPTIONS).unwrap();
    let opened =
        [BRANCH, child, grandchild].map(|branch| bytes(&lineage.open_branch(branch).unwrap()));
    assert_eq!(opened, expected);
}
